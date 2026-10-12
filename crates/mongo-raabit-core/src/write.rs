//! Every change Mongo Raabit can make to a database, as data.
//!
//! A [`Write`] says exactly what will run: the views show it ([`Write::shell`],
//! [`Write::title`], [`Connection::preview`]) before [`Connection::execute`] runs it.
//! `execute` is the only way to change data or structure: the write methods of the
//! connection are private to this crate, and a read-only connection refuses every
//! `Write` with [`crate::RaabitError::ReadOnly`] before anything reaches the server.

use crate::connection::{Connection, NewIndex, RoleRef};
use crate::error::Result;
use crate::format;
use crate::shell::{self, ShellStyle};
use crate::transfer::{ImportMode, ImportReport};
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use std::future::IntoFuture;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// How long a preview may count before it gives up ([`Affected::Unknown`]).
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(4);

/// Longest argument list printed on one line by [`Write::shell`].
const COMPACT_WIDTH: usize = 80;
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
    /// The document editor's Save: replace the document with this `_id`.
    ReplaceDocument {
        db: String,
        collection: String,
        id: Bson,
        replacement: Document,
    },
    /// An inline edit: `$set` (or with `None`, `$unset`) one field of one document.
    SetField {
        db: String,
        collection: String,
        id: Bson,
        path: String,
        value: Option<Bson>,
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

/// Arguments of a call: on one line when short, else each one pretty-printed.
fn args(values: &[Bson]) -> String {
    let compact: Vec<String> = values
        .iter()
        .map(|v| shell::to_shell_string(v, ShellStyle::Compact))
        .collect();
    let line = compact.join(", ");
    if line.chars().count() <= COMPACT_WIDTH && !line.contains('\n') {
        return line;
    }
    values
        .iter()
        .map(|v| shell::to_shell_string(v, ShellStyle::Pretty(2)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn call(target: &str, method: &str, values: &[Bson]) -> String {
    format!("{target}.{method}({})", args(values))
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

impl Write {
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
        let id_filter = |id: &Bson| Bson::Document(doc! {"_id": id.clone()});
        match self {
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
            Self::CloneDocument {
                collection,
                document,
                ..
            } => {
                let mut copy = document.clone();
                copy.remove("_id");
                call(&coll(collection), "insertOne", &[Bson::Document(copy)])
            }
            Self::ReplaceDocument {
                collection,
                id,
                replacement,
                ..
            } => call(
                &coll(collection),
                "replaceOne",
                &[id_filter(id), Bson::Document(replacement.clone())],
            ),
            Self::SetField {
                collection,
                id,
                path,
                value,
                ..
            } => {
                let update = match value {
                    Some(value) => doc! {"$set": {path.as_str(): value.clone()}},
                    None => doc! {"$unset": {path.as_str(): ""}},
                };
                call(
                    &coll(collection),
                    "updateOne",
                    &[id_filter(id), Bson::Document(update)],
                )
            }
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
            Self::DeleteDocuments {
                collection, ids, ..
            } => match ids.as_slice() {
                [id] => call(&coll(collection), "deleteOne", &[id_filter(id)]),
                many => call(
                    &coll(collection),
                    "deleteMany",
                    &[Bson::Document(doc! {"_id": {"$in": many.to_vec()}})],
                ),
            },
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
            Self::CreateDatabase {
                db,
                first_collection,
            } => call(
                &sibling(db),
                "createCollection",
                &[Bson::String(first_collection.clone())],
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
            Write::ReplaceDocument {
                db, collection, id, ..
            }
            | Write::SetField {
                db, collection, id, ..
            } => {
                self.preview_count(db, collection, doc! {"_id": id.clone()}, 1)
                    .await
            }
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
                db,
                collection,
                id,
                replacement,
            } => {
                self.replace_document(&db, &collection, &id, replacement)
                    .await?;
                WriteOutcome::Updated {
                    matched: 1,
                    modified: 1,
                }
            }
            Write::SetField {
                db,
                collection,
                id,
                path,
                value,
            } => {
                self.set_field(&db, &collection, &id, &path, value).await?;
                WriteOutcome::Updated {
                    matched: 1,
                    modified: 1,
                }
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
                let cursor = self
                    .collection(&db, &collection)
                    .aggregate(pipeline)
                    .allow_disk_use(true)
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
        };
        assert_eq!(
            write.shell(),
            "db.getCollection(\"orders\").updateOne({ _id: ObjectId(\"65f000000000000000000001\") }, { $set: { price: 12.5 } })"
        );
        assert_eq!(write.title(), "Update 1 document in shop.orders");
        assert!(!write.is_destructive());
        let unset = Write::SetField {
            db: "shop".into(),
            collection: "orders".into(),
            id: Bson::Int32(7),
            path: "a.b c".into(),
            value: None,
        };
        assert_eq!(
            unset.shell(),
            "db.getCollection(\"orders\").updateOne({ _id: 7 }, { $unset: { \"a.b c\": \"\" } })"
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
            replacement: doc! {"_id": 1, "a": 2},
        };
        assert_eq!(
            replace.shell(),
            "db.getCollection(\"orders\").replaceOne({ _id: 1 }, { _id: 1, a: 2 })"
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
            "db.getCollection(\"orders\").createIndex({ email: 1 }, { name: \"by_email\", unique: true, expireAfterSeconds: 60 })"
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
