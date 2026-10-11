//! View state of mongo-raabit and the operations that load data in the background.

use crate::dialogs::{
    CodeDialog, ConfirmDelete, Dialog, DocumentEditor, DocumentMode, ExplainDialog,
};
use crate::jobs::Jobs;
use crate::manager::ConnectionManager;
use mongo_raabit_core::Connection;
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::codegen::{CodeRequest, Operation};
use mongo_raabit_core::connection::{
    CollectionInfo, CollectionKind, CollectionStats, DatabaseInfo, ServerInfo,
};
use mongo_raabit_core::profile::{ConnectionProfile, ProfileStore};
use mongo_raabit_core::query::{Page, Query, QueryText};
use mongo_raabit_core::secrets::{MemorySecretStore, SecretStore};
use mongo_raabit_core::shell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

pub type SessionId = u64;
pub type TabId = u64;

/// Something loaded in the background.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
}

pub struct CollectionEntry {
    pub info: CollectionInfo,
    pub stats: Option<CollectionStats>,
}

pub struct DatabaseEntry {
    pub info: DatabaseInfo,
    pub collections: Load<Vec<CollectionEntry>>,
}

pub enum SessionState {
    Connecting,
    Connected(Connection),
    Failed(String),
}

/// An opened connection in the connection tree.
pub struct Session {
    pub id: SessionId,
    pub profile: ConnectionProfile,
    pub state: SessionState,
    pub databases: Load<Vec<DatabaseEntry>>,
}

impl Session {
    pub fn connection(&self) -> Option<&Connection> {
        match &self.state {
            SessionState::Connected(conn) => Some(conn),
            _ => None,
        }
    }

    pub fn server(&self) -> Option<&ServerInfo> {
        self.connection().map(Connection::server)
    }
}

/// How the query of a collection tab is written: visually, as shell code, as SQL, or
/// as a free mongosh script (IntelliShell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryMode {
    #[default]
    Builder,
    Code,
    Sql,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultView {
    Table,
    Tree,
    Json,
}

pub struct CollectionTab {
    pub session: SessionId,
    pub db: String,
    pub collection: String,
    pub kind: CollectionKind,
    pub query: QueryText,
    pub page: u64,
    pub page_size: u64,
    pub result: Load<Page>,
    pub count: Load<u64>,
    pub view: ResultView,
    /// Drill-down path in the table view (`address`, `items`…).
    pub drill: Vec<String>,
    /// Expanded tree rows by path key.
    pub open: HashSet<String>,
    pub selected: Option<usize>,
    /// Incremented per run; stale results are dropped.
    pub seq: u64,
    pub ran: Option<Query>,
    pub query_error: Option<String>,
    /// Inline table cell edit in progress.
    pub editing: Option<CellEdit>,
    pub mode: QueryMode,
    /// Visual query builder state (Builder mode).
    pub builder: crate::query_builder::QueryBuilder,
    /// SQL mode state.
    pub sql: crate::tools::SqlTab,
    /// Shell mode state, created when the mode is first opened.
    pub shell: Option<Box<crate::tools::ShellTab>>,
}

/// An inline edit of one field of one document.
#[derive(Debug, Clone, PartialEq)]
pub struct CellEdit {
    pub doc_index: usize,
    /// Dotted field path inside the document.
    pub path: String,
    pub text: String,
    /// The value before editing (strings stay strings).
    pub original: Option<Bson>,
    /// Focus the editor on the next frame.
    pub focus: bool,
}

impl CollectionTab {
    pub fn new(session: SessionId, db: &str, collection: &str, kind: CollectionKind) -> Self {
        Self {
            session,
            db: db.to_owned(),
            collection: collection.to_owned(),
            kind,
            query: QueryText::default(),
            page: 0,
            page_size: 50,
            result: Load::Idle,
            count: Load::Idle,
            view: ResultView::Table,
            drill: Vec::new(),
            open: HashSet::new(),
            selected: None,
            seq: 0,
            ran: None,
            query_error: None,
            editing: None,
            mode: QueryMode::default(),
            builder: Default::default(),
            sql: crate::tools::SqlTab {
                session,
                db: db.to_owned(),
                sql: format!("SELECT *\nFROM {collection}\nLIMIT 50"),
                result: Load::Idle,
                view: ResultView::Tree,
                open: HashSet::new(),
            },
            shell: None,
        }
    }
}

/// The three tool tabs: Query (builder, code or SQL), Aggregation and Structure
/// (schema and indexes).
pub enum TabKind {
    Welcome,
    Collection(Box<CollectionTab>),
    Aggregation(Box<crate::tools::AggregationTab>),
    Structure(Box<crate::tools::StructureTab>),
}

impl TabKind {
    /// The session a tab belongs to (`None` for the welcome tab).
    pub fn session(&self) -> Option<SessionId> {
        match self {
            Self::Welcome => None,
            Self::Collection(t) => Some(t.session),
            Self::Aggregation(t) => Some(t.session),
            Self::Structure(t) => Some(t.schema.session),
        }
    }
}

pub struct Tab {
    pub id: TabId,
    pub kind: TabKind,
}

/// Tree node identity, used for selection and expansion.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Session(SessionId),
    Database(SessionId, String),
    Collection(SessionId, String, String),
}

pub struct MongoRaabit {
    pub jobs: Jobs<MongoRaabit>,
    pub secrets: Arc<dyn SecretStore>,
    pub store: ProfileStore,
    pub store_path: Option<PathBuf>,
    pub sessions: Vec<Session>,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub manager: Option<ConnectionManager>,
    pub dialog: Option<Dialog>,
    pub expanded: HashSet<NodeKey>,
    pub selected: Option<NodeKey>,
    pub tree_filter: String,
    pub status: String,
    next_id: u64,
}

impl Default for MongoRaabit {
    fn default() -> Self {
        Self::new(ProfileStore::default_path())
    }
}

impl MongoRaabit {
    /// Create the app, loading saved connections from `store_path` (none = in memory).
    pub fn new(store_path: Option<PathBuf>) -> Self {
        let (store, status) = match store_path.as_deref().map(ProfileStore::load) {
            Some(Ok(store)) => (store, String::new()),
            Some(Err(err)) => (
                ProfileStore::default(),
                format!("Could not load connections: {err}"),
            ),
            None => (ProfileStore::default(), String::new()),
        };
        Self {
            jobs: Jobs::new(),
            secrets: Arc::new(MemorySecretStore::default()),
            store,
            store_path,
            sessions: Vec::new(),
            tabs: vec![Tab {
                id: 0,
                kind: TabKind::Welcome,
            }],
            active_tab: 0,
            manager: None,
            dialog: None,
            expanded: HashSet::new(),
            selected: None,
            tree_filter: String::new(),
            status,
            next_id: 1,
        }
    }

    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Apply finished background work. Call once per frame.
    pub fn poll(&mut self) {
        for apply in self.jobs.take() {
            apply(self);
        }
    }

    /// Block until background work settles (tests, scripted screenshots).
    pub fn wait_idle(&mut self, timeout: std::time::Duration) {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            self.poll();
            if !self.jobs.busy() || std::time::Instant::now() > deadline {
                self.poll();
                if !self.jobs.busy() || std::time::Instant::now() > deadline {
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    pub fn save_store(&mut self) {
        if let Some(path) = &self.store_path
            && let Err(err) = self.store.save(path)
        {
            self.status = format!("Could not save connections: {err}");
        }
    }

    pub fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn session_mut(&mut self, id: SessionId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub fn open_manager(&mut self) {
        if self.manager.is_none() {
            self.manager = Some(ConnectionManager::new(&self.store));
        }
    }

    // ----- connections -------------------------------------------------------------------

    /// Open a session for `profile` and load its databases.
    pub fn connect(&mut self, profile: ConnectionProfile) -> SessionId {
        let id = self.next_id();
        self.status = format!("Connecting to {}…", profile.name);
        self.sessions.push(Session {
            id,
            profile: profile.clone(),
            state: SessionState::Connecting,
            databases: Load::Idle,
        });
        self.expanded.insert(NodeKey::Session(id));
        self.selected = Some(NodeKey::Session(id));
        let secrets = self.secrets.clone();
        self.jobs.spawn(
            async move { Connection::connect(&profile, secrets.as_ref()).await },
            move |app: &mut MongoRaabit, result| {
                let Some(session) = app.session_mut(id) else {
                    return;
                };
                let name = session.profile.name.clone();
                let connected = match result {
                    Ok(conn) => {
                        let status = format!(
                            "Connected to {name} — MongoDB {} ({})",
                            conn.server().version,
                            conn.server().topology
                        );
                        session.state = SessionState::Connected(conn);
                        Some(status)
                    }
                    Err(err) => {
                        session.state = SessionState::Failed(err.to_string());
                        None
                    }
                };
                match connected {
                    Some(status) => {
                        app.status = status;
                        app.load_databases(id);
                    }
                    None => app.status = format!("Connection to {name} failed"),
                }
            },
        );
        id
    }

    pub fn disconnect(&mut self, id: SessionId) {
        if let Some(index) = self.sessions.iter().position(|s| s.id == id) {
            let session = self.sessions.remove(index);
            self.tabs.retain(|tab| tab.kind.session() != Some(id));
            self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
            self.status = format!("Disconnected from {}", session.profile.name);
            if let SessionState::Connected(conn) = session.state {
                self.jobs.spawn(conn.disconnect(), |_, ()| {});
            }
        }
    }

    pub fn load_databases(&mut self, id: SessionId) {
        let Some(session) = self.session_mut(id) else {
            return;
        };
        let Some(conn) = session.connection().cloned() else {
            return;
        };
        session.databases = Load::Loading;
        self.jobs.spawn(
            async move { conn.databases().await },
            move |app: &mut MongoRaabit, result| {
                let Some(session) = app.session_mut(id) else {
                    return;
                };
                session.databases = match result {
                    Ok(list) => Load::Ready(
                        list.into_iter()
                            .map(|info| DatabaseEntry {
                                info,
                                collections: Load::Idle,
                            })
                            .collect(),
                    ),
                    Err(err) => Load::Failed(err.to_string()),
                };
            },
        );
    }

    pub fn load_collections(&mut self, id: SessionId, db: &str) {
        let Some(session) = self.session_mut(id) else {
            return;
        };
        let Some(conn) = session.connection().cloned() else {
            return;
        };
        let Load::Ready(databases) = &mut session.databases else {
            return;
        };
        let Some(entry) = databases.iter_mut().find(|d| d.info.name == db) else {
            return;
        };
        entry.collections = Load::Loading;
        let db = db.to_owned();
        let db_for_apply = db.clone();
        self.jobs.spawn(
            async move {
                let collections = conn.collections(&db).await?;
                let mut out = Vec::new();
                for info in collections {
                    let stats = if matches!(
                        info.kind,
                        CollectionKind::Collection | CollectionKind::TimeSeries
                    ) {
                        conn.collection_stats(&db, &info.name).await.ok()
                    } else {
                        None
                    };
                    out.push(CollectionEntry { info, stats });
                }
                Ok::<_, mongo_raabit_core::RaabitError>(out)
            },
            move |app: &mut MongoRaabit, result| {
                let Some(session) = app.session_mut(id) else {
                    return;
                };
                if let Load::Ready(databases) = &mut session.databases
                    && let Some(entry) = databases.iter_mut().find(|d| d.info.name == db_for_apply)
                {
                    entry.collections = match result {
                        Ok(list) => Load::Ready(list),
                        Err(err) => Load::Failed(err.to_string()),
                    };
                }
            },
        );
    }

    // ----- tabs --------------------------------------------------------------------------

    pub fn active_tab_id(&self) -> Option<TabId> {
        self.tabs.get(self.active_tab).map(|t| t.id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn collection_tab_mut(&mut self, id: TabId) -> Option<&mut CollectionTab> {
        match &mut self.tab_mut(id)?.kind {
            TabKind::Collection(tab) => Some(tab),
            _ => None,
        }
    }

    pub fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs.remove(index);
            if self.active_tab >= index && self.active_tab > 0 {
                self.active_tab -= 1;
            }
        }
    }

    /// Focus the tab of `db.collection` or open a new one, then run its query.
    pub fn open_collection(
        &mut self,
        session: SessionId,
        db: &str,
        collection: &str,
        kind: CollectionKind,
    ) -> TabId {
        let existing = self.tabs.iter().position(|t| {
            matches!(&t.kind, TabKind::Collection(c) if c.session == session && c.db == db && c.collection == collection)
        });
        if let Some(index) = existing {
            self.active_tab = index;
            return self.tabs[index].id;
        }
        let id = self.next_id();
        self.tabs.push(Tab {
            id,
            kind: TabKind::Collection(Box::new(CollectionTab::new(session, db, collection, kind))),
        });
        self.active_tab = self.tabs.len() - 1;
        self.run_query(id, true);
        id
    }

    /// Kind of `db.collection` from the loaded tree metadata (plain collection if unknown).
    pub fn collection_kind(
        &self,
        session: SessionId,
        db: &str,
        collection: &str,
    ) -> CollectionKind {
        self.session(session)
            .and_then(|s| s.databases.ready())
            .and_then(|dbs| dbs.iter().find(|d| d.info.name == db))
            .and_then(|d| d.collections.ready())
            .and_then(|colls| colls.iter().find(|c| c.info.name == collection))
            .map(|c| c.info.kind)
            .unwrap_or(CollectionKind::Collection)
    }

    /// First collection of `db` in the loaded tree metadata.
    pub fn first_collection(&self, session: SessionId, db: &str) -> Option<String> {
        self.session(session)?
            .databases
            .ready()?
            .iter()
            .find(|d| d.info.name == db)?
            .collections
            .ready()?
            .first()
            .map(|c| c.info.name.clone())
    }

    /// Run (or re-page) the query of a collection tab. `recount` refreshes the total.
    pub fn run_query(&mut self, tab_id: TabId, recount: bool) {
        let Some(tab) = self.collection_tab_mut(tab_id) else {
            return;
        };
        let query = match tab.query.parse() {
            Ok(query) => query,
            Err(err) => {
                tab.query_error = Some(err.to_string());
                return;
            }
        };
        tab.query_error = None;
        tab.seq += 1;
        let seq = tab.seq;
        let (session_id, db, coll, page, page_size) = (
            tab.session,
            tab.db.clone(),
            tab.collection.clone(),
            tab.page,
            tab.page_size,
        );
        if recount || tab.ran.as_ref() != Some(&query) {
            tab.count = Load::Loading;
        }
        let recount = tab.count.is_loading();
        tab.result = Load::Loading;
        tab.ran = Some(query.clone());
        let Some(conn) = self
            .session(session_id)
            .and_then(Session::connection)
            .cloned()
        else {
            if let Some(tab) = self.collection_tab_mut(tab_id) {
                tab.result = Load::Failed("not connected".into());
            }
            return;
        };
        {
            let conn = conn.clone();
            let (db, coll, query) = (db.clone(), coll.clone(), query.clone());
            self.jobs.spawn(
                async move { conn.find_page(&db, &coll, &query, page, page_size).await },
                move |app: &mut MongoRaabit, result| {
                    if let Some(tab) = app.collection_tab_mut(tab_id)
                        && tab.seq == seq
                    {
                        tab.result = match result {
                            Ok(page) => Load::Ready(page),
                            Err(err) => Load::Failed(err.to_string()),
                        };
                    }
                },
            );
        }
        if recount {
            self.jobs.spawn(
                async move { conn.count(&db, &coll, &query).await },
                move |app: &mut MongoRaabit, result| {
                    if let Some(tab) = app.collection_tab_mut(tab_id)
                        && tab.seq == seq
                    {
                        tab.count = match result {
                            Ok(n) => Load::Ready(n),
                            Err(err) => Load::Failed(err.to_string()),
                        };
                    }
                },
            );
        }
    }

    // ----- documents ---------------------------------------------------------------------

    fn tab_context(&mut self, tab_id: TabId) -> Option<(Connection, String, String)> {
        let tab = self.collection_tab_mut(tab_id)?;
        let (session, db, coll) = (tab.session, tab.db.clone(), tab.collection.clone());
        let conn = self.session(session)?.connection()?.clone();
        Some((conn, db, coll))
    }

    /// The selected document of a collection tab's current page.
    pub fn selected_document(&mut self, tab_id: TabId) -> Option<Document> {
        let tab = self.collection_tab_mut(tab_id)?;
        let index = tab.selected?;
        tab.result.ready()?.documents.get(index).cloned()
    }

    pub fn edit_selected(&mut self, tab_id: TabId) {
        if let Some(doc) = self.selected_document(tab_id) {
            self.dialog = Some(Dialog::Document(DocumentEditor::edit(tab_id, &doc)));
        }
    }

    pub fn add_document(&mut self, tab_id: TabId) {
        self.dialog = Some(Dialog::Document(DocumentEditor::insert(tab_id, None)));
    }

    pub fn clone_selected(&mut self, tab_id: TabId) {
        if let Some(mut doc) = self.selected_document(tab_id) {
            doc.remove("_id");
            self.dialog = Some(Dialog::Document(DocumentEditor::insert(tab_id, Some(&doc))));
        }
    }

    pub fn request_delete(&mut self, tab_id: TabId) {
        if let Some(id) = self
            .selected_document(tab_id)
            .and_then(|d| d.get("_id").cloned())
        {
            self.dialog = Some(Dialog::Delete(ConfirmDelete {
                tab: tab_id,
                ids: vec![id],
                busy: false,
            }));
        }
    }

    /// Save the document editor's content (replace or insert), then refresh the page.
    pub fn save_document(&mut self, tab_id: TabId, mode: DocumentMode, doc: Document) {
        let Some((conn, db, coll)) = self.tab_context(tab_id) else {
            return;
        };
        self.jobs.spawn(
            async move {
                match mode {
                    DocumentMode::Edit(id) => conn
                        .replace_document(&db, &coll, &id, doc)
                        .await
                        .map(|()| "Document saved".to_owned()),
                    DocumentMode::Insert => {
                        conn.insert_documents(&db, &coll, vec![doc])
                            .await
                            .map(|ids| {
                                format!(
                                    "Inserted document {}",
                                    ids.first()
                                        .map(|id| shell::to_shell_string(
                                            id,
                                            shell::ShellStyle::Compact
                                        ))
                                        .unwrap_or_default()
                                )
                            })
                    }
                }
            },
            move |app: &mut MongoRaabit, result| match result {
                Ok(message) => {
                    app.status = message;
                    app.dialog = None;
                    app.run_query(tab_id, true);
                }
                Err(err) => {
                    if let Some(Dialog::Document(editor)) = &mut app.dialog {
                        editor.saving = false;
                        editor.error = Some(err.to_string());
                    }
                }
            },
        );
    }

    pub fn delete_documents(&mut self, tab_id: TabId, ids: Vec<Bson>) {
        let Some((conn, db, coll)) = self.tab_context(tab_id) else {
            return;
        };
        self.jobs.spawn(
            async move { conn.delete_documents(&db, &coll, &ids).await },
            move |app: &mut MongoRaabit, result| {
                app.dialog = None;
                match result {
                    Ok(n) => {
                        app.status =
                            format!("Deleted {n} document{}", if n == 1 { "" } else { "s" });
                        if let Some(tab) = app.collection_tab_mut(tab_id) {
                            tab.selected = None;
                        }
                        app.run_query(tab_id, true);
                    }
                    Err(err) => app.status = format!("Delete failed: {err}"),
                }
            },
        );
    }

    /// Commit an inline cell edit with `$set`.
    pub fn commit_cell_edit(&mut self, tab_id: TabId) {
        let Some(tab) = self.collection_tab_mut(tab_id) else {
            return;
        };
        let Some(edit) = tab.editing.take() else {
            return;
        };
        let Some(id) = tab
            .result
            .ready()
            .and_then(|page| page.documents.get(edit.doc_index))
            .and_then(|doc| doc.get("_id").cloned())
        else {
            return;
        };
        let value = match (&edit.original, shell::parse_value(&edit.text)) {
            (Some(Bson::String(_)), _) => Bson::String(edit.text.clone()),
            (_, Ok(value)) => value,
            (_, Err(err)) => {
                self.status = format!("Not saved: {err}");
                return;
            }
        };
        if edit.original.as_ref() == Some(&value) {
            return;
        }
        let Some((conn, db, coll)) = self.tab_context(tab_id) else {
            return;
        };
        let path = edit.path.clone();
        self.jobs.spawn(
            async move { conn.set_field(&db, &coll, &id, &path, Some(value)).await },
            move |app: &mut MongoRaabit, result| match result {
                Ok(()) => {
                    app.status = format!("Updated {}", edit.path);
                    app.run_query(tab_id, false);
                }
                Err(err) => app.status = format!("Update failed: {err}"),
            },
        );
    }

    pub fn explain(&mut self, tab_id: TabId) {
        let Some(tab) = self.collection_tab_mut(tab_id) else {
            return;
        };
        let query = match tab.query.parse() {
            Ok(q) => q,
            Err(err) => {
                tab.query_error = Some(err.to_string());
                return;
            }
        };
        let title = format!("Explain  {}.{}", tab.db, tab.collection);
        let Some((conn, db, coll)) = self.tab_context(tab_id) else {
            return;
        };
        self.dialog = Some(Dialog::Explain(ExplainDialog {
            title,
            result: Load::Loading,
            show_raw: false,
        }));
        self.jobs.spawn(
            async move { conn.explain(&db, &coll, &query).await },
            move |app: &mut MongoRaabit, result| {
                if let Some(Dialog::Explain(dialog)) = &mut app.dialog {
                    dialog.result = match result {
                        Ok(plan) => Load::Ready(plan),
                        Err(err) => Load::Failed(err.to_string()),
                    };
                }
            },
        );
    }

    pub fn query_code(&mut self, tab_id: TabId) {
        let Some(tab) = self.collection_tab_mut(tab_id) else {
            return;
        };
        let query = match tab.query.parse() {
            Ok(q) => q,
            Err(err) => {
                tab.query_error = Some(err.to_string());
                return;
            }
        };
        let (session, db, coll) = (tab.session, tab.db.clone(), tab.collection.clone());
        // Never put real passwords into generated code.
        let uri = self
            .session(session)
            .map(|s| s.profile.display_uri().replace(":****@", ":<password>@"))
            .unwrap_or_else(|| "mongodb://localhost:27017".into());
        self.dialog = Some(Dialog::Code(CodeDialog {
            title: format!("Query Code  {db}.{coll}"),
            shell: query.to_shell(&coll),
            request: CodeRequest {
                uri,
                database: db,
                collection: coll,
                operation: Operation::Find(query),
            },
            language: None,
        }));
    }
}
