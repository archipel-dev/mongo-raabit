//! Query Code: export a find or aggregation as a runnable snippet for the official
//! drivers of JavaScript (Node.js), Python, Java, C#, Go, Rust, PHP and Ruby.

use crate::query::Query;
use mongodb::bson::{Bson, Document, spec::BinarySubtype};
use serde::Serialize;
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Language {
    JavaScript,
    Python,
    Java,
    CSharp,
    Go,
    Rust,
    Php,
    Ruby,
}

impl Language {
    pub const ALL: [Self; 8] = [
        Self::JavaScript,
        Self::Python,
        Self::Java,
        Self::CSharp,
        Self::Go,
        Self::Rust,
        Self::Php,
        Self::Ruby,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::JavaScript => "JavaScript (Node.js)",
            Self::Python => "Python (PyMongo)",
            Self::Java => "Java",
            Self::CSharp => "C#",
            Self::Go => "Go",
            Self::Rust => "Rust",
            Self::Php => "PHP",
            Self::Ruby => "Ruby",
        }
    }

    /// File extension for "Save as…".
    pub fn extension(self) -> &'static str {
        match self {
            Self::JavaScript => "js",
            Self::Python => "py",
            Self::Java => "java",
            Self::CSharp => "cs",
            Self::Go => "go",
            Self::Rust => "rs",
            Self::Php => "php",
            Self::Ruby => "rb",
        }
    }
}

/// What to generate code for.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // built once per request
pub enum Operation {
    Find(Query),
    Aggregate(Vec<Document>),
}

/// Input of the generator.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeRequest {
    /// Connection string placed in the snippet; never include real passwords.
    pub uri: String,
    pub database: String,
    pub collection: String,
    pub operation: Operation,
}

pub fn generate(language: Language, request: &CodeRequest) -> String {
    match language {
        Language::JavaScript => javascript(request),
        Language::Python => python(request),
        Language::Java => java(request),
        Language::CSharp => csharp(request),
        Language::Go => go(request),
        Language::Rust => rust(request),
        Language::Php => php(request),
        Language::Ruby => ruby(request),
    }
}

// ----- shared value rendering ------------------------------------------------------------

/// Per-language literal syntax.
struct Syntax {
    doc_open: &'static str,
    doc_close: &'static str,
    arr_open: &'static str,
    arr_close: &'static str,
    /// Formats one `key: value` pair given the rendered key string literal and value.
    pair: fn(&str, &str) -> String,
    string: fn(&str) -> String,
    scalar: fn(&Bson) -> Option<String>,
    indent: usize,
}

fn quote_double(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
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
    out
}

fn quote_single(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn iso(date: &mongodb::bson::DateTime) -> String {
    date.try_to_rfc3339_string()
        .unwrap_or_else(|_| date.timestamp_millis().to_string())
}

fn double(v: f64) -> String {
    if v.fract() == 0.0 && v.is_finite() && v.abs() < 1e15 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

fn binary_base64(value: &Bson) -> String {
    value
        .clone()
        .into_canonical_extjson()
        .pointer("/$binary/base64")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn render(value: &Bson, syntax: &Syntax, level: usize) -> String {
    if let Some(scalar) = (syntax.scalar)(value) {
        return scalar;
    }
    let pad = |l: usize| " ".repeat(syntax.indent * l);
    match value {
        Bson::Document(doc) if doc.is_empty() => format!("{}{}", syntax.doc_open, syntax.doc_close),
        Bson::Document(doc) => {
            let items: Vec<String> = doc
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}{}",
                        pad(level + 1),
                        (syntax.pair)(&(syntax.string)(k), &render(v, syntax, level + 1))
                    )
                })
                .collect();
            format!(
                "{}\n{}\n{}{}",
                syntax.doc_open,
                items.join(",\n"),
                pad(level),
                syntax.doc_close
            )
        }
        Bson::Array(items) if items.is_empty() => {
            format!("{}{}", syntax.arr_open, syntax.arr_close)
        }
        Bson::Array(items) => {
            let items: Vec<String> = items
                .iter()
                .map(|v| format!("{}{}", pad(level + 1), render(v, syntax, level + 1)))
                .collect();
            format!(
                "{}\n{}\n{}{}",
                syntax.arr_open,
                items.join(",\n"),
                pad(level),
                syntax.arr_close
            )
        }
        Bson::String(s) => (syntax.string)(s),
        Bson::Int32(v) => v.to_string(),
        Bson::Double(v) => double(*v),
        other => (syntax.string)(&other.to_string()),
    }
}

fn doc_value(doc: &Document) -> Bson {
    Bson::Document(doc.clone())
}

fn pipeline_value(stages: &[Document]) -> Bson {
    Bson::Array(stages.iter().cloned().map(Bson::Document).collect())
}

fn indent_tail(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.to_owned()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ----- JavaScript ------------------------------------------------------------------------

fn js_scalar(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::ObjectId(o) => format!("new ObjectId(\"{}\")", o.to_hex()),
        Bson::DateTime(d) => format!("new Date(\"{}\")", iso(d)),
        Bson::Int64(v) => format!("Long.fromString(\"{v}\")"),
        Bson::Decimal128(d) => format!("Decimal128.fromString(\"{d}\")"),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "null".into(),
        Bson::RegularExpression(r) => format!(
            "new BSONRegExp({}, \"{}\")",
            quote_double(&r.pattern),
            r.options
        ),
        Bson::Timestamp(t) => format!("new Timestamp({{ t: {}, i: {} }})", t.time, t.increment),
        Bson::Binary(b) if b.subtype == BinarySubtype::Uuid => match b.to_uuid() {
            Ok(u) => format!("new UUID(\"{u}\")"),
            Err(_) => return None,
        },
        Bson::Binary(b) => format!(
            "Binary.createFromBase64(\"{}\", {})",
            binary_base64(value),
            u8::from(b.subtype)
        ),
        Bson::MinKey => "new MinKey()".into(),
        Bson::MaxKey => "new MaxKey()".into(),
        _ => return None,
    })
}

/// Unquote a rendered key when it is a valid identifier (`$gte`, `status`).
fn js_key(quoted: &str) -> String {
    let inner = &quoted[1..quoted.len().saturating_sub(1).max(1)];
    let plain = inner
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if plain {
        inner.to_owned()
    } else {
        quoted.to_owned()
    }
}

const JS: Syntax = Syntax {
    doc_open: "{",
    doc_close: "}",
    arr_open: "[",
    arr_close: "]",
    pair: |k, v| format!("{}: {v}", js_key(k)),
    string: |s| quote_double(s),
    scalar: js_scalar,
    indent: 2,
};

fn javascript(r: &CodeRequest) -> String {
    let mut out = String::from(
        "// Requires: npm install mongodb\nconst { MongoClient, ObjectId, Long, Decimal128, BSONRegExp, Timestamp, UUID, Binary, MinKey, MaxKey } = require(\"mongodb\");\n\n",
    );
    let _ = writeln!(out, "async function main() {{");
    let _ = writeln!(
        out,
        "  const client = new MongoClient({});",
        quote_double(&r.uri)
    );
    let _ = writeln!(out, "  try {{");
    let _ = writeln!(
        out,
        "    const collection = client.db({}).collection({});",
        quote_double(&r.database),
        quote_double(&r.collection)
    );
    match &r.operation {
        Operation::Find(q) => {
            let _ = writeln!(
                out,
                "    const filter = {};",
                indent_tail(&render(&doc_value(&q.filter), &JS, 0), 4)
            );
            let mut options = Vec::new();
            if let Some(p) = &q.projection {
                options.push(format!("projection: {}", render(&doc_value(p), &JS, 3)));
            }
            if let Some(s) = &q.sort {
                options.push(format!("sort: {}", render(&doc_value(s), &JS, 3)));
            }
            if q.skip > 0 {
                options.push(format!("skip: {}", q.skip));
            }
            if q.limit > 0 {
                options.push(format!("limit: {}", q.limit));
            }
            if options.is_empty() {
                let _ = writeln!(out, "    const cursor = collection.find(filter);");
            } else {
                let body = options
                    .iter()
                    .map(|o| format!("      {o}"))
                    .collect::<Vec<_>>()
                    .join(",\n");
                let _ = writeln!(out, "    const options = {{\n{body}\n    }};");
                let _ = writeln!(out, "    const cursor = collection.find(filter, options);");
            }
        }
        Operation::Aggregate(stages) => {
            let _ = writeln!(
                out,
                "    const pipeline = {};",
                indent_tail(&render(&pipeline_value(stages), &JS, 0), 4)
            );
            let _ = writeln!(
                out,
                "    const cursor = collection.aggregate(pipeline, {{ allowDiskUse: true }});"
            );
        }
    }
    out.push_str("    for await (const doc of cursor) {\n      console.log(doc);\n    }\n  } finally {\n    await client.close();\n  }\n}\n\nmain().catch(console.error);\n");
    out
}

// ----- Python ----------------------------------------------------------------------------

fn py_scalar(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::ObjectId(o) => format!("ObjectId(\"{}\")", o.to_hex()),
        Bson::DateTime(d) => format!(
            "datetime.fromisoformat(\"{}\")",
            iso(d).replace('Z', "+00:00")
        ),
        Bson::Int64(v) => format!("Int64({v})"),
        Bson::Decimal128(d) => format!("Decimal128(\"{d}\")"),
        Bson::Boolean(true) => "True".into(),
        Bson::Boolean(false) => "False".into(),
        Bson::Null => "None".into(),
        Bson::RegularExpression(r) => {
            format!("Regex({}, \"{}\")", quote_double(&r.pattern), r.options)
        }
        Bson::Timestamp(t) => format!("Timestamp({}, {})", t.time, t.increment),
        Bson::Binary(b) if b.subtype == BinarySubtype::Uuid => match b.to_uuid() {
            Ok(u) => format!("Binary.from_uuid(uuid.UUID(\"{u}\"))"),
            Err(_) => return None,
        },
        Bson::Binary(b) => format!(
            "Binary(base64.b64decode(\"{}\"), {})",
            binary_base64(value),
            u8::from(b.subtype)
        ),
        Bson::MinKey => "MinKey()".into(),
        Bson::MaxKey => "MaxKey()".into(),
        _ => return None,
    })
}

const PY: Syntax = Syntax {
    doc_open: "{",
    doc_close: "}",
    arr_open: "[",
    arr_close: "]",
    pair: |k, v| format!("{k}: {v}"),
    string: |s| quote_double(s),
    scalar: py_scalar,
    indent: 4,
};

fn python(r: &CodeRequest) -> String {
    let mut out = String::from(
        "# Requires: pip install pymongo\nimport base64\nimport uuid\nfrom datetime import datetime\n\nfrom bson import Binary, Decimal128, Int64, MaxKey, MinKey, ObjectId, Regex, Timestamp\nfrom pymongo import MongoClient\n\n",
    );
    let _ = writeln!(out, "client = MongoClient({})", quote_double(&r.uri));
    let _ = writeln!(
        out,
        "collection = client[{}][{}]\n",
        quote_double(&r.database),
        quote_double(&r.collection)
    );
    match &r.operation {
        Operation::Find(q) => {
            let _ = writeln!(out, "filter = {}", render(&doc_value(&q.filter), &PY, 0));
            let mut args = vec!["filter".to_owned()];
            if let Some(p) = &q.projection {
                let _ = writeln!(out, "projection = {}", render(&doc_value(p), &PY, 0));
                args.push("projection=projection".into());
            }
            if let Some(s) = &q.sort {
                let pairs: Vec<String> = s
                    .iter()
                    .map(|(k, v)| format!("({}, {})", quote_double(k), render(v, &PY, 1)))
                    .collect();
                let _ = writeln!(out, "sort = [{}]", pairs.join(", "));
                args.push("sort=sort".into());
            }
            if q.skip > 0 {
                args.push(format!("skip={}", q.skip));
            }
            if q.limit > 0 {
                args.push(format!("limit={}", q.limit));
            }
            let _ = writeln!(out, "\ncursor = collection.find({})", args.join(", "));
        }
        Operation::Aggregate(stages) => {
            let _ = writeln!(
                out,
                "pipeline = {}",
                render(&pipeline_value(stages), &PY, 0)
            );
            let _ = writeln!(
                out,
                "\ncursor = collection.aggregate(pipeline, allowDiskUse=True)"
            );
        }
    }
    out.push_str("for document in cursor:\n    print(document)\n");
    out
}

// ----- Java ------------------------------------------------------------------------------

fn java_scalar(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::ObjectId(o) => format!("new ObjectId(\"{}\")", o.to_hex()),
        Bson::DateTime(d) => format!("Date.from(Instant.parse(\"{}\"))", iso(d)),
        Bson::Int64(v) => format!("{v}L"),
        Bson::Decimal128(d) => format!("Decimal128.parse(\"{d}\")"),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "null".into(),
        Bson::RegularExpression(r) => format!(
            "new BsonRegularExpression({}, \"{}\")",
            quote_double(&r.pattern),
            r.options
        ),
        Bson::Timestamp(t) => format!("new BsonTimestamp({}, {})", t.time, t.increment),
        Bson::Binary(b) if b.subtype == BinarySubtype::Uuid => match b.to_uuid() {
            Ok(u) => format!("UUID.fromString(\"{u}\")"),
            Err(_) => return None,
        },
        Bson::Binary(b) => format!(
            "new Binary((byte) {}, Base64.getDecoder().decode(\"{}\"))",
            u8::from(b.subtype),
            binary_base64(value)
        ),
        Bson::MinKey => "new MinKey()".into(),
        Bson::MaxKey => "new MaxKey()".into(),
        _ => return None,
    })
}

fn java_value(value: &Bson, level: usize) -> String {
    let pad = |l: usize| " ".repeat(4 * l);
    match value {
        Bson::Document(doc) if doc.is_empty() => "new Document()".into(),
        Bson::Document(doc) => {
            let mut out = String::new();
            for (i, (k, v)) in doc.iter().enumerate() {
                if i == 0 {
                    let _ = write!(
                        out,
                        "new Document({}, {})",
                        quote_double(k),
                        java_value(v, level + 1)
                    );
                } else {
                    let _ = write!(
                        out,
                        "\n{}.append({}, {})",
                        pad(level + 1),
                        quote_double(k),
                        java_value(v, level + 1)
                    );
                }
            }
            out
        }
        Bson::Array(items) if items.is_empty() => "Collections.emptyList()".into(),
        Bson::Array(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|v| format!("{}{}", pad(level + 1), java_value(v, level + 1)))
                .collect();
            format!("Arrays.asList(\n{}\n{})", parts.join(",\n"), pad(level))
        }
        Bson::String(s) => quote_double(s),
        Bson::Int32(v) => v.to_string(),
        Bson::Double(v) => double(*v),
        other => java_scalar(other).unwrap_or_else(|| quote_double(&other.to_string())),
    }
}

fn java(r: &CodeRequest) -> String {
    let mut out = String::from(
        "// Requires: org.mongodb:mongodb-driver-sync\nimport com.mongodb.client.*;\nimport org.bson.*;\nimport org.bson.conversions.Bson;\nimport org.bson.types.*;\nimport java.time.Instant;\nimport java.util.*;\n\npublic class Query {\n    public static void main(String[] args) {\n",
    );
    let _ = writeln!(
        out,
        "        try (MongoClient client = MongoClients.create({})) {{",
        quote_double(&r.uri)
    );
    let _ = writeln!(
        out,
        "            MongoCollection<Document> collection = client.getDatabase({}).getCollection({});",
        quote_double(&r.database),
        quote_double(&r.collection)
    );
    match &r.operation {
        Operation::Find(q) => {
            let _ = writeln!(
                out,
                "            Bson filter = {};",
                indent_tail(&java_value(&doc_value(&q.filter), 0), 12)
            );
            let mut chain = String::from("collection.find(filter)");
            if let Some(p) = &q.projection {
                let _ = write!(
                    chain,
                    "\n                .projection({})",
                    indent_tail(&java_value(&doc_value(p), 0), 16)
                );
            }
            if let Some(s) = &q.sort {
                let _ = write!(
                    chain,
                    "\n                .sort({})",
                    indent_tail(&java_value(&doc_value(s), 0), 16)
                );
            }
            if q.skip > 0 {
                let _ = write!(chain, "\n                .skip({})", q.skip);
            }
            if q.limit > 0 {
                let _ = write!(chain, "\n                .limit({})", q.limit);
            }
            let _ = writeln!(out, "            FindIterable<Document> result = {chain};");
        }
        Operation::Aggregate(stages) => {
            let items: Vec<String> = stages
                .iter()
                .map(|s| {
                    format!(
                        "                {}",
                        indent_tail(&java_value(&doc_value(s), 0), 16)
                    )
                })
                .collect();
            let _ = writeln!(
                out,
                "            List<Bson> pipeline = Arrays.asList(\n{}\n            );",
                items.join(",\n")
            );
            let _ = writeln!(
                out,
                "            AggregateIterable<Document> result = collection.aggregate(pipeline).allowDiskUse(true);"
            );
        }
    }
    out.push_str("            for (Document document : result) {\n                System.out.println(document.toJson());\n            }\n        }\n    }\n}\n");
    out
}

// ----- C# --------------------------------------------------------------------------------

fn cs_scalar(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::ObjectId(o) => format!("new ObjectId(\"{}\")", o.to_hex()),
        Bson::DateTime(d) => format!(
            "DateTime.Parse(\"{}\", null, System.Globalization.DateTimeStyles.AdjustToUniversal)",
            iso(d)
        ),
        Bson::Int64(v) => format!("{v}L"),
        Bson::Decimal128(d) => format!("Decimal128.Parse(\"{d}\")"),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "BsonNull.Value".into(),
        Bson::RegularExpression(r) => format!(
            "new BsonRegularExpression({}, \"{}\")",
            quote_double(&r.pattern),
            r.options
        ),
        Bson::Timestamp(t) => format!("new BsonTimestamp({}, {})", t.time, t.increment),
        Bson::Binary(b) if b.subtype == BinarySubtype::Uuid => match b.to_uuid() {
            Ok(u) => {
                format!("new BsonBinaryData(Guid.Parse(\"{u}\"), GuidRepresentation.Standard)")
            }
            Err(_) => return None,
        },
        Bson::Binary(b) => format!(
            "new BsonBinaryData(Convert.FromBase64String(\"{}\"), (BsonBinarySubType) {})",
            binary_base64(value),
            u8::from(b.subtype)
        ),
        Bson::MinKey => "BsonMinKey.Value".into(),
        Bson::MaxKey => "BsonMaxKey.Value".into(),
        _ => return None,
    })
}

const CS: Syntax = Syntax {
    doc_open: "new BsonDocument {",
    doc_close: "}",
    arr_open: "new BsonArray {",
    arr_close: "}",
    pair: |k, v| format!("{{ {k}, {v} }}"),
    string: |s| quote_double(s),
    scalar: cs_scalar,
    indent: 4,
};

fn csharp(r: &CodeRequest) -> String {
    let mut out = String::from(
        "// Requires: dotnet add package MongoDB.Driver\nusing System;\nusing MongoDB.Bson;\nusing MongoDB.Driver;\n\n",
    );
    let _ = writeln!(
        out,
        "var client = new MongoClient({});",
        quote_double(&r.uri)
    );
    let _ = writeln!(
        out,
        "var collection = client.GetDatabase({}).GetCollection<BsonDocument>({});\n",
        quote_double(&r.database),
        quote_double(&r.collection)
    );
    match &r.operation {
        Operation::Find(q) => {
            let _ = writeln!(
                out,
                "var filter = {};",
                render(&doc_value(&q.filter), &CS, 0)
            );
            let mut chain = String::from("collection.Find(filter)");
            if let Some(p) = &q.projection {
                let _ = writeln!(out, "var projection = {};", render(&doc_value(p), &CS, 0));
                chain.push_str("\n    .Project(projection)");
            }
            if let Some(s) = &q.sort {
                let _ = writeln!(out, "var sort = {};", render(&doc_value(s), &CS, 0));
                chain.push_str("\n    .Sort(sort)");
            }
            if q.skip > 0 {
                let _ = write!(chain, "\n    .Skip({})", q.skip);
            }
            if q.limit > 0 {
                let _ = write!(chain, "\n    .Limit({})", q.limit);
            }
            let _ = writeln!(out, "\nvar result = {chain}\n    .ToList();");
        }
        Operation::Aggregate(stages) => {
            let items: Vec<String> = stages
                .iter()
                .map(|s| format!("    {}", indent_tail(&render(&doc_value(s), &CS, 0), 4)))
                .collect();
            let _ = writeln!(out, "var pipeline = new[]\n{{\n{}\n}};", items.join(",\n"));
            let _ = writeln!(
                out,
                "\nvar result = collection.Aggregate<BsonDocument>(pipeline, new AggregateOptions {{ AllowDiskUse = true }}).ToList();"
            );
        }
    }
    out.push_str("foreach (var document in result)\n{\n    Console.WriteLine(document);\n}\n");
    out
}

// ----- Go --------------------------------------------------------------------------------

fn go_scalar(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::ObjectId(o) => format!("mustObjectID(\"{}\")", o.to_hex()),
        Bson::DateTime(d) => format!("mustTime(\"{}\")", iso(d)),
        Bson::Int64(v) => format!("int64({v})"),
        Bson::Int32(v) => format!("int32({v})"),
        Bson::Decimal128(d) => format!("mustDecimal(\"{d}\")"),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "nil".into(),
        Bson::RegularExpression(r) => format!(
            "bson.Regex{{Pattern: {}, Options: \"{}\"}}",
            quote_double(&r.pattern),
            r.options
        ),
        Bson::Timestamp(t) => format!("bson.Timestamp{{T: {}, I: {}}}", t.time, t.increment),
        Bson::Binary(b) => format!(
            "bson.Binary{{Subtype: {}, Data: mustBase64(\"{}\")}}",
            u8::from(b.subtype),
            binary_base64(value)
        ),
        Bson::MinKey => "bson.MinKey{}".into(),
        Bson::MaxKey => "bson.MaxKey{}".into(),
        _ => return None,
    })
}

const GO: Syntax = Syntax {
    doc_open: "bson.D{",
    doc_close: "}",
    arr_open: "bson.A{",
    arr_close: "}",
    pair: |k, v| format!("{{Key: {k}, Value: {v}}}"),
    string: |s| quote_double(s),
    scalar: go_scalar,
    indent: 1,
};

fn go_render(value: &Bson) -> String {
    // Go convention: tabs, trailing commas inside composite literals.
    let text = render(value, &GO, 0);
    text.lines()
        .map(|line| {
            let depth = line.len() - line.trim_start().len();
            let trimmed = line.trim_start();
            let mut l = format!("{}{}", "\t".repeat(depth), trimmed);
            if !trimmed.ends_with('{') && !trimmed.ends_with(',') && depth > 0 {
                l.push(',');
            }
            l
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn go(r: &CodeRequest) -> String {
    let mut out = String::new();
    out.push_str("func main() {\n\tctx := context.Background()\n");
    let _ = writeln!(
        out,
        "\tclient, err := mongo.Connect(options.Client().ApplyURI({}))\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer client.Disconnect(ctx)",
        quote_double(&r.uri)
    );
    let _ = writeln!(
        out,
        "\tcollection := client.Database({}).Collection({})\n",
        quote_double(&r.database),
        quote_double(&r.collection)
    );
    let tab = |text: String| indent_tail(&text, 0).replace('\n', "\n\t");
    match &r.operation {
        Operation::Find(q) => {
            let _ = writeln!(out, "\tfilter := {}", tab(go_render(&doc_value(&q.filter))));
            let _ = writeln!(out, "\topts := options.Find()");
            if let Some(p) = &q.projection {
                let _ = writeln!(
                    out,
                    "\topts.SetProjection({})",
                    tab(go_render(&doc_value(p)))
                );
            }
            if let Some(s) = &q.sort {
                let _ = writeln!(out, "\topts.SetSort({})", tab(go_render(&doc_value(s))));
            }
            if q.skip > 0 {
                let _ = writeln!(out, "\topts.SetSkip({})", q.skip);
            }
            if q.limit > 0 {
                let _ = writeln!(out, "\topts.SetLimit({})", q.limit);
            }
            out.push_str("\tcursor, err := collection.Find(ctx, filter, opts)\n");
        }
        Operation::Aggregate(stages) => {
            let _ = writeln!(out, "\tpipeline := mongo.Pipeline{{");
            for stage in stages {
                let _ = writeln!(
                    out,
                    "\t\t{},",
                    go_render(&doc_value(stage)).replace('\n', "\n\t\t")
                );
            }
            out.push_str("\t}\n\tcursor, err := collection.Aggregate(ctx, pipeline, options.Aggregate().SetAllowDiskUse(true))\n");
        }
    }
    out.push_str("\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer cursor.Close(ctx)\n\tfor cursor.Next(ctx) {\n\t\tvar document bson.M\n\t\tif err := cursor.Decode(&document); err != nil {\n\t\t\tpanic(err)\n\t\t}\n\t\tfmt.Println(document)\n\t}\n}\n");
    // Go rejects unused imports, so only emit the helpers the body needs.
    let uses = |name: &str| out.contains(&format!("{name}(\""));
    let mut imports = vec!["\t\"context\""];
    let mut helpers = String::new();
    if uses("mustBase64") {
        imports.push("\t\"encoding/base64\"");
        helpers.push_str("func mustBase64(value string) []byte {\n\tb, err := base64.StdEncoding.DecodeString(value)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\treturn b\n}\n\n");
    }
    imports.push("\t\"fmt\"");
    if uses("mustTime") {
        imports.push("\t\"time\"");
        helpers.push_str("func mustTime(value string) time.Time {\n\tt, err := time.Parse(time.RFC3339Nano, value)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\treturn t\n}\n\n");
    }
    if uses("mustObjectID") {
        helpers.push_str("func mustObjectID(hex string) bson.ObjectID {\n\tid, err := bson.ObjectIDFromHex(hex)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\treturn id\n}\n\n");
    }
    if uses("mustDecimal") {
        helpers.push_str("func mustDecimal(value string) bson.Decimal128 {\n\td, err := bson.ParseDecimal128(value)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\treturn d\n}\n\n");
    }
    format!(
        "// Requires: go get go.mongodb.org/mongo-driver/v2/mongo\npackage main\n\nimport (\n{}\n\n\t\"go.mongodb.org/mongo-driver/v2/bson\"\n\t\"go.mongodb.org/mongo-driver/v2/mongo\"\n\t\"go.mongodb.org/mongo-driver/v2/mongo/options\"\n)\n\n{helpers}{out}",
        imports.join("\n")
    )
}

// ----- Rust ------------------------------------------------------------------------------

fn rust_scalar(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::ObjectId(o) => format!("ObjectId::parse_str(\"{}\")?", o.to_hex()),
        Bson::DateTime(d) => format!("DateTime::parse_rfc3339_str(\"{}\")?", iso(d)),
        Bson::Int64(v) => format!("{v}_i64"),
        Bson::Double(v) => format!("{}_f64", double(*v)),
        Bson::Decimal128(d) => format!("\"{d}\".parse::<Decimal128>()?"),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "null".into(),
        Bson::RegularExpression(r) => format!(
            "Regex {{ pattern: {}.into(), options: \"{}\".into() }}",
            quote_double(&r.pattern),
            r.options
        ),
        Bson::Timestamp(t) => format!(
            "Timestamp {{ time: {}, increment: {} }}",
            t.time, t.increment
        ),
        Bson::Binary(b) if b.subtype == BinarySubtype::Uuid => match b.to_uuid() {
            Ok(u) => format!("Binary::from_uuid(Uuid::parse_str(\"{u}\")?)"),
            Err(_) => return None,
        },
        Bson::Binary(b) => format!(
            "Binary::from_base64(\"{}\", BinarySubtype::from({}u8))?",
            binary_base64(value),
            u8::from(b.subtype)
        ),
        Bson::MinKey => "Bson::MinKey".into(),
        Bson::MaxKey => "Bson::MaxKey".into(),
        _ => return None,
    })
}

const RUST: Syntax = Syntax {
    doc_open: "{",
    doc_close: "}",
    arr_open: "[",
    arr_close: "]",
    pair: |k, v| format!("{k}: {v}"),
    string: |s| quote_double(s),
    scalar: rust_scalar,
    indent: 4,
};

fn rust(r: &CodeRequest) -> String {
    let mut out = String::from(
        "// Requires: mongodb = \"3\", tokio = { version = \"1\", features = [\"full\"] }, futures = \"0.3\"\n#![allow(unused_imports)]\nuse futures::TryStreamExt;\nuse mongodb::bson::{doc, oid::ObjectId, spec::BinarySubtype, Binary, Bson, DateTime, Decimal128, Document, Regex, Timestamp, Uuid};\nuse mongodb::Client;\n\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
    );
    let _ = writeln!(
        out,
        "    let client = Client::with_uri_str({}).await?;",
        quote_double(&r.uri)
    );
    let _ = writeln!(
        out,
        "    let collection = client.database({}).collection::<Document>({});\n",
        quote_double(&r.database),
        quote_double(&r.collection)
    );
    let doc_macro = |d: &Document, base: usize| {
        format!(
            "doc! {}",
            indent_tail(&render(&doc_value(d), &RUST, 0), base)
        )
    };
    match &r.operation {
        Operation::Find(q) => {
            let _ = writeln!(out, "    let filter = {};", doc_macro(&q.filter, 4));
            let mut chain = String::from("collection\n        .find(filter)");
            if let Some(p) = &q.projection {
                let _ = write!(chain, "\n        .projection({})", doc_macro(p, 8));
            }
            if let Some(s) = &q.sort {
                let _ = write!(chain, "\n        .sort({})", doc_macro(s, 8));
            }
            if q.skip > 0 {
                let _ = write!(chain, "\n        .skip({})", q.skip);
            }
            if q.limit > 0 {
                let _ = write!(chain, "\n        .limit({})", q.limit);
            }
            let _ = writeln!(out, "    let mut cursor = {chain}\n        .await?;");
        }
        Operation::Aggregate(stages) => {
            let items: Vec<String> = stages
                .iter()
                .map(|s| format!("        {},", doc_macro(s, 8)))
                .collect();
            let _ = writeln!(
                out,
                "    let pipeline = vec![\n{}\n    ];",
                items.join("\n")
            );
            let _ = writeln!(
                out,
                "    let mut cursor = collection.aggregate(pipeline).allow_disk_use(true).await?;"
            );
        }
    }
    out.push_str("    while let Some(document) = cursor.try_next().await? {\n        println!(\"{document}\");\n    }\n    Ok(())\n}\n");
    out
}

// ----- PHP -------------------------------------------------------------------------------

fn php_scalar(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::ObjectId(o) => format!("new ObjectId('{}')", o.to_hex()),
        Bson::DateTime(d) => format!("new UTCDateTime({})", d.timestamp_millis()),
        Bson::Int64(v) => v.to_string(),
        Bson::Decimal128(d) => format!("new Decimal128('{d}')"),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "null".into(),
        Bson::RegularExpression(r) => {
            format!("new Regex({}, '{}')", quote_single(&r.pattern), r.options)
        }
        Bson::Timestamp(t) => format!("new Timestamp({}, {})", t.increment, t.time),
        Bson::Binary(b) => format!(
            "new Binary(base64_decode('{}'), {})",
            binary_base64(value),
            u8::from(b.subtype)
        ),
        Bson::MinKey => "new MinKey()".into(),
        Bson::MaxKey => "new MaxKey()".into(),
        _ => return None,
    })
}

const PHP: Syntax = Syntax {
    doc_open: "[",
    doc_close: "]",
    arr_open: "[",
    arr_close: "]",
    pair: |k, v| format!("{k} => {v}"),
    string: |s| quote_single(s),
    scalar: php_scalar,
    indent: 4,
};

fn php(r: &CodeRequest) -> String {
    let mut out = String::from(
        "<?php\n// Requires: composer require mongodb/mongodb\nrequire 'vendor/autoload.php';\n\nuse MongoDB\\BSON\\{Binary, Decimal128, MaxKey, MinKey, ObjectId, Regex, Timestamp, UTCDateTime};\nuse MongoDB\\Client;\n\n",
    );
    let _ = writeln!(out, "$client = new Client({});", quote_single(&r.uri));
    let _ = writeln!(
        out,
        "$collection = $client->selectCollection({}, {});\n",
        quote_single(&r.database),
        quote_single(&r.collection)
    );
    match &r.operation {
        Operation::Find(q) => {
            let _ = writeln!(out, "$filter = {};", render(&doc_value(&q.filter), &PHP, 0));
            let mut options = Vec::new();
            if let Some(p) = &q.projection {
                options.push(format!(
                    "    'projection' => {}",
                    indent_tail(&render(&doc_value(p), &PHP, 0), 4)
                ));
            }
            if let Some(s) = &q.sort {
                options.push(format!(
                    "    'sort' => {}",
                    indent_tail(&render(&doc_value(s), &PHP, 0), 4)
                ));
            }
            if q.skip > 0 {
                options.push(format!("    'skip' => {}", q.skip));
            }
            if q.limit > 0 {
                options.push(format!("    'limit' => {}", q.limit));
            }
            let _ = writeln!(out, "$options = [\n{}\n];", options.join(",\n"));
            out.push_str("\n$cursor = $collection->find($filter, $options);\n");
        }
        Operation::Aggregate(stages) => {
            let _ = writeln!(
                out,
                "$pipeline = {};",
                render(&pipeline_value(stages), &PHP, 0)
            );
            out.push_str(
                "\n$cursor = $collection->aggregate($pipeline, ['allowDiskUse' => true]);\n",
            );
        }
    }
    out.push_str(
        "foreach ($cursor as $document) {\n    echo json_encode($document), PHP_EOL;\n}\n",
    );
    out.replace("$options = [\n\n];\n", "$options = [];\n")
}

// ----- Ruby ------------------------------------------------------------------------------

fn ruby_scalar(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::ObjectId(o) => format!("BSON::ObjectId.from_string('{}')", o.to_hex()),
        Bson::DateTime(d) => format!("Time.iso8601('{}')", iso(d)),
        Bson::Int64(v) => format!("BSON::Int64.new({v})"),
        Bson::Decimal128(d) => format!("BSON::Decimal128.new('{d}')"),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "nil".into(),
        Bson::RegularExpression(r) => format!(
            "BSON::Regexp::Raw.new({}, '{}')",
            quote_single(&r.pattern),
            r.options
        ),
        Bson::Timestamp(t) => format!("BSON::Timestamp.new({}, {})", t.time, t.increment),
        Bson::Binary(b) => format!(
            "BSON::Binary.new(Base64.decode64('{}'), :{})",
            binary_base64(value),
            match b.subtype {
                BinarySubtype::Uuid => "uuid",
                BinarySubtype::Md5 => "md5",
                _ => "generic",
            }
        ),
        Bson::MinKey => "BSON::MinKey.new".into(),
        Bson::MaxKey => "BSON::MaxKey.new".into(),
        _ => return None,
    })
}

const RUBY: Syntax = Syntax {
    doc_open: "{",
    doc_close: "}",
    arr_open: "[",
    arr_close: "]",
    pair: |k, v| format!("{k} => {v}"),
    string: |s| quote_single(s),
    scalar: ruby_scalar,
    indent: 2,
};

fn ruby(r: &CodeRequest) -> String {
    let mut out = String::from(
        "# Requires: gem install mongo\nrequire 'base64'\nrequire 'mongo'\nrequire 'time'\n\n",
    );
    let _ = writeln!(
        out,
        "client = Mongo::Client.new({}, database: {})",
        quote_single(&r.uri),
        quote_single(&r.database)
    );
    let _ = writeln!(
        out,
        "collection = client[{}]\n",
        quote_single(&r.collection)
    );
    match &r.operation {
        Operation::Find(q) => {
            let _ = writeln!(out, "filter = {}", render(&doc_value(&q.filter), &RUBY, 0));
            let mut chain = String::from("collection.find(filter)");
            if let Some(p) = &q.projection {
                let _ = write!(
                    chain,
                    "\n  .projection({})",
                    indent_tail(&render(&doc_value(p), &RUBY, 0), 2)
                );
            }
            if let Some(s) = &q.sort {
                let _ = write!(
                    chain,
                    "\n  .sort({})",
                    indent_tail(&render(&doc_value(s), &RUBY, 0), 2)
                );
            }
            if q.skip > 0 {
                let _ = write!(chain, "\n  .skip({})", q.skip);
            }
            if q.limit > 0 {
                let _ = write!(chain, "\n  .limit({})", q.limit);
            }
            let _ = writeln!(out, "\nresult = {chain}");
        }
        Operation::Aggregate(stages) => {
            let _ = writeln!(
                out,
                "pipeline = {}",
                render(&pipeline_value(stages), &RUBY, 0)
            );
            let _ = writeln!(
                out,
                "\nresult = collection.aggregate(pipeline, allow_disk_use: true)"
            );
        }
    }
    out.push_str("result.each do |document|\n  puts document\nend\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{doc, oid::ObjectId};

    fn request(operation: Operation) -> CodeRequest {
        CodeRequest {
            uri: "mongodb://localhost:27017".into(),
            database: "shop".into(),
            collection: "orders".into(),
            operation,
        }
    }

    fn find() -> Operation {
        Operation::Find(Query {
            filter: doc! {
                "status": "shipped",
                "total": {"$gte": 10.5},
                "customerId": ObjectId::parse_str("5f1d7f1e2b3c4d5e6f708192").unwrap(),
                "big": 1_i64 << 40,
                "flag": true,
                "missing": null,
            },
            projection: Some(doc! {"total": 1}),
            sort: Some(doc! {"total": -1}),
            skip: 5,
            limit: 10,
        })
    }

    #[test]
    fn every_language_renders_find_and_aggregate() {
        let aggregate = Operation::Aggregate(vec![
            doc! {"$match": {"status": "shipped"}},
            doc! {"$group": {"_id": "$customerId", "n": {"$sum": 1}}},
        ]);
        for language in Language::ALL {
            for operation in [find(), aggregate.clone()] {
                let code = generate(language, &request(operation));
                assert!(code.contains("shop"), "{language:?}\n{code}");
                assert!(code.contains("orders"), "{language:?}");
                assert!(code.contains("mongodb://localhost:27017"), "{language:?}");
                // Balanced brackets is a cheap syntax sanity check for every language.
                for (open, close) in [('{', '}'), ('[', ']'), ('(', ')')] {
                    let opens = code.matches(open).count();
                    let closes = code.matches(close).count();
                    assert_eq!(
                        opens, closes,
                        "{language:?} unbalanced {open}{close}\n{code}"
                    );
                }
            }
        }
    }

    #[test]
    fn language_specific_literals() {
        let code = |l| generate(l, &request(find()));
        let py = code(Language::Python);
        assert!(py.contains("\"flag\": True"));
        assert!(py.contains("\"missing\": None"));
        assert!(py.contains("ObjectId(\"5f1d7f1e2b3c4d5e6f708192\")"));
        assert!(py.contains("sort = [(\"total\", -1)]"));
        assert!(py.contains("Int64(1099511627776)"));
        let java = code(Language::Java);
        assert!(java.contains("new Document(\"status\", \"shipped\")"));
        assert!(java.contains(".append(\"big\", 1099511627776L)"));
        assert!(java.contains(".limit(10)"));
        let go = code(Language::Go);
        assert!(go.contains("{Key: \"status\", Value: \"shipped\"},"));
        assert!(go.contains("opts.SetLimit(10)"));
        let rust = code(Language::Rust);
        assert!(rust.contains("\"total\": {\n            \"$gte\": 10.5_f64"));
        assert!(rust.contains(".skip(5)"));
        let php = code(Language::Php);
        assert!(php.contains("'flag' => true"));
        assert!(php.contains("'limit' => 10"));
        let ruby = code(Language::Ruby);
        assert!(ruby.contains("'missing' => nil"));
        let cs = code(Language::CSharp);
        assert!(cs.contains("{ \"status\", \"shipped\" }"));
        let js = code(Language::JavaScript);
        assert!(js.contains("customerId: new ObjectId(\"5f1d7f1e2b3c4d5e6f708192\")"));
        assert!(js.contains("$gte: 10.5"));
        assert!(js.contains("limit: 10"));
    }

    #[test]
    fn strings_are_escaped() {
        let code = generate(
            Language::Php,
            &request(Operation::Find(Query {
                filter: doc! {"name": "O'Brien \\ x"},
                ..Query::default()
            })),
        );
        assert!(code.contains("'O\\'Brien \\\\ x'"));
        let js = generate(
            Language::JavaScript,
            &request(Operation::Find(Query {
                filter: doc! {"q": "say \"hi\"\n"},
                ..Query::default()
            })),
        );
        assert!(js.contains("\"say \\\"hi\\\"\\n\""));
    }
}
