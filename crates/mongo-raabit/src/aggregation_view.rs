//! Aggregation Editor: stage list, stage editor with operator templates, per-stage
//! input/output preview and full pipeline runs.

use crate::app::{Load, MongoRaabit, ResultView, TabId, TabKind};
use crate::icons::{self, Icon};
use crate::results;
use crate::stage_builder::{self, Mode, Suggestions};
use crate::style::{self, Palette};
use crate::tools::RunResult;
use crate::values;
use crate::widgets;
use egui::{CornerRadius, FontId, RichText, Sense, Stroke, Ui, vec2};
use mongo_raabit_core::aggregate::{RunOptions, STAGE_OPERATORS, Stage, stage_operator};
use mongo_raabit_core::bson::Document;
use mongo_raabit_core::codegen::{CodeRequest, Operation};
use mongo_raabit_core::stage_form;

enum Action {
    Preview,
    Run,
    Code,
}

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let p = Palette::of(ui);
    let server = app
        .aggregation_tab_mut(tab_id)
        .map(|t| t.session)
        .and_then(|s| app.session(s))
        .map(|s| s.profile.name.clone())
        .unwrap_or_default();
    let collections = lookup_collections(app, tab_id);
    let Some(tab) = app.aggregation_tab_mut(tab_id) else {
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
                icons::icon(ui, Icon::Pipeline, p.accent_text);
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
                widgets::badge(ui, "Aggregation", p.accent_text);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, Some(Icon::Play), "Run Pipeline").clicked() {
                        actions.push(Action::Run);
                    }
                    if widgets::secondary_button(ui, Some(Icon::Code), "Query Code").clicked() {
                        actions.push(Action::Code);
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("docs").color(p.text_info));
                    let mut sample = tab.sample.to_string();
                    if ui
                        .add(egui::TextEdit::singleline(&mut sample).desired_width(56.0))
                        .lost_focus()
                        && let Ok(v) = sample.trim().parse()
                        && v != tab.sample
                    {
                        tab.sample = v;
                        actions.push(Action::Preview);
                    }
                    ui.label(RichText::new("Preview on first").color(p.text_info))
                        .on_hover_text(
                            "Stage previews read only this many input documents (0 = all)",
                        );
                });
            });
        });
    widgets::separator(ui);
    let results_height = if matches!(tab.result, Load::Idle) {
        0.0
    } else {
        (ui.available_height() * 0.38).max(180.0)
    };
    let body_height = ui.available_height() - results_height;
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), body_height),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            // ---- stage list
            egui::Frame::new()
                .fill(p.raised)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(270.0);
                    ui.set_height(body_height - 20.0);
                    ui.vertical(|ui| stage_list(ui, &p, tab, &mut actions));
                });
            // ---- stage editor and preview
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 14,
                    right: 14,
                    top: 10,
                    bottom: 6,
                })
                .show(ui, |ui| {
                    let width = ui.available_width() - 28.0;
                    ui.set_width(width);
                    ui.set_height(body_height - 16.0);
                    ui.vertical(|ui| stage_editor(ui, &p, tab, &collections, &mut actions));
                });
        },
    );
    if !matches!(tab.result, Load::Idle) {
        widgets::separator(ui);
        result_pane(ui, &p, tab_id, &mut tab.view, &mut tab.open, &tab.result);
    }
    let mut code = None;
    for action in actions {
        match action {
            Action::Preview => app.preview_stage(tab_id),
            Action::Run => app.run_pipeline(tab_id),
            Action::Code => code = Some(()),
        }
    }
    if code.is_some() {
        open_code(app, tab_id);
    }
}

/// Collection names of the tab's database, from the loaded tree (for `$lookup`).
fn lookup_collections(app: &MongoRaabit, tab_id: TabId) -> Vec<String> {
    let Some(TabKind::Aggregation(tab)) = app.tabs.iter().find(|t| t.id == tab_id).map(|t| &t.kind)
    else {
        return Vec::new();
    };
    app.session(tab.session)
        .and_then(|s| s.databases.ready())
        .and_then(|dbs| dbs.iter().find(|d| d.info.name == tab.db))
        .and_then(|d| d.collections.ready())
        .map(|colls| colls.iter().map(|c| c.info.name.clone()).collect())
        .unwrap_or_default()
}

fn open_code(app: &mut MongoRaabit, tab_id: TabId) {
    let Some(tab) = app.aggregation_tab_mut(tab_id) else {
        return;
    };
    let stages = match tab.pipeline.documents(None) {
        Ok(stages) => stages,
        Err(err) => {
            tab.stage_error = Some(err.to_string());
            return;
        }
    };
    let shell = tab
        .pipeline
        .to_shell(&tab.collection, RunOptions::default())
        .unwrap_or_default();
    let (session, db, coll) = (tab.session, tab.db.clone(), tab.collection.clone());
    let uri = app
        .session(session)
        .map(|s| s.profile.display_uri().replace(":****@", ":<password>@"))
        .unwrap_or_default();
    app.dialog = Some(crate::dialogs::Dialog::Code(crate::dialogs::CodeDialog {
        title: format!("Query Code  {db}.{coll}"),
        shell,
        request: CodeRequest {
            uri,
            database: db,
            collection: coll,
            operation: Operation::Aggregate(stages),
        },
        language: None,
    }));
}

fn stage_list(
    ui: &mut Ui,
    p: &Palette,
    tab: &mut crate::tools::AggregationTab,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Pipeline").strong());
        widgets::badge(
            ui,
            &format!("{} stages", tab.pipeline.stages.len()),
            p.text_secondary,
        );
    });
    ui.add_space(6.0);
    let mut select = None;
    let mut toggle = None;
    let mut remove = None;
    let mut move_by: Option<(usize, isize)> = None;
    egui::ScrollArea::vertical()
        .max_height(ui.available_height() - 40.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (i, stage) in tab.pipeline.stages.iter().enumerate() {
                let selected = i == tab.selected;
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::click());
                let fill = if selected {
                    p.selection
                } else if response.hovered() {
                    p.hover
                } else {
                    p.island
                };
                ui.painter().rect(
                    rect,
                    CornerRadius::same(8),
                    fill,
                    Stroke::new(1.0_f32, if selected { p.accent } else { p.border }),
                    egui::StrokeKind::Inside,
                );
                // Index bubble.
                let bubble = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 18.0, rect.top() + 18.0),
                    vec2(20.0, 20.0),
                );
                ui.painter().circle_filled(
                    bubble.center(),
                    10.0,
                    if stage.enabled {
                        p.accent.gamma_multiply(0.25)
                    } else {
                        p.selection_inactive
                    },
                );
                ui.painter().text(
                    bubble.center(),
                    egui::Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    FontId::proportional(11.0),
                    if stage.enabled {
                        p.accent_text
                    } else {
                        p.text_info
                    },
                );
                let name_color = if stage.enabled {
                    p.text_strong
                } else {
                    p.text_info
                };
                ui.painter().text(
                    egui::pos2(rect.left() + 36.0, rect.top() + 18.0),
                    egui::Align2::LEFT_CENTER,
                    &stage.operator,
                    FontId::monospace(13.0),
                    name_color,
                );
                let max_chars = ((rect.width() - 36.0 - 30.0) / 6.8).max(8.0) as usize;
                let summary = values::truncate(
                    &stage.body.split_whitespace().collect::<Vec<_>>().join(" "),
                    max_chars,
                );
                ui.painter().text(
                    egui::pos2(rect.left() + 36.0, rect.top() + 37.0),
                    egui::Align2::LEFT_CENTER,
                    summary,
                    FontId::monospace(11.0),
                    p.text_info,
                );
                // Row buttons.
                let button = |offset: f32, icon: Icon, tip: &str| -> bool {
                    let r = egui::Rect::from_center_size(
                        egui::pos2(rect.right() - offset, rect.top() + 18.0),
                        vec2(20.0, 20.0),
                    );
                    let resp = ui.interact(r, ui.id().with(("stage-btn", i, tip)), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(5), p.hover);
                    }
                    icons::paint(ui.painter(), r.shrink(3.0), icon, p.text_secondary);
                    resp.on_hover_text(tip).clicked()
                };
                if selected || response.hovered() {
                    if button(14.0, Icon::Trash, "Remove stage") {
                        remove = Some(i);
                    }
                    if button(36.0, Icon::ChevronDown, "Move down") {
                        move_by = Some((i, 1));
                    }
                    if button(58.0, Icon::ChevronUp, "Move up") {
                        move_by = Some((i, -1));
                    }
                }
                let check = egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 14.0, rect.top() + 39.0),
                    vec2(14.0, 14.0),
                );
                let check_resp =
                    ui.interact(check, ui.id().with(("stage-enable", i)), Sense::click());
                ui.painter().rect(
                    check,
                    CornerRadius::same(3),
                    if stage.enabled { p.accent } else { p.field },
                    Stroke::new(1.0_f32, p.border),
                    egui::StrokeKind::Inside,
                );
                if stage.enabled {
                    icons::paint(
                        ui.painter(),
                        check.shrink(1.0),
                        Icon::Check,
                        egui::Color32::WHITE,
                    );
                }
                if check_resp
                    .on_hover_text(if stage.enabled {
                        "Disable stage"
                    } else {
                        "Enable stage"
                    })
                    .clicked()
                {
                    toggle = Some(i);
                } else if response.clicked() {
                    select = Some(i);
                }
                ui.add_space(6.0);
            }
        });
    ui.add_space(4.0);
    let mut add = None;
    let add_button = ui.add_sized(
        vec2(ui.available_width(), 26.0),
        egui::Button::new(RichText::new("+  Add stage").color(p.accent_text)),
    );
    egui::Popup::menu(&add_button).show(|ui| {
        ui.set_min_width(260.0);
        for (name, label, description) in stage_builder::FRIENDLY_STAGES {
            let mut job = egui::text::LayoutJob::default();
            job.append(
                label,
                0.0,
                egui::TextFormat::simple(FontId::proportional(13.0), p.text_strong),
            );
            job.append(
                &format!("\n{description}"),
                0.0,
                egui::TextFormat::simple(FontId::proportional(11.5), p.text_info),
            );
            if ui.selectable_label(false, job).clicked() {
                add = Some(*name);
            }
        }
        ui.separator();
        ui.menu_button("More stages…", |ui| {
            ui.set_min_width(320.0);
            egui::ScrollArea::vertical()
                .max_height(380.0)
                .show(ui, |ui| {
                    for op in STAGE_OPERATORS {
                        if stage_builder::FRIENDLY_STAGES
                            .iter()
                            .any(|(name, _, _)| *name == op.name)
                        {
                            continue;
                        }
                        let label = format!("{:<16} {}", op.name, op.description);
                        if ui
                            .selectable_label(
                                false,
                                RichText::new(label).font(FontId::monospace(12.0)),
                            )
                            .clicked()
                        {
                            add = Some(op.name);
                        }
                    }
                });
        });
    });
    if let Some(name) = add {
        let at = (tab.selected + 1).min(tab.pipeline.stages.len());
        tab.insert_stage(at, Stage::new(name));
        tab.selected = at;
        actions.push(Action::Preview);
    }
    if let Some(i) = select {
        tab.selected = i;
        actions.push(Action::Preview);
    }
    if let Some(i) = toggle {
        tab.pipeline.stages[i].enabled = !tab.pipeline.stages[i].enabled;
        actions.push(Action::Preview);
    }
    if let Some((i, delta)) = move_by {
        let to = i as isize + delta;
        if to >= 0 && (to as usize) < tab.pipeline.stages.len() {
            tab.move_stage(i, to as usize);
            tab.selected = to as usize;
            actions.push(Action::Preview);
        }
    }
    if let Some(i) = remove {
        tab.remove_stage(i);
        tab.selected = tab
            .selected
            .min(tab.pipeline.stages.len().saturating_sub(1));
        actions.push(Action::Preview);
    }
}

fn stage_editor(
    ui: &mut Ui,
    p: &Palette,
    tab: &mut crate::tools::AggregationTab,
    collections: &[String],
    actions: &mut Vec<Action>,
) {
    let fields = match &tab.preview {
        Load::Ready(preview) => stage_builder::field_paths(&preview.input),
        _ => Vec::new(),
    };
    let Some(stage) = tab.pipeline.stages.get_mut(tab.selected) else {
        widgets::caption(ui, "Add a stage to start building the pipeline.");
        return;
    };
    let index = tab.selected;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Stage {}", index + 1)).strong());
        let before = stage.operator.clone();
        egui::ComboBox::from_id_salt(("raabit-stage-op", index))
            .selected_text(RichText::new(&stage.operator).font(FontId::monospace(13.0)))
            .width(180.0)
            .height(420.0)
            .show_ui(ui, |ui| {
                for op in STAGE_OPERATORS {
                    ui.selectable_value(&mut stage.operator, op.name.to_owned(), op.name);
                }
            });
        if stage.operator != before {
            // Switching operator loads its template when the body was untouched.
            if stage_operator(&before).is_some_and(|o| o.template == stage.body) {
                stage.body = stage_operator(&stage.operator)
                    .map(|o| o.template.to_owned())
                    .unwrap_or_default();
            }
            actions.push(Action::Preview);
        }
        if let Some(op) = stage_operator(&stage.operator) {
            ui.label(RichText::new(op.description).color(p.text_info));
        }
    });
    ui.add_space(6.0);
    let form_id = ui.make_persistent_id(("raabit-stage-form", index));
    let formable = stage_form::has_form(&stage.operator);
    let mut mode = if formable
        && !tab.stage_code.contains(&index)
        && stage_builder::form_available(ui, form_id, stage)
    {
        Mode::Form
    } else {
        Mode::Code
    };
    if formable {
        ui.horizontal(|ui| {
            if widgets::segmented(
                ui,
                &mut mode,
                &[
                    (Mode::Form, Icon::Edit, "Form"),
                    (Mode::Code, Icon::Code, "Code"),
                ],
            ) {
                match mode {
                    Mode::Form if stage_builder::form_available(ui, form_id, stage) => {
                        tab.stage_code.remove(&index);
                        tab.form_hint = None;
                    }
                    Mode::Form => {
                        tab.stage_code.insert(index);
                        tab.form_hint = Some(index);
                        mode = Mode::Code;
                    }
                    Mode::Code => {
                        tab.stage_code.insert(index);
                        tab.form_hint = None;
                    }
                }
            }
            if mode == Mode::Form {
                widgets::caption(
                    ui,
                    "Pick fields and values; the stage code is written for you.",
                );
            }
        });
        if tab.form_hint == Some(index) && mode == Mode::Code {
            ui.horizontal(|ui| {
                icons::icon(ui, Icon::Warning, p.warning);
                ui.label(
                    RichText::new(
                        "This stage uses features the form cannot show; edit it in Code.",
                    )
                    .color(p.warning),
                );
            });
        }
        ui.add_space(4.0);
    }
    let editor_height = if mode == Mode::Form {
        (ui.available_height() * 0.45).clamp(140.0, 340.0)
    } else {
        (ui.available_height() * 0.36).clamp(110.0, 260.0)
    };
    egui::ScrollArea::vertical()
        .id_salt(("raabit-stage-body", index, mode == Mode::Form))
        .max_height(editor_height)
        .show(ui, |ui| {
            if mode == Mode::Form {
                let suggest = Suggestions {
                    fields: &fields,
                    collections,
                };
                if stage_builder::stage_form(ui, stage, suggest, form_id) == Some(true)
                    && stage.to_document().is_ok()
                {
                    actions.push(Action::Preview);
                }
            } else if widgets::code_editor(ui, &mut stage.body, 7, ("raabit-stage-editor", index))
                .changed()
            {
                // Typing in the editor keeps the stage in Code mode.
                tab.stage_code.insert(index);
                if stage.to_document().is_ok() {
                    actions.push(Action::Preview);
                }
            }
        });
    if let Some(err) = &tab.stage_error {
        ui.horizontal(|ui| {
            icons::icon(ui, Icon::Warning, p.error);
            ui.label(RichText::new(err).color(p.error));
        });
    }
    ui.add_space(8.0);
    match &tab.preview {
        Load::Idle => {}
        Load::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Previewing…").color(p.text_secondary));
            });
        }
        Load::Failed(err) => {
            ui.add(egui::Label::new(RichText::new(err).color(p.error)).wrap());
        }
        Load::Ready(preview) => {
            let width = (ui.available_width() - 12.0) / 2.0;
            let height = ui.available_height();
            ui.horizontal_top(|ui| {
                preview_column(
                    ui,
                    p,
                    "Stage input",
                    &preview.input,
                    None,
                    width,
                    height,
                    ("in", index),
                );
                ui.add_space(12.0);
                preview_column(
                    ui,
                    p,
                    "Stage output",
                    &preview.output,
                    preview.skipped_reason.as_deref(),
                    width,
                    height,
                    ("out", index),
                );
            });
            let _ = style::duration(preview.elapsed);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn preview_column(
    ui: &mut Ui,
    p: &Palette,
    title: &str,
    docs: &[Document],
    note: Option<&str>,
    width: f32,
    height: f32,
    id: (&str, usize),
) {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            widgets::badge(ui, &format!("{} docs", docs.len()), p.text_secondary);
        });
        ui.add_space(4.0);
        if let Some(note) = note {
            egui::Frame::new()
                .fill(p.warning.gamma_multiply(0.14))
                .corner_radius(CornerRadius::same(7))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    ui.label(RichText::new(note).color(p.warning));
                });
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(height - 30.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for doc in docs {
                    egui::Frame::new()
                        .fill(p.field)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(7))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_width(width - 22.0);
                            let job = values::document_job(doc, p, FontId::monospace(11.5));
                            ui.add(egui::Label::new(job).wrap());
                        });
                    ui.add_space(6.0);
                }
                if docs.is_empty() {
                    widgets::caption(ui, "No documents");
                }
            });
    });
}

/// Results of a full run, shared with the SQL tab.
pub fn result_pane(
    ui: &mut Ui,
    p: &Palette,
    id: TabId,
    view: &mut ResultView,
    open: &mut std::collections::HashSet<String>,
    result: &Load<RunResult>,
) {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 8,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Results").strong());
                widgets::segmented(
                    ui,
                    view,
                    &[
                        (ResultView::Tree, Icon::Tree, "Tree"),
                        (ResultView::Json, Icon::Json, "JSON"),
                    ],
                );
                match result {
                    Load::Ready(r) => {
                        widgets::badge(
                            ui,
                            &format!("{} documents", r.documents.len()),
                            p.accent_text,
                        );
                        ui.label(RichText::new(style::duration(r.elapsed)).color(p.text_info));
                        if r.documents.len() as u64 >= crate::tools::RUN_LIMIT {
                            ui.label(
                                RichText::new(format!("first {} shown", crate::tools::RUN_LIMIT))
                                    .color(p.text_info),
                            );
                        }
                    }
                    Load::Loading => {
                        ui.spinner();
                    }
                    _ => {}
                }
            });
        });
    match result {
        Load::Failed(err) => {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(14, 4))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(err).color(p.error)).wrap());
                });
        }
        Load::Ready(r) => match view {
            ResultView::Json => results::json(ui, &r.documents, 1),
            _ => {
                if let Some(results::TreeEvent::Toggle(key)) = results::tree(
                    ui,
                    results::TreeState {
                        id: egui::Id::new(("raabit-run-tree", id)),
                        open,
                        selected: None,
                    },
                    &r.documents,
                    1,
                ) && !open.remove(&key)
                {
                    open.insert(key);
                }
            }
        },
        _ => {}
    }
}
