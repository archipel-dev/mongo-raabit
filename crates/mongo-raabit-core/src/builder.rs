//! Headless model of the visual query builder: condition groups, sort, projection, skip
//! and limit, turned into the shell query text of the query bar and back.
//!
//! Values are kept as the text the user typed together with a [`ValueType`]; they are
//! converted to BSON when the query text is generated, so an invalid value only disables
//! its own condition and reports an error for it instead of failing the whole query.

use crate::query::QueryText;
use crate::shell::{self, ShellStyle};
use mongodb::bson::{Bson, Document, Regex, oid::ObjectId};

/// How the items of a group combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatchMode {
    /// Every item must match (`$and`).
    #[default]
    All,
    /// At least one item must match (`$or`).
    Any,
}

impl MatchMode {
    pub const ALL: [Self; 2] = [Self::All, Self::Any];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Any => "Any",
        }
    }
}

/// Comparison of one condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Operator {
    #[default]
    Equals,
    NotEquals,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
    Exists,
    NotExists,
    IsNull,
    TypeIs,
    SizeIs,
    Between,
}

/// How many value fields an operator needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operands {
    None,
    One,
    /// A comma separated list in one field.
    List,
    Two,
    /// A BSON type picked from [`TypeName`].
    Type,
}

impl Operator {
    pub const ALL: [Self; 17] = [
        Self::Equals,
        Self::NotEquals,
        Self::Greater,
        Self::GreaterOrEqual,
        Self::Less,
        Self::LessOrEqual,
        Self::In,
        Self::NotIn,
        Self::Contains,
        Self::StartsWith,
        Self::EndsWith,
        Self::Exists,
        Self::NotExists,
        Self::IsNull,
        Self::TypeIs,
        Self::SizeIs,
        Self::Between,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Equals => "equals",
            Self::NotEquals => "not equals",
            Self::Greater => "greater than",
            Self::GreaterOrEqual => "≥ (at least)",
            Self::Less => "less than",
            Self::LessOrEqual => "≤ (at most)",
            Self::In => "in list",
            Self::NotIn => "not in list",
            Self::Contains => "contains",
            Self::StartsWith => "starts with",
            Self::EndsWith => "ends with",
            Self::Exists => "exists",
            Self::NotExists => "does not exist",
            Self::IsNull => "is null",
            Self::TypeIs => "type is",
            Self::SizeIs => "array size is",
            Self::Between => "between",
        }
    }

    pub fn operands(self) -> Operands {
        match self {
            Self::Exists | Self::NotExists | Self::IsNull => Operands::None,
            Self::In | Self::NotIn => Operands::List,
            Self::Between => Operands::Two,
            Self::TypeIs => Operands::Type,
            _ => Operands::One,
        }
    }

    /// Text operators match with a regular expression and ignore the value type.
    pub fn is_text(self) -> bool {
        matches!(self, Self::Contains | Self::StartsWith | Self::EndsWith)
    }

    /// Whether the value type picker applies to this operator.
    pub fn uses_value_type(self) -> bool {
        matches!(
            self.operands(),
            Operands::One | Operands::List | Operands::Two
        ) && !self.is_text()
            && self != Self::SizeIs
    }
}

/// How a typed value is converted to BSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ValueType {
    /// Inferred from the text (see [`infer`]).
    #[default]
    Auto,
    String,
    Number,
    Boolean,
    Date,
    ObjectId,
    Null,
}

impl ValueType {
    pub const ALL: [Self; 7] = [
        Self::Auto,
        Self::String,
        Self::Number,
        Self::Boolean,
        Self::Date,
        Self::ObjectId,
        Self::Null,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::String => "String",
            Self::Number => "Number",
            Self::Boolean => "Boolean",
            Self::Date => "Date",
            Self::ObjectId => "ObjectId",
            Self::Null => "Null",
        }
    }
}

/// BSON type aliases offered by the "type is" operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TypeName {
    #[default]
    String,
    Number,
    Bool,
    Date,
    ObjectId,
    Array,
    Object,
}

impl TypeName {
    pub const ALL: [Self; 7] = [
        Self::String,
        Self::Number,
        Self::Bool,
        Self::Date,
        Self::ObjectId,
        Self::Array,
        Self::Object,
    ];

    /// The `$type` alias.
    pub fn alias(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::Date => "date",
            Self::ObjectId => "objectId",
            Self::Array => "array",
            Self::Object => "object",
        }
    }

    fn from_alias(alias: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.alias() == alias)
    }
}

/// One `field operator value` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Condition {
    /// Dotted field path.
    pub field: String,
    pub operator: Operator,
    pub value: String,
    /// Upper bound of `between`.
    pub value2: String,
    pub value_type: ValueType,
    pub type_name: TypeName,
    /// Text operators: ignore case (`i` regex option).
    pub ignore_case: bool,
    /// Set on a condition made for a dropped field ([`Condition::for_field`]) until its
    /// value is edited: while set, an empty value is incomplete instead of the empty string
    /// (so dropping a text field does not filter on `""`). Text read back from a query never
    /// sets it.
    pub awaiting_value: bool,
}

/// A condition or a nested group.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Condition(Condition),
    Group(Group),
}

/// Items combined with All / Any.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Group {
    pub mode: MatchMode,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SortKey {
    pub field: String,
    pub descending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectionMode {
    /// Return only the listed fields.
    #[default]
    Include,
    /// Return everything but the listed fields.
    Exclude,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Projection {
    pub mode: ProjectionMode,
    pub fields: Vec<String>,
    pub exclude_id: bool,
}

/// The whole builder state of one query.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Builder {
    pub filter: Group,
    pub sort: Vec<SortKey>,
    pub projection: Projection,
    pub skip: u64,
    /// 0 means no limit.
    pub limit: u64,
}

/// What a field holds, as far as the query builder cares: it picks the operator and the
/// value type of a condition added for the field (see [`Condition::for_field`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum FieldType {
    String,
    Number,
    Boolean,
    Date,
    ObjectId,
    Array,
    Object,
    /// Only `null` values were seen.
    Null,
    /// Several kinds of values, or a kind the builder has no editor for.
    #[default]
    Mixed,
}

impl FieldType {
    pub const ALL: [Self; 9] = [
        Self::String,
        Self::Number,
        Self::Boolean,
        Self::Date,
        Self::ObjectId,
        Self::Array,
        Self::Object,
        Self::Null,
        Self::Mixed,
    ];

    /// The kind of a BSON type name as [`shell::type_name`] writes it (`Int32`, `Object`…).
    pub fn from_type_name(name: &str) -> Self {
        match name {
            "String" | "Symbol" => Self::String,
            "Int32" | "Int64" | "Double" | "Decimal128" => Self::Number,
            "Boolean" => Self::Boolean,
            "Date" | "Timestamp" => Self::Date,
            "ObjectId" => Self::ObjectId,
            "Array" => Self::Array,
            "Object" => Self::Object,
            "Null" | "Undefined" => Self::Null,
            _ => Self::Mixed,
        }
    }

    /// The kind of a field whose values had these BSON type names: their common kind
    /// (nulls aside), `Null` when only nulls were seen, `Mixed` otherwise.
    pub fn of_type_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut kind = None;
        let mut nulls = false;
        for name in names {
            match Self::from_type_name(name) {
                Self::Null => nulls = true,
                k if kind.is_none() => kind = Some(k),
                k if kind == Some(k) => {}
                _ => return Self::Mixed,
            }
        }
        match (kind, nulls) {
            (Some(kind), _) => kind,
            (None, true) => Self::Null,
            (None, false) => Self::Mixed,
        }
    }

    /// Short name for the type badge of a field row.
    pub fn label(self) -> &'static str {
        match self {
            Self::String => "String",
            Self::Number => "Number",
            Self::Boolean => "Boolean",
            Self::Date => "Date",
            Self::ObjectId => "ObjectId",
            Self::Array => "Array",
            Self::Object => "Object",
            Self::Null => "Null",
            Self::Mixed => "Mixed",
        }
    }
}

/// Infers the BSON value of `text` typed for `field`: numbers, `true`/`false`, ObjectIds
/// (24 hex digits on `_id` or `…Id` / `…_id` fields), ISO dates, otherwise a string.
pub fn infer(field: &str, text: &str) -> Bson {
    let trimmed = text.trim();
    if let Some(number) = parse_number(trimmed) {
        return number;
    }
    match trimmed {
        "true" => return Bson::Boolean(true),
        "false" => return Bson::Boolean(false),
        _ => {}
    }
    let last = field.rsplit('.').next().unwrap_or(field);
    let id_field = last == "_id" || last.ends_with("Id") || last.ends_with("_id");
    if id_field
        && trimmed.len() == 24
        && let Ok(oid) = ObjectId::parse_str(trimmed)
    {
        return Bson::ObjectId(oid);
    }
    if looks_like_date(trimmed)
        && let Ok(date) = shell::parse_date(trimmed)
    {
        return Bson::DateTime(date);
    }
    Bson::String(text.to_owned())
}

fn looks_like_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

/// Integers become Int32 (Int64 when large), anything with a fraction or exponent Double.
fn parse_number(text: &str) -> Option<Bson> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    if let Ok(v) = text.parse::<i64>() {
        return Some(match i32::try_from(v) {
            Ok(small) => Bson::Int32(small),
            Err(_) => Bson::Int64(v),
        });
    }
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(Bson::Double)
}

/// Converts one typed value.
pub fn convert(field: &str, text: &str, value_type: ValueType) -> Result<Bson, String> {
    let trimmed = text.trim();
    match value_type {
        ValueType::Null => Ok(Bson::Null),
        ValueType::String => Ok(Bson::String(text.to_owned())),
        _ if trimmed.is_empty() => Err("Enter a value".into()),
        ValueType::Auto => Ok(infer(field, text)),
        ValueType::Number => {
            parse_number(trimmed).ok_or_else(|| format!("'{trimmed}' is not a number"))
        }
        ValueType::Boolean => match trimmed {
            "true" => Ok(Bson::Boolean(true)),
            "false" => Ok(Bson::Boolean(false)),
            _ => Err(format!("'{trimmed}' is not true or false")),
        },
        ValueType::Date => shell::parse_date(trimmed)
            .map(Bson::DateTime)
            .map_err(|_| format!("'{trimmed}' is not a date (yyyy-mm-dd or ISO-8601)")),
        ValueType::ObjectId => ObjectId::parse_str(trimmed)
            .map(Bson::ObjectId)
            .map_err(|_| format!("'{trimmed}' is not an ObjectId (24 hex digits)")),
    }
}

/// A regex value with sorted options, as the server stores it.
fn regex(pattern: impl Into<String>, options: impl Into<String>) -> Regex {
    let mut options: Vec<char> = options.into().chars().collect();
    options.sort_unstable();
    Regex {
        pattern: pattern.into(),
        options: options.into_iter().collect(),
    }
}

/// Escapes regular expression metacharacters so `text` matches literally.
fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Inverse of [`escape_regex`]; `None` when the pattern uses real regex syntax.
fn unescape_regex(pattern: &str) -> Option<String> {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let next = chars.next()?;
                if !"\\.+*?()|[]{}^$/".contains(next) {
                    return None;
                }
                out.push(next);
            }
            c if ".+*?()|[]{}^$".contains(c) => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

impl Condition {
    pub fn new(field: &str, operator: Operator, value: &str) -> Self {
        Self {
            field: field.to_owned(),
            operator,
            value: value.to_owned(),
            ..Self::default()
        }
    }

    /// The condition a field dropped on the filter starts with: the operator and value type
    /// that suit what the field holds. Booleans start complete (`true`), so do the presence
    /// checks of objects and null-only fields; the others wait for a value.
    pub fn for_field(field: &str, kind: FieldType) -> Self {
        let (operator, value_type, value) = match kind {
            FieldType::String => (Operator::Equals, ValueType::String, ""),
            FieldType::Number => (Operator::Equals, ValueType::Number, ""),
            FieldType::Boolean => (Operator::Equals, ValueType::Boolean, "true"),
            FieldType::Date => (Operator::GreaterOrEqual, ValueType::Date, ""),
            FieldType::ObjectId => (Operator::Equals, ValueType::ObjectId, ""),
            FieldType::Array => (Operator::In, ValueType::Auto, ""),
            FieldType::Object => (Operator::Exists, ValueType::Auto, ""),
            FieldType::Null => (Operator::IsNull, ValueType::Auto, ""),
            FieldType::Mixed => (Operator::Equals, ValueType::Auto, ""),
        };
        Self {
            value_type,
            awaiting_value: value.is_empty(),
            ..Self::new(field, operator, value)
        }
    }

    /// The condition as a filter document `{field: …}`, or why it cannot be one.
    pub fn to_filter(&self) -> Result<Document, String> {
        let field = self.field.trim();
        if field.is_empty() {
            return Err("Choose a field".into());
        }
        if field.starts_with('$') {
            return Err("Field names cannot start with '$'".into());
        }
        let needs_value = matches!(
            self.operator.operands(),
            Operands::One | Operands::List | Operands::Two
        );
        if self.awaiting_value && needs_value && self.value.is_empty() {
            return Err("Enter a value".into());
        }
        if self.value_type == ValueType::Null
            && matches!(self.operator.operands(), Operands::List | Operands::Two)
        {
            return Err("Null is one value: use \"is null\" or another value type".into());
        }
        let value = |text: &str| convert(field, text, self.value_type);
        let op = |name: &str, v: Bson| -> Bson {
            let mut d = Document::new();
            d.insert(name, v);
            Bson::Document(d)
        };
        let expr = match self.operator {
            Operator::Equals => value(&self.value)?,
            Operator::NotEquals => op("$ne", value(&self.value)?),
            Operator::Greater => op("$gt", value(&self.value)?),
            Operator::GreaterOrEqual => op("$gte", value(&self.value)?),
            Operator::Less => op("$lt", value(&self.value)?),
            Operator::LessOrEqual => op("$lte", value(&self.value)?),
            Operator::In | Operator::NotIn => {
                let items: Vec<&str> = self
                    .value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if items.is_empty() {
                    return Err("Enter one or more values separated by commas".into());
                }
                let list = items
                    .into_iter()
                    .map(value)
                    .collect::<Result<Vec<_>, _>>()?;
                let name = if self.operator == Operator::In {
                    "$in"
                } else {
                    "$nin"
                };
                op(name, Bson::Array(list))
            }
            Operator::Contains | Operator::StartsWith | Operator::EndsWith => {
                if self.value.is_empty() {
                    return Err("Enter the text to look for".into());
                }
                let escaped = escape_regex(&self.value);
                let pattern = match self.operator {
                    Operator::StartsWith => format!("^{escaped}"),
                    Operator::EndsWith => format!("{escaped}$"),
                    _ => escaped,
                };
                let options = if self.ignore_case { "i" } else { "" };
                Bson::RegularExpression(regex(pattern, options))
            }
            Operator::Exists => op("$exists", Bson::Boolean(true)),
            Operator::NotExists => op("$exists", Bson::Boolean(false)),
            Operator::IsNull => Bson::Null,
            Operator::TypeIs => op("$type", Bson::String(self.type_name.alias().into())),
            Operator::SizeIs => {
                let size = self.value.trim();
                let n = size
                    .parse::<u32>()
                    .ok()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| format!("'{size}' is not a non-negative whole number"))?;
                op("$size", Bson::Int32(n))
            }
            Operator::Between => {
                let low = value(&self.value).map_err(|e| format!("From: {e}"))?;
                let high = value(&self.value2).map_err(|e| format!("To: {e}"))?;
                let mut d = Document::new();
                d.insert("$gte", low);
                d.insert("$lte", high);
                Bson::Document(d)
            }
        };
        let mut out = Document::new();
        out.insert(field, expr);
        Ok(out)
    }

    /// The error of this condition, if any.
    pub fn error(&self) -> Option<String> {
        self.to_filter().err()
    }
}

/// A group as it is written: what is left after invalid conditions and empty groups are
/// dropped, single items unwrapped and All groups inside All groups joined.
enum Effective {
    Empty,
    Leaf(Document),
    All(Vec<Effective>),
    Any(Vec<Effective>),
}

impl Effective {
    fn to_document(&self) -> Document {
        match self {
            Self::Empty => Document::new(),
            Self::Leaf(doc) => doc.clone(),
            Self::All(items) => {
                let parts: Vec<Document> = items.iter().map(Self::to_document).collect();
                let mut merged = Document::new();
                for (key, value) in parts.iter().flatten() {
                    if !merge_into(&mut merged, key, value) {
                        let mut d = Document::new();
                        d.insert("$and", parts);
                        return d;
                    }
                }
                merged
            }
            Self::Any(items) => {
                let parts: Vec<Document> = items.iter().map(Self::to_document).collect();
                let mut d = Document::new();
                d.insert("$or", parts);
                d
            }
        }
    }
}

impl Group {
    pub fn new(mode: MatchMode) -> Self {
        Self {
            mode,
            items: Vec::new(),
        }
    }

    /// The group as a filter; invalid conditions and empty groups are left out.
    /// An All group with distinct keys is written as one document, otherwise `$and`.
    ///
    /// The text is canonical, so it reads back ([`Group::from_filter`]) to a group that
    /// writes the same text again: a group with one remaining item is that item, and the
    /// items of an All group inside an All group join their parent's (All of All is All;
    /// reading `$and` puts its clauses into the group that holds it).
    pub fn to_filter(&self) -> Document {
        self.effective().to_document()
    }

    fn effective(&self) -> Effective {
        let mut items = Vec::new();
        for item in &self.items {
            let effective = match item {
                Item::Condition(c) => match c.to_filter() {
                    Ok(doc) => Effective::Leaf(doc),
                    Err(_) => continue,
                },
                Item::Group(g) => g.effective(),
            };
            match (self.mode, effective) {
                (_, Effective::Empty) => {}
                (MatchMode::All, Effective::All(inner)) => items.extend(inner),
                (_, effective) => items.push(effective),
            }
        }
        match (items.len(), self.mode) {
            (0, _) => Effective::Empty,
            (1, _) => items.pop().unwrap_or(Effective::Empty),
            (_, MatchMode::All) => Effective::All(items),
            (_, MatchMode::Any) => Effective::Any(items),
        }
    }

    /// Number of conditions with an error, nested groups included.
    pub fn error_count(&self) -> usize {
        self.items
            .iter()
            .map(|item| match item {
                Item::Condition(c) => usize::from(c.error().is_some()),
                Item::Group(g) => g.error_count(),
            })
            .sum()
    }

    /// Reads a filter document back; `Err` names the first shape the builder cannot show.
    pub fn from_filter(filter: &Document) -> Result<Self, String> {
        let mut group = Self::new(MatchMode::All);
        for (key, value) in filter {
            match key.as_str() {
                "$and" => {
                    for part in clauses(value, "$and")? {
                        group.items.extend(Self::from_filter(part)?.items);
                    }
                }
                "$or" => {
                    let mut any = Self::new(MatchMode::Any);
                    for part in clauses(value, "$or")? {
                        let mut inner = Self::from_filter(part)?;
                        if inner.items.len() == 1 {
                            any.items.extend(inner.items.pop());
                        } else {
                            any.items.push(Item::Group(inner));
                        }
                    }
                    group.items.push(Item::Group(any));
                }
                k if k.starts_with('$') => return Err(format!("operator {k}")),
                field => group.items.extend(
                    field_conditions(field, value)?
                        .into_iter()
                        .map(Item::Condition),
                ),
            }
        }
        Ok(group)
    }
}

/// Adds `key: value` to an All document. Operator documents on the same field merge
/// (`{a: {$gt: 1}}` + `{a: {$lt: 9}}` → `{a: {$gt: 1, $lt: 9}}`); returns false on a
/// conflict that needs `$and`.
fn merge_into(merged: &mut Document, key: &str, value: &Bson) -> bool {
    let operators = |v: &Bson| {
        v.as_document()
            .filter(|d| !d.is_empty() && d.keys().all(|k| k.starts_with('$')))
            .cloned()
    };
    match merged.get_mut(key) {
        None => {
            merged.insert(key, value.clone());
            true
        }
        Some(existing) => match (operators(existing), operators(value)) {
            (Some(mut a), Some(b))
                if !key.starts_with('$') && b.keys().all(|k| !a.contains_key(k)) =>
            {
                a.extend(b);
                *existing = Bson::Document(a);
                true
            }
            _ => false,
        },
    }
}

fn clauses<'a>(value: &'a Bson, name: &str) -> Result<Vec<&'a Document>, String> {
    let Bson::Array(items) = value else {
        return Err(format!("{name} without an array"));
    };
    items
        .iter()
        .map(|item| item.as_document().ok_or_else(|| format!("{name} element")))
        .collect()
}

/// The text and value type that reproduce `value` for `field`, if the builder can show it.
fn value_text(field: &str, value: &Bson) -> Option<(String, ValueType)> {
    let (text, explicit) = match value {
        Bson::String(s) => (s.clone(), ValueType::String),
        Bson::Int32(v) => (v.to_string(), ValueType::Number),
        Bson::Int64(v) => (v.to_string(), ValueType::Number),
        Bson::Double(v) if v.is_finite() => {
            let text = shell::to_shell_string(value, ShellStyle::Compact);
            (
                if *v == 0.0 { "0.0".into() } else { text },
                ValueType::Number,
            )
        }
        Bson::Boolean(v) => (v.to_string(), ValueType::Boolean),
        Bson::ObjectId(oid) => (oid.to_hex(), ValueType::ObjectId),
        Bson::DateTime(date) => (date.try_to_rfc3339_string().ok()?, ValueType::Date),
        Bson::Null => (String::new(), ValueType::Null),
        _ => return None,
    };
    // Auto needs some text to infer from: a blank string stays an explicit String.
    if explicit != ValueType::Null
        && !text.trim().is_empty()
        && same_value(&infer(field, &text), value)
    {
        Some((text, ValueType::Auto))
    } else {
        Some((text, explicit))
    }
}

/// Equality as the query sees it: numbers compare by value.
fn same_value(a: &Bson, b: &Bson) -> bool {
    let num = |v: &Bson| match v {
        Bson::Int32(v) => Some(f64::from(*v)),
        Bson::Int64(v) => Some(*v as f64),
        Bson::Double(v) => Some(*v),
        _ => None,
    };
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Conditions of one `field: value` entry of a filter.
fn field_conditions(field: &str, value: &Bson) -> Result<Vec<Condition>, String> {
    let unsupported = || Err(format!("the condition on '{field}'"));
    let base = |operator: Operator| Condition {
        field: field.to_owned(),
        operator,
        ..Condition::default()
    };
    let with_value = |operator: Operator, v: &Bson| -> Result<Condition, String> {
        let (text, value_type) =
            value_text(field, v).ok_or_else(|| format!("the value of '{field}'"))?;
        Ok(Condition {
            value: text,
            value_type,
            ..base(operator)
        })
    };
    match value {
        Bson::Null => Ok(vec![base(Operator::IsNull)]),
        Bson::RegularExpression(regex) => Ok(vec![regex_condition(field, regex)?]),
        Bson::Document(ops) if ops.keys().all(|k| k.starts_with('$')) && !ops.is_empty() => {
            // A range of one value type is "between"; otherwise its two bounds are read
            // as two conditions below.
            if ops.len() == 2
                && ops.keys().next().map(String::as_str) == Some("$gte")
                && let (Some(low), Some(high)) = (ops.get("$gte"), ops.get("$lte"))
                && let Some((texts, value_type)) = shared_type(field, &[low, high])
            {
                let [low, high]: [String; 2] = texts.try_into().map_err(|_| "range")?;
                let c = Condition {
                    value: low,
                    value2: high,
                    value_type,
                    ..base(Operator::Between)
                };
                return Ok(vec![c]);
            }
            if let (Some(Bson::String(pattern)), true) = (ops.get("$regex"), ops.len() <= 2) {
                let options = match ops.get("$options") {
                    None => "",
                    Some(Bson::String(o)) => o.as_str(),
                    Some(_) => return unsupported(),
                };
                if ops.len() == 2 && !ops.contains_key("$options") {
                    return unsupported();
                }
                return Ok(vec![regex_condition(
                    field,
                    &regex(pattern.clone(), options.to_owned()),
                )?]);
            }
            let mut out = Vec::new();
            for (name, v) in ops {
                let condition = match name.as_str() {
                    "$eq" if *v == Bson::Null => base(Operator::IsNull),
                    "$eq" => with_value(Operator::Equals, v)?,
                    "$ne" => with_value(Operator::NotEquals, v)?,
                    "$gt" => with_value(Operator::Greater, v)?,
                    "$gte" => with_value(Operator::GreaterOrEqual, v)?,
                    "$lt" => with_value(Operator::Less, v)?,
                    "$lte" => with_value(Operator::LessOrEqual, v)?,
                    "$in" | "$nin" => {
                        let operator = if name == "$in" {
                            Operator::In
                        } else {
                            Operator::NotIn
                        };
                        list_condition(base(operator), v)?
                    }
                    "$exists" => match v {
                        Bson::Boolean(true) | Bson::Int32(1) => base(Operator::Exists),
                        Bson::Boolean(false) | Bson::Int32(0) => base(Operator::NotExists),
                        _ => return unsupported(),
                    },
                    "$type" => match v {
                        Bson::String(alias) => Condition {
                            type_name: TypeName::from_alias(alias)
                                .ok_or_else(|| format!("type '{alias}' on '{field}'"))?,
                            ..base(Operator::TypeIs)
                        },
                        _ => return unsupported(),
                    },
                    "$size" => match v {
                        Bson::Int32(n) if *n >= 0 => Condition {
                            value: n.to_string(),
                            ..base(Operator::SizeIs)
                        },
                        Bson::Int64(n) if *n >= 0 && *n <= i64::from(i32::MAX) => Condition {
                            value: n.to_string(),
                            ..base(Operator::SizeIs)
                        },
                        _ => return unsupported(),
                    },
                    other => return Err(format!("operator {other} on '{field}'")),
                };
                out.push(condition);
            }
            Ok(out)
        }
        Bson::Document(_) | Bson::Array(_) => unsupported(),
        scalar => Ok(vec![with_value(Operator::Equals, scalar)?]),
    }
}

/// Texts of `values` with one value type shared by all of them: Auto when every value
/// infers back to itself, otherwise their common explicit type.
fn shared_type(field: &str, values: &[&Bson]) -> Option<(Vec<String>, ValueType)> {
    let mut texts = Vec::new();
    let mut all_auto = true;
    let mut explicit = None;
    for value in values {
        let (text, value_type) = value_text(field, value)?;
        all_auto &= value_type == ValueType::Auto;
        let this = match value {
            Bson::String(_) => ValueType::String,
            Bson::Boolean(_) => ValueType::Boolean,
            Bson::ObjectId(_) => ValueType::ObjectId,
            Bson::DateTime(_) => ValueType::Date,
            Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) => ValueType::Number,
            _ => return None,
        };
        if explicit.is_some_and(|t| t != this) {
            explicit = Some(ValueType::Auto);
        } else if explicit.is_none() {
            explicit = Some(this);
        }
        texts.push(text);
    }
    let value_type = if all_auto {
        ValueType::Auto
    } else {
        explicit.filter(|t| *t != ValueType::Auto)?
    };
    Some((texts, value_type))
}

/// `$in` / `$nin`: every element must reproduce with one shared value type.
fn list_condition(mut condition: Condition, value: &Bson) -> Result<Condition, String> {
    let unsupported = || format!("the list on '{}'", condition.field);
    let Bson::Array(items) = value else {
        return Err(unsupported());
    };
    let items: Vec<&Bson> = items.iter().collect();
    let (texts, value_type) = shared_type(&condition.field, &items).ok_or_else(unsupported)?;
    if texts.is_empty()
        || texts
            .iter()
            .any(|t| t.contains(',') || t.trim().is_empty() || t.trim() != t)
    {
        return Err(unsupported());
    }
    condition.value = texts.join(", ");
    condition.value_type = value_type;
    Ok(condition)
}

fn regex_condition(field: &str, regex: &Regex) -> Result<Condition, String> {
    let unsupported = || format!("the regular expression on '{field}'");
    let ignore_case = match regex.options.as_str() {
        "" => false,
        "i" => true,
        _ => return Err(unsupported()),
    };
    let pattern = regex.pattern.as_str();
    let (operator, body) = if let Some(rest) = pattern.strip_prefix('^') {
        (Operator::StartsWith, rest)
    } else if let Some(rest) = pattern.strip_suffix('$')
        // An anchor unless escaped: an even number of backslashes before it.
        && rest.chars().rev().take_while(|c| *c == '\\').count() % 2 == 0
    {
        (Operator::EndsWith, rest)
    } else {
        (Operator::Contains, pattern)
    };
    let value = unescape_regex(body).filter(|v| !v.is_empty());
    let value = value.ok_or_else(unsupported)?;
    Ok(Condition {
        field: field.to_owned(),
        operator,
        value,
        ignore_case,
        ..Condition::default()
    })
}

impl Builder {
    /// A new group combines with the opposite mode of the root, which is what groups are for.
    pub fn new_group(&self) -> Item {
        let mode = match self.filter.mode {
            MatchMode::All => MatchMode::Any,
            MatchMode::Any => MatchMode::All,
        };
        Item::Group(Group {
            mode,
            items: vec![Item::Condition(Condition::default())],
        })
    }

    /// The query as shell text for the query bar. Empty parts stay empty, so a blank
    /// builder equals `QueryText::default()`.
    pub fn to_query_text(&self) -> QueryText {
        let style = ShellStyle::Compact;
        let filter = self.filter.to_filter();
        let filter = if filter.is_empty() {
            String::new()
        } else {
            shell::document_to_shell(&filter, style)
        };
        let mut sort = Document::new();
        for key in &self.sort {
            let field = key.field.trim();
            if !field.is_empty() {
                sort.insert(field, if key.descending { -1 } else { 1 });
            }
        }
        let mut projection = Document::new();
        let flag = i32::from(self.projection.mode == ProjectionMode::Include);
        for field in &self.projection.fields {
            let field = field.trim();
            if !field.is_empty() && field != "_id" {
                projection.insert(field, flag);
            }
        }
        if self.projection.exclude_id {
            projection.insert("_id", 0);
        }
        let doc_text = |d: &Document| {
            if d.is_empty() {
                String::new()
            } else {
                shell::document_to_shell(d, style)
            }
        };
        let number = |n: u64| if n == 0 { String::new() } else { n.to_string() };
        QueryText {
            filter,
            projection: doc_text(&projection),
            sort: doc_text(&sort),
            skip: number(self.skip),
            limit: number(self.limit),
        }
    }

    /// Reads a query written in Code mode; `Err` says which part the builder cannot show.
    pub fn from_query_text(query: &QueryText) -> Result<Self, String> {
        let parsed = query.parse().map_err(|e| e.to_string())?;
        let filter = Group::from_filter(&parsed.filter)?;
        let mut sort = Vec::new();
        for (field, value) in parsed.sort.iter().flatten() {
            let descending = match value {
                Bson::Int32(1) | Bson::Int64(1) => false,
                Bson::Int32(-1) | Bson::Int64(-1) => true,
                Bson::Double(v) if *v == 1.0 => false,
                Bson::Double(v) if *v == -1.0 => true,
                _ => return Err(format!("the sort on '{field}'")),
            };
            sort.push(SortKey {
                field: field.clone(),
                descending,
            });
        }
        let mut projection = Projection::default();
        let mut include = Vec::new();
        let mut exclude = Vec::new();
        for (field, value) in parsed.projection.iter().flatten() {
            let on = match value {
                Bson::Int32(v) if *v == 0 || *v == 1 => *v == 1,
                Bson::Int64(v) if *v == 0 || *v == 1 => *v == 1,
                Bson::Boolean(v) => *v,
                _ => return Err(format!("the projection of '{field}'")),
            };
            if field == "_id" {
                projection.exclude_id = !on;
            } else if on {
                include.push(field.clone());
            } else {
                exclude.push(field.clone());
            }
        }
        match (include.is_empty(), exclude.is_empty()) {
            (_, true) => projection.fields = include,
            (true, false) => {
                projection.mode = ProjectionMode::Exclude;
                projection.fields = exclude;
            }
            (false, false) => return Err("a projection mixing included and excluded fields".into()),
        }
        Ok(Self {
            filter,
            sort,
            projection,
            skip: parsed.skip,
            limit: parsed.limit,
        })
    }

    /// Number of conditions with an error.
    pub fn error_count(&self) -> usize {
        self.filter.error_count()
    }

    /// Adds a condition on `field` to the top-level filter (a field dropped on Filter).
    /// Returns the index of the new item.
    pub fn add_filter_field(&mut self, field: &str, kind: FieldType) -> usize {
        self.filter
            .items
            .push(Item::Condition(Condition::for_field(field, kind)));
        self.filter.items.len() - 1
    }

    /// Adds `field` to the projection (a field dropped on Projection). `_id` is shown by
    /// default, so adding it hides it. Returns false when it was there already.
    pub fn add_projection_field(&mut self, field: &str) -> bool {
        let field = field.trim();
        if field.is_empty() {
            return false;
        }
        if field == "_id" {
            let added = !self.projection.exclude_id;
            self.projection.exclude_id = true;
            return added;
        }
        if self.projection.fields.iter().any(|f| f.trim() == field) {
            return false;
        }
        self.projection.fields.push(field.to_owned());
        true
    }

    /// Adds `field` to the sort, ascending (a field dropped on Sort). Returns false when
    /// the sort has it already.
    pub fn add_sort_field(&mut self, field: &str) -> bool {
        let field = field.trim();
        if field.is_empty() || self.sort.iter().any(|k| k.field.trim() == field) {
            return false;
        }
        self.sort.push(SortKey {
            field: field.to_owned(),
            descending: false,
        });
        true
    }

    /// Moves sort key `from` to position `to` (sort keys apply in order). Out-of-range
    /// indexes change nothing.
    pub fn move_sort(&mut self, from: usize, to: usize) {
        if from < self.sort.len() && to < self.sort.len() && from != to {
            let key = self.sort.remove(from);
            self.sort.insert(to, key);
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn new_groups_use_the_opposite_mode() {
        let mut builder = Builder::default();
        let Item::Group(group) = builder.new_group() else {
            panic!("a group")
        };
        assert_eq!(group.mode, MatchMode::Any);
        assert_eq!(group.items.len(), 1);
        builder.filter.mode = MatchMode::Any;
        let Item::Group(group) = builder.new_group() else {
            panic!("a group")
        };
        assert_eq!(group.mode, MatchMode::All);
    }
    use super::*;
    use mongodb::bson::{DateTime, doc};

    fn filter_of(conditions: Vec<Condition>, mode: MatchMode) -> Builder {
        Builder {
            filter: Group {
                mode,
                items: conditions.into_iter().map(Item::Condition).collect(),
            },
            ..Builder::default()
        }
    }

    fn one(condition: Condition) -> Builder {
        filter_of(vec![condition], MatchMode::All)
    }

    fn typed(field: &str, operator: Operator, value: &str, value_type: ValueType) -> Condition {
        Condition {
            value_type,
            ..Condition::new(field, operator, value)
        }
    }

    /// Generated text parses, and reading it back gives the same text again.
    fn round_trip(builder: &Builder) -> QueryText {
        let text = builder.to_query_text();
        text.parse()
            .unwrap_or_else(|e| panic!("{text:?} does not parse: {e}"));
        let back = Builder::from_query_text(&text)
            .unwrap_or_else(|e| panic!("{text:?} not taken over: {e}"));
        assert_eq!(back.to_query_text(), text);
        text
    }

    #[test]
    fn dropped_fields_get_an_operator_for_their_type() {
        let mut builder = Builder::default();
        for kind in FieldType::ALL {
            builder.add_filter_field(&format!("f_{}", kind.label()), kind);
        }
        let conditions: Vec<(Operator, ValueType, bool)> = builder
            .filter
            .items
            .iter()
            .map(|item| match item {
                Item::Condition(c) => (c.operator, c.value_type, c.error().is_none()),
                Item::Group(_) => panic!("a condition"),
            })
            .collect();
        assert_eq!(
            conditions,
            [
                (Operator::Equals, ValueType::String, false),
                (Operator::Equals, ValueType::Number, false),
                (Operator::Equals, ValueType::Boolean, true),
                (Operator::GreaterOrEqual, ValueType::Date, false),
                (Operator::Equals, ValueType::ObjectId, false),
                (Operator::In, ValueType::Auto, false),
                (Operator::Exists, ValueType::Auto, true),
                (Operator::IsNull, ValueType::Auto, true),
                (Operator::Equals, ValueType::Auto, false),
            ]
        );
        // Only the complete ones are in the query, in the order they were dropped.
        assert_eq!(
            builder.to_query_text().filter,
            "{ f_Boolean: true, f_Object: { $exists: true }, f_Null: null }"
        );
        // A fresh text condition waits for its value; once edited, "" is a value again.
        let Item::Condition(c) = &mut builder.filter.items[0] else {
            panic!()
        };
        assert!(c.awaiting_value);
        c.awaiting_value = false;
        assert!(c.error().is_none(), "the empty string, on purpose");
        c.value = "Ada".into();
        assert!(
            builder
                .to_query_text()
                .filter
                .starts_with("{ f_String: \"Ada\"")
        );
    }

    /// Cases the property test (`tests/builder_contract.rs`) found: each reads back to the
    /// same text now.
    #[test]
    fn canonical_text_for_cases_that_did_not_read_back() {
        let mut nested = Builder::default();
        nested.filter.items = vec![
            Item::Group(Group {
                mode: MatchMode::Any,
                items: vec![Item::Group(Group {
                    mode: MatchMode::All,
                    items: vec![
                        Item::Condition(Condition::new("a", Operator::Equals, "1")),
                        Item::Condition(Condition::new("b", Operator::IsNull, "")),
                    ],
                })],
            }),
            Item::Condition(Condition::new("a", Operator::Equals, "2")),
        ];
        assert_eq!(
            round_trip(&nested).filter,
            "{ $and: [ { a: 1 }, { b: null }, { a: 2 } ] }"
        );
        let cases = [
            typed("name", Operator::Equals, "", ValueType::String),
            Condition::new("name", Operator::EndsWith, "\\"),
            typed("name", Operator::In, "x", ValueType::Null),
        ];
        assert_eq!(round_trip(&one(cases[0].clone())).filter, "{ name: \"\" }");
        assert_eq!(
            round_trip(&one(cases[1].clone())).filter,
            "{ name: /\\\\$/ }"
        );
        assert!(cases[2].error().is_some(), "a list of nulls is not offered");
        let range = filter_of(
            vec![
                typed("a", Operator::LessOrEqual, "", ValueType::Null),
                typed("a", Operator::GreaterOrEqual, "x", ValueType::String),
            ],
            MatchMode::All,
        );
        assert_eq!(
            round_trip(&range).filter,
            "{ a: { $lte: null, $gte: \"x\" } }"
        );
    }

    #[test]
    fn type_names_map_to_field_types() {
        assert_eq!(
            FieldType::of_type_names(["Int32", "Double"]),
            FieldType::Number
        );
        assert_eq!(
            FieldType::of_type_names(["String", "Null"]),
            FieldType::String
        );
        assert_eq!(FieldType::of_type_names(["Null"]), FieldType::Null);
        assert_eq!(
            FieldType::of_type_names(["String", "Int32"]),
            FieldType::Mixed
        );
        assert_eq!(FieldType::of_type_names([]), FieldType::Mixed);
        assert_eq!(FieldType::of_type_names(["Binary"]), FieldType::Mixed);
        assert_eq!(FieldType::from_type_name("Object"), FieldType::Object);
    }

    #[test]
    fn projection_and_sort_drops_add_each_field_once() {
        let mut builder = Builder::default();
        assert!(builder.add_projection_field("name"));
        assert!(!builder.add_projection_field(" name "));
        assert!(builder.add_projection_field("_id"));
        assert!(!builder.add_projection_field("_id"));
        assert!(!builder.add_projection_field(""));
        assert!(builder.add_sort_field("age"));
        assert!(builder.add_sort_field("name"));
        assert!(!builder.add_sort_field("age"));
        builder.sort[1].descending = true;
        let text = round_trip(&builder);
        assert_eq!(text.projection, "{ name: 1, _id: 0 }");
        assert_eq!(text.sort, "{ age: 1, name: -1 }");
        builder.move_sort(1, 0);
        assert_eq!(builder.to_query_text().sort, "{ name: -1, age: 1 }");
        builder.move_sort(0, 9);
        builder.move_sort(7, 0);
        assert_eq!(builder.to_query_text().sort, "{ name: -1, age: 1 }");
        builder.projection.mode = ProjectionMode::Exclude;
        assert_eq!(round_trip(&builder).projection, "{ name: 0, _id: 0 }");
    }

    #[test]
    fn blank_builder_is_the_default_query() {
        assert_eq!(Builder::default().to_query_text(), QueryText::default());
        assert_eq!(
            Builder::from_query_text(&QueryText::default()).unwrap(),
            Builder::default()
        );
    }

    #[test]
    fn every_operator_round_trips() {
        let oid = "64b7f0c2a1b2c3d4e5f60718";
        let cases: Vec<(Condition, Document)> = vec![
            (
                Condition::new("name", Operator::Equals, "Ada"),
                doc! {"name": "Ada"},
            ),
            (
                Condition::new("age", Operator::NotEquals, "42"),
                doc! {"age": {"$ne": 42}},
            ),
            (
                Condition::new("age", Operator::Greater, "18"),
                doc! {"age": {"$gt": 18}},
            ),
            (
                Condition::new("age", Operator::GreaterOrEqual, "1.5"),
                doc! {"age": {"$gte": 1.5}},
            ),
            (
                Condition::new("age", Operator::Less, "-3"),
                doc! {"age": {"$lt": -3}},
            ),
            (
                Condition::new("age", Operator::LessOrEqual, "9000000000"),
                doc! {"age": {"$lte": 9_000_000_000_i64}},
            ),
            (
                Condition::new("tier", Operator::In, "pro, free ,3"),
                doc! {"tier": {"$in": ["pro", "free", 3]}},
            ),
            (
                typed("tier", Operator::NotIn, "1, 2", ValueType::String),
                doc! {"tier": {"$nin": ["1", "2"]}},
            ),
            (
                Condition::new("name", Operator::Contains, "a.b"),
                doc! {"name": regex("a\\.b", "")},
            ),
            (
                Condition {
                    ignore_case: true,
                    ..Condition::new("name", Operator::StartsWith, "Ad")
                },
                doc! {"name": regex("^Ad", "i")},
            ),
            (
                Condition::new("mail", Operator::EndsWith, ".com"),
                doc! {"mail": regex("\\.com$", "")},
            ),
            (
                Condition::new("x", Operator::Exists, ""),
                doc! {"x": {"$exists": true}},
            ),
            (
                Condition::new("x", Operator::NotExists, ""),
                doc! {"x": {"$exists": false}},
            ),
            (Condition::new("x", Operator::IsNull, ""), doc! {"x": null}),
            (
                Condition {
                    type_name: TypeName::ObjectId,
                    ..Condition::new("ref", Operator::TypeIs, "")
                },
                doc! {"ref": {"$type": "objectId"}},
            ),
            (
                Condition::new("tags", Operator::SizeIs, "3"),
                doc! {"tags": {"$size": 3}},
            ),
            (
                Condition {
                    value2: "20".into(),
                    ..Condition::new("age", Operator::Between, "10")
                },
                doc! {"age": {"$gte": 10, "$lte": 20}},
            ),
            (
                Condition {
                    value2: "2024-12-31".into(),
                    ..typed("at", Operator::Between, "2024-01-01", ValueType::Date)
                },
                doc! {"at": {
                    "$gte": shell::parse_date("2024-01-01").unwrap(),
                    "$lte": shell::parse_date("2024-12-31").unwrap(),
                }},
            ),
            (
                Condition::new("_id", Operator::Equals, oid),
                doc! {"_id": ObjectId::parse_str(oid).unwrap()},
            ),
        ];
        for (condition, expected) in cases {
            let builder = one(condition.clone());
            let text = round_trip(&builder);
            assert_eq!(
                text.parse().unwrap().filter,
                expected,
                "{:?}",
                condition.operator
            );
            let back = Builder::from_query_text(&text).unwrap();
            let Item::Condition(c) = &back.filter.items[0] else {
                panic!("not a condition")
            };
            assert_eq!(c.operator, condition.operator, "{text:?}");
        }
    }

    #[test]
    fn every_operator_and_value_type_generates_parseable_text() {
        for operator in Operator::ALL {
            for value_type in ValueType::ALL {
                for type_name in TypeName::ALL {
                    let c = Condition {
                        value_type,
                        type_name,
                        value2: "9".into(),
                        ..Condition::new("f", operator, "5")
                    };
                    let text = one(c).to_query_text();
                    text.parse()
                        .unwrap_or_else(|e| panic!("{text:?} does not parse: {e}"));
                }
            }
        }
    }

    #[test]
    fn all_and_any_nesting() {
        let any = Group {
            mode: MatchMode::Any,
            items: vec![
                Item::Condition(Condition::new("tier", Operator::Equals, "pro")),
                Item::Condition(Condition::new("age", Operator::Greater, "65")),
            ],
        };
        let builder = Builder {
            filter: Group {
                mode: MatchMode::All,
                items: vec![
                    Item::Condition(Condition::new("active", Operator::Equals, "true")),
                    Item::Group(any),
                ],
            },
            ..Builder::default()
        };
        let text = round_trip(&builder);
        assert_eq!(
            text.filter,
            r#"{ active: true, $or: [ { tier: "pro" }, { age: { $gt: 65 } } ] }"#
        );
        assert_eq!(Builder::from_query_text(&text).unwrap(), builder);

        // Operators on one field merge under All; conflicting ones need $and.
        let range = filter_of(
            vec![
                Condition::new("age", Operator::Greater, "1"),
                Condition::new("age", Operator::Less, "9"),
            ],
            MatchMode::All,
        );
        let text = round_trip(&range);
        assert_eq!(text.filter, "{ age: { $gt: 1, $lt: 9 } }");
        let conflict = filter_of(
            vec![
                Condition::new("age", Operator::Equals, "1"),
                Condition::new("age", Operator::NotEquals, "9"),
                Condition::new("age", Operator::NotEquals, "8"),
            ],
            MatchMode::All,
        );
        let text = round_trip(&conflict);
        assert!(text.filter.starts_with("{ $and: ["), "{}", text.filter);

        // Top-level Any.
        let any = filter_of(
            vec![
                Condition::new("a", Operator::Equals, "1"),
                Condition::new("b", Operator::Equals, "2"),
            ],
            MatchMode::Any,
        );
        let text = round_trip(&any);
        assert_eq!(
            text.parse().unwrap().filter,
            doc! {"$or": [{"a": 1}, {"b": 2}]}
        );
        let back = Builder::from_query_text(&text).unwrap();
        assert_eq!(back.to_query_text(), text);

        // An Any of All groups nests twice.
        let filter = doc! {"$or": [{"a": 1, "b": 2}, {"c": 3}]};
        let group = Group::from_filter(&filter).unwrap();
        assert_eq!(group.to_filter(), filter);
    }

    #[test]
    fn invalid_conditions_are_reported_and_skipped() {
        let builder = filter_of(
            vec![
                typed("age", Operator::Greater, "old", ValueType::Number),
                Condition::new("", Operator::Equals, "x"),
                typed("id", Operator::Equals, "zz", ValueType::ObjectId),
                typed("at", Operator::Equals, "yesterday", ValueType::Date),
                typed("ok", Operator::Equals, "yes", ValueType::Boolean),
                Condition::new("tags", Operator::SizeIs, "-1"),
                Condition::new("tags", Operator::In, " , "),
                Condition::new("name", Operator::Contains, ""),
                Condition::new("$where", Operator::Equals, "1"),
                Condition::new("n", Operator::Between, "1"),
                Condition::new("name", Operator::Equals, "Ada"),
            ],
            MatchMode::All,
        );
        assert_eq!(builder.error_count(), 10);
        assert_eq!(builder.to_query_text().filter, r#"{ name: "Ada" }"#);
        let Item::Condition(c) = &builder.filter.items[0] else {
            unreachable!()
        };
        assert_eq!(c.error().unwrap(), "'old' is not a number");
    }

    #[test]
    fn infers_value_types() {
        assert_eq!(infer("n", "42"), Bson::Int32(42));
        assert_eq!(infer("n", "-2.5"), Bson::Double(-2.5));
        assert_eq!(infer("n", "true"), Bson::Boolean(true));
        assert_eq!(infer("n", "false"), Bson::Boolean(false));
        let hex = "64b7f0c2a1b2c3d4e5f60718";
        let oid = Bson::ObjectId(ObjectId::parse_str(hex).unwrap());
        assert_eq!(infer("_id", hex), oid);
        assert_eq!(infer("userId", hex), oid);
        assert_eq!(infer("order.customer_id", hex), oid);
        assert_eq!(infer("name", hex), Bson::String(hex.into()));
        assert_eq!(
            infer("at", "2024-03-01"),
            Bson::DateTime(DateTime::parse_rfc3339_str("2024-03-01T00:00:00Z").unwrap())
        );
        assert!(matches!(
            infer("at", "2024-03-01T10:20:30Z"),
            Bson::DateTime(_)
        ));
        assert_eq!(infer("n", "Ada"), Bson::String("Ada".into()));
        assert_eq!(infer("n", "1e999"), Bson::String("1e999".into()));
        assert_eq!(infer("n", "NaN"), Bson::String("NaN".into()));
        assert_eq!(infer("n", "2024-13-45"), Bson::String("2024-13-45".into()));
    }

    #[test]
    fn string_values_that_look_like_numbers_keep_their_type() {
        let filter = doc! {"zip": "01234", "n": "42", "flag": "true"};
        let group = Group::from_filter(&filter).unwrap();
        for item in &group.items {
            let Item::Condition(c) = item else {
                unreachable!()
            };
            assert_eq!(c.value_type, ValueType::String, "{}", c.field);
        }
        assert_eq!(group.to_filter(), filter);
    }

    #[test]
    fn takes_over_supported_queries() {
        let query = QueryText {
            filter: r#"{ name: /^Ad/i, age: { $gte: 18, $lte: 65 }, tags: { $in: ["a", "b"] },
                       x: { $exists: true }, kind: { $type: "array" }, n: { $size: 2 },
                       "a.b": { $regex: "x\\.y", $options: "i" }, v: { $gt: 3, $ne: 4 }, w: { $eq: null } }"#
                .into(),
            projection: "{ name: 1, _id: 0 }".into(),
            sort: "{ age: -1, name: 1 }".into(),
            skip: "10".into(),
            limit: "5".into(),
        };
        let builder = Builder::from_query_text(&query).unwrap();
        assert_eq!(builder.filter.items.len(), 10);
        assert_eq!(builder.skip, 10);
        assert_eq!(builder.limit, 5);
        assert_eq!(builder.projection.fields, vec!["name".to_owned()]);
        assert!(builder.projection.exclude_id);
        assert_eq!(builder.sort.len(), 2);
        assert!(builder.sort[0].descending);
        let regenerated = builder.to_query_text();
        assert_eq!(regenerated.parse().unwrap().filter, {
            let mut f = query.parse().unwrap().filter;
            // `$regex` strings come back as regex literals.
            f.insert("a.b", regex("x\\.y", "i"));
            f.insert("w", Bson::Null);
            f
        });
        assert_eq!(regenerated.projection, query.projection);
        assert_eq!(regenerated.sort, query.sort);

        let exclude = QueryText {
            projection: "{ secret: 0 }".into(),
            ..QueryText::default()
        };
        let builder = Builder::from_query_text(&exclude).unwrap();
        assert_eq!(builder.projection.mode, ProjectionMode::Exclude);
        assert_eq!(builder.to_query_text(), exclude);
    }

    #[test]
    fn rejects_queries_the_builder_cannot_show() {
        for filter in [
            "{ a: { b: 1 } }",
            "{ a: [1, 2] }",
            "{ $nor: [{ a: 1 }] }",
            "{ $expr: { $gt: ['$a', '$b'] } }",
            "{ a: { $not: { $gt: 1 } } }",
            "{ a: { $elemMatch: { b: 1 } } }",
            "{ a: /x.*y/ }",
            "{ a: /x/m }",
            "{ a: NumberDecimal('1.5') }",
            "{ a: { $type: 'decimal' } }",
            "{ a: { $in: ['x, y'] } }",
            "{ a: { $in: [] } }",
            "{ a: { $in: [1, '2'] } }",
            "{ $or: { a: 1 } }",
            "{ a: { $size: -1 } }",
        ] {
            let query = QueryText {
                filter: filter.into(),
                ..QueryText::default()
            };
            assert!(
                Builder::from_query_text(&query).is_err(),
                "{filter} should be rejected"
            );
        }
        for (projection, sort) in [
            ("{ a: { $slice: 2 } }", ""),
            ("{ a: 1, b: 0 }", ""),
            ("", "{ a: 2 }"),
            ("", "{ score: { $meta: 'textScore' } }"),
        ] {
            let query = QueryText {
                projection: projection.into(),
                sort: sort.into(),
                ..QueryText::default()
            };
            assert!(Builder::from_query_text(&query).is_err(), "{query:?}");
        }
        let broken = QueryText {
            filter: "{ a: ".into(),
            ..QueryText::default()
        };
        assert!(Builder::from_query_text(&broken).is_err());
    }

    #[test]
    fn mixed_explicit_types_in_lists() {
        let oid = ObjectId::new();
        let filter = doc! {"ref": {"$in": [oid, ObjectId::new()]}};
        let group = Group::from_filter(&filter).unwrap();
        let Item::Condition(c) = &group.items[0] else {
            unreachable!()
        };
        assert_eq!(c.value_type, ValueType::ObjectId);
        assert_eq!(group.to_filter(), filter);
    }

    #[test]
    fn text_operators_escape_and_unescape() {
        let tricky = "a/b (c)? $5 [x] \\ ^";
        for operator in [Operator::Contains, Operator::StartsWith, Operator::EndsWith] {
            let builder = one(Condition::new("t", operator, tricky));
            let text = round_trip(&builder);
            let back = Builder::from_query_text(&text).unwrap();
            let Item::Condition(c) = &back.filter.items[0] else {
                unreachable!()
            };
            assert_eq!(c.value, tricky, "{text:?}");
            assert_eq!(c.operator, operator);
        }
    }

    #[test]
    fn sort_projection_skip_limit() {
        let builder = Builder {
            sort: vec![
                SortKey {
                    field: "age".into(),
                    descending: true,
                },
                SortKey::default(),
            ],
            projection: Projection {
                mode: ProjectionMode::Include,
                fields: vec!["name".into(), " ".into()],
                exclude_id: true,
            },
            skip: 20,
            limit: 10,
            ..Builder::default()
        };
        let text = round_trip(&builder);
        assert_eq!(text.sort, "{ age: -1 }");
        assert_eq!(text.projection, "{ name: 1, _id: 0 }");
        assert_eq!(text.skip, "20");
        assert_eq!(text.limit, "10");
        let query = text.parse().unwrap();
        assert_eq!(query.skip, 20);
        assert_eq!(query.limit, 10);
    }
}
