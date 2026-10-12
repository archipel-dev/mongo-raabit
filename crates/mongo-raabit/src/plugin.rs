//! Archipel plugin entry point: the whole mongo-raabit view as an editor tab
//! (Tools › MongoDB), the Databases tool window (left), the Collection tool window
//! (right: query builder, pipeline and indexes of the active collection tab), or the
//! standalone `raabit` app.

use crate::writes::HostRequest;
use crate::{MongoRaabit, tree};
use archipel_sdk::api::{
    CommandContribution, CommandError, CommandResult, Contributions, EditorTabRequest,
    MenuLocation, Notification, Plugin, PluginHost, PluginManifest, SettingField, SettingKind,
    SettingsPage, ToolWindowAnchor, ToolWindowContribution, Value, ViewContext, ViewFrame,
};
use archipel_sdk::ui;
use mongo_raabit_core::settings::{self, Settings};

pub const VIEW: &str = "mongo.main";
pub const DATABASES_VIEW: &str = "mongo.databases";
pub const DATABASES_WINDOW: &str = "mongo-databases";
pub const OPEN: &str = "mongo.open";
pub const SHOW_DATABASES: &str = "mongo.showDatabases";
pub const TOOLS_VIEW: &str = "mongo.tools";
pub const TOOLS_WINDOW: &str = "mongo-tools";
pub const SHOW_TOOLS: &str = "mongo.showTools";
const PROFILES_FILE: &str = "connections.json";
/// The host's "Open Settings" command (the standalone app's; Ctrl+, there).
pub const OPEN_SETTINGS: &str = archipel_sdk::app::CMD_SETTINGS;

/// The Settings page of Mongo Raabit: every field of [`Settings`].
pub fn settings_page() -> SettingsPage {
    let defaults = Settings::default();
    let number = |default: u32, (min, max): (u32, u32)| SettingKind::Number {
        default: f64::from(default),
        min: Some(f64::from(min)),
        max: Some(f64::from(max)),
    };
    SettingsPage::new("mongo-raabit", "Mongo Raabit")
        .field(
            SettingField::new(
                settings::CONFIRM_WRITES,
                "Confirm changes",
                SettingKind::Bool {
                    default: defaults.confirm_writes,
                },
            )
            .description(
                "Before any change runs, show the exact query (mongosh syntax) and how many documents it affects",
            ),
        )
        .field(
            SettingField::new(
                settings::NEW_CONNECTIONS_READ_ONLY,
                "Open connections read-only",
                SettingKind::Bool {
                    default: defaults.new_connections_read_only,
                },
            )
            .description(
                "Every connection opened from now on blocks all changes, whatever its own Read-only setting",
            ),
        )
        .field(
            SettingField::new(
                settings::PAGE_SIZE,
                "Documents per page",
                number(defaults.page_size, settings::PAGE_SIZE_RANGE),
            )
            .description("Page size of a new Query tab"),
        )
        .field(
            SettingField::new(
                settings::SCHEMA_SAMPLE_SIZE,
                "Schema sample size",
                number(defaults.schema_sample_size, settings::SCHEMA_SAMPLE_RANGE),
            )
            .description("Documents the Schema Explorer samples when it opens"),
        )
        .field(
            SettingField::new(
                settings::QUERY_TIMEOUT_SECS,
                "Query timeout (seconds)",
                number(defaults.query_timeout_secs, settings::QUERY_TIMEOUT_RANGE),
            )
            .description("Queries, aggregations and shell scripts that take longer are stopped"),
        )
}

/// Read the settings from the host before a frame.
fn read_settings(app: &mut MongoRaabit, host: &dyn PluginHost) {
    app.settings = Settings::from_values(|key| host.setting(key));
    app.sync_query_timeout();
}

/// Apply what the views asked of the host during a frame.
fn apply_requests(app: &mut MongoRaabit, host: &mut dyn PluginHost) {
    for request in std::mem::take(&mut app.host_requests) {
        match request {
            HostRequest::SetSetting(key, value) => {
                if let Err(err) = host.set_setting(key, value) {
                    host.notify(Notification::error("Settings", err.to_string()));
                }
            }
            HostRequest::OpenSettings => host.dispatch(OPEN_SETTINGS, Value::Null),
        }
    }
}

pub struct MongoRaabitPlugin {
    manifest: PluginManifest,
    /// Created on first render, once the host's data directory is known.
    app: Option<MongoRaabit>,
    /// The status line last sent to the host: the host repaints on every update.
    status: String,
    standalone: bool,
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
            standalone: false,
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

    fn activate(
        &mut self,
        host: &mut dyn PluginHost,
    ) -> Result<(), archipel_sdk::api::PluginError> {
        self.standalone = host.is_standalone();
        Ok(())
    }

    fn contributions(&self) -> Contributions {
        let contributions = Contributions::new()
            .command(CommandContribution::new(OPEN, "MongoDB (Mongo Raabit)"))
            .command(CommandContribution::new(
                SHOW_DATABASES,
                "Show MongoDB Databases",
            ))
            .command(CommandContribution::new(
                SHOW_TOOLS,
                "Show MongoDB Query Builder",
            ))
            .menu(MenuLocation::Tools, OPEN)
            .settings_page(settings_page());
        if self.standalone {
            return contributions;
        }
        contributions
            .tool_window(
                ToolWindowContribution::new(DATABASES_WINDOW, "Databases", DATABASES_VIEW)
                    .anchor(ToolWindowAnchor::Left)
                    .icon_text("Db"),
            )
            .tool_window(
                ToolWindowContribution::new(TOOLS_WINDOW, crate::tool_window::TITLE, TOOLS_VIEW)
                    .anchor(ToolWindowAnchor::Right)
                    .icon_text("Q"),
            )
    }

    fn execute(
        &mut self,
        command: &str,
        _params: Value,
        host: &mut dyn PluginHost,
    ) -> CommandResult {
        if host.is_standalone() {
            let app = self.app(host);
            match command {
                OPEN => {}
                SHOW_DATABASES => {
                    app.layout.left_hidden = false;
                    app.layout.maximized = None;
                }
                SHOW_TOOLS => app.show_tool(crate::app::ToolTab::Query),
                other => return Err(CommandError::Unknown(other.into())),
            }
            host.focus_app_view();
            host.request_repaint();
            return Ok(Value::Null);
        }
        match command {
            OPEN => {
                host.open_editor_tab(EditorTabRequest::new(VIEW, "MongoDB"));
                Ok(Value::Null)
            }
            SHOW_DATABASES => {
                host.open_tool_window(DATABASES_WINDOW);
                Ok(Value::Null)
            }
            SHOW_TOOLS => {
                host.open_tool_window(TOOLS_WINDOW);
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
            read_settings(app, host);
            app.ui(ui);
            apply_requests(app, host);
            // One status bar: the host's. It repaints on every update, so send changes only.
            let line = app.status_line();
            if line != self.status {
                host.show_status(&line);
                self.status = line;
            }
        } else if view.view == DATABASES_VIEW {
            let app = self.app(host);
            read_settings(app, host);
            app.jobs.set_context(ui.ctx());
            app.poll();
            tree::show(app, ui);
            apply_requests(app, host);
        } else if view.view == TOOLS_VIEW {
            let app = self.app(host);
            read_settings(app, host);
            app.jobs.set_context(ui.ctx());
            app.poll();
            crate::tool_window::show_in_host(app, ui);
            apply_requests(app, host);
        } else {
            ui.label(format!("Unknown view {}", view.view));
        }
    }

    fn poll(&mut self, host: &mut dyn PluginHost) {
        if let Some(app) = &mut self.app {
            read_settings(app, host);
            app.poll();
            apply_requests(app, host);
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
