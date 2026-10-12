//! SQL Query tab: SQL editor, live MongoDB translation and results.

use crate::aggregation_view::result_pane;
use crate::app::{MongoRaabit, TabId};
use crate::egui::{Id, Ui};
use crate::icons::Icon;
use crate::tools::SqlTab;
use crate::widgets::{self, SqlHighlighter};
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::code::JavaScript;
use archipel_sdk::kit::components::{
    Banner, Button, CodeEditor, CodeView, IconButton, Severity, SplitPane, Tag, Tooltip, TooltipExt,
};
use archipel_sdk::kit::tokens::size;
use mongo_raabit_core::aggregate::Pipeline;
use mongo_raabit_core::sql::SqlTranslation;

/// Default share of the height the editors get while the results are below.
const EDITORS_RATIO: f32 = 0.45;
const EDITORS_MIN_HEIGHT: f32 = size::CODE_LINE_HEIGHT * 8.0;
const RESULTS_MIN_HEIGHT: f32 = size::CODE_LINE_HEIGHT * 6.0;
const PANE_MIN_WIDTH: f32 = 240.0;

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let Some(tab) = app.sql_tab_mut(tab_id) else {
        return;
    };
    let translation = tab.translation();
    let SqlTab {
        session,
        db,
        sql,
        result,
        view,
        open,
    } = tab;
    let mut run = false;
    let mut to_aggregation: Option<(String, Pipeline)> = None;
    widgets::tool_header(
        ui,
        Icon::Sql,
        Tone::Accent,
        &[],
        db,
        Tag::new("SQL Query").tone(Tone::Accent),
        |_| {},
        |ui| {
            run = ui
                .add(Button::primary("Run").icon(Icon::Play))
                .tooltip_with(Tooltip::new("Run").shortcut("Ctrl+Enter"))
                .clicked();
        },
    );
    let ratio_id = ui.id().with(("raabit-sql-ratio", tab_id));
    let mut ratio = ui
        .data(|d| d.get_temp::<f32>(ratio_id))
        .unwrap_or(EDITORS_RATIO);
    SplitPane::vertical(("raabit-sql-body", tab_id), &mut ratio)
        .default_ratio(EDITORS_RATIO)
        .min_sizes(EDITORS_MIN_HEIGHT, RESULTS_MIN_HEIGHT)
        .label("Resize results")
        .show(
            ui,
            |ui| editors(ui, tab_id, sql, &translation, &mut run, &mut to_aggregation),
            |ui| result_pane(ui, tab_id, view, open, result),
        );
    ui.data_mut(|d| d.insert_temp(ratio_id, ratio));
    let (session, db) = (*session, db.clone());
    if run {
        app.run_sql(tab_id);
    }
    if let Some((collection, pipeline)) = to_aggregation {
        app.open_aggregation(session, &db, &collection, Some(pipeline));
    }
}

/// The SQL editor and its MongoDB translation side by side.
fn editors(
    ui: &mut Ui,
    tab_id: TabId,
    sql: &mut String,
    translation: &Result<SqlTranslation, String>,
    run: &mut bool,
    to_aggregation: &mut Option<(String, Pipeline)>,
) {
    let ratio_id = ui.id().with(("raabit-sql-io-ratio", tab_id));
    let mut ratio = ui.data(|d| d.get_temp::<f32>(ratio_id)).unwrap_or(0.5);
    SplitPane::horizontal(("raabit-sql-io", tab_id), &mut ratio)
        .min_size(PANE_MIN_WIDTH)
        .label("Resize translation")
        .show(
            ui,
            |ui| {
                widgets::padded(ui, |ui| {
                    widgets::title(ui, "SQL");
                    let id = Id::new(("raabit-sql-editor", tab_id));
                    ui.add(
                        CodeEditor::new(sql)
                            .label("SQL")
                            .highlighter(&SqlHighlighter)
                            .id(id)
                            .height(ui.available_height()),
                    );
                    if ui.memory(|m| m.has_focus(id))
                        && ui.input(|i| {
                            i.modifiers.command && i.key_pressed(crate::egui::Key::Enter)
                        })
                    {
                        *run = true;
                    }
                });
            },
            |ui| {
                widgets::padded(ui, |ui| {
                    translation_pane(ui, tab_id, translation, to_aggregation)
                });
            },
        );
    ui.data_mut(|d| d.insert_temp(ratio_id, ratio));
}

/// The generated MongoDB query, or why the SQL cannot be translated.
fn translation_pane(
    ui: &mut Ui,
    tab_id: TabId,
    translation: &Result<SqlTranslation, String>,
    to_aggregation: &mut Option<(String, Pipeline)>,
) {
    ui.horizontal(|ui| {
        widgets::title(ui, "MongoDB query");
        match translation {
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
        if let Ok(t) = translation {
            ui.with_layout(
                crate::egui::Layout::right_to_left(crate::egui::Align::Center),
                |ui| {
                    if ui
                        .add(IconButton::new(
                            Icon::Pipeline,
                            "Open in Aggregation Editor",
                        ))
                        .tooltip("Open in Aggregation Editor")
                        .clicked()
                        && let Some(pipeline) = t.to_pipeline()
                    {
                        *to_aggregation = Some((t.collection().to_owned(), pipeline));
                    }
                    if ui
                        .add(IconButton::new(Icon::Copy, "Copy query"))
                        .tooltip("Copy query")
                        .clicked()
                    {
                        ui.ctx().copy_text(t.to_shell());
                    }
                },
            );
        }
    });
    match translation {
        Ok(t) => {
            let text = t.to_shell();
            CodeView::new(&text)
                .label("MongoDB query")
                .revision(crate::egui::util::hash(&text))
                .highlighter(&JavaScript)
                .id(Id::new(("raabit-sql-out", tab_id)))
                .height(ui.available_height())
                .show(ui);
        }
        Err(err) => {
            Banner::new(Severity::Error, err).show(ui);
        }
    }
}
