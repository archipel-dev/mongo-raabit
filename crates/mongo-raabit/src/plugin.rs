//! Jetthing plugin entry point: the whole mongo-raabit view as an editor tab
//! (Tools › MongoDB) or as the standalone app (`jetthing --app dev.jetthing.mongo-raabit`).

use crate::MongoRaabit;
use jetthing_plugin_api::{
    CommandContribution, CommandError, CommandResult, Contributions, EditorTabRequest,
    MenuLocation, Plugin, PluginHost, PluginManifest, Value, ViewContext, ViewFrame,
};

pub const VIEW: &str = "mongo.main";
pub const OPEN: &str = "mongo.open";
const PROFILES_FILE: &str = "connections.json";

pub struct MongoRaabitPlugin {
    manifest: PluginManifest,
    /// Created on first render, once the host's data directory is known.
    app: Option<MongoRaabit>,
}

impl Default for MongoRaabitPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl MongoRaabitPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest::parse(include_str!("../jetthing-plugin.toml"))
                .expect("bundled manifest is valid"),
            app: None,
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
            .command(CommandContribution::new(OPEN, "MongoDB (mongo-raabit)"))
            .menu(MenuLocation::Tools, OPEN)
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
            other => Err(CommandError::Unknown(other.into())),
        }
    }

    fn render_view(
        &mut self,
        view: &ViewContext,
        frame: &mut ViewFrame<'_>,
        host: &mut dyn PluginHost,
    ) {
        let Some(ui) = jetthing_plugin_ui::ui(frame) else {
            return;
        };
        if view.view == VIEW {
            self.app(host).ui(ui);
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
    use jetthing_plugin_api::testing::RecordingHost;

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
