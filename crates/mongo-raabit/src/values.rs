//! Display helpers for BSON values: compact previews and syntax-colored shell text.

use crate::style::Palette;
use egui::text::LayoutJob;
use egui::{Color32, FontId, TextFormat};
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::shell::{self, ShellStyle};

const MAX_PREVIEW: usize = 160;

/// Color of a value by BSON type.
pub fn color(value: &Bson, p: &Palette) -> Color32 {
    match value {
        Bson::String(_) | Bson::Symbol(_) => p.string,
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => p.number,
        Bson::Boolean(_) | Bson::Null | Bson::Undefined | Bson::MinKey | Bson::MaxKey => p.keyword,
        Bson::ObjectId(_) | Bson::Binary(_) => p.object_id,
        Bson::DateTime(_) | Bson::Timestamp(_) => p.date,
        Bson::Document(_) | Bson::Array(_) => p.text_secondary,
        _ => p.text,
    }
}

/// Single-line preview for table cells and tree rows.
pub fn preview(value: &Bson) -> String {
    let text = match value {
        Bson::String(s) => s.replace(['\n', '\r'], " "),
        Bson::Document(d) => match d.len() {
            1 => "{ 1 field }".into(),
            n => format!("{{ {n} fields }}"),
        },
        Bson::Array(a) => match a.len() {
            1 => "[ 1 element ]".into(),
            n => format!("[ {n} elements ]"),
        },
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

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Pretty mongo shell syntax with syntax coloring.
pub fn document_job(doc: &Document, p: &Palette, font: FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let text = shell::document_to_shell(doc, ShellStyle::Pretty(2));
    highlight_into(&mut job, &text, p, font);
    job
}

/// Color shell text: keys, strings, numbers, constructors and punctuation.
pub fn highlight_into(job: &mut LayoutJob, text: &str, p: &Palette, font: FontId) {
    let fmt = |color: Color32| TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let start = i;
        let line_start = start == 0 || bytes[start - 1] == b'\n' || text[..start].ends_with(' ');
        let color = if (c == b'#' && line_start) || (c == b'/' && bytes.get(i + 1) == Some(&b'/')) {
            // Line comment (code snippets): until end of line.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            p.text_info
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < bytes.len() && bytes[i] != c {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            // A quoted key is followed by ':'.
            if text[i..].trim_start().starts_with(':') {
                p.key
            } else {
                p.string
            }
        } else if c.is_ascii_digit()
            || (c == b'-' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'.'
                    || bytes[i] == b'+'
                    || bytes[i] == b'-')
            {
                i += 1;
            }
            p.number
        } else if c.is_ascii_alphabetic() || c == b'$' || c == b'_' {
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'$' || bytes[i] == b'_')
            {
                i += 1;
            }
            let word = &text[start..i];
            if text[i..].starts_with(':') || text[i..].starts_with(" :") {
                p.key
            } else if matches!(
                word,
                "true" | "false" | "null" | "undefined" | "NaN" | "Infinity"
            ) {
                p.keyword
            } else if word == "ObjectId" || word == "UUID" || word == "BinData" {
                p.object_id
            } else if word == "ISODate" || word == "Timestamp" || word == "Date" {
                p.date
            } else if word.starts_with("Number") {
                p.number
            } else {
                p.text
            }
        } else {
            // Advance one whole UTF-8 character.
            let ch = text[i..].chars().next().map_or(1, char::len_utf8);
            i += ch;
            if matches!(c, b'{' | b'}' | b'[' | b']' | b',' | b':' | b'(' | b')') {
                p.text_secondary
            } else {
                p.text
            }
        };
        job.append(&text[start..i], 0.0, fmt(color));
    }
}

const SQL_KEYWORDS: &[&str] = &[
    "SELECT",
    "DISTINCT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "IN",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "IS",
    "NULL",
    "GROUP",
    "BY",
    "HAVING",
    "ORDER",
    "ASC",
    "DESC",
    "LIMIT",
    "OFFSET",
    "JOIN",
    "INNER",
    "LEFT",
    "OUTER",
    "ON",
    "AS",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "TRUE",
    "FALSE",
    "DATE",
    "TIMESTAMP",
];

/// SQL syntax coloring (keywords, strings, numbers, comments).
pub fn sql_job(text: &str, p: &Palette, font: FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let fmt = |color: Color32| TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i];
        let color = if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            p.text_info
        } else if c == b'\'' || c == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != c {
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            if c == b'\'' { p.string } else { p.key }
        } else if c.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            p.number
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            let word = text[start..i].to_ascii_uppercase();
            if SQL_KEYWORDS.contains(&word.as_str()) {
                p.keyword
            } else {
                p.text
            }
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            p.text_secondary
        };
        job.append(&text[start..i], 0.0, fmt(color));
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongo_raabit_core::bson::{DateTime, doc};

    #[test]
    fn previews_are_compact() {
        assert_eq!(
            preview(&Bson::Document(doc! {"a": 1, "b": 2})),
            "{ 2 fields }"
        );
        assert_eq!(preview(&Bson::Array(vec![Bson::Int32(1)])), "[ 1 element ]");
        assert_eq!(
            preview(&Bson::DateTime(DateTime::from_millis(0))),
            "1970-01-01 00:00:00"
        );
        assert_eq!(preview(&Bson::String("a\nb".into())), "a b");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(preview(&Bson::Double(2.0)), "2.0");
    }

    #[test]
    fn highlighting_covers_all_text() {
        let doc = doc! {"name": "Ada", "n": 3, "ok": true, "nested": {"x": "ü"}};
        let job = document_job(&doc, &Palette::DARK, FontId::monospace(12.0));
        assert_eq!(
            job.text,
            shell::document_to_shell(&doc, ShellStyle::Pretty(2))
        );
        let key = job
            .sections
            .iter()
            .find(|s| &job.text[s.byte_range.clone()] == "name")
            .unwrap();
        assert_eq!(key.format.color, Palette::DARK.key);
        let string = job
            .sections
            .iter()
            .find(|s| &job.text[s.byte_range.clone()] == "\"Ada\"")
            .unwrap();
        assert_eq!(string.format.color, Palette::DARK.string);
    }
}
