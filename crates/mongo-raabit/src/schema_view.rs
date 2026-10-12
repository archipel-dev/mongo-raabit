//! Schema Explorer: sampled field statistics as a tree table, with the distribution of the
//! selected field beside it.
//!
//! What the numbers say (summaries, shares, buckets) is computed in
//! `mongo_raabit_core::schema`; this module lays it out.

use crate::app::{Load, MongoRaabit, TabId};
use crate::egui::{Id, Ui};
use crate::icons::Icon;
use crate::tools::SchemaTab;
use crate::widgets;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::{
    Banner, Bucket, Button, Cell, Column, FieldLabel, Histogram, NumberField, ProgressBar,
    PropertyGrid, SectionHeader, SegmentedControl, Severity, SplitPane, Tag, TreeRow, TreeTable,
};
use archipel_sdk::kit::parts::scroll_vertical;
use archipel_sdk::kit::tokens::size;
use mongo_raabit_core::schema::{
    FieldStats, MAX_SAMPLE_SIZE, SampleMethod, SchemaReport, format_number,
};
use std::cell::Cell as StdCell;
use std::collections::HashSet;

/// Default share of the width the field table gets.
const TABLE_RATIO: f32 = 0.62;
const TABLE_MIN_WIDTH: f32 = 320.0;
const DETAIL_MIN_WIDTH: f32 = 260.0;
const SAMPLE_WIDTH: f32 = 80.0;

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let server = app
        .schema_tab_mut(tab_id)
        .map(|t| t.session)
        .and_then(|s| app.session(s))
        .map(|s| s.profile.name.clone())
        .unwrap_or_default();
    let Some(tab) = app.schema_tab_mut(tab_id) else {
        return;
    };
    let SchemaTab {
        db,
        collection,
        size: sample,
        method,
        report,
        selected,
        ..
    } = tab;
    let mut analyze = false;
    widgets::tool_header(
        ui,
        Icon::Schema,
        Tone::Accent,
        &[server.as_str(), db.as_str()],
        collection,
        Tag::new("Schema").tone(Tone::Accent),
        |_| {},
        |ui| {
            analyze = ui
                .add(Button::primary("Analyze").icon(Icon::Refresh))
                .clicked();
            ui.add(SegmentedControl::new(
                method,
                [
                    (SampleMethod::Last, "Last"),
                    (SampleMethod::First, "First"),
                    (SampleMethod::Random, "Random"),
                ],
            ));
            // Allocated at its own width: a field in a right-to-left row would take all
            // the room that is left.
            ui.allocate_ui(
                crate::egui::vec2(SAMPLE_WIDTH, size::CONTROL_HEIGHT),
                |ui| {
                    ui.add(
                        NumberField::new(sample)
                            .id(ui.id().with(("raabit-schema-sample", tab_id)))
                            .name("Sample size")
                            .range(1..=MAX_SAMPLE_SIZE)
                            .step(100)
                            .width(SAMPLE_WIDTH),
                    )
                },
            );
            ui.add(FieldLabel::new("Sample"));
        },
    );
    match &*report {
        Load::Idle => {}
        Load::Loading => widgets::busy(ui, "Sampling documents…"),
        Load::Failed(err) => {
            widgets::padded(ui, |ui| {
                Banner::new(Severity::Error, err).show(ui);
            });
        }
        Load::Ready(report) => result(ui, tab_id, report, selected),
    }
    if analyze {
        app.analyze_schema(tab_id);
    }
}

/// The field table and the details of the selected field.
fn result(ui: &mut Ui, tab_id: TabId, report: &SchemaReport, selected: &mut Option<usize>) {
    let ratio_id = ui.id().with(("raabit-schema-ratio", tab_id));
    let mut ratio = ui
        .data(|d| d.get_temp::<f32>(ratio_id))
        .unwrap_or(TABLE_RATIO);
    let picked = StdCell::new(None);
    let current = *selected;
    SplitPane::horizontal(("raabit-schema-split", tab_id), &mut ratio)
        .default_ratio(TABLE_RATIO)
        .min_sizes(TABLE_MIN_WIDTH, DETAIL_MIN_WIDTH)
        .label("Resize field details")
        .show(
            ui,
            |ui| {
                widgets::padded(ui, |ui| {
                    ui.horizontal(|ui| {
                        widgets::label(
                            ui,
                            Tone::Secondary,
                            format!("{} documents sampled", report.sampled),
                        );
                        widgets::label(ui, Tone::Muted, "·");
                        widgets::label(
                            ui,
                            Tone::Secondary,
                            format!("{} fields", report.fields.len()),
                        );
                    });
                    picked.set(field_table(ui, tab_id, report, current));
                });
            },
            |ui| {
                widgets::padded(ui, |ui| {
                    scroll_vertical()
                        .id_salt(("raabit-schema-detail", tab_id))
                        .auto_shrink([false, false])
                        .show(ui, |ui| match current.and_then(|i| report.fields.get(i)) {
                            Some(field) => details(ui, field),
                            None => {
                                widgets::label(
                                    ui,
                                    Tone::Muted,
                                    "Select a field to see its distribution.",
                                );
                            }
                        });
                });
            },
        );
    ui.data_mut(|d| d.insert_temp(ratio_id, ratio));
    if let Some(index) = picked.get() {
        *selected = Some(index);
        ui.ctx().request_repaint();
    }
}

/// Column widths and the collapsed fields of one table, kept between frames.
#[derive(Clone, Default)]
struct TableMemory {
    columns: Vec<Column>,
    collapsed: HashSet<String>,
}

/// The tree table of fields. Returns the field the user selected.
fn field_table(
    ui: &mut Ui,
    tab_id: TabId,
    report: &SchemaReport,
    selected: Option<usize>,
) -> Option<usize> {
    let mem_id = Id::new(("raabit-schema-table", tab_id, "memory"));
    let mut mem: TableMemory = ui.data(|d| d.get_temp(mem_id)).unwrap_or_default();
    if mem.columns.is_empty() {
        let width = (ui.available_width() - size::CONTENT_INSET * 2.0).max(TABLE_MIN_WIDTH);
        mem.columns = vec![
            Column::new("Field", (width * 0.34).round()).mono(),
            Column::new("Present", (width * 0.14).round()).right(),
            Column::new("Types", (width * 0.20).round()),
            Column::new("Values", (width * 0.32).round()),
        ];
    }
    let rows = report.visible_fields(&mem.collapsed);
    let tree_rows: Vec<TreeRow> = rows
        .iter()
        .map(|row| {
            let field = &report.fields[row.index];
            let tree_row = TreeRow::new(field.name(), field.depth);
            if row.parent {
                tree_row.parent(!mem.collapsed.contains(&field.path))
            } else {
                tree_row
            }
        })
        .collect();
    let mut position = selected.and_then(|i| rows.iter().position(|r| r.index == i));
    let before = position;
    let height = (ui.available_height() - size::ROW_HEIGHT).max(size::ROW_HEIGHT * 4.0);
    let out = TreeTable::new(
        ("raabit-schema-table", tab_id),
        &mut mem.columns,
        &tree_rows,
        &mut position,
    )
    .name("Fields")
    .height(height)
    .show(ui, |row, column| {
        let field = &report.fields[rows[row].index];
        match column {
            1 => Cell::new(field.presence_label()).muted(),
            2 => Cell::new(field.type_label()).muted(),
            _ => Cell::new(field.summary()).muted(),
        }
    });
    if let Some(row) = out.toggled {
        let path = report.fields[rows[row].index].path.clone();
        if !mem.collapsed.remove(&path) {
            mem.collapsed.insert(path);
        }
    }
    let picked = (position != before)
        .then_some(position)
        .flatten()
        .and_then(|p| rows.get(p))
        .map(|row| row.index);
    ui.data_mut(|d| d.insert_temp(mem_id, mem));
    picked
}

/// Type shares, presence, the value distribution and the most frequent values of a field.
fn details(ui: &mut Ui, field: &FieldStats) {
    widgets::title(ui, &field.path);
    widgets::label(ui, Tone::Secondary, field.details_line());
    ui.add(
        ProgressBar::new(Some(field.probability() as f32)).label(format!(
            "Presence of {}: {}",
            field.path,
            field.presence_label()
        )),
    );

    ui.add(SectionHeader::new("Types"));
    let types: Vec<Bucket> = field
        .type_shares()
        .into_iter()
        .map(|(name, count, _)| Bucket::new(name, count))
        .collect();
    Histogram::new(&format!("types of {}", field.path), &types)
        .horizontal(true)
        .unit("values")
        .show(ui);

    if let (Some(min), Some(max), Some(mean)) = (field.min, field.max, field.mean) {
        ui.add(SectionHeader::new("Distribution"));
        PropertyGrid::new(("raabit-schema-stats", &field.path))
            .row("Min", format_number(min))
            .row("Mean", format_number(mean))
            .row("Max", format_number(max))
            .show(ui);
        let buckets: Vec<Bucket> = field
            .histogram_ranges()
            .iter()
            .map(|b| Bucket::range(b.low, b.high, b.count))
            .collect();
        Histogram::new(&field.path, &buckets).show(ui);
    }
    if let Some((first, last)) = field.date_range() {
        ui.add(SectionHeader::new("Date range"));
        PropertyGrid::new(("raabit-schema-dates", &field.path))
            .mono_row("From", first)
            .mono_row("To", last)
            .show(ui);
    }
    if let Some((what, min, max)) = field.length_range() {
        ui.add(SectionHeader::new(what));
        PropertyGrid::new(("raabit-schema-length", &field.path))
            .row("Smallest", min.to_string())
            .row("Largest", max.to_string())
            .show(ui);
    }
    if !field.top_values.is_empty() {
        ui.add(SectionHeader::new("Top values"));
        widgets::label(ui, Tone::Muted, field.distinct_label());
        let values: Vec<Bucket> = field
            .top_values
            .iter()
            .map(|(value, count)| Bucket::new(value.as_str(), *count))
            .collect();
        Histogram::new(&format!("top values of {}", field.path), &values)
            .horizontal(true)
            .unit("occurrences")
            .show(ui);
    }
}
