//! Import / Export: JSON (array, lines, Extended JSON or shell syntax), CSV with a field
//! mapping, BSON (mongodump `.bson` layout) and SQL `INSERT` statements.

use crate::connection::Connection;
use crate::error::{RaabitError, Result};
use crate::shell::{self, ShellStyle};
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, Decimal128, Document, doc, oid::ObjectId};
use mongodb::error::ErrorKind;
use serde::Serialize;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};

// ----- formats ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum JsonMode {
    /// Relaxed Extended JSON (plain numbers and ISO dates).
    #[default]
    Relaxed,
    /// Canonical Extended JSON (type-preserving).
    Canonical,
    /// mongo shell syntax (`ObjectId("…")`).
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ExportFormat {
    /// One JSON array; `pretty` indents it.
    JsonArray {
        mode: JsonMode,
        pretty: bool,
    },
    /// One document per line (mongoexport default).
    JsonLines {
        mode: JsonMode,
    },
    Csv(CsvOptions),
    /// Concatenated BSON documents, readable by `mongorestore`.
    Bson,
    /// `INSERT INTO table (…) VALUES (…);` per document over the given columns.
    Sql {
        table: String,
        columns: Vec<String>,
    },
}

impl ExportFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::JsonArray { .. } => "json",
            Self::JsonLines { .. } => "jsonl",
            Self::Csv(_) => "csv",
            Self::Bson => "bson",
            Self::Sql { .. } => "sql",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CsvOptions {
    /// Dotted field paths to export, in column order.
    pub columns: Vec<String>,
    pub delimiter: u8,
    pub header: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            delimiter: b',',
            header: true,
        }
    }
}

// ----- flattening ------------------------------------------------------------------------

/// Flatten embedded documents into dotted paths; arrays stay whole.
pub fn flatten(document: &Document) -> Vec<(String, Bson)> {
    fn walk(prefix: &str, doc: &Document, out: &mut Vec<(String, Bson)>) {
        for (key, value) in doc {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            match value {
                Bson::Document(inner) if !inner.is_empty() => walk(&path, inner, out),
                other => out.push((path, other.clone())),
            }
        }
    }
    let mut out = Vec::new();
    walk("", document, &mut out);
    out
}

/// Union of flattened field paths in first-seen order (CSV/SQL column discovery).
pub fn discover_columns<'a>(documents: impl IntoIterator<Item = &'a Document>) -> Vec<String> {
    let mut columns: Vec<String> = Vec::new();
    for document in documents {
        for (path, _) in flatten(document) {
            if !columns.contains(&path) {
                columns.push(path);
            }
        }
    }
    columns
}

/// Value at a dotted path (embedded documents only).
pub fn get_path<'a>(document: &'a Document, path: &str) -> Option<&'a Bson> {
    let mut current = document;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        let value = current.get(part)?;
        if parts.peek().is_none() {
            return Some(value);
        }
        current = value.as_document()?;
    }
    None
}

/// Set a dotted path, creating embedded documents as needed.
pub fn set_path(document: &mut Document, path: &str, value: Bson) -> Result<()> {
    let mut current = document;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        if part.is_empty() {
            return Err(RaabitError::Invalid(format!("invalid field path '{path}'")));
        }
        if parts.peek().is_none() {
            current.insert(part, value);
            return Ok(());
        }
        let entry = current
            .entry(part.to_owned())
            .or_insert_with(|| Bson::Document(Document::new()));
        current = match entry {
            Bson::Document(inner) => inner,
            _ => {
                return Err(RaabitError::Invalid(format!(
                    "'{part}' in '{path}' is not an embedded document"
                )));
            }
        };
    }
    Ok(())
}

/// Text of a scalar for CSV cells.
pub fn cell_text(value: &Bson) -> String {
    match value {
        Bson::String(s) => s.clone(),
        Bson::Null | Bson::Undefined => String::new(),
        Bson::Boolean(b) => b.to_string(),
        Bson::Int32(v) => v.to_string(),
        Bson::Int64(v) => v.to_string(),
        Bson::Double(v) => v.to_string(),
        Bson::Decimal128(d) => d.to_string(),
        Bson::ObjectId(o) => o.to_hex(),
        Bson::DateTime(d) => d
            .try_to_rfc3339_string()
            .unwrap_or_else(|_| d.timestamp_millis().to_string()),
        other => other.clone().into_relaxed_extjson().to_string(),
    }
}

fn sql_literal(value: Option<&Bson>) -> String {
    match value {
        None | Some(Bson::Null) | Some(Bson::Undefined) => "NULL".into(),
        Some(Bson::Boolean(b)) => if *b { "TRUE" } else { "FALSE" }.into(),
        Some(Bson::Int32(v)) => v.to_string(),
        Some(Bson::Int64(v)) => v.to_string(),
        Some(Bson::Double(v)) if v.is_finite() => v.to_string(),
        Some(Bson::Decimal128(d)) => d.to_string(),
        Some(other) => format!("'{}'", cell_text(other).replace('\'', "''")),
    }
}

fn sql_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn json_text(document: &Document, mode: JsonMode, pretty: bool) -> Result<String> {
    let value = Bson::Document(document.clone());
    let json = match mode {
        JsonMode::Relaxed => value.into_relaxed_extjson(),
        JsonMode::Canonical => value.into_canonical_extjson(),
        JsonMode::Shell => {
            return Ok(shell::to_shell_string(
                &value,
                if pretty {
                    ShellStyle::Pretty(2)
                } else {
                    ShellStyle::Compact
                },
            ));
        }
    };
    if pretty {
        serde_json::to_string_pretty(&json)
    } else {
        serde_json::to_string(&json)
    }
    .map_err(|e| RaabitError::Serde(e.to_string()))
}

// ----- export ----------------------------------------------------------------------------

/// Streaming writer for one export file.
pub struct Exporter<W: Write> {
    writer: W,
    format: ExportFormat,
    count: u64,
}

/// One CSV record with proper quoting, including the line terminator.
fn csv_line(fields: &[String], delimiter: u8) -> Result<Vec<u8>> {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(Vec::new());
    writer
        .write_record(fields)
        .map_err(|e| RaabitError::Serde(e.to_string()))?;
    writer
        .into_inner()
        .map_err(|e| RaabitError::Serde(e.to_string()))
}

impl<W: Write> Exporter<W> {
    pub fn new(mut writer: W, format: ExportFormat) -> Result<Self> {
        match &format {
            ExportFormat::Csv(options) => {
                if options.columns.is_empty() {
                    return Err(RaabitError::Invalid(
                        "choose at least one CSV column".into(),
                    ));
                }
                if options.header {
                    writer.write_all(&csv_line(&options.columns, options.delimiter)?)?;
                }
            }
            ExportFormat::JsonArray { .. } => writer.write_all(b"[")?,
            ExportFormat::Sql { columns, .. } if columns.is_empty() => {
                return Err(RaabitError::Invalid(
                    "choose at least one SQL column".into(),
                ));
            }
            _ => {}
        }
        Ok(Self {
            writer,
            format,
            count: 0,
        })
    }

    pub fn write(&mut self, document: &Document) -> Result<()> {
        match &self.format {
            ExportFormat::JsonArray { mode, pretty } => {
                if self.count > 0 {
                    self.writer.write_all(b",")?;
                }
                if *pretty {
                    self.writer.write_all(b"\n")?;
                    let text = json_text(document, *mode, true)?;
                    for (i, line) in text.lines().enumerate() {
                        if i > 0 {
                            self.writer.write_all(b"\n")?;
                        }
                        self.writer.write_all(b"  ")?;
                        self.writer.write_all(line.as_bytes())?;
                    }
                } else {
                    self.writer
                        .write_all(json_text(document, *mode, false)?.as_bytes())?;
                }
            }
            ExportFormat::JsonLines { mode } => {
                self.writer
                    .write_all(json_text(document, *mode, false)?.as_bytes())?;
                self.writer.write_all(b"\n")?;
            }
            ExportFormat::Csv(options) => {
                let record: Vec<String> = options
                    .columns
                    .iter()
                    .map(|c| get_path(document, c).map(cell_text).unwrap_or_default())
                    .collect();
                self.writer
                    .write_all(&csv_line(&record, options.delimiter)?)?;
            }
            ExportFormat::Bson => {
                let mut bytes = Vec::new();
                document
                    .to_writer(&mut bytes)
                    .map_err(|e| RaabitError::Serde(e.to_string()))?;
                self.writer.write_all(&bytes)?;
            }
            ExportFormat::Sql { table, columns } => {
                let names: Vec<String> = columns.iter().map(|c| sql_ident(c)).collect();
                let values: Vec<String> = columns
                    .iter()
                    .map(|c| sql_literal(get_path(document, c)))
                    .collect();
                writeln!(
                    self.writer,
                    "INSERT INTO {} ({}) VALUES ({});",
                    sql_ident(table),
                    names.join(", "),
                    values.join(", ")
                )?;
            }
        }
        self.count += 1;
        Ok(())
    }

    /// Close the file; returns the number of documents written.
    pub fn finish(self) -> Result<u64> {
        self.into_inner().map(|(_, count)| count)
    }

    /// Close the export and hand back the writer.
    pub fn into_inner(mut self) -> Result<(W, u64)> {
        if let ExportFormat::JsonArray { pretty, .. } = self.format {
            self.writer.write_all(if pretty && self.count > 0 {
                b"\n]\n"
            } else {
                b"]\n"
            })?;
        }
        self.writer.flush()?;
        Ok((self.writer, self.count))
    }
}

/// Where exported documents come from.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // built once per export
pub enum ExportSource {
    Query(crate::query::Query),
    Aggregate(Vec<Document>),
}

// ----- import ----------------------------------------------------------------------------

/// Read JSON: an array, JSON Lines or concatenated documents, in Extended JSON or
/// mongo shell syntax. Calls `sink` for every document.
pub fn read_json(
    mut reader: impl BufRead,
    mut sink: impl FnMut(Document) -> Result<()>,
) -> Result<u64> {
    let mut text = String::new();
    reader.read_to_string(&mut text)?;
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    let mut count = 0;
    let mut emit = |value: serde_json::Value, count: &mut u64| -> Result<()> {
        match Bson::try_from(value) {
            Ok(Bson::Document(doc)) => {
                *count += 1;
                sink(doc)
            }
            Ok(other) => Err(RaabitError::Invalid(format!(
                "document {} is a {}, not an object",
                *count + 1,
                shell::type_name(&other)
            ))),
            Err(e) => Err(RaabitError::Invalid(format!(
                "document {}: invalid Extended JSON: {e}",
                *count + 1
            ))),
        }
    };
    // Strict JSON first (fast, streaming); fall back to shell syntax.
    let stream = serde_json::Deserializer::from_str(trimmed).into_iter::<serde_json::Value>();
    let mut values = Vec::new();
    let mut strict = true;
    for value in stream {
        match value {
            Ok(v) => values.push(v),
            Err(_) => {
                strict = false;
                break;
            }
        }
    }
    if strict {
        for value in values {
            match value {
                serde_json::Value::Array(items) => {
                    for item in items {
                        emit(item, &mut count)?;
                    }
                }
                other => emit(other, &mut count)?,
            }
        }
        return Ok(count);
    }
    let docs = if trimmed.starts_with('[') {
        shell::parse_document_array(trimmed).map_err(|e| RaabitError::parse("JSON", e))?
    } else {
        trimmed
            .lines()
            .filter(|l| !l.trim().is_empty())
            .enumerate()
            .map(|(i, line)| {
                shell::parse_document(line)
                    .map_err(|e| RaabitError::Invalid(format!("line {}: {e}", i + 1)))
            })
            .collect::<Result<Vec<_>>>()?
    };
    let mut count = 0;
    for doc in docs {
        count += 1;
        sink(doc)?;
    }
    Ok(count)
}

/// Read concatenated BSON documents (mongodump `.bson`).
pub fn read_bson(
    mut reader: impl Read,
    mut sink: impl FnMut(Document) -> Result<()>,
) -> Result<u64> {
    let mut count = 0;
    loop {
        let mut len_bytes = [0u8; 4];
        match reader.read_exact(&mut len_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(count),
            Err(e) => return Err(e.into()),
        }
        let len = i32::from_le_bytes(len_bytes);
        if !(5..=48 * 1024 * 1024).contains(&len) {
            return Err(RaabitError::Invalid(format!(
                "document {} has an invalid BSON length {len}",
                count + 1
            )));
        }
        let mut bytes = len_bytes.to_vec();
        bytes.resize(len as usize, 0);
        reader
            .read_exact(&mut bytes[4..])
            .map_err(|_| RaabitError::Invalid(format!("document {} is truncated", count + 1)))?;
        let doc = Document::from_reader(bytes.as_slice()).map_err(|e| {
            RaabitError::Invalid(format!("document {} is not valid BSON: {e}", count + 1))
        })?;
        count += 1;
        sink(doc)?;
    }
}

/// Target type of a CSV column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum FieldType {
    /// Infer per cell: integer, double, boolean, ISO date, otherwise string.
    #[default]
    Auto,
    String,
    Int32,
    Int64,
    Double,
    Decimal,
    Boolean,
    Date,
    ObjectId,
    /// Cell holds JSON / shell syntax (arrays, embedded documents).
    Json,
    /// Do not import the column.
    Skip,
}

impl FieldType {
    pub const ALL: [Self; 11] = [
        Self::Auto,
        Self::String,
        Self::Int32,
        Self::Int64,
        Self::Double,
        Self::Decimal,
        Self::Boolean,
        Self::Date,
        Self::ObjectId,
        Self::Json,
        Self::Skip,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::String => "String",
            Self::Int32 => "Int32",
            Self::Int64 => "Int64",
            Self::Double => "Double",
            Self::Decimal => "Decimal128",
            Self::Boolean => "Boolean",
            Self::Date => "Date",
            Self::ObjectId => "ObjectId",
            Self::Json => "JSON",
            Self::Skip => "Skip",
        }
    }
}

/// How one CSV column maps into documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldMapping {
    pub column: String,
    /// Dotted target path.
    pub target: String,
    pub kind: FieldType,
}

/// Headers and the first rows of a CSV file, for the mapping step of the wizard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CsvPreview {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

fn csv_reader<R: Read>(reader: R, delimiter: u8, header: bool) -> csv::Reader<R> {
    csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(header)
        .flexible(true)
        .from_reader(reader)
}

pub fn csv_preview(
    reader: impl Read,
    delimiter: u8,
    header: bool,
    rows: usize,
) -> Result<CsvPreview> {
    let mut csv = csv_reader(reader, delimiter, header);
    let mut preview = CsvPreview::default();
    if header {
        preview.headers = csv
            .headers()
            .map_err(|e| RaabitError::Invalid(format!("CSV header: {e}")))?
            .iter()
            .map(|h| h.trim_start_matches('\u{feff}').to_owned())
            .collect();
    }
    for record in csv.records().take(rows) {
        let record = record.map_err(|e| RaabitError::Invalid(format!("CSV: {e}")))?;
        preview
            .rows
            .push(record.iter().map(str::to_owned).collect());
    }
    if !header {
        let width = preview.rows.iter().map(Vec::len).max().unwrap_or(0);
        preview.headers = (1..=width).map(|i| format!("field{i}")).collect();
    }
    Ok(preview)
}

/// Default mapping: same names, `Auto` types, `_id` columns typed as ObjectId when every
/// sampled value is 24 hex characters.
pub fn default_mapping(preview: &CsvPreview) -> Vec<FieldMapping> {
    preview
        .headers
        .iter()
        .enumerate()
        .map(|(i, header)| {
            let all_oids = !preview.rows.is_empty()
                && preview.rows.iter().all(|row| {
                    row.get(i)
                        .is_some_and(|cell| ObjectId::parse_str(cell).is_ok())
                });
            FieldMapping {
                column: header.clone(),
                target: header.clone(),
                kind: if all_oids {
                    FieldType::ObjectId
                } else {
                    FieldType::Auto
                },
            }
        })
        .collect()
}

/// Convert one cell. Empty cells yield `None` (field omitted) except for strings.
pub fn convert_cell(cell: &str, kind: FieldType) -> Result<Option<Bson>> {
    let fail = |what: &str| RaabitError::Invalid(format!("'{cell}' is not a valid {what}"));
    if cell.is_empty() && kind != FieldType::String {
        return Ok(None);
    }
    Ok(Some(match kind {
        FieldType::Skip => return Ok(None),
        FieldType::String => Bson::String(cell.to_owned()),
        FieldType::Int32 => Bson::Int32(cell.trim().parse().map_err(|_| fail("Int32"))?),
        FieldType::Int64 => Bson::Int64(cell.trim().parse().map_err(|_| fail("Int64"))?),
        FieldType::Double => Bson::Double(cell.trim().parse().map_err(|_| fail("Double"))?),
        FieldType::Decimal => Bson::Decimal128(
            cell.trim()
                .parse::<Decimal128>()
                .map_err(|_| fail("Decimal128"))?,
        ),
        FieldType::Boolean => match cell.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "y" => Bson::Boolean(true),
            "false" | "0" | "no" | "n" => Bson::Boolean(false),
            _ => return Err(fail("Boolean")),
        },
        FieldType::Date => Bson::DateTime(shell::parse_date(cell).map_err(RaabitError::Invalid)?),
        FieldType::ObjectId => {
            Bson::ObjectId(ObjectId::parse_str(cell.trim()).map_err(|_| fail("ObjectId"))?)
        }
        FieldType::Json => {
            shell::parse_value(cell).map_err(|e| RaabitError::parse("JSON cell", e))?
        }
        FieldType::Auto => {
            let t = cell.trim();
            if let Ok(i) = t.parse::<i64>() {
                i32::try_from(i).map(Bson::Int32).unwrap_or(Bson::Int64(i))
            } else if let Ok(f) = t.parse::<f64>()
                && f.is_finite()
                && t.chars().any(|c| c.is_ascii_digit())
            {
                Bson::Double(f)
            } else if t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("false") {
                Bson::Boolean(t.eq_ignore_ascii_case("true"))
            } else if t.len() >= 10
                && t.as_bytes()[4] == b'-'
                && t.as_bytes()[7] == b'-'
                && let Ok(date) = shell::parse_date(t)
            {
                Bson::DateTime(date)
            } else {
                Bson::String(cell.to_owned())
            }
        }
    }))
}

/// Read CSV rows into documents using `mapping` (columns matched by header name, or by
/// position as `field1…` when the file has no header).
pub fn read_csv(
    reader: impl Read,
    delimiter: u8,
    header: bool,
    mapping: &[FieldMapping],
    mut sink: impl FnMut(Document) -> Result<()>,
) -> Result<u64> {
    let mut csv = csv_reader(reader, delimiter, header);
    let headers: Vec<String> = if header {
        csv.headers()
            .map_err(|e| RaabitError::Invalid(format!("CSV header: {e}")))?
            .iter()
            .map(|h| h.trim_start_matches('\u{feff}').to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let index_of = |column: &str| -> Option<usize> {
        if header {
            headers.iter().position(|h| h == column)
        } else {
            column
                .strip_prefix("field")?
                .parse::<usize>()
                .ok()?
                .checked_sub(1)
        }
    };
    let plan: Vec<(usize, &FieldMapping)> = mapping
        .iter()
        .filter(|m| m.kind != FieldType::Skip && !m.target.is_empty())
        .map(|m| {
            index_of(&m.column)
                .map(|i| (i, m))
                .ok_or_else(|| RaabitError::Invalid(format!("column '{}' not found", m.column)))
        })
        .collect::<Result<_>>()?;
    let mut count = 0;
    for (row, record) in csv.records().enumerate() {
        let line = row + if header { 2 } else { 1 };
        let record = record.map_err(|e| RaabitError::Invalid(format!("CSV line {line}: {e}")))?;
        let mut doc = Document::new();
        for (index, mapping) in &plan {
            let cell = record.get(*index).unwrap_or("");
            let value = convert_cell(cell, mapping.kind).map_err(|e| {
                RaabitError::Invalid(format!("CSV line {line}, column '{}': {e}", mapping.column))
            })?;
            if let Some(value) = value {
                set_path(&mut doc, &mapping.target, value)?;
            }
        }
        count += 1;
        sink(doc)?;
    }
    Ok(count)
}

// ----- server side -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum ImportMode {
    /// Insert; duplicates are reported as failures and the rest continue.
    #[default]
    Insert,
    /// Replace documents with the same `_id`, insert the others.
    Upsert,
    /// Drop the collection first.
    DropAndInsert,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ImportReport {
    pub inserted: u64,
    pub upserted: u64,
    pub replaced: u64,
    /// (0-based document index, message); capped at 100 entries.
    pub failures: Vec<(u64, String)>,
    pub failed: u64,
    pub cancelled: bool,
}

const BATCH: usize = 1000;
const MAX_REPORTED_FAILURES: usize = 100;

impl Connection {
    /// Stream query or aggregation results into `exporter`.
    pub async fn export<W: Write>(
        &self,
        db: &str,
        collection: &str,
        source: &ExportSource,
        exporter: &mut Exporter<W>,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<u64> {
        let handle = self.collection(db, collection);
        let mut cursor = match source {
            ExportSource::Query(query) => {
                let mut find = handle.find(query.filter.clone());
                if let Some(projection) = &query.projection {
                    find = find.projection(projection.clone());
                }
                if let Some(sort) = &query.sort {
                    find = find.sort(sort.clone());
                }
                if query.skip > 0 {
                    find = find.skip(query.skip);
                }
                if query.limit > 0 {
                    find = find.limit(i64::try_from(query.limit).unwrap_or(i64::MAX));
                }
                find.await?
            }
            ExportSource::Aggregate(pipeline) => {
                handle
                    .aggregate(pipeline.clone())
                    .allow_disk_use(true)
                    .await?
            }
        };
        let mut count = 0;
        while let Some(document) = cursor.try_next().await? {
            if cancel.load(Ordering::Relaxed) {
                return Err(RaabitError::Invalid("export cancelled".into()));
            }
            exporter.write(&document)?;
            count += 1;
            if count % 500 == 0 {
                progress(count);
            }
        }
        progress(count);
        Ok(count)
    }

    /// Write documents in batches of 1000, continuing past individual failures.
    pub async fn import(
        &self,
        db: &str,
        collection: &str,
        documents: Vec<Document>,
        mode: ImportMode,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<ImportReport> {
        let handle = self.collection(db, collection);
        if mode == ImportMode::DropAndInsert {
            handle.drop().await?;
        }
        let mut report = ImportReport::default();
        let fail = |report: &mut ImportReport, index: u64, message: String| {
            report.failed += 1;
            if report.failures.len() < MAX_REPORTED_FAILURES {
                report.failures.push((index, message));
            }
        };
        let mut offset = 0u64;
        for batch in documents.chunks(BATCH) {
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                break;
            }
            match mode {
                ImportMode::Insert | ImportMode::DropAndInsert => {
                    match handle.insert_many(batch.to_vec()).ordered(false).await {
                        Ok(result) => report.inserted += result.inserted_ids.len() as u64,
                        Err(error) => match *error.kind {
                            ErrorKind::InsertMany(ref failure) => {
                                let errors = failure.write_errors.clone().unwrap_or_default();
                                report.inserted += (batch.len() - errors.len()) as u64;
                                for e in errors {
                                    fail(&mut report, offset + e.index as u64, e.message);
                                }
                            }
                            _ => return Err(error.into()),
                        },
                    }
                }
                ImportMode::Upsert => {
                    for (i, document) in batch.iter().enumerate() {
                        let index = offset + i as u64;
                        let result = match document.get("_id") {
                            Some(id) => handle
                                .replace_one(doc! {"_id": id.clone()}, document.clone())
                                .upsert(true)
                                .await
                                .map(|r| (r.matched_count, r.upserted_id.is_some())),
                            None => handle.insert_one(document.clone()).await.map(|_| (0, true)),
                        };
                        match result {
                            Ok((matched, upserted)) => {
                                report.replaced += matched;
                                report.upserted += u64::from(upserted);
                            }
                            Err(e) => fail(&mut report, index, e.to_string()),
                        }
                    }
                }
            }
            offset += batch.len() as u64;
            progress(offset);
        }
        Ok(report)
    }
}

/// Convenience: serialize documents to an in-memory buffer.
pub fn export_to_vec<'a>(
    documents: impl IntoIterator<Item = &'a Document>,
    format: ExportFormat,
) -> Result<Vec<u8>> {
    let mut exporter = Exporter::new(Vec::new(), format)?;
    for document in documents {
        exporter.write(document)?;
    }
    Ok(exporter.into_inner()?.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::DateTime;

    fn sample() -> Vec<Document> {
        vec![
            doc! {"_id": 1, "name": "Ada", "address": {"city": "London", "zip": "N1"}, "tags": ["a", "b"], "at": DateTime::from_millis(0)},
            doc! {"_id": 2, "name": "O'Neil, \"Q\"", "score": 2.5, "ok": true},
        ]
    }

    #[test]
    fn json_round_trips_in_every_mode() {
        for format in [
            ExportFormat::JsonArray {
                mode: JsonMode::Relaxed,
                pretty: true,
            },
            ExportFormat::JsonArray {
                mode: JsonMode::Canonical,
                pretty: false,
            },
            ExportFormat::JsonLines {
                mode: JsonMode::Relaxed,
            },
            ExportFormat::JsonLines {
                mode: JsonMode::Shell,
            },
            ExportFormat::JsonArray {
                mode: JsonMode::Shell,
                pretty: true,
            },
        ] {
            let bytes = export_to_vec(&sample(), format.clone()).unwrap();
            let mut back = Vec::new();
            read_json(bytes.as_slice(), |d| {
                back.push(d);
                Ok(())
            })
            .unwrap();
            assert_eq!(
                back,
                sample(),
                "{format:?}\n{}",
                String::from_utf8_lossy(&bytes)
            );
        }
        let mut n = 0;
        assert_eq!(read_json("".as_bytes(), |_| Ok(())).unwrap(), 0);
        assert!(read_json("[1, 2]".as_bytes(), |_| Ok(())).is_err());
        read_json(
            "{a: ObjectId('5f1d7f1e2b3c4d5e6f708192')}\n{b: 2}\n".as_bytes(),
            |_| {
                n += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn bson_round_trip_and_corruption() {
        let bytes = export_to_vec(&sample(), ExportFormat::Bson).unwrap();
        let mut back = Vec::new();
        read_bson(bytes.as_slice(), |d| {
            back.push(d);
            Ok(())
        })
        .unwrap();
        assert_eq!(back, sample());
        assert!(read_bson(&bytes[..bytes.len() - 3], |_| Ok(())).is_err());
        assert!(read_bson(&[1u8, 0, 0, 0][..], |_| Ok(())).is_err());
    }

    #[test]
    fn csv_export_import_with_mapping() {
        let docs = sample();
        let columns = discover_columns(&docs);
        assert_eq!(
            columns,
            [
                "_id",
                "name",
                "address.city",
                "address.zip",
                "tags",
                "at",
                "score",
                "ok"
            ]
        );
        let bytes = export_to_vec(
            &docs,
            ExportFormat::Csv(CsvOptions {
                columns: columns.clone(),
                ..CsvOptions::default()
            }),
        )
        .unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.starts_with(
                "_id,name,address.city,address.zip,tags,at,score,ok\n1,Ada,London,N1,"
            )
        );
        assert!(text.contains("\"O'Neil, \"\"Q\"\"\""));

        let preview = csv_preview(bytes.as_slice(), b',', true, 10).unwrap();
        assert_eq!(preview.headers, columns);
        let mut mapping = default_mapping(&preview);
        mapping[4].kind = FieldType::Json;
        mapping[3].kind = FieldType::Skip;
        let mut back = Vec::new();
        read_csv(bytes.as_slice(), b',', true, &mapping, |d| {
            back.push(d);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            back[0],
            doc! {"_id": 1, "name": "Ada", "address": {"city": "London"}, "tags": ["a", "b"], "at": DateTime::from_millis(0)}
        );
        assert_eq!(
            back[1],
            doc! {"_id": 2, "name": "O'Neil, \"Q\"", "score": 2.5, "ok": true}
        );

        mapping[0].kind = FieldType::Boolean;
        let err = read_csv(bytes.as_slice(), b',', true, &mapping, |_| Ok(())).unwrap_err();
        assert_eq!(
            err.to_string(),
            "CSV line 3, column '_id': '2' is not a valid Boolean"
        );
    }

    #[test]
    fn headerless_csv_and_cells() {
        let data = "x;1\ny;2\n";
        let preview = csv_preview(data.as_bytes(), b';', false, 5).unwrap();
        assert_eq!(preview.headers, ["field1", "field2"]);
        let mut back = Vec::new();
        read_csv(
            data.as_bytes(),
            b';',
            false,
            &default_mapping(&preview),
            |d| {
                back.push(d);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(back[1], doc! {"field1": "y", "field2": 2});
        assert_eq!(convert_cell("", FieldType::Int32).unwrap(), None);
        assert_eq!(
            convert_cell("", FieldType::String).unwrap(),
            Some(Bson::String(String::new()))
        );
        assert_eq!(
            convert_cell("2024-05-01", FieldType::Auto).unwrap(),
            Some(Bson::DateTime(
                DateTime::parse_rfc3339_str("2024-05-01T00:00:00Z").unwrap()
            ))
        );
        assert_eq!(
            convert_cell("12345678901", FieldType::Auto).unwrap(),
            Some(Bson::Int64(12_345_678_901))
        );
        assert_eq!(
            convert_cell("NaN", FieldType::Auto).unwrap(),
            Some(Bson::String("NaN".into()))
        );
        let mut d = doc! {"a": 1};
        assert!(set_path(&mut d, "a.b", Bson::Int32(1)).is_err());
        assert!(set_path(&mut d, "x..y", Bson::Int32(1)).is_err());
    }

    #[test]
    fn sql_inserts() {
        let bytes = export_to_vec(
            &sample(),
            ExportFormat::Sql {
                table: "people".into(),
                columns: vec![
                    "_id".into(),
                    "name".into(),
                    "address.city".into(),
                    "ok".into(),
                ],
            },
        )
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text,
            "INSERT INTO \"people\" (\"_id\", \"name\", \"address.city\", \"ok\") VALUES (1, 'Ada', 'London', NULL);\n\
             INSERT INTO \"people\" (\"_id\", \"name\", \"address.city\", \"ok\") VALUES (2, 'O''Neil, \"Q\"', NULL, TRUE);\n"
        );
        assert!(Exporter::new(Vec::new(), ExportFormat::Csv(CsvOptions::default())).is_err());
    }
}
