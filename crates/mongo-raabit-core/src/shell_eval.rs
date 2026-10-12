//! Built-in shell for when `mongosh` is not installed: evaluates the common mongo shell
//! commands (`show`, `use`, `db.<coll>.find/aggregate/count/insert/update/delete…`,
//! `db.runCommand`) with the driver. Arguments use the shell literal syntax of
//! [`crate::shell`]; arbitrary JavaScript is not supported.

use crate::connection::Connection;
use crate::connection::NewIndex;
use crate::error::{RaabitError, Result};
use crate::shell;
use crate::write::{Write, WriteOutcome, is_read_command, pipeline_writes};
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};

/// Default number of documents a `find` prints, like the shell's batch.
pub const BATCH: i64 = 50;

/// One evaluated statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated {
    pub statement: String,
    pub result: std::result::Result<Bson, String>,
}

/// Methods offered by autocomplete after `db.<collection>.`.
pub const COLLECTION_METHODS: &[&str] = &[
    "find(",
    "findOne(",
    "aggregate([",
    "countDocuments(",
    "estimatedDocumentCount()",
    "distinct(",
    "insertOne(",
    "insertMany([",
    "updateOne(",
    "updateMany(",
    "replaceOne(",
    "deleteOne(",
    "deleteMany(",
    "getIndexes()",
    "createIndex(",
    "dropIndex(",
    "drop()",
    "stats()",
];

/// Methods offered after `db.`.
pub const DB_METHODS: &[&str] = &[
    "getCollection(",
    "getCollectionNames()",
    "runCommand(",
    "adminCommand(",
    "stats()",
    "getName()",
    "createCollection(",
    "dropDatabase()",
];

/// Cursor modifiers offered after `.find(…)`.
pub const CURSOR_METHODS: &[&str] = &[
    "sort(",
    "limit(",
    "skip(",
    "projection(",
    "count()",
    "toArray()",
];

/// Split a script into statements at top-level `;` and newlines (a line starting with
/// `.` continues the previous statement).
pub fn split_statements(script: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let chars: Vec<char> = script.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            current.push(c);
            if c == '\\' && i + 1 < chars.len() {
                current.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '"' | '\'' => {
                quote = Some(c);
                current.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(c);
            }
            ';' if depth <= 0 => {
                statements.push(std::mem::take(&mut current));
            }
            '\n' if depth <= 0 => {
                let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
                if next == Some(&'.') {
                    current.push(' ');
                } else {
                    statements.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
        i += 1;
    }
    statements.push(current);
    statements
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A parsed call chain such as `db.orders.find({a: 1}).limit(5)`.
#[derive(Debug, Clone, PartialEq)]
struct Call {
    name: String,
    args: Option<Vec<Bson>>,
}

fn invalid(message: impl Into<String>) -> RaabitError {
    RaabitError::Invalid(message.into())
}

/// Index of the bracket closing the one at `open`.
fn matching(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut quote: Option<u8> = None;
    let mut i = open;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 1;
            } else if c == q {
                quote = None;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = Some(c),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn parse_args(text: &str) -> Result<Vec<Bson>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    // Parsing as an array splits at top-level commas with full literal support.
    match shell::parse_value(&format!("[{text}]")) {
        Ok(Bson::Array(items)) => Ok(items),
        Ok(_) => Err(invalid("invalid arguments")),
        Err(e) => Err(RaabitError::parse("arguments", e)),
    }
}

/// A parsed statement: the database it names (`db.getSiblingDB("x")…`), the collection
/// (`db.orders`, `db["a.b"]`, `db.getCollection("x")`) and the calls that follow.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    db: Option<String>,
    collection: Option<String>,
    calls: Vec<Call>,
}

#[cfg(test)]
fn parse_chain(statement: &str) -> Result<(Option<String>, Vec<Call>)> {
    let target = parse_target(statement)?;
    Ok((target.collection, target.calls))
}

fn parse_target(statement: &str) -> Result<Target> {
    let s = statement.trim();
    let Some(mut rest) = s.strip_prefix("db") else {
        return Err(invalid(format!(
            "only `db…`, `show …` and `use …` statements are supported without mongosh (got `{}`)",
            s.split_whitespace().next().unwrap_or("")
        )));
    };
    let mut calls: Vec<Call> = Vec::new();
    let mut collection = None;
    // `db.getSiblingDB("x")` may be followed by a collection, like `db` itself.
    let after_database = |calls: &[Call], collection: &Option<String>| {
        collection.is_none()
            && (calls.is_empty() || (calls.len() == 1 && calls[0].name == "getSiblingDB"))
    };
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(after) = rest.strip_prefix('[') {
            if !after_database(&calls, &collection) {
                return Err(invalid("`[…]` must name a collection right after `db`"));
            }
            let end = matching(rest, 0).ok_or_else(|| invalid("unclosed ["))?;
            let name = shell::parse_value(&after[..end - 1])
                .map_err(|e| RaabitError::parse("collection name", e))?;
            collection = Some(
                name.as_str()
                    .ok_or_else(|| invalid("collection name must be a string"))?
                    .to_owned(),
            );
            rest = &rest[end + 1..];
            continue;
        }
        let Some(after) = rest.strip_prefix('.') else {
            return Err(invalid(format!("unexpected `{rest}`")));
        };
        let name_len = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .unwrap_or(after.len());
        let name = &after[..name_len];
        if name.is_empty() {
            return Err(invalid("expected a name after `.`"));
        }
        let after_name = after[name_len..].trim_start();
        if after_name.starts_with('(') {
            let open = rest.len() - after_name.len();
            let close =
                matching(rest, open).ok_or_else(|| invalid(format!("unclosed ( after {name}")))?;
            let args = parse_args(&rest[open + 1..close])?;
            calls.push(Call {
                name: name.to_owned(),
                args: Some(args),
            });
            rest = &rest[close + 1..];
        } else {
            if !after_database(&calls, &collection) {
                return Err(invalid(format!("`{name}` is not a method call")));
            }
            collection = Some(name.to_owned());
            rest = after_name;
        }
    }
    let mut db = None;
    if calls.first().is_some_and(|c| c.name == "getSiblingDB") {
        let first = calls.remove(0);
        let name = first
            .args
            .and_then(|a| a.into_iter().next())
            .and_then(|b| b.as_str().map(str::to_owned));
        db = Some(name.ok_or_else(|| invalid("getSiblingDB expects a database name"))?);
    }
    // `db.getCollection("x")` is the same as `db.x`.
    if collection.is_none() && calls.first().is_some_and(|c| c.name == "getCollection") {
        let first = calls.remove(0);
        let name = first
            .args
            .and_then(|a| a.into_iter().next())
            .and_then(|b| b.as_str().map(str::to_owned));
        collection = Some(name.ok_or_else(|| invalid("getCollection expects a name"))?);
    }
    Ok(Target {
        db,
        collection,
        calls,
    })
}

fn arg_doc(args: &[Bson], i: usize) -> Result<Document> {
    match args.get(i) {
        None | Some(Bson::Null) => Ok(Document::new()),
        Some(Bson::Document(d)) => Ok(d.clone()),
        Some(other) => Err(invalid(format!(
            "argument {} must be a document, found {}",
            i + 1,
            shell::type_name(other)
        ))),
    }
}

/// A document argument a write cannot do without (a filter, an update): `{}` must be
/// typed, a missing one is an error rather than "everything".
fn required_doc(method: &str, args: &[Bson], i: usize) -> Result<Document> {
    match args.get(i) {
        Some(Bson::Document(d)) => Ok(d.clone()),
        Some(other) => Err(invalid(format!(
            "{method}: argument {} must be a document, found {}",
            i + 1,
            shell::type_name(other)
        ))),
        None => Err(invalid(format!(
            "{method}: argument {} is missing (use {{}} for all documents)",
            i + 1
        ))),
    }
}

fn required_string(method: &str, args: &[Bson], i: usize) -> Result<String> {
    match args.get(i) {
        Some(Bson::String(s)) => Ok(s.clone()),
        _ => Err(invalid(format!(
            "{method}: argument {} must be a string",
            i + 1
        ))),
    }
}

/// An array of documents; any other element is an error (nothing is dropped silently).
fn document_array(method: &str, value: Option<&Bson>) -> Result<Vec<Document>> {
    match value {
        Some(Bson::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                item.as_document().cloned().ok_or_else(|| {
                    invalid(format!(
                        "{method}: element {i} must be a document, found {}",
                        shell::type_name(item)
                    ))
                })
            })
            .collect(),
        _ => Err(invalid(format!("{method} expects an array of documents"))),
    }
}

/// The arguments of a write call, at most `max` of them: options the built-in shell does
/// not apply are an error, never ignored.
fn write_args(call: &Call, max: usize) -> Result<&[Bson]> {
    let args = call.args.as_deref().unwrap_or_default();
    if args.len() > max {
        return Err(invalid(format!(
            "`{}` takes {max} argument{} in the built-in shell; options are not supported (install mongosh)",
            call.name,
            if max == 1 { "" } else { "s" }
        )));
    }
    Ok(args)
}

/// `createIndex(keys, options)`: the options the Index Manager knows.
fn new_index(args: &[Bson]) -> Result<NewIndex> {
    let keys = required_doc("createIndex", args, 0)?;
    let options = arg_doc(args, 1)?;
    let mut index = NewIndex {
        keys,
        ..NewIndex::default()
    };
    let flag = |name: &str, value: &Bson| match value {
        Bson::Boolean(b) => Ok(*b),
        _ => Err(invalid(format!(
            "createIndex: {name} must be true or false"
        ))),
    };
    for (name, value) in options {
        match name.as_str() {
            "name" => {
                index.name = Some(
                    value
                        .as_str()
                        .ok_or_else(|| invalid("createIndex: name must be a string"))?
                        .to_owned(),
                )
            }
            "unique" => index.unique = flag(&name, &value)?,
            "sparse" => index.sparse = flag(&name, &value)?,
            "hidden" => index.hidden = flag(&name, &value)?,
            "expireAfterSeconds" => {
                let seconds = match value {
                    Bson::Int32(v) => u64::try_from(v).ok(),
                    Bson::Int64(v) => u64::try_from(v).ok(),
                    _ => None,
                };
                index.ttl_seconds = Some(seconds.ok_or_else(|| {
                    invalid("createIndex: expireAfterSeconds must be a non-negative integer")
                })?);
            }
            "partialFilterExpression" => {
                index.partial_filter = Some(value.as_document().cloned().ok_or_else(|| {
                    invalid("createIndex: partialFilterExpression must be a document")
                })?)
            }
            other => {
                return Err(invalid(format!(
                    "createIndex: the option `{other}` is not supported by the built-in shell (install mongosh)"
                )));
            }
        }
    }
    Ok(index)
}

/// `db.adminCommand({ renameCollection: "a.b", to: "c.d", dropTarget })` as the
/// [`Write::RenameCollection`] it is; `None` for any other command.
fn rename_command(command: &Document) -> Option<Write> {
    if command
        .keys()
        .any(|k| !matches!(k.as_str(), "renameCollection" | "to" | "dropTarget"))
    {
        return None;
    }
    let (db, from) = command.get_str("renameCollection").ok()?.split_once('.')?;
    let (to_db, to) = command.get_str("to").ok()?.split_once('.')?;
    let drop_target = match command.get("dropTarget") {
        None => false,
        Some(Bson::Boolean(b)) => *b,
        Some(_) => return None,
    };
    Some(Write::RenameCollection {
        db: db.to_owned(),
        from: from.to_owned(),
        to_db: to_db.to_owned(),
        to: to.to_owned(),
        drop_target,
    })
}

/// Collection methods that only read.
const READ_METHODS: &[&str] = &[
    "find",
    "findOne",
    "countDocuments",
    "count",
    "estimatedDocumentCount",
    "distinct",
    "getIndexes",
    "stats",
];

/// Database methods that only read.
const DB_READ_METHODS: &[&str] = &["getCollectionNames", "getName", "stats"];

/// The [`Write`] a parsed statement runs on database `db`, without running it: `None`
/// for a statement that only reads, an error for one the built-in shell cannot run.
fn target_write(db: &str, target: &Target) -> Result<Option<Write>> {
    let write = call_write(db, target)?;
    if write.is_some()
        && let [call, next, ..] = target.calls.as_slice()
    {
        return Err(invalid(format!(
            "`.{}` cannot follow `{}` in the built-in shell",
            next.name, call.name
        )));
    }
    Ok(write)
}

fn call_write(db: &str, target: &Target) -> Result<Option<Write>> {
    let db = target.db.as_deref().unwrap_or(db).to_owned();
    let Some(call) = target.calls.first() else {
        return Ok(None);
    };
    let name = call.name.as_str();
    let Some(collection) = target.collection.clone() else {
        let write = match name {
            "runCommand" => {
                let command = required_doc(name, write_args(call, 1)?, 0)?;
                (!is_read_command(&command)).then_some(Write::RunCommand { db, command })
            }
            "adminCommand" => {
                let command = required_doc(name, write_args(call, 1)?, 0)?;
                rename_command(&command).or_else(|| {
                    (!is_read_command(&command)).then(|| Write::RunCommand {
                        db: "admin".into(),
                        command,
                    })
                })
            }
            "createCollection" => {
                let args = write_args(call, 2)?;
                Some(Write::CreateCollection {
                    db,
                    name: required_string(name, args, 0)?,
                    options: arg_doc(args, 1)?,
                })
            }
            "createView" => {
                let args = write_args(call, 3)?;
                Some(Write::CreateView {
                    db,
                    name: required_string(name, args, 0)?,
                    view_on: required_string(name, args, 1)?,
                    pipeline: document_array(name, args.get(2))?,
                })
            }
            "dropDatabase" => {
                write_args(call, 0)?;
                Some(Write::DropDatabase { db })
            }
            "dropUser" => Some(Write::DropUser {
                db,
                user: required_string(name, write_args(call, 1)?, 0)?,
            }),
            "killOp" => match write_args(call, 1)?.first() {
                Some(opid) => Some(Write::KillOp { opid: opid.clone() }),
                None => return Err(invalid("killOp expects an operation id")),
            },
            read if DB_READ_METHODS.contains(&read) => None,
            other => {
                return Err(invalid(format!(
                    "`db.{other}()` is not supported by the built-in shell; install mongosh for full JavaScript"
                )));
            }
        };
        return Ok(write);
    };
    let write = match name {
        "insertOne" => Write::InsertDocuments {
            db,
            collection,
            documents: vec![required_doc(name, write_args(call, 1)?, 0)?],
        },
        "insertMany" => Write::InsertDocuments {
            db,
            collection,
            documents: document_array(name, write_args(call, 1)?.first())?,
        },
        "updateOne" | "updateMany" => {
            let args = write_args(call, 2)?;
            let (filter, update) = (required_doc(name, args, 0)?, required_doc(name, args, 1)?);
            if name == "updateOne" {
                Write::UpdateOne {
                    db,
                    collection,
                    filter,
                    update,
                }
            } else {
                Write::UpdateMany {
                    db,
                    collection,
                    filter,
                    update,
                }
            }
        }
        "replaceOne" => {
            let args = write_args(call, 2)?;
            Write::ReplaceOne {
                db,
                collection,
                filter: required_doc(name, args, 0)?,
                replacement: required_doc(name, args, 1)?,
            }
        }
        "deleteOne" | "deleteMany" => {
            let filter = required_doc(name, write_args(call, 1)?, 0)?;
            if name == "deleteOne" {
                Write::DeleteOne {
                    db,
                    collection,
                    filter,
                }
            } else {
                Write::DeleteMany {
                    db,
                    collection,
                    filter,
                }
            }
        }
        "createIndex" => Write::CreateIndex {
            db,
            collection,
            index: new_index(write_args(call, 2)?)?,
        },
        "dropIndex" => Write::DropIndex {
            db,
            collection,
            name: required_string(name, write_args(call, 1)?, 0)?,
        },
        "hideIndex" | "unhideIndex" => Write::SetIndexHidden {
            db,
            collection,
            name: required_string(name, write_args(call, 1)?, 0)?,
            hidden: name == "hideIndex",
        },
        "drop" => {
            write_args(call, 0)?;
            Write::DropCollection { db, collection }
        }
        "renameCollection" => {
            let args = write_args(call, 2)?;
            let drop_target = match args.get(1) {
                None => false,
                Some(Bson::Boolean(b)) => *b,
                Some(_) => {
                    return Err(invalid(
                        "renameCollection: dropTarget must be true or false",
                    ));
                }
            };
            Write::RenameCollection {
                to_db: db.clone(),
                db,
                from: collection,
                to: required_string(name, args, 0)?,
                drop_target,
            }
        }
        "aggregate" => {
            let pipeline = document_array(name, call.args.as_deref().unwrap_or_default().first())?;
            if !pipeline_writes(&pipeline) {
                return Ok(None);
            }
            write_args(call, 1)?;
            Write::Aggregate {
                db,
                collection,
                pipeline,
            }
        }
        read if READ_METHODS.contains(&read) => return Ok(None),
        other => {
            return Err(invalid(format!(
                "`{other}` is not supported by the built-in shell; install mongosh for full JavaScript"
            )));
        }
    };
    Ok(Some(write))
}

/// The [`Write`] that `statement` runs on database `db`, without running it (what
/// [`Connection::eval_builtin`] executes for it): `None` for a statement that only reads,
/// an error for one the built-in shell cannot run. `use x` switches `db`.
///
/// This is how the shell text of a [`Write`] is checked: `statement_write(w.shell())`
/// gives back `w.canonical()` (see `docs/safety.md`).
pub fn statement_write(db: &mut String, statement: &str) -> Result<Option<Write>> {
    let words: Vec<&str> = statement.split_whitespace().collect();
    match words.as_slice() {
        ["show", ..] => return Ok(None),
        ["use", name] => {
            *db = name.trim_end_matches(';').to_owned();
            return Ok(None);
        }
        _ => {}
    }
    target_write(db, &parse_target(statement)?)
}

/// Every write a script would run, in order, without running anything; reads are left
/// out. Fails on the first statement the built-in shell cannot run.
pub fn script_plan(db: &str, script: &str) -> Result<Vec<Write>> {
    let mut db = db.to_owned();
    let mut writes = Vec::new();
    for statement in split_statements(script) {
        if let Some(write) = statement_write(&mut db, &statement)? {
            writes.push(write);
        }
    }
    Ok(writes)
}

fn arg_int(args: &[Bson], what: &str) -> Result<i64> {
    match args.first() {
        Some(Bson::Int32(v)) => Ok(i64::from(*v)),
        Some(Bson::Int64(v)) => Ok(*v),
        Some(Bson::Double(v)) if v.fract() == 0.0 => Ok(*v as i64),
        _ => Err(invalid(format!("{what} expects a number"))),
    }
}

/// The shell's reply to a write, as mongosh prints it.
fn outcome_value(outcome: WriteOutcome) -> Bson {
    match outcome {
        WriteOutcome::Inserted(ids) => match <[Bson; 1]>::try_from(ids) {
            Ok([id]) => Bson::Document(doc! {"acknowledged": true, "insertedId": id}),
            Err(ids) => Bson::Document(doc! {"acknowledged": true, "insertedIds": ids}),
        },
        WriteOutcome::Updated { matched, modified } => {
            Bson::Document(doc! {"matchedCount": matched as i64, "modifiedCount": modified as i64})
        }
        WriteOutcome::Deleted(n) => Bson::Document(doc! {"deletedCount": n as i64}),
        WriteOutcome::IndexCreated(name) => Bson::String(name),
        WriteOutcome::Aggregated { documents, .. } => docs(documents),
        WriteOutcome::Command(reply) => Bson::Document(reply),
        WriteOutcome::Imported(_) | WriteOutcome::Done => Bson::Document(doc! {"ok": 1}),
    }
}

/// The reply of write method `method` on `db`, shaped like mongosh's.
fn write_reply(method: &str, db: &str, outcome: WriteOutcome) -> Bson {
    match (method, outcome) {
        ("insertMany", WriteOutcome::Inserted(ids)) => {
            Bson::Document(doc! {"acknowledged": true, "insertedIds": ids})
        }
        ("drop" | "renameCollection", _) => Bson::Boolean(true),
        ("dropDatabase", _) => Bson::Document(doc! {"ok": 1, "dropped": db}),
        (
            "dropIndex" | "hideIndex" | "unhideIndex" | "createCollection" | "createView"
            | "dropUser" | "killOp",
            _,
        ) => Bson::Document(doc! {"ok": 1}),
        (_, outcome) => outcome_value(outcome),
    }
}

fn docs(list: Vec<Document>) -> Bson {
    Bson::Array(list.into_iter().map(Bson::Document).collect())
}

/// Most documents a `find` or `aggregate` of the built-in shell returns: `limit(0)` and
/// larger limits are capped, so a script never loads a whole collection.
pub const MAX_SHELL_DOCUMENTS: i64 = 1000;

impl Connection {
    /// Evaluate a script statement by statement. `db` is the current database and is
    /// updated by `use`.
    pub async fn eval_builtin(&self, db: &mut String, script: &str) -> Vec<Evaluated> {
        let mut out = Vec::new();
        for statement in split_statements(script) {
            let result = self
                .eval_statement(db, &statement)
                .await
                .map_err(|e| e.to_string());
            let failed = result.is_err();
            out.push(Evaluated { statement, result });
            if failed {
                break;
            }
        }
        out
    }

    async fn eval_statement(&self, db: &mut String, statement: &str) -> Result<Bson> {
        let words: Vec<&str> = statement.split_whitespace().collect();
        match words.as_slice() {
            ["show", "dbs" | "databases"] => {
                let list = self.databases().await?;
                return Ok(docs(
                    list.into_iter()
                        .map(|d| doc! {"name": d.name, "sizeOnDisk": d.size_on_disk as i64})
                        .collect(),
                ));
            }
            ["show", "collections" | "tables"] => {
                let list = self.collections(db).await?;
                return Ok(Bson::Array(
                    list.into_iter().map(|c| Bson::String(c.name)).collect(),
                ));
            }
            ["use", name] => {
                *db = name.trim_end_matches(';').to_owned();
                return Ok(Bson::String(format!("switched to db {db}")));
            }
            _ => {}
        }
        let target = parse_target(statement)?;
        let current = target.db.clone().unwrap_or_else(|| db.clone());
        // A write runs exactly as `statement_write` reads it, through `execute`.
        if let Some(write) = target_write(&current, &target)? {
            let method = target
                .calls
                .first()
                .map(|c| c.name.clone())
                .unwrap_or_default();
            return Ok(write_reply(&method, &current, self.execute(write).await?));
        }
        let Some(collection) = target.collection else {
            return self.eval_db_call(&current, &target.calls).await;
        };
        let calls = target.calls;
        let max_time = self.query_timeout();
        let handle = self.collection(&current, &collection);
        let Some((first, modifiers)) = calls.split_first() else {
            return Err(invalid(format!(
                "`db.{collection}` needs a method, e.g. db.{collection}.find()"
            )));
        };
        let args = first.args.clone().unwrap_or_default();
        match first.name.as_str() {
            "find" => {
                let mut find = handle.find(arg_doc(&args, 0)?).max_time(max_time);
                let projection = arg_doc(&args, 1)?;
                if !projection.is_empty() {
                    find = find.projection(projection);
                }
                let mut limit = BATCH;
                let mut count = false;
                for call in modifiers {
                    let margs = call.args.clone().unwrap_or_default();
                    match call.name.as_str() {
                        "sort" => find = find.sort(arg_doc(&margs, 0)?),
                        "limit" => limit = arg_int(&margs, "limit")?,
                        "skip" => {
                            find = find.skip(u64::try_from(arg_int(&margs, "skip")?).unwrap_or(0))
                        }
                        "projection" => find = find.projection(arg_doc(&margs, 0)?),
                        "count" => count = true,
                        "toArray" | "pretty" => {}
                        other => {
                            return Err(invalid(format!(
                                "cursor method `{other}` is not supported"
                            )));
                        }
                    }
                }
                if count {
                    let n = handle
                        .count_documents(arg_doc(&args, 0)?)
                        .max_time(max_time)
                        .await?;
                    return Ok(Bson::Int64(n as i64));
                }
                // `limit(0)` (no limit) and huge limits load at most this many.
                let limit = match limit.unsigned_abs() {
                    0 => MAX_SHELL_DOCUMENTS,
                    n => i64::try_from(n)
                        .unwrap_or(i64::MAX)
                        .min(MAX_SHELL_DOCUMENTS),
                };
                let cursor = find.limit(limit).await?;
                Ok(docs(cursor.try_collect().await?))
            }
            "findOne" => Ok(handle
                .find_one(arg_doc(&args, 0)?)
                .max_time(max_time)
                .await?
                .map_or(Bson::Null, Bson::Document)),
            "aggregate" => {
                let stages = document_array("aggregate", args.first())?;
                let mut cursor = handle
                    .aggregate(stages)
                    .allow_disk_use(true)
                    .max_time(max_time)
                    .await?;
                // Read only what is shown; dropping the cursor ends it on the server.
                let mut list = Vec::new();
                while (list.len() as i64) < BATCH * 4 {
                    match cursor.try_next().await? {
                        Some(document) => list.push(document),
                        None => break,
                    }
                }
                Ok(docs(list))
            }
            "countDocuments" | "count" => Ok(Bson::Int64(
                handle
                    .count_documents(arg_doc(&args, 0)?)
                    .max_time(max_time)
                    .await? as i64,
            )),
            "estimatedDocumentCount" => Ok(Bson::Int64(
                handle.estimated_document_count().max_time(max_time).await? as i64,
            )),
            "distinct" => {
                let field = args
                    .first()
                    .and_then(Bson::as_str)
                    .ok_or_else(|| invalid("distinct expects a field name"))?;
                Ok(Bson::Array(
                    handle
                        .distinct(field, arg_doc(&args, 1)?)
                        .max_time(max_time)
                        .await?,
                ))
            }
            "getIndexes" => Ok(docs(
                self.indexes(&current, &collection)
                    .await?
                    .into_iter()
                    .map(|i| i.raw)
                    .collect(),
            )),
            "stats" => {
                let stats = self.collection_stats(&current, &collection).await?;
                Ok(Bson::Document(doc! {
                    "count": stats.count as i64, "size": stats.size as i64, "avgObjSize": stats.avg_obj_size as i64,
                    "storageSize": stats.storage_size as i64, "nindexes": stats.index_count as i64,
                    "totalIndexSize": stats.total_index_size as i64,
                }))
            }
            other => Err(invalid(format!(
                "`{other}` is not supported by the built-in shell; install mongosh for full JavaScript"
            ))),
        }
    }

    /// The database calls that read (writes went through `target_write`).
    async fn eval_db_call(&self, db: &str, calls: &[Call]) -> Result<Bson> {
        let Some(call) = calls.first() else {
            return Ok(Bson::String(db.to_owned()));
        };
        let args = call.args.clone().unwrap_or_default();
        match call.name.as_str() {
            "runCommand" => Ok(Bson::Document(
                self.run_command(db, arg_doc(&args, 0)?).await?,
            )),
            "adminCommand" => Ok(Bson::Document(
                self.run_command("admin", arg_doc(&args, 0)?).await?,
            )),
            "getCollectionNames" => Ok(Bson::Array(
                self.collections(db)
                    .await?
                    .into_iter()
                    .map(|c| Bson::String(c.name))
                    .collect(),
            )),
            "getName" => Ok(Bson::String(db.to_owned())),
            "stats" => Ok(Bson::Document(
                self.run_command(db, doc! {"dbStats": 1}).await?,
            )),
            other => Err(invalid(format!(
                "`db.{other}()` is not supported by the built-in shell; install mongosh for full JavaScript"
            ))),
        }
    }
}

/// Shell methods that change data or structure. Scripts calling any of them are
/// confirmed before they run.
pub const WRITE_METHODS: &[&str] = &[
    "insert",
    "insertOne",
    "insertMany",
    "update",
    "updateOne",
    "updateMany",
    "replaceOne",
    "findOneAndUpdate",
    "findOneAndReplace",
    "findOneAndDelete",
    "findAndModify",
    "bulkWrite",
    "save",
    "remove",
    "deleteOne",
    "deleteMany",
    "drop",
    "dropDatabase",
    "dropIndex",
    "dropIndexes",
    "createIndex",
    "createIndexes",
    "hideIndex",
    "unhideIndex",
    "createCollection",
    "createView",
    "renameCollection",
    "createUser",
    "updateUser",
    "dropUser",
    "dropAllUsers",
    "grantRolesToUser",
    "revokeRolesFromUser",
    "createRole",
    "dropRole",
    "killOp",
    "runCommand",
    "adminCommand",
];

/// Of [`WRITE_METHODS`], those that remove or overwrite data.
const DESTRUCTIVE_METHODS: &[&str] = &[
    "replaceOne",
    "findOneAndReplace",
    "findOneAndDelete",
    "remove",
    "deleteOne",
    "deleteMany",
    "drop",
    "dropDatabase",
    "dropIndex",
    "dropIndexes",
    "renameCollection",
    "dropUser",
    "dropAllUsers",
    "dropRole",
    "killOp",
];

/// What a script may change: the write methods it calls (`name(`), plus `$out` and
/// `$merge` stages, in order of appearance. Text in strings and comments counts too: the
/// check errs on the side of asking.
pub fn script_writes(script: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let bytes = script.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let ident_start = c == b'$' || c == b'_' || c.is_ascii_alphabetic();
        if !ident_start || (i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
        {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        let word = &script[start..i];
        let called = script[i..].trim_start().starts_with('(');
        let hit = (called && WRITE_METHODS.contains(&word)) || matches!(word, "$out" | "$merge");
        if hit && !found.iter().any(|f| f == word) {
            found.push(word.to_owned());
        }
    }
    found
}

/// True when [`script_writes`] found a method that removes or overwrites data.
pub fn script_is_destructive(script: &str) -> bool {
    script_writes(script)
        .iter()
        .any(|w| DESTRUCTIVE_METHODS.contains(&w.as_str()) || w == "$out")
}

/// Completions for the identifier chain ending at the cursor.
pub fn complete(prefix: &str, collections: &[String]) -> Vec<String> {
    // After a call (`….find({}).li`) offer cursor modifiers.
    let partial_len = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .count();
    let (head, partial) = prefix.split_at(prefix.len() - partial_len);
    if head.ends_with(").") && head.contains("db.") {
        return CURSOR_METHODS
            .iter()
            .filter(|m| m.to_lowercase().starts_with(&partial.to_lowercase()))
            .map(|m| m.to_string())
            .collect();
    }
    let token_start = prefix
        .rfind(|c: char| c.is_whitespace() || matches!(c, '(' | ',' | ';' | '='))
        .map_or(0, |i| i + 1);
    let token = &prefix[token_start..];
    let Some(rest) = token.strip_prefix("db.") else {
        return ["db.", "show dbs", "show collections", "use "]
            .iter()
            .filter(|c| c.starts_with(token) && !token.is_empty())
            .map(|c| c.to_string())
            .collect();
    };
    let filter = |items: &mut dyn Iterator<Item = String>, partial: &str| -> Vec<String> {
        items
            .filter(|c| c.to_lowercase().starts_with(&partial.to_lowercase()))
            .take(12)
            .collect()
    };
    match rest.split_once('.') {
        None => filter(
            &mut collections
                .iter()
                .cloned()
                .chain(DB_METHODS.iter().map(|m| m.to_string())),
            rest,
        ),
        Some((_, method)) if !method.contains('(') => filter(
            &mut COLLECTION_METHODS.iter().map(|m| m.to_string()),
            method,
        ),
        Some((_, chain)) => {
            let last = chain.rsplit_once(").").map(|(_, m)| m).unwrap_or("");
            if chain.contains(").") {
                filter(&mut CURSOR_METHODS.iter().map(|m| m.to_string()), last)
            } else {
                Vec::new()
            }
        }
    }
}

/// What to offer at a caret in a script: the suggestions and the char index where the
/// partial word they replace starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionSite {
    pub start: usize,
    pub items: Vec<String>,
}

/// Completions for the script up to `caret` (a char index), with the word they replace.
/// `None` when nothing completes there.
pub fn completions_at(
    script: &str,
    caret: usize,
    collections: &[String],
) -> Option<CompletionSite> {
    let byte = script
        .char_indices()
        .nth(caret)
        .map_or(script.len(), |(i, _)| i);
    let prefix = &script[..byte];
    let items = complete(prefix, collections);
    if items.is_empty() {
        return None;
    }
    let partial = if prefix.ends_with('.') {
        0
    } else {
        prefix
            .chars()
            .rev()
            .take_while(|c| {
                c.is_alphanumeric()
                    || *c == '_'
                    || *c == ' ' && prefix.trim_start().starts_with("sh")
            })
            .count()
            .min(prefix.chars().count())
    };
    Some(CompletionSite {
        start: caret.min(prefix.chars().count()) - partial.min(caret),
        items,
    })
}

/// The console text of a mongosh run: stdout, then stderr on its own lines.
pub fn mongosh_console(stdout: &str, stderr: &str) -> String {
    let mut output = stdout.trim_end().to_owned();
    if !stderr.trim().is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(stderr.trim_end());
    }
    output
}

/// The console text of built-in evaluations, whether all succeeded, and the last value.
pub fn builtin_console(results: &[Evaluated]) -> (String, bool, Option<Bson>) {
    let mut output = String::new();
    let mut ok = true;
    let mut value = None;
    for evaluated in results {
        match &evaluated.result {
            Ok(v) => {
                output.push_str(&shell::to_shell_string(v, shell::ShellStyle::Pretty(2)));
                value = Some(v.clone());
            }
            Err(err) => {
                ok = false;
                output.push_str(&format!("Error: {err}"));
            }
        }
        output.push('\n');
    }
    (output.trim_end().to_owned(), ok, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_statements() {
        let s = split_statements(
            "use shop\ndb.orders.find({a: ';'})\n  .limit(2); // comment\nshow dbs",
        );
        assert_eq!(
            s,
            [
                "use shop",
                "db.orders.find({a: ';'})   .limit(2)",
                "show dbs"
            ]
        );
        assert_eq!(
            split_statements("db.x.insertOne({\n a: 1\n})"),
            ["db.x.insertOne({\n a: 1\n})"]
        );
    }

    #[test]
    fn parses_call_chains() {
        let (coll, calls) =
            parse_chain("db.orders.find({status: 'x'}, {_id: 0}).sort({a: -1}).limit(5)").unwrap();
        assert_eq!(coll.as_deref(), Some("orders"));
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].args.as_ref().unwrap().len(), 2);
        let (coll, _) = parse_chain("db.getCollection('my coll').countDocuments()").unwrap();
        assert_eq!(coll.as_deref(), Some("my coll"));
        let (coll, _) = parse_chain("db['a.b'].find()").unwrap();
        assert_eq!(coll.as_deref(), Some("a.b"));
        let (coll, calls) = parse_chain("db.runCommand({ping: 1})").unwrap();
        assert!(coll.is_none());
        assert_eq!(calls[0].name, "runCommand");
        assert!(parse_chain("print(1)").is_err());
        assert!(parse_chain("db.orders.find({a: }").is_err());
    }

    fn write_of(statement: &str) -> Result<Option<Write>> {
        statement_write(&mut "shop".to_owned(), statement)
    }

    #[test]
    fn statements_map_to_the_write_they_run() {
        assert_eq!(
            write_of("db.orders.updateMany({ a: 1 }, { $set: { b: 2 } })").unwrap(),
            Some(Write::UpdateMany {
                db: "shop".into(),
                collection: "orders".into(),
                filter: doc! {"a": 1},
                update: doc! {"$set": {"b": 2}},
            })
        );
        assert_eq!(
            write_of("db.getSiblingDB('archive').getCollection('a.b').deleteOne({ _id: 1 })")
                .unwrap(),
            Some(Write::DeleteOne {
                db: "archive".into(),
                collection: "a.b".into(),
                filter: doc! {"_id": 1},
            })
        );
        assert_eq!(
            write_of("db.getSiblingDB('archive').logs.drop()").unwrap(),
            Some(Write::DropCollection {
                db: "archive".into(),
                collection: "logs".into(),
            })
        );
        assert_eq!(
            write_of("db.adminCommand({ renameCollection: 'shop.a.b', to: 'old.c' })").unwrap(),
            Some(Write::RenameCollection {
                db: "shop".into(),
                from: "a.b".into(),
                to_db: "old".into(),
                to: "c".into(),
                drop_target: false,
            })
        );
        assert_eq!(
            write_of("db.orders.createIndex({ a: 1 }, { unique: true, name: 'u' })").unwrap(),
            Some(Write::CreateIndex {
                db: "shop".into(),
                collection: "orders".into(),
                index: NewIndex {
                    keys: doc! {"a": 1},
                    name: Some("u".into()),
                    unique: true,
                    ..NewIndex::default()
                },
            })
        );
        assert_eq!(
            write_of("db.killOp(12)").unwrap(),
            Some(Write::KillOp {
                opid: Bson::Int32(12)
            })
        );
        // Reads are not writes.
        for read in [
            "db.orders.find({}).limit(5)",
            "db.orders.aggregate([{ $match: {} }]).toArray()",
            "db.runCommand({ ping: 1 })",
            "db.adminCommand({ listDatabases: 1 })",
            "db.getCollectionNames()",
            "show dbs",
        ] {
            assert_eq!(write_of(read).unwrap(), None, "{read}");
        }
        assert!(matches!(
            write_of("db.orders.aggregate([{ $out: 'x' }])").unwrap(),
            Some(Write::Aggregate { .. })
        ));
        assert!(matches!(
            write_of("db.runCommand({ compact: 'orders' })").unwrap(),
            Some(Write::RunCommand { .. })
        ));
    }

    #[test]
    fn writes_the_shell_cannot_run_as_typed_are_refused() {
        for (statement, why) in [
            ("db.orders.deleteMany()", "argument 1 is missing"),
            ("db.orders.updateOne({ a: 1 })", "argument 2 is missing"),
            ("db.orders.deleteMany(null)", "must be a document"),
            ("db.orders.insertOne()", "argument 1 is missing"),
            (
                "db.orders.insertMany([{ a: 1 }, 2])",
                "element 1 must be a document",
            ),
            (
                "db.orders.updateOne({}, { $set: { a: 1 } }, { upsert: true })",
                "options are not supported",
            ),
            (
                "db.orders.insertMany([{}], { ordered: false })",
                "options are not supported",
            ),
            (
                "db.orders.createIndex({ a: 1 }, { collation: { locale: 'fr' } })",
                "collation",
            ),
            ("db.orders.deleteMany({}).limit(1)", "cannot follow"),
            ("db.orders.findOneAndDelete({})", "not supported"),
            (
                "db.createUser({ user: 'x', pwd: 'y', roles: [] })",
                "not supported",
            ),
        ] {
            let err = write_of(statement).unwrap_err().to_string();
            assert!(err.contains(why), "{statement}: {err}");
        }
    }

    #[test]
    fn a_script_plan_lists_its_writes_in_order() {
        let plan = script_plan(
            "shop",
            "db.a.find({})\nuse archive\ndb.b.insertOne({ x: 1 }); db.getSiblingDB('shop').c.drop()",
        )
        .unwrap();
        assert_eq!(
            plan,
            [
                Write::InsertDocuments {
                    db: "archive".into(),
                    collection: "b".into(),
                    documents: vec![doc! {"x": 1}],
                },
                Write::DropCollection {
                    db: "shop".into(),
                    collection: "c".into(),
                },
            ]
        );
        assert!(script_plan("shop", "db.a.find({})\nprint(1)").is_err());
    }

    #[test]
    fn completes_names_and_methods() {
        let colls = vec!["orders".to_string(), "outbox".to_string()];
        assert_eq!(complete("db.o", &colls), ["orders", "outbox"]);
        assert!(complete("db.orders.fi", &colls).contains(&"find(".to_string()));
        assert!(complete("db.orders.find({}).li", &colls).contains(&"limit(".to_string()));
        assert_eq!(complete("sh", &colls), ["show dbs", "show collections"]);
    }

    #[test]
    fn finds_the_writes_of_a_script() {
        assert!(script_writes("db.orders.find({}).limit(20)\nshow dbs").is_empty());
        assert!(script_writes("db.orders.countDocuments({ dropped: 1 })").is_empty());
        assert_eq!(
            script_writes("db.a.insertOne({x: 1})\ndb.a.deleteMany({})\ndb.a.insertOne({})"),
            ["insertOne", "deleteMany"]
        );
        assert_eq!(
            script_writes("db.a.aggregate([{$match: {}}, {$out: 'b'}])"),
            ["$out"]
        );
        assert_eq!(script_writes("db.runCommand({ping: 1})"), ["runCommand"]);
        assert!(!script_is_destructive("db.a.updateOne({}, {$set: {a: 1}})"));
        assert!(script_is_destructive("db.a.drop ()"));
    }

    #[test]
    fn completion_sites_replace_the_partial_word() {
        let collections = vec!["orders".to_owned(), "products".to_owned()];
        let script = "db.pro";
        let site = completions_at(script, script.chars().count(), &collections).unwrap();
        assert_eq!(site.start, 3);
        assert_eq!(site.items, ["products"]);
        // After a dot nothing is replaced.
        let site = completions_at("db.", 3, &collections).unwrap();
        assert_eq!(site.start, 3);
        assert!(site.items.contains(&"orders".to_owned()));
        // Shell commands replace what was typed of them.
        let site = completions_at("sh", 2, &collections).unwrap();
        assert_eq!(site.items, ["show dbs", "show collections"]);
        assert_eq!(site.start, 0);
        // The caret decides, not the end of the script.
        let site = completions_at("db.pro\nprint(1)", 6, &collections).unwrap();
        assert_eq!(site.start, 3);
        assert_eq!(completions_at("x = 5", 5, &collections), None);
    }

    #[test]
    fn console_text_of_runs() {
        assert_eq!(mongosh_console("out\n\n", ""), "out");
        assert_eq!(mongosh_console("out\n", "warn\n"), "out\nwarn");
        assert_eq!(mongosh_console("", "warn"), "warn");
        let results = [
            Evaluated {
                statement: "1".into(),
                result: Ok(Bson::Int32(1)),
            },
            Evaluated {
                statement: "x".into(),
                result: Err("boom".into()),
            },
        ];
        let (output, ok, value) = builtin_console(&results);
        assert_eq!(output, "1\nError: boom");
        assert!(!ok);
        assert_eq!(value, Some(Bson::Int32(1)));
        assert_eq!(builtin_console(&[]), (String::new(), true, None));
    }
}
