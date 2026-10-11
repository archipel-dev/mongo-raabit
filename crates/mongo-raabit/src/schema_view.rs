//! Schema Explorer: sampled field statistics with type, presence, histogram and
//! top-value charts.

use crate::app::{Load, MongoRaabit, TabId};
use crate::icons::{self, Icon};
use crate::style::{self, Palette};
use crate::widgets;
use egui::{Align2, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use mongo_raabit_core::schema::{FieldStats, HISTOGRAM_BUCKETS, SampleMethod};

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let p = Palette::of(ui);
    let server = app
        .schema_tab_mut(tab_id)
        .map(|t| t.session)
        .and_then(|s| app.session(s))
        .map(|s| s.profile.name.clone())
        .unwrap_or_default();
    let Some(tab) = app.schema_tab_mut(tab_id) else {
        return;
    };
    let mut analyze = false;
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(ui, Icon::Schema, p.leaf);
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
                widgets::badge(ui, "Schema", p.leaf);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, Some(Icon::Refresh), "Analyze").clicked() {
                        analyze = true;
                    }
                    widgets::segmented(
                        ui,
                        &mut tab.method,
                        &[
                            (SampleMethod::Last, Icon::ChevronDown, "Last"),
                            (SampleMethod::First, Icon::ChevronUp, "First"),
                            (SampleMethod::Random, Icon::Refresh, "Random"),
                        ],
                    );
                    let mut size = tab.size.to_string();
                    if ui
                        .add(egui::TextEdit::singleline(&mut size).desired_width(56.0))
                        .changed()
                        && let Ok(v) = size.trim().parse::<u64>()
                    {
                        tab.size = v.clamp(1, 100_000);
                    }
                    ui.label(RichText::new("Sample").color(p.text_info));
                });
            });
        });
    widgets::separator(ui);
    match &tab.report {
        Load::Idle => {}
        Load::Loading => {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label(RichText::new("Sampling documents…").color(p.text_secondary));
            });
        }
        Load::Failed(err) => {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(err).color(p.error)));
        }
        Load::Ready(report) => {
            let report = report.clone();
            let detail_width = 400.0;
            let height = ui.available_height();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - detail_width - 12.0);
                    ui.set_height(height);
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 14,
                            right: 0,
                            top: 8,
                            bottom: 6,
                        })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("{} documents sampled", report.sampled))
                                        .color(p.text_secondary),
                                );
                                ui.label(RichText::new("·").color(p.text_info));
                                ui.label(
                                    RichText::new(format!("{} fields", report.fields.len()))
                                        .color(p.text_secondary),
                                );
                            });
                        });
                    if let Some(i) = field_table(ui, &p, &report.fields, tab.selected, tab_id) {
                        tab.selected = Some(i);
                    }
                });
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, p.separator);
                ui.vertical(|ui| {
                    ui.set_width(detail_width);
                    egui::ScrollArea::vertical()
                        .id_salt(("raabit-schema-detail", tab_id))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .inner_margin(egui::Margin::same(14))
                                .show(ui, |ui| {
                                    ui.set_width(detail_width - 28.0);
                                    match tab.selected.and_then(|i| report.fields.get(i)) {
                                        Some(field) => details(ui, &p, field),
                                        None => widgets::caption(
                                            ui,
                                            "Select a field to see its distribution.",
                                        ),
                                    }
                                });
                        });
                });
            });
        }
    }
    if analyze {
        app.analyze_schema(tab_id);
    }
}

fn field_table(
    ui: &mut Ui,
    p: &Palette,
    fields: &[FieldStats],
    selected: Option<usize>,
    tab_id: TabId,
) -> Option<usize> {
    let mut clicked = None;
    let header_bg = p.raised;
    TableBuilder::new(ui)
        .id_salt(("raabit-schema-table", tab_id))
        .striped(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(230.0).at_least(140.0).clip(true))
        .column(Column::initial(150.0).at_least(100.0))
        .column(Column::initial(200.0).at_least(120.0))
        .column(Column::remainder().at_least(120.0).clip(true))
        .header(26.0, |mut header| {
            for title in ["Field", "Present", "Types", "Values"] {
                header.col(|ui| {
                    ui.painter()
                        .rect_filled(ui.max_rect().expand2(vec2(4.0, 0.0)), 0.0, header_bg);
                    ui.label(RichText::new(title).strong());
                });
            }
        })
        .body(|body| {
            body.rows(28.0, fields.len(), |mut row| {
                let i = row.index();
                let field = &fields[i];
                row.set_selected(selected == Some(i));
                row.col(|ui| {
                    ui.add_space(field.depth as f32 * 14.0);
                    let name = field.path.rsplit('.').next().unwrap_or(&field.path);
                    ui.label(
                        RichText::new(name)
                            .font(FontId::monospace(12.5))
                            .color(p.key),
                    )
                    .on_hover_text(&field.path);
                });
                row.col(|ui| probability_bar(ui, p, field.probability()));
                row.col(|ui| type_bar(ui, p, field));
                row.col(|ui| {
                    ui.label(
                        RichText::new(summary(field))
                            .size(12.0)
                            .color(p.text_secondary),
                    );
                });
                if row.response().clicked() {
                    clicked = Some(i);
                }
            });
        });
    clicked
}

fn summary(field: &FieldStats) -> String {
    if let (Some(min), Some(max)) = (field.min, field.max) {
        return format!("{} … {}", number(min), number(max));
    }
    if let (Some(a), Some(b)) = (field.min_date, field.max_date) {
        let day = |d: mongo_raabit_core::bson::DateTime| {
            d.try_to_rfc3339_string()
                .map(|s| s[..10].to_owned())
                .unwrap_or_default()
        };
        return format!("{} … {}", day(a), day(b));
    }
    if !field.top_values.is_empty() {
        let shown: Vec<&str> = field
            .top_values
            .iter()
            .take(3)
            .map(|(v, _)| v.as_str())
            .collect();
        let more = if field.distinct_overflow { "many" } else { "" };
        return format!(
            "{}{}",
            shown.join(", "),
            if field.top_values.len() > 3 || !more.is_empty() {
                ", …"
            } else {
                ""
            }
        );
    }
    if let (Some(a), Some(b)) = (field.min_length, field.max_length) {
        return format!("length {a} … {b}");
    }
    String::new()
}

fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e12 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Presence as a thin bar with the percentage in text ink.
fn probability_bar(ui: &mut Ui, p: &Palette, probability: f64) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width().min(140.0), 18.0), Sense::hover());
    let track = Rect::from_min_size(
        pos2(rect.left(), rect.center().y - 3.0),
        vec2(rect.width() - 44.0, 6.0),
    );
    ui.painter()
        .rect_filled(track, CornerRadius::same(3), p.selection_inactive);
    let filled = Rect::from_min_size(
        track.min,
        vec2(track.width() * probability as f32, track.height()),
    );
    // Neutral ink: presence is not a type, so it must not borrow a type color.
    ui.painter()
        .rect_filled(filled, CornerRadius::same(3), p.text_secondary);
    ui.painter().text(
        pos2(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        format!("{:.0}%", probability * 100.0),
        FontId::proportional(12.0),
        p.text_secondary,
    );
    response.on_hover_text(format!("present in {:.1}% of parents", probability * 100.0));
}

/// Stacked type-share bar: one segment per type, 2 px gaps, rounded outer ends.
fn type_bar(ui: &mut Ui, p: &Palette, field: &FieldStats) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width().min(190.0), 18.0), Sense::hover());
    let total: u64 = field.types.iter().map(|(_, n)| n).sum();
    if total == 0 {
        return;
    }
    let bar = Rect::from_min_size(
        pos2(rect.left(), rect.center().y - 4.0),
        vec2(rect.width() - 74.0, 8.0),
    );
    let mut x = bar.left();
    let n = field.types.len();
    for (i, (name, count)) in field.types.iter().enumerate() {
        let w = bar.width() * (*count as f32 / total as f32);
        let gap = if i + 1 < n { 2.0 } else { 0.0 };
        let seg = Rect::from_min_max(
            pos2(x, bar.top()),
            pos2((x + w - gap).max(x + 1.0), bar.bottom()),
        );
        let radius = CornerRadius {
            nw: if i == 0 { 4 } else { 0 },
            sw: if i == 0 { 4 } else { 0 },
            ne: if i + 1 == n { 4 } else { 0 },
            se: if i + 1 == n { 4 } else { 0 },
        };
        ui.painter().rect_filled(seg, radius, p.type_color(name));
        let hover = ui.interact(
            seg.expand2(vec2(0.0, 5.0)),
            ui.id().with(("type-seg", &field.path, i)),
            Sense::hover(),
        );
        hover.on_hover_text(format!(
            "{name}: {count} ({:.1}%)",
            *count as f64 * 100.0 / total as f64
        ));
        x += w;
    }
    // Text label of the dominant type keeps identity readable without color.
    if let Some((name, count)) = field.types.first() {
        let label = if n > 1 {
            format!("{name} +{}", n - 1)
        } else {
            name.clone()
        };
        let _ = count;
        ui.painter().text(
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            label,
            FontId::proportional(11.5),
            p.text_secondary,
        );
    }
}

fn details(ui: &mut Ui, p: &Palette, field: &FieldStats) {
    ui.label(
        RichText::new(&field.path)
            .font(FontId::monospace(14.0))
            .strong()
            .color(p.text_strong),
    );
    ui.label(
        RichText::new(format!(
            "{} values · present in {:.1}% · {} null",
            field.count,
            field.probability() * 100.0,
            field.null_count
        ))
        .color(p.text_secondary),
    );
    ui.add_space(12.0);
    // Types legend (always present: identity is never color alone).
    widgets::caption(ui, "TYPES");
    ui.add_space(4.0);
    let total: u64 = field.types.iter().map(|(_, n)| n).sum::<u64>().max(1);
    for (name, count) in &field.types {
        ui.horizontal(|ui| {
            let (sw, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter()
                .rect_filled(sw, CornerRadius::same(2), p.type_color(name));
            ui.label(RichText::new(name).color(p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "{count}  ·  {:.1}%",
                        *count as f64 * 100.0 / total as f64
                    ))
                    .color(p.text_secondary),
                );
            });
        });
    }
    if let (Some(min), Some(max), Some(mean)) = (field.min, field.max, field.mean) {
        ui.add_space(14.0);
        widgets::caption(ui, "DISTRIBUTION");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            stat_inline(ui, p, "min", &number(min));
            stat_inline(ui, p, "mean", &number(mean));
            stat_inline(ui, p, "max", &number(max));
        });
        ui.add_space(18.0);
        histogram(ui, p, &field.histogram, min, max);
    }
    if let (Some(a), Some(b)) = (field.min_date, field.max_date) {
        ui.add_space(14.0);
        widgets::caption(ui, "DATE RANGE");
        let fmt =
            |d: mongo_raabit_core::bson::DateTime| d.try_to_rfc3339_string().unwrap_or_default();
        ui.label(RichText::new(format!("{}\n{}", fmt(a), fmt(b))).font(FontId::monospace(12.0)));
    }
    if let (Some(a), Some(b)) = (field.min_length, field.max_length) {
        ui.add_space(10.0);
        let what = if field.primary_type() == Some("Array") {
            "Elements"
        } else {
            "Length"
        };
        ui.label(RichText::new(format!("{what}: {a} … {b}")).color(p.text_secondary));
    }
    if !field.top_values.is_empty() {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            widgets::caption(ui, "TOP VALUES");
            let distinct = if field.distinct_overflow {
                format!("{}+ distinct", field.distinct_count)
            } else {
                format!("{} distinct", field.distinct_count)
            };
            ui.label(RichText::new(distinct).size(11.5).color(p.text_info));
        });
        ui.add_space(4.0);
        top_values(ui, p, &field.top_values);
    }
}

fn stat_inline(ui: &mut Ui, p: &Palette, label: &str, value: &str) {
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(11.0).color(p.text_info));
                ui.label(RichText::new(value).strong().color(p.text_strong));
            });
        });
}

/// Single-series histogram: thin bars with 2 px gaps and 4 px rounded tops anchored to
/// the baseline, axis labels at both ends, a hover tooltip per bucket.
fn histogram(ui: &mut Ui, p: &Palette, buckets: &[u64], min: f64, max: f64) {
    let height = 120.0;
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), height + 20.0), Sense::hover());
    let plot = Rect::from_min_size(rect.min, vec2(rect.width(), height));
    let peak = buckets.iter().copied().max().unwrap_or(0).max(1);
    // Recessive gridline at the peak and a baseline.
    ui.painter().hline(
        plot.x_range(),
        plot.top(),
        Stroke::new(1.0_f32, p.separator),
    );
    ui.painter().hline(
        plot.x_range(),
        plot.bottom(),
        Stroke::new(1.0_f32, p.border),
    );
    ui.painter().text(
        pos2(plot.left(), plot.top() - 1.0),
        Align2::LEFT_BOTTOM,
        peak.to_string(),
        FontId::proportional(10.5),
        p.text_info,
    );
    let n = buckets.len().max(1);
    let slot = plot.width() / n as f32;
    let width = (max - min) / HISTOGRAM_BUCKETS as f64;
    for (i, count) in buckets.iter().enumerate() {
        let h = (*count as f32 / peak as f32) * (height - 14.0);
        let bar = Rect::from_min_max(
            pos2(plot.left() + slot * i as f32 + 1.0, plot.bottom() - h),
            pos2(plot.left() + slot * (i + 1) as f32 - 1.0, plot.bottom()),
        );
        if *count > 0 {
            ui.painter().rect_filled(
                bar,
                CornerRadius {
                    nw: 4,
                    ne: 4,
                    sw: 0,
                    se: 0,
                },
                p.chart(0),
            );
        }
        let hit = Rect::from_min_max(
            pos2(bar.left(), plot.top()),
            pos2(bar.right(), plot.bottom()),
        );
        let response = ui.interact(hit, ui.id().with(("hist", i)), Sense::hover());
        if response.hovered() {
            ui.painter()
                .rect_filled(hit, CornerRadius::same(3), p.hover);
        }
        let lo = min + width * i as f64;
        response.on_hover_text(format!("{} – {}: {count}", number(lo), number(lo + width)));
    }
    ui.painter().text(
        pos2(plot.left(), rect.bottom()),
        Align2::LEFT_BOTTOM,
        number(min),
        FontId::proportional(11.0),
        p.text_secondary,
    );
    ui.painter().text(
        pos2(plot.right(), rect.bottom()),
        Align2::RIGHT_BOTTOM,
        number(max),
        FontId::proportional(11.0),
        p.text_secondary,
    );
}

/// Horizontal bars of the most frequent values with direct labels.
fn top_values(ui: &mut Ui, p: &Palette, values: &[(String, u64)]) {
    let peak = values.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    for (value, count) in values {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        let label_w = rect.width() * 0.42;
        let label = crate::values::truncate(value, 22);
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::monospace(11.5),
            p.text,
        );
        let track = Rect::from_min_max(
            pos2(rect.left() + label_w, rect.center().y - 4.0),
            pos2(rect.right() - 40.0, rect.center().y + 4.0),
        );
        let w = track.width() * (*count as f32 / peak as f32);
        ui.painter().rect_filled(
            Rect::from_min_size(track.min, vec2(w.max(2.0), track.height())),
            CornerRadius {
                nw: 0,
                sw: 0,
                ne: 4,
                se: 4,
            },
            p.chart(0),
        );
        ui.painter().text(
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(11.5),
            p.text_secondary,
        );
        response.on_hover_text(format!("{value}: {count}"));
    }
    let _ = style::RADIUS;
}
