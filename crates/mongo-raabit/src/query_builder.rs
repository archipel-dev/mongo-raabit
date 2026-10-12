//! The Query tool window: the visual query builder of the active collection tab, in the
//! right island.
//!
//! The fields of the collection (from its schema sample) are listed with their type; a
//! field is dragged onto **Filter** (a condition with an operator for its type),
//! **Projection** (include or exclude it) or **Sort** (ascending or descending, in the
//! order of the rows, which are dragged to reorder). Double-click or Enter on a field adds
//! it to the filter; its context menu adds it anywhere. **Text** shows the same query as
//! shell text. The model and its text live in `mongo_raabit_core::builder`; this module
//! draws it and reports what changed.

use crate::app::QueryMode;
use crate::egui::{Id, Key, Modifiers, Ui};
use crate::icons::Icon;
use crate::widgets;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::behaviour::{Behaviour, ContextMenu, DragSource, DropTarget};
use archipel_sdk::kit::components::{
    Banner, Button, Checkbox, DropZone, FieldLabel, IconButton, ListRow, MenuItem, NumberField,
    SectionHeader, SegmentedControl, Severity, Spinner, Tag, TextField, Tooltip, TooltipExt,
};
use archipel_sdk::kit::parts::{Slot, Surface, scroll_vertical};
use archipel_sdk::kit::tokens::{size, typography};
use mongo_raabit_core::builder::{
    Builder, Condition, FieldType, Group, Item, MatchMode, Operands, Operator, ProjectionMode,
    TypeName, ValueType,
};
use mongo_raabit_core::fields::QueryField;
use mongo_raabit_core::query::QueryText;

pub use mongo_raabit_core::fields::field_names;

/// Shown when a text query cannot be shown by the builder.
pub const UNSUPPORTED_NOTE: &str = "The builder cannot show this query, so it stays in text mode. Edit it here, or Reset to start over in the builder.";

pub const FILTER_HINT: &str = "Drop fields here to filter";
pub const PROJECTION_HINT: &str = "Drop fields here to choose what is returned";
pub const SORT_HINT: &str = "Drop fields here to sort";

const OPERATOR_WIDTH: f32 = 120.0;
const TYPE_WIDTH: f32 = 110.0;
const MODE_WIDTH: f32 = 80.0;
const NUMBER_WIDTH: f32 = 80.0;
/// Height of the field list before it scrolls.
const FIELDS_HEIGHT: f32 = size::ROW_HEIGHT * 7.0;
const MODE_TIP: &str = "All: every condition must match (AND). Any: at least one (OR).";
const PROJECTION_TIP: &str = "MongoDB returns either only the included fields or all but the excluded ones, so the fields switch together; _id is shown unless you hide it.";

/// What a field row carries while it is dragged.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDrag {
    pub path: String,
    pub kind: FieldType,
}

/// A sort row being dragged to a new position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortDrag(pub usize);

/// What happened in the Query tool window this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    None,
    /// The builder's query changed; it runs on Run.
    Changed,
    /// Run the query on the tab.
    Run,
}

/// Which sections of the builder are open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sections {
    pub fields: bool,
    pub filter: bool,
    pub projection: bool,
    pub sort: bool,
    pub paging: bool,
}

impl Default for Sections {
    fn default() -> Self {
        Self {
            fields: true,
            filter: true,
            projection: true,
            sort: true,
            paging: true,
        }
    }
}

/// Builder state of one collection tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryBuilder {
    pub model: Builder,
    /// Set when the text query could not be taken over by the builder.
    pub note: Option<String>,
    /// What the builder could not show, for the note.
    pub note_detail: String,
    pub open: Sections,
    /// The text that filters the field list.
    pub field_search: String,
    /// The selected row of the field list (its path).
    pub selected_field: Option<String>,
    /// `_id` was dropped on Projection: its row stays while the user decides.
    pub id_row: bool,
}

impl QueryBuilder {
    /// The query as shell text, as the text mode shows it.
    pub fn to_query_text(&self) -> QueryText {
        self.model.to_query_text()
    }

    /// Takes over a query written as text when the builder can represent it; otherwise
    /// keeps the builder as it was and sets the note.
    pub fn take_over(&mut self, query: &QueryText) {
        if *query == self.to_query_text() {
            self.note = None;
            return;
        }
        match Builder::from_query_text(query) {
            Ok(model) => {
                self.id_row = model.projection.exclude_id;
                self.model = model;
                self.note = None;
            }
            Err(detail) => {
                self.note = Some(UNSUPPORTED_NOTE.into());
                self.note_detail = format!("Not representable: {detail}.");
            }
        }
    }

    /// A field dropped on (or sent to) a zone.
    pub fn apply(&mut self, action: &FieldAction) {
        match action {
            FieldAction::Filter(path, kind) => {
                self.model.add_filter_field(path, *kind);
                self.open.filter = true;
            }
            FieldAction::Projection(path) => {
                if path == "_id" {
                    self.id_row = true;
                }
                self.model.add_projection_field(path);
                self.open.projection = true;
            }
            FieldAction::Sort(path) => {
                self.model.add_sort_field(path);
                self.open.sort = true;
            }
        }
    }

    /// Clears the query, keeping which sections are open.
    pub fn reset(&mut self) {
        *self = Self {
            open: self.open,
            field_search: std::mem::take(&mut self.field_search),
            ..Self::default()
        };
    }
}

/// A field sent to one of the zones, by a drop, a double-click, Enter or the menu.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldAction {
    Filter(String, FieldType),
    Projection(String),
    Sort(String),
}

/// The fields the builder lists and where they come from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldList {
    pub fields: Vec<QueryField>,
    /// "1,000 sampled documents", "this page".
    pub source: String,
    pub loading: bool,
}

/// Draws the Query tool window for one tab. In Builder mode a change of the builder is
/// reported as [`Event::Changed`]; the caller then takes its text. In Code mode `query` is
/// edited in place.
pub fn show(
    ui: &mut Ui,
    builder: &mut QueryBuilder,
    mode: &mut QueryMode,
    query: &mut QueryText,
    fields: &FieldList,
) -> Event {
    let before = builder.to_query_text();
    let id = Id::new("raabit-query-tool");
    let mut run = false;
    let mut reset = false;
    inset(ui, |ui| {
        ui.add_space(size::SECTION_SPACING);
        ui.horizontal(|ui| {
            let previous = *mode;
            ui.add(SegmentedControl::new(
                mode,
                [(QueryMode::Builder, "Visual"), (QueryMode::Code, "Text")],
            ));
            if previous == QueryMode::Code && *mode == QueryMode::Builder {
                builder.take_over(query);
                if builder.note.is_some() {
                    // What the builder cannot show stays in text mode, with the note.
                    *mode = QueryMode::Code;
                }
            } else if previous == QueryMode::Builder && *mode == QueryMode::Code {
                *query = builder.to_query_text();
            }
            ui.with_layout(
                crate::egui::Layout::right_to_left(crate::egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
                    if ui
                        .add(Button::primary("Run").icon(Icon::Play))
                        .tooltip_with(Tooltip::new("Run query").shortcut("Ctrl+Enter"))
                        .clicked()
                    {
                        run = true;
                    }
                    if ui
                        .add(IconButton::new(Icon::Refresh, "Reset query"))
                        .tooltip("Clear filter, projection, sort, skip and limit, and run")
                        .clicked()
                    {
                        reset = true;
                    }
                },
            );
        });
        ui.add_space(size::SECTION_SPACING);
    });
    match *mode {
        QueryMode::Builder => run |= visual(ui, builder, fields, id),
        QueryMode::Code => run |= text(ui, builder, query),
    }
    run |= ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
    if reset {
        builder.reset();
        *query = QueryText::default();
        run = true;
    }
    let changed = *mode == QueryMode::Builder && builder.to_query_text() != before;
    if changed {
        // The text query the note was about has been replaced now.
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

/// Content with the tool window's side inset (rows bring their own).
fn inset<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    crate::egui::Frame::new()
        .inner_margin(crate::egui::Margin::symmetric(size::CONTENT_INSET as i8, 0))
        .show(ui, add)
        .inner
}

/// The builder: fields, the three drop zones, skip and limit and the generated text.
/// Returns true when Enter was pressed in a value.
fn visual(ui: &mut Ui, builder: &mut QueryBuilder, fields: &FieldList, id: Id) -> bool {
    let mut enter = false;
    let mut actions = Vec::new();
    field_section(ui, builder, fields, &mut actions);
    let names: Vec<String> = fields.fields.iter().map(|f| f.path.clone()).collect();
    enter |= filter_section(ui, builder, &names, id.with("filter"), &mut actions);
    projection_section(ui, builder, &mut actions);
    sort_section(ui, builder, &mut actions);
    paging_section(ui, builder, id.with("paging"));
    for action in &actions {
        builder.apply(action);
        // A dropped field waits for its value: type it right away.
        if let FieldAction::Filter(..) = action
            && let Some(Item::Condition(condition)) = builder.model.filter.items.last()
        {
            let name = match condition.operator.operands() {
                Operands::One => Some("Value"),
                Operands::List => Some("Values"),
                Operands::Two => Some("From"),
                Operands::None | Operands::Type => None,
            };
            if let Some(name) = name {
                let index = builder.model.filter.items.len() - 1;
                let target = id.with("filter").with("items").with(index).with(name);
                ui.data_mut(|d| d.insert_temp(Id::new(FOCUS_VALUE), target));
            }
        }
    }
    inset(ui, |ui| {
        ui.add_space(size::SECTION_SPACING);
        preview(ui, &builder.to_query_text());
        ui.add_space(size::SECTION_SPACING);
    });
    enter
}

fn section(ui: &mut Ui, title: String, open: &mut bool) {
    inset(ui, |ui| {
        ui.add(SectionHeader::new(title).collapsible(open));
    });
}

/// A section title such as "Sort (2)".
fn titled(label: &str, count: usize) -> String {
    if count > 0 {
        format!("{label} ({count})")
    } else {
        label.to_owned()
    }
}

/// The field list: draggable rows with a type badge, filtered by the search text.
fn field_section(
    ui: &mut Ui,
    builder: &mut QueryBuilder,
    fields: &FieldList,
    actions: &mut Vec<FieldAction>,
) {
    section(
        ui,
        titled("Fields", fields.fields.len()),
        &mut builder.open.fields,
    );
    if !builder.open.fields {
        return;
    }
    inset(ui, |ui| {
        ui.horizontal(|ui| {
            if fields.loading {
                ui.add(Spinner::new().label("Sampling fields"));
            }
            widgets::note(
                ui,
                Tone::Muted,
                if fields.loading {
                    "Sampling the collection…".to_owned()
                } else if fields.fields.is_empty() {
                    "No fields yet: run a query or sample the schema.".to_owned()
                } else {
                    format!("From {}. Drag a field onto a zone below.", fields.source)
                },
            );
        });
        if fields.fields.len() > 8 {
            ui.add(
                TextField::new(&mut builder.field_search)
                    .name("Filter fields")
                    .placeholder("Filter fields")
                    .width(ui.available_width()),
            );
        }
    });
    let needle = builder.field_search.trim().to_lowercase();
    let shown: Vec<&QueryField> = fields
        .fields
        .iter()
        .filter(|f| needle.is_empty() || f.path.to_lowercase().contains(&needle))
        .collect();
    scroll_vertical()
        .id_salt("raabit-query-fields")
        .max_height(FIELDS_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for field in shown {
                field_row(ui, builder, field, actions);
            }
        });
}

fn field_row(
    ui: &mut Ui,
    builder: &mut QueryBuilder,
    field: &QueryField,
    actions: &mut Vec<FieldAction>,
) {
    let color = crate::values::field_kind(field.kind).resolve(ui.ctx());
    let label = if needs_full_path(builder, field) {
        field.path.as_str()
    } else {
        field.name()
    };
    let tip = format!(
        "{} · {} · in {:.0}% of documents",
        field.path,
        field.kind.label(),
        field.presence * 100.0
    );
    let menu_actions = std::cell::RefCell::new(Vec::new());
    let row = ListRow::new(label)
        .depth(if builder.field_search.trim().is_empty() {
            field.depth
        } else {
            0
        })
        .icon_painter(move |painter, rect, _| {
            crate::icons::paint(painter, rect, Icon::Field, color);
        })
        .trailing(Slot::text_tone(field.kind.label(), Tone::Muted))
        .selected(builder.selected_field.as_deref() == Some(field.path.as_str()))
        .with(DragSource(FieldDrag {
            path: field.path.clone(),
            kind: field.kind,
        }))
        .with(Tooltip::new(tip))
        .with(ContextMenu::new(|ui| {
            for (label, action) in [
                (
                    "Add to Filter",
                    FieldAction::Filter(field.path.clone(), field.kind),
                ),
                (
                    "Add to Projection",
                    FieldAction::Projection(field.path.clone()),
                ),
                ("Add to Sort", FieldAction::Sort(field.path.clone())),
            ] {
                if ui.add(MenuItem::new(label)).clicked() {
                    menu_actions.borrow_mut().push(action);
                }
            }
        }))
        .show(ui);
    actions.extend(menu_actions.into_inner());
    let enter = row.response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if row.response.double_clicked() || enter {
        actions.push(FieldAction::Filter(field.path.clone(), field.kind));
    }
    if row.response.clicked() {
        // Selected and focused, so Enter adds it.
        builder.selected_field = Some(field.path.clone());
        row.response.request_focus();
    }
}

/// While the list is filtered, rows show whole paths (the nesting is gone).
fn needs_full_path(builder: &QueryBuilder, field: &QueryField) -> bool {
    !builder.field_search.trim().is_empty() && field.depth > 0
}

/// Filter: the conditions, in a zone fields are dropped on. Returns true on Enter in a value.
fn filter_section(
    ui: &mut Ui,
    builder: &mut QueryBuilder,
    fields: &[String],
    id: Id,
    actions: &mut Vec<FieldAction>,
) -> bool {
    let count = builder.model.filter.items.len();
    section(ui, titled("Filter", count), &mut builder.open.filter);
    if !builder.open.filter {
        return false;
    }
    let mut enter = false;
    inset(ui, |ui| {
        let errors = builder.model.error_count();
        if count > 0 {
            ui.horizontal(|ui| {
                ui.add(FieldLabel::new("Match"));
                mode_choice(ui, id.with("mode"), &mut builder.model.filter.mode);
                ui.add(FieldLabel::new("of these"));
                if errors > 0 {
                    let text = format!("{errors} incomplete");
                    ui.add(Tag::new(text).tone(Tone::Warning))
                        .tooltip("Incomplete conditions are left out of the query");
                }
            });
            ui.add_space(size::FORM_ROW_GAP);
        }
        let model = &mut builder.model;
        let group = &mut model.filter;
        let zone = DropZone::new(id.with("zone"))
            .hint(FILTER_HINT)
            .show_hint(group.items.is_empty())
            .show::<FieldDrag, _>(ui, |ui| items(ui, group, fields, id.with("items")));
        enter |= zone.inner;
        if let Some(field) = zone.dropped {
            actions.push(FieldAction::Filter(field.path.clone(), field.kind));
        }
        ui.add_space(size::FORM_ROW_GAP);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
            if ui
                .add(Button::secondary("Condition").icon(Icon::Plus).small(true))
                .tooltip("Add an empty condition")
                .clicked()
            {
                model
                    .filter
                    .items
                    .push(Item::Condition(Condition::default()));
            }
            if ui
                .add(Button::secondary("Group").icon(Icon::Plus).small(true))
                .tooltip("Add a group of conditions with its own All / Any")
                .clicked()
            {
                let item = model.new_group();
                model.filter.items.push(item);
            }
        });
        ui.add_space(size::SECTION_SPACING);
    });
    enter
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
fn items(ui: &mut Ui, group: &mut Group, fields: &[String], id: Id) -> bool {
    let mut enter = false;
    let mut remove = None;
    for (index, item) in group.items.iter_mut().enumerate() {
        let item_id = id.with(index);
        let removed = match item {
            Item::Condition(condition) => {
                condition_card(ui, condition, fields, item_id, &mut enter)
            }
            Item::Group(inner) => group_card(ui, inner, fields, item_id, &mut enter),
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

/// A card on the field surface, as wide as the zone.
fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    let inner = Surface::Field
        .frame(&ctx)
        .inner_margin(crate::egui::Margin::same(size::SECTION_SPACING as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = size::FORM_ROW_GAP;
            add(ui)
        })
        .inner;
    ui.add_space(size::FORM_ROW_GAP);
    inner
}

/// A nested group: its own All / Any and conditions. Returns true when removed.
fn group_card(ui: &mut Ui, group: &mut Group, fields: &[String], id: Id, enter: &mut bool) -> bool {
    card(ui, |ui| {
        let mut remove = false;
        ui.horizontal(|ui| {
            ui.add(FieldLabel::new("Match"));
            mode_choice(ui, id.with("mode"), &mut group.mode);
            ui.add(FieldLabel::new("of:"));
            ui.with_layout(
                crate::egui::Layout::right_to_left(crate::egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
                    remove = ui
                        .add(IconButton::new(Icon::Trash, "Remove group"))
                        .clicked();
                    if ui
                        .add(IconButton::new(Icon::Plus, "Add a condition to this group"))
                        .clicked()
                    {
                        group.items.push(Item::Condition(Condition::default()));
                    }
                },
            );
        });
        if group.items.is_empty() {
            widgets::note(ui, Tone::Muted, "Empty group: it is left out of the query.");
        }
        *enter |= items(ui, group, fields, id.with("items"));
        remove
    })
}

/// One condition: field and remove, then operator and value, then the value's type (or
/// "ignore case"). Returns true when removed.
fn condition_card(
    ui: &mut Ui,
    condition: &mut Condition,
    fields: &[String],
    id: Id,
    enter: &mut bool,
) -> bool {
    let value_before = (condition.value.clone(), condition.value2.clone());
    let removed = card(ui, |ui| {
        let mut remove = false;
        let spacing = ui.spacing().item_spacing.x;
        ui.horizontal(|ui| {
            let width = ui.available_width() - size::ICON_BUTTON - spacing;
            TextField::new(&mut condition.field)
                .id(id.with("field"))
                .name("Field")
                .placeholder("field")
                .monospace(true)
                .width(width)
                .suggestions(fields)
                .show(ui);
            remove = ui
                .add(IconButton::new(Icon::Close, "Remove condition"))
                .clicked();
        });
        let null = condition.value_type == ValueType::Null && condition.operator.uses_value_type();
        ui.horizontal(|ui| {
            widgets::choice(
                ui,
                id.with("operator"),
                "Operator",
                &mut condition.operator,
                Operator::ALL.iter().map(|o| (*o, o.label())),
                OPERATOR_WIDTH,
            );
            let width = ui.available_width();
            match condition.operator.operands() {
                Operands::None => {}
                Operands::One => {
                    let hint = match condition.operator {
                        Operator::Contains | Operator::StartsWith | Operator::EndsWith => "text",
                        Operator::SizeIs => "number of elements",
                        _ => "value",
                    };
                    *enter |= value_edit(
                        ui,
                        id.with("Value"),
                        "Value",
                        &mut condition.value,
                        hint,
                        width,
                        !null,
                    );
                }
                Operands::List => {
                    *enter |= value_edit(
                        ui,
                        id.with("Values"),
                        "Values",
                        &mut condition.value,
                        "a, b, c",
                        width,
                        !null,
                    );
                }
                Operands::Two => {
                    *enter |= value_edit(
                        ui,
                        id.with("From"),
                        "From",
                        &mut condition.value,
                        "from",
                        width,
                        !null,
                    );
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
        });
        if condition.operator.operands() == Operands::Two {
            ui.horizontal(|ui| {
                let label = ui.add(FieldLabel::new("and"));
                let width = OPERATOR_WIDTH - label.rect.width() - spacing;
                ui.add_space(width.max(0.0));
                let width = ui.available_width();
                *enter |= value_edit(
                    ui,
                    id.with("To"),
                    "To",
                    &mut condition.value2,
                    "to",
                    width,
                    !null,
                );
            });
        }
        if condition.operator.uses_value_type() {
            ui.horizontal(|ui| {
                widgets::choice(
                    ui,
                    id.with("value-type"),
                    "Value type",
                    &mut condition.value_type,
                    ValueType::ALL.iter().map(|t| (*t, t.label())),
                    TYPE_WIDTH,
                )
                .tooltip("Value type (Auto infers numbers, booleans, dates and ObjectIds)");
            });
        } else if condition.operator.is_text() {
            ui.add(Checkbox::new(&mut condition.ignore_case, "Ignore case"))
                .tooltip("Match upper and lower case alike");
        }
        if let Some(error) = condition.error() {
            // A fresh row only needs a gentle nudge; real mistakes are errors.
            let gentle = condition.field.trim().is_empty() || condition.awaiting_value;
            widgets::note(ui, if gentle { Tone::Muted } else { Tone::Error }, error);
        }
        remove
    });
    if (condition.value.as_str(), condition.value2.as_str())
        != (value_before.0.as_str(), value_before.1.as_str())
    {
        // Typed into: an empty value now means the empty string.
        condition.awaiting_value = false;
    }
    removed
}

/// A single-line value field. Returns true when Enter was pressed in it.
/// The id keeps the focus while the card moves (rows above it appear or go).
fn value_edit(
    ui: &mut Ui,
    id: Id,
    name: &str,
    text: &mut String,
    hint: &str,
    width: f32,
    enabled: bool,
) -> bool {
    let out = TextField::new(text)
        .id(id)
        .name(name)
        .placeholder(hint)
        .monospace(true)
        .enabled(enabled)
        .width(width)
        .show(ui);
    let pending = Id::new(FOCUS_VALUE);
    if ui.data(|d| d.get_temp::<Id>(pending)) == Some(id) {
        out.response.request_focus();
        ui.data_mut(|d| d.remove::<Id>(pending));
    }
    out.submitted
}

/// Where the value field a dropped field's condition waits for is kept until it is drawn.
const FOCUS_VALUE: &str = "raabit-query-focus-value";

/// Projection: the fields returned (or left out), in a zone fields are dropped on.
fn projection_section(ui: &mut Ui, builder: &mut QueryBuilder, actions: &mut Vec<FieldAction>) {
    let projection = &builder.model.projection;
    let id_row = builder.id_row || projection.exclude_id;
    let count = projection.fields.len() + usize::from(id_row);
    section(
        ui,
        titled("Projection", count),
        &mut builder.open.projection,
    );
    if !builder.open.projection {
        return;
    }
    let QueryBuilder {
        model,
        id_row: keep_id_row,
        ..
    } = builder;
    let projection = &mut model.projection;
    inset(ui, |ui| {
        let mut remove = None;
        let mut remove_id = false;
        let mut include = projection.mode == ProjectionMode::Include;
        let mut show_id = !projection.exclude_id;
        let zone = DropZone::new("raabit-projection-zone")
            .hint(PROJECTION_HINT)
            .show_hint(count == 0)
            .show::<FieldDrag, _>(ui, |ui| {
                for (index, field) in projection.fields.iter().enumerate() {
                    if projection_row(ui, field, &mut include) {
                        remove = Some(index);
                    }
                }
                if id_row && projection_row(ui, "_id", &mut show_id) {
                    remove_id = true;
                }
            });
        if let Some(field) = zone.dropped {
            actions.push(FieldAction::Projection(field.path.clone()));
        }
        projection.mode = if include {
            ProjectionMode::Include
        } else {
            ProjectionMode::Exclude
        };
        projection.exclude_id = !show_id;
        if let Some(index) = remove {
            projection.fields.remove(index);
        }
        if remove_id {
            projection.exclude_id = false;
            *keep_id_row = false;
        }
        if projection.fields.len() > 1 {
            widgets::note(ui, Tone::Muted, PROJECTION_TIP);
        }
        ui.add_space(size::SECTION_SPACING);
    });
}

/// A projected field: include or exclude it, or remove it. Returns true when removed.
fn projection_row(ui: &mut Ui, field: &str, include: &mut bool) -> bool {
    let mut remove = false;
    let trailing = 2.0 * (size::ICON + 2.0 * size::SECTION_SPACING)
        + size::ICON_BUTTON_SMALL
        + 2.0 * size::TOOLBAR_GAP;
    let icon = if *include {
        Icon::Include
    } else {
        Icon::Exclude
    };
    ListRow::new(field)
        .icon(icon)
        .icon_tone(if *include { Tone::Default } else { Tone::Muted })
        .label_tone(if *include { Tone::Default } else { Tone::Muted })
        .trailing(Slot::ui_width(trailing, |ui| {
            ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
            remove = ui
                .add(IconButton::new(Icon::Close, format!("Remove {field}")).small(true))
                .clicked();
            ui.add(
                SegmentedControl::new(include, [(true, "Include"), (false, "Exclude")])
                    .icon(true, Icon::Include)
                    .icon(false, Icon::Exclude)
                    .icon_only(true),
            );
        }))
        .show(ui);
    remove
}

/// Sort: the sort keys in order, in a zone fields are dropped on; rows drag to reorder.
fn sort_section(ui: &mut Ui, builder: &mut QueryBuilder, actions: &mut Vec<FieldAction>) {
    let count = builder.model.sort.len();
    section(ui, titled("Sort", count), &mut builder.open.sort);
    if !builder.open.sort {
        return;
    }
    let model = &mut builder.model;
    inset(ui, |ui| {
        let sort = &mut model.sort;
        let mut remove = None;
        let mut moved: Option<(usize, usize)> = None;
        let len = sort.len();
        let zone = DropZone::new("raabit-sort-zone")
            .hint(SORT_HINT)
            .show_hint(sort.is_empty())
            .show::<FieldDrag, _>(ui, |ui| {
                for (index, key) in sort.iter_mut().enumerate() {
                    let mut dropped: Option<SortDrag> = None;
                    let trailing = 2.0 * (size::ICON + 2.0 * size::SECTION_SPACING)
                        + 3.0 * size::ICON_BUTTON_SMALL
                        + 4.0 * size::TOOLBAR_GAP;
                    let label = key.field.clone();
                    ListRow::new(&label)
                        .icon(Icon::Sort)
                        .with(DragSource(SortDrag(index)))
                        .with(reorder_target(&mut dropped))
                        .with(Tooltip::new(format!(
                            "Sort key {} of {len}: drag to reorder",
                            index + 1
                        )))
                        .trailing(Slot::ui_width(trailing, |ui| {
                            ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
                            if ui
                                .add(
                                    IconButton::new(Icon::Close, format!("Remove {label}"))
                                        .small(true),
                                )
                                .clicked()
                            {
                                remove = Some(index);
                            }
                            if ui
                                .add(
                                    IconButton::new(
                                        Icon::ChevronDown,
                                        format!("Move {label} down"),
                                    )
                                    .small(true)
                                    .enabled(index + 1 < len),
                                )
                                .clicked()
                            {
                                moved = Some((index, index + 1));
                            }
                            if ui
                                .add(
                                    IconButton::new(Icon::ChevronUp, format!("Move {label} up"))
                                        .small(true)
                                        .enabled(index > 0),
                                )
                                .clicked()
                            {
                                moved = Some((index, index - 1));
                            }
                            ui.add(
                                SegmentedControl::new(
                                    &mut key.descending,
                                    [(false, "Ascending"), (true, "Descending")],
                                )
                                .icon(false, Icon::Ascending)
                                .icon(true, Icon::Descending)
                                .icon_only(true),
                            );
                        }))
                        .show(ui);
                    if let Some(SortDrag(from)) = dropped {
                        moved = Some((from, index));
                    }
                }
            });
        if let Some(field) = zone.dropped {
            actions.push(FieldAction::Sort(field.path.clone()));
        }
        if let Some((from, to)) = moved {
            model.move_sort(from, to);
        } else if let Some(index) = remove {
            model.sort.remove(index);
        }
        if model.sort.is_empty() {
            widgets::note(ui, Tone::Muted, "No sort: natural order.");
        }
        ui.add_space(size::SECTION_SPACING);
    });
}

/// A sort row as the target of another sort row. The kit's `DropTarget` takes whatever is
/// released over it, whatever its type, so a field dropped on a row of the Sort zone would
/// vanish; it only listens while a sort row is dragged. (Fixed in the kit on
/// `claude/raabit-island-needs`; this guard can go once that is released.)
fn reorder_target<'a>(
    dropped: &'a mut Option<SortDrag>,
) -> impl FnOnce(&mut Ui, crate::egui::Response) -> crate::egui::Response + 'a {
    move |ui, response| {
        if crate::egui::DragAndDrop::has_payload_of_type::<SortDrag>(ui.ctx()) {
            DropTarget::new(dropped).attach(ui, response)
        } else {
            response
        }
    }
}

fn paging_section(ui: &mut Ui, builder: &mut QueryBuilder, id: Id) {
    let model = &mut builder.model;
    let held = usize::from(model.skip > 0) + usize::from(model.limit > 0);
    section(ui, titled("Skip and limit", held), &mut builder.open.paging);
    if !builder.open.paging {
        return;
    }
    inset(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(FieldLabel::new("Skip"));
            ui.add(
                NumberField::new(&mut model.skip)
                    .id(id.with("skip"))
                    .name("Skip")
                    .width(NUMBER_WIDTH),
            );
            ui.add_space(size::FORM_CONTROL_SPACE);
            ui.add(FieldLabel::new("Limit"));
            ui.add(
                NumberField::new(&mut model.limit)
                    .id(id.with("limit"))
                    .name("Limit")
                    .width(NUMBER_WIDTH),
            )
            .tooltip("0 = no limit");
        });
    });
}

/// Text mode: the same query as shell text.
fn text(ui: &mut Ui, builder: &QueryBuilder, query: &mut QueryText) -> bool {
    let mut enter = false;
    inset(ui, |ui| {
        if let Some(note) = &builder.note {
            let text = format!("{note} {}", builder.note_detail);
            Banner::new(Severity::Warning, text.trim()).show(ui);
            ui.add_space(size::SECTION_SPACING);
        }
        let width = ui.available_width();
        for (caption, text, hint) in [
            ("Filter", &mut query.filter, "{ field: \"value\" }"),
            ("Projection", &mut query.projection, "{ name: 1 }"),
            ("Sort", &mut query.sort, "{ _id: -1 }"),
        ] {
            ui.add(FieldLabel::new(caption));
            let out = TextField::new(text)
                .name(caption)
                .placeholder(hint)
                .monospace(true)
                .width(width)
                .show(ui);
            enter |= out.submitted;
            ui.add_space(size::FORM_ROW_GAP);
        }
        ui.horizontal(|ui| {
            for (caption, text) in [("Skip", &mut query.skip), ("Limit", &mut query.limit)] {
                ui.add(FieldLabel::new(caption));
                let out = TextField::new(text)
                    .name(caption)
                    .placeholder("0")
                    .monospace(true)
                    .width(NUMBER_WIDTH)
                    .show(ui);
                enter |= out.submitted;
                ui.add_space(size::FORM_CONTROL_SPACE);
            }
        });
        ui.add_space(size::SECTION_SPACING);
        widgets::note(
            ui,
            Tone::Muted,
            "Shell syntax: ObjectId(\"…\"), ISODate(\"…\"), /regex/i. Enter runs.",
        );
    });
    enter
}

/// The generated query in shell syntax, so users learn the code.
fn preview(ui: &mut Ui, query: &QueryText) {
    let text = query.preview();
    let rich = crate::egui::RichText::new(&text)
        .font(crate::egui::FontId::monospace(typography::SMALL))
        .color(Tone::Muted.resolve(ui.ctx()));
    ui.add(crate::egui::Label::new(rich).wrap()).tooltip(text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use archipel_sdk::kit::{self, Theme};
    use mongo_raabit_core::bson::doc;

    fn fields() -> FieldList {
        let docs =
            [doc! {"_id": 1, "name": "Ada", "age": 36, "address": {"city": "Oslo"}, "tags": ["a"]}];
        FieldList {
            fields: mongo_raabit_core::fields::query_fields(&mongo_raabit_core::schema::analyze(
                &docs,
            )),
            source: "this page".into(),
            loading: false,
        }
    }

    fn render(
        builder: &mut QueryBuilder,
        mode: &mut QueryMode,
        query: &mut QueryText,
        dark: bool,
    ) -> Vec<Event> {
        let ctx = crate::egui::Context::default();
        kit::install(&ctx, if dark { Theme::dark() } else { Theme::light() });
        let fields = fields();
        let mut events = Vec::new();
        for _ in 0..2 {
            let mut event = None;
            let _ = ctx.run(crate::egui::RawInput::default(), |ctx| {
                crate::egui::CentralPanel::default().show(ctx, |ui| {
                    event = Some(show(ui, builder, mode, query, &fields));
                });
            });
            events.extend(event);
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
    fn takes_over_text_queries_or_keeps_state_with_a_note() {
        let builder = sample();
        assert!(builder.note.is_none());
        assert_eq!(builder.model.filter.items.len(), 7);
        assert!(builder.id_row, "the hidden _id shows as a row");

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
            for mut mode in [QueryMode::Builder, QueryMode::Code] {
                let mut query = builder.to_query_text();
                let events = render(&mut builder, &mut mode, &mut query, dark);
                assert_eq!(events, [Event::None, Event::None]);
            }
        }
        let mut empty = QueryBuilder::default();
        let mut query = QueryText::default();
        assert_eq!(
            render(&mut empty, &mut QueryMode::Builder, &mut query, true),
            [Event::None, Event::None]
        );
        empty.note = Some(UNSUPPORTED_NOTE.into());
        render(&mut empty, &mut QueryMode::Code, &mut query, false);
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
    fn field_actions_fill_the_zones() {
        let mut builder = QueryBuilder::default();
        builder.apply(&FieldAction::Filter("active".into(), FieldType::Boolean));
        builder.apply(&FieldAction::Projection("name".into()));
        builder.apply(&FieldAction::Projection("_id".into()));
        builder.apply(&FieldAction::Sort("age".into()));
        let text = builder.to_query_text();
        assert_eq!(text.filter, "{ active: true }");
        assert_eq!(text.projection, "{ name: 1, _id: 0 }");
        assert_eq!(text.sort, "{ age: 1 }");
        assert!(builder.id_row);
        builder.reset();
        assert_eq!(builder.to_query_text(), QueryText::default());
        assert!(!builder.id_row);
    }

    #[test]
    fn ctrl_enter_runs() {
        let ctx = crate::egui::Context::default();
        let mut builder = QueryBuilder::default();
        let mut query = QueryText::default();
        let mut mode = QueryMode::Builder;
        let fields = FieldList::default();
        let mut events = Vec::new();
        for frame in 0..2 {
            let mut input = crate::egui::RawInput::default();
            if frame == 1 {
                input.modifiers = Modifiers::COMMAND;
                input.events.push(crate::egui::Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                });
            }
            let _ = ctx.run(input, |ctx| {
                crate::egui::CentralPanel::default().show(ctx, |ui| {
                    events.push(show(ui, &mut builder, &mut mode, &mut query, &fields));
                });
            });
        }
        assert_eq!(events, [Event::None, Event::Run]);
    }
}
