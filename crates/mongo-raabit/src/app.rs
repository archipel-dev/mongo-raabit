//! View state of mongo-raabit and the operations that load data in the background.

use crate::dialogs::{
    CodeDialog, ConfirmDelete, Dialog, DocumentEditor, DocumentMode, ExplainDialog,
};
use crate::jobs::Jobs;
use crate::manager::ConnectionManager;
use crate::writes::{HostRequest, PendingWrite, WriteOrigin};
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::codegen::{CodeRequest, Operation};
use mongo_raabit_core::connection::{
    CollectionInfo, CollectionKind, CollectionStats, DatabaseInfo, ServerInfo,
};
use mongo_raabit_core::profile::{ConnectionProfile, ProfileStore};
use mongo_raabit_core::query::{Page, Query, QueryText};
use mongo_raabit_core::secrets::{MemorySecretStore, SecretStore};
use mongo_raabit_core::{Affected, ConnectOptions, Connection, RaabitError, Settings, Write};
use mongo_raabit_core::{documents, status, tabs};
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

/// How the Query tool window writes the query of a collection tab: with the visual builder
/// or as shell text (filter, projection, sort, skip and limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryMode {
    #[default]
    Builder,
    Code,
}

/// What the center island shows for a collection tab: the documents of its query, the
/// output of its aggregation pipeline, its schema, or the SQL and shell consoles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CenterView {
    #[default]
    Documents,
    Aggregation,
    Schema,
    Sql,
    Shell,
}

/// The content tabs of the right tool window: the tools that act on the active
/// collection tab. Their order is the order of [`ToolTab::TITLES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolTab {
    #[default]
    Query,
    Aggregation,
    Indexes,
}

impl ToolTab {
    pub const ALL: [Self; 3] = [Self::Query, Self::Aggregation, Self::Indexes];
    pub const TITLES: [&'static str; 3] = ["Query", "Pipeline", "Indexes"];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
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
    /// What the center island shows.
    pub center: CenterView,
    /// Visual query builder state (Builder mode).
    pub builder: crate::query_builder::QueryBuilder,
    /// SQL view state.
    pub sql: crate::tools::SqlTab,
    /// Shell view state, created when the view is first opened.
    pub shell: Option<Box<crate::tools::ShellTab>>,
    /// Aggregation pipeline (Aggregation tool window) and its output, created on first use.
    pub aggregation: Option<Box<crate::tools::AggregationTab>>,
    /// Schema sample (fields of the Query tool window, the Schema view) and indexes
    /// (Indexes tool window), created on first use.
    pub structure: Option<Box<crate::tools::StructureTab>>,
    /// The fields the Query tool window lists, kept until the sample or the page changes.
    pub fields: crate::tool_window::FieldsCache,
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
            center: CenterView::default(),
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
            aggregation: None,
            structure: None,
            fields: Default::default(),
        }
    }
}

/// The editor tabs of the center island: the welcome page and one tab per opened
/// collection. The tools for a collection (query builder, pipeline, indexes) live in the
/// right tool window and act on the active collection tab.
pub enum TabKind {
    Welcome,
    Collection(Box<CollectionTab>),
}

impl TabKind {
    /// The session a tab belongs to (`None` for the welcome tab).
    pub fn session(&self) -> Option<SessionId> {
        match self {
            Self::Welcome => None,
            Self::Collection(t) => Some(t.session),
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
    /// Island sizes and visibility of the window.
    pub layout: archipel_sdk::kit::components::WorkbenchState,
    /// The tool the right tool window shows (kept here: the layout is lent to the window
    /// while it draws).
    pub tool: ToolTab,
    /// A tool asked for during a frame ([`MongoRaabit::show_tool`]), applied to the layout
    /// after it.
    pub tool_request: Option<ToolTab>,
    /// The user's settings; the plugin refreshes them from the host every frame.
    pub settings: Settings,
    /// Requests for the host (store a setting, open Settings), taken by the plugin.
    pub host_requests: Vec<HostRequest>,
    /// A write the user just confirmed, run after the dialogs of this frame, with the
    /// count its confirmation showed (to check the outcome against).
    pub confirmed: Option<(PendingWrite, Option<Affected>)>,
    /// When `Some`, every write handed to execution is appended here, exactly as it is
    /// executed. Off (`None`) unless a test turns it on: it observes the write path, it
    /// does not change it.
    pub write_log: Option<Vec<Write>>,
    next_id: u64,
}

/// Extra time the views give a read beyond the query timeout: the server stops it at the
/// timeout (`maxTimeMS`) and reports that; this only covers a server that does not answer.
const BOUND_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Run `future`, failing when it takes longer than `limit` (the query timeout setting) and
/// a grace period. Reads also carry `limit` to the server as `maxTimeMS`
/// ([`Connection::set_query_timeout`]), so the server stops them first and nothing keeps
/// running after the view gave up.
pub(crate) async fn bounded<T>(
    limit: std::time::Duration,
    future: impl std::future::Future<Output = mongo_raabit_core::Result<T>>,
) -> mongo_raabit_core::Result<T> {
    tokio::time::timeout(limit + BOUND_GRACE, future)
        .await
        .unwrap_or_else(|_| {
            Err(RaabitError::Invalid(format!(
                "did not finish within {} s (Settings › Query timeout)",
                limit.as_secs()
            )))
        })
}

/// The islands at their first start: the connections on the left, the data in the
/// middle and the collection tools on the right, wide enough for the query builder.
pub fn default_layout() -> archipel_sdk::kit::components::WorkbenchState {
    archipel_sdk::kit::components::WorkbenchState {
        left_ratio: 0.18,
        center_ratio: 0.62,
        ..Default::default()
    }
}

impl Default for MongoRaabit {
    fn default() -> Self {
        Self::new(ProfileStore::default_path())
    }
}

impl MongoRaabit {
    /// Create the app, loading saved connections from `store_path` (none = in memory).
    pub fn new(store_path: Option<PathBuf>) -> Self {
        // A connections file that cannot be read is backed up and never saved over: the
        // store stays in memory for this run (`store_path` is dropped).
        let (store, status, store_path) = match store_path.as_deref().map(ProfileStore::open) {
            Some(opened) if opened.writable => (opened.store, String::new(), store_path),
            Some(opened) => (opened.store, opened.problem.unwrap_or_default(), None),
            None => (ProfileStore::default(), String::new(), None),
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
            layout: default_layout(),
            tool: ToolTab::default(),
            tool_request: None,
            settings: Settings::default(),
            host_requests: Vec::new(),
            confirmed: None,
            write_log: None,
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

    /// Give every open connection the query timeout setting: its queries, counts and
    /// aggregations carry it to the server as `maxTimeMS`.
    pub fn sync_query_timeout(&mut self) {
        let limit = self.settings.query_timeout();
        for conn in self.sessions.iter().filter_map(Session::connection) {
            if conn.query_timeout() != limit {
                conn.set_query_timeout(limit);
            }
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
        self.status = status::connecting(&profile.name);
        self.sessions.push(Session {
            id,
            profile: profile.clone(),
            state: SessionState::Connecting,
            databases: Load::Idle,
        });
        self.expanded.insert(NodeKey::Session(id));
        self.selected = Some(NodeKey::Session(id));
        let secrets = self.secrets.clone();
        let options = ConnectOptions {
            read_only: self.settings.new_connections_read_only,
            query_timeout: Some(self.settings.query_timeout()),
        };
        self.jobs.spawn(
            async move { Connection::connect_with(&profile, secrets.as_ref(), options).await },
            move |app: &mut MongoRaabit, result| {
                let Some(session) = app.session_mut(id) else {
                    return;
                };
                let name = session.profile.name.clone();
                let connected = match result {
                    Ok(conn) => {
                        let status = status::connected(&name, conn.server());
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
                    None => app.status = status::connection_failed(&name),
                }
            },
        );
        id
    }

    pub fn disconnect(&mut self, id: SessionId) {
        if let Some(index) = self.sessions.iter().position(|s| s.id == id) {
            let session = self.sessions.remove(index);
            self.tabs.retain(|tab| tab.kind.session() != Some(id));
            self.active_tab = tabs::clamp_active(self.active_tab, self.tabs.len());
            self.status = status::disconnected(&session.profile.name);
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

    /// The user picked `tool` in the right tool window: the center follows where the tool
    /// has output of its own (a pipeline's output), and comes back to the documents from it.
    pub fn tool_picked(&mut self, tool: ToolTab) {
        let Some(id) = self.active_collection_tab() else {
            return;
        };
        if tool == ToolTab::Aggregation {
            if self.ensure_aggregation(id) {
                self.preview_stage(id);
            }
            if let Some(tab) = self.collection_tab_mut(id) {
                tab.center = CenterView::Aggregation;
            }
        } else if let Some(tab) = self.collection_tab_mut(id)
            && tab.center == CenterView::Aggregation
        {
            tab.center = CenterView::Documents;
        }
    }

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
            self.active_tab = tabs::active_after_close(self.active_tab, index);
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
        let mut tab = CollectionTab::new(session, db, collection, kind);
        tab.page_size = u64::from(self.settings.page_size);
        self.tabs.push(Tab {
            id,
            kind: TabKind::Collection(Box::new(tab)),
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
                if recount {
                    tab.count = Load::Failed("not connected".into());
                }
            }
            return;
        };
        let limit = self.settings.query_timeout();
        {
            let conn = conn.clone();
            let (db, coll, query) = (db.clone(), coll.clone(), query.clone());
            self.jobs.spawn(
                async move {
                    bounded(limit, conn.find_page(&db, &coll, &query, page, page_size)).await
                },
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
                async move { bounded(limit, conn.count(&db, &coll, &query)).await },
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
        // A projected document is not the whole document: saving it would replace the
        // stored one with a part of it (the Save's guard would refuse it as changed).
        if self
            .collection_tab_mut(tab_id)
            .and_then(|tab| tab.ran.as_ref())
            .is_some_and(|query| query.projection.is_some())
        {
            self.status = "Clear the projection to edit whole documents".into();
            return;
        }
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

    /// Delete the selected document: through the write confirmation when it is on, else
    /// after a plain "Delete document?" question (deletes always ask).
    pub fn request_delete(&mut self, tab_id: TabId) {
        if let Some(id) = self
            .selected_document(tab_id)
            .and_then(|d| d.get("_id").cloned())
        {
            if self.settings.confirm_writes {
                self.delete_documents(tab_id, vec![id]);
            } else {
                self.dialog = Some(Dialog::Delete(ConfirmDelete {
                    tab: tab_id,
                    ids: vec![id],
                    busy: false,
                }));
            }
        }
    }

    /// The session, database and collection of a collection tab.
    fn tab_namespace(&mut self, tab_id: TabId) -> Option<(SessionId, String, String)> {
        let tab = self.collection_tab_mut(tab_id)?;
        Some((tab.session, tab.db.clone(), tab.collection.clone()))
    }

    /// Save the document editor's content (replace or insert), then refresh the page.
    /// `original` is the document as the editor loaded it: a replace only applies while
    /// the stored document is still exactly that (see `write::replace_filter`).
    pub fn save_document(
        &mut self,
        tab_id: TabId,
        mode: DocumentMode,
        original: Option<Document>,
        doc: Document,
    ) {
        let Some((session, db, collection)) = self.tab_namespace(tab_id) else {
            return;
        };
        let write = match (mode, original) {
            (DocumentMode::Edit(id), Some(original)) => Write::ReplaceDocument {
                db,
                collection,
                id,
                original,
                replacement: doc,
            },
            (DocumentMode::Edit(_), None) => {
                let error = RaabitError::Invalid(
                    "the document as it was loaded is unknown; reopen it to edit".into(),
                );
                self.write_done(WriteOrigin::Document(tab_id), None, None, Err(error));
                return;
            }
            (DocumentMode::Insert, _) => Write::InsertDocuments {
                db,
                collection,
                documents: vec![doc],
            },
        };
        self.request_write(PendingWrite {
            session,
            write,
            origin: WriteOrigin::Document(tab_id),
        });
    }

    pub fn delete_documents(&mut self, tab_id: TabId, ids: Vec<Bson>) {
        let Some((session, db, collection)) = self.tab_namespace(tab_id) else {
            return;
        };
        self.request_write(PendingWrite {
            session,
            write: Write::DeleteDocuments {
                db,
                collection,
                ids,
            },
            origin: WriteOrigin::Delete(tab_id),
        });
    }

    /// Commit an inline cell edit: a `$set` of the field on the document's `_id`, only
    /// while the field still has the value that was loaded (see `write::set_field_filter`).
    pub fn commit_cell_edit(&mut self, tab_id: TabId) {
        let Some(tab) = self.collection_tab_mut(tab_id) else {
            return;
        };
        let Some(edit) = tab.editing.take() else {
            return;
        };
        let Some(loaded) = tab
            .result
            .ready()
            .and_then(|page| page.documents.get(edit.doc_index))
        else {
            return;
        };
        let Some(id) = loaded.get("_id").cloned() else {
            return;
        };
        // The guard compares with the value as the page loaded it.
        let original = documents::value_at(loaded, &edit.path).cloned();
        let value = match documents::cell_edit_value(edit.original.as_ref(), &edit.text) {
            Ok(Some(value)) => value,
            Ok(None) => return,
            Err(err) => {
                self.status = format!("Not saved: {err}");
                return;
            }
        };
        let Some((session, db, collection)) = self.tab_namespace(tab_id) else {
            return;
        };
        self.request_write(PendingWrite {
            session,
            write: Write::SetField {
                db,
                collection,
                id,
                path: edit.path,
                value: Some(value),
                original,
            },
            origin: WriteOrigin::Cell(tab_id),
        });
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
        let uri = status::code_uri(self.session(session).map(|s| &s.profile));
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
