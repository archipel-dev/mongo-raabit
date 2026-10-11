//! mongo-raabit: a clean-room MongoDB GUI for Jetthing.
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
mod results;
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
pub use mongo_raabit_core as core;
pub use plugin::MongoRaabitPlugin;

use app::TabKind;
use egui::{CornerRadius, FontId, RichText, Sense, Stroke, Ui, vec2};
use icons::Icon;
use style::Palette;

pub const PLUGIN_ID: &str = "dev.jetthing.mongo-raabit";

impl MongoRaabit {
    /// Render the full application view into `ui`.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.jobs.set_context(ui.ctx());
        self.poll();
        let p = Palette::of(ui);
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
            self.open_manager();
        }
        let full = ui.available_rect_before_wrap();
        ui.painter().rect_filled(full, 0.0, p.frame);

        egui::TopBottomPanel::top("raabit-toolbar")
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 10,
                right: 10,
                top: 6,
                bottom: 6,
            }))
            .show_separator_line(false)
            .show_inside(ui, |ui| self.toolbar(ui, &p));
        egui::TopBottomPanel::bottom("raabit-status")
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 3,
                bottom: 4,
            }))
            .show_separator_line(false)
            .show_inside(ui, |ui| self.status_bar(ui, &p));
        egui::SidePanel::left("raabit-tree")
            .resizable(true)
            .default_width(290.0)
            .width_range(200.0..=520.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 6,
                right: 0,
                top: 0,
                bottom: 4,
            }))
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                style::island(&p).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    tree::show(self, ui);
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: style::GAP as i8 + 2,
                right: 6,
                top: 0,
                bottom: 4,
            }))
            .show_inside(ui, |ui| {
                style::island(&p).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.tab_bar(ui, &p);
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

    fn toolbar(&mut self, ui: &mut Ui, p: &Palette) {
        ui.horizontal(|ui| {
            icons::icon(ui, Icon::Leaf, p.leaf);
            ui.label(
                RichText::new("mongo-raabit")
                    .size(14.0)
                    .strong()
                    .color(p.text_strong),
            );
            ui.add_space(14.0);
            if widgets::secondary_button(ui, Some(Icon::Plug), "Connect")
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
            if widgets::icon_button_tinted(
                ui,
                Icon::Unplug,
                "Disconnect",
                None,
                selected_session.is_some(),
            )
            .clicked()
                && let Some(id) = selected_session
            {
                self.disconnect(id);
            }
            if widgets::icon_button_tinted(
                ui,
                Icon::Refresh,
                "Refresh",
                None,
                selected_session.is_some(),
            )
            .clicked()
                && let Some(id) = selected_session
            {
                self.load_databases(id);
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, p: &Palette) {
        ui.horizontal(|ui| {
            let connected = self
                .sessions
                .iter()
                .filter(|s| s.connection().is_some())
                .count();
            let (color, text) = if connected > 0 {
                (p.success, format!("{connected} connected"))
            } else {
                (p.text_info, "Not connected".to_owned())
            };
            let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.5, color);
            ui.label(RichText::new(text).size(12.0).color(p.text_secondary));
            if !self.status.is_empty() {
                ui.label(RichText::new("·").color(p.text_info));
                ui.label(
                    RichText::new(&self.status)
                        .size(12.0)
                        .color(p.text_secondary),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.jobs.busy() {
                    ui.spinner();
                }
            });
        });
    }

    fn tab_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let mut activate = None;
        let mut close = None;
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 6,
                right: 6,
                top: 4,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (index, tab) in self.tabs.iter().enumerate() {
                        let (icon, title, color) = match &tab.kind {
                            TabKind::Welcome => (Icon::Leaf, "Welcome".to_owned(), p.leaf),
                            TabKind::Collection(c) => (
                                match c.kind {
                                    core::connection::CollectionKind::View => Icon::View,
                                    _ => Icon::Collection,
                                },
                                format!("{} · Query", c.collection),
                                p.text_secondary,
                            ),
                            TabKind::Aggregation(t) => (
                                Icon::Pipeline,
                                format!("{} · Aggregate", t.collection),
                                p.accent_text,
                            ),
                            TabKind::Structure(t) => (
                                Icon::Schema,
                                format!("{} · Structure", t.schema.collection),
                                p.leaf,
                            ),
                        };
                        let active = index == self.active_tab;
                        let font = FontId::proportional(13.0);
                        let text_color = if active {
                            p.text_strong
                        } else {
                            p.text_secondary
                        };
                        let galley = ui.painter().layout_no_wrap(title, font, text_color);
                        let width = galley.size().x + 58.0;
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
                        if active {
                            ui.painter().rect_filled(
                                rect.shrink2(vec2(0.0, 3.0)),
                                CornerRadius::same(7),
                                p.selection_inactive,
                            );
                        } else if response.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink2(vec2(0.0, 3.0)),
                                CornerRadius::same(7),
                                p.hover,
                            );
                        }
                        icons::paint(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                egui::pos2(rect.left() + 16.0, rect.center().y),
                                vec2(14.0, 14.0),
                            ),
                            icon,
                            color,
                        );
                        ui.painter().galley(
                            egui::pos2(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0),
                            galley,
                            text_color,
                        );
                        let close_rect = egui::Rect::from_center_size(
                            egui::pos2(rect.right() - 14.0, rect.center().y),
                            vec2(16.0, 16.0),
                        );
                        let close_hover = response
                            .hover_pos()
                            .is_some_and(|pos| close_rect.contains(pos));
                        if active || response.hovered() {
                            if close_hover {
                                ui.painter().rect_filled(
                                    close_rect,
                                    CornerRadius::same(4),
                                    p.hover,
                                );
                            }
                            icons::paint(
                                ui.painter(),
                                close_rect.shrink(3.0),
                                Icon::Close,
                                p.text_info,
                            );
                        }
                        if response.clicked() {
                            if close_hover {
                                close = Some(index);
                            } else {
                                activate = Some(index);
                            }
                        }
                        if response.middle_clicked() {
                            close = Some(index);
                        }
                    }
                });
            });
        let rect = ui.available_rect_before_wrap();
        ui.painter().hline(
            rect.x_range(),
            rect.top(),
            Stroke::new(1.0_f32, p.separator),
        );
        if let Some(index) = activate {
            self.active_tab = index;
        }
        if let Some(index) = close {
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
