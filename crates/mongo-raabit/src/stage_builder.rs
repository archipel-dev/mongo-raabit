//! Visual forms for aggregation stages: the "Form" side of the stage editor.
//!
//! The form edits a draft [`StageForm`] kept in egui memory and rewrites the stage body
//! whenever the draft is valid, so previews, Query Code and runs keep reading the body.
//! A body changed elsewhere (Code mode, operator switch) is parsed again.

use crate::egui::{Id, Ui};
use crate::icons::Icon;
use crate::widgets;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::{
    Banner, Button, Checkbox, FieldLabel, Form, FormUi, IconButton, SegmentedControl, Severity,
    TooltipExt,
};
use archipel_sdk::kit::parts::Surface;
use archipel_sdk::kit::tokens::size;
use mongo_raabit_core::aggregate::Stage;
use mongo_raabit_core::stage_form::{
    Accumulator, AccumulatorRow, AddFieldRow, Combine, Condition, MatchOp, ProjectAction,
    ProjectRow, SortRow, StageForm, ValueSource,
};

pub use mongo_raabit_core::fields::field_paths;

const PICKER_WIDTH: f32 = 200.0;
const WIDE_WIDTH: f32 = 240.0;
const VALUE_WIDTH: f32 = 180.0;
const NAME_WIDTH: f32 = 150.0;
const CHOICE_WIDTH: f32 = 120.0;
const NO_FIELDS: &str = "No suggestions yet (they come from the stage input preview)";
const NO_COLLECTIONS: &str = "No collections are known yet";

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
    let mut rewritten = false;
    let changed = Surface::Field
        .frame(ui.ctx())
        .inner_margin(crate::egui::Margin::same(size::CONTENT_INSET as i8))
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
        Banner::new(Severity::Warning, err).show(ui);
    }
    ui.data_mut(|d| d.insert_temp(id, draft));
    Some(rewritten)
}

/// Draw `form`. Returns true when the user changed it.
pub fn show(ui: &mut Ui, form: &mut StageForm, suggest: Suggestions, id: Id) -> bool {
    Form::new().show(ui, |rows| match form {
        StageForm::Match(form) => match_form(rows, form, suggest, id),
        StageForm::Group(form) => group_form(rows, form, suggest, id),
        StageForm::Sort(form) => sort_form(rows, form, suggest, id),
        StageForm::Project(form) => project_form(rows, form, suggest, id),
        StageForm::Limit(n) => number_row(rows, id, "Keep the first", n, "documents"),
        StageForm::Skip(n) => number_row(rows, id, "Skip the first", n, "documents"),
        StageForm::Count(name) => {
            let mut changed = false;
            rows.fields(|ui| {
                ui.add(FieldLabel::new("Count documents into the field"));
                changed |= text_field(ui, "Count field", name, "count", PICKER_WIDTH);
            });
            changed
        }
        StageForm::Unwind(form) => {
            let mut changed = false;
            rows.fields(|ui| {
                ui.add(FieldLabel::new("One document per element of"));
                changed |= widgets::name_picker(
                    ui,
                    id.with("path"),
                    "Array field",
                    &mut form.path,
                    suggest.fields,
                    "array field",
                    NO_FIELDS,
                    PICKER_WIDTH,
                );
            });
            changed |= rows
                .add(Checkbox::new(
                    &mut form.preserve_empty,
                    "Keep documents whose array is missing or empty",
                ))
                .changed();
            changed
        }
        StageForm::Lookup(form) => {
            let mut changed = false;
            rows.row("Join collection:", |ui| {
                changed |= widgets::name_picker(
                    ui,
                    id.with("from"),
                    "Join collection",
                    &mut form.from,
                    suggest.collections,
                    "collection",
                    NO_COLLECTIONS,
                    WIDE_WIDTH,
                );
            });
            rows.row("Local field:", |ui| {
                changed |= widgets::name_picker(
                    ui,
                    id.with("local"),
                    "Local field",
                    &mut form.local_field,
                    suggest.fields,
                    "field in this pipeline",
                    NO_FIELDS,
                    WIDE_WIDTH,
                );
            });
            rows.row("Foreign field:", |ui| {
                changed |= text_field(
                    ui,
                    "Foreign field",
                    &mut form.foreign_field,
                    "field in joined collection",
                    WIDE_WIDTH,
                );
            });
            rows.row("Output array:", |ui| {
                changed |= text_field(
                    ui,
                    "Output array",
                    &mut form.as_field,
                    "new array field",
                    WIDE_WIDTH,
                );
            });
            changed
        }
        StageForm::AddFields(form) => add_fields_form(rows, form, suggest, id),
    })
}

fn match_form(
    rows: &mut FormUi<'_>,
    form: &mut mongo_raabit_core::stage_form::MatchForm,
    suggest: Suggestions,
    id: Id,
) -> bool {
    let mut changed = false;
    rows.fields(|ui| {
        ui.add(FieldLabel::new("Keep documents matching"));
        changed |= ui
            .add(SegmentedControl::new(
                &mut form.combine,
                [(Combine::All, "all (AND)"), (Combine::Any, "any (OR)")],
            ))
            .changed();
        ui.add(FieldLabel::new("of these conditions"));
    });
    let mut remove = None;
    for (i, c) in form.conditions.iter_mut().enumerate() {
        let row = id.with(("cond", i));
        rows.fields(|ui| {
            changed |= widgets::name_picker(
                ui,
                row.with("field"),
                "Field",
                &mut c.field,
                suggest.fields,
                "field",
                NO_FIELDS,
                PICKER_WIDTH,
            );
            let before = c.op;
            let description = c.op.description();
            widgets::choice(
                ui,
                row.with("op"),
                "Operator",
                &mut c.op,
                MatchOp::ALL.iter().map(|o| (*o, o.label())),
                CHOICE_WIDTH,
            )
            .tooltip(description);
            if c.op != before {
                changed = true;
                if c.op == MatchOp::Exists {
                    c.value = "true".into();
                } else if before == MatchOp::Exists {
                    c.value.clear();
                }
            }
            if c.op == MatchOp::Exists {
                changed |= ui
                    .add(SegmentedControl::new(
                        &mut c.value,
                        [("true".to_owned(), "yes"), ("false".to_owned(), "no")],
                    ))
                    .changed();
            } else {
                let hint = match c.op {
                    MatchOp::In => "a, b, c",
                    MatchOp::Contains => "text",
                    _ => "value",
                };
                changed |= text_field(ui, "Value", &mut c.value, hint, VALUE_WIDTH);
            }
            if remove_button(ui) {
                remove = Some(i);
            }
        });
    }
    changed |= removed(&mut form.conditions, remove);
    if add_button(rows, "Condition") {
        form.conditions.push(Condition::default());
        changed = true;
    }
    if form.conditions.is_empty() {
        widgets::caption(rows.ui(), "No conditions: every document passes.");
    }
    changed
}

fn group_form(
    rows: &mut FormUi<'_>,
    form: &mut mongo_raabit_core::stage_form::GroupForm,
    suggest: Suggestions,
    id: Id,
) -> bool {
    let mut changed = false;
    rows.group("Group by", |rows| {
        let mut remove = None;
        for (i, key) in form.keys.iter_mut().enumerate() {
            rows.fields(|ui| {
                changed |= widgets::name_picker(
                    ui,
                    id.with(("key", i)),
                    "Group key",
                    key,
                    suggest.fields,
                    "field",
                    NO_FIELDS,
                    PICKER_WIDTH,
                );
                if remove_button(ui) {
                    remove = Some(i);
                }
            });
        }
        changed |= removed(&mut form.keys, remove);
        if form.keys.is_empty() {
            widgets::caption(
                rows.ui(),
                "No group key: all documents are summarized into one.",
            );
        }
        if add_button(rows, "Group key") {
            form.keys.push(String::new());
            changed = true;
        }
    });
    rows.group("Outputs", |rows| {
        let mut remove = None;
        for (i, row) in form.accumulators.iter_mut().enumerate() {
            let rid = id.with(("acc", i));
            rows.fields(|ui| {
                changed |= text_field(ui, "Output name", &mut row.name, "output name", NAME_WIDTH);
                ui.add(FieldLabel::new("="));
                changed |= widgets::choice(
                    ui,
                    rid.with("fn"),
                    "Accumulator",
                    &mut row.function,
                    Accumulator::ALL.iter().map(|f| (*f, f.label())),
                    CHOICE_WIDTH,
                )
                .changed();
                if row.function.needs_field() {
                    ui.add(FieldLabel::new("of"));
                    changed |= widgets::name_picker(
                        ui,
                        rid.with("field"),
                        "Accumulated field",
                        &mut row.field,
                        suggest.fields,
                        "field",
                        NO_FIELDS,
                        PICKER_WIDTH,
                    );
                } else {
                    widgets::label(ui, Tone::Muted, "documents");
                }
                if remove_button(ui) {
                    remove = Some(i);
                }
            });
        }
        changed |= removed(&mut form.accumulators, remove);
        if add_button(rows, "Accumulator") {
            let name = if form.accumulators.is_empty() {
                "count"
            } else {
                ""
            };
            form.accumulators
                .push(AccumulatorRow::new(name, Accumulator::Count, ""));
            changed = true;
        }
    });
    changed
}

fn sort_form(
    rows: &mut FormUi<'_>,
    form: &mut mongo_raabit_core::stage_form::SortForm,
    suggest: Suggestions,
    id: Id,
) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (i, row) in form.fields.iter_mut().enumerate() {
        let rid = id.with(("sort", i));
        rows.fields(|ui| {
            changed |= widgets::name_picker(
                ui,
                rid,
                "Sort field",
                &mut row.field,
                suggest.fields,
                "field",
                NO_FIELDS,
                PICKER_WIDTH,
            );
            changed |= ui
                .add(SegmentedControl::new(
                    &mut row.descending,
                    [(false, "Ascending"), (true, "Descending")],
                ))
                .changed();
            if remove_button(ui) {
                remove = Some(i);
            }
        });
    }
    changed |= removed(&mut form.fields, remove);
    if add_button(rows, "Sort field") {
        form.fields.push(SortRow::default());
        changed = true;
    }
    changed
}

fn project_form(
    rows: &mut FormUi<'_>,
    form: &mut mongo_raabit_core::stage_form::ProjectForm,
    suggest: Suggestions,
    id: Id,
) -> bool {
    let mut changed = rows
        .add(
            Checkbox::new(&mut form.exclude_id, "Hide _id")
                .comment("Leave the _id field out of the output"),
        )
        .changed();
    let mut remove = None;
    for (i, row) in form.fields.iter_mut().enumerate() {
        let rid = id.with(("proj", i));
        rows.fields(|ui| {
            changed |= widgets::name_picker(
                ui,
                rid,
                "Field",
                &mut row.field,
                suggest.fields,
                "field",
                NO_FIELDS,
                PICKER_WIDTH,
            );
            let mut kind = row.action.kind_index();
            let picked = widgets::choice(
                ui,
                rid.with("action"),
                "Action",
                &mut kind,
                ProjectAction::KIND_LABELS.iter().copied().enumerate(),
                CHOICE_WIDTH,
            )
            .changed();
            if picked {
                row.action = ProjectAction::from_kind_index(kind);
                changed = true;
            }
            if let ProjectAction::RenameTo(name) = &mut row.action {
                changed |= text_field(ui, "New name", name, "new name", NAME_WIDTH);
            }
            if remove_button(ui) {
                remove = Some(i);
            }
        });
    }
    changed |= removed(&mut form.fields, remove);
    if add_button(rows, "Field") {
        form.fields.push(ProjectRow::default());
        changed = true;
    }
    changed
}

fn add_fields_form(
    rows: &mut FormUi<'_>,
    form: &mut mongo_raabit_core::stage_form::AddFieldsForm,
    suggest: Suggestions,
    id: Id,
) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (i, row) in form.fields.iter_mut().enumerate() {
        let rid = id.with(("add", i));
        rows.fields(|ui| {
            changed |= text_field(ui, "New field", &mut row.name, "new field", NAME_WIDTH);
            ui.add(FieldLabel::new("="));
            changed |= ui
                .add(SegmentedControl::new(
                    &mut row.source,
                    [(ValueSource::Field, "field"), (ValueSource::Value, "value")],
                ))
                .changed();
            match row.source {
                ValueSource::Field => {
                    changed |= widgets::name_picker(
                        ui,
                        rid,
                        "Source field",
                        &mut row.value,
                        suggest.fields,
                        "field",
                        NO_FIELDS,
                        VALUE_WIDTH,
                    );
                }
                ValueSource::Value => {
                    changed |= text_field(ui, "Value", &mut row.value, "value", VALUE_WIDTH);
                }
            }
            if remove_button(ui) {
                remove = Some(i);
            }
        });
    }
    changed |= removed(&mut form.fields, remove);
    if add_button(rows, "Field") {
        form.fields.push(AddFieldRow::default());
        changed = true;
    }
    changed
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

fn number_row(rows: &mut FormUi<'_>, id: Id, before: &str, value: &mut u64, after: &str) -> bool {
    let mut changed = false;
    rows.fields(|ui| {
        ui.add(FieldLabel::new(before));
        changed |=
            widgets::number_field(ui, id.with("number"), before, value, CHOICE_WIDTH).changed;
        ui.add(FieldLabel::new(after));
    });
    changed
}

fn text_field(ui: &mut Ui, name: &str, text: &mut String, hint: &str, width: f32) -> bool {
    widgets::value_field(ui, name, text, hint, width, true)
        .response
        .changed()
}

fn remove_button(ui: &mut Ui) -> bool {
    ui.add(IconButton::new(Icon::Close, "Remove")).clicked()
}

fn add_button(rows: &mut FormUi<'_>, what: &str) -> bool {
    rows.fields(|ui| {
        ui.add(Button::ghost(what).icon(Icon::Plus).min_width(0.0))
            .clicked()
    })
}
