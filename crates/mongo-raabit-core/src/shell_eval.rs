//! Built-in shell for when `mongosh` is not installed: evaluates the common mongo shell
//! commands (`show`, `use`, `db.<coll>.find/aggregate/count/insert/update/delete…`,
//! `db.runCommand`) with the driver. Arguments use the shell literal syntax of
//! [`crate::shell`]; arbitrary JavaScript is not supported.

use crate::connection::Connection;
use crate::error::{RaabitError, Result};
use crate::shell;
use crate::write::{Write, WriteOutcome};
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

fn parse_chain(statement: &str) -> Result<(Option<String>, Vec<Call>)> {
    let s = statement.trim();
    let Some(mut rest) = s.strip_prefix("db") else {
        return Err(invalid(format!(
            "only `db…`, `show …` and `use …` statements are supported without mongosh (got `{}`)",
            s.split_whitespace().next().unwrap_or("")
        )));
    };
    let mut calls = Vec::new();
    let mut collection = None;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(after) = rest.strip_prefix('[') {
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
            if !calls.is_empty() || collection.is_some() {
                return Err(invalid(format!("`{name}` is not a method call")));
            }
            collection = Some(name.to_owned());
            rest = after_name;
        }
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
    Ok((collection, calls))
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

fn docs(list: Vec<Document>) -> Bson {
    Bson::Array(list.into_iter().map(Bson::Document).collect())
}

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
        let (collection, calls) = parse_chain(statement)?;
        let Some(collection) = collection else {
            return self.eval_db_call(db, &calls).await;
        };
        let handle = self.collection(db, &collection);
        let Some((first, modifiers)) = calls.split_first() else {
            return Err(invalid(format!(
                "`db.{collection}` needs a method, e.g. db.{collection}.find()"
            )));
        };
        let args = first.args.clone().unwrap_or_default();
        match first.name.as_str() {
            "find" => {
                let mut find = handle.find(arg_doc(&args, 0)?);
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
                    let n = handle.count_documents(arg_doc(&args, 0)?).await?;
                    return Ok(Bson::Int64(n as i64));
                }
                let cursor = find.limit(limit).await?;
                Ok(docs(cursor.try_collect().await?))
            }
            "findOne" => Ok(handle
                .find_one(arg_doc(&args, 0)?)
                .await?
                .map_or(Bson::Null, Bson::Document)),
            "aggregate" => {
                let stages = match args.first() {
                    Some(Bson::Array(items)) => items
                        .iter()
                        .map(|s| {
                            s.as_document()
                                .cloned()
                                .ok_or_else(|| invalid("each stage must be a document"))
                        })
                        .collect::<Result<Vec<_>>>()?,
                    _ => return Err(invalid("aggregate expects an array of stages")),
                };
                if crate::write::pipeline_writes(&stages) {
                    let write = Write::Aggregate {
                        db: db.clone(),
                        collection: collection.clone(),
                        pipeline: stages,
                    };
                    return Ok(outcome_value(self.execute(write).await?));
                }
                let cursor = handle.aggregate(stages).allow_disk_use(true).await?;
                let mut list: Vec<Document> = cursor.try_collect().await?;
                list.truncate(BATCH as usize * 4);
                Ok(docs(list))
            }
            "countDocuments" | "count" => Ok(Bson::Int64(
                handle.count_documents(arg_doc(&args, 0)?).await? as i64,
            )),
            "estimatedDocumentCount" => {
                Ok(Bson::Int64(handle.estimated_document_count().await? as i64))
            }
            "distinct" => {
                let field = args
                    .first()
                    .and_then(Bson::as_str)
                    .ok_or_else(|| invalid("distinct expects a field name"))?;
                Ok(Bson::Array(
                    handle.distinct(field, arg_doc(&args, 1)?).await?,
                ))
            }
            "insertOne" => {
                let write = Write::InsertDocuments {
                    db: db.clone(),
                    collection: collection.clone(),
                    documents: vec![arg_doc(&args, 0)?],
                };
                Ok(outcome_value(self.execute(write).await?))
            }
            "insertMany" => {
                let documents = match args.first() {
                    Some(Bson::Array(items)) => items
                        .iter()
                        .filter_map(|d| d.as_document().cloned())
                        .collect(),
                    _ => return Err(invalid("insertMany expects an array of documents")),
                };
                let write = Write::InsertDocuments {
                    db: db.clone(),
                    collection: collection.clone(),
                    documents,
                };
                match self.execute(write).await? {
                    WriteOutcome::Inserted(ids) => Ok(Bson::Document(
                        doc! {"acknowledged": true, "insertedIds": ids},
                    )),
                    other => Ok(outcome_value(other)),
                }
            }
            "updateOne" | "updateMany" => {
                let (db, collection) = (db.clone(), collection.clone());
                let (filter, update) = (arg_doc(&args, 0)?, arg_doc(&args, 1)?);
                let write = if first.name == "updateOne" {
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
                };
                Ok(outcome_value(self.execute(write).await?))
            }
            "replaceOne" => {
                let write = Write::ReplaceOne {
                    db: db.clone(),
                    collection: collection.clone(),
                    filter: arg_doc(&args, 0)?,
                    replacement: arg_doc(&args, 1)?,
                };
                Ok(outcome_value(self.execute(write).await?))
            }
            "deleteOne" | "deleteMany" => {
                let (db, collection) = (db.clone(), collection.clone());
                let filter = arg_doc(&args, 0)?;
                let write = if first.name == "deleteOne" {
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
                };
                Ok(outcome_value(self.execute(write).await?))
            }
            "getIndexes" => Ok(docs(
                self.indexes(db, &collection)
                    .await?
                    .into_iter()
                    .map(|i| i.raw)
                    .collect(),
            )),
            "createIndex" => {
                let write = Write::CreateIndex {
                    db: db.clone(),
                    collection: collection.clone(),
                    index: crate::connection::NewIndex {
                        keys: arg_doc(&args, 0)?,
                        ..Default::default()
                    },
                };
                Ok(outcome_value(self.execute(write).await?))
            }
            "dropIndex" => {
                let name = args
                    .first()
                    .and_then(Bson::as_str)
                    .ok_or_else(|| invalid("dropIndex expects an index name"))?;
                let write = Write::DropIndex {
                    db: db.clone(),
                    collection: collection.clone(),
                    name: name.to_owned(),
                };
                self.execute(write).await?;
                Ok(Bson::Document(doc! {"ok": 1}))
            }
            "drop" => {
                let write = Write::DropCollection {
                    db: db.clone(),
                    collection: collection.clone(),
                };
                self.execute(write).await?;
                Ok(Bson::Boolean(true))
            }
            "stats" => {
                let stats = self.collection_stats(db, &collection).await?;
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
            "createCollection" => {
                let name = args
                    .first()
                    .and_then(Bson::as_str)
                    .ok_or_else(|| invalid("createCollection expects a name"))?;
                let write = Write::CreateCollection {
                    db: db.to_owned(),
                    name: name.to_owned(),
                    options: arg_doc(&args, 1)?,
                };
                self.execute(write).await?;
                Ok(Bson::Document(doc! {"ok": 1}))
            }
            "dropDatabase" => {
                self.execute(Write::DropDatabase { db: db.to_owned() })
                    .await?;
                Ok(Bson::Document(doc! {"ok": 1, "dropped": db}))
            }
            other => Err(invalid(format!(
                "`db.{other}()` is not supported by the built-in shell; install mongosh for full JavaScript"
            ))),
        }
    }
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

    #[test]
    fn completes_names_and_methods() {
        let colls = vec!["orders".to_string(), "outbox".to_string()];
        assert_eq!(complete("db.o", &colls), ["orders", "outbox"]);
        assert!(complete("db.orders.fi", &colls).contains(&"find(".to_string()));
        assert!(complete("db.orders.find({}).li", &colls).contains(&"limit(".to_string()));
        assert_eq!(complete("sh", &colls), ["show dbs", "show collections"]);
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
