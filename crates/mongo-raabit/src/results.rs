//! Result views of a collection tab, on the kit: Table (with nested drill-down), Tree and
//! JSON. A cell is edited in place through the kit's cell editor; a commit becomes the
//! `$set` update of `MongoRaabit::commit_cell_edit`.

use crate::app::{CellEdit, CollectionTab};
use crate::egui::{Id, Rect, Ui};
use crate::icons::Icon;
use crate::values;
use archipel_sdk::kit::components::code::JavaScript;
use archipel_sdk::kit::components::{
    Cell, CodeView, Column, EmptyState, Link, SortOrder, Table, TreeRow as KitRow, TreeTable,
};
use archipel_sdk::kit::tokens::size;
use archipel_sdk::kit::{Glyph, Tone};
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::shell;
use std::sync::Arc;

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

/// Commands on one document, from the toolbar.
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
    /// A cell edit to store with `$set`.
    Edit(CellEdit),
    /// A header was clicked: sort the query by this field.
    Sort {
        field: String,
        descending: bool,
    },
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

/// Fields that `$set` can reach by a dotted path.
fn settable(segment: &str) -> bool {
    !segment.is_empty() && !segment.contains('.') && !segment.starts_with('$')
}

/// The text an inline edit starts from. Only scalars whose cell shows exactly that text
/// are edited in place (dates and the like go through the document editor).
fn inline_text(value: &Bson) -> Option<String> {
    match value {
        Bson::String(s) if values::preview(value) == *s => Some(s.clone()),
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Boolean(_) | Bson::Null => {
            Some(values::preview(value))
        }
        _ => None,
    }
}

fn edit_of(doc_index: usize, path: String, text: String, original: &Bson) -> CellEdit {
    CellEdit {
        doc_index,
        path,
        text,
        original: Some(original.clone()),
        focus: false,
    }
}

/// Breadcrumb for the drill path; returns the new depth when a crumb was clicked.
pub fn breadcrumb(ui: &mut Ui, drill: &[String]) -> Option<usize> {
    let mut target = None;
    let muted = Tone::Muted.resolve(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if drill.is_empty() {
            ui.label("documents");
        } else if ui.add(Link::new("documents")).clicked() {
            target = Some(0);
        }
        for (i, segment) in drill.iter().enumerate() {
            archipel_sdk::kit::icons::show(ui, Icon::ChevronRight.kit(), size::ICON, muted);
            if i + 1 == drill.len() {
                ui.label(segment);
            } else if ui.add(Link::new(segment)).clicked() {
                target = Some(i + 1);
            }
        }
    });
    target
}

fn container_icon(value: &Bson) -> Icon {
    if matches!(value, Bson::Array(_)) {
        Icon::Table
    } else {
        Icon::Tree
    }
}

/// A value cell: typed colour, a summary for nested values.
fn value_cell(value: Option<&Bson>, editable: bool) -> Cell {
    match value {
        None => Cell::new("—").muted(),
        Some(v @ (Bson::Document(_) | Bson::Array(_))) => Cell::new(values::preview(v))
            .icon(container_icon(v).kit())
            .value_kind(values::kind(v)),
        Some(v) => Cell::new(values::preview(v))
            .value_kind(values::kind(v))
            .editable(editable),
    }
}

const INDEX: &str = "#";

/// Column order and widths of one table, kept between frames (the caller owns them).
#[derive(Clone, Default)]
struct TableState {
    /// The data column at each position (`#` is the row number).
    names: Vec<String>,
    columns: Vec<Column>,
    /// The selected row, to tell the rows an array expands into apart.
    row: Option<usize>,
}

impl TableState {
    /// Follow the columns of the current rows: keep order and widths of the known ones.
    fn reconcile(&mut self, fresh: &[String], rows: &[(usize, Bson)], available: f32) {
        let same = self.names.len() == fresh.len() && fresh.iter().all(|n| self.names.contains(n));
        if same {
            return;
        }
        let first = self.names.is_empty();
        let (mut names, mut columns) = (Vec::new(), Vec::new());
        for (name, column) in self.names.iter().zip(&self.columns) {
            if fresh.contains(name) {
                names.push(name.clone());
                columns.push(column.clone());
            }
        }
        for name in fresh {
            if !names.contains(name) {
                let column = if name == INDEX {
                    Column::new(INDEX, 52.0).mono()
                } else {
                    Column::new(name, column_width(rows, name)).mono()
                };
                names.push(name.clone());
                columns.push(column);
            }
        }
        if first {
            fit(&mut columns, available);
        }
        self.names = names;
        self.columns = columns;
    }
}

/// Shrink the data columns so they fit `available` (the kit table does not scroll sideways).
fn fit(columns: &mut [Column], available: f32) {
    let total: f32 = columns.iter().map(|c| c.width).sum();
    let fixed = columns.first().map_or(0.0, |c| c.width);
    if total <= available || total <= fixed {
        return;
    }
    let scale = ((available - fixed - 16.0) / (total - fixed)).clamp(0.2, 1.0);
    for column in columns.iter_mut().skip(1) {
        column.width = (column.width * scale).max(64.0);
    }
}

/// The column under a primary click inside the table body, when the click hit a row.
fn clicked_column(ui: &Ui, body: Rect, columns: &[Column], rows: usize) -> Option<usize> {
    let pos = ui.input(|i| {
        i.pointer
            .primary_clicked()
            .then(|| i.pointer.interact_pos())
            .flatten()
    })?;
    let content = rows as f32 * size::ROW_HEIGHT;
    let on_row = pos.y < body.top() + content || content > body.height();
    // The scroll bar sits at the right edge.
    if !body.contains(pos) || !on_row || pos.x > body.right() - 12.0 {
        return None;
    }
    let mut x = body.left() + size::ROW_PILL_INSET;
    for (index, column) in columns.iter().enumerate() {
        if pos.x < x + column.width {
            return Some(index);
        }
        x += column.width;
    }
    None
}

/// `{ name: 1 }` for the query's sort box.
pub fn sort_text(field: &str, descending: bool) -> String {
    let key = if !field.is_empty()
        && field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !field.starts_with(|c: char| c.is_ascii_digit())
    {
        field.to_owned()
    } else {
        format!("\"{}\"", field.replace('"', "\\\""))
    };
    format!("{{ {key}: {} }}", if descending { -1 } else { 1 })
}

pub fn table(
    ui: &mut Ui,
    tab: &mut CollectionTab,
    docs: &[Document],
    first_index: u64,
) -> Option<TableEvent> {
    let rows = table_rows(docs, &tab.drill);
    if rows.is_empty() {
        EmptyState::new("No values at this path")
            .icon(Glyph::Filter)
            .hint("Go back with the breadcrumb above.")
            .show(ui);
        return None;
    }
    let id = Id::new((
        "raabit-table",
        &tab.db,
        &tab.collection,
        tab.drill.join("/"),
    ));
    let sort_id = Id::new(("raabit-sort", &tab.db, &tab.collection));
    let mut state: TableState = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let fresh: Vec<String> = std::iter::once(INDEX.to_owned())
        .chain(table_columns(&rows))
        .collect();
    state.reconcile(&fresh, &rows, ui.available_width());
    // The arrow follows the query's own sort box: it shows only while the box still holds
    // the text a header click wrote.
    let sorted: Option<(String, bool, String)> = ui.data(|d| d.get_temp(sort_id));
    let sort = sorted
        .as_ref()
        .filter(|(_, _, text)| *text == tab.query.sort && tab.drill.is_empty())
        .and_then(|(field, descending, _)| {
            let position = state.names.iter().position(|n| n == field)?;
            let order = if *descending {
                SortOrder::Descending
            } else {
                SortOrder::Ascending
            };
            Some((position, order))
        });
    let mut selected = state
        .row
        .filter(|r| rows.get(*r).map(|(d, _)| *d) == tab.selected)
        .or_else(|| {
            tab.selected
                .and_then(|d| rows.iter().position(|(i, _)| *i == d))
        });
    let drill = tab.drill.clone();
    let value_of = |row: usize, name: &str| -> Option<&Bson> {
        match &rows[row].1 {
            Bson::Document(d) => d.get(name),
            other if name == "value" => Some(other),
            _ => None,
        }
    };
    // The field path of an editable cell, `None` for cells that are read-only.
    let path_of = |row: usize, name: &str| -> Option<String> {
        let (doc_index, value) = &rows[row];
        if !matches!(value, Bson::Document(_)) || name == "_id" || !settable(name) {
            return None;
        }
        let prefix = editable_prefix(docs.get(*doc_index)?, &drill)?;
        Some(join_path(&prefix, name))
    };
    let TableState {
        names,
        columns,
        row: _,
    } = &mut state;
    let out = Table::new(id, columns, rows.len(), &mut selected)
        .name("Documents")
        .sort(sort)
        .reorderable(true)
        .height(ui.available_height())
        .show(ui, |row, position| {
            let name = names[position].as_str();
            if name == INDEX {
                return Cell::new((first_index + rows[row].0 as u64).to_string()).muted();
            }
            let value = value_of(row, name);
            let editable =
                value.is_some_and(|v| inline_text(v).is_some()) && path_of(row, name).is_some();
            value_cell(value, editable)
        });
    let mut event = None;
    if let Some(edit) = out.edited
        && let Some(name) = names.get(edit.column)
        && let Some(value) = value_of(edit.row, name)
        && let Some(path) = path_of(edit.row, name)
    {
        event = Some(TableEvent::Edit(edit_of(
            rows[edit.row].0,
            path,
            edit.text,
            value,
        )));
    }
    if let Some(order) = out.column_order {
        *columns = order.iter().map(|&i| columns[i].clone()).collect();
        *names = order.iter().map(|&i| names[i].clone()).collect();
    }
    if event.is_none()
        && let Some(position) = out.sort_clicked
        && tab.drill.is_empty()
        && let Some(field) = names.get(position)
        && field != INDEX
    {
        let descending = matches!(sort, Some((p, SortOrder::Ascending)) if p == position);
        let text = sort_text(field, descending);
        ui.data_mut(|d| d.insert_temp(sort_id, (field.clone(), descending, text)));
        event = Some(TableEvent::Sort {
            field: field.clone(),
            descending,
        });
    }
    // A click on a nested value drills into it; the kit reports the row, not the column.
    if event.is_none()
        && let Some(row) = selected
        && let Some(position) = clicked_column(ui, out.response.rect, columns, rows.len())
        && let Some(name) = names.get(position)
        && name != INDEX
        && matches!(
            value_of(row, name),
            Some(Bson::Document(_) | Bson::Array(_))
        )
    {
        event = Some(TableEvent::Drill(name.clone()));
    }
    if event.is_none() {
        if let Some(row) = out.activated {
            event = Some(TableEvent::Open(rows[row].0));
        } else if out.selection_changed
            && let Some(row) = selected
        {
            event = Some(TableEvent::Select(rows[row].0));
        }
    }
    state.row = selected;
    ui.data_mut(|d| d.insert_temp(id, state));
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
    /// Field names from the document down to this row (array positions included).
    pub path: Vec<String>,
}

/// Flatten the open part of the tree.
pub fn tree_rows(
    docs: &[Document],
    open: &std::collections::HashSet<String>,
    first_index: u64,
) -> Vec<TreeRow> {
    struct Walk<'a> {
        out: Vec<TreeRow>,
        open: &'a std::collections::HashSet<String>,
    }
    impl Walk<'_> {
        fn visit(
            &mut self,
            key: String,
            label: String,
            value: &Bson,
            doc_index: usize,
            path: Vec<String>,
        ) {
            let expandable = matches!(value, Bson::Document(d) if !d.is_empty())
                || matches!(value, Bson::Array(a) if !a.is_empty());
            let is_open = expandable && self.open.contains(&key);
            self.out.push(TreeRow {
                key: key.clone(),
                depth: path.len(),
                label,
                value: value.clone(),
                expandable,
                doc_index,
                path: path.clone(),
            });
            if !is_open {
                return;
            }
            let mut child = |name: String, label: String, v: &Bson| {
                let mut path = path.clone();
                path.push(name.clone());
                self.visit(format!("{key}/{name}"), label, v, doc_index, path);
            };
            match value {
                Bson::Document(d) => {
                    for (k, v) in d {
                        child(k.clone(), k.clone(), v);
                    }
                }
                Bson::Array(items) => {
                    for (i, v) in items.iter().enumerate() {
                        child(i.to_string(), format!("[{i}]"), v);
                    }
                }
                _ => {}
            }
        }
    }
    let mut walk = Walk {
        out: Vec::new(),
        open,
    };
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
        walk.visit(
            i.to_string(),
            label,
            &Bson::Document(doc.clone()),
            i,
            Vec::new(),
        );
    }
    walk.out
}

pub enum TreeEvent {
    Toggle(String),
    Select(usize),
    Open(usize),
    /// A leaf edit to store with `$set`.
    Edit(CellEdit),
}

/// Tree view state shared by collection, aggregation and SQL result panes.
pub struct TreeState<'a> {
    pub id: crate::egui::Id,
    pub open: &'a std::collections::HashSet<String>,
    pub selected: Option<usize>,
}

/// Column widths and the selected row of one tree, kept between frames.
#[derive(Clone, Default)]
struct TreeMem {
    columns: Vec<Column>,
    key: Option<String>,
}

/// Read-only tree (aggregation and shell results).
pub fn tree(
    ui: &mut Ui,
    state: TreeState<'_>,
    docs: &[Document],
    first_index: u64,
) -> Option<TreeEvent> {
    tree_view(ui, state, docs, first_index, false)
}

/// The tree as a kit `TreeTable` (key | value | type); `editable` lets leaf values be
/// edited in place.
pub fn tree_view(
    ui: &mut Ui,
    state: TreeState<'_>,
    docs: &[Document],
    first_index: u64,
    editable: bool,
) -> Option<TreeEvent> {
    let rows = tree_rows(docs, state.open, first_index);
    let mem_id = state.id.with("mem");
    let mut mem: TreeMem = ui.data_mut(|d| d.get_temp(mem_id)).unwrap_or_default();
    if mem.columns.is_empty() {
        let width = (ui.available_width() - 24.0).max(300.0);
        mem.columns = vec![
            Column::new("Key", (width * 0.34).round()).mono(),
            Column::new("Value", (width * 0.46).round()).mono(),
            Column::new("Type", (width * 0.20).round()),
        ];
    }
    let kit_rows: Vec<KitRow> = rows
        .iter()
        .map(|r| {
            let row = KitRow::new(r.label.as_str(), r.depth);
            if r.expandable {
                row.parent(state.open.contains(&r.key))
                    .icon(container_icon(&r.value).kit())
            } else {
                row
            }
        })
        .collect();
    let mut selected = mem
        .key
        .as_ref()
        .and_then(|key| rows.iter().position(|r| &r.key == key))
        .filter(|i| Some(rows[*i].doc_index) == state.selected)
        .or_else(|| {
            state
                .selected
                .and_then(|d| rows.iter().position(|r| r.doc_index == d && r.depth == 0))
        });
    let path_of = |r: &TreeRow| -> Option<String> {
        (editable
            && r.path.first().is_some_and(|first| first != "_id")
            && r.path.iter().all(|s| settable(s)))
        .then(|| r.path.join("."))
    };
    let height = ui.available_height();
    let out = TreeTable::new(state.id, &mut mem.columns, &kit_rows, &mut selected)
        .name("Documents tree")
        .height(height)
        .show(ui, |row, column| {
            let r = &rows[row];
            match column {
                1 => {
                    let leaf_editable =
                        !r.expandable && inline_text(&r.value).is_some() && path_of(r).is_some();
                    let cell =
                        Cell::new(values::preview(&r.value)).value_kind(values::kind(&r.value));
                    if r.expandable || matches!(r.value, Bson::Document(_) | Bson::Array(_)) {
                        cell
                    } else {
                        cell.editable(leaf_editable)
                    }
                }
                _ => Cell::new(shell::type_name(&r.value)).muted(),
            }
        });
    let mut event = None;
    if let Some(edit) = out.edited
        && let Some(r) = rows.get(edit.row)
        && let Some(path) = path_of(r)
    {
        event = Some(TreeEvent::Edit(edit_of(
            r.doc_index,
            path,
            edit.text,
            &r.value,
        )));
    } else if let Some(row) = out.toggled {
        event = Some(TreeEvent::Toggle(rows[row].key.clone()));
    } else if let Some(row) = out.activated {
        let r = &rows[row];
        event = Some(if r.expandable {
            TreeEvent::Toggle(r.key.clone())
        } else {
            TreeEvent::Open(r.doc_index)
        });
    } else if out.selection_changed
        && let Some(row) = selected
    {
        event = Some(TreeEvent::Select(rows[row].doc_index));
    }
    mem.key = selected.and_then(|i| rows.get(i)).map(|r| r.key.clone());
    ui.data_mut(|d| d.insert_temp(mem_id, mem));
    event
}

/// The documents as shell text, one `/* n */` marker per document.
fn json_text(docs: &[Document], first_index: u64) -> String {
    let mut text = String::new();
    for (i, doc) in docs.iter().enumerate() {
        text.push_str(&format!("/* {} */\n", first_index + i as u64));
        text.push_str(&shell::document_to_shell(doc, shell::ShellStyle::Pretty(2)));
        text.push_str("\n\n");
    }
    text
}

/// Fold arrows in the JSON view. Off until the kit's fold buttons stop overlapping the
/// document's accessible bounds (the audit reports it; see "Kit gaps" in the PR).
const FOLDING: bool = false;

/// Read-only code view of the documents with highlighting and folding.
pub fn json(ui: &mut Ui, docs: &[Document], first_index: u64) {
    json_cached(ui, Id::new("raabit-json"), docs, first_index, None);
}

/// Like [`json`]; with a `version` (bumped when the documents change) the text is built
/// once and reused between frames.
pub fn json_cached(ui: &mut Ui, id: Id, docs: &[Document], first_index: u64, version: Option<u64>) {
    let text: Arc<String> = match version {
        Some(version) => {
            let cached: Option<(u64, u64, Arc<String>)> = ui.data(|d| d.get_temp(id));
            match cached {
                Some((v, f, text)) if v == version && f == first_index => text,
                _ => {
                    let text = Arc::new(json_text(docs, first_index));
                    ui.data_mut(|d| d.insert_temp(id, (version, first_index, text.clone())));
                    text
                }
            }
        }
        None => Arc::new(json_text(docs, first_index)),
    };
    CodeView::new(text.as_str())
        .id(id.with("view"))
        .label("Documents as JSON")
        .highlighter(&JavaScript)
        .line_numbers(true)
        .folding(FOLDING)
        .revision(version.unwrap_or(0))
        .height(ui.available_height())
        .show(ui);
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
