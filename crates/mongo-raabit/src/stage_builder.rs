//! Visual forms for aggregation stages: the "Form" side of the stage editor.
//!
//! The form edits a draft [`StageForm`] kept in egui memory and rewrites the stage body
//! whenever the draft is valid, so previews, Query Code and runs keep reading the body.
//! A body changed elsewhere (Code mode, operator switch) is parsed again.

use crate::icons::{self, Icon};
use crate::style::Palette;
use crate::widgets;
use egui::{CornerRadius, FontId, Id, RichText, Stroke, Ui};
use mongo_raabit_core::aggregate::Stage;
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::stage_form::{
    Accumulator, AccumulatorRow, AddFieldRow, Combine, Condition, MatchOp, ProjectAction,
    ProjectRow, SortRow, StageForm, ValueSource,
};

/// Editing mode of one stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Form,
    Code,
}

/// Names offered by the field and collection pickers.
#[derive(Debug, Clone, Copy, Default)]
pub struct Suggestions<'a> {
    pub fields: &'a [String],
    pub collections: &'a [String],
}

/// Common stages with a readable label, in the order of the "Add stage" menu.
pub const FRIENDLY_STAGES: &[(&str, &str, &str)] = &[
    (
        "$match",
        "Filter documents ($match)",
        "Keep only documents that meet conditions",
    ),
    (
        "$group",
        "Group and summarize ($group)",
        "Totals, averages and counts per group",
    ),
    (
        "$sort",
        "Sort ($sort)",
        "Order documents by one or more fields",
    ),
    (
        "$project",
        "Choose fields ($project)",
        "Keep, remove or rename fields",
    ),
    ("$limit", "Limit ($limit)", "Keep only the first documents"),
    ("$skip", "Skip ($skip)", "Leave out the first documents"),
    (
        "$count",
        "Count ($count)",
        "Replace documents by their number",
    ),
    (
        "$unwind",
        "Unwind array ($unwind)",
        "One document per element of an array field",
    ),
    (
        "$lookup",
        "Join collection ($lookup)",
        "Attach matching documents from another collection",
    ),
    (
        "$addFields",
        "Add fields ($addFields)",
        "Add fields copied from others or set to a value",
    ),
];

/// Dotted field paths of `docs` (nested documents up to three levels), first seen first.
pub fn field_paths(docs: &[Document]) -> Vec<String> {
    fn walk(doc: &Document, prefix: &str, depth: usize, out: &mut Vec<String>) {
        for (key, value) in doc {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if !out.contains(&path) {
                out.push(path.clone());
            }
            if let Bson::Document(inner) = value
                && depth < 3
            {
                walk(inner, &path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for doc in docs.iter().take(200) {
        walk(doc, "", 1, &mut out);
        if out.len() > 400 {
            break;
        }
    }
    out
}

#[derive(Clone)]
struct Draft {
    operator: String,
    body: String,
    form: StageForm,
    error: Option<String>,
}

fn draft(ui: &Ui, id: Id, stage: &Stage) -> Option<Draft> {
    let cached = ui
        .data(|d| d.get_temp::<Draft>(id))
        .filter(|d| d.operator == stage.operator && d.body == stage.body);
    cached.or_else(|| {
        StageForm::parse(&stage.operator, &stage.body).map(|form| Draft {
            operator: stage.operator.clone(),
            body: stage.body.clone(),
            form,
            error: None,
        })
    })
}

/// Whether the stage can be shown as a form.
pub fn form_available(ui: &Ui, id: Id, stage: &Stage) -> bool {
    draft(ui, id, stage).is_some()
}

/// Draw the form of `stage`. Returns `None` when the body cannot be shown as a form,
/// otherwise whether `stage.body` was rewritten this frame.
pub fn stage_form(ui: &mut Ui, stage: &mut Stage, suggest: Suggestions, id: Id) -> Option<bool> {
    let mut draft = draft(ui, id, stage)?;
    let p = Palette::of(ui);
    let mut rewritten = false;
    let changed = egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            show(ui, &mut draft.form, suggest, id)
        })
        .inner;
    if changed {
        match draft.form.to_body() {
            Ok(body) => {
                if body != stage.body {
                    stage.body = body.clone();
                    rewritten = true;
                }
                draft.body = body;
                draft.error = None;
            }
            Err(err) => draft.error = Some(err.to_string()),
        }
    }
    if let Some(err) = &draft.error {
        ui.horizontal(|ui| {
            icons::icon(ui, Icon::Warning, p.warning);
            ui.label(RichText::new(err).color(p.warning));
        });
    }
    ui.data_mut(|d| d.insert_temp(id, draft));
    Some(rewritten)
}

/// Draw `form`. Returns true when the user changed it.
pub fn show(ui: &mut Ui, form: &mut StageForm, suggest: Suggestions, id: Id) -> bool {
    ui.spacing_mut().item_spacing.y = 6.0;
    match form {
        StageForm::Match(form) => {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Keep documents matching");
                changed |= choice(
                    ui,
                    id.with("combine"),
                    &mut form.combine,
                    &[(Combine::All, "all (AND)"), (Combine::Any, "any (OR)")],
                    96.0,
                );
                ui.label("of these conditions");
            });
            let mut remove = None;
            for (i, c) in form.conditions.iter_mut().enumerate() {
                let row = id.with(("cond", i));
                ui.horizontal(|ui| {
                    changed |= field_picker(ui, &mut c.field, suggest.fields, "field", 170.0, row);
                    let before = c.op;
                    egui::ComboBox::from_id_salt(row.with("op"))
                        .selected_text(c.op.label())
                        .width(84.0)
                        .show_ui(ui, |ui| {
                            for op in MatchOp::ALL {
                                ui.selectable_value(&mut c.op, op, op.label())
                                    .on_hover_text(op.description());
                            }
                        });
                    if c.op != before {
                        changed = true;
                        if c.op == MatchOp::Exists {
                            c.value = "true".into();
                        } else if before == MatchOp::Exists {
                            c.value.clear();
                        }
                    }
                    if c.op == MatchOp::Exists {
                        changed |= choice(
                            ui,
                            row.with("exists"),
                            &mut c.value,
                            &[("true".to_owned(), "yes"), ("false".to_owned(), "no")],
                            70.0,
                        );
                    } else {
                        let hint = match c.op {
                            MatchOp::In => "a, b, c",
                            MatchOp::Contains => "text",
                            _ => "value",
                        };
                        changed |= text_field(ui, &mut c.value, hint, 170.0);
                    }
                    if remove_button(ui) {
                        remove = Some(i);
                    }
                });
            }
            changed |= removed(&mut form.conditions, remove);
            if add_button(ui, "Condition") {
                form.conditions.push(Condition::default());
                changed = true;
            }
            if form.conditions.is_empty() {
                widgets::caption(ui, "No conditions: every document passes.");
            }
            changed
        }
        StageForm::Group(form) => {
            let mut changed = false;
            ui.label(RichText::new("Group by").strong());
            let mut remove = None;
            for (i, key) in form.keys.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    changed |=
                        field_picker(ui, key, suggest.fields, "field", 200.0, id.with(("key", i)));
                    if remove_button(ui) {
                        remove = Some(i);
                    }
                });
            }
            changed |= removed(&mut form.keys, remove);
            if form.keys.is_empty() {
                widgets::caption(ui, "No group key: all documents are summarized into one.");
            }
            if add_button(ui, "Group key") {
                form.keys.push(String::new());
                changed = true;
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Outputs").strong());
            let mut remove = None;
            for (i, row) in form.accumulators.iter_mut().enumerate() {
                let rid = id.with(("acc", i));
                ui.horizontal(|ui| {
                    changed |= text_field(ui, &mut row.name, "output name", 130.0);
                    ui.label("=");
                    egui::ComboBox::from_id_salt(rid.with("fn"))
                        .selected_text(row.function.label())
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for f in Accumulator::ALL {
                                changed |= ui
                                    .selectable_value(&mut row.function, f, f.label())
                                    .changed();
                            }
                        });
                    if row.function.needs_field() {
                        ui.label("of");
                        changed |=
                            field_picker(ui, &mut row.field, suggest.fields, "field", 170.0, rid);
                    } else {
                        ui.label(RichText::new("documents").color(Palette::of(ui).text_info));
                    }
                    if remove_button(ui) {
                        remove = Some(i);
                    }
                });
            }
            changed |= removed(&mut form.accumulators, remove);
            if add_button(ui, "Accumulator") {
                let name = if form.accumulators.is_empty() {
                    "count"
                } else {
                    ""
                };
                form.accumulators
                    .push(AccumulatorRow::new(name, Accumulator::Count, ""));
                changed = true;
            }
            changed
        }
        StageForm::Sort(form) => {
            let mut changed = false;
            let mut remove = None;
            for (i, row) in form.fields.iter_mut().enumerate() {
                let rid = id.with(("sort", i));
                ui.horizontal(|ui| {
                    changed |=
                        field_picker(ui, &mut row.field, suggest.fields, "field", 200.0, rid);
                    changed |= widgets::segmented(
                        ui,
                        &mut row.descending,
                        &[
                            (false, Icon::ChevronUp, "Ascending"),
                            (true, Icon::ChevronDown, "Descending"),
                        ],
                    );
                    if remove_button(ui) {
                        remove = Some(i);
                    }
                });
            }
            changed |= removed(&mut form.fields, remove);
            if add_button(ui, "Sort field") {
                form.fields.push(SortRow::default());
                changed = true;
            }
            changed
        }
        StageForm::Project(form) => {
            let mut changed = ui
                .checkbox(&mut form.exclude_id, "Hide _id")
                .on_hover_text("Leave the _id field out of the output")
                .changed();
            let mut remove = None;
            for (i, row) in form.fields.iter_mut().enumerate() {
                let rid = id.with(("proj", i));
                ui.horizontal(|ui| {
                    changed |=
                        field_picker(ui, &mut row.field, suggest.fields, "field", 200.0, rid);
                    let label = match &row.action {
                        ProjectAction::Include => "keep",
                        ProjectAction::Exclude => "remove",
                        ProjectAction::RenameTo(_) => "rename to",
                    };
                    egui::ComboBox::from_id_salt(rid.with("action"))
                        .selected_text(label)
                        .width(96.0)
                        .show_ui(ui, |ui| {
                            for (action, text) in [
                                (ProjectAction::Include, "keep"),
                                (ProjectAction::Exclude, "remove"),
                                (ProjectAction::RenameTo(String::new()), "rename to"),
                            ] {
                                let selected = std::mem::discriminant(&row.action)
                                    == std::mem::discriminant(&action);
                                if ui.selectable_label(selected, text).clicked() && !selected {
                                    row.action = action;
                                    changed = true;
                                }
                            }
                        });
                    if let ProjectAction::RenameTo(name) = &mut row.action {
                        changed |= text_field(ui, name, "new name", 140.0);
                    }
                    if remove_button(ui) {
                        remove = Some(i);
                    }
                });
            }
            changed |= removed(&mut form.fields, remove);
            if add_button(ui, "Field") {
                form.fields.push(ProjectRow::default());
                changed = true;
            }
            changed
        }
        StageForm::Limit(n) => number_row(ui, "Keep the first", n, "documents"),
        StageForm::Skip(n) => number_row(ui, "Skip the first", n, "documents"),
        StageForm::Count(name) => {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Count documents into the field");
                changed |= text_field(ui, name, "count", 160.0);
            });
            changed
        }
        StageForm::Unwind(form) => {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("One document per element of");
                changed |= field_picker(
                    ui,
                    &mut form.path,
                    suggest.fields,
                    "array field",
                    200.0,
                    id.with("path"),
                );
            });
            changed |= ui
                .checkbox(
                    &mut form.preserve_empty,
                    "Keep documents whose array is missing or empty",
                )
                .changed();
            changed
        }
        StageForm::Lookup(form) => {
            let mut changed = false;
            let label_width = 110.0;
            widgets::form_row(ui, "Join collection", label_width, |ui| {
                changed |= field_picker(
                    ui,
                    &mut form.from,
                    suggest.collections,
                    "collection",
                    220.0,
                    id.with("from"),
                );
            });
            widgets::form_row(ui, "Local field", label_width, |ui| {
                changed |= field_picker(
                    ui,
                    &mut form.local_field,
                    suggest.fields,
                    "field in this pipeline",
                    220.0,
                    id.with("local"),
                );
            });
            widgets::form_row(ui, "Foreign field", label_width, |ui| {
                changed |= text_field(
                    ui,
                    &mut form.foreign_field,
                    "field in joined collection",
                    220.0,
                );
            });
            widgets::form_row(ui, "Output array", label_width, |ui| {
                changed |= text_field(ui, &mut form.as_field, "new array field", 220.0);
            });
            changed
        }
        StageForm::AddFields(form) => {
            let mut changed = false;
            let mut remove = None;
            for (i, row) in form.fields.iter_mut().enumerate() {
                let rid = id.with(("add", i));
                ui.horizontal(|ui| {
                    changed |= text_field(ui, &mut row.name, "new field", 140.0);
                    ui.label("=");
                    changed |= choice(
                        ui,
                        rid.with("source"),
                        &mut row.source,
                        &[(ValueSource::Field, "field"), (ValueSource::Value, "value")],
                        70.0,
                    );
                    match row.source {
                        ValueSource::Field => {
                            changed |= field_picker(
                                ui,
                                &mut row.value,
                                suggest.fields,
                                "field",
                                180.0,
                                rid,
                            );
                        }
                        ValueSource::Value => {
                            changed |= text_field(ui, &mut row.value, "value", 180.0);
                        }
                    }
                    if remove_button(ui) {
                        remove = Some(i);
                    }
                });
            }
            changed |= removed(&mut form.fields, remove);
            if add_button(ui, "Field") {
                form.fields.push(AddFieldRow::default());
                changed = true;
            }
            changed
        }
    }
}

fn removed<T>(rows: &mut Vec<T>, index: Option<usize>) -> bool {
    match index {
        Some(i) if i < rows.len() => {
            rows.remove(i);
            true
        }
        _ => false,
    }
}

fn number_row(ui: &mut Ui, before: &str, value: &mut u64, after: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(before);
        changed |= ui
            .add(egui::DragValue::new(value).range(0..=u64::MAX).speed(1.0))
            .changed();
        ui.label(after);
    });
    changed
}

/// Small combo box over a fixed list of choices.
fn choice<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: Id,
    value: &mut T,
    options: &[(T, &str)],
    width: f32,
) -> bool {
    let mut changed = false;
    let current = options
        .iter()
        .find(|(v, _)| v == value)
        .map_or("…", |(_, label)| *label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .width(width)
        .show_ui(ui, |ui| {
            for (option, label) in options {
                let selected = option == value;
                if ui.selectable_label(selected, *label).clicked() && !selected {
                    *value = option.clone();
                    changed = true;
                }
            }
        });
    changed
}

fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> bool {
    let hint = widgets::hint(ui, hint);
    ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .font(FontId::monospace(12.5))
            .desired_width(width),
    )
    .changed()
}

/// Free-text field with a drop-down of suggested names.
fn field_picker(
    ui: &mut Ui,
    text: &mut String,
    suggestions: &[String],
    hint: &str,
    width: f32,
    id: Id,
) -> bool {
    let p = Palette::of(ui);
    let mut changed = false;
    egui::Frame::new()
        .fill(p.island)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                let hint = widgets::hint(ui, hint);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(text)
                            .id_salt(id.with("text"))
                            .hint_text(hint)
                            .font(FontId::monospace(12.5))
                            .frame(false)
                            .desired_width(width - 30.0),
                    )
                    .changed();
                let tip = if suggestions.is_empty() {
                    "No suggestions yet (they come from the stage input preview)"
                } else {
                    "Pick a name"
                };
                let button = widgets::icon_button_tinted(
                    ui,
                    Icon::ChevronDown,
                    tip,
                    None,
                    !suggestions.is_empty(),
                );
                egui::Popup::menu(&button).show(|ui| {
                    ui.set_min_width(width);
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for name in suggestions {
                                let selected = name == text;
                                if ui
                                    .selectable_label(
                                        selected,
                                        RichText::new(name).font(FontId::monospace(12.0)),
                                    )
                                    .clicked()
                                    && !selected
                                {
                                    text.clone_from(name);
                                    changed = true;
                                }
                            }
                        });
                });
            });
        });
    changed
}

fn remove_button(ui: &mut Ui) -> bool {
    widgets::icon_button(ui, Icon::Close, "Remove").clicked()
}

fn add_button(ui: &mut Ui, what: &str) -> bool {
    let p = Palette::of(ui);
    ui.add(egui::Button::new(RichText::new(format!("+ {what}")).color(p.accent_text)).frame(false))
        .clicked()
}
