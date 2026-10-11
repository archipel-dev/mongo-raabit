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
mod structure_view;
pub mod style;
pub mod tools;
mod tree;
mod values;
mod welcome;
pub mod widgets;

pub use app::MongoRaabit;
pub use archipel_sdk::kit::egui;
pub use mongo_raabit_core as core;
pub use plugin::MongoRaabitPlugin;

use crate::egui::{Frame, Margin, Ui};
use app::TabKind;
use archipel_sdk::kit::components::{Button, IconButton, Spinner, Tab, TabBar};
use archipel_sdk::kit::parts::Surface;
use archipel_sdk::kit::tokens::{size, typography};
use archipel_sdk::kit::{Glyph, Tone, icons as kit_icons};
use icons::Icon;

pub const PLUGIN_ID: &str = "dev.archipel.mongo-raabit";

/// The gap between islands and the window edge.
const GAP: i8 = size::ISLAND_GAP as i8;

impl MongoRaabit {
    /// Render the full application view into `ui`.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.jobs.set_context(ui.ctx());
        self.poll();
        if ui.input_mut(|i| i.consume_key(crate::egui::Modifiers::COMMAND, crate::egui::Key::N)) {
            self.open_manager();
        }

        crate::egui::TopBottomPanel::top("raabit-toolbar")
            .frame(Frame::new().inner_margin(Margin::symmetric(size::CONTENT_INSET as i8 - 2, GAP)))
            .show_separator_line(false)
            .show_inside(ui, |ui| self.toolbar(ui));
        crate::egui::TopBottomPanel::bottom("raabit-status")
            .frame(Frame::new().inner_margin(Margin::symmetric(size::CONTENT_INSET as i8, GAP)))
            .show_separator_line(false)
            .show_inside(ui, |ui| self.status_bar(ui));
        crate::egui::SidePanel::left("raabit-tree")
            .resizable(true)
            .default_width(290.0)
            .width_range(200.0..=520.0)
            .frame(Frame::new().inner_margin(Margin {
                left: GAP,
                right: 0,
                top: 0,
                bottom: GAP,
            }))
            .show_separator_line(false)
            .show_inside(ui, |ui| tree::show(self, ui));
        crate::egui::CentralPanel::default()
            .frame(Frame::new().inner_margin(Margin {
                left: GAP,
                right: GAP,
                top: 0,
                bottom: GAP,
            }))
            .show_inside(ui, |ui| {
                Surface::Island
                    .frame(ui.ctx())
                    .inner_margin(Margin::ZERO)
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.tab_bar(ui);
                        self.tab_content(ui);
                    });
            });
        manager::show(self, ui.ctx());
        dialogs::show(self, ui.ctx());
        if self.jobs.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let colors = archipel_sdk::kit::Theme::colors(ui.ctx());
            kit_icons::show(ui, Glyph::Database, size::ICON, colors.icon);
            ui.label(
                crate::egui::RichText::new("Mongo Raabit")
                    .font(typography::semibold(ui.ctx(), typography::TITLE + 1.0))
                    .color(colors.text_strong),
            );
            ui.add_space(size::CONTENT_INSET);
            if ui
                .add(Button::secondary("Connect").icon(Icon::Plug))
                .on_hover_text("Connection Manager (Ctrl+N)")
                .clicked()
            {
                self.open_manager();
            }
            let selected_session = self.selected.as_ref().map(|key| match key {
                app::NodeKey::Session(s)
                | app::NodeKey::Database(s, _)
                | app::NodeKey::Collection(s, _, _) => *s,
            });
            let enabled = selected_session.is_some();
            if ui
                .add(IconButton::new(Icon::Unplug, "Disconnect").enabled(enabled))
                .clicked()
                && let Some(id) = selected_session
            {
                self.disconnect(id);
            }
            if ui
                .add(IconButton::new(Icon::Refresh, "Refresh").enabled(enabled))
                .clicked()
                && let Some(id) = selected_session
            {
                self.load_databases(id);
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let colors = archipel_sdk::kit::Theme::colors(ui.ctx());
            let connected = self
                .sessions
                .iter()
                .filter(|s| s.connection().is_some())
                .count();
            let (glyph, color, text) = if connected > 0 {
                (
                    Glyph::Success,
                    colors.success,
                    format!("{connected} connected"),
                )
            } else {
                (
                    Glyph::Info,
                    colors.text_placeholder,
                    "Not connected".to_owned(),
                )
            };
            kit_icons::show(ui, glyph, size::ICON - 2.0, color);
            ui.label(crate::egui::RichText::new(text).color(Tone::Secondary.resolve(ui.ctx())));
            if !self.status.is_empty() {
                ui.label(crate::egui::RichText::new("·").color(Tone::Muted.resolve(ui.ctx())));
                ui.label(
                    crate::egui::RichText::new(&self.status)
                        .color(Tone::Secondary.resolve(ui.ctx())),
                );
            }
            ui.with_layout(
                crate::egui::Layout::right_to_left(crate::egui::Align::Center),
                |ui| {
                    if self.jobs.busy() {
                        ui.add(Spinner::new().label("Working"));
                    }
                },
            );
        });
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
                        format!("{} · Query", c.collection),
                    ),
                    TabKind::Aggregation(t) => {
                        (Icon::Pipeline, format!("{} · Aggregate", t.collection))
                    }
                    TabKind::Structure(t) => {
                        (Icon::Schema, format!("{} · Structure", t.schema.collection))
                    }
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
            TabKind::Aggregation(_) => {
                let id = tab.id;
                aggregation_view::show(self, id, ui);
            }
            TabKind::Structure(_) => {
                let id = tab.id;
                structure_view::show(self, id, ui);
            }
        }
    }
}
