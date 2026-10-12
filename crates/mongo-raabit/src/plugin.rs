//! Archipel plugin entry point: the whole mongo-raabit view as an editor tab
//! (Tools › MongoDB), the Databases tool window, or the standalone `raabit` app.

use crate::{MongoRaabit, tree};
use archipel_sdk::api::{
    CommandContribution, CommandError, CommandResult, Contributions, EditorTabRequest,
    MenuLocation, Plugin, PluginHost, PluginManifest, ToolWindowAnchor, ToolWindowContribution,
    Value, ViewContext, ViewFrame,
};
use archipel_sdk::ui;

pub const VIEW: &str = "mongo.main";
pub const DATABASES_VIEW: &str = "mongo.databases";
pub const DATABASES_WINDOW: &str = "mongo-databases";
pub const OPEN: &str = "mongo.open";
pub const SHOW_DATABASES: &str = "mongo.showDatabases";
const PROFILES_FILE: &str = "connections.json";

pub struct MongoRaabitPlugin {
    manifest: PluginManifest,
    /// Created on first render, once the host's data directory is known.
    app: Option<MongoRaabit>,
    /// The status line last sent to the host: the host repaints on every update.
    status: String,
}

impl Default for MongoRaabitPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl MongoRaabitPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest::parse(include_str!("../archipel-plugin.toml"))
                .expect("bundled manifest is valid"),
            app: None,
            status: String::new(),
        }
    }

    fn app(&mut self, host: &dyn PluginHost) -> &mut MongoRaabit {
        self.app.get_or_insert_with(|| {
            // Without a data directory, connections live in memory for this run.
            let store = host.data_dir().ok().map(|dir| dir.join(PROFILES_FILE));
            MongoRaabit::new(store)
        })
    }
}

impl Plugin for MongoRaabitPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn contributions(&self) -> Contributions {
        Contributions::new()
            .command(CommandContribution::new(OPEN, "MongoDB (Mongo Raabit)"))
            .command(CommandContribution::new(
                SHOW_DATABASES,
                "Show MongoDB Databases",
            ))
            .menu(MenuLocation::Tools, OPEN)
            .tool_window(
                ToolWindowContribution::new(DATABASES_WINDOW, "Databases", DATABASES_VIEW)
                    .anchor(ToolWindowAnchor::Left)
                    .icon_text("Db"),
            )
    }

    fn execute(
        &mut self,
        command: &str,
        _params: Value,
        host: &mut dyn PluginHost,
    ) -> CommandResult {
        match command {
            OPEN => {
                host.open_editor_tab(EditorTabRequest::new(VIEW, "MongoDB"));
                Ok(Value::Null)
            }
            SHOW_DATABASES => {
                host.open_tool_window(DATABASES_WINDOW);
                Ok(Value::Null)
            }
            other => Err(CommandError::Unknown(other.into())),
        }
    }

    fn render_view(
        &mut self,
        view: &ViewContext,
        frame: &mut ViewFrame<'_>,
        host: &mut dyn PluginHost,
    ) {
        let Some(ui) = ui::ui(frame) else {
            return;
        };
        if view.view == VIEW {
            let app = self.app(host);
            app.ui(ui);
            // One status bar: the host's. It repaints on every update, so send changes only.
            let line = app.status_line();
            if line != self.status {
                host.show_status(&line);
                self.status = line;
            }
        } else if view.view == DATABASES_VIEW {
            let app = self.app(host);
            app.jobs.set_context(ui.ctx());
            app.poll();
            tree::show(app, ui);
        } else {
            ui.label(format!("Unknown view {}", view.view));
        }
    }

    fn poll(&mut self, host: &mut dyn PluginHost) {
        if let Some(app) = &mut self.app {
            app.poll();
            if app.jobs.busy() {
                host.request_repaint();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archipel_sdk::testing::RecordingHost;

    #[test]
    fn open_command_opens_the_editor_tab() {
        let mut plugin = MongoRaabitPlugin::new();
        let contributions = plugin.contributions();
        assert!(contributions.commands.iter().any(|c| c.id == OPEN));
        assert_eq!(
            plugin.manifest().app.as_ref().map(|a| a.view.as_str()),
            Some(VIEW)
        );
        let mut host = RecordingHost::default();
        plugin.execute(OPEN, Value::Null, &mut host).unwrap();
        assert_eq!(host.editor_tabs[0].view, VIEW);
    }
}
