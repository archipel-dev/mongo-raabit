//! Visual Query Builder: filter conditions, sort and projection without writing code.
//!
//! The model lives in `mongo_raabit_core::builder`; this module draws it in the query
//! bar's place and reports when the generated query text changed or should run.

use crate::egui::{Id, Ui, vec2};
use crate::icons::Icon;
use crate::widgets;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::{
    Banner, Button, Checkbox, FieldLabel, Form, FormUi, IconButton, NumberField, SegmentedControl,
    Severity, Tag, TextField, Tooltip, TooltipExt,
};
use archipel_sdk::kit::parts::Surface;
use archipel_sdk::kit::tokens::{size, typography};
use mongo_raabit_core::builder::{
    Builder, Condition, Group, Item, MatchMode, Operands, Operator, ProjectionMode, SortKey,
    TypeName, ValueType,
};
use mongo_raabit_core::query::QueryText;

pub use mongo_raabit_core::fields::field_names;

/// Shown when a Code-mode query cannot be taken over.
pub const UNSUPPORTED_NOTE: &str =
    "The code query uses features the builder cannot show; edit it in Code mode.";

const FIELD_WIDTH: f32 = 170.0;
const OPERATOR_WIDTH: f32 = 130.0;
const TYPE_WIDTH: f32 = 110.0;
const MODE_WIDTH: f32 = 80.0;
const AND_WIDTH: f32 = 28.0;
const PICKER_WIDTH: f32 = 200.0;
const DIRECTION_WIDTH: f32 = 130.0;
const MODE_TIP: &str = "All: every condition must match (AND). Any: at least one (OR).";

/// What happened in the builder this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    None,
    Changed,
    Run,
}

/// Builder state of one collection tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryBuilder {
    pub model: Builder,
    /// Set when the last Code-mode query could not be taken over.
    pub note: Option<String>,
    /// What the builder could not show, for the note's tooltip.
    pub note_detail: String,
    pub show_sort: bool,
    pub show_projection: bool,
    pub show_paging: bool,
}

impl QueryBuilder {
    /// The query as shell text, as the Code mode shows it.
    pub fn to_query_text(&self) -> QueryText {
        self.model.to_query_text()
    }

    /// Takes over a query written in Code mode when the builder can represent it.
    pub fn take_over(&mut self, query: &QueryText) {
        if *query == self.to_query_text() {
            self.note = None;
            return;
        }
        match Builder::from_query_text(query) {
            Ok(model) => {
                self.show_sort |= !model.sort.is_empty();
                self.show_projection |=
                    !model.projection.fields.is_empty() || model.projection.exclude_id;
                self.show_paging |= model.skip > 0 || model.limit > 0;
                self.model = model;
                self.note = None;
            }
            Err(detail) => {
                self.note = Some(UNSUPPORTED_NOTE.into());
                self.note_detail = format!("Not representable: {detail}");
            }
        }
    }
}

pub fn show(ui: &mut Ui, builder: &mut QueryBuilder, fields: &[String]) -> Event {
    let before = builder.to_query_text();
    let id = ui.id().with("raabit-query-builder");
    let mut run = false;
    let mut reset = false;
    Surface::Island.frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        Form::new().show(ui, |form| {
            header(form, builder, id, &mut run, &mut reset);
            if let Some(note) = &builder.note {
                let text = format!("{note} {}", builder.note_detail);
                Banner::new(Severity::Warning, text.trim()).show(form.ui());
            }
            if builder.model.filter.items.is_empty() {
                widgets::label(
                    form.ui(),
                    Tone::Muted,
                    "No conditions: every document matches. Add a condition to filter.",
                );
            } else {
                run |= items(form, &mut builder.model.filter, fields, id.with("root"));
            }
            toolbar(form, builder);
            sort_section(
                form,
                &mut builder.show_sort,
                &mut builder.model.sort,
                fields,
                id.with("sort"),
            );
            projection_section(
                form,
                &mut builder.show_projection,
                &mut builder.model,
                fields,
                id.with("projection"),
            );
            paging_section(
                form,
                &mut builder.show_paging,
                &mut builder.model,
                id.with("paging"),
            );
            preview(form.ui(), &builder.to_query_text());
        });
    });
    run |=
        ui.input_mut(|i| i.consume_key(crate::egui::Modifiers::COMMAND, crate::egui::Key::Enter));
    if reset {
        *builder = QueryBuilder {
            show_sort: builder.show_sort,
            show_projection: builder.show_projection,
            show_paging: builder.show_paging,
            ..QueryBuilder::default()
        };
        run = true;
    }
    let changed = builder.to_query_text() != before;
    if changed {
        // The code query the note was about has been replaced now.
        builder.note = None;
    }
    if run {
        Event::Run
    } else if changed {
        Event::Changed
    } else {
        Event::None
    }
}

/// The root's All / Any, the incomplete-conditions tag and the Run and Reset buttons.
fn header(
    form: &mut FormUi<'_>,
    builder: &mut QueryBuilder,
    id: Id,
    run: &mut bool,
    reset: &mut bool,
) {
    let errors = builder.model.error_count();
    form.fields(|ui| {
        ui.add(FieldLabel::new("Match"));
        mode_choice(ui, id.with("root-mode"), &mut builder.model.filter.mode);
        ui.add(FieldLabel::new("of the following conditions"));
        if errors > 0 {
            let text = format!(
                "{errors} incomplete condition{}",
                if errors == 1 { "" } else { "s" }
            );
            ui.add(Tag::new(text).tone(Tone::Warning))
                .tooltip("Incomplete conditions are left out of the query");
        }
        ui.with_layout(
            crate::egui::Layout::right_to_left(crate::egui::Align::Center),
            |ui| {
                let run_button = ui.add(Button::primary("Run").icon(Icon::Play));
                if run_button
                    .tooltip_with(Tooltip::new("Run query").shortcut("Ctrl+Enter"))
                    .clicked()
                {
                    *run = true;
                }
                let reset_button = ui.add(Button::secondary("Reset"));
                if reset_button
                    .tooltip("Clear conditions, sort, projection, skip and limit")
                    .clicked()
                {
                    *reset = true;
                }
            },
        );
    });
}

/// "Add condition / group".
fn toolbar(form: &mut FormUi<'_>, builder: &mut QueryBuilder) {
    form.fields(|ui| {
        if ui
            .add(Button::secondary("Condition").icon(Icon::Plus))
            .tooltip("Add a condition")
            .clicked()
        {
            builder
                .model
                .filter
                .items
                .push(Item::Condition(Condition::default()));
        }
        if ui
            .add(Button::secondary("Group").icon(Icon::Plus))
            .tooltip("Add a group of conditions with its own All / Any")
            .clicked()
        {
            let group = builder.model.new_group();
            builder.model.filter.items.push(group);
        }
    });
}

fn mode_choice(ui: &mut Ui, id: Id, mode: &mut MatchMode) {
    widgets::choice(
        ui,
        id,
        "Match mode",
        mode,
        MatchMode::ALL.iter().map(|m| (*m, m.label())),
        MODE_WIDTH,
    )
    .tooltip(MODE_TIP);
}

/// The rows of a group. Returns true when Enter was pressed in a value field.
fn items(form: &mut FormUi<'_>, group: &mut Group, fields: &[String], id: Id) -> bool {
    let mut enter = false;
    let mut remove = None;
    for (index, item) in group.items.iter_mut().enumerate() {
        let item_id = id.with(index);
        let removed = match item {
            Item::Condition(condition) => {
                condition_row(form, condition, fields, item_id, &mut enter)
            }
            Item::Group(inner) => group_box(form, inner, fields, item_id, &mut enter),
        };
        if removed {
            remove = Some(index);
        }
    }
    if let Some(index) = remove {
        group.items.remove(index);
    }
    enter
}

/// A nested group: an indented box with its own All / Any. Returns true when removed.
fn group_box(
    form: &mut FormUi<'_>,
    group: &mut Group,
    fields: &[String],
    id: Id,
    enter: &mut bool,
) -> bool {
    let mut remove = false;
    form.indent(|form| {
        let ctx = form.ui().ctx().clone();
        Surface::Field
            .frame(&ctx)
            .inner_margin(crate::egui::Margin::same(size::SECTION_SPACING as i8))
            .show(form.ui(), |ui| {
                ui.set_width(ui.available_width());
                Form::new().show(ui, |inner| {
                    inner.fields(|ui| {
                        ui.add(FieldLabel::new("Match"));
                        mode_choice(ui, id.with("mode"), &mut group.mode);
                        ui.add(FieldLabel::new("of:"));
                        ui.with_layout(
                            crate::egui::Layout::right_to_left(crate::egui::Align::Center),
                            |ui| {
                                remove = ui
                                    .add(IconButton::new(Icon::Trash, "Remove group"))
                                    .clicked();
                                if ui
                                    .add(IconButton::new(
                                        Icon::Plus,
                                        "Add a condition to this group",
                                    ))
                                    .clicked()
                                {
                                    group.items.push(Item::Condition(Condition::default()));
                                }
                            },
                        );
                    });
                    if group.items.is_empty() {
                        widgets::label(
                            inner.ui(),
                            Tone::Muted,
                            "Empty group: it is left out of the query.",
                        );
                    }
                    *enter |= items(inner, group, fields, id.with("items"));
                });
            });
    });
    remove
}

/// One `field operator value type ×` row. Returns true when removed.
fn condition_row(
    form: &mut FormUi<'_>,
    condition: &mut Condition,
    fields: &[String],
    id: Id,
    enter: &mut bool,
) -> bool {
    let mut remove = false;
    form.fields(|ui| {
        TextField::new(&mut condition.field)
            .id(id.with("field"))
            .name("Field")
            .placeholder("field")
            .monospace(true)
            .width(FIELD_WIDTH)
            .suggestions(fields)
            .show(ui)
            .response
            .changed();
        widgets::choice(
            ui,
            id.with("operator"),
            "Operator",
            &mut condition.operator,
            Operator::ALL.iter().map(|o| (*o, o.label())),
            OPERATOR_WIDTH,
        );
        let spacing = ui.spacing().item_spacing.x;
        let tail = TYPE_WIDTH + size::ICON_BUTTON + spacing * 2.0;
        let width = (ui.available_width() - tail).max(size::COMPLETION_WIDTH / 3.0);
        let null = condition.value_type == ValueType::Null && condition.operator.uses_value_type();
        match condition.operator.operands() {
            Operands::None => {
                ui.allocate_space(vec2(width, size::CONTROL_HEIGHT));
            }
            Operands::One => {
                let hint = match condition.operator {
                    Operator::Contains | Operator::StartsWith | Operator::EndsWith => "text",
                    Operator::SizeIs => "number of elements",
                    _ => "value",
                };
                *enter |= value_edit(ui, "Value", &mut condition.value, hint, width, !null);
            }
            Operands::List => {
                *enter |= value_edit(ui, "Values", &mut condition.value, "a, b, c", width, !null);
            }
            Operands::Two => {
                let half = ((width - AND_WIDTH - spacing * 2.0) / 2.0).max(size::ICON * 2.0);
                *enter |= value_edit(ui, "From", &mut condition.value, "from", half, !null);
                ui.add(FieldLabel::new("and"));
                *enter |= value_edit(ui, "To", &mut condition.value2, "to", half, !null);
            }
            Operands::Type => {
                widgets::choice(
                    ui,
                    id.with("type-name"),
                    "Type",
                    &mut condition.type_name,
                    TypeName::ALL.iter().map(|t| (*t, t.alias())),
                    width,
                );
            }
        }
        if condition.operator.uses_value_type() {
            widgets::choice(
                ui,
                id.with("value-type"),
                "Value type",
                &mut condition.value_type,
                ValueType::ALL.iter().map(|t| (*t, t.label())),
                TYPE_WIDTH,
            )
            .tooltip("Value type (Auto infers numbers, booleans, dates and ObjectIds)");
        } else if condition.operator.is_text() {
            ui.add(Checkbox::new(&mut condition.ignore_case, "Ignore case"))
                .tooltip("Match upper and lower case alike");
        } else {
            ui.allocate_space(vec2(TYPE_WIDTH, size::CONTROL_HEIGHT));
        }
        remove = ui
            .add(IconButton::new(Icon::Close, "Remove condition"))
            .clicked();
    });
    if let Some(error) = condition.error() {
        // A fresh row only needs a gentle nudge; real mistakes are errors.
        let tone = if condition.field.trim().is_empty() {
            Tone::Muted
        } else {
            Tone::Error
        };
        widgets::note(form.ui(), tone, error);
    }
    remove
}

/// A single-line value field. Returns true when Enter was pressed in it.
fn value_edit(
    ui: &mut Ui,
    name: &str,
    text: &mut String,
    hint: &str,
    width: f32,
    enabled: bool,
) -> bool {
    widgets::value_field(ui, name, text, hint, width, enabled).submitted
}

/// A section title such as "Sort (2)": the label and how many settings it holds.
fn section_title(label: &str, count: usize) -> String {
    if count > 0 {
        format!("{label} ({count})")
    } else {
        label.to_owned()
    }
}

fn sort_section(
    form: &mut FormUi<'_>,
    open: &mut bool,
    sort: &mut Vec<SortKey>,
    fields: &[String],
    id: Id,
) {
    let title = section_title("Sort", sort.len());
    form.collapsible(&title, open, |form| {
        let mut remove = None;
        for (index, key) in sort.iter_mut().enumerate() {
            let key_id = id.with(index);
            form.fields(|ui| {
                TextField::new(&mut key.field)
                    .id(key_id.with("field"))
                    .name("Sort field")
                    .placeholder("field")
                    .monospace(true)
                    .width(PICKER_WIDTH)
                    .suggestions(fields)
                    .show(ui)
                    .response
                    .changed();
                ui.add(SegmentedControl::new(
                    &mut key.descending,
                    [(false, "Ascending"), (true, "Descending")],
                ));
                if ui
                    .add(IconButton::new(Icon::Close, "Remove sort field"))
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            sort.remove(index);
        }
        form.fields(|ui| {
            if ui
                .add(IconButton::new(Icon::Plus, "Add sort field"))
                .clicked()
            {
                sort.push(SortKey::default());
            }
            if sort.is_empty() {
                widgets::label(ui, Tone::Muted, "natural order");
            }
        });
    });
}

fn projection_section(
    form: &mut FormUi<'_>,
    open: &mut bool,
    model: &mut Builder,
    fields: &[String],
    id: Id,
) {
    let projection = &mut model.projection;
    let held = projection.fields.len() + usize::from(projection.exclude_id);
    let title = section_title("Projection", held);
    form.collapsible(&title, open, |form| {
        form.fields(|ui| {
            widgets::choice(
                ui,
                id.with("mode"),
                "Projection mode",
                &mut projection.mode,
                [
                    (ProjectionMode::Include, "Only"),
                    (ProjectionMode::Exclude, "All except"),
                ],
                DIRECTION_WIDTH,
            )
            .tooltip(
                "Only: return just the listed fields. All except: leave the listed fields out.",
            );
        });
        let mut remove = None;
        for (index, field) in projection.fields.iter_mut().enumerate() {
            form.fields(|ui| {
                TextField::new(field)
                    .id(id.with(index))
                    .name("Projected field")
                    .placeholder("field")
                    .monospace(true)
                    .width(PICKER_WIDTH)
                    .suggestions(fields)
                    .show(ui)
                    .response
                    .changed();
                if ui
                    .add(IconButton::new(Icon::Close, "Remove field"))
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            projection.fields.remove(index);
        }
        form.fields(|ui| {
            if ui.add(IconButton::new(Icon::Plus, "Add field")).clicked() {
                projection.fields.push(String::new());
            }
        });
        form.add(Checkbox::new(&mut projection.exclude_id, "Hide _id"));
    });
}

fn paging_section(form: &mut FormUi<'_>, open: &mut bool, model: &mut Builder, id: Id) {
    let held = usize::from(model.skip > 0) + usize::from(model.limit > 0);
    let title = section_title("Skip / Limit", held);
    form.collapsible(&title, open, |form| {
        form.row("Skip:", |ui| {
            ui.add(
                NumberField::new(&mut model.skip)
                    .id(id.with("skip"))
                    .name("Skip")
                    .width(TYPE_WIDTH),
            );
        });
        form.row("Limit:", |ui| {
            ui.add(
                NumberField::new(&mut model.limit)
                    .id(id.with("limit"))
                    .name("Limit")
                    .width(TYPE_WIDTH),
            );
            widgets::label(ui, Tone::Muted, "0 = no limit");
        });
    });
}

/// The generated query in shell syntax, so users learn the code.
fn preview(ui: &mut Ui, query: &QueryText) {
    let text = query.preview();
    let rich = crate::egui::RichText::new(&text)
        .font(crate::egui::FontId::monospace(typography::SMALL))
        .color(Tone::Muted.resolve(ui.ctx()));
    ui.add(crate::egui::Label::new(rich).truncate())
        .tooltip(text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use archipel_sdk::kit::{self, Theme};
    use mongo_raabit_core::bson::doc;

    fn render(builder: &mut QueryBuilder, fields: &[String], dark: bool) -> Vec<Event> {
        let ctx = crate::egui::Context::default();
        kit::install(&ctx, if dark { Theme::dark() } else { Theme::light() });
        let mut events = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run(crate::egui::RawInput::default(), |ctx| {
                crate::egui::CentralPanel::default().show(ctx, |ui| {
                    events.push(show(ui, builder, fields));
                });
            });
        }
        events
    }

    fn sample() -> QueryBuilder {
        let mut builder = QueryBuilder::default();
        builder.take_over(&QueryText {
            filter: r#"{ name: /^Ad/i, age: { $gte: 18, $lte: 65 }, tags: { $in: ["a", "b"] },
                         x: { $exists: false }, kind: { $type: "array" }, n: { $size: 2 },
                         $or: [{ tier: "pro" }, { score: { $gt: 9.5 } }] }"#
                .into(),
            projection: "{ name: 1, _id: 0 }".into(),
            sort: "{ age: -1 }".into(),
            skip: "5".into(),
            limit: "10".into(),
        });
        builder
    }

    #[test]
    fn takes_over_code_queries_or_keeps_state_with_a_note() {
        let builder = sample();
        assert!(builder.note.is_none());
        assert_eq!(builder.model.filter.items.len(), 7);
        assert!(builder.show_sort && builder.show_projection && builder.show_paging);

        let mut kept = builder.clone();
        kept.take_over(&QueryText {
            filter: "{ $where: 'this.a > 1' }".into(),
            ..QueryText::default()
        });
        assert_eq!(kept.model, builder.model);
        assert_eq!(kept.note.as_deref(), Some(UNSUPPORTED_NOTE));
        assert!(kept.note_detail.contains("$where"));

        // Taking over its own text is a no-op that clears the note.
        let text = kept.to_query_text();
        kept.take_over(&text);
        assert!(kept.note.is_none());
    }

    #[test]
    fn renders_every_operator_in_both_themes() {
        let fields = vec!["_id".to_owned(), "name".to_owned()];
        let mut builder = sample();
        for operator in Operator::ALL {
            builder
                .model
                .filter
                .items
                .push(Item::Condition(Condition::new("name", operator, "x")));
        }
        builder
            .model
            .filter
            .items
            .push(Item::Condition(Condition::default()));
        builder
            .model
            .filter
            .items
            .push(Item::Group(Group::default()));
        for dark in [true, false] {
            let events = render(&mut builder, &fields, dark);
            assert_eq!(events, [Event::None, Event::None]);
        }
        let mut empty = QueryBuilder::default();
        assert_eq!(render(&mut empty, &[], true), [Event::None, Event::None]);
        empty.note = Some(UNSUPPORTED_NOTE.into());
        render(&mut empty, &[], false);
    }

    #[test]
    fn generated_query_parses_for_the_tab() {
        let builder = sample();
        let text = builder.to_query_text();
        let query = text.parse().unwrap();
        assert_eq!(query.skip, 5);
        assert_eq!(query.limit, 10);
        assert_eq!(query.sort, Some(doc! {"age": -1}));
        assert!(text.preview().starts_with("find({ name: /^Ad/i"));
        assert!(
            text.preview()
                .ends_with(".sort({ age: -1 }).skip(5).limit(10)")
        );
    }

    #[test]
    fn ctrl_enter_runs() {
        let ctx = crate::egui::Context::default();
        let mut builder = QueryBuilder::default();
        let mut events = Vec::new();
        for frame in 0..2 {
            let mut input = crate::egui::RawInput::default();
            if frame == 1 {
                input.modifiers = crate::egui::Modifiers::COMMAND;
                input.events.push(crate::egui::Event::Key {
                    key: crate::egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: crate::egui::Modifiers::COMMAND,
                });
            }
            let _ = ctx.run(input, |ctx| {
                crate::egui::CentralPanel::default().show(ctx, |ui| {
                    events.push(show(ui, &mut builder, &[]));
                });
            });
        }
        assert_eq!(events, [Event::None, Event::Run]);
    }
}
