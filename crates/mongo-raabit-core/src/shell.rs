//! Parser and printer for the mongo shell literal syntax.
//!
//! Users type filters, projections, documents and pipelines the way they would in
//! `mongosh`: unquoted keys, single-quoted strings, trailing commas, comments, regular
//! expression literals and BSON constructors such as `ObjectId('…')` or `ISODate('…')`.
//! Canonical and relaxed Extended JSON (`{"$oid": "…"}`) is accepted as well, so strict
//! JSON is a subset of what this parser understands.

use mongodb::bson::{
    self, Binary, Bson, DateTime, Decimal128, Document, Regex, Timestamp, oid::ObjectId,
    spec::BinarySubtype,
};
use std::fmt::Write as _;

/// A parse failure with the byte offset where it happened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} at position {offset}")]
pub struct ShellParseError {
    pub message: String,
    pub offset: usize,
}

/// Parse a document literal. Blank input is the empty document `{}`.
pub fn parse_document(input: &str) -> Result<Document, ShellParseError> {
    if input.trim().is_empty() {
        return Ok(Document::new());
    }
    match parse_value(input)? {
        Bson::Document(doc) => Ok(doc),
        other => Err(ShellParseError {
            message: format!("expected a document, found {}", type_name(&other)),
            offset: 0,
        }),
    }
}

/// Parse an array of documents, e.g. an aggregation pipeline or an insert batch.
/// A single document is accepted as a one-element array.
pub fn parse_document_array(input: &str) -> Result<Vec<Document>, ShellParseError> {
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }
    match parse_value(input)? {
        Bson::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(index, item)| match item {
                Bson::Document(doc) => Ok(doc),
                other => Err(ShellParseError {
                    message: format!(
                        "element {index} must be a document, found {}",
                        type_name(&other)
                    ),
                    offset: 0,
                }),
            })
            .collect(),
        Bson::Document(doc) => Ok(vec![doc]),
        other => Err(ShellParseError {
            message: format!(
                "expected an array of documents, found {}",
                type_name(&other)
            ),
            offset: 0,
        }),
    }
}

/// Parse any single shell value.
pub fn parse_value(input: &str) -> Result<Bson, ShellParseError> {
    let mut parser = Parser {
        src: input,
        bytes: input.as_bytes(),
        pos: 0,
        depth: 0,
    };
    let value = parser.value()?;
    parser.skip_ws()?;
    if parser.pos < parser.bytes.len() {
        return Err(parser.error("unexpected trailing input"));
    }
    Ok(value)
}

/// Human readable BSON type name, as shown in the type column of result views.
pub fn type_name(value: &Bson) -> &'static str {
    match value {
        Bson::Double(_) => "Double",
        Bson::String(_) => "String",
        Bson::Array(_) => "Array",
        Bson::Document(_) => "Object",
        Bson::Boolean(_) => "Boolean",
        Bson::Null => "Null",
        Bson::RegularExpression(_) => "Regex",
        Bson::JavaScriptCode(_) => "JavaScript",
        Bson::JavaScriptCodeWithScope(_) => "JavaScript (scope)",
        Bson::Int32(_) => "Int32",
        Bson::Int64(_) => "Int64",
        Bson::Timestamp(_) => "Timestamp",
        Bson::Binary(_) => "Binary",
        Bson::ObjectId(_) => "ObjectId",
        Bson::DateTime(_) => "Date",
        Bson::Symbol(_) => "Symbol",
        Bson::Decimal128(_) => "Decimal128",
        Bson::Undefined => "Undefined",
        Bson::MaxKey => "MaxKey",
        Bson::MinKey => "MinKey",
        Bson::DbPointer(_) => "DBPointer",
    }
}

const MAX_DEPTH: usize = 200;

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, message: impl Into<String>) -> ShellParseError {
        ShellParseError {
            message: message.into(),
            offset: self.pos,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) -> Result<(), ShellParseError> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.pos += 1,
                Some(b'/') if self.bytes.get(self.pos + 1) == Some(&b'/') => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.bytes.get(self.pos + 1) == Some(&b'*') => {
                    let start = self.pos;
                    self.pos += 2;
                    loop {
                        match self.peek() {
                            None => {
                                self.pos = start;
                                return Err(self.error("unterminated comment"));
                            }
                            Some(b'*') if self.bytes.get(self.pos + 1) == Some(&b'/') => {
                                self.pos += 2;
                                break;
                            }
                            Some(_) => self.pos += 1,
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ShellParseError> {
        self.skip_ws()?;
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected '{}'", byte as char)))
        }
    }

    fn value(&mut self) -> Result<Bson, ShellParseError> {
        self.skip_ws()?;
        match self.peek() {
            None => Err(self.error("unexpected end of input")),
            Some(b'{') => self.nested(Self::object),
            Some(b'[') => self.nested(Self::array),
            Some(b'"' | b'\'') => Ok(Bson::String(self.string()?)),
            Some(b'/') => self.regex(),
            Some(b'-' | b'+' | b'.' | b'0'..=b'9') => self.number(),
            Some(c) if is_ident_start(c) => self.word(),
            Some(c) => Err(self.error(format!("unexpected character '{}'", c as char))),
        }
    }

    fn nested(
        &mut self,
        f: fn(&mut Self) -> Result<Bson, ShellParseError>,
    ) -> Result<Bson, ShellParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("nesting too deep"));
        }
        let result = f(self);
        self.depth -= 1;
        result
    }

    fn object(&mut self) -> Result<Bson, ShellParseError> {
        self.pos += 1; // '{'
        let mut doc = Document::new();
        loop {
            self.skip_ws()?;
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                None => return Err(self.error("unterminated document, expected '}'")),
                _ => {}
            }
            let key = match self.peek() {
                Some(b'"' | b'\'') => self.string()?,
                Some(c) if is_ident_start(c) || c.is_ascii_digit() => self.key_word(),
                _ => return Err(self.error("expected a field name")),
            };
            self.expect(b':')?;
            let value = self.value()?;
            doc.insert(key, value);
            self.skip_ws()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {}
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
        Ok(decode_extended_json(doc))
    }

    fn array(&mut self) -> Result<Bson, ShellParseError> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_ws()?;
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                None => return Err(self.error("unterminated array, expected ']'")),
                _ => {}
            }
            items.push(self.value()?);
            self.skip_ws()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {}
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
        Ok(Bson::Array(items))
    }

    fn key_word(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_ident_char(c) || c == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.src[start..self.pos].to_owned()
    }

    fn string(&mut self) -> Result<String, ShellParseError> {
        let quote = self.bytes[self.pos];
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                self.pos = start;
                return Err(self.error("unterminated string"));
            };
            if c == quote {
                self.pos += 1;
                return Ok(out);
            }
            if c == b'\\' {
                self.pos += 1;
                let Some(esc) = self.peek() else {
                    return Err(self.error("unterminated escape"));
                };
                self.pos += 1;
                match esc {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'0' => out.push('\0'),
                    b'u' => out.push(self.unicode_escape()?),
                    _ => {
                        // Any other escaped character stands for itself (\" \' \\ \/).
                        self.pos -= 1;
                        let ch = self.src[self.pos..].chars().next().unwrap_or('\\');
                        self.pos += ch.len_utf8();
                        out.push(ch);
                    }
                }
                continue;
            }
            let ch = self.src[self.pos..].chars().next().unwrap_or('\u{fffd}');
            self.pos += ch.len_utf8();
            out.push(ch);
        }
    }

    fn hex4(&mut self) -> Result<u32, ShellParseError> {
        let digits = self
            .src
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.error("incomplete \\u escape"))?;
        let value =
            u32::from_str_radix(digits, 16).map_err(|_| self.error("invalid \\u escape"))?;
        self.pos += 4;
        Ok(value)
    }

    fn unicode_escape(&mut self) -> Result<char, ShellParseError> {
        let first = self.hex4()?;
        if (0xD800..0xDC00).contains(&first) && self.src[self.pos..].starts_with("\\u") {
            self.pos += 2;
            let second = self.hex4()?;
            let code = 0x10000 + ((first - 0xD800) << 10) + (second.wrapping_sub(0xDC00) & 0x3FF);
            return char::from_u32(code).ok_or_else(|| self.error("invalid surrogate pair"));
        }
        Ok(char::from_u32(first).unwrap_or('\u{fffd}'))
    }

    fn regex(&mut self) -> Result<Bson, ShellParseError> {
        let start = self.pos;
        self.pos += 1;
        let mut pattern = String::new();
        let mut in_class = false;
        loop {
            let Some(c) = self.peek() else {
                self.pos = start;
                return Err(self.error("unterminated regular expression"));
            };
            match c {
                b'\\' => {
                    pattern.push('\\');
                    self.pos += 1;
                    if let Some(ch) = self.src[self.pos..].chars().next() {
                        pattern.push(ch);
                        self.pos += ch.len_utf8();
                    }
                    continue;
                }
                b'[' => in_class = true,
                b']' => in_class = false,
                b'/' if !in_class => {
                    self.pos += 1;
                    break;
                }
                b'\n' => {
                    self.pos = start;
                    return Err(self.error("unterminated regular expression"));
                }
                _ => {}
            }
            let ch = self.src[self.pos..].chars().next().unwrap_or('\u{fffd}');
            pattern.push(ch);
            self.pos += ch.len_utf8();
        }
        let flag_start = self.pos;
        while matches!(self.peek(), Some(b'a'..=b'z')) {
            self.pos += 1;
        }
        let mut flags: Vec<char> = self.src[flag_start..self.pos].chars().collect();
        if let Some(bad) = flags.iter().find(|f| !"imsxlu".contains(**f)) {
            return Err(self.error(format!("unsupported regex flag '{bad}'")));
        }
        flags.sort_unstable();
        flags.dedup();
        Ok(Bson::RegularExpression(Regex {
            pattern,
            options: flags.into_iter().collect(),
        }))
    }

    fn number(&mut self) -> Result<Bson, ShellParseError> {
        let start = self.pos;
        if matches!(self.peek(), Some(b'-' | b'+')) {
            self.pos += 1;
        }
        if self.src[self.pos..].starts_with("Infinity") {
            self.pos += "Infinity".len();
            let negative = self.bytes[start] == b'-';
            return Ok(Bson::Double(if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }));
        }
        if self.src[self.pos..].starts_with("0x") || self.src[self.pos..].starts_with("0X") {
            self.pos += 2;
            let digits_start = self.pos;
            while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            let value = i64::from_str_radix(&self.src[digits_start..self.pos], 16)
                .map_err(|_| self.error("invalid hexadecimal number"))?;
            let value = if self.bytes[start] == b'-' {
                -value
            } else {
                value
            };
            return Ok(integer(value));
        }
        let mut is_float = false;
        while let Some(c) = self.peek() {
            match c {
                b'0'..=b'9' | b'_' => self.pos += 1,
                b'.' | b'e' | b'E' => {
                    is_float = true;
                    self.pos += 1;
                    if matches!(c, b'e' | b'E') && matches!(self.peek(), Some(b'-' | b'+')) {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        let text: String = self.src[start..self.pos]
            .chars()
            .filter(|c| *c != '_' && *c != '+')
            .collect();
        if is_float {
            text.parse::<f64>()
                .map(Bson::Double)
                .map_err(|_| self.error(format!("invalid number '{text}'")))
        } else {
            match text.parse::<i64>() {
                Ok(value) => Ok(integer(value)),
                Err(_) => text
                    .parse::<f64>()
                    .map(Bson::Double)
                    .map_err(|_| self.error(format!("invalid number '{text}'"))),
            }
        }
    }

    fn identifier(&mut self) -> &str {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if is_ident_char(c)) {
            self.pos += 1;
        }
        &self.src[start..self.pos]
    }

    fn word(&mut self) -> Result<Bson, ShellParseError> {
        let start = self.pos;
        let mut name = self.identifier().to_owned();
        if name == "new" {
            self.skip_ws()?;
            name = self.identifier().to_owned();
        }
        match name.as_str() {
            "true" => return Ok(Bson::Boolean(true)),
            "false" => return Ok(Bson::Boolean(false)),
            "null" => return Ok(Bson::Null),
            "undefined" => return Ok(Bson::Undefined),
            "NaN" => return Ok(Bson::Double(f64::NAN)),
            "Infinity" => return Ok(Bson::Double(f64::INFINITY)),
            _ => {}
        }
        self.skip_ws()?;
        if self.peek() != Some(b'(') {
            self.pos = start;
            return Err(self.error(format!("unknown identifier '{name}'")));
        }
        self.pos += 1;
        let mut args = Vec::new();
        loop {
            self.skip_ws()?;
            match self.peek() {
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                None => return Err(self.error("unterminated argument list")),
                _ => {}
            }
            args.push(self.value()?);
            self.skip_ws()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b')') => {}
                _ => return Err(self.error("expected ',' or ')'")),
            }
        }
        let at = start;
        self.construct(&name, args)
            .map_err(|message| ShellParseError {
                message,
                offset: at,
            })
    }

    fn construct(&self, name: &str, args: Vec<Bson>) -> Result<Bson, String> {
        let text_arg = |index: usize| -> Option<String> {
            match args.get(index) {
                Some(Bson::String(s)) => Some(s.clone()),
                Some(Bson::Int32(v)) => Some(v.to_string()),
                Some(Bson::Int64(v)) => Some(v.to_string()),
                Some(Bson::Double(v)) => Some(v.to_string()),
                _ => None,
            }
        };
        match name {
            "ObjectId" | "ObjectID" => match args.first() {
                None => Ok(Bson::ObjectId(ObjectId::new())),
                Some(Bson::String(hex)) => ObjectId::parse_str(hex)
                    .map(Bson::ObjectId)
                    .map_err(|_| format!("invalid ObjectId '{hex}': expected 24 hex characters")),
                Some(_) => Err("ObjectId expects a hex string".into()),
            },
            "ISODate" | "Date" => match args.first() {
                None => Ok(Bson::DateTime(DateTime::now())),
                Some(Bson::String(text)) => parse_date(text).map(Bson::DateTime),
                Some(Bson::Int32(ms)) => Ok(Bson::DateTime(DateTime::from_millis(i64::from(*ms)))),
                Some(Bson::Int64(ms)) => Ok(Bson::DateTime(DateTime::from_millis(*ms))),
                Some(Bson::Double(ms)) => Ok(Bson::DateTime(DateTime::from_millis(*ms as i64))),
                Some(_) => Err(format!("{name} expects an ISO-8601 string or milliseconds")),
            },
            "NumberLong" | "Long" => text_arg(0)
                .and_then(|t| t.parse::<i64>().ok())
                .map(Bson::Int64)
                .ok_or_else(|| "NumberLong expects an integer".to_owned()),
            "NumberInt" | "Int32" => text_arg(0)
                .and_then(|t| t.parse::<i32>().ok())
                .map(Bson::Int32)
                .ok_or_else(|| "NumberInt expects a 32-bit integer".to_owned()),
            "Double" => text_arg(0)
                .and_then(|t| t.parse::<f64>().ok())
                .map(Bson::Double)
                .ok_or_else(|| "Double expects a number".to_owned()),
            "NumberDecimal" | "Decimal128" => text_arg(0)
                .and_then(|t| t.parse::<Decimal128>().ok())
                .map(Bson::Decimal128)
                .ok_or_else(|| "NumberDecimal expects a decimal string".to_owned()),
            "Timestamp" => {
                let (t, i) = match (args.first(), args.get(1)) {
                    (Some(Bson::Document(doc)), None) => {
                        (doc.get("t").and_then(as_u32), doc.get("i").and_then(as_u32))
                    }
                    (Some(t), Some(i)) => (as_u32(t), as_u32(i)),
                    _ => (None, None),
                };
                match (t, i) {
                    (Some(time), Some(increment)) => {
                        Ok(Bson::Timestamp(Timestamp { time, increment }))
                    }
                    _ => Err("Timestamp expects (seconds, increment)".into()),
                }
            }
            "UUID" => match args.first() {
                None => Ok(Bson::Binary(Binary::from_uuid(bson::Uuid::new()))),
                Some(Bson::String(text)) => bson::Uuid::parse_str(text)
                    .map(|uuid| Bson::Binary(Binary::from_uuid(uuid)))
                    .map_err(|_| format!("invalid UUID '{text}'")),
                Some(_) => Err("UUID expects a string".into()),
            },
            "BinData" => match (args.first().and_then(as_u32), args.get(1)) {
                (Some(subtype), Some(Bson::String(data))) if subtype <= 255 => {
                    Binary::from_base64(data, BinarySubtype::from(subtype as u8))
                        .map(Bson::Binary)
                        .map_err(|_| "BinData expects valid base64".to_owned())
                }
                _ => Err("BinData expects (subtype, base64)".into()),
            },
            "RegExp" => match (args.first(), args.get(1)) {
                (Some(Bson::String(pattern)), flags) => {
                    let options = match flags {
                        Some(Bson::String(f)) => {
                            let mut chars: Vec<char> = f.chars().collect();
                            chars.sort_unstable();
                            chars.into_iter().collect()
                        }
                        _ => String::new(),
                    };
                    Ok(Bson::RegularExpression(Regex {
                        pattern: pattern.clone(),
                        options,
                    }))
                }
                _ => Err("RegExp expects a pattern string".into()),
            },
            "MinKey" => Ok(Bson::MinKey),
            "MaxKey" => Ok(Bson::MaxKey),
            other => Err(format!("unknown constructor '{other}'")),
        }
    }
}

fn integer(value: i64) -> Bson {
    match i32::try_from(value) {
        Ok(small) => Bson::Int32(small),
        Err(_) => Bson::Int64(value),
    }
}

fn as_u32(value: &Bson) -> Option<u32> {
    match value {
        Bson::Int32(v) => u32::try_from(*v).ok(),
        Bson::Int64(v) => u32::try_from(*v).ok(),
        Bson::Double(v) if v.fract() == 0.0 && *v >= 0.0 && *v <= f64::from(u32::MAX) => {
            Some(*v as u32)
        }
        _ => None,
    }
}

/// Parse an ISO-8601 date; a bare `YYYY-MM-DD` is midnight UTC and a missing zone is UTC.
pub fn parse_date(text: &str) -> Result<DateTime, String> {
    let trimmed = text.trim();
    let candidates = [
        trimmed.to_owned(),
        format!("{trimmed}Z"),
        format!("{trimmed}T00:00:00Z"),
        trimmed.replacen(' ', "T", 1) + "Z",
    ];
    candidates
        .iter()
        .find_map(|c| DateTime::parse_rfc3339_str(c).ok())
        .ok_or_else(|| format!("invalid date '{text}': expected ISO-8601"))
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

const EXTJSON_KEYS: &[&str] = &[
    "$oid",
    "$date",
    "$numberLong",
    "$numberInt",
    "$numberDouble",
    "$numberDecimal",
    "$binary",
    "$uuid",
    "$timestamp",
    "$regularExpression",
    "$minKey",
    "$maxKey",
    "$undefined",
    "$symbol",
    "$code",
    "$dbPointer",
];

/// Convert an Extended JSON wrapper (`{"$oid": …}`) into its BSON value. Ordinary
/// documents, including query operators such as `{$gt: 1}`, are returned unchanged.
fn decode_extended_json(doc: Document) -> Bson {
    let is_wrapper = doc.len() <= 2
        && doc
            .keys()
            .next()
            .is_some_and(|key| EXTJSON_KEYS.contains(&key.as_str()));
    if !is_wrapper {
        return Bson::Document(doc);
    }
    let json = Bson::Document(doc.clone()).into_relaxed_extjson();
    match Bson::try_from(json) {
        Ok(Bson::Document(_)) | Err(_) => Bson::Document(doc),
        Ok(value) => value,
    }
}

/// Output style of [`to_shell_string`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellStyle {
    /// Single line, as in table cells and the query bar.
    Compact,
    /// Indented with the given number of spaces, as in the JSON view and editor.
    Pretty(usize),
}

/// Render a value in mongo shell syntax (round-trips through [`parse_value`]).
pub fn to_shell_string(value: &Bson, style: ShellStyle) -> String {
    let mut out = String::new();
    write_value(&mut out, value, style, 0);
    out
}

/// Render a document in mongo shell syntax.
pub fn document_to_shell(doc: &Document, style: ShellStyle) -> String {
    let mut out = String::new();
    write_document(&mut out, doc, style, 0);
    out
}

fn write_document(out: &mut String, doc: &Document, style: ShellStyle, level: usize) {
    if doc.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    let mut first = true;
    for (key, value) in doc {
        if !first {
            out.push(',');
        }
        first = false;
        newline(out, style, level + 1);
        write_key(out, key);
        out.push_str(": ");
        write_value(out, value, style, level + 1);
    }
    newline(out, style, level);
    out.push('}');
}

fn newline(out: &mut String, style: ShellStyle, level: usize) {
    match style {
        ShellStyle::Compact => out.push(' '),
        ShellStyle::Pretty(width) => {
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', width * level));
        }
    }
}

fn write_key(out: &mut String, key: &str) {
    let plain = key.bytes().next().is_some_and(is_ident_start) && key.bytes().all(is_ident_char);
    if plain {
        out.push_str(key);
    } else {
        write_string(out, key);
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_value(out: &mut String, value: &Bson, style: ShellStyle, level: usize) {
    match value {
        Bson::Document(doc) => write_document(out, doc, style, level),
        Bson::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, style, level + 1);
                write_value(out, item, style, level + 1);
            }
            newline(out, style, level);
            out.push(']');
        }
        Bson::String(s) => write_string(out, s),
        Bson::Double(v) => out.push_str(&format_double(*v)),
        Bson::Int32(v) => {
            let _ = write!(out, "{v}");
        }
        Bson::Int64(v) => {
            let _ = write!(out, "NumberLong({v})");
        }
        Bson::Decimal128(v) => {
            let _ = write!(out, "NumberDecimal(\"{v}\")");
        }
        Bson::Boolean(v) => {
            let _ = write!(out, "{v}");
        }
        Bson::Null => out.push_str("null"),
        Bson::Undefined => out.push_str("undefined"),
        Bson::ObjectId(oid) => {
            let _ = write!(out, "ObjectId(\"{}\")", oid.to_hex());
        }
        Bson::DateTime(date) => match date.try_to_rfc3339_string() {
            Ok(text) => {
                let _ = write!(out, "ISODate(\"{text}\")");
            }
            Err(_) => {
                let _ = write!(out, "Date({})", date.timestamp_millis());
            }
        },
        Bson::RegularExpression(regex) => {
            let _ = write!(
                out,
                "/{}/{}",
                regex.pattern.replace('/', "\\/"),
                regex.options
            );
        }
        Bson::Timestamp(ts) => {
            let _ = write!(out, "Timestamp({}, {})", ts.time, ts.increment);
        }
        Bson::Binary(binary) => {
            if binary.subtype == BinarySubtype::Uuid
                && let Ok(uuid) = binary.to_uuid()
            {
                let _ = write!(out, "UUID(\"{uuid}\")");
                return;
            }
            let json = Bson::Binary(binary.clone()).into_canonical_extjson();
            let base64 = json
                .pointer("/$binary/base64")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            let _ = write!(out, "BinData({}, \"{base64}\")", u8::from(binary.subtype));
        }
        Bson::MinKey => out.push_str("MinKey()"),
        Bson::MaxKey => out.push_str("MaxKey()"),
        other => {
            // Deprecated types (symbol, code, DBPointer) round-trip as Extended JSON.
            out.push_str(&other.clone().into_relaxed_extjson().to_string());
        }
    }
}

fn format_double(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn parses_shell_syntax() {
        let doc = parse_document(
            "{ name: 'Ada', 'age': {$gte: 30}, tags: ['a', \"b\",], // comment\n \
             _id: ObjectId('5f1d7f1e2b3c4d5e6f708192'), at: ISODate('2024-01-02'), \
             big: NumberLong('9007199254740993'), re: /^ad\\/a/i, n: -1.5e3, x: null }",
        )
        .unwrap();
        assert_eq!(doc.get_str("name").unwrap(), "Ada");
        assert_eq!(doc.get_document("age").unwrap(), &doc! {"$gte": 30});
        assert_eq!(doc.get_array("tags").unwrap().len(), 2);
        assert_eq!(
            doc.get_object_id("_id").unwrap().to_hex(),
            "5f1d7f1e2b3c4d5e6f708192"
        );
        assert_eq!(
            doc.get_datetime("at")
                .unwrap()
                .try_to_rfc3339_string()
                .unwrap(),
            "2024-01-02T00:00:00Z"
        );
        assert_eq!(doc.get_i64("big").unwrap(), 9_007_199_254_740_993);
        let re = doc.get("re").unwrap();
        assert!(
            matches!(re, Bson::RegularExpression(r) if r.pattern == "^ad\\/a" && r.options == "i")
        );
        assert_eq!(doc.get_f64("n").unwrap(), -1500.0);
        assert_eq!(doc.get("x"), Some(&Bson::Null));
    }

    #[test]
    fn accepts_extended_json() {
        let doc = parse_document(
            r#"{"_id": {"$oid": "5f1d7f1e2b3c4d5e6f708192"}, "n": {"$numberLong": "5"}, "q": {"$gt": 1}}"#,
        )
        .unwrap();
        assert!(matches!(doc.get("_id"), Some(Bson::ObjectId(_))));
        assert_eq!(doc.get_i64("n").unwrap(), 5);
        assert_eq!(doc.get_document("q").unwrap(), &doc! {"$gt": 1});
    }

    #[test]
    fn round_trips_through_printer() {
        let original = doc! {
            "_id": ObjectId::parse_str("5f1d7f1e2b3c4d5e6f708192").unwrap(),
            "name": "line\n\"quoted\"",
            "weird key": 1,
            "n": 3.0,
            "big": 1_i64 << 40,
            "nested": { "arr": [1, { "deep": true }], "empty": {} },
            "when": DateTime::from_millis(1_700_000_000_000),
            "uuid": Binary::from_uuid(bson::Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap()),
            "ts": Timestamp { time: 5, increment: 2 },
        };
        for style in [ShellStyle::Compact, ShellStyle::Pretty(2)] {
            let text = document_to_shell(&original, style);
            assert_eq!(parse_document(&text).unwrap(), original, "{text}");
        }
    }

    #[test]
    fn reports_errors_with_offsets() {
        let err = parse_document("{a: 1,, }").unwrap_err();
        assert_eq!(err.offset, 6);
        assert!(parse_document("{a: ObjectId('xyz')}").is_err());
        assert!(parse_document("{a: 'open").is_err());
        assert!(parse_document("[1]").is_err());
        assert!(parse_document("{a: foo}").is_err());
        let deep = "[".repeat(500);
        assert!(parse_value(&deep).is_err());
    }

    #[test]
    fn parses_pipelines() {
        let stages =
            parse_document_array("[{$match: {a: 1}}, {$group: {_id: '$b', n: {$sum: 1}}}]")
                .unwrap();
        assert_eq!(stages.len(), 2);
        assert!(parse_document_array("[1]").is_err());
        assert_eq!(parse_document_array("").unwrap(), Vec::<Document>::new());
    }
}
