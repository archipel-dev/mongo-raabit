//! SQL Query tab: SQL editor, live MongoDB translation and results.

use crate::aggregation_view::result_pane;
use crate::app::{MongoRaabit, TabId};
use crate::egui::{CornerRadius, FontId, RichText, Stroke, Ui, vec2};
use crate::icons::{self, Icon};
use crate::style::Palette;
use crate::values;
use crate::widgets;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::{Button, Separator, Tag};
use mongo_raabit_core::aggregate::Pipeline;
use mongo_raabit_core::shell::{self, ShellStyle};
use mongo_raabit_core::sql::SqlTranslation;

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let p = Palette::of(ui);
    let Some(tab) = app.sql_tab_mut(tab_id) else {
        return;
    };
    let mut run = false;
    let mut to_aggregation: Option<(String, Pipeline)> = None;
    crate::egui::Frame::new()
        .inner_margin(crate::egui::Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(ui, Icon::Sql, p.keyword);
                ui.label(
                    RichText::new(&tab.db)
                        .size(15.0)
                        .strong()
                        .color(p.text_strong),
                );
                ui.add(Tag::new("SQL Query").tone(Tone::Accent));
                ui.with_layout(
                    crate::egui::Layout::right_to_left(crate::egui::Align::Center),
                    |ui| {
                        if ui
                            .add(Button::primary("Run").icon(Icon::Play))
                            .on_hover_text("Run (Ctrl+Enter)")
                            .clicked()
                        {
                            run = true;
                        }
                    },
                );
            });
        });
    ui.add(Separator);
    let translation = tab.translation();
    let editor_height = (ui.available_height() * 0.42).max(200.0);
    crate::egui::Frame::new()
        .inner_margin(crate::egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            let width = (ui.available_width() - 14.0) / 2.0;
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(width);
                    widgets::caption(ui, "SQL");
                    ui.add_space(2.0);
                    let font = FontId::monospace(13.0);
                    let palette = p;
                    let mut layouter =
                        |ui: &Ui, buffer: &dyn crate::egui::TextBuffer, wrap: f32| {
                            let mut job = values::sql_job(buffer.as_str(), &palette, font.clone());
                            job.wrap.max_width = wrap;
                            ui.fonts_mut(|f| f.layout_job(job))
                        };
                    crate::egui::Frame::new()
                        .fill(p.field)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(crate::egui::Margin::same(8))
                        .show(ui, |ui| {
                            crate::egui::ScrollArea::vertical()
                                .id_salt(("raabit-sql", tab_id))
                                .max_height(editor_height - 30.0)
                                .show(ui, |ui| {
                                    let response = ui.add(
                                        crate::egui::TextEdit::multiline(&mut tab.sql)
                                            .code_editor()
                                            .frame(false)
                                            .desired_rows(10)
                                            .desired_width(f32::INFINITY)
                                            .layouter(&mut layouter),
                                    );
                                    if response.has_focus()
                                        && ui.input(|i| {
                                            i.modifiers.command
                                                && i.key_pressed(crate::egui::Key::Enter)
                                        })
                                    {
                                        run = true;
                                    }
                                });
                        });
                });
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        widgets::caption(ui, "MONGODB QUERY");
                        match &translation {
                            Ok(SqlTranslation::Find { .. }) => {
                                ui.add(Tag::new("find").tone(Tone::Success));
                            }
                            Ok(SqlTranslation::Aggregate { .. }) => {
                                ui.add(Tag::new("aggregate").tone(Tone::Accent));
                            }
                            Err(_) => {
                                ui.add(Tag::new("not translatable").tone(Tone::Error));
                            }
                        }
                    });
                    ui.add_space(2.0);
                    crate::egui::Frame::new()
                        .fill(p.raised)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(crate::egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.set_min_height(editor_height - 70.0);
                            crate::egui::ScrollArea::both()
                                .id_salt(("raabit-sql-out", tab_id))
                                .max_height(editor_height - 70.0)
                                .show(ui, |ui| match &translation {
                                    Ok(t) => {
                                        let mut job = crate::egui::text::LayoutJob::default();
                                        values::highlight_into(
                                            &mut job,
                                            &t.to_shell(),
                                            &p,
                                            FontId::monospace(12.5),
                                        );
                                        ui.add(
                                            crate::egui::Label::new(job).selectable(true).extend(),
                                        );
                                    }
                                    Err(err) => {
                                        ui.horizontal_wrapped(|ui| {
                                            icons::icon(ui, Icon::Warning, p.error);
                                            ui.label(RichText::new(err).color(p.error));
                                        });
                                    }
                                });
                        });
                    ui.add_space(6.0);
                    if let Ok(t) = &translation {
                        ui.horizontal(|ui| {
                            if ui.add(Button::secondary("Copy").icon(Icon::Copy)).clicked() {
                                ui.ctx().copy_text(t.to_shell());
                            }
                            if ui
                                .add(
                                    Button::secondary("Open in Aggregation Editor")
                                        .icon(Icon::Pipeline),
                                )
                                .clicked()
                            {
                                let text = shell::to_shell_string(
                                    &mongo_raabit_core::bson::Bson::Array(
                                        t.pipeline().into_iter().map(Into::into).collect(),
                                    ),
                                    ShellStyle::Compact,
                                );
                                if let Ok(pipeline) = Pipeline::from_text(&text) {
                                    to_aggregation = Some((t.collection().to_owned(), pipeline));
                                }
                            }
                        });
                    }
                    let _ = vec2(0.0, 0.0);
                });
            });
        });
    ui.add(Separator);
    result_pane(ui, &p, tab_id, &mut tab.view, &mut tab.open, &tab.result);
    let (session, db) = (tab.session, tab.db.clone());
    if run {
        app.run_sql(tab_id);
    }
    if let Some((collection, pipeline)) = to_aggregation {
        app.open_aggregation(session, &db, &collection, Some(pipeline));
    }
}
