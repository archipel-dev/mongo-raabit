//! State and background operations of the collection tools: the Aggregation Editor, the
//! SQL and shell views, the Schema Explorer and the Index Manager. Each lives on the
//! collection tab it belongs to; the right tool window and the center island draw it.

use crate::app::bounded;
use crate::app::{CenterView, Load, MongoRaabit, ResultView, SessionId, TabId, ToolTab};
use crate::writes::{PendingWrite, WriteOrigin};
use mongo_raabit_core::aggregate::{Pipeline, RunOptions, Stage, StagePreview};
use mongo_raabit_core::bson::Document;
use mongo_raabit_core::connection::IndexInfo;
use mongo_raabit_core::schema::{SampleMethod, SchemaReport};
use mongo_raabit_core::shell_eval;
use mongo_raabit_core::sql::{self, SqlTranslation};
use mongo_raabit_core::tabs;
use mongo_raabit_core::{Connection, Write};
use std::collections::HashSet;
use std::time::Duration;

/// Results of a pipeline or SQL run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunResult {
    pub documents: Vec<Document>,
    pub elapsed: Duration,
}

/// Maximum documents fetched by "Run" in the aggregation and SQL tabs.
pub const RUN_LIMIT: u64 = 500;

pub struct AggregationTab {
    pub session: SessionId,
    pub db: String,
    pub collection: String,
    pub pipeline: Pipeline,
    pub selected: usize,
    pub preview: Load<StagePreview>,
    pub preview_seq: u64,
    pub preview_limit: u64,
    /// Input sample size for previews (0 = whole collection).
    pub sample: u64,
    pub result: Load<RunResult>,
    pub view: ResultView,
    pub open: HashSet<String>,
    pub stage_error: Option<String>,
    /// Stages (by index) the user switched to the code editor.
    pub stage_code: HashSet<usize>,
    /// Stage whose switch to the form was refused because the form cannot show it.
    pub form_hint: Option<usize>,
}

impl AggregationTab {
    /// Insert a stage at `at`, keeping per-stage editor modes aligned.
    pub fn insert_stage(&mut self, at: usize, stage: Stage) {
        let at = at.min(self.pipeline.stages.len());
        self.pipeline.stages.insert(at, stage);
        self.remap_stages(|i| Some(tabs::after_insert(i, at)));
    }

    /// Remove stage `index`, keeping per-stage editor modes aligned.
    pub fn remove_stage(&mut self, index: usize) {
        if index >= self.pipeline.stages.len() {
            return;
        }
        self.pipeline.stages.remove(index);
        self.remap_stages(|i| tabs::after_remove(i, index));
    }

    /// Move a stage, keeping per-stage editor modes aligned.
    pub fn move_stage(&mut self, from: usize, to: usize) {
        let len = self.pipeline.stages.len();
        if from >= len || to >= len {
            return;
        }
        self.pipeline.move_stage(from, to);
        self.remap_stages(|i| Some(tabs::after_move(i, from, to)));
    }

    fn remap_stages(&mut self, map: impl Fn(usize) -> Option<usize>) {
        self.stage_code = self.stage_code.iter().filter_map(|&i| map(i)).collect();
        self.form_hint = self.form_hint.and_then(&map);
    }
}

pub struct SqlTab {
    pub session: SessionId,
    pub db: String,
    pub sql: String,
    pub result: Load<RunResult>,
    pub view: ResultView,
    pub open: HashSet<String>,
}

impl SqlTab {
    /// Live translation of the current SQL text.
    pub fn translation(&self) -> Result<SqlTranslation, String> {
        sql::translate(&self.sql).map_err(|e| e.to_string())
    }
}

pub struct SchemaTab {
    pub session: SessionId,
    pub db: String,
    pub collection: String,
    pub size: u64,
    pub method: SampleMethod,
    pub report: Load<SchemaReport>,
    pub selected: Option<usize>,
    /// Counts the reports received, so views can tell a new one from the last.
    pub generation: u64,
}

pub use mongo_raabit_core::indexes::IndexForm;

/// Schema Explorer and Index Manager of one collection.
pub struct StructureTab {
    pub schema: SchemaTab,
    pub indexes: IndexTab,
}

pub struct IndexTab {
    pub session: SessionId,
    pub db: String,
    pub collection: String,
    pub indexes: Load<Vec<IndexInfo>>,
    pub form: IndexForm,
    pub form_open: bool,
    pub error: Option<String>,
    pub busy: bool,
    /// Index awaiting a second click to drop.
    pub confirm_drop: Option<String>,
}

impl MongoRaabit {
    pub(crate) fn connection_for(&self, session: SessionId) -> Option<Connection> {
        self.session(session)?.connection().cloned()
    }

    /// Shows `tool` in the right tool window (opening the window when it was hidden).
    pub fn show_tool(&mut self, tool: ToolTab) {
        self.tool = tool;
        self.tool_request = Some(tool);
        self.apply_tool_request();
    }

    /// Puts a requested tool into the layout (after a frame, when the layout is back).
    pub(crate) fn apply_tool_request(&mut self) {
        if let Some(tool) = self.tool_request {
            self.layout.tool_tabs[1] = tool.index();
            self.layout.right_hidden = false;
            self.layout.maximized = None;
        }
    }

    /// The tool shown in the right tool window.
    pub fn current_tool(&self) -> ToolTab {
        self.tool
    }

    /// The id of the active tab when it is a collection tab.
    pub fn active_collection_tab(&self) -> Option<TabId> {
        let tab = self.tabs.get(self.active_tab)?;
        matches!(tab.kind, crate::app::TabKind::Collection(_)).then_some(tab.id)
    }

    /// Focuses (or opens) the collection tab of `db.collection`, without rerunning it.
    fn collection_for_tool(&mut self, session: SessionId, db: &str, collection: &str) -> TabId {
        let kind = self.collection_kind(session, db, collection);
        self.open_collection(session, db, collection, kind)
    }

    // ----- aggregation ------------------------------------------------------------------

    /// Opens the pipeline of `db.collection` in the Aggregation tool window, its output in
    /// the center; `pipeline` replaces the tab's pipeline (from SQL, for example).
    pub fn open_aggregation(
        &mut self,
        session: SessionId,
        db: &str,
        collection: &str,
        pipeline: Option<Pipeline>,
    ) -> TabId {
        let id = self.collection_for_tool(session, db, collection);
        let created = self.ensure_aggregation(id);
        if let Some(pipeline) = pipeline
            && let Some(tab) = self.aggregation_tab_mut(id)
        {
            tab.pipeline = pipeline;
            tab.selected = 0;
            tab.stage_code.clear();
            tab.form_hint = None;
            tab.result = Load::Idle;
            tab.stage_error = None;
            self.preview_stage(id);
        } else if created {
            self.preview_stage(id);
        }
        if let Some(tab) = self.collection_tab_mut(id) {
            tab.center = CenterView::Aggregation;
        }
        self.show_tool(ToolTab::Aggregation);
        id
    }

    /// Creates the aggregation state of a collection tab on first use (one `$match` stage).
    /// Returns true when it was created.
    pub fn ensure_aggregation(&mut self, id: TabId) -> bool {
        let Some(tab) = self.collection_tab_mut(id) else {
            return false;
        };
        if tab.aggregation.is_some() {
            return false;
        }
        tab.aggregation = Some(Box::new(AggregationTab {
            session: tab.session,
            db: tab.db.clone(),
            collection: tab.collection.clone(),
            pipeline: Pipeline {
                stages: vec![Stage::new("$match")],
            },
            selected: 0,
            preview: Load::Idle,
            preview_seq: 0,
            preview_limit: 20,
            sample: 1000,
            result: Load::Idle,
            view: ResultView::Tree,
            open: HashSet::new(),
            stage_error: None,
            stage_code: HashSet::new(),
            form_hint: None,
        }));
        true
    }

    pub fn aggregation_tab_mut(&mut self, id: TabId) -> Option<&mut AggregationTab> {
        self.collection_tab_mut(id)?.aggregation.as_deref_mut()
    }

    /// Recompute the input/output preview of the selected stage.
    pub fn preview_stage(&mut self, tab_id: TabId) {
        let Some(tab) = self.aggregation_tab_mut(tab_id) else {
            return;
        };
        if tab.pipeline.stages.is_empty() {
            tab.preview = Load::Idle;
            return;
        }
        tab.selected = tab.selected.min(tab.pipeline.stages.len() - 1);
        if let Err(err) = tab.pipeline.documents(Some(tab.selected)) {
            tab.stage_error = Some(err.to_string());
            return;
        }
        tab.stage_error = None;
        tab.preview_seq += 1;
        tab.preview = Load::Loading;
        let (seq, session, db, coll, pipeline, index, limit, sample) = (
            tab.preview_seq,
            tab.session,
            tab.db.clone(),
            tab.collection.clone(),
            tab.pipeline.clone(),
            tab.selected,
            tab.preview_limit,
            tab.sample,
        );
        let Some(conn) = self.connection_for(session) else {
            if let Some(tab) = self.aggregation_tab_mut(tab_id) {
                tab.preview = Load::Failed("not connected".into());
            }
            return;
        };
        let options = RunOptions {
            input_sample: sample,
            ..RunOptions::default()
        };
        self.jobs.spawn(
            async move {
                conn.preview_stage(&db, &coll, &pipeline, index, limit, options)
                    .await
            },
            move |app: &mut MongoRaabit, result| {
                if let Some(tab) = app.aggregation_tab_mut(tab_id)
                    && tab.preview_seq == seq
                {
                    tab.preview = match result {
                        Ok(preview) => Load::Ready(preview),
                        Err(err) => Load::Failed(err.to_string()),
                    };
                }
            },
        );
    }

    /// Run the whole pipeline (writing stages included) and show up to [`RUN_LIMIT`] results.
    pub fn run_pipeline(&mut self, tab_id: TabId) {
        let Some(tab) = self.aggregation_tab_mut(tab_id) else {
            return;
        };
        let stages = match tab.pipeline.documents(None) {
            Ok(stages) => stages,
            Err(err) => {
                tab.stage_error = Some(err.to_string());
                return;
            }
        };
        let (session, db, coll) = (tab.session, tab.db.clone(), tab.collection.clone());
        if mongo_raabit_core::write::pipeline_writes(&stages) {
            // `$out` / `$merge` change data: the guarded write path, with its confirmation.
            self.request_write(PendingWrite {
                session,
                write: Write::Aggregate {
                    db,
                    collection: coll,
                    pipeline: stages,
                },
                origin: WriteOrigin::Pipeline(tab_id),
            });
            return;
        }
        tab.result = Load::Loading;
        if let Some(tab) = self.collection_tab_mut(tab_id) {
            tab.center = CenterView::Aggregation;
        }
        let Some(conn) = self.connection_for(session) else {
            if let Some(tab) = self.aggregation_tab_mut(tab_id) {
                tab.result = Load::Failed("not connected".into());
            }
            return;
        };
        let limit = self.settings.query_timeout();
        self.jobs.spawn(
            async move {
                bounded(
                    limit,
                    conn.aggregate(&db, &coll, stages, RunOptions::default(), RUN_LIMIT),
                )
                .await
            },
            move |app: &mut MongoRaabit, result| {
                if let Some(tab) = app.aggregation_tab_mut(tab_id) {
                    tab.result = match result {
                        Ok((documents, elapsed)) => Load::Ready(RunResult { documents, elapsed }),
                        Err(err) => Load::Failed(err.to_string()),
                    };
                }
            },
        );
    }

    // ----- SQL --------------------------------------------------------------------------

    /// Opens the Query tab of `collection` in SQL mode. Without a collection (SQL on a
    /// database) the first collection of the database is used.
    pub fn open_sql(
        &mut self,
        session: SessionId,
        db: &str,
        collection: Option<&str>,
    ) -> Option<TabId> {
        let collection = match collection {
            Some(coll) => coll.to_owned(),
            None => self.first_collection(session, db)?,
        };
        let id = self.collection_for_tool(session, db, &collection);
        if let Some(tab) = self.collection_tab_mut(id) {
            tab.center = CenterView::Sql;
        }
        Some(id)
    }

    pub fn sql_tab_mut(&mut self, id: TabId) -> Option<&mut SqlTab> {
        self.collection_tab_mut(id).map(|tab| &mut tab.sql)
    }

    pub fn run_sql(&mut self, tab_id: TabId) {
        let Some(tab) = self.sql_tab_mut(tab_id) else {
            return;
        };
        let translation = match tab.translation() {
            Ok(t) => t,
            Err(err) => {
                tab.result = Load::Failed(err);
                return;
            }
        };
        tab.result = Load::Loading;
        let (session, db) = (tab.session, tab.db.clone());
        let Some(conn) = self.connection_for(session) else {
            if let Some(tab) = self.sql_tab_mut(tab_id) {
                tab.result = Load::Failed("not connected".into());
            }
            return;
        };
        let collection = translation.collection().to_owned();
        let pipeline = translation.pipeline();
        let limit = self.settings.query_timeout();
        self.jobs.spawn(
            async move {
                bounded(
                    limit,
                    conn.aggregate(&db, &collection, pipeline, RunOptions::default(), RUN_LIMIT),
                )
                .await
            },
            move |app: &mut MongoRaabit, result| {
                if let Some(tab) = app.sql_tab_mut(tab_id) {
                    tab.result = match result {
                        Ok((documents, elapsed)) => Load::Ready(RunResult { documents, elapsed }),
                        Err(err) => Load::Failed(err.to_string()),
                    };
                }
            },
        );
    }

    // ----- schema -----------------------------------------------------------------------

    /// Shows the schema of `db.collection` in the center.
    pub fn open_schema(&mut self, session: SessionId, db: &str, collection: &str) -> TabId {
        let id = self.collection_for_tool(session, db, collection);
        self.ensure_structure(id);
        if let Some(tab) = self.collection_tab_mut(id) {
            tab.center = CenterView::Schema;
        }
        id
    }

    /// Creates the schema and index state of a collection tab on first use, and starts
    /// sampling the schema and loading the indexes. Returns true when it was created.
    pub fn ensure_structure(&mut self, id: TabId) -> bool {
        let sample = u64::from(self.settings.schema_sample_size);
        let Some(tab) = self.collection_tab_mut(id) else {
            return false;
        };
        if tab.structure.is_some() {
            return false;
        }
        let (session, db, collection) = (tab.session, tab.db.clone(), tab.collection.clone());
        tab.structure = Some(Box::new(StructureTab {
            schema: SchemaTab {
                session,
                db: db.clone(),
                collection: collection.clone(),
                size: sample,
                method: SampleMethod::Random,
                report: Load::Idle,
                selected: None,
                generation: 0,
            },
            indexes: IndexTab {
                session,
                db,
                collection,
                indexes: Load::Idle,
                form: IndexForm {
                    keys: "{  : 1 }".into(),
                    ..IndexForm::default()
                },
                form_open: false,
                error: None,
                busy: false,
                confirm_drop: None,
            },
        }));
        self.analyze_schema(id);
        self.load_indexes(id);
        true
    }

    pub fn structure_tab_mut(&mut self, id: TabId) -> Option<&mut StructureTab> {
        self.collection_tab_mut(id)?.structure.as_deref_mut()
    }

    pub fn schema_tab_mut(&mut self, id: TabId) -> Option<&mut SchemaTab> {
        self.structure_tab_mut(id).map(|tab| &mut tab.schema)
    }

    pub fn analyze_schema(&mut self, tab_id: TabId) {
        let Some(tab) = self.schema_tab_mut(tab_id) else {
            return;
        };
        let (session, db, coll, size, method) = (
            tab.session,
            tab.db.clone(),
            tab.collection.clone(),
            tab.size,
            tab.method,
        );
        let Some(conn) = self.connection_for(session) else {
            if let Some(tab) = self.schema_tab_mut(tab_id) {
                tab.report = Load::Failed("not connected".into());
            }
            return;
        };
        if let Some(tab) = self.schema_tab_mut(tab_id) {
            tab.report = Load::Loading;
        }
        self.jobs.spawn(
            async move {
                conn.sample_schema(&db, &coll, size, method, Document::new())
                    .await
            },
            move |app: &mut MongoRaabit, result| {
                if let Some(tab) = app.schema_tab_mut(tab_id) {
                    tab.generation += 1;
                    tab.report = match result {
                        Ok(report) => {
                            tab.selected = report
                                .fields
                                .iter()
                                .position(|f| f.histogram.len() > 1 || f.top_values.len() > 1);
                            Load::Ready(report)
                        }
                        Err(err) => Load::Failed(err.to_string()),
                    };
                }
            },
        );
    }

    // ----- indexes ----------------------------------------------------------------------

    /// Shows the indexes of `db.collection` in the Indexes tool window.
    pub fn open_indexes(&mut self, session: SessionId, db: &str, collection: &str) -> TabId {
        let id = self.collection_for_tool(session, db, collection);
        self.ensure_structure(id);
        self.show_tool(ToolTab::Indexes);
        id
    }

    pub fn index_tab_mut(&mut self, id: TabId) -> Option<&mut IndexTab> {
        self.structure_tab_mut(id).map(|tab| &mut tab.indexes)
    }

    pub fn load_indexes(&mut self, tab_id: TabId) {
        let Some(tab) = self.index_tab_mut(tab_id) else {
            return;
        };
        let (session, db, coll) = (tab.session, tab.db.clone(), tab.collection.clone());
        let Some(conn) = self.connection_for(session) else {
            if let Some(tab) = self.index_tab_mut(tab_id) {
                tab.indexes = Load::Failed("not connected".into());
            }
            return;
        };
        if let Some(tab) = self.index_tab_mut(tab_id) {
            tab.indexes = Load::Loading;
        }
        self.jobs.spawn(
            async move { conn.indexes(&db, &coll).await },
            move |app: &mut MongoRaabit, result| {
                if let Some(tab) = app.index_tab_mut(tab_id) {
                    tab.indexes = match result {
                        Ok(list) => Load::Ready(list),
                        Err(err) => Load::Failed(err.to_string()),
                    };
                }
            },
        );
    }

    /// Ask for an index change; the list reloads after it ran.
    fn index_operation(
        &mut self,
        tab_id: TabId,
        done: String,
        write: impl FnOnce(String, String) -> Write,
    ) {
        let Some(tab) = self.index_tab_mut(tab_id) else {
            return;
        };
        let (session, db, coll) = (tab.session, tab.db.clone(), tab.collection.clone());
        self.request_write(PendingWrite {
            session,
            write: write(db, coll),
            origin: WriteOrigin::Index(tab_id, done),
        });
    }

    pub fn create_index(&mut self, tab_id: TabId) {
        let Some(tab) = self.index_tab_mut(tab_id) else {
            return;
        };
        let index = match tab.form.to_new_index() {
            Ok(index) => index,
            Err(err) => {
                tab.error = Some(err);
                return;
            }
        };
        self.index_operation(tab_id, "Index created".into(), move |db, collection| {
            Write::CreateIndex {
                db,
                collection,
                index,
            }
        });
    }

    pub fn drop_index(&mut self, tab_id: TabId, name: String) {
        self.index_operation(
            tab_id,
            format!("Dropped index {name}"),
            move |db, collection| Write::DropIndex {
                db,
                collection,
                name,
            },
        );
    }

    pub fn set_index_hidden(&mut self, tab_id: TabId, name: String, hidden: bool) {
        let verb = if hidden { "Hid" } else { "Unhid" };
        self.index_operation(
            tab_id,
            format!("{verb} index {name}"),
            move |db, collection| Write::SetIndexHidden {
                db,
                collection,
                name,
                hidden,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_modes_follow_stage_edits() {
        let mut tab = AggregationTab {
            session: SessionId::default(),
            db: "db".into(),
            collection: "c".into(),
            pipeline: Pipeline {
                stages: vec![
                    Stage::new("$match"),
                    Stage::new("$sort"),
                    Stage::new("$limit"),
                ],
            },
            selected: 0,
            preview: Load::Idle,
            preview_seq: 0,
            preview_limit: 20,
            sample: 0,
            result: Load::Idle,
            view: ResultView::Tree,
            open: HashSet::new(),
            stage_error: None,
            stage_code: HashSet::from([1]),
            form_hint: Some(2),
        };
        tab.insert_stage(0, Stage::new("$skip"));
        assert_eq!(tab.stage_code, HashSet::from([2]));
        assert_eq!(tab.form_hint, Some(3));
        tab.move_stage(2, 0);
        assert_eq!(tab.pipeline.stages[0].operator, "$sort");
        assert_eq!(tab.stage_code, HashSet::from([0]));
        assert_eq!(tab.form_hint, Some(3));
        tab.move_stage(0, 3);
        assert_eq!(tab.stage_code, HashSet::from([3]));
        assert_eq!(tab.form_hint, Some(2));
        tab.remove_stage(3);
        assert!(tab.stage_code.is_empty());
        assert_eq!(tab.form_hint, Some(2));
        tab.remove_stage(9);
        assert_eq!(tab.pipeline.stages.len(), 3);
    }
}

// ----- shell ------------------------------------------------------------------------------

/// How the shell tab evaluates scripts.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellEngine {
    Detecting,
    Mongosh {
        path: std::path::PathBuf,
        version: String,
    },
    /// mongosh is not on PATH.
    Builtin,
}

/// One run in the shell history.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellEntry {
    pub script: String,
    /// Console text (stdout and stderr, or per-statement results).
    pub output: String,
    pub ok: bool,
    pub elapsed: Duration,
    /// Parsed result for the tree view.
    pub value: Option<mongo_raabit_core::bson::Bson>,
    pub engine: &'static str,
}

pub struct ShellTab {
    pub session: SessionId,
    pub db: String,
    pub script: String,
    pub engine: ShellEngine,
    /// Use the built-in evaluator even when mongosh exists.
    pub force_builtin: bool,
    pub history: Vec<ShellEntry>,
    pub running: bool,
    pub open: HashSet<String>,
    pub collections: Vec<String>,
}

/// Results as documents for the tree view (see [`result_documents`]).
pub use mongo_raabit_core::shell::result_documents as shell_documents;

impl MongoRaabit {
    /// Opens the Query tab of `collection` in Shell mode. Without a collection the first
    /// collection of the database is used.
    pub fn open_shell(
        &mut self,
        session: SessionId,
        db: &str,
        collection: Option<&str>,
    ) -> Option<TabId> {
        let collection = match collection {
            Some(coll) => coll.to_owned(),
            None => self.first_collection(session, db)?,
        };
        let id = self.collection_for_tool(session, db, &collection);
        if let Some(tab) = self.collection_tab_mut(id) {
            tab.center = CenterView::Shell;
        }
        self.init_shell(id);
        Some(id)
    }

    /// Creates the shell state of a Query tab on first use and detects mongosh.
    pub fn init_shell(&mut self, id: TabId) {
        let Some(tab) = self.collection_tab_mut(id) else {
            return;
        };
        if tab.shell.is_some() {
            return;
        }
        let (session, db) = (tab.session, tab.db.clone());
        let script = format!(
            "db.getCollection(\"{}\").find({{}}).limit(20)",
            tab.collection
        );
        tab.shell = Some(Box::new(ShellTab {
            session,
            db: db.clone(),
            script,
            engine: ShellEngine::Detecting,
            force_builtin: false,
            history: Vec::new(),
            running: false,
            open: HashSet::new(),
            collections: Vec::new(),
        }));
        self.jobs.spawn(
            async move {
                match mongo_raabit_core::mongosh::find_mongosh() {
                    Some(path) => {
                        let version = mongo_raabit_core::mongosh::version(&path)
                            .await
                            .unwrap_or_default();
                        ShellEngine::Mongosh { path, version }
                    }
                    None => ShellEngine::Builtin,
                }
            },
            move |app: &mut MongoRaabit, engine| {
                if let Some(tab) = app.shell_tab_mut(id) {
                    tab.engine = engine;
                }
            },
        );
        if let Some(conn) = self.connection_for(session) {
            let db = db.to_owned();
            self.jobs.spawn(
                async move { conn.collections(&db).await },
                move |app: &mut MongoRaabit, result| {
                    if let (Some(tab), Ok(list)) = (app.shell_tab_mut(id), result) {
                        tab.collections = list.into_iter().map(|c| c.name).collect();
                    }
                },
            );
        }
    }

    pub fn shell_tab_mut(&mut self, id: TabId) -> Option<&mut ShellTab> {
        self.collection_tab_mut(id)?.shell.as_deref_mut()
    }

    /// Run the shell tab's script (or `selection`, when given). A script that calls write
    /// methods is confirmed first when the settings ask for it.
    pub fn run_shell(&mut self, tab_id: TabId, selection: Option<String>) {
        let Some(tab) = self.shell_tab_mut(tab_id) else {
            return;
        };
        if tab.running {
            return;
        }
        let script = selection.unwrap_or_else(|| tab.script.clone());
        if script.trim().is_empty() {
            return;
        }
        let (session, db) = (tab.session, tab.db.clone());
        // A read-only connection cannot write anyway: no question to ask.
        if !self.session_read_only(session) && self.confirm_script(tab_id, &db, &script) {
            return;
        }
        self.run_shell_script(tab_id, script);
    }

    /// Run `script` in the shell tab now. On a read-only connection it always runs in
    /// the built-in shell, whose writes the core refuses (mongosh cannot be limited).
    pub fn run_shell_script(&mut self, tab_id: TabId, script: String) {
        let read_only = self
            .shell_tab_mut(tab_id)
            .map(|t| t.session)
            .is_some_and(|s| self.session_read_only(s));
        let timeout = self.settings.query_timeout();
        let Some(tab) = self.shell_tab_mut(tab_id) else {
            return;
        };
        if tab.running {
            return;
        }
        tab.running = true;
        let engine = if tab.force_builtin || read_only {
            ShellEngine::Builtin
        } else {
            tab.engine.clone()
        };
        let (session, db) = (tab.session, tab.db.clone());
        let Some(conn) = self.connection_for(session) else {
            return;
        };
        self.jobs.spawn(
            async move {
                let start = std::time::Instant::now();
                match engine {
                    ShellEngine::Mongosh { path, .. } => {
                        match conn.run_mongosh(&path, &db, &script, timeout).await {
                            Ok(out) => (
                                ShellEntry {
                                    script,
                                    output: shell_eval::mongosh_console(&out.stdout, &out.stderr),
                                    ok: out.success,
                                    elapsed: out.elapsed,
                                    value: out.value,
                                    engine: "mongosh",
                                },
                                db,
                            ),
                            Err(err) => (
                                ShellEntry {
                                    script,
                                    output: err.to_string(),
                                    ok: false,
                                    elapsed: start.elapsed(),
                                    value: None,
                                    engine: "mongosh",
                                },
                                db,
                            ),
                        }
                    }
                    _ => {
                        let mut current = db;
                        let results = conn.eval_builtin(&mut current, &script).await;
                        let (output, ok, value) = shell_eval::builtin_console(&results);
                        (
                            ShellEntry {
                                script,
                                output,
                                ok,
                                elapsed: start.elapsed(),
                                value,
                                engine: "built-in",
                            },
                            current,
                        )
                    }
                }
            },
            move |app: &mut MongoRaabit, (entry, db)| {
                if let Some(tab) = app.shell_tab_mut(tab_id) {
                    tab.running = false;
                    tab.db = db;
                    tab.open.clear();
                    tab.history.push(entry);
                }
            },
        );
    }
}
