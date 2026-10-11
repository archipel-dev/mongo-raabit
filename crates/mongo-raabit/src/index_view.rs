//! Index Manager: list with sizes, usage and properties; create, hide/unhide and drop.

use crate::app::{Load, MongoRaabit, TabId};
use crate::icons::{self, Icon};
use crate::style::{self, Palette};
use crate::widgets;
use egui::{RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use mongo_raabit_core::shell::{self, ShellStyle};

enum Action {
    Create,
    Drop(String),
    Hide(String, bool),
    Refresh,
}

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let p = Palette::of(ui);
    let server = app
        .index_tab_mut(tab_id)
        .map(|t| t.session)
        .and_then(|s| app.session(s))
        .map(|s| s.profile.name.clone())
        .unwrap_or_default();
    let Some(tab) = app.index_tab_mut(tab_id) else {
        return;
    };
    let mut actions = Vec::new();
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(ui, Icon::Index, p.date);
                ui.label(RichText::new(&server).color(p.text_info));
                icons::icon(ui, Icon::ChevronRight, p.text_info);
                ui.label(RichText::new(&tab.db).color(p.text_secondary));
                icons::icon(ui, Icon::ChevronRight, p.text_info);
                ui.label(
                    RichText::new(&tab.collection)
                        .size(15.0)
                        .strong()
                        .color(p.text_strong),
                );
                widgets::badge(ui, "Indexes", p.date);
                if tab.busy {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, Some(Icon::Plus), "New Index").clicked() {
                        tab.form_open = !tab.form_open;
                    }
                    if widgets::icon_button(ui, Icon::Refresh, "Refresh").clicked() {
                        actions.push(Action::Refresh);
                    }
                });
            });
            if tab.form_open {
                ui.add_space(10.0);
                style::card(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("New index").strong());
                    ui.add_space(6.0);
                    widgets::form_row(ui, "Keys", 110.0, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut tab.form.keys)
                                .font(egui::FontId::monospace(12.5))
                                .desired_width(360.0),
                        )
                        .on_hover_text(
                            "1 / -1 for ascending / descending, \"text\", \"2dsphere\", \"hashed\"",
                        );
                    });
                    widgets::form_row(ui, "Name", 110.0, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut tab.form.name)
                                .hint_text(widgets::hint(ui, "generated from keys"))
                                .desired_width(360.0),
                        );
                    });
                    widgets::form_row(ui, "Options", 110.0, |ui| {
                        ui.checkbox(&mut tab.form.unique, "Unique");
                        ui.checkbox(&mut tab.form.sparse, "Sparse");
                        ui.checkbox(&mut tab.form.hidden, "Hidden");
                    });
                    widgets::form_row(ui, "TTL (seconds)", 110.0, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut tab.form.ttl)
                                .hint_text(widgets::hint(ui, "none"))
                                .desired_width(120.0),
                        );
                    });
                    widgets::form_row(ui, "Partial filter", 110.0, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut tab.form.partial)
                                .hint_text(widgets::hint(ui, "{ active: true }"))
                                .font(egui::FontId::monospace(12.5))
                                .desired_width(360.0),
                        );
                    });
                    ui.add_space(6.0);
                    widgets::form_row(ui, "", 110.0, |ui| {
                        if widgets::primary_button(ui, Some(Icon::Check), "Create Index").clicked()
                        {
                            actions.push(Action::Create);
                        }
                        if widgets::secondary_button(ui, None, "Cancel").clicked() {
                            tab.form_open = false;
                        }
                    });
                });
            }
            if let Some(err) = &tab.error {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    icons::icon(ui, Icon::Warning, p.error);
                    ui.label(RichText::new(err).color(p.error));
                });
            }
        });
    widgets::separator(ui);
    match &tab.indexes {
        Load::Loading | Load::Idle => {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.spinner());
        }
        Load::Failed(err) => {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(err).color(p.error)));
        }
        Load::Ready(indexes) => {
            let total: u64 = indexes.iter().filter_map(|i| i.size).sum();
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(14, 6))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} indexes · {}",
                            indexes.len(),
                            style::bytes(total)
                        ))
                        .color(p.text_secondary),
                    );
                });
            let max_ops = indexes
                .iter()
                .filter_map(|i| i.ops)
                .max()
                .unwrap_or(0)
                .max(1);
            let header_bg = p.raised;
            let confirm = tab.confirm_drop.clone();
            let mut new_confirm = confirm.clone();
            TableBuilder::new(ui)
                .id_salt(("raabit-indexes", tab_id))
                .striped(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(200.0).at_least(120.0).clip(true))
                .column(Column::initial(260.0).at_least(140.0).clip(true))
                .column(Column::initial(220.0).at_least(120.0))
                .column(Column::initial(90.0))
                .column(Column::initial(150.0))
                .column(Column::remainder().at_least(150.0))
                .header(26.0, |mut header| {
                    for title in ["Name", "Keys", "Properties", "Size", "Usage", ""] {
                        header.col(|ui| {
                            ui.painter().rect_filled(ui.max_rect().expand2(vec2(4.0, 0.0)), 0.0, header_bg);
                            ui.label(RichText::new(title).strong());
                        });
                    }
                })
                .body(|body| {
                    body.rows(30.0, indexes.len(), |mut row| {
                        let index = &indexes[row.index()];
                        row.col(|ui| {
                            icons::icon(ui, Icon::Index, if index.hidden { p.text_info } else { p.date });
                            ui.label(RichText::new(&index.name).font(egui::FontId::monospace(12.5)).color(if index.hidden { p.text_info } else { p.text_strong }));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(shell::document_to_shell(&index.keys, ShellStyle::Compact)).font(egui::FontId::monospace(12.0)).color(p.text_secondary));
                        });
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if index.unique {
                                widgets::badge(ui, "unique", p.accent_text);
                            }
                            if index.sparse {
                                widgets::badge(ui, "sparse", p.keyword);
                            }
                            if let Some(ttl) = index.ttl_seconds {
                                widgets::badge(ui, &format!("TTL {}", ttl_label(ttl)), p.date);
                            }
                            if let Some(filter) = &index.partial_filter {
                                widgets::badge(ui, "partial", p.string).on_hover_text(shell::document_to_shell(filter, ShellStyle::Compact));
                            }
                            if index.hidden {
                                widgets::badge(ui, "hidden", p.text_info);
                            }
                            if index.name == "_id_" {
                                widgets::badge(ui, "primary", p.leaf);
                            }
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(index.size.map(style::bytes).unwrap_or_default()).color(p.text_secondary));
                        });
                        row.col(|ui| {
                            // Usage: thin bar relative to the most used index, with the count as text.
                            let ops = index.ops.unwrap_or(0);
                            let (rect, _) = ui.allocate_exact_size(vec2(70.0, 6.0), Sense::hover());
                            ui.painter().rect_filled(rect, egui::CornerRadius::same(3), p.selection_inactive);
                            let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * ops as f32 / max_ops as f32, rect.height()));
                            ui.painter().rect_filled(filled, egui::CornerRadius::same(3), p.chart(0));
                            ui.label(RichText::new(match index.ops {
                                Some(n) => format!("{n} ops"),
                                None => "—".into(),
                            }).color(p.text_secondary));
                        });
                        row.col(|ui| {
                            if index.name == "_id_" {
                                return;
                            }
                            let label = if index.hidden { "Unhide" } else { "Hide" };
                            if ui.small_button(label).on_hover_text("Hidden indexes are kept up to date but not used by the planner").clicked() {
                                actions.push(Action::Hide(index.name.clone(), !index.hidden));
                            }
                            if confirm.as_deref() == Some(index.name.as_str()) {
                                let drop = egui::Button::new(RichText::new("Confirm drop").color(egui::Color32::WHITE)).fill(p.error).small();
                                if ui.add(drop).clicked() {
                                    actions.push(Action::Drop(index.name.clone()));
                                    new_confirm = None;
                                }
                            } else if ui.small_button("Drop…").clicked() {
                                new_confirm = Some(index.name.clone());
                            }
                        });
                    });
                });
            tab.confirm_drop = new_confirm;
        }
    }
    for action in actions {
        match action {
            Action::Create => app.create_index(tab_id),
            Action::Drop(name) => app.drop_index(tab_id, name),
            Action::Hide(name, hidden) => app.set_index_hidden(tab_id, name, hidden),
            Action::Refresh => app.load_indexes(tab_id),
        }
    }
}

fn ttl_label(seconds: i64) -> String {
    match seconds {
        s if s % 86_400 == 0 && s >= 86_400 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 && s >= 3600 => format!("{}h", s / 3600),
        s => format!("{s}s"),
    }
}
