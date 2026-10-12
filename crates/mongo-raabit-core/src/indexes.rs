//! Index Manager logic: the "New index" form and how existing indexes are described.

use crate::connection::{IndexInfo, NewIndex};
use crate::shell::{self, ShellStyle};

/// The fields of the "New index" form, as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexForm {
    pub keys: String,
    pub name: String,
    pub unique: bool,
    pub sparse: bool,
    pub hidden: bool,
    pub ttl: String,
    pub partial: String,
}

impl IndexForm {
    /// The index to create, or what is wrong with the form.
    pub fn to_new_index(&self) -> Result<NewIndex, String> {
        let keys = shell::parse_document(&self.keys).map_err(|e| format!("keys: {e}"))?;
        if keys.is_empty() {
            return Err("add at least one key, e.g. { email: 1 }".into());
        }
        let partial =
            shell::parse_document(&self.partial).map_err(|e| format!("partial filter: {e}"))?;
        let ttl = if self.ttl.trim().is_empty() {
            None
        } else {
            Some(
                self.ttl
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "TTL must be seconds".to_owned())?,
            )
        };
        Ok(NewIndex {
            keys,
            name: Some(self.name.trim().to_owned()).filter(|n| !n.is_empty()),
            unique: self.unique,
            sparse: self.sparse,
            hidden: self.hidden,
            ttl_seconds: ttl,
            partial_filter: (!partial.is_empty()).then_some(partial),
        })
    }
}

/// The name of the index every collection has; it cannot be hidden or dropped.
pub const PRIMARY_INDEX: &str = "_id_";

/// A property of an index worth showing next to its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexProperty {
    Unique,
    Sparse,
    /// Expires documents after this many seconds.
    Ttl(i64),
    /// Indexes only documents matching this filter (shell text).
    Partial(String),
    Hidden,
    Primary,
}

impl IndexProperty {
    /// Short text for the property (`unique`, `TTL 7d`, `partial { a: 1 }`).
    pub fn label(&self) -> String {
        match self {
            Self::Unique => "unique".into(),
            Self::Sparse => "sparse".into(),
            Self::Ttl(seconds) => format!("TTL {}", ttl_label(*seconds)),
            Self::Partial(filter) => format!("partial {filter}"),
            Self::Hidden => "hidden".into(),
            Self::Primary => "primary".into(),
        }
    }
}

/// The properties of `index` in display order.
pub fn properties(index: &IndexInfo) -> Vec<IndexProperty> {
    let mut found = Vec::new();
    if index.unique {
        found.push(IndexProperty::Unique);
    }
    if index.sparse {
        found.push(IndexProperty::Sparse);
    }
    if let Some(ttl) = index.ttl_seconds {
        found.push(IndexProperty::Ttl(ttl));
    }
    if let Some(filter) = &index.partial_filter {
        found.push(IndexProperty::Partial(shell::document_to_shell(
            filter,
            ShellStyle::Compact,
        )));
    }
    if index.hidden {
        found.push(IndexProperty::Hidden);
    }
    if index.name == PRIMARY_INDEX {
        found.push(IndexProperty::Primary);
    }
    found
}

/// The properties of `index` as one line (`unique · TTL 7d`).
pub fn properties_label(index: &IndexInfo) -> String {
    properties(index)
        .iter()
        .map(IndexProperty::label)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The index keys in compact shell syntax (`{ a: 1, b: -1 }`).
pub fn keys_label(index: &IndexInfo) -> String {
    shell::document_to_shell(&index.keys, ShellStyle::Compact)
}

/// How often the index was used since the server started tracking (`12 ops`, `—`).
pub fn usage_label(ops: Option<u64>) -> String {
    ops.map_or_else(|| "—".into(), |n| format!("{n} ops"))
}

/// Number of indexes and their total size in bytes.
pub fn totals(indexes: &[IndexInfo]) -> (usize, u64) {
    (indexes.len(), indexes.iter().filter_map(|i| i.size).sum())
}

/// A TTL as the largest whole unit (`7d`, `2h`, `90s`).
pub fn ttl_label(seconds: i64) -> String {
    match seconds {
        s if s % 86_400 == 0 && s >= 86_400 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 && s >= 3600 => format!("{}h", s / 3600),
        s => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn index_form_validation() {
        let mut form = IndexForm {
            keys: "{ email: 1, 'address.city': -1 }".into(),
            ttl: "3600".into(),
            partial: "{ active: true }".into(),
            ..IndexForm::default()
        };
        let index = form.to_new_index().unwrap();
        assert_eq!(index.keys.len(), 2);
        assert_eq!(index.ttl_seconds, Some(3600));
        assert!(index.partial_filter.is_some());
        form.keys = "{}".into();
        assert!(form.to_new_index().is_err());
        form.keys = "{a: 1}".into();
        form.ttl = "soon".into();
        assert!(form.to_new_index().is_err());
    }

    #[test]
    fn form_names_and_flags_carry_over() {
        let form = IndexForm {
            keys: "{a: 1}".into(),
            name: "  by_a ".into(),
            unique: true,
            hidden: true,
            ..IndexForm::default()
        };
        let index = form.to_new_index().unwrap();
        assert_eq!(index.name.as_deref(), Some("by_a"));
        assert!(index.unique && index.hidden && !index.sparse);
        assert_eq!(index.ttl_seconds, None);
        assert_eq!(index.partial_filter, None);
        let unnamed = IndexForm {
            keys: "{a: 1}".into(),
            ..IndexForm::default()
        };
        assert_eq!(unnamed.to_new_index().unwrap().name, None);
    }

    fn info(name: &str) -> IndexInfo {
        IndexInfo {
            name: name.into(),
            keys: doc! {"a": 1},
            unique: false,
            sparse: false,
            hidden: false,
            ttl_seconds: None,
            partial_filter: None,
            size: None,
            ops: None,
            raw: doc! {},
        }
    }

    #[test]
    fn properties_follow_display_order() {
        let mut index = info("by_a");
        assert!(properties(&index).is_empty());
        index.unique = true;
        index.ttl_seconds = Some(7 * 86_400);
        index.partial_filter = Some(doc! {"active": true});
        index.hidden = true;
        let labels: Vec<String> = properties(&index)
            .iter()
            .map(IndexProperty::label)
            .collect();
        assert_eq!(
            labels,
            ["unique", "TTL 7d", "partial { active: true }", "hidden"]
        );
        assert_eq!(properties(&info(PRIMARY_INDEX)), [IndexProperty::Primary]);
        assert_eq!(
            properties_label(&index),
            "unique · TTL 7d · partial { active: true } · hidden"
        );
        assert_eq!(properties_label(&info("plain")), "");
        assert_eq!(keys_label(&index), "{ a: 1 }");
    }

    #[test]
    fn labels_and_totals() {
        assert_eq!(ttl_label(86_400), "1d");
        assert_eq!(ttl_label(7200), "2h");
        assert_eq!(ttl_label(90), "90s");
        assert_eq!(ttl_label(3601), "3601s");
        assert_eq!(usage_label(Some(3)), "3 ops");
        assert_eq!(usage_label(None), "—");
        let mut a = info("a");
        a.size = Some(10);
        let mut b = info("b");
        b.size = Some(5);
        assert_eq!(totals(&[a, b, info("c")]), (3, 15));
    }
}
