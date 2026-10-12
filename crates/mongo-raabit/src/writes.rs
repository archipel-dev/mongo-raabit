//! The one place the views change the database: every write action builds a
//! [`Write`] and hands it to [`MongoRaabit::request_write`]. With "Confirm changes" on
//! (the default), the write is previewed in the background and shown in a confirmation
//! dialog (the query in mongosh syntax and what it affects) before it runs; otherwise it
//! runs at once. Read-only connections refuse it here, and `Connection::execute` refuses
//! it again in the core.

use crate::app::{Load, MongoRaabit, SessionId, TabId};
use crate::dialogs::{ConfirmWrite, Confirmed, Dialog};
use crate::tools::RunResult;
use mongo_raabit_core::settings;
use mongo_raabit_core::shell_eval;
use mongo_raabit_core::{Write, WriteOutcome, status};

/// What the views do with a write's outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteOrigin {
    /// An inline cell edit of a collection tab.
    Cell(TabId),
    /// The document editor's Save or Insert.
    Document(TabId),
    /// Deleting the selected documents of a collection tab.
    Delete(TabId),
    /// The Index Manager; the text is the status line on success.
    Index(TabId, String),
    /// A pipeline with `$out` / `$merge` run from the Aggregation tab.
    Pipeline(TabId),
}

/// A write on its way: the session it runs on, what it is and who asked.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingWrite {
    pub session: SessionId,
    pub write: Write,
    pub origin: WriteOrigin,
}

/// What the views ask of the host, applied by the plugin after the frame.
#[derive(Debug, Clone, PartialEq)]
pub enum HostRequest {
    /// Store a setting (`key`, value).
    SetSetting(&'static str, serde_json::Value),
    /// Open the host's Settings.
    OpenSettings,
}

const NOT_CONNECTED: &str = "not connected";

/// The tooltip of write actions on a read-only connection.
pub const READ_ONLY_TIP: &str = "Read-only connection";
/// The same, inside a label: "Delete document (read-only connection)".
pub const READ_ONLY_TIP_LOWER: &str = "read-only connection";

impl MongoRaabit {
    /// True when changes through `session` are blocked: its connection is read-only, or
    /// (while it connects) its profile or the "open connections read-only" setting says so.
    pub fn session_read_only(&self, session: SessionId) -> bool {
        match self.session(session) {
            Some(s) => match s.connection() {
                Some(conn) => conn.is_read_only(),
                None => s.profile.read_only || self.settings.new_connections_read_only,
            },
            None => false,
        }
    }

    /// Run `pending`, after a confirmation when the settings ask for one.
    pub fn request_write(&mut self, pending: PendingWrite) {
        if self.session_read_only(pending.session) {
            self.status = format!("{READ_ONLY_TIP}: \"{}\" was not run", pending.write.title());
            self.write_cancelled(&pending.origin);
            return;
        }
        if !self.settings.confirm_writes {
            self.run_write(pending);
            return;
        }
        let conn = self.connection_for(pending.session);
        let id = self.next_id();
        let write = pending.write.clone();
        self.dialog = Some(Dialog::ConfirmWrite(ConfirmWrite {
            id,
            title: pending.write.title(),
            shell: pending.write.shell(),
            destructive: pending.write.is_destructive(),
            summary: if conn.is_some() {
                Load::Loading
            } else {
                Load::Failed(NOT_CONNECTED.into())
            },
            dont_ask: false,
            running: false,
            back: self.dialog.take().map(Box::new),
            action: Confirmed::Write(Box::new(pending)),
        }));
        let Some(conn) = conn else {
            return;
        };
        self.jobs.spawn(
            async move {
                let preview = conn.preview(&write).await;
                preview.map(|p| write.summary(&p.affected))
            },
            move |app: &mut MongoRaabit, result| {
                if let Some(Dialog::ConfirmWrite(confirm)) = &mut app.dialog
                    && confirm.id == id
                {
                    confirm.summary = match result {
                        Ok(summary) => Load::Ready(summary),
                        Err(err) => Load::Failed(format!("Could not count: {err}")),
                    };
                }
            },
        );
    }

    /// Ask before running a shell script that calls write methods. Returns true when a
    /// confirmation was opened (the script then runs from the dialog).
    pub fn confirm_script(&mut self, tab: TabId, db: &str, script: &str) -> bool {
        let writes = shell_eval::script_writes(script);
        if !self.settings.confirm_writes || writes.is_empty() {
            return false;
        }
        let id = self.next_id();
        self.dialog = Some(Dialog::ConfirmWrite(ConfirmWrite {
            id,
            title: format!("Run a script that can change {db}"),
            shell: script.to_owned(),
            destructive: shell_eval::script_is_destructive(script),
            summary: Load::Ready(format!(
                "The script calls {}: it can modify data",
                writes.join(", ")
            )),
            dont_ask: false,
            running: false,
            back: self.dialog.take().map(Box::new),
            action: Confirmed::Script {
                tab,
                script: script.to_owned(),
            },
        }));
        true
    }

    /// Turn the write confirmation off (the dialog's "Don't ask again").
    pub fn stop_confirming_writes(&mut self) {
        self.settings.confirm_writes = false;
        self.host_requests.push(HostRequest::SetSetting(
            settings::CONFIRM_WRITES,
            serde_json::Value::Bool(false),
        ));
    }

    /// Run `pending` now (confirmed, or confirmation off).
    pub fn run_write(&mut self, pending: PendingWrite) {
        let Some(conn) = self.connection_for(pending.session) else {
            let error = mongo_raabit_core::RaabitError::Invalid(NOT_CONNECTED.into());
            self.write_done(pending.origin, Err(error));
            return;
        };
        match &pending.origin {
            WriteOrigin::Index(tab, _) => {
                if let Some(tab) = self.index_tab_mut(*tab) {
                    tab.busy = true;
                    tab.error = None;
                }
            }
            WriteOrigin::Pipeline(tab) => {
                if let Some(tab) = self.aggregation_tab_mut(*tab) {
                    tab.result = Load::Loading;
                }
            }
            WriteOrigin::Document(_) => {
                if let Some(Dialog::Document(editor)) = &mut self.dialog {
                    editor.saving = true;
                    editor.error = None;
                }
            }
            WriteOrigin::Cell(_) | WriteOrigin::Delete(_) => {}
        }
        let origin = pending.origin;
        let limit = self.settings.query_timeout();
        let is_pipeline = matches!(origin, WriteOrigin::Pipeline(_));
        self.jobs.spawn(
            async move {
                let run = conn.execute(pending.write);
                if is_pipeline {
                    crate::app::bounded(limit, run).await
                } else {
                    run.await
                }
            },
            move |app: &mut MongoRaabit, result| app.write_done(origin, result),
        );
    }

    /// The confirmation was cancelled: put back what asked for it.
    pub fn write_cancelled(&mut self, origin: &WriteOrigin) {
        match origin {
            WriteOrigin::Document(_) => {
                if let Some(Dialog::Document(editor)) = &mut self.dialog {
                    editor.saving = false;
                }
            }
            WriteOrigin::Delete(_) => {
                if let Some(Dialog::Delete(confirm)) = &mut self.dialog {
                    confirm.busy = false;
                }
            }
            WriteOrigin::Index(tab, _) => {
                if let Some(tab) = self.index_tab_mut(*tab) {
                    tab.busy = false;
                }
            }
            WriteOrigin::Pipeline(tab) => {
                if let Some(tab) = self.aggregation_tab_mut(*tab)
                    && tab.result.is_loading()
                {
                    tab.result = Load::Idle;
                }
            }
            WriteOrigin::Cell(_) => {}
        }
    }

    /// Apply a write's outcome to the view that asked for it.
    fn write_done(&mut self, origin: WriteOrigin, result: mongo_raabit_core::Result<WriteOutcome>) {
        // The confirmation (still open while the write ran) gives way to what it covered.
        if let Some(Dialog::ConfirmWrite(_)) = &self.dialog
            && let Some(Dialog::ConfirmWrite(confirm)) = self.dialog.take()
        {
            self.dialog = confirm.back.map(|b| *b);
        }
        match origin {
            WriteOrigin::Cell(tab) => match result {
                Ok(outcome) => {
                    self.status = outcome.message();
                    self.run_query(tab, false);
                }
                Err(err) => self.status = format!("Update failed: {err}"),
            },
            WriteOrigin::Document(tab) => match result {
                Ok(outcome) => {
                    self.status = match outcome {
                        WriteOutcome::Inserted(_) => outcome.message(),
                        _ => "Document saved".to_owned(),
                    };
                    self.dialog = None;
                    self.run_query(tab, true);
                }
                Err(err) => {
                    if let Some(Dialog::Document(editor)) = &mut self.dialog {
                        editor.saving = false;
                        editor.error = Some(err.to_string());
                    } else {
                        self.status = format!("Not saved: {err}");
                    }
                }
            },
            WriteOrigin::Delete(tab) => {
                if let Some(Dialog::Delete(_)) = &self.dialog {
                    self.dialog = None;
                }
                match result {
                    Ok(outcome) => {
                        let n = match outcome {
                            WriteOutcome::Deleted(n) => n,
                            _ => 0,
                        };
                        self.status = status::deleted_documents(n);
                        if let Some(tab) = self.collection_tab_mut(tab) {
                            tab.selected = None;
                        }
                        self.run_query(tab, true);
                    }
                    Err(err) => self.status = format!("Delete failed: {err}"),
                }
            }
            WriteOrigin::Index(tab_id, done) => {
                if let Some(tab) = self.index_tab_mut(tab_id) {
                    tab.busy = false;
                    match result {
                        Ok(_) => {
                            tab.form_open = false;
                            self.status = done;
                            self.load_indexes(tab_id);
                        }
                        Err(err) => tab.error = Some(err.to_string()),
                    }
                }
            }
            WriteOrigin::Pipeline(tab_id) => {
                if let Some(tab) = self.aggregation_tab_mut(tab_id) {
                    tab.result = match result {
                        Ok(WriteOutcome::Aggregated { documents, elapsed }) => {
                            Load::Ready(RunResult { documents, elapsed })
                        }
                        Ok(_) => Load::Ready(RunResult::default()),
                        Err(err) => Load::Failed(err.to_string()),
                    };
                }
            }
        }
    }
}
