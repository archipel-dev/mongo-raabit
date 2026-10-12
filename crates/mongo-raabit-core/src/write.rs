//! Every change Mongo Raabit can make to a database, as data.
//!
//! A [`Write`] says exactly what will run: the views show it ([`Write::shell`],
//! [`Write::title`], [`Connection::preview`]) before [`Connection::execute`] runs it.
//! `execute` is the only way to change data or structure: the write methods of the
//! connection are private to this crate, and a read-only connection refuses every
//! `Write` with [`crate::RaabitError::ReadOnly`] before anything reaches the server.

use crate::connection::{Connection, NewIndex, RoleRef};
use crate::error::{RaabitError, Result};
use crate::format;
use crate::shell::{self, ShellStyle};
use crate::transfer::{ImportMode, ImportReport};
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, DateTime, Document, doc};
use std::future::IntoFuture;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// How long a preview may count before it gives up ([`Affected::Unknown`]).
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(4);

/// Longest call printed on one line by [`Write::shell`] (what a confirmation shows
/// without scrolling sideways); longer calls print each argument indented.
const COMPACT_WIDTH: usize = 88;
/// Documents of an insert or import printed in full by [`Write::shell`].
const SHOWN_DOCUMENTS: usize = 3;

/// One change to the database, with the namespace and the exact arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum Write {
    /// `insertOne` / `insertMany`; documents without `_id` get a fresh `ObjectId`.
    InsertDocuments {
        db: String,
        collection: String,
        documents: Vec<Document>,
    },
    /// Insert a copy of `document` under a fresh `_id`.
    CloneDocument {
        db: String,
        collection: String,
        document: Document,
    },
    /// The document editor's Save: replace the document with this `_id`, only if it is
    /// still exactly `original` (the document as it was loaded; see [`replace_filter`]).
    ReplaceDocument {
        db: String,
        collection: String,
        id: Bson,
        /// The whole document as it was loaded into the editor.
        original: Document,
        replacement: Document,
    },
    /// An inline edit: `$set` (or with `None`, `$unset`) one field of one document, only
    /// if the field still has the value that was loaded (see [`set_field_filter`]).
    SetField {
        db: String,
        collection: String,
        id: Bson,
        path: String,
        value: Option<Bson>,
        /// The field's value as it was loaded; `None` when the field was missing.
        original: Option<Bson>,
    },
    UpdateOne {
        db: String,
        collection: String,
        filter: Document,
        update: Document,
    },
    UpdateMany {
        db: String,
        collection: String,
        filter: Document,
        update: Document,
    },
    ReplaceOne {
        db: String,
        collection: String,
        filter: Document,
        replacement: Document,
    },
    /// Delete documents by `_id` (`deleteOne` for one, `deleteMany` with `$in` for more).
    DeleteDocuments {
        db: String,
        collection: String,
        ids: Vec<Bson>,
    },
    DeleteOne {
        db: String,
        collection: String,
        filter: Document,
    },
    DeleteMany {
        db: String,
        collection: String,
        filter: Document,
    },
    /// MongoDB creates databases lazily: this creates their first collection.
    CreateDatabase {
        db: String,
        first_collection: String,
    },
    DropDatabase {
        db: String,
    },
    /// `options` is the `create` command body (capped, validator…).
    CreateCollection {
        db: String,
        name: String,
        options: Document,
    },
    CreateView {
        db: String,
        name: String,
        view_on: String,
        pipeline: Vec<Document>,
    },
    DropCollection {
        db: String,
        collection: String,
    },
    RenameCollection {
        db: String,
        from: String,
        to_db: String,
        to: String,
        drop_target: bool,
    },
    CreateIndex {
        db: String,
        collection: String,
        index: NewIndex,
    },
    DropIndex {
        db: String,
        collection: String,
        name: String,
    },
    SetIndexHidden {
        db: String,
        collection: String,
        name: String,
        hidden: bool,
    },
    CreateUser {
        db: String,
        user: String,
        password: String,
        roles: Vec<RoleRef>,
    },
    DropUser {
        db: String,
        user: String,
    },
    KillOp {
        opid: Bson,
    },
    /// Import documents (insert, upsert by `_id`, or drop the collection first).
    Import {
        db: String,
        collection: String,
        documents: Vec<Document>,
        mode: ImportMode,
    },
    /// An aggregation whose last stage is `$out` or `$merge`.
    Aggregate {
        db: String,
        collection: String,
        pipeline: Vec<Document>,
    },
    /// A database command that is not known to only read.
    RunCommand {
        db: String,
        command: Document,
    },
}

/// What [`Connection::execute`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteOutcome {
    /// The `_id`s of the inserted documents, in order.
    Inserted(Vec<Bson>),
    Updated {
        matched: u64,
        modified: u64,
    },
    Deleted(u64),
    IndexCreated(String),
    Imported(ImportReport),
    /// The documents an aggregation returned (none for `$out` / `$merge`).
    Aggregated {
        documents: Vec<Document>,
        elapsed: Duration,
    },
    Command(Document),
    Done,
}

/// What a write will touch, counted before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Affected {
    /// Documents matched by the filter (updated, replaced or deleted).
    Documents(u64),
    /// Documents that will be inserted.
    Inserted(u64),
    /// The collection that is dropped or overwritten, and how many documents it holds.
    Collection { name: String, documents: u64 },
    /// The database that is dropped.
    Database { collections: u64, documents: u64 },
    /// Changes structure (indexes, users, commands), not documents.
    NotCounted,
    /// Counting failed or took too long.
    Unknown,
}

/// What the confirmation shows: the command and what it affects.
#[derive(Debug, Clone, PartialEq)]
pub struct WritePreview {
    pub shell: String,
    pub affected: Affected,
}

fn quoted(text: &str) -> String {
    shell::to_shell_string(&Bson::String(text.to_owned()), ShellStyle::Compact)
}

/// `db.getCollection("orders")`.
fn coll(collection: &str) -> String {
    format!("db.getCollection({})", quoted(collection))
}

/// `db.getSiblingDB("shop")`.
fn sibling(db: &str) -> String {
    format!("db.getSiblingDB({})", quoted(db))
}

/// Arguments of a call: on one line when the call (`prefix` and arguments) is short,
/// else each one pretty-printed.
fn args(prefix: usize, values: &[Bson]) -> String {
    let compact: Vec<String> = values
        .iter()
        .map(|v| shell::to_shell_string(v, ShellStyle::Compact))
        .collect();
    let line = compact.join(", ");
    if prefix + line.chars().count() < COMPACT_WIDTH && !line.contains('\n') {
        return line;
    }
    values
        .iter()
        .map(|v| shell::to_shell_string(v, ShellStyle::Pretty(2)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn call(target: &str, method: &str, values: &[Bson]) -> String {
    let prefix = target.chars().count() + method.chars().count() + 3;
    format!("{target}.{method}({})", args(prefix, values))
}

/// A list of documents, the first few in full, the rest as a comment.
fn document_list(documents: &[Document]) -> String {
    if documents.len() <= SHOWN_DOCUMENTS {
        let all: Vec<Bson> = documents.iter().cloned().map(Bson::Document).collect();
        return shell::to_shell_string(&Bson::Array(all), ShellStyle::Pretty(2));
    }
    let mut out = String::from("[");
    for document in &documents[..SHOWN_DOCUMENTS] {
        out.push_str("\n  ");
        let text = shell::document_to_shell(document, ShellStyle::Pretty(2));
        out.push_str(&text.replace('\n', "\n  "));
        out.push(',');
    }
    out.push_str(&format!(
        "\n  // … and {} more documents\n]",
        format::group((documents.len() - SHOWN_DOCUMENTS) as u64)
    ));
    out
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", format::group(n))
    }
}

fn documents(n: u64) -> String {
    plural(n, "document", "documents")
}

/// The name of a command document (its first key).
fn command_name(command: &Document) -> &str {
    command.keys().next().map_or("", String::as_str)
}

/// True when the pipeline writes (`$out` or `$merge`, which must be its last stage, but
/// any position counts).
pub fn pipeline_writes(pipeline: &[Document]) -> bool {
    pipeline
        .iter()
        .any(|stage| stage.contains_key("$out") || stage.contains_key("$merge"))
}

/// Commands a read-only connection may run: they only read data or server state.
pub const READ_COMMANDS: &[&str] = &[
    "aggregate",
    "buildInfo",
    "buildinfo",
    "collStats",
    "connectionStatus",
    "count",
    "currentOp",
    "dataSize",
    "dbHash",
    "dbStats",
    "distinct",
    "explain",
    "features",
    "find",
    "getCmdLineOpts",
    "getLog",
    "getMore",
    "getParameter",
    "hello",
    "hostInfo",
    "isMaster",
    "ismaster",
    "listCollections",
    "listCommands",
    "listDatabases",
    "listIndexes",
    "ping",
    "replSetGetStatus",
    "rolesInfo",
    "serverStatus",
    "top",
    "usersInfo",
    "whatsmyuri",
];

/// True when `command` only reads: its name is in [`READ_COMMANDS`] and, for
/// `aggregate` (also inside `explain`), its pipeline has no `$out` or `$merge`.
pub fn is_read_command(command: &Document) -> bool {
    let name = command_name(command);
    if !READ_COMMANDS.contains(&name) {
        return false;
    }
    let pipeline_reads = |cmd: &Document| match cmd.get_array("pipeline") {
        Ok(stages) => {
            let stages: Vec<Document> = stages
                .iter()
                .filter_map(|s| s.as_document().cloned())
                .collect();
            !pipeline_writes(&stages)
        }
        Err(_) => true,
    };
    match name {
        "aggregate" => pipeline_reads(command),
        // Explain never applies writes, but an explained `$out` pipeline still runs
        // with `executionStats` on some servers: treat it as a write.
        "explain" => command
            .get_document("explain")
            .map(|inner| command_name(inner) != "aggregate" || pipeline_reads(inner))
            .unwrap_or(true),
        _ => true,
    }
}

/// Where an aggregation's `$out` / `$merge` writes: `(db, collection)`.
fn output_target(db: &str, pipeline: &[Document]) -> Option<(bool, String, String)> {
    let last = pipeline.last()?;
    let (is_out, spec) = match (last.get("$out"), last.get("$merge")) {
        (Some(spec), _) => (true, spec),
        (None, Some(Bson::Document(merge))) => (false, merge.get("into")?),
        (None, Some(spec)) => (false, spec),
        _ => return None,
    };
    match spec {
        Bson::String(name) => Some((is_out, db.to_owned(), name.clone())),
        Bson::Document(target) => Some((
            is_out,
            target.get_str("db").unwrap_or(db).to_owned(),
            target.get_str("coll").ok()?.to_owned(),
        )),
        _ => None,
    }
}

fn index_options(index: &NewIndex) -> Document {
    let mut options = Document::new();
    if let Some(name) = index.name.as_ref().filter(|n| !n.trim().is_empty()) {
        options.insert("name", name.clone());
    }
    if index.unique {
        options.insert("unique", true);
    }
    if index.sparse {
        options.insert("sparse", true);
    }
    if index.hidden {
        options.insert("hidden", true);
    }
    if let Some(ttl) = index.ttl_seconds {
        let ttl = i32::try_from(ttl)
            .map(Bson::Int32)
            .unwrap_or_else(|_| Bson::Int64(i64::try_from(ttl).unwrap_or(i64::MAX)));
        options.insert("expireAfterSeconds", ttl);
    }
    if let Some(filter) = &index.partial_filter {
        options.insert("partialFilterExpression", filter.clone());
    }
    options
}

/// The message of [`crate::RaabitError::Conflict`]: a single-document write found the
/// document changed or gone, and wrote nothing.
pub const CONFLICT: &str =
    "The document changed (or was deleted) since it was loaded. Reload and try again.";

/// The filter of an inline edit ([`Write::SetField`]): the document's `_id`, and the field
/// at `path` still holding exactly `original`, the value that was loaded (`None`: the
/// field was missing, so it must still be missing).
///
/// A value is matched with `$eq` (a document is a value, not operators; a regular
/// expression is a value, not a pattern), its BSON type with `$type` (`1` that became
/// `1.0` or `"1"` changed; `null` does not match a missing field), and `$not: { $type:
/// "array" }` (an array that now merely contains the value changed). An original that is
/// itself an array is matched with `$eq` and `$type: "array"`.
pub fn set_field_filter(id: &Bson, path: &str, original: Option<&Bson>) -> Document {
    let condition = match original {
        None => doc! {"$exists": false},
        Some(value @ Bson::Array(_)) => doc! {"$eq": value.clone(), "$type": "array"},
        Some(value) => doc! {
            "$eq": value.clone(),
            "$type": type_alias(value),
            "$not": {"$type": "array"},
        },
    };
    let mut filter = doc! {"_id": id.clone()};
    filter.insert(path, condition);
    filter
}

/// The `$type` alias of a value's BSON type.
fn type_alias(value: &Bson) -> &'static str {
    match value {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Document(_) => "object",
        Bson::Array(_) => "array",
        Bson::Binary(_) => "binData",
        Bson::Undefined => "undefined",
        Bson::ObjectId(_) => "objectId",
        Bson::Boolean(_) => "bool",
        Bson::DateTime(_) => "date",
        Bson::Null => "null",
        Bson::RegularExpression(_) => "regex",
        Bson::DbPointer(_) => "dbPointer",
        Bson::JavaScriptCode(_) => "javascript",
        Bson::Symbol(_) => "symbol",
        Bson::JavaScriptCodeWithScope(_) => "javascriptWithScope",
        Bson::Int32(_) => "int",
        Bson::Timestamp(_) => "timestamp",
        Bson::Int64(_) => "long",
        Bson::Decimal128(_) => "decimal",
        Bson::MinKey => "minKey",
        Bson::MaxKey => "maxKey",
    }
}

/// The filter of the document editor's Save ([`Write::ReplaceDocument`]): the `_id`, and
/// the whole stored document still equal to `original`, the document that was loaded.
///
/// Comparing the whole document (`$$ROOT`) is the simplest check that is complete: it
/// also notices fields that someone else added or removed, which a filter on the loaded
/// top-level fields would miss (replacing would silently drop them). `$literal` keeps the
/// server from reading strings that start with `$` as expressions. `$eq` compares numbers
/// by value, so the stored size (`$bsonSize`) must match too: a number stored with another
/// type of a different width (`1` that became `1.0`) is a change.
pub fn replace_filter(id: &Bson, original: &Document) -> Document {
    let size = mongodb::bson::to_vec(original).map_or(0, |bytes| bytes.len());
    doc! {
        "_id": id.clone(),
        "$expr": {"$and": [
            {"$eq": ["$$ROOT", {"$literal": original.clone()}]},
            {"$eq": [{"$bsonSize": "$$ROOT"}, i32::try_from(size).unwrap_or(i32::MAX)]},
        ]},
    }
}

/// The update of an inline edit: `$set` of the value, or `$unset` without one.
fn set_field_update(path: &str, value: Option<&Bson>) -> Document {
    match value {
        Some(value) => doc! {"$set": {path: value.clone()}},
        None => doc! {"$unset": {path: ""}},
    }
}

/// Every variant of [`Write`], by name, in declaration order (what [`Write::kind`]
/// returns). The contract tests check that each one has a sample, a live test, a
/// read-only test and a round-trip test.
pub const KINDS: &[&str] = &[
    "InsertDocuments",
    "CloneDocument",
    "ReplaceDocument",
    "SetField",
    "UpdateOne",
    "UpdateMany",
    "ReplaceOne",
    "DeleteDocuments",
    "DeleteOne",
    "DeleteMany",
    "CreateDatabase",
    "DropDatabase",
    "CreateCollection",
    "CreateView",
    "DropCollection",
    "RenameCollection",
    "CreateIndex",
    "DropIndex",
    "SetIndexHidden",
    "CreateUser",
    "DropUser",
    "KillOp",
    "Import",
    "Aggregate",
    "RunCommand",
];

impl Write {
    /// The variant's name (one of [`KINDS`]).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InsertDocuments { .. } => "InsertDocuments",
            Self::CloneDocument { .. } => "CloneDocument",
            Self::ReplaceDocument { .. } => "ReplaceDocument",
            Self::SetField { .. } => "SetField",
            Self::UpdateOne { .. } => "UpdateOne",
            Self::UpdateMany { .. } => "UpdateMany",
            Self::ReplaceOne { .. } => "ReplaceOne",
            Self::DeleteDocuments { .. } => "DeleteDocuments",
            Self::DeleteOne { .. } => "DeleteOne",
            Self::DeleteMany { .. } => "DeleteMany",
            Self::CreateDatabase { .. } => "CreateDatabase",
            Self::DropDatabase { .. } => "DropDatabase",
            Self::CreateCollection { .. } => "CreateCollection",
            Self::CreateView { .. } => "CreateView",
            Self::DropCollection { .. } => "DropCollection",
            Self::RenameCollection { .. } => "RenameCollection",
            Self::CreateIndex { .. } => "CreateIndex",
            Self::DropIndex { .. } => "DropIndex",
            Self::SetIndexHidden { .. } => "SetIndexHidden",
            Self::CreateUser { .. } => "CreateUser",
            Self::DropUser { .. } => "DropUser",
            Self::KillOp { .. } => "KillOp",
            Self::Import { .. } => "Import",
            Self::Aggregate { .. } => "Aggregate",
            Self::RunCommand { .. } => "RunCommand",
        }
    }

    /// The write as the plain mongosh statement it runs, which is what [`Write::shell`]
    /// prints and what the built-in shell reads back
    /// ([`crate::shell_eval::statement_write`]). The variants the views build for one
    /// purpose become the general call that runs on the server: an inline edit is an
    /// `updateOne` with its guard filter, a Save a `replaceOne` with its guard filter, a
    /// delete of selected documents a `deleteOne`/`deleteMany` by `_id`, a clone an
    /// `insertOne` without `_id`, and creating a database the `createCollection` of its
    /// first collection. Every other write is returned as it is.
    pub fn canonical(&self) -> Write {
        match self.clone() {
            Self::CloneDocument {
                db,
                collection,
                mut document,
            } => {
                document.remove("_id");
                Self::InsertDocuments {
                    db,
                    collection,
                    documents: vec![document],
                }
            }
            Self::ReplaceDocument {
                db,
                collection,
                id,
                original,
                replacement,
            } => Self::ReplaceOne {
                db,
                collection,
                filter: replace_filter(&id, &original),
                replacement,
            },
            Self::SetField {
                db,
                collection,
                id,
                path,
                value,
                original,
            } => Self::UpdateOne {
                db,
                collection,
                filter: set_field_filter(&id, &path, original.as_ref()),
                update: set_field_update(&path, value.as_ref()),
            },
            Self::DeleteDocuments {
                db,
                collection,
                ids,
            } => match <[Bson; 1]>::try_from(ids) {
                Ok([id]) => Self::DeleteOne {
                    db,
                    collection,
                    filter: doc! {"_id": id},
                },
                Err(ids) => Self::DeleteMany {
                    db,
                    collection,
                    filter: doc! {"_id": {"$in": ids}},
                },
            },
            Self::CreateDatabase {
                db,
                first_collection,
            } => Self::CreateCollection {
                db,
                name: first_collection,
                options: Document::new(),
            },
            Self::CreateIndex {
                db,
                collection,
                mut index,
            } => {
                index.name = index.name.filter(|n| !n.trim().is_empty());
                index.ttl_seconds = index.ttl_seconds.map(|t| t.min(i64::MAX as u64));
                Self::CreateIndex {
                    db,
                    collection,
                    index,
                }
            }
            other => other,
        }
    }

    /// The database the write changes.
    pub fn db(&self) -> &str {
        match self {
            Self::InsertDocuments { db, .. }
            | Self::CloneDocument { db, .. }
            | Self::ReplaceDocument { db, .. }
            | Self::SetField { db, .. }
            | Self::UpdateOne { db, .. }
            | Self::UpdateMany { db, .. }
            | Self::ReplaceOne { db, .. }
            | Self::DeleteDocuments { db, .. }
            | Self::DeleteOne { db, .. }
            | Self::DeleteMany { db, .. }
            | Self::CreateDatabase { db, .. }
            | Self::DropDatabase { db }
            | Self::CreateCollection { db, .. }
            | Self::CreateView { db, .. }
            | Self::DropCollection { db, .. }
            | Self::RenameCollection { db, .. }
            | Self::CreateIndex { db, .. }
            | Self::DropIndex { db, .. }
            | Self::SetIndexHidden { db, .. }
            | Self::CreateUser { db, .. }
            | Self::DropUser { db, .. }
            | Self::Import { db, .. }
            | Self::Aggregate { db, .. }
            | Self::RunCommand { db, .. } => db,
            Self::KillOp { .. } => "admin",
        }
    }

    /// The collection the write changes, when it changes one.
    pub fn collection(&self) -> Option<&str> {
        match self {
            Self::InsertDocuments { collection, .. }
            | Self::CloneDocument { collection, .. }
            | Self::ReplaceDocument { collection, .. }
            | Self::SetField { collection, .. }
            | Self::UpdateOne { collection, .. }
            | Self::UpdateMany { collection, .. }
            | Self::ReplaceOne { collection, .. }
            | Self::DeleteDocuments { collection, .. }
            | Self::DeleteOne { collection, .. }
            | Self::DeleteMany { collection, .. }
            | Self::DropCollection { collection, .. }
            | Self::CreateIndex { collection, .. }
            | Self::DropIndex { collection, .. }
            | Self::SetIndexHidden { collection, .. }
            | Self::Import { collection, .. }
            | Self::Aggregate { collection, .. } => Some(collection),
            Self::CreateCollection { name, .. } | Self::CreateView { name, .. } => Some(name),
            Self::RenameCollection { from, .. } => Some(from),
            Self::CreateDatabase {
                first_collection, ..
            } => Some(first_collection),
            _ => None,
        }
    }

    fn namespace(&self) -> String {
        match self.collection() {
            Some(collection) => format!("{}.{collection}", self.db()),
            None => self.db().to_owned(),
        }
    }

    /// The equivalent mongosh command, as the confirmation shows it. Collection writes are
    /// relative to the write's database (`db.getCollection(…)`), database-level ones name
    /// it (`db.getSiblingDB(…)`). Passwords are never printed (`passwordPrompt()`).
    pub fn shell(&self) -> String {
        match self {
            // Printed as the general call they run (see `canonical`), so the text shown is
            // the text the shell reads back.
            Self::CloneDocument { .. }
            | Self::ReplaceDocument { .. }
            | Self::SetField { .. }
            | Self::DeleteDocuments { .. }
            | Self::CreateDatabase { .. } => self.canonical().shell(),
            Self::InsertDocuments {
                collection,
                documents,
                ..
            } => match documents.as_slice() {
                [one] => call(
                    &coll(collection),
                    "insertOne",
                    &[Bson::Document(one.clone())],
                ),
                many => format!("{}.insertMany({})", coll(collection), document_list(many)),
            },
            Self::UpdateOne {
                collection,
                filter,
                update,
                ..
            } => call(
                &coll(collection),
                "updateOne",
                &[
                    Bson::Document(filter.clone()),
                    Bson::Document(update.clone()),
                ],
            ),
            Self::UpdateMany {
                collection,
                filter,
                update,
                ..
            } => call(
                &coll(collection),
                "updateMany",
                &[
                    Bson::Document(filter.clone()),
                    Bson::Document(update.clone()),
                ],
            ),
            Self::ReplaceOne {
                collection,
                filter,
                replacement,
                ..
            } => call(
                &coll(collection),
                "replaceOne",
                &[
                    Bson::Document(filter.clone()),
                    Bson::Document(replacement.clone()),
                ],
            ),
            Self::DeleteOne {
                collection, filter, ..
            } => call(
                &coll(collection),
                "deleteOne",
                &[Bson::Document(filter.clone())],
            ),
            Self::DeleteMany {
                collection, filter, ..
            } => call(
                &coll(collection),
                "deleteMany",
                &[Bson::Document(filter.clone())],
            ),
            Self::DropDatabase { db } => format!("{}.dropDatabase()", sibling(db)),
            Self::CreateCollection { db, name, options } => {
                let mut values = vec![Bson::String(name.clone())];
                if !options.is_empty() {
                    values.push(Bson::Document(options.clone()));
                }
                call(&sibling(db), "createCollection", &values)
            }
            Self::CreateView {
                db,
                name,
                view_on,
                pipeline,
            } => call(
                &sibling(db),
                "createView",
                &[
                    Bson::String(name.clone()),
                    Bson::String(view_on.clone()),
                    Bson::Array(pipeline.iter().cloned().map(Bson::Document).collect()),
                ],
            ),
            Self::DropCollection { collection, .. } => format!("{}.drop()", coll(collection)),
            Self::RenameCollection {
                db,
                from,
                to_db,
                to,
                drop_target,
            } => {
                if db == to_db {
                    call(
                        &coll(from),
                        "renameCollection",
                        &[Bson::String(to.clone()), Bson::Boolean(*drop_target)],
                    )
                } else {
                    call(
                        "db",
                        "adminCommand",
                        &[Bson::Document(doc! {
                            "renameCollection": format!("{db}.{from}"),
                            "to": format!("{to_db}.{to}"),
                            "dropTarget": *drop_target,
                        })],
                    )
                }
            }
            Self::CreateIndex {
                collection, index, ..
            } => {
                let mut values = vec![Bson::Document(index.keys.clone())];
                let options = index_options(index);
                if !options.is_empty() {
                    values.push(Bson::Document(options));
                }
                call(&coll(collection), "createIndex", &values)
            }
            Self::DropIndex {
                collection, name, ..
            } => call(
                &coll(collection),
                "dropIndex",
                &[Bson::String(name.clone())],
            ),
            Self::SetIndexHidden {
                collection,
                name,
                hidden,
                ..
            } => call(
                &coll(collection),
                if *hidden { "hideIndex" } else { "unhideIndex" },
                &[Bson::String(name.clone())],
            ),
            Self::CreateUser {
                db, user, roles, ..
            } => {
                let roles: Vec<Bson> = roles
                    .iter()
                    .map(|r| Bson::Document(doc! {"role": &r.role, "db": &r.db}))
                    .collect();
                let spec = shell::document_to_shell(
                    &doc! {"user": user.as_str(), "pwd": "\u{1}", "roles": roles},
                    ShellStyle::Compact,
                );
                format!(
                    "{}.createUser({})",
                    sibling(db),
                    spec.replace("\"\\u0001\"", "passwordPrompt()")
                )
            }
            Self::DropUser { db, user } => {
                call(&sibling(db), "dropUser", &[Bson::String(user.clone())])
            }
            Self::KillOp { opid } => call("db", "killOp", std::slice::from_ref(opid)),
            Self::Import {
                collection,
                documents,
                mode,
                ..
            } => {
                let target = coll(collection);
                let list = document_list(documents);
                match mode {
                    ImportMode::Insert => {
                        format!("{target}.insertMany({list}, {{ ordered: false }})")
                    }
                    ImportMode::DropAndInsert => format!(
                        "{target}.drop()\n{target}.insertMany({list}, {{ ordered: false }})"
                    ),
                    ImportMode::Upsert => format!(
                        "// For each document: replace the one with its _id, or insert it\n{list}.forEach(d => {target}.replaceOne({{ _id: d._id }}, d, {{ upsert: true }}))"
                    ),
                }
            }
            Self::Aggregate {
                collection,
                pipeline,
                ..
            } => format!(
                "{}.aggregate({})",
                coll(collection),
                shell::to_shell_string(
                    &Bson::Array(pipeline.iter().cloned().map(Bson::Document).collect()),
                    ShellStyle::Pretty(2)
                )
            ),
            Self::RunCommand { db, command } => call(
                &sibling(db),
                "runCommand",
                &[Bson::Document(command.clone())],
            ),
        }
    }

    /// One line naming the change, e.g. "Update 1 document in shop.orders".
    pub fn title(&self) -> String {
        let ns = self.namespace();
        match self {
            Self::InsertDocuments { documents: d, .. } => {
                format!("Insert {} into {ns}", documents(d.len() as u64))
            }
            Self::CloneDocument { .. } => format!("Insert a copy of 1 document into {ns}"),
            Self::ReplaceDocument { .. } | Self::ReplaceOne { .. } => {
                format!("Replace 1 document in {ns}")
            }
            Self::SetField { .. } | Self::UpdateOne { .. } => {
                format!("Update 1 document in {ns}")
            }
            Self::UpdateMany { .. } => format!("Update documents in {ns}"),
            Self::DeleteDocuments { ids, .. } => {
                format!("Delete {} from {ns}", documents(ids.len() as u64))
            }
            Self::DeleteOne { .. } => format!("Delete 1 document from {ns}"),
            Self::DeleteMany { .. } => format!("Delete documents from {ns}"),
            Self::CreateDatabase { db, .. } => format!("Create database {db}"),
            Self::DropDatabase { db } => format!("Drop database {db}"),
            Self::CreateCollection { .. } => format!("Create collection {ns}"),
            Self::CreateView { .. } => format!("Create view {ns}"),
            Self::DropCollection { .. } => format!("Drop collection {ns}"),
            Self::RenameCollection { to_db, to, .. } => format!("Rename {ns} to {to_db}.{to}"),
            Self::CreateIndex { .. } => format!("Create an index on {ns}"),
            Self::DropIndex { name, .. } => format!("Drop index {name} of {ns}"),
            Self::SetIndexHidden { name, hidden, .. } => format!(
                "{} index {name} of {ns}",
                if *hidden { "Hide" } else { "Unhide" }
            ),
            Self::CreateUser { db, user, .. } => format!("Create user {user} in {db}"),
            Self::DropUser { db, user } => format!("Drop user {user} from {db}"),
            Self::KillOp { opid } => format!(
                "Kill operation {}",
                shell::to_shell_string(opid, ShellStyle::Compact)
            ),
            Self::Import {
                documents: d, mode, ..
            } => match mode {
                ImportMode::DropAndInsert => format!(
                    "Replace {ns} with {} from an import",
                    documents(d.len() as u64)
                ),
                _ => format!("Import {} into {ns}", documents(d.len() as u64)),
            },
            Self::Aggregate { pipeline, .. } => match output_target(self.db(), pipeline) {
                Some((true, db, target)) => {
                    format!("Run a pipeline on {ns} that replaces {db}.{target}")
                }
                Some((false, db, target)) => {
                    format!("Run a pipeline on {ns} that merges into {db}.{target}")
                }
                None => format!("Run a writing pipeline on {ns}"),
            },
            Self::RunCommand { db, command } => {
                format!("Run command {} on {db}", command_name(command))
            }
        }
    }

    /// True for changes that remove or overwrite data or interrupt work: deletes, drops,
    /// replacements, renames, killOp, `$out`, dropping imports and unknown commands.
    pub fn is_destructive(&self) -> bool {
        match self {
            Self::ReplaceDocument { .. }
            | Self::ReplaceOne { .. }
            | Self::DeleteDocuments { .. }
            | Self::DeleteOne { .. }
            | Self::DeleteMany { .. }
            | Self::DropDatabase { .. }
            | Self::DropCollection { .. }
            | Self::RenameCollection { .. }
            | Self::DropIndex { .. }
            | Self::DropUser { .. }
            | Self::KillOp { .. }
            | Self::RunCommand { .. } => true,
            Self::Import { mode, .. } => *mode == ImportMode::DropAndInsert,
            Self::Aggregate { pipeline, .. } => {
                pipeline.last().is_some_and(|s| s.contains_key("$out"))
            }
            Self::InsertDocuments { .. }
            | Self::CloneDocument { .. }
            | Self::SetField { .. }
            | Self::UpdateOne { .. }
            | Self::UpdateMany { .. }
            | Self::CreateDatabase { .. }
            | Self::CreateCollection { .. }
            | Self::CreateView { .. }
            | Self::CreateIndex { .. }
            | Self::SetIndexHidden { .. }
            | Self::CreateUser { .. } => false,
        }
    }

    /// The line under the command in the confirmation: what the write will touch.
    pub fn summary(&self, affected: &Affected) -> String {
        let verb = match self {
            Self::DeleteDocuments { .. } | Self::DeleteOne { .. } | Self::DeleteMany { .. } => {
                "deleted"
            }
            Self::ReplaceDocument { .. } | Self::ReplaceOne { .. } => "replaced",
            _ => "modified",
        };
        match affected {
            Affected::Documents(0) => "No document matches: nothing will change".to_owned(),
            Affected::Documents(1) => format!("1 document will be {verb}"),
            Affected::Documents(n) => format!("{} will be {verb}", documents(*n)),
            Affected::Inserted(n) => {
                let what = if *n == 1 {
                    "1 document will be inserted".to_owned()
                } else {
                    format!("{} will be inserted", documents(*n))
                };
                match self {
                    Self::Import {
                        mode: ImportMode::DropAndInsert,
                        ..
                    } => format!("The collection is dropped first, then {what}"),
                    Self::Import {
                        mode: ImportMode::Upsert,
                        ..
                    } => format!("{} will be replaced or inserted", documents(*n)),
                    _ => what,
                }
            }
            Affected::Collection { name, documents: n } => match self {
                Self::Aggregate { .. } => format!(
                    "This replaces the collection {name} and its {} with the pipeline output",
                    documents(*n)
                ),
                Self::RenameCollection { .. } => {
                    format!(
                        "The collection and its {} move to the new name",
                        documents(*n)
                    )
                }
                _ => format!("This deletes the collection and its {}", documents(*n)),
            },
            Affected::Database {
                collections,
                documents: n,
            } => format!(
                "This deletes the database, its {} and {}",
                plural(*collections, "collection", "collections"),
                documents(*n)
            ),
            Affected::NotCounted => match self {
                Self::Aggregate { .. } => {
                    "The pipeline output is merged into the target collection".to_owned()
                }
                Self::CreateIndex { .. } => {
                    "The index is built over every document of the collection".to_owned()
                }
                _ => "Changes the database structure; no documents are counted".to_owned(),
            },
            Affected::Unknown => "The affected documents could not be counted in time".to_owned(),
        }
    }
}

impl Write {
    /// Compare what ran with what was expected: the count the confirmation showed
    /// (`expected`, from [`Connection::preview`]) and, for a delete of selected documents,
    /// how many were selected. A warning when they differ, e.g. "Expected 12 documents,
    /// 14 were matched — the data changed between preview and run"; `None` when they
    /// agree or nothing was counted.
    pub fn check_outcome(
        &self,
        expected: Option<&Affected>,
        outcome: &WriteOutcome,
    ) -> Option<String> {
        let (actual, verb) = match outcome {
            WriteOutcome::Updated { matched, .. } => (*matched, "matched"),
            WriteOutcome::Deleted(n) => (*n, "deleted"),
            WriteOutcome::Inserted(ids) => (ids.len() as u64, "inserted"),
            _ => return None,
        };
        if let Self::DeleteDocuments { ids, .. } = self
            && actual < ids.len() as u64
        {
            let selected = ids.len() as u64;
            return Some(format!(
                "Deleted {} of {} selected {}: {} no longer existed",
                format::group(actual),
                format::group(selected),
                if selected == 1 {
                    "document"
                } else {
                    "documents"
                },
                format::group(selected - actual)
            ));
        }
        let expected = match (self, expected) {
            (
                Self::UpdateOne { .. }
                | Self::UpdateMany { .. }
                | Self::ReplaceOne { .. }
                | Self::DeleteOne { .. }
                | Self::DeleteMany { .. }
                | Self::DeleteDocuments { .. }
                | Self::SetField { .. }
                | Self::ReplaceDocument { .. },
                Some(Affected::Documents(n)),
            ) => *n,
            (
                Self::InsertDocuments { .. } | Self::CloneDocument { .. },
                Some(Affected::Inserted(n)),
            ) => *n,
            _ => return None,
        };
        (actual != expected).then(|| {
            format!(
                "Expected {}, {} {} {verb} — the data changed between preview and run",
                documents(expected),
                format::group(actual),
                if actual == 1 { "was" } else { "were" }
            )
        })
    }
}

impl WriteOutcome {
    /// A status line for the outcome.
    pub fn message(&self) -> String {
        match self {
            Self::Inserted(ids) => match ids.as_slice() {
                [id] => format!(
                    "Inserted document {}",
                    shell::to_shell_string(id, ShellStyle::Compact)
                ),
                ids => format!("Inserted {}", documents(ids.len() as u64)),
            },
            Self::Updated { matched, modified } => match (matched, modified) {
                (0, _) => "No document matched; nothing changed".to_owned(),
                (_, 0) => "The document already had this value".to_owned(),
                (_, n) => format!("Updated {}", documents(*n)),
            },
            Self::Deleted(n) => crate::status::deleted_documents(*n),
            Self::IndexCreated(name) => format!("Created index {name}"),
            Self::Imported(report) => format!(
                "Imported {}{}",
                documents(report.inserted + report.upserted + report.replaced),
                if report.failed > 0 {
                    format!(", {} failed", format::group(report.failed))
                } else {
                    String::new()
                }
            ),
            Self::Aggregated { elapsed, .. } => {
                format!("Pipeline ran in {}", format::duration(*elapsed))
            }
            Self::Command(_) | Self::Done => "Done".to_owned(),
        }
    }
}

/// The outcome of a guarded single-document write: exactly one document matched, or the
/// document changed since it was loaded ([`RaabitError::Conflict`]; nothing was written).
fn single_document(matched: u64, modified: u64) -> Result<WriteOutcome> {
    if matched != 1 {
        return Err(RaabitError::Conflict(CONFLICT.into()));
    }
    Ok(WriteOutcome::Updated { matched, modified })
}

/// Run `count` with [`PREVIEW_TIMEOUT`]; a failure or timeout is `None`.
async fn bounded<F>(count: F) -> Option<u64>
where
    F: IntoFuture<Output = mongodb::error::Result<u64>>,
{
    match tokio::time::timeout(PREVIEW_TIMEOUT + Duration::from_secs(1), count).await {
        Ok(Ok(n)) => Some(n),
        _ => None,
    }
}

impl Connection {
    /// Documents matching `filter`, counting at most `limit` (0 = all).
    async fn preview_count(
        &self,
        db: &str,
        collection: &str,
        filter: Document,
        limit: u64,
    ) -> Affected {
        let handle = self.collection(db, collection);
        let mut count = handle.count_documents(filter).max_time(PREVIEW_TIMEOUT);
        if limit > 0 {
            count = count.limit(limit);
        }
        bounded(count)
            .await
            .map_or(Affected::Unknown, Affected::Documents)
    }

    async fn estimated(&self, db: &str, collection: &str) -> Option<u64> {
        let handle = self.collection(db, collection);
        bounded(handle.estimated_document_count().max_time(PREVIEW_TIMEOUT)).await
    }

    /// What `write` would run and touch, counted cheaply (bounded by
    /// [`PREVIEW_TIMEOUT`]; a count that fails or takes too long is [`Affected::Unknown`]).
    /// Changes nothing, also on a read-only connection.
    pub async fn preview(&self, write: &Write) -> Result<WritePreview> {
        let affected = match write {
            Write::InsertDocuments { documents, .. } => Affected::Inserted(documents.len() as u64),
            Write::Import { documents, .. } => Affected::Inserted(documents.len() as u64),
            Write::CloneDocument { .. } => Affected::Inserted(1),
            // Counted with the guard filter: a document that already changed shows as
            // "No document matches" before anything runs.
            Write::ReplaceDocument { db, collection, .. }
            | Write::SetField { db, collection, .. } => match write.canonical() {
                Write::ReplaceOne { filter, .. } | Write::UpdateOne { filter, .. } => {
                    self.preview_count(db, collection, filter, 1).await
                }
                _ => Affected::Unknown,
            },
            Write::UpdateOne {
                db,
                collection,
                filter,
                ..
            }
            | Write::ReplaceOne {
                db,
                collection,
                filter,
                ..
            }
            | Write::DeleteOne {
                db,
                collection,
                filter,
            } => self.preview_count(db, collection, filter.clone(), 1).await,
            Write::UpdateMany {
                db,
                collection,
                filter,
                ..
            }
            | Write::DeleteMany {
                db,
                collection,
                filter,
            } => self.preview_count(db, collection, filter.clone(), 0).await,
            Write::DeleteDocuments {
                db,
                collection,
                ids,
            } => {
                self.preview_count(db, collection, doc! {"_id": {"$in": ids.clone()}}, 0)
                    .await
            }
            Write::DropCollection { db, collection } => {
                match self.estimated(db, collection).await {
                    Some(documents) => Affected::Collection {
                        name: collection.clone(),
                        documents,
                    },
                    None => Affected::Unknown,
                }
            }
            Write::RenameCollection { db, from, .. } => match self.estimated(db, from).await {
                Some(documents) => Affected::Collection {
                    name: from.clone(),
                    documents,
                },
                None => Affected::Unknown,
            },
            Write::DropDatabase { db } => {
                let listed = tokio::time::timeout(PREVIEW_TIMEOUT, self.collections(db)).await;
                match listed {
                    Ok(Ok(list)) => {
                        let mut documents = 0;
                        let mut known = true;
                        let mut collections = 0;
                        for info in &list {
                            if info.kind == crate::connection::CollectionKind::GridFsBucket {
                                continue;
                            }
                            collections += 1;
                            if info.kind == crate::connection::CollectionKind::View {
                                continue;
                            }
                            match self.estimated(db, &info.name).await {
                                Some(n) => documents += n,
                                None => known = false,
                            }
                        }
                        if known {
                            Affected::Database {
                                collections,
                                documents,
                            }
                        } else {
                            Affected::Unknown
                        }
                    }
                    _ => Affected::Unknown,
                }
            }
            Write::Aggregate { db, pipeline, .. } => match output_target(db, pipeline) {
                Some((true, out_db, target)) => match self.estimated(&out_db, &target).await {
                    Some(documents) => Affected::Collection {
                        name: format!("{out_db}.{target}"),
                        documents,
                    },
                    None => Affected::Unknown,
                },
                _ => Affected::NotCounted,
            },
            Write::CreateDatabase { .. }
            | Write::CreateCollection { .. }
            | Write::CreateView { .. }
            | Write::CreateIndex { .. }
            | Write::DropIndex { .. }
            | Write::SetIndexHidden { .. }
            | Write::CreateUser { .. }
            | Write::DropUser { .. }
            | Write::KillOp { .. }
            | Write::RunCommand { .. } => Affected::NotCounted,
        };
        Ok(WritePreview {
            shell: write.shell(),
            affected,
        })
    }

    /// Run `write`: the only way to change the database. A read-only connection refuses
    /// it with [`crate::RaabitError::ReadOnly`] before anything is sent.
    pub async fn execute(&self, write: Write) -> Result<WriteOutcome> {
        self.execute_with(write, &AtomicBool::new(false), &|_| {})
            .await
    }

    /// [`Connection::execute`] with a cancel flag and progress (documents written) for
    /// long imports.
    pub async fn execute_with(
        &self,
        write: Write,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<WriteOutcome> {
        self.ensure_writable(|| format!("\"{}\"", write.title()))?;
        Ok(match write {
            Write::InsertDocuments {
                db,
                collection,
                documents,
            } => WriteOutcome::Inserted(self.insert_documents(&db, &collection, documents).await?),
            Write::CloneDocument {
                db,
                collection,
                document,
            } => WriteOutcome::Inserted(vec![
                self.clone_document(&db, &collection, &document).await?,
            ]),
            Write::ReplaceDocument {
                ref db,
                ref collection,
                ref id,
                ref replacement,
                ..
            } => {
                if let Some(new_id) = replacement.get("_id")
                    && new_id != id
                {
                    return Err(RaabitError::Invalid(
                        "_id cannot be changed; clone the document instead".into(),
                    ));
                }
                // Runs the `replaceOne` that `shell()` printed, guard filter included.
                let Write::ReplaceOne {
                    filter,
                    replacement,
                    ..
                } = write.canonical()
                else {
                    unreachable!("a ReplaceDocument is a replaceOne");
                };
                let r = self
                    .collection(db, collection)
                    .replace_one(filter, replacement)
                    .await?;
                single_document(r.matched_count, r.modified_count)?
            }
            Write::SetField {
                ref db,
                ref collection,
                ref path,
                ..
            } => {
                if path.is_empty() || path == "_id" || path.starts_with("_id.") {
                    return Err(RaabitError::Invalid("_id cannot be edited in place".into()));
                }
                // Runs the `updateOne` that `shell()` printed, guard filter included.
                let Write::UpdateOne { filter, update, .. } = write.canonical() else {
                    unreachable!("a SetField is an updateOne");
                };
                let r = self
                    .collection(db, collection)
                    .update_one(filter, update)
                    .await?;
                single_document(r.matched_count, r.modified_count)?
            }
            Write::UpdateOne {
                db,
                collection,
                filter,
                update,
            } => {
                let r = self
                    .collection(&db, &collection)
                    .update_one(filter, update)
                    .await?;
                WriteOutcome::Updated {
                    matched: r.matched_count,
                    modified: r.modified_count,
                }
            }
            Write::UpdateMany {
                db,
                collection,
                filter,
                update,
            } => {
                let r = self
                    .collection(&db, &collection)
                    .update_many(filter, update)
                    .await?;
                WriteOutcome::Updated {
                    matched: r.matched_count,
                    modified: r.modified_count,
                }
            }
            Write::ReplaceOne {
                db,
                collection,
                filter,
                replacement,
            } => {
                let r = self
                    .collection(&db, &collection)
                    .replace_one(filter, replacement)
                    .await?;
                WriteOutcome::Updated {
                    matched: r.matched_count,
                    modified: r.modified_count,
                }
            }
            Write::DeleteDocuments {
                db,
                collection,
                ids,
            } => WriteOutcome::Deleted(self.delete_documents(&db, &collection, &ids).await?),
            Write::DeleteOne {
                db,
                collection,
                filter,
            } => WriteOutcome::Deleted(
                self.collection(&db, &collection)
                    .delete_one(filter)
                    .await?
                    .deleted_count,
            ),
            Write::DeleteMany {
                db,
                collection,
                filter,
            } => WriteOutcome::Deleted(
                self.collection(&db, &collection)
                    .delete_many(filter)
                    .await?
                    .deleted_count,
            ),
            Write::CreateDatabase {
                db,
                first_collection,
            } => {
                self.create_database(&db, &first_collection).await?;
                WriteOutcome::Done
            }
            Write::DropDatabase { db } => {
                self.drop_database(&db).await?;
                WriteOutcome::Done
            }
            Write::CreateCollection { db, name, options } => {
                self.create_collection(&db, &name, options).await?;
                WriteOutcome::Done
            }
            Write::CreateView {
                db,
                name,
                view_on,
                pipeline,
            } => {
                self.create_view(&db, &name, &view_on, pipeline).await?;
                WriteOutcome::Done
            }
            Write::DropCollection { db, collection } => {
                self.drop_collection(&db, &collection).await?;
                WriteOutcome::Done
            }
            Write::RenameCollection {
                db,
                from,
                to_db,
                to,
                drop_target,
            } => {
                self.rename_collection(&db, &from, &to_db, &to, drop_target)
                    .await?;
                WriteOutcome::Done
            }
            Write::CreateIndex {
                db,
                collection,
                index,
            } => WriteOutcome::IndexCreated(self.create_index(&db, &collection, index).await?),
            Write::DropIndex {
                db,
                collection,
                name,
            } => {
                self.drop_index(&db, &collection, &name).await?;
                WriteOutcome::Done
            }
            Write::SetIndexHidden {
                db,
                collection,
                name,
                hidden,
            } => {
                self.set_index_hidden(&db, &collection, &name, hidden)
                    .await?;
                WriteOutcome::Done
            }
            Write::CreateUser {
                db,
                user,
                password,
                roles,
            } => {
                self.create_user(&db, &user, &password, &roles).await?;
                WriteOutcome::Done
            }
            Write::DropUser { db, user } => {
                self.drop_user(&db, &user).await?;
                WriteOutcome::Done
            }
            Write::KillOp { opid } => {
                self.kill_op(opid).await?;
                WriteOutcome::Done
            }
            Write::Import {
                db,
                collection,
                documents,
                mode,
            } => WriteOutcome::Imported(
                self.import(&db, &collection, documents, mode, cancel, progress)
                    .await?,
            ),
            Write::Aggregate {
                db,
                collection,
                pipeline,
            } => {
                let start = Instant::now();
                // Bounded on the server, so a pipeline that runs too long stops there
                // (`$out` then leaves its target as it was) instead of finishing after the
                // view gave up on it.
                let cursor = self
                    .collection(&db, &collection)
                    .aggregate(pipeline)
                    .allow_disk_use(true)
                    .max_time(self.query_timeout())
                    .await?;
                let documents: Vec<Document> = cursor.try_collect().await?;
                WriteOutcome::Aggregated {
                    documents,
                    elapsed: start.elapsed(),
                }
            }
            Write::RunCommand { db, command } => {
                WriteOutcome::Command(self.command(&db, command).await?)
            }
        })
    }
}

/// The documents of `orders` the [`samples`] expect: `_id` 1 to 3, an index `n_1`.
pub fn sample_seed() -> Vec<Document> {
    vec![
        doc! {"_id": 1, "status": "open", "n": 1, "tags": ["a", "b"]},
        doc! {"_id": 2, "status": "open", "n": 2, "address": {"city": "Oslo"}},
        doc! {"_id": 3, "status": "done", "n": 3},
    ]
}

/// One write of every [`Write`] variant on `db`, written against [`sample_seed`] (the
/// collection `orders` with its index `n_1`, and a user `reporter`). The contract tests
/// run each of them: the round trip through the shell, the live test of its exact
/// result, and the read-only refusal. `tests/write_contract.rs` fails when a variant of
/// the enum has no sample here.
pub fn samples(db: &str) -> Vec<Write> {
    let db = db.to_owned();
    let orders = "orders".to_owned();
    let seed = sample_seed();
    vec![
        Write::InsertDocuments {
            db: db.clone(),
            collection: orders.clone(),
            documents: vec![
                doc! {"_id": 10, "name": "Ada \"the first\" Lovelace"},
                doc! {"_id": 11, "when": DateTime::from_millis(1_700_000_000_123)},
            ],
        },
        Write::CloneDocument {
            db: db.clone(),
            collection: orders.clone(),
            document: seed[0].clone(),
        },
        Write::ReplaceDocument {
            db: db.clone(),
            collection: orders.clone(),
            id: Bson::Int32(1),
            original: seed[0].clone(),
            replacement: doc! {"_id": 1, "status": "done", "n": 1},
        },
        Write::SetField {
            db: db.clone(),
            collection: orders.clone(),
            id: Bson::Int32(2),
            path: "address.city".into(),
            value: Some(Bson::String("Bergen".into())),
            original: Some(Bson::String("Oslo".into())),
        },
        Write::UpdateOne {
            db: db.clone(),
            collection: orders.clone(),
            filter: doc! {"_id": 3},
            update: doc! {"$inc": {"n": 10}},
        },
        Write::UpdateMany {
            db: db.clone(),
            collection: orders.clone(),
            filter: doc! {"status": "open"},
            update: doc! {"$set": {"flag": true}},
        },
        Write::ReplaceOne {
            db: db.clone(),
            collection: orders.clone(),
            filter: doc! {"_id": 2},
            replacement: doc! {"_id": 2, "status": "replaced"},
        },
        Write::DeleteDocuments {
            db: db.clone(),
            collection: orders.clone(),
            ids: vec![Bson::Int32(1), Bson::Int32(2)],
        },
        Write::DeleteOne {
            db: db.clone(),
            collection: orders.clone(),
            filter: doc! {"_id": 3},
        },
        Write::DeleteMany {
            db: db.clone(),
            collection: orders.clone(),
            filter: doc! {"status": "open"},
        },
        Write::CreateDatabase {
            db: db.clone(),
            first_collection: "fresh".into(),
        },
        Write::DropDatabase { db: db.clone() },
        Write::CreateCollection {
            db: db.clone(),
            name: "logs".into(),
            options: doc! {"capped": true, "size": 4096},
        },
        Write::CreateView {
            db: db.clone(),
            name: "open_orders".into(),
            view_on: orders.clone(),
            pipeline: vec![doc! {"$match": {"status": "open"}}],
        },
        Write::DropCollection {
            db: db.clone(),
            collection: orders.clone(),
        },
        Write::RenameCollection {
            db: db.clone(),
            from: orders.clone(),
            to_db: db.clone(),
            to: "archive".into(),
            drop_target: false,
        },
        Write::CreateIndex {
            db: db.clone(),
            collection: orders.clone(),
            index: NewIndex {
                keys: doc! {"status": 1, "n": -1},
                name: Some("by_status".into()),
                sparse: true,
                ..NewIndex::default()
            },
        },
        Write::DropIndex {
            db: db.clone(),
            collection: orders.clone(),
            name: "n_1".into(),
        },
        Write::SetIndexHidden {
            db: db.clone(),
            collection: orders.clone(),
            name: "n_1".into(),
            hidden: true,
        },
        Write::CreateUser {
            db: db.clone(),
            user: "auditor".into(),
            password: "s3cret".into(),
            roles: vec![RoleRef {
                role: "read".into(),
                db: db.clone(),
            }],
        },
        Write::DropUser {
            db: db.clone(),
            user: "reporter".into(),
        },
        Write::KillOp {
            opid: Bson::Int32(2_000_000_000),
        },
        Write::Import {
            db: db.clone(),
            collection: orders.clone(),
            documents: vec![doc! {"_id": 3, "status": "imported"}, doc! {"_id": 20}],
            mode: ImportMode::Upsert,
        },
        Write::Aggregate {
            db: db.clone(),
            collection: orders.clone(),
            pipeline: vec![
                doc! {"$match": {"status": "open"}},
                doc! {"$out": "open_copy"},
            ],
        },
        Write::RunCommand {
            db,
            command: doc! {"collMod": orders, "validator": {"n": {"$gte": 0}}},
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RaabitError;
    use mongodb::bson::oid::ObjectId;

    fn ns() -> (String, String) {
        ("shop".into(), "orders".into())
    }

    #[test]
    fn inline_edit_is_an_update_one_with_set() {
        let (db, collection) = ns();
        let id = ObjectId::parse_str("65f000000000000000000001").unwrap();
        let write = Write::SetField {
            db,
            collection,
            id: Bson::ObjectId(id),
            path: "price".into(),
            value: Some(Bson::Double(12.5)),
            original: Some(Bson::Double(10.0)),
        };
        assert_eq!(
            write.shell(),
            "db.getCollection(\"orders\").updateOne({\n  _id: ObjectId(\"65f000000000000000000001\"),\n  price: {\n    $eq: 10.0,\n    $type: \"double\",\n    $not: {\n      $type: \"array\"\n    }\n  }\n}, {\n  $set: {\n    price: 12.5\n  }\n})"
        );
        assert_eq!(write.title(), "Update 1 document in shop.orders");
        assert!(!write.is_destructive());
        let unset = Write::SetField {
            db: "shop".into(),
            collection: "orders".into(),
            id: Bson::Int32(7),
            path: "a.b c".into(),
            value: None,
            original: Some(Bson::Int32(3)),
        };
        assert_eq!(
            unset.shell(),
            "db.getCollection(\"orders\").updateOne({\n  _id: 7,\n  \"a.b c\": {\n    $eq: 3,\n    $type: \"int\",\n    $not: {\n      $type: \"array\"\n    }\n  }\n}, {\n  $unset: {\n    \"a.b c\": \"\"\n  }\n})"
        );
    }

    #[test]
    fn edits_only_apply_to_the_value_that_was_loaded() {
        let id = Bson::Int32(1);
        assert_eq!(
            set_field_filter(&id, "a.b", Some(&Bson::String("x".into()))),
            doc! {"_id": 1, "a.b": {"$eq": "x", "$type": "string", "$not": {"$type": "array"}}}
        );
        assert_eq!(
            set_field_filter(&id, "a", None),
            doc! {"_id": 1, "a": {"$exists": false}},
            "a missing field must still be missing"
        );
        assert_eq!(
            set_field_filter(&id, "a", Some(&Bson::Null)),
            doc! {"_id": 1, "a": {"$eq": Bson::Null, "$type": "null", "$not": {"$type": "array"}}},
            "{{ a: null }} alone would also match a missing field"
        );
        assert_eq!(
            set_field_filter(&id, "a", Some(&Bson::Int64(1))),
            doc! {"_id": 1, "a": {"$eq": 1_i64, "$type": "long", "$not": {"$type": "array"}}},
            "1 that became 1.0 is a change"
        );
        assert_eq!(
            set_field_filter(&id, "a", Some(&Bson::Document(doc! {"$gt": 1}))),
            doc! {"_id": 1, "a": {"$eq": {"$gt": 1}, "$type": "object", "$not": {"$type": "array"}}},
            "a document is a value, not an operator"
        );
        let regex = Bson::RegularExpression(mongodb::bson::Regex {
            pattern: "^a".into(),
            options: String::new(),
        });
        assert_eq!(
            set_field_filter(&id, "a", Some(&regex)),
            doc! {"_id": 1, "a": {"$eq": regex.clone(), "$type": "regex", "$not": {"$type": "array"}}},
            "a regular expression is a value, not a pattern"
        );
        assert_eq!(
            set_field_filter(&id, "a", Some(&Bson::Array(vec![Bson::Int32(1)]))),
            doc! {"_id": 1, "a": {"$eq": [1], "$type": "array"}}
        );
        assert_eq!(
            replace_filter(&id, &doc! {"_id": 1, "s": "$x"}),
            doc! {"_id": 1, "$expr": {"$and": [
                {"$eq": ["$$ROOT", {"$literal": {"_id": 1, "s": "$x"}}]},
                {"$eq": [{"$bsonSize": "$$ROOT"}, 24]},
            ]}}
        );
    }

    #[test]
    fn document_writes_print_their_arguments() {
        let (db, collection) = ns();
        let insert = Write::InsertDocuments {
            db: db.clone(),
            collection: collection.clone(),
            documents: vec![doc! {"name": "Ada"}],
        };
        assert_eq!(
            insert.shell(),
            "db.getCollection(\"orders\").insertOne({ name: \"Ada\" })"
        );
        assert_eq!(insert.title(), "Insert 1 document into shop.orders");
        let many = Write::InsertDocuments {
            db: db.clone(),
            collection: collection.clone(),
            documents: (0..5).map(|i| doc! {"i": i}).collect(),
        };
        let text = many.shell();
        assert!(
            text.starts_with("db.getCollection(\"orders\").insertMany(["),
            "{text}"
        );
        assert!(text.contains("// … and 2 more documents"), "{text}");
        assert_eq!(many.title(), "Insert 5 documents into shop.orders");

        let clone = Write::CloneDocument {
            db: db.clone(),
            collection: collection.clone(),
            document: doc! {"_id": 1, "a": 1},
        };
        assert_eq!(
            clone.shell(),
            "db.getCollection(\"orders\").insertOne({ a: 1 })"
        );

        let replace = Write::ReplaceDocument {
            db: db.clone(),
            collection: collection.clone(),
            id: Bson::Int32(1),
            original: doc! {"_id": 1, "a": 1},
            replacement: doc! {"_id": 1, "a": 2},
        };
        assert_eq!(
            replace.shell(),
            "db.getCollection(\"orders\").replaceOne({\n  _id: 1,\n  $expr: {\n    $and: [\n      {\n        $eq: [\n          \"$$ROOT\",\n          {\n            $literal: {\n              _id: 1,\n              a: 1\n            }\n          }\n        ]\n      },\n      {\n        $eq: [\n          {\n            $bsonSize: \"$$ROOT\"\n          },\n          21\n        ]\n      }\n    ]\n  }\n}, {\n  _id: 1,\n  a: 2\n})"
        );
        assert!(replace.is_destructive());

        let update = Write::UpdateMany {
            db: db.clone(),
            collection: collection.clone(),
            filter: doc! {"status": "open"},
            update: doc! {"$inc": {"n": 1}},
        };
        assert_eq!(
            update.shell(),
            "db.getCollection(\"orders\").updateMany({ status: \"open\" }, { $inc: { n: 1 } })"
        );
        assert_eq!(update.title(), "Update documents in shop.orders");

        let long = Write::UpdateOne {
            db: db.clone(),
            collection: collection.clone(),
            filter: doc! {"description": "x".repeat(60)},
            update: doc! {"$set": {"a": 1}},
        };
        assert_eq!(
            long.shell(),
            format!(
                "db.getCollection(\"orders\").updateOne({{\n  description: \"{}\"\n}}, {{\n  $set: {{\n    a: 1\n  }}\n}})",
                "x".repeat(60)
            )
        );

        let one = Write::DeleteDocuments {
            db: db.clone(),
            collection: collection.clone(),
            ids: vec![Bson::Int32(3)],
        };
        assert_eq!(
            one.shell(),
            "db.getCollection(\"orders\").deleteOne({ _id: 3 })"
        );
        assert_eq!(one.title(), "Delete 1 document from shop.orders");
        let two = Write::DeleteDocuments {
            db,
            collection,
            ids: vec![Bson::Int32(3), Bson::Int32(4)],
        };
        assert_eq!(
            two.shell(),
            "db.getCollection(\"orders\").deleteMany({ _id: { $in: [ 3, 4 ] } })"
        );
        assert!(two.is_destructive());
    }

    #[test]
    fn structure_writes_print_shell_helpers() {
        let drop_db = Write::DropDatabase { db: "shop".into() };
        assert_eq!(drop_db.shell(), "db.getSiblingDB(\"shop\").dropDatabase()");
        assert_eq!(drop_db.title(), "Drop database shop");
        assert!(drop_db.is_destructive());

        let create = Write::CreateCollection {
            db: "shop".into(),
            name: "logs".into(),
            options: doc! {"capped": true, "size": 4096},
        };
        assert_eq!(
            create.shell(),
            "db.getSiblingDB(\"shop\").createCollection(\"logs\", { capped: true, size: 4096 })"
        );
        assert!(!create.is_destructive());
        assert_eq!(
            Write::CreateView {
                db: "shop".into(),
                name: "v".into(),
                view_on: "orders".into(),
                pipeline: vec![doc! {"$match": {"a": 1}}],
            }
            .shell(),
            "db.getSiblingDB(\"shop\").createView(\"v\", \"orders\", [ { $match: { a: 1 } } ])"
        );
        let drop = Write::DropCollection {
            db: "shop".into(),
            collection: "orders".into(),
        };
        assert_eq!(drop.shell(), "db.getCollection(\"orders\").drop()");
        assert_eq!(drop.title(), "Drop collection shop.orders");
        let rename = Write::RenameCollection {
            db: "shop".into(),
            from: "a".into(),
            to_db: "shop".into(),
            to: "b".into(),
            drop_target: false,
        };
        assert_eq!(
            rename.shell(),
            "db.getCollection(\"a\").renameCollection(\"b\", false)"
        );
        assert_eq!(rename.title(), "Rename shop.a to shop.b");
        assert!(rename.is_destructive());
        let across = Write::RenameCollection {
            db: "shop".into(),
            from: "a".into(),
            to_db: "archive".into(),
            to: "b".into(),
            drop_target: false,
        };
        assert_eq!(
            across.shell(),
            "db.adminCommand({ renameCollection: \"shop.a\", to: \"archive.b\", dropTarget: false })"
        );

        let index = Write::CreateIndex {
            db: "shop".into(),
            collection: "orders".into(),
            index: NewIndex {
                keys: doc! {"email": 1},
                name: Some("by_email".into()),
                unique: true,
                ttl_seconds: Some(60),
                ..NewIndex::default()
            },
        };
        assert_eq!(
            index.shell(),
            "db.getCollection(\"orders\").createIndex({\n  email: 1\n}, {\n  name: \"by_email\",\n  unique: true,\n  expireAfterSeconds: 60\n})"
        );
        assert!(!index.is_destructive());
        let hide = Write::SetIndexHidden {
            db: "shop".into(),
            collection: "orders".into(),
            name: "by_email".into(),
            hidden: true,
        };
        assert_eq!(
            hide.shell(),
            "db.getCollection(\"orders\").hideIndex(\"by_email\")"
        );
        assert_eq!(hide.title(), "Hide index by_email of shop.orders");
        let drop_index = Write::DropIndex {
            db: "shop".into(),
            collection: "orders".into(),
            name: "by_email".into(),
        };
        assert_eq!(
            drop_index.shell(),
            "db.getCollection(\"orders\").dropIndex(\"by_email\")"
        );
        assert!(drop_index.is_destructive());

        let user = Write::CreateUser {
            db: "admin".into(),
            user: "reporter".into(),
            password: "s3cret".into(),
            roles: vec![RoleRef {
                role: "read".into(),
                db: "shop".into(),
            }],
        };
        let text = user.shell();
        assert!(!text.contains("s3cret"), "{text}");
        assert_eq!(
            text,
            "db.getSiblingDB(\"admin\").createUser({ user: \"reporter\", pwd: passwordPrompt(), roles: [ { role: \"read\", db: \"shop\" } ] })"
        );
        assert!(!user.is_destructive());
        let kill = Write::KillOp {
            opid: Bson::Int32(42),
        };
        assert_eq!(kill.shell(), "db.killOp(42)");
        assert_eq!(kill.title(), "Kill operation 42");
        assert!(kill.is_destructive());
        assert!(
            Write::DropUser {
                db: "admin".into(),
                user: "x".into()
            }
            .is_destructive()
        );
    }

    #[test]
    fn imports_aggregations_and_commands() {
        let import = Write::Import {
            db: "shop".into(),
            collection: "orders".into(),
            documents: vec![doc! {"a": 1}],
            mode: ImportMode::DropAndInsert,
        };
        assert_eq!(
            import.shell(),
            "db.getCollection(\"orders\").drop()\ndb.getCollection(\"orders\").insertMany([\n  {\n    a: 1\n  }\n], { ordered: false })"
        );
        assert!(import.is_destructive());
        assert_eq!(
            import.summary(&Affected::Inserted(1)),
            "The collection is dropped first, then 1 document will be inserted"
        );
        let insert = Write::Import {
            db: "shop".into(),
            collection: "orders".into(),
            documents: vec![doc! {"a": 1}],
            mode: ImportMode::Insert,
        };
        assert!(!insert.is_destructive());
        assert_eq!(insert.title(), "Import 1 document into shop.orders");

        let out = Write::Aggregate {
            db: "shop".into(),
            collection: "orders".into(),
            pipeline: vec![doc! {"$match": {}}, doc! {"$out": "totals"}],
        };
        assert!(
            out.shell()
                .starts_with("db.getCollection(\"orders\").aggregate([\n")
        );
        assert!(out.is_destructive());
        assert_eq!(
            out.title(),
            "Run a pipeline on shop.orders that replaces shop.totals"
        );
        let merge = Write::Aggregate {
            db: "shop".into(),
            collection: "orders".into(),
            pipeline: vec![doc! {"$merge": {"into": {"db": "x", "coll": "y"}}}],
        };
        assert!(!merge.is_destructive());
        assert_eq!(
            merge.title(),
            "Run a pipeline on shop.orders that merges into x.y"
        );

        let command = Write::RunCommand {
            db: "shop".into(),
            command: doc! {"compact": "orders"},
        };
        assert_eq!(
            command.shell(),
            "db.getSiblingDB(\"shop\").runCommand({ compact: \"orders\" })"
        );
        assert_eq!(command.title(), "Run command compact on shop");
        assert!(command.is_destructive());
    }

    #[test]
    fn summaries_say_what_is_affected() {
        let (db, collection) = ns();
        let update = Write::UpdateMany {
            db: db.clone(),
            collection: collection.clone(),
            filter: doc! {},
            update: doc! {"$set": {"a": 1}},
        };
        assert_eq!(
            update.summary(&Affected::Documents(2)),
            "2 documents will be modified"
        );
        assert_eq!(
            update.summary(&Affected::Documents(1)),
            "1 document will be modified"
        );
        assert_eq!(
            update.summary(&Affected::Documents(0)),
            "No document matches: nothing will change"
        );
        let delete = Write::DeleteMany {
            db: db.clone(),
            collection: collection.clone(),
            filter: doc! {},
        };
        assert_eq!(
            delete.summary(&Affected::Documents(1500)),
            "1,500 documents will be deleted"
        );
        let drop = Write::DropCollection { db, collection };
        assert_eq!(
            drop.summary(&Affected::Collection {
                name: "orders".into(),
                documents: 1204
            }),
            "This deletes the collection and its 1,204 documents"
        );
        assert_eq!(
            Write::DropDatabase { db: "shop".into() }.summary(&Affected::Database {
                collections: 3,
                documents: 1
            }),
            "This deletes the database, its 3 collections and 1 document"
        );
        assert_eq!(
            drop.summary(&Affected::Unknown),
            "The affected documents could not be counted in time"
        );
    }

    #[test]
    fn only_known_read_commands_are_reads() {
        for read in [
            doc! {"find": "orders", "filter": {}},
            doc! {"count": "orders"},
            doc! {"distinct": "orders", "key": "a"},
            doc! {"listCollections": 1},
            doc! {"listDatabases": 1},
            doc! {"dbStats": 1},
            doc! {"collStats": "orders"},
            doc! {"serverStatus": 1},
            doc! {"hello": 1},
            doc! {"buildInfo": 1},
            doc! {"connectionStatus": 1},
            doc! {"currentOp": 1},
            doc! {"hostInfo": 1},
            doc! {"getParameter": "*"},
            doc! {"aggregate": "orders", "pipeline": [{"$match": {}}], "cursor": {}},
            doc! {"explain": {"find": "orders"}},
            doc! {"explain": {"delete": "orders", "deletes": []}},
        ] {
            assert!(is_read_command(&read), "{read}");
        }
        for write in [
            doc! {"insert": "orders", "documents": []},
            doc! {"update": "orders", "updates": []},
            doc! {"delete": "orders", "deletes": []},
            doc! {"drop": "orders"},
            doc! {"dropDatabase": 1},
            doc! {"createIndexes": "orders"},
            doc! {"findAndModify": "orders"},
            doc! {"killOp": 1, "op": 3},
            doc! {"aggregate": "orders", "pipeline": [{"$out": "x"}], "cursor": {}},
            doc! {"aggregate": "orders", "pipeline": [{"$merge": "x"}], "cursor": {}},
            doc! {"explain": {"aggregate": "orders", "pipeline": [{"$out": "x"}]}},
            doc! {"Find": "orders"},
            doc! {},
        ] {
            assert!(!is_read_command(&write), "{write}");
        }
        assert!(pipeline_writes(&[doc! {"$out": "x"}]));
        assert!(!pipeline_writes(&[doc! {"$match": {}}]));
    }

    #[test]
    fn outcomes_read_as_status_lines() {
        assert_eq!(
            WriteOutcome::Inserted(vec![Bson::Int32(5)]).message(),
            "Inserted document 5"
        );
        assert_eq!(
            WriteOutcome::Updated {
                matched: 1,
                modified: 1
            }
            .message(),
            "Updated 1 document"
        );
        assert_eq!(
            WriteOutcome::IndexCreated("a_1".into()).message(),
            "Created index a_1"
        );
        assert_eq!(
            RaabitError::ReadOnly("\"Drop database shop\"".into()).to_string(),
            "read-only connection: \"Drop database shop\" was blocked, nothing was changed"
        );
    }
}
