//! Result documents as the views show them: previews, table rows and columns, the tree
//! flattened to visible rows, the JSON text and the sort text of a header click.

use crate::shell::{self, ShellStyle};
use mongodb::bson::{Bson, Document};
use std::collections::HashSet;

const MAX_PREVIEW: usize = 160;

/// Single-line preview for table cells and tree rows.
pub fn preview(value: &Bson) -> String {
    let text = match value {
        Bson::String(s) => s.replace(['\n', '\r'], " "),
        Bson::Document(d) => match d.len() {
            1 => "{ 1 field }".into(),
            n => format!("{{ {n} fields }}"),
        },
        Bson::Array(a) => format!("[ {} ]", a.len()),
        Bson::ObjectId(o) => o.to_hex(),
        Bson::DateTime(d) => d
            .try_to_rfc3339_string()
            .map(|s| {
                s.replace('T', " ")
                    .trim_end_matches('Z')
                    .trim_end_matches(".000")
                    .to_owned()
            })
            .unwrap_or_else(|_| d.timestamp_millis().to_string()),
        Bson::Double(v) => {
            if v.fract() == 0.0 && v.abs() < 1e15 {
                format!("{v:.1}")
            } else {
                v.to_string()
            }
        }
        Bson::Int32(v) => v.to_string(),
        Bson::Int64(v) => v.to_string(),
        Bson::Decimal128(d) => d.to_string(),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "null".into(),
        other => shell::to_shell_string(other, ShellStyle::Compact),
    };
    truncate(&text, MAX_PREVIEW)
}

/// `text` cut to `max` characters, with an ellipsis when it was longer.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max).collect();
        out.push('…');
        out
    }
}

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

/// The value of column `name` in a table row (`value` is the column of scalar rows).
pub fn cell_value<'a>(row: &'a Bson, name: &str) -> Option<&'a Bson> {
    match row {
        Bson::Document(d) => d.get(name),
        other if name == "value" => Some(other),
        _ => None,
    }
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

/// `name` below `prefix` as a dotted path.
pub fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

/// Fields that `$set` can reach by a dotted path.
pub fn settable(segment: &str) -> bool {
    !segment.is_empty() && !segment.contains('.') && !segment.starts_with('$')
}

/// The text an inline edit starts from. Only scalars whose cell shows exactly that text
/// are edited in place (dates and the like go through the document editor).
pub fn inline_text(value: &Bson) -> Option<String> {
    match value {
        Bson::String(s) if preview(value) == *s => Some(s.clone()),
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Boolean(_) | Bson::Null => {
            Some(preview(value))
        }
        _ => None,
    }
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

/// One visible row of the tree view.
#[derive(Debug, Clone)]
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

/// Flatten the open part of the tree. `first_index` numbers the first document.
pub fn tree_rows(docs: &[Document], open: &HashSet<String>, first_index: u64) -> Vec<TreeRow> {
    struct Walk<'a> {
        out: Vec<TreeRow>,
        open: &'a HashSet<String>,
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
            Some(id) => preview(id),
            None => doc
                .iter()
                .next()
                .map(|(k, v)| format!("{k}: {}", preview(v)))
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

/// The documents as shell text, one `// n` marker per document.
pub fn json_text(docs: &[Document], first_index: u64) -> String {
    let mut text = String::new();
    for (i, doc) in docs.iter().enumerate() {
        text.push_str(&format!("// {}\n", first_index + i as u64));
        text.push_str(&shell::document_to_shell(doc, ShellStyle::Pretty(2)));
        text.push_str("\n\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{DateTime, doc};

    #[test]
    fn previews_are_compact() {
        assert_eq!(
            preview(&Bson::Document(doc! {"a": 1, "b": 2})),
            "{ 2 fields }"
        );
        assert_eq!(preview(&Bson::Array(vec![Bson::Int32(1)])), "[ 1 ]");
        assert_eq!(
            preview(&Bson::DateTime(DateTime::from_millis(0))),
            "1970-01-01 00:00:00"
        );
        assert_eq!(preview(&Bson::String("a\nb".into())), "a b");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(preview(&Bson::Double(2.0)), "2.0");
    }

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
        assert_eq!(
            cell_value(&tags[0].1, "value"),
            Some(&Bson::String("x".into()))
        );
        assert_eq!(cell_value(&tags[0].1, "other"), None);
    }

    #[test]
    fn tree_rows_follow_open_set() {
        let docs = vec![doc! {"_id": 1, "a": {"b": [1, 2]}}];
        let mut open = HashSet::new();
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

    #[test]
    fn edit_paths_and_texts() {
        let doc = doc! {"a": {"b": 1}, "l": [1]};
        assert_eq!(editable_prefix(&doc, &[]), Some(String::new()));
        assert_eq!(editable_prefix(&doc, &["a".into()]), Some("a".to_string()));
        assert_eq!(editable_prefix(&doc, &["l".into()]), None);
        assert_eq!(join_path("", "x"), "x");
        assert_eq!(join_path("a", "x"), "a.x");
        assert!(settable("name") && !settable("a.b") && !settable("$x") && !settable(""));
        assert_eq!(inline_text(&Bson::Int32(3)), Some("3".into()));
        assert_eq!(inline_text(&Bson::String("a".into())), Some("a".into()));
        assert_eq!(inline_text(&Bson::String("a\nb".into())), None);
        assert_eq!(inline_text(&Bson::DateTime(DateTime::from_millis(0))), None);
    }

    #[test]
    fn sort_text_quotes_odd_names() {
        assert_eq!(sort_text("name", false), "{ name: 1 }");
        assert_eq!(sort_text("a.b", true), "{ \"a.b\": -1 }");
        assert_eq!(sort_text("1x", false), "{ \"1x\": 1 }");
    }

    #[test]
    fn json_text_marks_documents() {
        let text = json_text(&[doc! {"a": 1}, doc! {"a": 2}], 5);
        assert!(text.starts_with("// 5\n"));
        assert!(text.contains("// 6\n"));
    }
}
