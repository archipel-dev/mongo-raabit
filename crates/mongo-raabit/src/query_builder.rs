//! Visual Query Builder: filter conditions, sort and projection without writing code.
//!
//! The model lives in `mongo_raabit_core::builder`; this module draws it in the query
//! bar's place and reports when the generated query text changed or should run.

use crate::icons::{self, Icon};
use crate::style::Palette;
use crate::widgets;
use egui::{CornerRadius, FontId, Id, Rect, RichText, Sense, Stroke, Ui, Vec2, vec2};
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::builder::{
    Builder, Condition, Group, Item, MatchMode, Operands, Operator, ProjectionMode, SortKey,
    TypeName, ValueType,
};
use mongo_raabit_core::query::QueryText;
use std::collections::BTreeSet;

/// Shown when a Code-mode query cannot be taken over.
pub const UNSUPPORTED_NOTE: &str =
    "The code query uses features the builder cannot show; edit it in Code mode.";

/// Field paths offered in the field pickers.
const MAX_FIELDS: usize = 200;
/// Nesting depth walked by [`field_names`].
const MAX_FIELD_DEPTH: usize = 8;

const FIELD_WIDTH: f32 = 150.0;
const OPERATOR_WIDTH: f32 = 118.0;
const TYPE_WIDTH: f32 = 78.0;

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

/// Field paths seen in `documents` (dotted for nested fields, including the fields of
/// documents inside arrays), `_id` first, then sorted.
pub fn field_names(documents: &[Document]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    for doc in documents {
        collect_fields(doc, "", 0, &mut seen);
    }
    let mut names: Vec<String> = seen.into_iter().collect();
    if let Some(pos) = names.iter().position(|f| f == "_id") {
        let id = names.remove(pos);
        names.insert(0, id);
    }
    names.truncate(MAX_FIELDS);
    names
}

fn collect_fields(doc: &Document, prefix: &str, depth: usize, seen: &mut BTreeSet<String>) {
    for (key, value) in doc {
        // Bound the work on very wide documents; the list is capped anyway.
        if seen.len() >= MAX_FIELDS * 5 {
            return;
        }
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if depth < MAX_FIELD_DEPTH {
            match value {
                Bson::Document(inner) => collect_fields(inner, &path, depth + 1, seen),
                Bson::Array(items) => {
                    for item in items.iter().filter_map(Bson::as_document) {
                        collect_fields(item, &path, depth + 1, seen);
                    }
                }
                _ => {}
            }
        }
        seen.insert(path);
    }
}

pub fn show(ui: &mut Ui, p: &Palette, builder: &mut QueryBuilder, fields: &[String]) -> Event {
    let before = builder.to_query_text();
    let id = ui.id().with("raabit-query-builder");
    let mut run = false;
    let mut reset = false;
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Match").color(p.text_secondary));
                mode_combo(ui, id.with("root-mode"), &mut builder.model.filter.mode);
                ui.label(RichText::new("of the following conditions").color(p.text_secondary));
                let errors = builder.model.error_count();
                if errors > 0 {
                    widgets::badge(
                        ui,
                        &format!(
                            "{errors} incomplete condition{}",
                            if errors == 1 { "" } else { "s" }
                        ),
                        p.warning,
                    )
                    .on_hover_text("Incomplete conditions are left out of the query");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, Some(Icon::Play), "Run")
                        .on_hover_text("Run query (Ctrl+Enter)")
                        .clicked()
                    {
                        run = true;
                    }
                    if widgets::secondary_button(ui, None, "Reset")
                        .on_hover_text("Clear conditions, sort, projection, skip and limit")
                        .clicked()
                    {
                        reset = true;
                    }
                });
            });
            if let Some(note) = &builder.note {
                ui.horizontal(|ui| {
                    icons::icon(ui, Icon::Warning, p.warning);
                    ui.label(RichText::new(note).color(p.warning))
                        .on_hover_text(&builder.note_detail);
                });
            }
            if builder.model.filter.items.is_empty() {
                ui.label(widgets::hint(
                    ui,
                    "No conditions: every document matches. Add a condition to filter.",
                ));
            } else {
                run |= items(ui, p, &mut builder.model.filter, fields, id.with("root"), 0);
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if widgets::secondary_button(ui, Some(Icon::Plus), "Condition")
                    .on_hover_text("Add a condition")
                    .clicked()
                {
                    builder
                        .model
                        .filter
                        .items
                        .push(Item::Condition(Condition::default()));
                }
                if widgets::secondary_button(ui, Some(Icon::Plus), "Group")
                    .on_hover_text("Add a group of conditions with its own All / Any")
                    .clicked()
                {
                    builder.model.filter.items.push(new_group(&builder.model));
                }
                ui.add_space(12.0);
                let sort = builder.model.sort.len();
                section_toggle(ui, p, &mut builder.show_sort, "Sort", sort);
                let projection = &builder.model.projection;
                let projected = projection.fields.len() + usize::from(projection.exclude_id);
                section_toggle(ui, p, &mut builder.show_projection, "Projection", projected);
                let paged =
                    usize::from(builder.model.skip > 0) + usize::from(builder.model.limit > 0);
                section_toggle(ui, p, &mut builder.show_paging, "Skip / Limit", paged);
            });
            if builder.show_sort {
                sort_section(ui, p, &mut builder.model.sort, fields, id.with("sort"));
            }
            if builder.show_projection {
                projection_section(ui, p, &mut builder.model, fields, id.with("projection"));
            }
            if builder.show_paging {
                paging_section(ui, p, &mut builder.model);
            }
            preview(ui, p, &builder.to_query_text());
        });
    run |= ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
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

/// A new group combines with the opposite mode of the root, which is what groups are for.
fn new_group(model: &Builder) -> Item {
    let mode = match model.filter.mode {
        MatchMode::All => MatchMode::Any,
        MatchMode::Any => MatchMode::All,
    };
    Item::Group(Group {
        mode,
        items: vec![Item::Condition(Condition::default())],
    })
}

fn mode_combo(ui: &mut Ui, id: Id, mode: &mut MatchMode) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(mode.label())
        .width(58.0)
        .show_ui(ui, |ui| {
            for option in MatchMode::ALL {
                let tip = match option {
                    MatchMode::All => "Every condition must match (AND)",
                    MatchMode::Any => "At least one condition must match (OR)",
                };
                ui.selectable_value(mode, option, option.label())
                    .on_hover_text(tip);
            }
        });
}

/// The rows of a group. Returns true when Enter was pressed in a value field.
fn items(
    ui: &mut Ui,
    p: &Palette,
    group: &mut Group,
    fields: &[String],
    id: Id,
    depth: usize,
) -> bool {
    let mut enter = false;
    let mut remove = None;
    for (index, item) in group.items.iter_mut().enumerate() {
        let item_id = id.with(index);
        let removed = match item {
            Item::Condition(condition) => {
                condition_row(ui, p, condition, fields, item_id, &mut enter)
            }
            Item::Group(inner) => group_box(ui, p, inner, fields, item_id, depth + 1, &mut enter),
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
    ui: &mut Ui,
    p: &Palette,
    group: &mut Group,
    fields: &[String],
    id: Id,
    depth: usize,
    enter: &mut bool,
) -> bool {
    let mut remove = false;
    egui::Frame::new()
        .fill(if p.dark { p.field } else { p.island })
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(6))
        .outer_margin(egui::Margin {
            left: 18,
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Match").color(p.text_secondary));
                mode_combo(ui, id.with("mode"), &mut group.mode);
                ui.label(RichText::new("of:").color(p.text_secondary));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    remove = widgets::icon_button(ui, Icon::Trash, "Remove group").clicked();
                    if widgets::icon_button(ui, Icon::Plus, "Add a condition to this group")
                        .clicked()
                    {
                        group.items.push(Item::Condition(Condition::default()));
                    }
                });
            });
            if group.items.is_empty() {
                ui.label(widgets::hint(
                    ui,
                    "Empty group: it is left out of the query.",
                ));
            }
            *enter |= items(ui, p, group, fields, id.with("items"), depth);
        });
    remove
}

/// One `field operator value type ×` row. Returns true when removed.
fn condition_row(
    ui: &mut Ui,
    p: &Palette,
    condition: &mut Condition,
    fields: &[String],
    id: Id,
    enter: &mut bool,
) -> bool {
    let mut remove = false;
    ui.horizontal(|ui| {
        field_picker(
            ui,
            p,
            &mut condition.field,
            fields,
            id.with("field"),
            FIELD_WIDTH,
        );
        egui::ComboBox::from_id_salt(id.with("operator"))
            .selected_text(condition.operator.label())
            .width(OPERATOR_WIDTH)
            .show_ui(ui, |ui| {
                for operator in Operator::ALL {
                    ui.selectable_value(&mut condition.operator, operator, operator.label());
                }
            });
        let spacing = ui.spacing().item_spacing.x;
        let tail = TYPE_WIDTH + 26.0 + spacing * 2.0 + 8.0;
        let width = (ui.available_width() - tail).max(80.0);
        let null = condition.value_type == ValueType::Null && condition.operator.uses_value_type();
        match condition.operator.operands() {
            Operands::None => {
                ui.allocate_space(vec2(width, 20.0));
            }
            Operands::One => {
                let hint = match condition.operator {
                    Operator::Contains | Operator::StartsWith | Operator::EndsWith => "text",
                    Operator::SizeIs => "number of elements",
                    _ => "value",
                };
                *enter |= value_edit(ui, &mut condition.value, hint, width, !null);
            }
            Operands::List => {
                *enter |= value_edit(ui, &mut condition.value, "a, b, c", width, !null);
            }
            Operands::Two => {
                let and = 26.0;
                let half = ((width - and - spacing * 2.0) / 2.0).max(40.0);
                *enter |= value_edit(ui, &mut condition.value, "from", half, !null);
                ui.allocate_ui(vec2(and, 20.0), |ui| {
                    ui.label(RichText::new("and").color(p.text_secondary));
                });
                *enter |= value_edit(ui, &mut condition.value2, "to", half, !null);
            }
            Operands::Type => {
                egui::ComboBox::from_id_salt(id.with("type-name"))
                    .selected_text(condition.type_name.alias())
                    .width(width - 8.0)
                    .show_ui(ui, |ui| {
                        for name in TypeName::ALL {
                            ui.selectable_value(&mut condition.type_name, name, name.alias());
                        }
                    });
            }
        }
        if condition.operator.uses_value_type() {
            egui::ComboBox::from_id_salt(id.with("value-type"))
                .selected_text(condition.value_type.label())
                .width(TYPE_WIDTH - 8.0)
                .show_ui(ui, |ui| {
                    for value_type in ValueType::ALL {
                        ui.selectable_value(
                            &mut condition.value_type,
                            value_type,
                            value_type.label(),
                        );
                    }
                })
                .response
                .on_hover_text("Value type (Auto infers numbers, booleans, dates and ObjectIds)");
        } else if condition.operator.is_text() {
            ui.allocate_ui(vec2(TYPE_WIDTH, 22.0), |ui| {
                ui.toggle_value(&mut condition.ignore_case, "Ignore case")
                    .on_hover_text("Match upper and lower case alike");
            });
        } else {
            ui.allocate_space(vec2(TYPE_WIDTH, 20.0));
        }
        remove = widgets::icon_button(ui, Icon::Close, "Remove condition").clicked();
    });
    if let Some(error) = condition.error() {
        // A fresh row only needs a gentle nudge; real mistakes are errors.
        let color = if condition.field.trim().is_empty() {
            p.text_info
        } else {
            p.error
        };
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(error).size(11.5).color(color));
        });
    }
    remove
}

/// A single-line value field. Returns true when Enter was pressed in it.
fn value_edit(ui: &mut Ui, text: &mut String, hint: &str, width: f32, enabled: bool) -> bool {
    let edit = egui::TextEdit::singleline(text)
        .hint_text(widgets::hint(ui, hint))
        .font(FontId::monospace(12.5))
        .desired_width(width);
    let response = ui.add_enabled(enabled, edit);
    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// An editable field path with a picker of the fields seen in the results.
fn field_picker(
    ui: &mut Ui,
    p: &Palette,
    field: &mut String,
    fields: &[String],
    id: Id,
    width: f32,
) {
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.add(
        egui::TextEdit::singleline(field)
            .id(id.with("text"))
            .hint_text(widgets::hint(ui, "field"))
            .font(FontId::monospace(12.5))
            .desired_width(width),
    );
    let button = widgets::icon_button(ui, Icon::ChevronDown, "Pick a field");
    ui.spacing_mut().item_spacing.x = 6.0;
    egui::Popup::menu(&button).id(id.with("popup")).show(|ui| {
        ui.set_min_width(200.0);
        if fields.is_empty() {
            ui.label(widgets::hint(ui, "Run the query to list the fields"));
            return;
        }
        let needle = field.trim().to_lowercase();
        let filtered = !needle.is_empty() && !fields.iter().any(|f| f == field);
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for name in fields {
                    if filtered && !name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    let text = RichText::new(name).monospace().color(p.text);
                    if ui.selectable_label(name == field, text).clicked() {
                        *field = name.clone();
                    }
                }
            });
    });
}

/// A collapsible section header such as "Sort (2)".
fn section_toggle(ui: &mut Ui, p: &Palette, open: &mut bool, label: &str, count: usize) {
    let text = if count > 0 {
        format!("{label} ({count})")
    } else {
        label.to_owned()
    };
    let color = if count > 0 {
        p.accent_text
    } else {
        p.text_secondary
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text, FontId::proportional(12.5), color);
    let (rect, response) =
        ui.allocate_exact_size(vec2(galley.size().x + 26.0, 24.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    icons::paint(
        ui.painter(),
        Rect::from_center_size(
            egui::pos2(rect.left() + 11.0, rect.center().y),
            Vec2::splat(12.0),
        ),
        if *open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        color,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    if response.clicked() {
        *open = !*open;
    }
}

fn section_label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.allocate_ui_with_layout(
        vec2(72.0, 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(RichText::new(text).color(p.text_secondary));
        },
    );
}

fn sort_section(ui: &mut Ui, p: &Palette, sort: &mut Vec<SortKey>, fields: &[String], id: Id) {
    ui.horizontal_wrapped(|ui| {
        section_label(ui, p, "Sort by");
        let mut remove = None;
        for (index, key) in sort.iter_mut().enumerate() {
            let key_id = id.with(index);
            field_picker(ui, p, &mut key.field, fields, key_id.with("field"), 120.0);
            egui::ComboBox::from_id_salt(key_id.with("direction"))
                .selected_text(if key.descending {
                    "Descending"
                } else {
                    "Ascending"
                })
                .width(96.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut key.descending, false, "Ascending");
                    ui.selectable_value(&mut key.descending, true, "Descending");
                });
            if widgets::icon_button(ui, Icon::Close, "Remove sort field").clicked() {
                remove = Some(index);
            }
            ui.add_space(6.0);
        }
        if let Some(index) = remove {
            sort.remove(index);
        }
        if widgets::icon_button(ui, Icon::Plus, "Add sort field").clicked() {
            sort.push(SortKey::default());
        }
        if sort.is_empty() {
            ui.label(widgets::hint(ui, "natural order"));
        }
    });
}

fn projection_section(ui: &mut Ui, p: &Palette, model: &mut Builder, fields: &[String], id: Id) {
    let projection = &mut model.projection;
    ui.horizontal_wrapped(|ui| {
        section_label(ui, p, "Fields");
        egui::ComboBox::from_id_salt(id.with("mode"))
            .selected_text(match projection.mode {
                ProjectionMode::Include => "Only",
                ProjectionMode::Exclude => "All except",
            })
            .width(84.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut projection.mode, ProjectionMode::Include, "Only")
                    .on_hover_text("Return only the listed fields");
                ui.selectable_value(&mut projection.mode, ProjectionMode::Exclude, "All except")
                    .on_hover_text("Return every field except the listed ones");
            });
        let mut remove = None;
        for (index, field) in projection.fields.iter_mut().enumerate() {
            field_picker(ui, p, field, fields, id.with(index), 120.0);
            if widgets::icon_button(ui, Icon::Close, "Remove field").clicked() {
                remove = Some(index);
            }
            ui.add_space(6.0);
        }
        if let Some(index) = remove {
            projection.fields.remove(index);
        }
        if widgets::icon_button(ui, Icon::Plus, "Add field").clicked() {
            projection.fields.push(String::new());
        }
        ui.add_space(8.0);
        ui.checkbox(&mut projection.exclude_id, "Hide _id");
    });
}

fn paging_section(ui: &mut Ui, p: &Palette, model: &mut Builder) {
    ui.horizontal(|ui| {
        section_label(ui, p, "Skip");
        ui.add(egui::DragValue::new(&mut model.skip).speed(1.0));
        ui.add_space(12.0);
        ui.label(RichText::new("Limit").color(p.text_secondary));
        ui.add(egui::DragValue::new(&mut model.limit).speed(1.0));
        ui.label(widgets::hint(ui, "0 = no limit"));
    });
}

/// The generated query in shell syntax, so users learn the code.
fn preview(ui: &mut Ui, p: &Palette, query: &QueryText) {
    let text = preview_text(query);
    ui.add(
        egui::Label::new(
            RichText::new(&text)
                .font(FontId::monospace(11.5))
                .color(p.text_info),
        )
        .truncate(),
    )
    .on_hover_text(text);
}

fn preview_text(query: &QueryText) -> String {
    let or_empty = |s: &str| {
        if s.is_empty() {
            "{}".to_owned()
        } else {
            s.to_owned()
        }
    };
    let mut out = format!("find({}", or_empty(&query.filter));
    if !query.projection.is_empty() {
        out.push_str(", ");
        out.push_str(&query.projection);
    }
    out.push(')');
    if !query.sort.is_empty() {
        out.push_str(&format!(".sort({})", query.sort));
    }
    if !query.skip.is_empty() {
        out.push_str(&format!(".skip({})", query.skip));
    }
    if !query.limit.is_empty() {
        out.push_str(&format!(".limit({})", query.limit));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongo_raabit_core::bson::doc;

    fn render(builder: &mut QueryBuilder, fields: &[String], dark: bool) -> Vec<Event> {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut events = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let p = Palette::of(ui);
                    events.push(show(ui, &p, builder, fields));
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
    fn collects_dotted_field_names() {
        let docs = [
            doc! {"name": "Ada", "_id": 1, "address": {"city": "x", "geo": {"lat": 1}}},
            doc! {"items": [{"sku": "a"}, 3, {"qty": 1}], "name": "B"},
        ];
        assert_eq!(
            field_names(&docs),
            [
                "_id",
                "address",
                "address.city",
                "address.geo",
                "address.geo.lat",
                "items",
                "items.qty",
                "items.sku",
                "name"
            ]
        );
        let wide = Document::from_iter((0..500).map(|i| (format!("f{i:03}"), Bson::Int32(i))));
        assert_eq!(field_names(&[wide]).len(), MAX_FIELDS);
        assert!(field_names(&[]).is_empty());
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
        assert_eq!(
            preview_text(&QueryText::default()),
            "find({})",
            "blank builder previews as find all"
        );
        assert!(preview_text(&text).starts_with("find({ name: /^Ad/i"));
        assert!(preview_text(&text).ends_with(".sort({ age: -1 }).skip(5).limit(10)"));
    }

    #[test]
    fn ctrl_enter_runs() {
        let ctx = egui::Context::default();
        let mut builder = QueryBuilder::default();
        let mut events = Vec::new();
        for frame in 0..2 {
            let mut input = egui::RawInput::default();
            if frame == 1 {
                input.modifiers = egui::Modifiers::COMMAND;
                input.events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                });
            }
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let p = Palette::of(ui);
                    events.push(show(ui, &p, &mut builder, &[]));
                });
            });
        }
        assert_eq!(events, [Event::None, Event::Run]);
    }
}
