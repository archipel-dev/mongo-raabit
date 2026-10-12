//! Mongo Raabit: a clean-room MongoDB GUI for the Archipel suite.
//!
//! [`MongoRaabit`] is the whole view state; render it with [`MongoRaabit::ui`] inside
//! any egui `Ui` (a plugin tool window, an editor tab or a standalone window). All
//! database work runs on a background runtime and is applied in [`MongoRaabit::poll`].

mod aggregation_view;
pub mod app;
mod collection;
pub mod dialogs;
pub mod icons;
mod index_view;
pub mod jobs;
pub mod manager;
pub mod plugin;
pub mod query_builder;
pub mod results;
mod schema_view;
mod shell_view;
mod sql_view;
pub mod stage_builder;
pub mod tool_window;
pub mod tools;
mod tree;
mod values;
mod welcome;
pub mod widgets;
pub mod writes;

pub use app::MongoRaabit;
pub use archipel_sdk::kit::egui;
pub use mongo_raabit_core as core;
pub use plugin::MongoRaabitPlugin;

use crate::egui::Ui;
use app::TabKind;
use archipel_sdk::kit::components::{Tab, TabBar, Workbench, WorkbenchPlace, WorkbenchRegion};
use icons::Icon;

pub const PLUGIN_ID: &str = "dev.archipel.mongo-raabit";

impl MongoRaabit {
    /// Render the full application view into `ui`.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.jobs.set_context(ui.ctx());
        self.poll();
        if ui.input_mut(|i| i.consume_key(crate::egui::Modifiers::COMMAND, crate::egui::Key::N)) {
            self.open_manager();
        }

        let mut layout = std::mem::take(&mut self.layout);
        // The host gives the whole area; the islands keep the gap to its edges.
        let gap = archipel_sdk::kit::tokens::size::ISLAND_GAP;
        let area = ui.available_rect_before_wrap().shrink(gap);
        let mut ui = ui.new_child(crate::egui::UiBuilder::new().max_rect(area));
        let ui = &mut ui;
        // The right tool window's tool is kept with the layout, which is lent to the
        // workbench while it draws: tools asked for meanwhile are applied after the frame.
        self.tool = app::ToolTab::from_index(layout.tool_tab(WorkbenchRegion::Right));
        self.tool_request = None;
        Workbench::new("raabit", &mut layout)
            .frame_background(false)
            .tool(WorkbenchRegion::Left, "Connections", Icon::Database)
            .tool(WorkbenchRegion::Right, tool_window::TITLE, Icon::Filter)
            .center_padded(false)
            .show_inside_with(ui, self, |place, ui, app| match place {
                WorkbenchPlace::Tool(WorkbenchRegion::Left) => tree::show(app, ui),
                WorkbenchPlace::ToolActions(WorkbenchRegion::Left) => tree::header_actions(app, ui),
                WorkbenchPlace::Tool(WorkbenchRegion::Right) => tool_window::show(app, ui),
                WorkbenchPlace::Center => {
                    app.tab_bar(ui);
                    app.tab_content(ui);
                }
                _ => {}
            });
        self.layout = layout;
        if self.tool_request.take().is_some() {
            // `show_tool` set the tool; put it into the layout that is back now.
            self.tool_request = Some(self.tool);
            self.apply_tool_request();
            self.tool_request = None;
        }
        manager::show(self, ui.ctx());
        dialogs::show(self, ui.ctx());
        if self.jobs.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// What the status bar says about the app: connections, the last message, whether a
    /// job runs.
    pub fn status_line(&self) -> String {
        let connected = self
            .sessions
            .iter()
            .filter(|s| s.connection().is_some())
            .count();
        core::status::status_line(connected, &self.status, self.jobs.busy())
    }

    fn tab_bar(&mut self, ui: &mut Ui) {
        let tabs: Vec<Tab> = self
            .tabs
            .iter()
            .map(|tab| {
                let (icon, title) = match &tab.kind {
                    TabKind::Welcome => (Icon::Database, "Welcome".to_owned()),
                    TabKind::Collection(c) => (
                        match c.kind {
                            core::connection::CollectionKind::View => Icon::View,
                            _ => Icon::Collection,
                        },
                        c.collection.clone(),
                    ),
                };
                Tab::new(title).icon(icon).closable(true)
            })
            .collect();
        let out = TabBar::new(&tabs, self.active_tab).show(ui);
        if let Some(index) = out.selected {
            self.active_tab = index;
        }
        if let Some(index) = out.closed {
            self.close_tab(index);
        }
    }

    fn tab_content(&mut self, ui: &mut Ui) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            welcome::show(self, ui);
            return;
        };
        match &tab.kind {
            TabKind::Welcome => welcome::show(self, ui),
            TabKind::Collection(_) => {
                let id = tab.id;
                collection::show(self, id, ui);
            }
        }
    }
}
