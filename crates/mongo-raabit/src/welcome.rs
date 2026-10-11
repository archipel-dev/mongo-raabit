//! Welcome tab: quick actions and saved connections.

use crate::app::MongoRaabit;
use crate::icons::{self, Icon};
use crate::style::Palette;
use crate::widgets;
use egui::{Align2, CornerRadius, FontId, RichText, Sense, Stroke, Ui, pos2, vec2};
use mongo_raabit_core::profile::ConnectionProfile;

pub fn show(app: &mut MongoRaabit, ui: &mut Ui) {
    let p = Palette::of(ui);
    let mut connect: Option<ConnectionProfile> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                let (rect, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(16), p.leaf.gamma_multiply(0.15));
                icons::paint(ui.painter(), rect.shrink(12.0), Icon::Leaf, p.leaf);
                ui.add_space(12.0);
                ui.label(
                    RichText::new("mongo-raabit")
                        .size(22.0)
                        .strong()
                        .color(p.text_strong),
                );
                ui.label(
                    RichText::new("Browse, query and shape your MongoDB data")
                        .color(p.text_secondary),
                );
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    let width = 2.0 * 150.0 + 8.0;
                    ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                    if widgets::primary_button(ui, Some(Icon::Plus), "New Connection").clicked() {
                        app.open_manager();
                    }
                    if widgets::secondary_button(ui, Some(Icon::Plug), "Connect to localhost")
                        .clicked()
                    {
                        // The default profile is a standalone server on localhost:27017.
                        connect = Some(ConnectionProfile {
                            name: "localhost".into(),
                            ..ConnectionProfile::default()
                        });
                    }
                });
                ui.add_space(28.0);
            });
            let saved: Vec<ConnectionProfile> = app.store.profiles.clone();
            if !saved.is_empty() {
                let width = ui.available_width().min(640.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - width) / 2.0);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        widgets::caption(ui, "SAVED CONNECTIONS");
                        ui.add_space(4.0);
                        for profile in saved {
                            let (rect, response) =
                                ui.allocate_exact_size(vec2(width, 52.0), Sense::click());
                            let fill = if response.hovered() {
                                p.raised
                            } else {
                                p.island
                            };
                            ui.painter().rect(
                                rect,
                                CornerRadius::same(9),
                                fill,
                                Stroke::new(1.0_f32, p.border),
                                egui::StrokeKind::Inside,
                            );
                            icons::paint(
                                ui.painter(),
                                egui::Rect::from_center_size(
                                    pos2(rect.left() + 24.0, rect.center().y),
                                    vec2(18.0, 18.0),
                                ),
                                Icon::Server,
                                p.leaf,
                            );
                            ui.painter().text(
                                pos2(rect.left() + 46.0, rect.top() + 17.0),
                                Align2::LEFT_CENTER,
                                &profile.name,
                                FontId::proportional(13.5),
                                p.text_strong,
                            );
                            ui.painter().text(
                                pos2(rect.left() + 46.0, rect.top() + 35.0),
                                Align2::LEFT_CENTER,
                                crate::values::truncate(
                                    &profile.display_uri(),
                                    ((width - 200.0) / 7.2) as usize,
                                ),
                                FontId::monospace(11.5),
                                p.text_info,
                            );
                            if !profile.folder.is_empty() {
                                ui.painter().text(
                                    pos2(rect.right() - 14.0, rect.center().y),
                                    Align2::RIGHT_CENTER,
                                    &profile.folder,
                                    FontId::proportional(11.5),
                                    p.text_secondary,
                                );
                            }
                            if response.clicked() {
                                connect = Some(profile.clone());
                            }
                            ui.add_space(6.0);
                        }
                    });
                });
            }
        });
    if let Some(profile) = connect {
        app.connect(profile);
    }
}
