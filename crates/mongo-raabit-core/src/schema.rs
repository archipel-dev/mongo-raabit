//! Schema Explorer: sampling-based field, type and value statistics.

use crate::connection::Connection;
use crate::error::Result;
use crate::shell::{self, ShellStyle};
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, DateTime, Document, doc};
use serde::Serialize;
use std::collections::HashMap;

/// How documents are picked for analysis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum SampleMethod {
    /// `$sample` — random documents.
    #[default]
    Random,
    /// Natural order, first n.
    First,
    /// Natural order reversed, last n.
    Last,
}

const MAX_TRACKED_VALUES: usize = 1000;
const TOP_VALUES: usize = 10;
pub const HISTOGRAM_BUCKETS: usize = 10;

/// Per-field statistics.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FieldStats {
    /// Dotted path; array elements use `[]`, e.g. `items[].sku`.
    pub path: String,
    pub depth: usize,
    /// Number of values seen (documents, or array elements for `[]` paths).
    pub count: u64,
    /// Number of parent values that could have contained the field.
    pub parent_count: u64,
    /// Type name → occurrences, most frequent first.
    pub types: Vec<(String, u64)>,
    pub null_count: u64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub mean: Option<f64>,
    pub min_date: Option<DateTime>,
    pub max_date: Option<DateTime>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    /// Most frequent scalar values (shell syntax) with counts.
    pub top_values: Vec<(String, u64)>,
    /// More distinct values exist than were tracked.
    pub distinct_overflow: bool,
    pub distinct_count: usize,
    /// Equal-width histogram over `[min, max]` for numeric fields.
    pub histogram: Vec<u64>,
}

impl FieldStats {
    /// Share of parent values containing the field, 0.0–1.0.
    pub fn probability(&self) -> f64 {
        if self.parent_count == 0 {
            0.0
        } else {
            self.count as f64 / self.parent_count as f64
        }
    }

    /// Field name (last path segment).
    pub fn name(&self) -> &str {
        self.path.rsplit('.').next().unwrap_or(&self.path)
    }

    /// The dominant type, if any.
    pub fn primary_type(&self) -> Option<&str> {
        self.types.first().map(|(t, _)| t.as_str())
    }
}

/// Result of a schema analysis.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SchemaReport {
    pub sampled: u64,
    /// Fields in first-seen, depth-first order.
    pub fields: Vec<FieldStats>,
}

#[derive(Default)]
struct Accumulator {
    order: Vec<String>,
    fields: HashMap<String, FieldAcc>,
}

#[derive(Default)]
struct FieldAcc {
    depth: usize,
    count: u64,
    types: HashMap<&'static str, u64>,
    nulls: u64,
    numbers: Vec<f64>,
    min_date: Option<DateTime>,
    max_date: Option<DateTime>,
    min_len: Option<usize>,
    max_len: Option<usize>,
    values: HashMap<String, u64>,
    overflow: bool,
}

impl Accumulator {
    fn field(&mut self, path: &str, depth: usize) -> &mut FieldAcc {
        if !self.fields.contains_key(path) {
            self.order.push(path.to_owned());
        }
        let acc = self.fields.entry(path.to_owned()).or_default();
        acc.depth = depth;
        acc
    }

    fn document(&mut self, prefix: &str, doc: &Document, depth: usize) {
        // Parent counts: every field ever seen under this prefix could have appeared.
        for (key, value) in doc {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            self.value(&path, value, depth);
        }
    }

    fn value(&mut self, path: &str, value: &Bson, depth: usize) {
        let acc = self.field(path, depth);
        acc.count += 1;
        *acc.types.entry(shell::type_name(value)).or_default() += 1;
        let track = |acc: &mut FieldAcc, key: String| {
            if let Some(n) = acc.values.get_mut(&key) {
                *n += 1;
            } else if acc.values.len() < MAX_TRACKED_VALUES {
                acc.values.insert(key, 1);
            } else {
                acc.overflow = true;
            }
        };
        match value {
            Bson::Null | Bson::Undefined => acc.nulls += 1,
            Bson::Int32(v) => {
                acc.numbers.push(f64::from(*v));
                track(acc, v.to_string());
            }
            Bson::Int64(v) => {
                acc.numbers.push(*v as f64);
                track(acc, v.to_string());
            }
            Bson::Double(v) => {
                if v.is_finite() {
                    acc.numbers.push(*v);
                }
                track(acc, shell::to_shell_string(value, ShellStyle::Compact));
            }
            Bson::Decimal128(d) => {
                if let Ok(v) = d.to_string().parse::<f64>() {
                    acc.numbers.push(v);
                }
                track(acc, d.to_string());
            }
            Bson::String(s) => {
                let len = s.chars().count();
                acc.min_len = Some(acc.min_len.map_or(len, |m| m.min(len)));
                acc.max_len = Some(acc.max_len.map_or(len, |m| m.max(len)));
                track(acc, s.clone());
            }
            Bson::Boolean(b) => track(acc, b.to_string()),
            Bson::DateTime(d) => {
                acc.min_date = Some(acc.min_date.map_or(*d, |m| m.min(*d)));
                acc.max_date = Some(acc.max_date.map_or(*d, |m| m.max(*d)));
            }
            Bson::ObjectId(_) => acc.overflow = true, // unique by nature; not worth tracking
            Bson::Document(inner) => {
                self.document(path, inner, depth + 1);
            }
            Bson::Array(items) => {
                let len = items.len();
                acc.min_len = Some(acc.min_len.map_or(len, |m| m.min(len)));
                acc.max_len = Some(acc.max_len.map_or(len, |m| m.max(len)));
                let element_path = format!("{path}[]");
                for item in items {
                    self.value(&element_path, item, depth + 1);
                }
            }
            _ => {}
        }
    }

    fn finish(self, sampled: u64) -> SchemaReport {
        let Self { order, mut fields } = self;
        // A nested field could appear once per object value of its parent path.
        let objects: HashMap<String, u64> = fields
            .iter()
            .map(|(path, acc)| (path.clone(), acc.types.get("Object").copied().unwrap_or(0)))
            .collect();
        let fields = order
            .into_iter()
            .filter_map(|path| {
                let acc = fields.remove(&path)?;
                let mut types: Vec<(String, u64)> = acc
                    .types
                    .into_iter()
                    .map(|(t, n)| (t.to_owned(), n))
                    .collect();
                types.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                let distinct_count = acc.values.len();
                let mut top: Vec<(String, u64)> = acc.values.into_iter().collect();
                top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                top.truncate(TOP_VALUES);
                let (min, max, mean) = if acc.numbers.is_empty() {
                    (None, None, None)
                } else {
                    let min = acc.numbers.iter().copied().fold(f64::INFINITY, f64::min);
                    let max = acc
                        .numbers
                        .iter()
                        .copied()
                        .fold(f64::NEG_INFINITY, f64::max);
                    let mean = acc.numbers.iter().sum::<f64>() / acc.numbers.len() as f64;
                    (Some(min), Some(max), Some(mean))
                };
                let histogram = match (min, max) {
                    (Some(min), Some(max)) => histogram(&acc.numbers, min, max),
                    _ => Vec::new(),
                };
                let parent_count = if path.ends_with("[]") {
                    acc.count
                } else {
                    match path.rsplit_once('.') {
                        Some((parent, _)) => objects.get(parent).copied().unwrap_or(acc.count),
                        None => sampled,
                    }
                };
                Some(FieldStats {
                    depth: acc.depth,
                    count: acc.count,
                    parent_count,
                    types,
                    null_count: acc.nulls,
                    min,
                    max,
                    mean,
                    min_date: acc.min_date,
                    max_date: acc.max_date,
                    min_length: acc.min_len,
                    max_length: acc.max_len,
                    top_values: top,
                    distinct_overflow: acc.overflow,
                    distinct_count,
                    histogram,
                    path,
                })
            })
            .collect();
        SchemaReport { sampled, fields }
    }
}

fn histogram(values: &[f64], min: f64, max: f64) -> Vec<u64> {
    let mut buckets = vec![0u64; HISTOGRAM_BUCKETS];
    let width = (max - min) / HISTOGRAM_BUCKETS as f64;
    for value in values {
        let index = if width <= 0.0 {
            0
        } else {
            (((value - min) / width) as usize).min(HISTOGRAM_BUCKETS - 1)
        };
        buckets[index] += 1;
    }
    buckets
}

/// Analyze already-fetched documents.
pub fn analyze(documents: &[Document]) -> SchemaReport {
    let mut acc = Accumulator::default();
    for document in documents {
        acc.document("", document, 0);
    }
    acc.finish(documents.len() as u64)
}

impl Connection {
    /// Sample up to `size` documents and analyze them.
    pub async fn sample_schema(
        &self,
        db: &str,
        collection: &str,
        size: u64,
        method: SampleMethod,
        filter: Document,
    ) -> Result<SchemaReport> {
        let size = i64::try_from(size.clamp(1, 100_000)).unwrap_or(1000);
        let handle = self.collection(db, collection);
        let docs: Vec<Document> = match method {
            SampleMethod::Random => {
                let mut pipeline = Vec::new();
                if !filter.is_empty() {
                    pipeline.push(doc! {"$match": filter});
                }
                pipeline.push(doc! {"$sample": {"size": size}});
                handle.aggregate(pipeline).await?.try_collect().await?
            }
            // `$natural` order is only available to find, not to `$sort`.
            SampleMethod::First | SampleMethod::Last => {
                let direction = if method == SampleMethod::First { 1 } else { -1 };
                handle
                    .find(filter)
                    .sort(doc! {"$natural": direction})
                    .limit(size)
                    .await?
                    .try_collect()
                    .await?
            }
        };
        Ok(analyze(&docs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyzes_types_frequencies_and_nesting() {
        let docs = vec![
            doc! {"name": "Ada", "age": 36, "address": {"city": "London"}, "tags": ["a", "b"]},
            doc! {"name": "Grace", "age": 45.5, "address": {"city": "NYC", "zip": "10001"}, "tags": []},
            doc! {"name": "Linus", "age": null, "items": [{"sku": "x", "qty": 2}, {"sku": "y"}]},
            doc! {"name": 7},
        ];
        let report = analyze(&docs);
        assert_eq!(report.sampled, 4);
        let field = |p: &str| report.fields.iter().find(|f| f.path == p).unwrap();

        let name = field("name");
        assert_eq!(name.count, 4);
        assert_eq!(name.probability(), 1.0);
        assert_eq!(name.types, vec![("String".into(), 3), ("Int32".into(), 1)]);
        assert_eq!(name.min_length, Some(3));

        let age = field("age");
        assert_eq!(age.null_count, 1);
        assert_eq!(age.min, Some(36.0));
        assert_eq!(age.max, Some(45.5));
        assert_eq!(age.histogram.iter().sum::<u64>(), 2);
        assert_eq!(age.histogram[0], 1);
        assert_eq!(age.histogram[HISTOGRAM_BUCKETS - 1], 1);

        let zip = field("address.zip");
        assert_eq!(zip.depth, 1);
        assert_eq!(zip.count, 1);
        assert_eq!(zip.parent_count, 2);
        assert_eq!(zip.probability(), 0.5);
        assert_eq!(field("address").probability(), 0.5);

        let tags = field("tags");
        assert_eq!((tags.min_length, tags.max_length), (Some(0), Some(2)));
        assert_eq!(field("tags[]").count, 2);
        let qty = field("items[].qty");
        assert_eq!(qty.count, 1);
        assert_eq!(qty.parent_count, 2);
        assert_eq!(field("name").top_values[0].1, 1);
        assert_eq!(report.fields[0].path, "name");
    }
}
