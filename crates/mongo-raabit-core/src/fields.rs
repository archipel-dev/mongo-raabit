//! Field names for pickers: the paths seen in sample documents and their filtering.

use mongodb::bson::{Bson, Document};
use std::collections::BTreeSet;

/// Field paths offered in the query builder's pickers.
pub const MAX_FIELDS: usize = 200;
/// Nesting depth walked by [`field_names`].
const MAX_FIELD_DEPTH: usize = 8;

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

/// The names to list for what is typed in a picker: those containing the text (ignoring
/// case), or all of them while the text is empty or already one of the names.
pub fn filter_names<'a>(names: &'a [String], typed: &str) -> Vec<&'a String> {
    let needle = typed.trim().to_lowercase();
    let filtered = !needle.is_empty() && !names.iter().any(|n| n == typed);
    names
        .iter()
        .filter(|name| !filtered || name.to_lowercase().contains(&needle))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

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
    fn field_paths_keep_first_seen_order_and_depth() {
        let docs = [
            doc! {"b": 1, "a": {"c": {"d": {"e": {"f": 1}}}}},
            doc! {"z": 1, "b": 2},
        ];
        assert_eq!(field_paths(&docs), ["b", "a", "a.c", "a.c.d", "z"]);
    }

    #[test]
    fn filters_names_by_what_is_typed() {
        let names: Vec<String> = ["_id", "name", "address.Name"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(filter_names(&names, "").len(), 3);
        assert_eq!(filter_names(&names, "  ").len(), 3);
        let hits = filter_names(&names, "NAM");
        assert_eq!(hits, [&names[1], &names[2]]);
        // A complete name lists everything, to switch to another one.
        assert_eq!(filter_names(&names, "name").len(), 3);
        assert!(filter_names(&names, "zzz").is_empty());
    }
}
