//! Visual forms for the common aggregation stages.
//!
//! A [`StageForm`] is a structured view of a stage body: rows of fields, operators and
//! values that a non-programmer can edit. Forms convert to the stage body text (mongo
//! shell syntax, see [`crate::shell`]) and back. Parsing back only succeeds for bodies of
//! the simple shapes a form can show; anything else stays in the code editor.

use crate::shell::{self, ShellStyle};
use mongodb::bson::{Bson, Document, Regex};
use std::fmt;

/// Operators that have a visual form.
pub const FORM_OPERATORS: &[&str] = &[
    "$match",
    "$group",
    "$sort",
    "$project",
    "$limit",
    "$skip",
    "$count",
    "$unwind",
    "$lookup",
    "$addFields",
    "$set",
];

/// Whether `operator` has a visual form.
pub fn has_form(operator: &str) -> bool {
    FORM_OPERATORS.contains(&operator)
}

/// A validation problem of a form, phrased for the person editing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormError(pub String);

impl fmt::Display for FormError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormError {}

fn err<T>(message: impl Into<String>) -> Result<T, FormError> {
    Err(FormError(message.into()))
}

/// Structured form of one stage body.
#[derive(Debug, Clone, PartialEq)]
pub enum StageForm {
    Match(MatchForm),
    Group(GroupForm),
    Sort(SortForm),
    Project(ProjectForm),
    Limit(u64),
    Skip(u64),
    /// `$count`: name of the output field.
    Count(String),
    Unwind(UnwindForm),
    Lookup(LookupForm),
    /// `$addFields` and its alias `$set`.
    AddFields(AddFieldsForm),
}

impl StageForm {
    /// An empty form for `operator`, if it has one.
    pub fn empty(operator: &str) -> Option<Self> {
        Some(match operator {
            "$match" => Self::Match(MatchForm::default()),
            "$group" => Self::Group(GroupForm::default()),
            "$sort" => Self::Sort(SortForm::default()),
            "$project" => Self::Project(ProjectForm::default()),
            "$limit" => Self::Limit(10),
            "$skip" => Self::Skip(10),
            "$count" => Self::Count("count".into()),
            "$unwind" => Self::Unwind(UnwindForm::default()),
            "$lookup" => Self::Lookup(LookupForm::default()),
            "$addFields" | "$set" => Self::AddFields(AddFieldsForm::default()),
            _ => return None,
        })
    }

    /// Read a stage body into a form. `None` when the operator has no form, the body
    /// does not parse, or it uses features the form cannot show.
    pub fn parse(operator: &str, body: &str) -> Option<Self> {
        if !has_form(operator) {
            return None;
        }
        let value = shell::parse_value(body).ok()?;
        Self::from_bson(operator, &value)
    }

    /// Read a parsed stage body into a form.
    pub fn from_bson(operator: &str, value: &Bson) -> Option<Self> {
        Some(match operator {
            "$match" => Self::Match(MatchForm::from_bson(value)?),
            "$group" => Self::Group(GroupForm::from_bson(value)?),
            "$sort" => Self::Sort(SortForm::from_bson(value)?),
            "$project" => Self::Project(ProjectForm::from_bson(value)?),
            "$limit" => Self::Limit(whole_number(value)?),
            "$skip" => Self::Skip(whole_number(value)?),
            "$count" => match value {
                Bson::String(name) => Self::Count(name.clone()),
                _ => return None,
            },
            "$unwind" => Self::Unwind(UnwindForm::from_bson(value)?),
            "$lookup" => Self::Lookup(LookupForm::from_bson(value)?),
            "$addFields" | "$set" => Self::AddFields(AddFieldsForm::from_bson(value)?),
            _ => return None,
        })
    }

    /// The stage body as a BSON value, or the first problem that prevents it.
    pub fn to_bson(&self) -> Result<Bson, FormError> {
        match self {
            Self::Match(form) => form.to_bson(),
            Self::Group(form) => form.to_bson(),
            Self::Sort(form) => form.to_bson(),
            Self::Project(form) => form.to_bson(),
            Self::Limit(n) => {
                if *n == 0 {
                    return err("Limit must be at least 1");
                }
                Ok(integer(*n))
            }
            Self::Skip(n) => Ok(integer(*n)),
            Self::Count(name) => {
                check_output_name(name, "The count field")?;
                Ok(Bson::String(name.trim().to_owned()))
            }
            Self::Unwind(form) => form.to_bson(),
            Self::Lookup(form) => form.to_bson(),
            Self::AddFields(form) => form.to_bson(),
        }
    }

    /// The stage body in shell syntax, as stored in [`crate::aggregate::Stage::body`].
    pub fn to_body(&self) -> Result<String, FormError> {
        self.to_bson()
            .map(|value| shell::to_shell_string(&value, ShellStyle::Pretty(2)))
    }
}

// ----- values -----------------------------------------------------------------------------

fn integer(n: u64) -> Bson {
    match i32::try_from(n) {
        Ok(small) => Bson::Int32(small),
        Err(_) => Bson::Int64(i64::try_from(n).unwrap_or(i64::MAX)),
    }
}

fn whole_number(value: &Bson) -> Option<u64> {
    match value {
        Bson::Int32(v) => u64::try_from(*v).ok(),
        Bson::Int64(v) => u64::try_from(*v).ok(),
        Bson::Double(v) if v.fract() == 0.0 && *v >= 0.0 && *v < 9.0e15 => Some(*v as u64),
        _ => None,
    }
}

const CONSTRUCTORS: &[&str] = &[
    "ObjectId(",
    "ISODate(",
    "Date(",
    "NumberLong(",
    "NumberInt(",
    "NumberDecimal(",
    "UUID(",
    "Timestamp(",
];

fn looks_like_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 10 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[7] == b'-'
}

fn looks_numeric(text: &str) -> bool {
    text.bytes().any(|c| c.is_ascii_digit())
        && text
            .bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
}

/// Interpret a typed value: numbers, `true`/`false`, `null`, ISO dates (as dates),
/// quoted text and shell constructors such as `ObjectId("…")`; anything else is text.
pub fn infer_value(text: &str) -> Result<Bson, FormError> {
    let t = text.trim();
    match t {
        "true" => return Ok(Bson::Boolean(true)),
        "false" => return Ok(Bson::Boolean(false)),
        "null" => return Ok(Bson::Null),
        _ => {}
    }
    if looks_numeric(t) {
        if let Ok(n) = t.parse::<i64>() {
            return Ok(match i32::try_from(n) {
                Ok(small) => Bson::Int32(small),
                Err(_) => Bson::Int64(n),
            });
        }
        if let Ok(f) = t.parse::<f64>()
            && f.is_finite()
        {
            return Ok(Bson::Double(f));
        }
    }
    if looks_like_date(t)
        && let Ok(date) = shell::parse_date(t)
    {
        return Ok(Bson::DateTime(date));
    }
    let quoted = t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')));
    if quoted || CONSTRUCTORS.iter().any(|c| t.starts_with(c)) {
        return match shell::parse_value(t) {
            Ok(Bson::Document(_) | Bson::Array(_)) => err(format!("'{t}' is not a single value")),
            Ok(value) => Ok(value),
            Err(e) => err(format!("'{t}': {e}")),
        };
    }
    Ok(Bson::String(t.to_owned()))
}

/// The text that [`infer_value`] reads back as `value`, if there is one.
pub fn format_value(value: &Bson) -> Option<String> {
    let candidate = match value {
        Bson::String(s) => {
            if infer_value(s).ok().as_ref() == Some(value) {
                s.clone()
            } else {
                shell::to_shell_string(value, ShellStyle::Compact)
            }
        }
        Bson::Int64(v) if i32::try_from(*v).is_err() => v.to_string(),
        Bson::DateTime(date) => date.try_to_rfc3339_string().ok()?,
        Bson::Document(_) | Bson::Array(_) | Bson::RegularExpression(_) => return None,
        other => shell::to_shell_string(other, ShellStyle::Compact),
    };
    (infer_value(&candidate).ok().as_ref() == Some(value)).then_some(candidate)
}

/// Split a comma separated list, keeping commas inside quotes.
fn split_list(text: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            current.push(ch);
            continue;
        }
        match (ch, quote) {
            ('\\', Some(_)) => {
                escaped = true;
                current.push(ch);
            }
            ('"' | '\'', None) => {
                quote = Some(ch);
                current.push(ch);
            }
            (c, Some(q)) if c == q => {
                quote = None;
                current.push(ch);
            }
            (',', None) => items.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    items.push(current);
    items
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn infer_list(text: &str) -> Result<Vec<Bson>, FormError> {
    split_list(text)
        .iter()
        .map(|item| infer_value(item))
        .collect()
}

fn format_list(items: &[Bson]) -> Option<String> {
    let mut out = Vec::new();
    for item in items {
        let mut text = format_value(item)?;
        if text.contains(',') && !text.starts_with('"') {
            text = shell::to_shell_string(item, ShellStyle::Compact);
            if infer_value(&text).ok().as_ref() != Some(item) {
                return None;
            }
        }
        out.push(text);
    }
    Some(out.join(", "))
}

const REGEX_META: &str = "\\.^$|?*+()[]{}/";

fn escape_regex(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if REGEX_META.contains(ch) && ch != '/' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// The literal text a regex pattern matches, if it has no special syntax.
fn unescape_regex(pattern: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let next = chars.next()?;
            if !REGEX_META.contains(next) {
                return None;
            }
            out.push(next);
        } else if REGEX_META.contains(ch) && ch != '/' {
            return None;
        } else {
            out.push(ch);
        }
    }
    Some(out)
}

fn field_ref(field: &str) -> Bson {
    Bson::String(format!("${}", field.trim()))
}

/// The field of a `"$field"` reference (not `$$variables`).
fn as_field_ref(value: &Bson) -> Option<&str> {
    match value {
        Bson::String(s) if s.len() > 1 && s.starts_with('$') && !s.starts_with("$$") => {
            Some(&s[1..])
        }
        _ => None,
    }
}

fn check_field(field: &str, what: &str) -> Result<(), FormError> {
    let field = field.trim();
    if field.is_empty() {
        return err(format!("{what}: choose a field"));
    }
    if field.starts_with('$') {
        return err(format!("{what}: field names cannot start with '$'"));
    }
    if field.starts_with('.') || field.ends_with('.') || field.contains("..") {
        return err(format!("{what}: '{field}' is not a valid field path"));
    }
    Ok(())
}

fn check_output_name(name: &str, what: &str) -> Result<(), FormError> {
    let name = name.trim();
    if name.is_empty() {
        return err(format!("{what} needs a name"));
    }
    if name.starts_with('$') {
        return err(format!("{what} '{name}' cannot start with '$'"));
    }
    if name.contains('.') {
        return err(format!("{what} '{name}' cannot contain '.'"));
    }
    Ok(())
}

fn insert_unique(doc: &mut Document, key: &str, value: Bson) -> Result<(), FormError> {
    if doc.contains_key(key) {
        return err(format!("'{key}' appears more than once"));
    }
    doc.insert(key, value);
    Ok(())
}

// ----- $match ------------------------------------------------------------------------------

/// Comparison of one `$match` condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatchOp {
    #[default]
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    /// One of a comma separated list of values.
    In,
    /// Contains the text, ignoring case (a regular expression on escaped text).
    Contains,
    /// The field exists (`true`) or is missing (`false`).
    Exists,
}

impl MatchOp {
    pub const ALL: [Self; 9] = [
        Self::Eq,
        Self::Ne,
        Self::Gt,
        Self::Gte,
        Self::Lt,
        Self::Lte,
        Self::In,
        Self::Contains,
        Self::Exists,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "≠",
            Self::Gt => ">",
            Self::Gte => "≥",
            Self::Lt => "<",
            Self::Lte => "≤",
            Self::In => "in",
            Self::Contains => "contains",
            Self::Exists => "exists",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Eq => "equals",
            Self::Ne => "does not equal",
            Self::Gt => "greater than",
            Self::Gte => "greater than or equal",
            Self::Lt => "less than",
            Self::Lte => "less than or equal",
            Self::In => "one of a comma separated list",
            Self::Contains => "contains text (ignoring case)",
            Self::Exists => "field is present (true) or missing (false)",
        }
    }

    fn operator(self) -> Option<&'static str> {
        Some(match self {
            Self::Ne => "$ne",
            Self::Gt => "$gt",
            Self::Gte => "$gte",
            Self::Lt => "$lt",
            Self::Lte => "$lte",
            Self::In => "$in",
            Self::Exists => "$exists",
            Self::Eq | Self::Contains => return None,
        })
    }

    fn from_operator(name: &str) -> Option<Self> {
        Some(match name {
            "$eq" => Self::Eq,
            "$ne" => Self::Ne,
            "$gt" => Self::Gt,
            "$gte" => Self::Gte,
            "$lt" => Self::Lt,
            "$lte" => Self::Lte,
            "$in" => Self::In,
            "$exists" => Self::Exists,
            _ => return None,
        })
    }
}

/// How the conditions of a `$match` combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Combine {
    /// Every condition must hold (AND).
    #[default]
    All,
    /// At least one condition must hold (OR).
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Condition {
    pub field: String,
    pub op: MatchOp,
    /// Typed value; see [`infer_value`]. For `in` a comma separated list, for
    /// `exists` `true` or `false`.
    pub value: String,
}

impl Condition {
    pub fn new(field: &str, op: MatchOp, value: &str) -> Self {
        Self {
            field: field.into(),
            op,
            value: value.into(),
        }
    }

    fn is_blank(&self) -> bool {
        self.field.trim().is_empty() && self.value.trim().is_empty()
    }

    /// The condition's value part (what goes after `field:`).
    fn to_bson(&self, what: &str) -> Result<Bson, FormError> {
        let prefix = |e: FormError| FormError(format!("{what}: {e}"));
        let value = match self.op {
            MatchOp::Eq => return infer_value(&self.value).map_err(prefix),
            MatchOp::Contains => {
                let mut doc = Document::new();
                doc.insert("$regex", escape_regex(self.value.trim()));
                doc.insert("$options", "i");
                return Ok(Bson::Document(doc));
            }
            MatchOp::In => Bson::Array(infer_list(&self.value).map_err(prefix)?),
            MatchOp::Exists => match self.value.trim() {
                "" | "true" => Bson::Boolean(true),
                "false" => Bson::Boolean(false),
                _ => return err(format!("{what}: 'exists' needs true or false")),
            },
            _ => infer_value(&self.value).map_err(prefix)?,
        };
        let mut doc = Document::new();
        doc.insert(self.op.operator().unwrap_or("$eq"), value);
        Ok(Bson::Document(doc))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MatchForm {
    pub combine: Combine,
    pub conditions: Vec<Condition>,
}

impl MatchForm {
    fn to_bson(&self) -> Result<Bson, FormError> {
        let mut parts = Vec::new();
        for (i, c) in self.conditions.iter().enumerate() {
            if c.is_blank() {
                continue;
            }
            let what = format!("Condition {}", i + 1);
            check_field(&c.field, &what)?;
            parts.push((c.field.trim().to_owned(), c.to_bson(&what)?));
        }
        if parts.is_empty() {
            return Ok(Bson::Document(Document::new()));
        }
        let single = |(field, value): (String, Bson)| {
            let mut doc = Document::new();
            doc.insert(field, value);
            Bson::Document(doc)
        };
        let distinct = {
            let mut seen = std::collections::HashSet::new();
            parts.iter().all(|(f, _)| seen.insert(f.clone()))
        };
        let mut doc = Document::new();
        match self.combine {
            Combine::All if distinct => {
                for (field, value) in parts {
                    doc.insert(field, value);
                }
            }
            Combine::All => {
                doc.insert("$and", parts.into_iter().map(single).collect::<Vec<_>>());
            }
            Combine::Any => {
                doc.insert("$or", parts.into_iter().map(single).collect::<Vec<_>>());
            }
        }
        Ok(Bson::Document(doc))
    }

    fn from_bson(value: &Bson) -> Option<Self> {
        let Bson::Document(doc) = value else {
            return None;
        };
        if doc.len() == 1
            && let Some((key, Bson::Array(items))) = doc.iter().next()
            && (key == "$or" || key == "$and")
        {
            let mut conditions = Vec::new();
            for item in items {
                let Bson::Document(part) = item else {
                    return None;
                };
                let found = conditions_of(part)?;
                if found.len() != 1 {
                    return None;
                }
                conditions.extend(found);
            }
            let combine = if key == "$or" {
                Combine::Any
            } else {
                Combine::All
            };
            return Some(Self {
                combine,
                conditions,
            });
        }
        Some(Self {
            combine: Combine::All,
            conditions: conditions_of(doc)?,
        })
    }
}

/// Conditions of a plain filter document (no top-level operators).
fn conditions_of(doc: &Document) -> Option<Vec<Condition>> {
    let mut out = Vec::new();
    for (field, value) in doc {
        if field.starts_with('$') || field.is_empty() {
            return None;
        }
        match value {
            Bson::RegularExpression(regex) => out.push(contains(field, regex)?),
            Bson::Document(ops) => {
                if ops.contains_key("$regex") {
                    let pattern = ops.get_str("$regex").ok()?;
                    let options = match ops.get("$options") {
                        None => "",
                        Some(Bson::String(o)) => o,
                        Some(_) => return None,
                    };
                    if ops.len() != 1 + usize::from(ops.contains_key("$options")) {
                        return None;
                    }
                    let regex = Regex {
                        pattern: pattern.to_owned(),
                        options: options.to_owned(),
                    };
                    out.push(contains(field, &regex)?);
                    continue;
                }
                if ops.is_empty() {
                    return None;
                }
                for (name, operand) in ops {
                    let op = MatchOp::from_operator(name)?;
                    let value = match (op, operand) {
                        (MatchOp::In, Bson::Array(items)) => format_list(items)?,
                        (MatchOp::In, _) => return None,
                        (MatchOp::Exists, Bson::Boolean(b)) => b.to_string(),
                        (MatchOp::Exists, _) => return None,
                        (_, other) => format_value(other)?,
                    };
                    out.push(Condition::new(field, op, &value));
                }
            }
            other => out.push(Condition::new(field, MatchOp::Eq, &format_value(other)?)),
        }
    }
    Some(out)
}

fn contains(field: &str, regex: &Regex) -> Option<Condition> {
    if regex.options != "i" {
        return None;
    }
    Some(Condition::new(
        field,
        MatchOp::Contains,
        &unescape_regex(&regex.pattern)?,
    ))
}

// ----- $group ------------------------------------------------------------------------------

/// Accumulator function of a `$group` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accumulator {
    Sum,
    Avg,
    Min,
    Max,
    /// Number of documents (`{$sum: 1}`).
    #[default]
    Count,
    First,
    Last,
    Push,
    AddToSet,
}

impl Accumulator {
    pub const ALL: [Self; 9] = [
        Self::Count,
        Self::Sum,
        Self::Avg,
        Self::Min,
        Self::Max,
        Self::First,
        Self::Last,
        Self::Push,
        Self::AddToSet,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Avg => "average",
            Self::Min => "minimum",
            Self::Max => "maximum",
            Self::Count => "count",
            Self::First => "first",
            Self::Last => "last",
            Self::Push => "list of",
            Self::AddToSet => "distinct list of",
        }
    }

    fn operator(self) -> &'static str {
        match self {
            Self::Sum | Self::Count => "$sum",
            Self::Avg => "$avg",
            Self::Min => "$min",
            Self::Max => "$max",
            Self::First => "$first",
            Self::Last => "$last",
            Self::Push => "$push",
            Self::AddToSet => "$addToSet",
        }
    }

    fn from_operator(name: &str) -> Option<Self> {
        Some(match name {
            "$sum" => Self::Sum,
            "$avg" => Self::Avg,
            "$min" => Self::Min,
            "$max" => Self::Max,
            "$first" => Self::First,
            "$last" => Self::Last,
            "$push" => Self::Push,
            "$addToSet" => Self::AddToSet,
            _ => return None,
        })
    }

    /// Whether the function reads a field (all but count).
    pub fn needs_field(self) -> bool {
        self != Self::Count
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccumulatorRow {
    pub name: String,
    pub function: Accumulator,
    pub field: String,
}

impl AccumulatorRow {
    pub fn new(name: &str, function: Accumulator, field: &str) -> Self {
        Self {
            name: name.into(),
            function,
            field: field.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GroupForm {
    /// Group-by fields; none groups everything into one document.
    pub keys: Vec<String>,
    pub accumulators: Vec<AccumulatorRow>,
}

/// Key under which a group-by field appears in a compound `_id`.
fn key_name(field: &str) -> String {
    field.trim().replace('.', "_")
}

impl GroupForm {
    fn to_bson(&self) -> Result<Bson, FormError> {
        let keys: Vec<&str> = self
            .keys
            .iter()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .collect();
        let mut doc = Document::new();
        let id = match keys.as_slice() {
            [] => Bson::Null,
            [one] => {
                check_field(one, "Group by")?;
                field_ref(one)
            }
            many => {
                let mut id = Document::new();
                for key in many {
                    check_field(key, "Group by")?;
                    insert_unique(&mut id, &key_name(key), field_ref(key))?;
                }
                Bson::Document(id)
            }
        };
        doc.insert("_id", id);
        for (i, row) in self.accumulators.iter().enumerate() {
            if row.name.trim().is_empty() && row.field.trim().is_empty() {
                continue;
            }
            let what = format!("Output {}", i + 1);
            check_output_name(&row.name, &what)?;
            if row.name.trim() == "_id" {
                return err(format!(
                    "{what}: '_id' holds the group key; pick another name"
                ));
            }
            let operand = if row.function.needs_field() {
                check_field(&row.field, &what)?;
                field_ref(&row.field)
            } else {
                Bson::Int32(1)
            };
            let mut acc = Document::new();
            acc.insert(row.function.operator(), operand);
            insert_unique(&mut doc, row.name.trim(), Bson::Document(acc))?;
        }
        Ok(Bson::Document(doc))
    }

    fn from_bson(value: &Bson) -> Option<Self> {
        let Bson::Document(doc) = value else {
            return None;
        };
        let keys = match doc.get("_id")? {
            Bson::Null => Vec::new(),
            Bson::Document(id) if !id.is_empty() => {
                let mut keys = Vec::new();
                for (name, value) in id {
                    let field = as_field_ref(value)?;
                    if *name != key_name(field) {
                        return None;
                    }
                    keys.push(field.to_owned());
                }
                keys
            }
            other => vec![as_field_ref(other)?.to_owned()],
        };
        let mut accumulators = Vec::new();
        for (name, value) in doc {
            if name == "_id" {
                continue;
            }
            let Bson::Document(acc) = value else {
                return None;
            };
            if acc.len() != 1 {
                return None;
            }
            let (op, operand) = acc.iter().next()?;
            let function = Accumulator::from_operator(op)?;
            let row = match (function, operand) {
                (Accumulator::Sum, Bson::Int32(1) | Bson::Int64(1)) => {
                    AccumulatorRow::new(name, Accumulator::Count, "")
                }
                (Accumulator::Sum, Bson::Double(v)) if *v == 1.0 => {
                    AccumulatorRow::new(name, Accumulator::Count, "")
                }
                (function, operand) => AccumulatorRow::new(name, function, as_field_ref(operand)?),
            };
            accumulators.push(row);
        }
        Some(Self { keys, accumulators })
    }
}

// ----- $sort -------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SortRow {
    pub field: String,
    pub descending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SortForm {
    pub fields: Vec<SortRow>,
}

impl SortForm {
    fn to_bson(&self) -> Result<Bson, FormError> {
        let mut doc = Document::new();
        for (i, row) in self.fields.iter().enumerate() {
            if row.field.trim().is_empty() {
                continue;
            }
            check_field(&row.field, &format!("Sort {}", i + 1))?;
            let dir = Bson::Int32(if row.descending { -1 } else { 1 });
            insert_unique(&mut doc, row.field.trim(), dir)?;
        }
        if doc.is_empty() {
            return err("Add at least one field to sort by");
        }
        Ok(Bson::Document(doc))
    }

    fn from_bson(value: &Bson) -> Option<Self> {
        let Bson::Document(doc) = value else {
            return None;
        };
        let mut fields = Vec::new();
        for (field, dir) in doc {
            let descending = match dir {
                Bson::Int32(1) | Bson::Int64(1) => false,
                Bson::Int32(-1) | Bson::Int64(-1) => true,
                Bson::Double(v) if *v == 1.0 => false,
                Bson::Double(v) if *v == -1.0 => true,
                _ => return None,
            };
            fields.push(SortRow {
                field: field.clone(),
                descending,
            });
        }
        Some(Self { fields })
    }
}

// ----- $project ----------------------------------------------------------------------------

/// What `$project` does with one field.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectAction {
    #[default]
    Include,
    Exclude,
    /// Output the field under a new name.
    RenameTo(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectRow {
    pub field: String,
    pub action: ProjectAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectForm {
    pub exclude_id: bool,
    pub fields: Vec<ProjectRow>,
}

impl ProjectForm {
    fn to_bson(&self) -> Result<Bson, FormError> {
        let mut doc = Document::new();
        if self.exclude_id {
            doc.insert("_id", 0);
        }
        let (mut includes, mut excludes) = (false, false);
        for (i, row) in self.fields.iter().enumerate() {
            if row.field.trim().is_empty() {
                continue;
            }
            let what = format!("Field {}", i + 1);
            check_field(&row.field, &what)?;
            let field = row.field.trim();
            match &row.action {
                ProjectAction::Include => {
                    includes = true;
                    insert_unique(&mut doc, field, Bson::Int32(1))?;
                }
                ProjectAction::Exclude if field == "_id" => {
                    if !doc.contains_key("_id") {
                        doc.insert("_id", 0);
                    }
                }
                ProjectAction::Exclude => {
                    excludes = true;
                    insert_unique(&mut doc, field, Bson::Int32(0))?;
                }
                ProjectAction::RenameTo(name) => {
                    includes = true;
                    if name.trim().is_empty() {
                        return err(format!("{what}: enter the new name for '{field}'"));
                    }
                    check_output_name(name, &what)?;
                    insert_unique(&mut doc, name.trim(), field_ref(field))?;
                }
            }
        }
        if includes && excludes {
            return err(
                "A projection either keeps chosen fields or removes chosen fields, not both",
            );
        }
        if doc.is_empty() {
            return err("Choose at least one field to keep or remove");
        }
        Ok(Bson::Document(doc))
    }

    fn from_bson(value: &Bson) -> Option<Self> {
        let Bson::Document(doc) = value else {
            return None;
        };
        let mut form = Self::default();
        for (key, value) in doc {
            let flag = match value {
                Bson::Int32(v) => Some(*v != 0),
                Bson::Int64(v) => Some(*v != 0),
                Bson::Double(v) => Some(*v != 0.0),
                Bson::Boolean(b) => Some(*b),
                _ => None,
            };
            if key == "_id"
                && let Some(flag) = flag
            {
                form.exclude_id = !flag;
                continue;
            }
            let row = match (flag, as_field_ref(value)) {
                (Some(true), _) => ProjectRow {
                    field: key.clone(),
                    action: ProjectAction::Include,
                },
                (Some(false), _) => ProjectRow {
                    field: key.clone(),
                    action: ProjectAction::Exclude,
                },
                (None, Some(field)) if !key.contains('.') => ProjectRow {
                    field: field.to_owned(),
                    action: ProjectAction::RenameTo(key.clone()),
                },
                _ => return None,
            };
            form.fields.push(row);
        }
        Some(form)
    }
}

// ----- $unwind -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnwindForm {
    /// Array field to unwind (without `$`).
    pub path: String,
    /// Keep documents whose array is missing, null or empty.
    pub preserve_empty: bool,
}

impl UnwindForm {
    fn to_bson(&self) -> Result<Bson, FormError> {
        check_field(&self.path, "Array field")?;
        if !self.preserve_empty {
            return Ok(field_ref(&self.path));
        }
        let mut doc = Document::new();
        doc.insert("path", field_ref(&self.path));
        doc.insert("preserveNullAndEmptyArrays", true);
        Ok(Bson::Document(doc))
    }

    fn from_bson(value: &Bson) -> Option<Self> {
        if let Some(path) = as_field_ref(value) {
            return Some(Self {
                path: path.to_owned(),
                preserve_empty: false,
            });
        }
        let Bson::Document(doc) = value else {
            return None;
        };
        let path = as_field_ref(doc.get("path")?)?.to_owned();
        let preserve_empty = match doc.get("preserveNullAndEmptyArrays") {
            None => false,
            Some(Bson::Boolean(b)) => *b,
            Some(_) => return None,
        };
        let expected = 1 + usize::from(doc.contains_key("preserveNullAndEmptyArrays"));
        (doc.len() == expected).then_some(Self {
            path,
            preserve_empty,
        })
    }
}

// ----- $lookup -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LookupForm {
    pub from: String,
    pub local_field: String,
    pub foreign_field: String,
    pub as_field: String,
}

const LOOKUP_KEYS: [&str; 4] = ["from", "localField", "foreignField", "as"];

impl LookupForm {
    fn to_bson(&self) -> Result<Bson, FormError> {
        if self.from.trim().is_empty() {
            return err("Choose the collection to join");
        }
        check_field(&self.local_field, "Local field")?;
        check_field(&self.foreign_field, "Foreign field")?;
        check_field(&self.as_field, "Output array")?;
        let mut doc = Document::new();
        for (key, value) in LOOKUP_KEYS.iter().zip([
            &self.from,
            &self.local_field,
            &self.foreign_field,
            &self.as_field,
        ]) {
            doc.insert(*key, value.trim());
        }
        Ok(Bson::Document(doc))
    }

    fn from_bson(value: &Bson) -> Option<Self> {
        let Bson::Document(doc) = value else {
            return None;
        };
        if doc.len() != LOOKUP_KEYS.len() {
            return None;
        }
        let get = |key: &str| doc.get_str(key).ok().map(str::to_owned);
        Some(Self {
            from: get("from")?,
            local_field: get("localField")?,
            foreign_field: get("foreignField")?,
            as_field: get("as")?,
        })
    }
}

// ----- $addFields / $set -------------------------------------------------------------------

/// Where the value of an added field comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ValueSource {
    /// Copy of another field.
    #[default]
    Field,
    /// A typed constant (see [`infer_value`]).
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddFieldRow {
    pub name: String,
    pub source: ValueSource,
    /// Field name (for [`ValueSource::Field`]) or typed value.
    pub value: String,
}

impl AddFieldRow {
    pub fn new(name: &str, source: ValueSource, value: &str) -> Self {
        Self {
            name: name.into(),
            source,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddFieldsForm {
    pub fields: Vec<AddFieldRow>,
}

impl AddFieldsForm {
    fn to_bson(&self) -> Result<Bson, FormError> {
        let mut doc = Document::new();
        for (i, row) in self.fields.iter().enumerate() {
            if row.name.trim().is_empty() && row.value.trim().is_empty() {
                continue;
            }
            let what = format!("Field {}", i + 1);
            check_field(&row.name, &what)?;
            let value = match row.source {
                ValueSource::Field => {
                    check_field(&row.value, &what)?;
                    field_ref(&row.value)
                }
                ValueSource::Value => match infer_value(&row.value) {
                    // A text starting with '$' would read as a field reference.
                    Ok(Bson::String(s)) if s.starts_with('$') => {
                        let mut literal = Document::new();
                        literal.insert("$literal", s);
                        Bson::Document(literal)
                    }
                    Ok(value) => value,
                    Err(e) => return err(format!("{what}: {e}")),
                },
            };
            insert_unique(&mut doc, row.name.trim(), value)?;
        }
        if doc.is_empty() {
            return err("Add at least one field");
        }
        Ok(Bson::Document(doc))
    }

    fn from_bson(value: &Bson) -> Option<Self> {
        let Bson::Document(doc) = value else {
            return None;
        };
        let mut fields = Vec::new();
        for (name, value) in doc {
            if name.starts_with('$') {
                return None;
            }
            let row = if let Some(field) = as_field_ref(value) {
                AddFieldRow::new(name, ValueSource::Field, field)
            } else if let Bson::Document(literal) = value
                && literal.len() == 1
                && let Ok(text) = literal.get_str("$literal")
                && text.starts_with('$')
            {
                AddFieldRow::new(name, ValueSource::Value, text)
            } else {
                AddFieldRow::new(name, ValueSource::Value, &format_value(value)?)
            };
            fields.push(row);
        }
        Some(Self { fields })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{STAGE_OPERATORS, Stage};
    use mongodb::bson::{DateTime, doc};

    fn operator_of(form: &StageForm) -> &'static str {
        match form {
            StageForm::Match(_) => "$match",
            StageForm::Group(_) => "$group",
            StageForm::Sort(_) => "$sort",
            StageForm::Project(_) => "$project",
            StageForm::Limit(_) => "$limit",
            StageForm::Skip(_) => "$skip",
            StageForm::Count(_) => "$count",
            StageForm::Unwind(_) => "$unwind",
            StageForm::Lookup(_) => "$lookup",
            StageForm::AddFields(_) => "$addFields",
        }
    }

    /// Form → body → stage parser → form, returning the stage document.
    fn round_trip(form: StageForm) -> Document {
        let operator = operator_of(&form);
        let body = form.to_body().unwrap_or_else(|e| panic!("{form:?}: {e}"));
        let stage = Stage {
            operator: operator.into(),
            body: body.clone(),
            enabled: true,
        };
        let document = stage
            .to_document()
            .unwrap_or_else(|e| panic!("{body}: {e}"));
        assert_eq!(
            StageForm::parse(operator, &body).as_ref(),
            Some(&form),
            "{body}"
        );
        document
    }

    fn match_form(combine: Combine, conditions: Vec<Condition>) -> StageForm {
        StageForm::Match(MatchForm {
            combine,
            conditions,
        })
    }

    #[test]
    fn infers_value_types() {
        assert_eq!(infer_value("42").unwrap(), Bson::Int32(42));
        assert_eq!(
            infer_value("9007199254740993").unwrap(),
            Bson::Int64(9_007_199_254_740_993)
        );
        assert_eq!(infer_value(" 2.5 ").unwrap(), Bson::Double(2.5));
        assert_eq!(infer_value("-1e3").unwrap(), Bson::Double(-1000.0));
        assert_eq!(infer_value("true").unwrap(), Bson::Boolean(true));
        assert_eq!(infer_value("null").unwrap(), Bson::Null);
        assert_eq!(
            infer_value("2024-01-02").unwrap(),
            Bson::DateTime(DateTime::parse_rfc3339_str("2024-01-02T00:00:00Z").unwrap())
        );
        assert_eq!(
            infer_value("Berlin").unwrap(),
            Bson::String("Berlin".into())
        );
        assert_eq!(infer_value("inf").unwrap(), Bson::String("inf".into()));
        assert_eq!(infer_value("\"42\"").unwrap(), Bson::String("42".into()));
        assert_eq!(infer_value("'a, b'").unwrap(), Bson::String("a, b".into()));
        assert!(matches!(
            infer_value("ObjectId('5f1d7f1e2b3c4d5e6f708192')").unwrap(),
            Bson::ObjectId(_)
        ));
        assert!(infer_value("ObjectId('nope')").is_err());
        assert!(infer_value("\"open").is_ok(), "unbalanced quote is text");
        assert!(infer_value("2024-13-45").is_ok(), "bad date stays text");
    }

    #[test]
    fn formats_values_so_they_infer_back() {
        for value in [
            Bson::Int32(7),
            Bson::Int64(7),
            Bson::Int64(1 << 40),
            Bson::Double(3.0),
            Bson::Double(0.1),
            Bson::Boolean(false),
            Bson::Null,
            Bson::String("42".into()),
            Bson::String("true".into()),
            Bson::String(" padded ".into()),
            Bson::String("say \"hi\"".into()),
            Bson::String("plain text".into()),
            Bson::DateTime(DateTime::from_millis(1_700_000_000_123)),
        ] {
            let text = format_value(&value).unwrap_or_else(|| panic!("{value:?}"));
            assert_eq!(infer_value(&text).unwrap(), value, "{text}");
        }
        assert_eq!(format_value(&Bson::Document(doc! {"a": 1})), None);
        assert_eq!(format_value(&Bson::Double(f64::NAN)), None);
    }

    #[test]
    fn match_forms_round_trip() {
        let all = round_trip(match_form(
            Combine::All,
            vec![
                Condition::new("status", MatchOp::Eq, "active"),
                Condition::new("age", MatchOp::Gte, "18"),
                Condition::new("score", MatchOp::Lt, "9.5"),
                Condition::new("city", MatchOp::Ne, "Berlin"),
                Condition::new("tags", MatchOp::In, "a, b, 3"),
                Condition::new("name", MatchOp::Contains, "a.b (c)"),
                Condition::new("email", MatchOp::Exists, "true"),
                Condition::new("deleted", MatchOp::Exists, "false"),
                Condition::new("created", MatchOp::Gt, "2024-01-01T00:00:00Z"),
                Condition::new("note", MatchOp::Eq, "null"),
                Condition::new("address.zip", MatchOp::Lte, "99999"),
                Condition::new("total", MatchOp::Gt, "100"),
            ],
        ));
        let filter = all.get_document("$match").unwrap();
        assert_eq!(filter.get_str("status").unwrap(), "active");
        assert_eq!(filter.get_document("age").unwrap(), &doc! {"$gte": 18});
        assert_eq!(
            filter.get_document("tags").unwrap(),
            &doc! {"$in": ["a", "b", 3]}
        );
        assert_eq!(
            filter.get_document("name").unwrap(),
            &doc! {"$regex": "a\\.b \\(c\\)", "$options": "i"}
        );
        assert!(matches!(
            filter.get_document("created").unwrap().get("$gt"),
            Some(Bson::DateTime(_))
        ));

        let any = round_trip(match_form(
            Combine::Any,
            vec![
                Condition::new("a", MatchOp::Eq, "1"),
                Condition::new("b", MatchOp::Gt, "2"),
            ],
        ));
        assert_eq!(any, doc! {"$match": {"$or": [{"a": 1}, {"b": {"$gt": 2}}]}});

        // The same field twice needs $and.
        let range = round_trip(match_form(
            Combine::All,
            vec![
                Condition::new("n", MatchOp::Gt, "1"),
                Condition::new("n", MatchOp::Lt, "5"),
            ],
        ));
        assert_eq!(
            range,
            doc! {"$match": {"$and": [{"n": {"$gt": 1}}, {"n": {"$lt": 5}}]}}
        );
        assert_eq!(
            round_trip(match_form(Combine::All, vec![])),
            doc! {"$match": {}}
        );
        // In-lists keep quoted commas and quoted numbers.
        round_trip(match_form(
            Combine::All,
            vec![Condition::new("x", MatchOp::In, "\"a, b\", \"7\", 7")],
        ));
    }

    #[test]
    fn match_parses_shell_shapes() {
        let form = StageForm::parse("$match", "{n: {$gt: 1, $lt: 5}, s: /abc/i, 'a.b': 'x'}");
        assert_eq!(
            form,
            Some(match_form(
                Combine::All,
                vec![
                    Condition::new("n", MatchOp::Gt, "1"),
                    Condition::new("n", MatchOp::Lt, "5"),
                    Condition::new("s", MatchOp::Contains, "abc"),
                    Condition::new("a.b", MatchOp::Eq, "x"),
                ]
            ))
        );
        let form = StageForm::parse("$match", "{a: {$eq: 'x'}}").unwrap();
        assert_eq!(
            form,
            match_form(Combine::All, vec![Condition::new("a", MatchOp::Eq, "x")])
        );
        // Blank rows are drafts and do not reach the body.
        let draft = match_form(Combine::All, vec![Condition::default()]);
        assert_eq!(draft.to_body().unwrap(), "{}");
    }

    #[test]
    fn match_rejects_unsupported_shapes() {
        for body in [
            "{$expr: {$gt: ['$a', '$b']}}",
            "{a: {b: 1}}",
            "{a: [1, 2]}",
            "{a: /^x/i}",
            "{a: /x/}",
            "{a: {$regex: 'x', $options: 'i', $foo: 1}}",
            "{a: {$elemMatch: {b: 1}}}",
            "{a: {$in: 5}}",
            "{a: {}}",
            "{$or: [{a: 1, b: 2}]}",
            "{$nor: [{a: 1}]}",
            "[1]",
            "{a: }",
        ] {
            assert_eq!(StageForm::parse("$match", body), None, "{body}");
        }
    }

    #[test]
    fn match_reports_invalid_input() {
        let form = match_form(
            Combine::All,
            vec![Condition::new("", MatchOp::Eq, "orphan value")],
        );
        assert!(form.to_body().unwrap_err().0.contains("choose a field"));
        let form = match_form(Combine::All, vec![Condition::new("$a", MatchOp::Eq, "1")]);
        assert!(form.to_body().is_err());
        let form = match_form(
            Combine::All,
            vec![Condition::new("a", MatchOp::Exists, "maybe")],
        );
        assert!(form.to_body().unwrap_err().0.contains("true or false"));
        let form = match_form(
            Combine::All,
            vec![Condition::new("a", MatchOp::Eq, "ObjectId('zz')")],
        );
        assert!(form.to_body().unwrap_err().0.starts_with("Condition 1"));
    }

    #[test]
    fn group_forms_round_trip() {
        let none = round_trip(StageForm::Group(GroupForm {
            keys: vec![],
            accumulators: vec![AccumulatorRow::new("n", Accumulator::Count, "")],
        }));
        assert_eq!(none, doc! {"$group": {"_id": null, "n": {"$sum": 1}}});
        let one = round_trip(StageForm::Group(GroupForm {
            keys: vec!["region".into()],
            accumulators: vec![
                AccumulatorRow::new("total", Accumulator::Sum, "amount"),
                AccumulatorRow::new("avg", Accumulator::Avg, "amount"),
                AccumulatorRow::new("lo", Accumulator::Min, "amount"),
                AccumulatorRow::new("hi", Accumulator::Max, "amount"),
                AccumulatorRow::new("first", Accumulator::First, "name"),
                AccumulatorRow::new("last", Accumulator::Last, "name"),
                AccumulatorRow::new("all", Accumulator::Push, "name"),
                AccumulatorRow::new("distinct", Accumulator::AddToSet, "name"),
            ],
        }));
        let group = one.get_document("$group").unwrap();
        assert_eq!(group.get_str("_id").unwrap(), "$region");
        assert_eq!(group.keys().next().unwrap(), "_id");
        let many = round_trip(StageForm::Group(GroupForm {
            keys: vec!["region".into(), "address.city".into()],
            accumulators: vec![],
        }));
        assert_eq!(
            many,
            doc! {"$group": {"_id": {"region": "$region", "address_city": "$address.city"}}}
        );
        assert_eq!(
            StageForm::parse("$group", "{_id: '$a', n: {$sum: 1.0}}"),
            Some(StageForm::Group(GroupForm {
                keys: vec!["a".into()],
                accumulators: vec![AccumulatorRow::new("n", Accumulator::Count, "")],
            }))
        );
    }

    #[test]
    fn group_rejects_unsupported_and_invalid() {
        for body in [
            "{n: {$sum: 1}}",
            "{_id: {$year: '$d'}}",
            "{_id: {y: '$a'}}",
            "{_id: 'x'}",
            "{_id: '$a', n: {$sum: 2}}",
            "{_id: '$a', n: {$sum: {$multiply: ['$p', '$q']}}}",
            "{_id: '$a', n: {$stdDevPop: '$x'}}",
            "{_id: '$a', n: 1}",
        ] {
            assert_eq!(StageForm::parse("$group", body), None, "{body}");
        }
        let bad = |rows: Vec<AccumulatorRow>| {
            StageForm::Group(GroupForm {
                keys: vec![],
                accumulators: rows,
            })
            .to_body()
        };
        assert!(bad(vec![AccumulatorRow::new("_id", Accumulator::Count, "")]).is_err());
        assert!(bad(vec![AccumulatorRow::new("t", Accumulator::Sum, "")]).is_err());
        assert!(bad(vec![AccumulatorRow::new("", Accumulator::Sum, "x")]).is_err());
        assert!(
            bad(vec![
                AccumulatorRow::new("t", Accumulator::Count, ""),
                AccumulatorRow::new("t", Accumulator::Count, ""),
            ])
            .unwrap_err()
            .0
            .contains("more than once")
        );
    }

    #[test]
    fn sort_project_and_simple_stages_round_trip() {
        let sort = round_trip(StageForm::Sort(SortForm {
            fields: vec![
                SortRow {
                    field: "a".into(),
                    descending: false,
                },
                SortRow {
                    field: "b.c".into(),
                    descending: true,
                },
            ],
        }));
        assert_eq!(sort, doc! {"$sort": {"a": 1, "b.c": -1}});
        assert!(StageForm::Sort(SortForm::default()).to_body().is_err());
        assert_eq!(StageForm::parse("$sort", "{s: {$meta: 'textScore'}}"), None);

        let project = round_trip(StageForm::Project(ProjectForm {
            exclude_id: true,
            fields: vec![
                ProjectRow {
                    field: "name".into(),
                    action: ProjectAction::Include,
                },
                ProjectRow {
                    field: "address.city".into(),
                    action: ProjectAction::RenameTo("city".into()),
                },
            ],
        }));
        assert_eq!(
            project,
            doc! {"$project": {"_id": 0, "name": 1, "city": "$address.city"}}
        );
        round_trip(StageForm::Project(ProjectForm {
            exclude_id: false,
            fields: vec![ProjectRow {
                field: "secret".into(),
                action: ProjectAction::Exclude,
            }],
        }));
        let mixed = ProjectForm {
            exclude_id: false,
            fields: vec![
                ProjectRow {
                    field: "a".into(),
                    action: ProjectAction::Include,
                },
                ProjectRow {
                    field: "b".into(),
                    action: ProjectAction::Exclude,
                },
            ],
        };
        assert!(StageForm::Project(mixed).to_body().is_err());
        assert!(
            StageForm::Project(ProjectForm::default())
                .to_body()
                .is_err()
        );
        for body in ["{a: {$toUpper: '$a'}}", "{a: 'text'}", "{'x.y': '$a'}"] {
            assert_eq!(StageForm::parse("$project", body), None, "{body}");
        }

        assert_eq!(round_trip(StageForm::Limit(25)), doc! {"$limit": 25});
        assert!(StageForm::Limit(0).to_body().is_err());
        assert_eq!(round_trip(StageForm::Skip(0)), doc! {"$skip": 0});
        assert_eq!(
            round_trip(StageForm::Limit(5_000_000_000)),
            doc! {"$limit": 5_000_000_000_i64}
        );
        assert_eq!(StageForm::parse("$limit", "-1"), None);
        assert_eq!(StageForm::parse("$limit", "'ten'"), None);
        assert_eq!(
            round_trip(StageForm::Count("total".into())),
            doc! {"$count": "total"}
        );
        assert!(StageForm::Count("$x".into()).to_body().is_err());
        assert!(StageForm::Count(" ".into()).to_body().is_err());
    }

    #[test]
    fn unwind_lookup_and_add_fields_round_trip() {
        let plain = round_trip(StageForm::Unwind(UnwindForm {
            path: "items".into(),
            preserve_empty: false,
        }));
        assert_eq!(plain, doc! {"$unwind": "$items"});
        let keep = round_trip(StageForm::Unwind(UnwindForm {
            path: "items".into(),
            preserve_empty: true,
        }));
        assert_eq!(
            keep,
            doc! {"$unwind": {"path": "$items", "preserveNullAndEmptyArrays": true}}
        );
        assert_eq!(
            StageForm::parse("$unwind", "{path: '$a', includeArrayIndex: 'i'}"),
            None
        );
        assert!(StageForm::Unwind(UnwindForm::default()).to_body().is_err());

        let lookup = round_trip(StageForm::Lookup(LookupForm {
            from: "customers".into(),
            local_field: "customerId".into(),
            foreign_field: "_id".into(),
            as_field: "customer".into(),
        }));
        assert_eq!(
            lookup,
            doc! {"$lookup": {"from": "customers", "localField": "customerId", "foreignField": "_id", "as": "customer"}}
        );
        assert_eq!(
            StageForm::parse(
                "$lookup",
                "{from: 'c', let: {x: '$a'}, pipeline: [], as: 'j'}"
            ),
            None
        );
        assert!(StageForm::Lookup(LookupForm::default()).to_body().is_err());

        let add = round_trip(StageForm::AddFields(AddFieldsForm {
            fields: vec![
                AddFieldRow::new("copy", ValueSource::Field, "price"),
                AddFieldRow::new("flag", ValueSource::Value, "true"),
                AddFieldRow::new("label", ValueSource::Value, "new"),
                AddFieldRow::new("n", ValueSource::Value, "3"),
                AddFieldRow::new("cost", ValueSource::Value, "$5"),
            ],
        }));
        assert_eq!(
            add,
            doc! {"$addFields": {"copy": "$price", "flag": true, "label": "new", "n": 3, "cost": {"$literal": "$5"}}}
        );
        assert_eq!(
            StageForm::parse("$set", "{a: {$concat: ['$x', '$y']}}"),
            None
        );
        assert!(
            StageForm::AddFields(AddFieldsForm::default())
                .to_body()
                .is_err()
        );
    }

    #[test]
    fn templates_and_empty_forms() {
        for operator in STAGE_OPERATORS {
            let parsed = StageForm::parse(operator.name, operator.template);
            assert_eq!(
                parsed.is_some(),
                has_form(operator.name),
                "{}",
                operator.name
            );
        }
        for operator in FORM_OPERATORS {
            assert!(StageForm::empty(operator).is_some(), "{operator}");
        }
        assert_eq!(StageForm::empty("$facet"), None);
        assert_eq!(StageForm::parse("$facet", "{}"), None);
        assert_eq!(StageForm::parse("$match", "{ unterminated"), None);
    }
}
