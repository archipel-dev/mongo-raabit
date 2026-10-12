//! Aggregation Editor: stage list, stage editor with operator templates, per-stage
//! input/output preview and full pipeline runs.

use crate::app::{Load, MongoRaabit, ResultView, TabId, TabKind};
use crate::egui::{Id, Ui, vec2};
use crate::icons::Icon;
use crate::results;
use crate::stage_builder::{self, Mode, Suggestions};
use crate::tools::{AggregationTab, RunResult};
use crate::widgets;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::code::ExtendedJson;
use archipel_sdk::kit::components::{
    Banner, Button, Checkbox, CodeEditor, CodeView, Counter, Dropdown, IconButton, ListRow,
    MenuItem, MenuSeparator, NumberField, SegmentedControl, Separator, Severity, Spinner,
    SplitPane, Tag, TooltipExt,
};
use archipel_sdk::kit::parts::{Slot, scroll_vertical};
use archipel_sdk::kit::tokens::size;
use mongo_raabit_core::aggregate::{RunOptions, STAGE_OPERATORS, Stage, stage_operator};
use mongo_raabit_core::bson::Document;
use mongo_raabit_core::codegen::{CodeRequest, Operation};
use mongo_raabit_core::documents;
use mongo_raabit_core::format;
use mongo_raabit_core::stage_form;
use std::cell::RefCell;

enum Action {
    Preview,
    Run,
    Code,
}

/// Default share of the height the stage editor gets while results are shown.
const BODY_RATIO: f32 = 0.62;
const SAMPLE_WIDTH: f32 = 80.0;
const OPERATOR_WIDTH: f32 = 180.0;

/// The Aggregation tool window: Run and the preview sample, the stage list and the
/// editor of the selected stage. The stage's input and output and the pipeline's results
/// show in the center ([`output`]).
pub fn tool(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let collections = lookup_collections(app, tab_id);
    let read_only = app
        .aggregation_tab_mut(tab_id)
        .map(|t| t.session)
        .is_some_and(|s| app.session_read_only(s));
    let Some(tab) = app.aggregation_tab_mut(tab_id) else {
        return;
    };
    let mut actions = Vec::new();
    widgets::padded(ui, |ui| {
        header(ui, tab, read_only, &mut actions);
        ui.add_space(size::SECTION_SPACING);
        stage_list(ui, tab, &mut actions);
    });
    ui.add(Separator);
    widgets::padded(ui, |ui| {
        stage_editor(ui, tab, &collections, &mut actions);
    });
    apply(app, tab_id, actions);
}

fn apply(app: &mut MongoRaabit, tab_id: TabId, actions: Vec<Action>) {
    let mut code = false;
    for action in actions {
        match action {
            Action::Preview => app.preview_stage(tab_id),
            Action::Run => app.run_pipeline(tab_id),
            Action::Code => code = true,
        }
    }
    if code {
        open_code(app, tab_id);
    }
}

/// The Aggregation view of the center: the input and output of the selected stage and,
/// once the pipeline ran, its results below.
pub fn output(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let Some(tab) = app.aggregation_tab_mut(tab_id) else {
        return;
    };
    let mut actions = Vec::new();
    let idle = matches!(tab.result, Load::Idle);
    let size = ui.available_size();
    let tab_cell = RefCell::new(&mut *tab);
    let preview = |ui: &mut Ui| {
        widgets::padded(ui, |ui| preview_pane(ui, &tab_cell.borrow()));
    };
    ui.allocate_ui(size, |ui| {
        if idle {
            preview(ui);
            return;
        }
        let ratio_id = ui.id().with("raabit-agg-body-ratio");
        let mut ratio = ui
            .data(|d| d.get_temp::<f32>(ratio_id))
            .unwrap_or(BODY_RATIO);
        SplitPane::vertical(("raabit-agg-body", tab_id), &mut ratio)
            .default_ratio(BODY_RATIO)
            .min_sizes(size::CODE_LINE_HEIGHT * 6.0, size::CODE_LINE_HEIGHT * 6.0)
            .label("Resize results")
            .show(ui, preview, |ui| {
                let mut tab = tab_cell.borrow_mut();
                let tab = &mut *tab;
                result_pane(ui, tab_id, &mut tab.view, &mut tab.open, &tab.result);
            });
        ui.data_mut(|d| d.insert_temp(ratio_id, ratio));
    });
    if matches!(
        app.aggregation_tab_mut(tab_id).map(|t| &t.result),
        Some(Load::Idle)
    ) && ui.input(|i| i.modifiers.command && i.key_pressed(crate::egui::Key::Enter))
    {
        actions.push(Action::Run);
    }
    apply(app, tab_id, actions);
}

/// Run, the code of the pipeline and the sample size of the stage previews.
fn header(ui: &mut Ui, tab: &mut AggregationTab, read_only: bool, actions: &mut Vec<Action>) {
    // A pipeline ending in `$out` / `$merge` writes: blocked on a read-only connection.
    let blocked = read_only
        && tab
            .pipeline
            .documents(None)
            .is_ok_and(|stages| mongo_raabit_core::write::pipeline_writes(&stages));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        let run = ui.add(
            Button::primary("Run Pipeline")
                .icon(Icon::Play)
                .enabled(!blocked),
        );
        let run = if blocked {
            run.tooltip("Read-only connection: $out and $merge write to a collection")
        } else {
            run
        };
        if run.clicked() {
            actions.push(Action::Run);
        }
        if ui
            .add(IconButton::new(Icon::Code, "Query Code"))
            .tooltip("The pipeline as code (JS, Python, Java, C#, Go, Rust, PHP, Ruby)")
            .clicked()
        {
            actions.push(Action::Code);
        }
        if read_only {
            ui.add(Tag::new("Read-only").tone(Tone::Warning));
        }
    });
    ui.add_space(size::FORM_ROW_GAP);
    ui.horizontal(|ui| {
        widgets::label(ui, Tone::Muted, "Preview on first")
            .tooltip("Stage previews read only this many input documents (0 = all)");
        let id = ui.id().with("raabit-sample");
        let field = ui.add(
            NumberField::new(&mut tab.sample)
                .id(id)
                .name("Preview sample size")
                .width(SAMPLE_WIDTH),
        );
        // The preview runs when the user leaves the field after changing it.
        let dirty = id.with("dirty");
        if field.changed() {
            ui.data_mut(|d| d.insert_temp(dirty, true));
        }
        if field.lost_focus()
            && ui
                .data_mut(|d| d.remove_temp::<bool>(dirty))
                .unwrap_or(false)
        {
            actions.push(Action::Preview);
        }
        widgets::label(ui, Tone::Muted, "docs");
    });
}

/// Collection names of the tab's database, from the loaded tree (for `$lookup`).
fn lookup_collections(app: &MongoRaabit, tab_id: TabId) -> Vec<String> {
    let Some(TabKind::Collection(tab)) = app.tabs.iter().find(|t| t.id == tab_id).map(|t| &t.kind)
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

fn stage_list(ui: &mut Ui, tab: &mut AggregationTab, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        widgets::title(ui, "Pipeline");
        ui.add(Tag::new(format!("{} stages", tab.pipeline.stages.len())).tone(Tone::Secondary));
    });
    let mut select = None;
    let mut toggle = None;
    let mut remove = None;
    let mut move_by: Option<(usize, isize)> = None;
    scroll_vertical()
        .max_height(size::ROW_HEIGHT * 8.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (i, stage) in tab.pipeline.stages.iter().enumerate() {
                let selected = i == tab.selected;
                let mut enabled = stage.enabled;
                let tone = if stage.enabled {
                    Tone::Strong
                } else {
                    Tone::Muted
                };
                let leading_width = size::ICON + size::ICON_TEXT_GAP + size::ROW_HEIGHT;
                let trailing_width = 3.0 * size::ICON_BUTTON_SMALL + 2.0 * size::TOOLBAR_GAP;
                let row = ListRow::new(&stage.operator)
                    .secondary(stage.summary())
                    .label_tone(tone)
                    .selected(selected)
                    .leading(Slot::ui_width(leading_width, |ui| {
                        // Laid out right to left: the checkbox first.
                        ui.add(
                            Checkbox::new(&mut enabled, format!("Enable stage {}", i + 1))
                                .label_visible(false),
                        );
                        ui.add(Counter::new(i + 1).accent(stage.enabled));
                    }))
                    .trailing_on_hover(true)
                    .trailing(Slot::ui_width(trailing_width, |ui| {
                        if stage_button(ui, Icon::Trash, "Remove stage") {
                            remove = Some(i);
                        }
                        if stage_button(ui, Icon::ChevronDown, "Move down") {
                            move_by = Some((i, 1));
                        }
                        if stage_button(ui, Icon::ChevronUp, "Move up") {
                            move_by = Some((i, -1));
                        }
                    }))
                    .show(ui);
                if enabled != stage.enabled {
                    toggle = Some(i);
                } else if row.response.clicked() {
                    select = Some(i);
                }
            }
        });
    ui.add_space(size::SECTION_SPACING / 2.0);
    let mut add = None;
    let add_button = ui.add_sized(
        vec2(ui.available_width(), size::CONTROL_HEIGHT),
        Button::secondary("Add stage").icon(Icon::Plus),
    );
    crate::egui::Popup::menu(&add_button).show(|ui| {
        ui.set_min_width(size::COMPLETION_WIDTH);
        for (name, label, description) in stage_builder::FRIENDLY_STAGES {
            if ui
                .add(MenuItem::new(*label).description(*description))
                .clicked()
            {
                add = Some(*name);
            }
        }
        ui.add(MenuSeparator);
        let more = MenuItem::new("More stages…").submenu(|ui| {
            ui.set_min_width(size::COMPLETION_WIDTH);
            scroll_vertical()
                .max_height(ui.ctx().content_rect().height() / 2.0)
                .show(ui, |ui| {
                    for op in STAGE_OPERATORS {
                        if stage_builder::FRIENDLY_STAGES
                            .iter()
                            .any(|(name, _, _)| *name == op.name)
                        {
                            continue;
                        }
                        if ui
                            .add(MenuItem::new(op.name).description(op.description))
                            .clicked()
                        {
                            add = Some(op.name);
                        }
                    }
                });
        });
        ui.add(more);
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

/// A small action button in a stage row.
fn stage_button(ui: &mut Ui, icon: Icon, name: &str) -> bool {
    ui.add(IconButton::new(icon, name).small(true)).clicked()
}

fn stage_editor(
    ui: &mut Ui,
    tab: &mut AggregationTab,
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
        widgets::title(ui, format!("Stage {}", index + 1));
        let before = stage.operator.clone();
        ui.add(
            Dropdown::new(
                ("raabit-stage-op", index),
                &mut stage.operator,
                STAGE_OPERATORS.iter().map(|op| op.name.to_owned()),
            )
            .label("Stage operator")
            .width(OPERATOR_WIDTH),
        );
        if stage.operator != before {
            // Switching operator loads its template when the body was untouched.
            if stage_operator(&before).is_some_and(|o| o.template == stage.body) {
                stage.body = stage_operator(&stage.operator)
                    .map(|o| o.template.to_owned())
                    .unwrap_or_default();
            }
            actions.push(Action::Preview);
        }
    });
    if let Some(op) = stage_operator(&stage.operator) {
        widgets::note(ui, Tone::Muted, op.description);
    }
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
            if ui
                .add(SegmentedControl::new(
                    &mut mode,
                    [(Mode::Form, "Form"), (Mode::Code, "Code")],
                ))
                .changed()
            {
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
        });
        if mode == Mode::Form {
            widgets::note(
                ui,
                Tone::Muted,
                "Pick fields and values; the stage code is written for you.",
            );
        }
        if tab.form_hint == Some(index) && mode == Mode::Code {
            Banner::new(
                Severity::Warning,
                "This stage uses features the form cannot show; edit it in Code.",
            )
            .show(ui);
        }
    }
    let editor_height = if mode == Mode::Form {
        size::CODE_LINE_HEIGHT * 17.0
    } else {
        size::CODE_LINE_HEIGHT * 12.0
    };
    if mode == Mode::Form {
        scroll_vertical()
            .id_salt(("raabit-stage-body", index))
            .max_height(editor_height)
            .show(ui, |ui| {
                let suggest = Suggestions {
                    fields: &fields,
                    collections,
                };
                if stage_builder::stage_form(ui, stage, suggest, form_id) == Some(true)
                    && stage.to_document().is_ok()
                {
                    actions.push(Action::Preview);
                }
            });
    } else if ui
        .add(
            CodeEditor::new(&mut stage.body)
                .label("Stage body")
                .highlighter(&ExtendedJson)
                .id(Id::new(("raabit-stage-editor", index)))
                .height(editor_height),
        )
        .changed()
    {
        // Typing in the editor keeps the stage in Code mode.
        tab.stage_code.insert(index);
        if stage.to_document().is_ok() {
            actions.push(Action::Preview);
        }
    }
    if let Some(err) = &tab.stage_error {
        Banner::new(Severity::Error, err).show(ui);
    }
    if tab.preview.is_loading() {
        ui.horizontal(|ui| {
            ui.add(Spinner::new().label("Previewing"));
            widgets::label(ui, Tone::Secondary, "Previewing…");
        });
    }
}

/// The input and output of the selected stage, side by side.
fn preview_pane(ui: &mut Ui, tab: &AggregationTab) {
    let index = tab.selected;
    ui.horizontal(|ui| {
        widgets::title(ui, format!("Stage {}", index + 1));
        if let Some(stage) = tab.pipeline.stages.get(index) {
            ui.add(Tag::new(stage.operator.as_str()).tone(Tone::Accent));
            widgets::label(
                ui,
                Tone::Muted,
                "Preview of the stage selected in the Aggregation tool window",
            );
        }
    });
    ui.add_space(size::SECTION_SPACING);
    match &tab.preview {
        Load::Idle => {
            widgets::caption(ui, "Select a stage to preview its input and output.");
        }
        Load::Loading => {
            ui.horizontal(|ui| {
                ui.add(Spinner::new().label("Previewing"));
                widgets::label(ui, Tone::Secondary, "Previewing…");
            });
        }
        Load::Failed(err) => {
            Banner::new(Severity::Error, err).show(ui);
        }
        Load::Ready(preview) => {
            let width = (ui.available_width() - size::CONTENT_INSET) / 2.0;
            let height = ui.available_height();
            ui.horizontal_top(|ui| {
                preview_column(
                    ui,
                    "Stage input",
                    &preview.input,
                    None,
                    width,
                    height,
                    ("in", index),
                );
                ui.add_space(size::CONTENT_INSET);
                preview_column(
                    ui,
                    "Stage output",
                    &preview.output,
                    preview.skipped_reason.as_deref(),
                    width,
                    height,
                    ("out", index),
                );
            });
        }
    }
}

fn preview_column(
    ui: &mut Ui,
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
            widgets::title(ui, title);
            ui.add(Tag::new(format!("{} docs", docs.len())).tone(Tone::Secondary));
        });
        if let Some(note) = note {
            Banner::new(Severity::Warning, note).show(ui);
            return;
        }
        if docs.is_empty() {
            widgets::caption(ui, "No documents");
            return;
        }
        let text = documents::json_text(docs, 1);
        CodeView::new(&text)
            .id(crate::egui::Id::new(("raabit-preview", id)))
            .label(title)
            .highlighter(&ExtendedJson)
            .revision(crate::egui::util::hash(&text))
            .height(height - size::CONTROL_HEIGHT)
            .show(ui);
    });
}

/// Results of a full run, shared with the SQL tab.
pub fn result_pane(
    ui: &mut Ui,
    id: TabId,
    view: &mut ResultView,
    open: &mut std::collections::HashSet<String>,
    result: &Load<RunResult>,
) {
    widgets::padded(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::title(ui, "Results");
            ui.add(SegmentedControl::new(
                view,
                [(ResultView::Tree, "Tree"), (ResultView::Json, "JSON")],
            ));
            match result {
                Load::Ready(r) => {
                    ui.add(Tag::new(format!("{} documents", r.documents.len())).tone(Tone::Accent));
                    widgets::label(ui, Tone::Muted, format::duration(r.elapsed));
                    if r.documents.len() as u64 >= crate::tools::RUN_LIMIT {
                        widgets::label(
                            ui,
                            Tone::Muted,
                            format!("first {} shown", crate::tools::RUN_LIMIT),
                        );
                    }
                }
                Load::Loading => {
                    ui.add(Spinner::new().label("Running"));
                }
                _ => {}
            }
        });
    });
    match result {
        Load::Failed(err) => {
            widgets::padded(ui, |ui| {
                Banner::new(Severity::Error, err).show(ui);
            });
        }
        Load::Ready(r) => match view {
            ResultView::Json => results::json(ui, &r.documents, 1),
            _ => {
                if let Some(results::TreeEvent::Toggle(key)) = results::tree(
                    ui,
                    results::TreeState {
                        id: crate::egui::Id::new(("raabit-run-tree", id)),
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
