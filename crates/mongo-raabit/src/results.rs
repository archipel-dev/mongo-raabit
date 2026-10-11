//! Result views of a collection tab: Table (with nested drill-down), Tree and JSON.

use crate::app::{CellEdit, CollectionTab};
use crate::icons::{self, Icon};
use crate::style::Palette;
use crate::values;
use egui::{Align2, CornerRadius, FontId, RichText, Sense, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::shell;

/// Rows shown by the table for the current drill path: (source document index, row).
pub fn table_rows(docs: &[Document], drill: &[String]) -> Vec<(usize, Bson)> {
    let mut rows = Vec::new();
    for (index, doc) in docs.iter().enumerate() {
        let mut current = vec![Bson::Document(doc.clone())];
        for segment in drill {
            let mut next = Vec::new();
            for value in current {
                if let Bson::Document(d) = value
                    && let Some(v) = d.get(segment)
                {
                    match v {
                        // Arrays expand into one row per element.
                        Bson::Array(items) => next.extend(items.iter().cloned()),
                        other => next.push(other.clone()),
                    }
                }
            }
            current = next;
        }
        rows.extend(current.into_iter().map(|v| (index, v)));
    }
    rows
}

/// Column names: union of the rows' document keys in first-seen order, or a single
/// `value` column for scalar rows.
pub fn table_columns(rows: &[(usize, Bson)]) -> Vec<String> {
    let mut columns: Vec<String> = Vec::new();
    let mut scalar = false;
    for (_, row) in rows {
        match row {
            Bson::Document(d) => {
                for key in d.keys() {
                    if !columns.iter().any(|c| c == key) {
                        columns.push(key.clone());
                    }
                }
            }
            _ => scalar = true,
        }
    }
    if scalar {
        columns.insert(0, "value".into());
    }
    columns
}

fn cell(ui: &mut Ui, p: &Palette, value: Option<&Bson>) -> egui::Response {
    match value {
        None => ui.add(
            egui::Label::new(RichText::new("—").color(p.text_info.gamma_multiply(0.6)))
                .sense(Sense::click()),
        ),
        Some(value @ (Bson::Document(_) | Bson::Array(_))) => {
            // Nested values render as a pill that drills down on click.
            let text = values::preview(value);
            let galley =
                ui.painter()
                    .layout_no_wrap(text, FontId::proportional(12.0), p.text_secondary);
            let (rect, response) =
                ui.allocate_exact_size(galley.size() + vec2(22.0, 4.0), Sense::click());
            let fill = if response.hovered() {
                p.selection
            } else {
                p.selection_inactive
            };
            ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
            icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(
                    pos2(rect.left() + 9.0, rect.center().y),
                    vec2(11.0, 11.0),
                ),
                if matches!(value, Bson::Array(_)) {
                    Icon::Table
                } else {
                    Icon::Tree
                },
                p.text_secondary,
            );
            ui.painter().galley(
                pos2(rect.left() + 18.0, rect.top() + 2.0),
                galley,
                p.text_secondary,
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            response.on_hover_text("Drill down")
        }
        Some(value) => ui.add(
            egui::Label::new(
                RichText::new(values::preview(value))
                    .font(FontId::monospace(12.5))
                    .color(values::color(value, p)),
            )
            .truncate()
            .sense(Sense::click()),
        ),
    }
}

/// Initial column width from the header and the first rows' previews.
fn column_width(rows: &[(usize, Bson)], name: &str) -> f32 {
    let header = name.chars().count() as f32 * 7.5 + 70.0;
    let content = rows
        .iter()
        .take(40)
        .filter_map(|(_, row)| match row {
            Bson::Document(d) => d.get(name).map(|v| match v {
                Bson::Document(_) | Bson::Array(_) => {
                    values::preview(v).chars().count() as f32 * 6.6 + 30.0
                }
                other => values::preview(other).chars().count() as f32 * 7.6 + 16.0,
            }),
            other if name == "value" => {
                Some(values::preview(other).chars().count() as f32 * 7.6 + 16.0)
            }
            _ => None,
        })
        .fold(0.0_f32, f32::max);
    header.max(content).clamp(64.0, 340.0)
}

/// Commands on one document, from context menus and the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocCommand {
    Edit,
    Clone,
    Delete,
    CopyJson,
    CopyId,
}

pub enum TableEvent {
    Drill(String),
    Select(usize),
    Open(usize),
    Command(DocCommand, usize),
    CommitEdit,
}

/// Dotted path of the drill position when it only crosses embedded documents (inline
/// edits need a unique field path; rows expanded from arrays have none).
pub fn editable_prefix(doc: &Document, drill: &[String]) -> Option<String> {
    let mut current = doc;
    for segment in drill {
        current = current.get(segment)?.as_document()?;
    }
    Some(drill.join("."))
}

fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

/// Context menu shared by table and tree rows.
pub fn document_menu(
    ui: &mut Ui,
    doc_index: usize,
    value: Option<&Bson>,
    event: &mut Option<TableEvent>,
) {
    if ui.button("Edit Document…").clicked() {
        *event = Some(TableEvent::Command(DocCommand::Edit, doc_index));
        ui.close();
    }
    if ui.button("Clone Document…").clicked() {
        *event = Some(TableEvent::Command(DocCommand::Clone, doc_index));
        ui.close();
    }
    ui.separator();
    if let Some(value) = value
        && ui.button("Copy Value").clicked()
    {
        ui.ctx()
            .copy_text(shell::to_shell_string(value, shell::ShellStyle::Compact));
        ui.close();
    }
    if ui.button("Copy Document").clicked() {
        *event = Some(TableEvent::Command(DocCommand::CopyJson, doc_index));
        ui.close();
    }
    if ui.button("Copy _id").clicked() {
        *event = Some(TableEvent::Command(DocCommand::CopyId, doc_index));
        ui.close();
    }
    ui.separator();
    if ui.button("Delete Document…").clicked() {
        *event = Some(TableEvent::Command(DocCommand::Delete, doc_index));
        ui.close();
    }
}

/// Breadcrumb for the drill path; returns the new depth when a crumb was clicked.
pub fn breadcrumb(ui: &mut Ui, p: &Palette, drill: &[String]) -> Option<usize> {
    let mut target = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let root = ui.add(
            egui::Label::new(RichText::new("documents").color(if drill.is_empty() {
                p.text
            } else {
                p.accent_text
            }))
            .sense(Sense::click()),
        );
        if root.clicked() {
            target = Some(0);
        }
        for (i, segment) in drill.iter().enumerate() {
            icons::icon(ui, Icon::ChevronRight, p.text_info);
            let last = i + 1 == drill.len();
            let r = ui.add(
                egui::Label::new(RichText::new(segment).color(if last {
                    p.text
                } else {
                    p.accent_text
                }))
                .sense(Sense::click()),
            );
            if r.clicked() && !last {
                target = Some(i + 1);
            }
        }
    });
    target
}

pub fn table(
    ui: &mut Ui,
    tab: &mut CollectionTab,
    docs: &[Document],
    first_index: u64,
) -> Option<TableEvent> {
    let p = Palette::of(ui);
    let rows = table_rows(docs, &tab.drill);
    let columns = table_columns(&rows);
    let mut event = None;
    if rows.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No values at this path").color(p.text_secondary))
        });
        return None;
    }
    let header_bg = p.raised;
    let mut start_edit: Option<CellEdit> = None;
    let mut cancel_edit = false;
    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut builder = TableBuilder::new(ui)
                .id_salt(("raabit-table", &tab.db, &tab.collection, tab.drill.len()))
                .striped(true)
                .resizable(true)
                .sense(Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(52.0));
            for name in &columns {
                builder = builder.column(
                    Column::initial(column_width(&rows, name))
                        .at_least(48.0)
                        .clip(true),
                );
            }
            builder
                .header(26.0, |mut header| {
                    header.col(|ui| {
                        ui.painter().rect_filled(
                            ui.max_rect().expand2(vec2(4.0, 0.0)),
                            0.0,
                            header_bg,
                        );
                        ui.label(RichText::new("#").color(p.text_info));
                    });
                    for name in &columns {
                        header.col(|ui| {
                            ui.painter().rect_filled(
                                ui.max_rect().expand2(vec2(4.0, 0.0)),
                                0.0,
                                header_bg,
                            );
                            // Dominant type of the column, shown as a dimmed suffix.
                            let ty = rows.iter().find_map(|(_, r)| match r {
                                Bson::Document(d) => d.get(name).map(shell::type_name),
                                other if name == "value" => Some(shell::type_name(other)),
                                _ => None,
                            });
                            ui.label(RichText::new(name).strong().color(p.text));
                            if let Some(ty) = ty {
                                ui.label(RichText::new(ty).size(10.5).color(p.text_info));
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(24.0, rows.len(), |mut row| {
                        let (doc_index, value) = &rows[row.index()];
                        row.set_selected(tab.selected == Some(*doc_index));
                        row.col(|ui| {
                            ui.label(
                                RichText::new((first_index + *doc_index as u64).to_string())
                                    .font(FontId::monospace(11.5))
                                    .color(p.text_info),
                            );
                        });
                        let prefix = docs
                            .get(*doc_index)
                            .and_then(|doc| editable_prefix(doc, &tab.drill))
                            .filter(|_| matches!(value, Bson::Document(_)));
                        for name in &columns {
                            row.col(|ui| {
                                let v = match value {
                                    Bson::Document(d) => d.get(name),
                                    other if name == "value" => Some(other),
                                    _ => None,
                                };
                                let path = prefix.as_ref().map(|prefix| join_path(prefix, name));
                                if let Some(edit) = &mut tab.editing
                                    && edit.doc_index == *doc_index
                                    && Some(&edit.path) == path.as_ref()
                                {
                                    let r = ui.add(
                                        egui::TextEdit::singleline(&mut edit.text)
                                            .font(FontId::monospace(12.5))
                                            .desired_width(f32::INFINITY),
                                    );
                                    if edit.focus {
                                        r.request_focus();
                                        edit.focus = false;
                                    } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        cancel_edit = true;
                                    } else if r.lost_focus() {
                                        event = Some(TableEvent::CommitEdit);
                                    }
                                    return;
                                }
                                let response = cell(ui, &p, v);
                                if response.clicked() {
                                    event = Some(
                                        if matches!(v, Some(Bson::Document(_) | Bson::Array(_))) {
                                            TableEvent::Drill(name.clone())
                                        } else {
                                            TableEvent::Select(*doc_index)
                                        },
                                    );
                                }
                                if response.double_clicked() {
                                    match (&path, v) {
                                        (Some(path), None | Some(_))
                                            if name != "_id"
                                                && !matches!(
                                                    v,
                                                    Some(Bson::Document(_) | Bson::Array(_))
                                                ) =>
                                        {
                                            start_edit = Some(CellEdit {
                                                doc_index: *doc_index,
                                                path: path.clone(),
                                                text: match v {
                                                    Some(Bson::String(s)) => s.clone(),
                                                    Some(other) => shell::to_shell_string(
                                                        other,
                                                        shell::ShellStyle::Compact,
                                                    ),
                                                    None => String::new(),
                                                },
                                                original: v.cloned(),
                                                focus: true,
                                            });
                                        }
                                        _ => event = Some(TableEvent::Open(*doc_index)),
                                    }
                                }
                                response.context_menu(|ui| {
                                    document_menu(ui, *doc_index, v, &mut event)
                                });
                            });
                        }
                        let response = row.response();
                        if response.double_clicked()
                            && start_edit.is_none()
                            && tab.editing.is_none()
                        {
                            event = Some(TableEvent::Open(*doc_index));
                        } else if response.clicked() && event.is_none() {
                            event = Some(TableEvent::Select(*doc_index));
                        }
                    });
                });
        });
    if cancel_edit {
        tab.editing = None;
    }
    if let Some(edit) = start_edit {
        tab.selected = Some(edit.doc_index);
        tab.editing = Some(edit);
    }
    event
}

/// One visible row of the tree view.
pub struct TreeRow {
    pub key: String,
    pub depth: usize,
    pub label: String,
    pub value: Bson,
    pub expandable: bool,
    pub doc_index: usize,
}

/// Flatten the open part of the tree.
pub fn tree_rows(
    docs: &[Document],
    open: &std::collections::HashSet<String>,
    first_index: u64,
) -> Vec<TreeRow> {
    fn walk(
        out: &mut Vec<TreeRow>,
        open: &std::collections::HashSet<String>,
        key: String,
        depth: usize,
        label: String,
        value: &Bson,
        doc_index: usize,
    ) {
        let expandable = matches!(value, Bson::Document(d) if !d.is_empty())
            || matches!(value, Bson::Array(a) if !a.is_empty());
        let is_open = expandable && open.contains(&key);
        out.push(TreeRow {
            key: key.clone(),
            depth,
            label,
            value: value.clone(),
            expandable,
            doc_index,
        });
        if !is_open {
            return;
        }
        match value {
            Bson::Document(d) => {
                for (k, v) in d {
                    walk(
                        out,
                        open,
                        format!("{key}/{k}"),
                        depth + 1,
                        k.clone(),
                        v,
                        doc_index,
                    );
                }
            }
            Bson::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    walk(
                        out,
                        open,
                        format!("{key}/{i}"),
                        depth + 1,
                        format!("[{i}]"),
                        v,
                        doc_index,
                    );
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (i, doc) in docs.iter().enumerate() {
        // Label by `_id`, or by the first field when the projection dropped it.
        let id = match doc.get("_id") {
            Some(id) => values::preview(id),
            None => doc
                .iter()
                .next()
                .map(|(k, v)| format!("{k}: {}", values::preview(v)))
                .unwrap_or_default(),
        };
        let label = format!("({}) {}", first_index + i as u64, id);
        walk(
            &mut out,
            open,
            i.to_string(),
            0,
            label,
            &Bson::Document(doc.clone()),
            i,
        );
    }
    out
}

pub enum TreeEvent {
    Toggle(String),
    Select(usize),
    Open(usize),
    Command(DocCommand, usize),
}

/// Tree view state shared by collection, aggregation and SQL result panes.
pub struct TreeState<'a> {
    pub id: egui::Id,
    pub open: &'a std::collections::HashSet<String>,
    pub selected: Option<usize>,
}

pub fn tree(
    ui: &mut Ui,
    tab: TreeState<'_>,
    docs: &[Document],
    first_index: u64,
) -> Option<TreeEvent> {
    let p = Palette::of(ui);
    let rows = tree_rows(docs, tab.open, first_index);
    let mut event = None;
    let header_bg = p.raised;
    TableBuilder::new(ui)
        .id_salt(tab.id)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(300.0).at_least(120.0).clip(true))
        .column(Column::remainder().at_least(160.0).clip(true))
        .column(Column::initial(110.0).at_least(70.0))
        .header(26.0, |mut header| {
            for title in ["Key", "Value", "Type"] {
                header.col(|ui| {
                    ui.painter()
                        .rect_filled(ui.max_rect().expand2(vec2(4.0, 0.0)), 0.0, header_bg);
                    ui.label(RichText::new(title).strong());
                });
            }
        })
        .body(|body| {
            body.rows(24.0, rows.len(), |mut row| {
                let r = &rows[row.index()];
                row.set_selected(r.depth == 0 && tab.selected == Some(r.doc_index));
                row.col(|ui| {
                    ui.add_space(r.depth as f32 * 16.0);
                    let (rect, response) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
                    if r.expandable {
                        icons::paint(
                            ui.painter(),
                            rect,
                            if tab.open.contains(&r.key) {
                                Icon::ChevronDown
                            } else {
                                Icon::ChevronRight
                            },
                            p.text_info,
                        );
                        if response.clicked() {
                            event = Some(TreeEvent::Toggle(r.key.clone()));
                        }
                    }
                    let icon = match &r.value {
                        Bson::Document(_) => Icon::Tree,
                        Bson::Array(_) => Icon::Table,
                        _ => Icon::Json,
                    };
                    icons::icon(ui, icon, if r.depth == 0 { p.leaf } else { p.text_info });
                    let color = if r.depth == 0 {
                        p.text_strong
                    } else if r.label.starts_with('[') {
                        p.text_secondary
                    } else {
                        p.key
                    };
                    ui.label(
                        RichText::new(&r.label)
                            .font(FontId::monospace(12.5))
                            .color(color),
                    );
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(values::preview(&r.value))
                            .font(FontId::monospace(12.5))
                            .color(values::color(&r.value, &p)),
                    );
                });
                row.col(|ui| {
                    ui.label(RichText::new(shell::type_name(&r.value)).color(p.text_info));
                });
                let response = row.response();
                response.context_menu(|ui| {
                    let mut menu_event = None;
                    document_menu(ui, r.doc_index, Some(&r.value), &mut menu_event);
                    if let Some(TableEvent::Command(command, index)) = menu_event {
                        event = Some(TreeEvent::Command(command, index));
                    }
                });
                if response.double_clicked() {
                    event = Some(if r.expandable {
                        TreeEvent::Toggle(r.key.clone())
                    } else {
                        TreeEvent::Open(r.doc_index)
                    });
                } else if response.clicked() && event.is_none() {
                    event = Some(TreeEvent::Select(r.doc_index));
                }
            });
        });
    event
}

pub fn json(ui: &mut Ui, docs: &[Document], first_index: u64) {
    let p = Palette::of(ui);
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    for (i, doc) in docs.iter().enumerate() {
                        let (rect, _) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
                        ui.painter().text(
                            pos2(rect.left(), rect.center().y),
                            Align2::LEFT_CENTER,
                            format!("/* {} */", first_index + i as u64),
                            FontId::monospace(11.5),
                            p.text_info,
                        );
                        let job = values::document_job(doc, &p, FontId::monospace(12.5));
                        ui.add(egui::Label::new(job).selectable(true));
                        ui.add_space(8.0);
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongo_raabit_core::bson::doc;

    #[test]
    fn drill_down_expands_arrays_and_documents() {
        let docs = vec![
            doc! {"_id": 1, "address": {"city": "Oslo"}, "items": [{"sku": "a"}, {"sku": "b", "qty": 2}], "tags": ["x"]},
            doc! {"_id": 2, "items": [{"sku": "c"}]},
        ];
        let rows = table_rows(&docs, &[]);
        assert_eq!(table_columns(&rows), ["_id", "address", "items", "tags"]);
        let items = table_rows(&docs, &["items".into()]);
        assert_eq!(items.len(), 3);
        assert_eq!(items[2].0, 1);
        assert_eq!(table_columns(&items), ["sku", "qty"]);
        let tags = table_rows(&docs, &["tags".into()]);
        assert_eq!(table_columns(&tags), ["value"]);
        assert_eq!(table_rows(&docs, &["address".into()]).len(), 1);
    }

    #[test]
    fn tree_rows_follow_open_set() {
        let docs = vec![doc! {"_id": 1, "a": {"b": [1, 2]}}];
        let mut open = std::collections::HashSet::new();
        assert_eq!(tree_rows(&docs, &open, 1).len(), 1);
        open.insert("0".to_string());
        open.insert("0/a".to_string());
        let rows = tree_rows(&docs, &open, 1);
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            ["(1) 1", "_id", "a", "b"]
        );
        open.insert("0/a/b".to_string());
        assert_eq!(tree_rows(&docs, &open, 1).len(), 6);
    }
}
