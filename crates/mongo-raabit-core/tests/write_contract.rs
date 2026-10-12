//! The contract every [`Write`] keeps (see `docs/safety.md`):
//!
//! - every variant of the enum is listed in `write::KINDS` and has a sample in
//!   `write::samples`, which the live test (`live_write_paths.rs`) runs against a server,
//!   checking the exact result and the read-only refusal;
//! - the text a confirmation shows, [`Write::shell`], read back by the built-in shell
//!   without running it ([`shell_eval::script_plan`]), is the same write
//!   ([`Write::canonical`]): unit tests for every variant, and a property test over
//!   random values for the writes users make most.
//!
//! A variant added to the enum without a sample, a live test and a round trip fails
//! here.

use mongo_raabit_core::bson::{
    Binary, Bson, DateTime, Decimal128, Document, Regex, Timestamp, oid::ObjectId,
    spec::BinarySubtype,
};
use mongo_raabit_core::shell_eval::script_plan;
use mongo_raabit_core::write::{KINDS, sample_seed, samples};
use mongo_raabit_core::{Affected, Write, WriteOutcome};
use proptest::prelude::*;

/// Variants whose confirmation text is a description that cannot be run back, and why.
const NOT_ROUND_TRIPPED: &[(&str, &str)] = &[
    (
        "CreateUser",
        "the password is never shown: the text says passwordPrompt() in its place",
    ),
    (
        "Import",
        "an import of a file shows its first documents and how many follow, and as a \
         loop over them for upserts; the import itself is checked by the live test",
    ),
];

/// The variant names of `pub enum Write` in `src/write.rs`, read from the source.
fn variants_in_source() -> Vec<String> {
    let source = include_str!("../src/write.rs");
    let start = source
        .find("pub enum Write {")
        .expect("write.rs declares `pub enum Write`");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the enum ends");
    body[..end]
        .lines()
        .skip(1)
        .filter_map(|line| {
            let name = line.strip_prefix("    ")?;
            let first = name.chars().next()?;
            if !first.is_ascii_uppercase() {
                return None;
            }
            Some(
                name.chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn every_variant_is_listed_and_has_a_sample() {
    let source = variants_in_source();
    assert_eq!(source.len(), 25, "variants read from write.rs: {source:?}");
    assert_eq!(
        source,
        KINDS.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
        "write::KINDS lists every variant of `Write`, in order"
    );
    let sampled: Vec<&str> = samples("shop").iter().map(Write::kind).collect();
    assert_eq!(
        sampled, KINDS,
        "write::samples has one sample of every variant, in order"
    );
}

#[test]
fn every_variant_has_a_live_and_a_read_only_test() {
    let live = include_str!("live_write_paths.rs");
    for kind in KINDS {
        assert!(
            live.contains(&format!("Write::{kind} {{")),
            "live_write_paths.rs checks the result of every write: add an arm for Write::{kind}"
        );
    }
    // The read-only test runs every sample.
    assert!(live.contains("fn a_read_only_connection_refuses_every_write"));
}

/// Two writes are the same when their debug forms are: stricter than `==`, which takes
/// `-0.0` for `0.0` and never matches `NaN`.
fn same(a: &Write, b: &Write) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

/// `write.shell()` read back by the built-in shell, on the write's database.
fn read_back(write: &Write) -> Result<Vec<Write>, String> {
    script_plan(write.db(), &write.shell()).map_err(|e| e.to_string())
}

fn assert_round_trip(write: &Write) {
    let text = write.shell();
    let back = read_back(write).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(back.len(), 1, "one statement: {text}");
    assert!(
        same(&back[0], &write.canonical()),
        "the shell text reads back as a different write\ntext: {text}\nwrite: {:?}\nread:  {:?}",
        write.canonical(),
        back[0]
    );
}

#[test]
fn every_write_reads_back_from_its_shell_text() {
    for write in samples("shop") {
        if NOT_ROUND_TRIPPED.iter().any(|(k, _)| *k == write.kind()) {
            continue;
        }
        assert_round_trip(&write);
    }
    for (kind, why) in NOT_ROUND_TRIPPED {
        assert!(KINDS.contains(kind), "{kind} is a variant");
        assert!(!why.is_empty());
    }
}

#[test]
fn writes_name_the_database_they_change() {
    // Collection writes are relative to the current database; the shell reads them on
    // the write's database, so read on another one they change that one instead.
    let write = Write::DropCollection {
        db: "shop".into(),
        collection: "orders".into(),
    };
    let elsewhere = script_plan("archive", &write.shell()).unwrap();
    assert_eq!(elsewhere[0].db(), "archive");
    // Database writes name theirs.
    let drop = Write::DropDatabase { db: "shop".into() };
    assert_eq!(script_plan("archive", &drop.shell()).unwrap()[0], drop);
}

#[test]
fn the_specific_writes_read_back_as_the_call_they_run() {
    let edit = &samples("shop")[3];
    let Write::UpdateOne { filter, update, .. } = edit.canonical() else {
        panic!("an inline edit is an updateOne");
    };
    assert_eq!(
        filter,
        mongo_raabit_core::bson::doc! {
            "_id": 2,
            "address.city": {"$eq": "Oslo", "$type": "string", "$not": {"$type": "array"}},
        }
    );
    assert_eq!(
        update,
        mongo_raabit_core::bson::doc! {"$set": {"address.city": "Bergen"}}
    );
    let save = &samples("shop")[2];
    let Write::ReplaceOne { filter, .. } = save.canonical() else {
        panic!("a Save is a replaceOne");
    };
    assert_eq!(
        filter,
        mongo_raabit_core::write::replace_filter(&Bson::Int32(1), &sample_seed()[0])
    );
}

#[test]
fn long_inserts_show_their_first_documents_and_the_count() {
    // An insert of more than three documents shows the first three and says how many
    // follow; what it shows reads back as those three.
    let documents: Vec<Document> = (0..5)
        .map(|i| mongo_raabit_core::bson::doc! {"i": i})
        .collect();
    let write = Write::InsertDocuments {
        db: "shop".into(),
        collection: "orders".into(),
        documents: documents.clone(),
    };
    let text = write.shell();
    assert!(text.contains("// … and 2 more documents"), "{text}");
    let back = read_back(&write).unwrap();
    assert_eq!(
        back,
        [Write::InsertDocuments {
            db: "shop".into(),
            collection: "orders".into(),
            documents: documents[..3].to_vec(),
        }]
    );
}

#[test]
fn outcomes_are_checked_against_the_preview() {
    let [_, _, _, edit, _, update, ..] = <[Write; 25]>::try_from(samples("shop")).unwrap();
    assert_eq!(
        update.check_outcome(
            Some(&Affected::Documents(12)),
            &WriteOutcome::Updated {
                matched: 14,
                modified: 14
            }
        ),
        Some(
            "Expected 12 documents, 14 were matched — the data changed between preview and run"
                .into()
        )
    );
    assert_eq!(
        update.check_outcome(
            Some(&Affected::Documents(12)),
            &WriteOutcome::Updated {
                matched: 12,
                modified: 3
            }
        ),
        None,
        "documents that already had the value are matched, not modified"
    );
    assert_eq!(
        update.check_outcome(
            Some(&Affected::Unknown),
            &WriteOutcome::Updated {
                matched: 1,
                modified: 1
            }
        ),
        None
    );
    assert_eq!(
        edit.check_outcome(
            Some(&Affected::Documents(1)),
            &WriteOutcome::Updated {
                matched: 1,
                modified: 1
            }
        ),
        None
    );
    let delete = &samples("shop")[7];
    assert_eq!(
        delete.check_outcome(Some(&Affected::Documents(2)), &WriteOutcome::Deleted(1)),
        Some("Deleted 1 of 2 selected documents: 1 no longer existed".into())
    );
    assert_eq!(delete.check_outcome(None, &WriteOutcome::Deleted(2)), None);
    let many = &samples("shop")[9];
    assert_eq!(
        many.check_outcome(Some(&Affected::Documents(1)), &WriteOutcome::Deleted(0)),
        Some(
            "Expected 1 document, 0 were deleted — the data changed between preview and run".into()
        )
    );
}

// ----- property test ------------------------------------------------------------------

/// Text with the characters that break quoting: quotes, backslashes, line breaks,
/// control characters, `//` and `/*`, brackets, and any Unicode.
fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        "\\PC{0,12}",
        proptest::collection::vec(
            prop_oneof![
                Just("\"".to_owned()),
                Just("'".to_owned()),
                Just("\\".to_owned()),
                Just("\n".to_owned()),
                Just("\r\t".to_owned()),
                Just("\u{0}".to_owned()),
                Just("\u{1f}".to_owned()),
                Just("//".to_owned()),
                Just("/*".to_owned()),
                Just("${}".to_owned()),
                Just("({[".to_owned()),
                Just("]})".to_owned()),
                Just(";".to_owned()),
                Just("é漢🐇".to_owned()),
                Just("\u{2028}".to_owned()),
                "[a-z]{1,3}",
            ],
            0..6
        )
        .prop_map(|parts| parts.concat()),
    ]
}

/// A field name: anything a document may hold except a leading `$`.
fn key() -> impl Strategy<Value = String> {
    text().prop_filter("no operator-like keys", |k| !k.starts_with('$'))
}

fn decimal() -> impl Strategy<Value = Bson> {
    prop_oneof![
        (any::<i64>(), -40i32..40).prop_map(|(m, e)| format!("{m}E{e}")),
        Just("0".to_owned()),
        Just("-0".to_owned()),
        Just("0.1".to_owned()),
        Just("9999999999999999999999999999999999".to_owned()),
        Just("1E+6144".to_owned()),
        Just("1E-6176".to_owned()),
        Just("NaN".to_owned()),
        Just("Infinity".to_owned()),
        Just("-Infinity".to_owned()),
    ]
    .prop_map(|text| Bson::Decimal128(text.parse::<Decimal128>().expect("valid decimal")))
}

fn double() -> impl Strategy<Value = f64> {
    prop_oneof![
        any::<f64>(),
        Just(0.0),
        Just(-0.0),
        Just(f64::NAN),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        Just(f64::MAX),
        Just(f64::MIN),
        Just(f64::MIN_POSITIVE),
        Just(f64::EPSILON),
        Just(1e15),
        Just(-1e15),
        Just(9_007_199_254_740_993.0),
        Just(1e300),
        Just(0.1),
        (-1_000_000i64..1_000_000).prop_map(|i| i as f64),
    ]
}

/// Scalars of every kind the views show, with their edge values.
fn scalar() -> impl Strategy<Value = Bson> {
    prop_oneof![
        text().prop_map(Bson::String),
        prop_oneof![
            any::<i32>(),
            Just(i32::MIN),
            Just(i32::MAX),
            Just(0),
            Just(-1)
        ]
        .prop_map(Bson::Int32),
        prop_oneof![
            any::<i64>(),
            Just(i64::MIN),
            Just(i64::MAX),
            Just(0),
            Just(i64::from(i32::MAX) + 1)
        ]
        .prop_map(Bson::Int64),
        double().prop_map(Bson::Double),
        decimal(),
        any::<bool>().prop_map(Bson::Boolean),
        Just(Bson::Null),
        any::<[u8; 12]>().prop_map(|b| Bson::ObjectId(ObjectId::from_bytes(b))),
        prop_oneof![
            any::<i64>(),
            Just(0i64),
            Just(-1),
            Just(253_402_300_799_999),
            Just(253_402_300_800_000),
            Just(-62_135_596_800_000),
            -100_000_000_000_000i64..100_000_000_000_000,
        ]
        .prop_map(|ms| Bson::DateTime(DateTime::from_millis(ms))),
        (
            text(),
            proptest::sample::subsequence(vec!['i', 'm', 's', 'x'], 0..=4)
        )
            .prop_map(|(pattern, flags)| Bson::RegularExpression(Regex {
                pattern,
                options: flags.into_iter().collect(),
            })),
        (any::<u32>(), any::<u32>())
            .prop_map(|(time, increment)| Bson::Timestamp(Timestamp { time, increment })),
        (
            prop_oneof![Just(0u8), Just(4u8), Just(5u8), Just(0x80u8)],
            proptest::collection::vec(any::<u8>(), 0..20)
        )
            .prop_map(|(subtype, bytes)| Bson::Binary(Binary {
                subtype: BinarySubtype::from(subtype),
                bytes,
            })),
        Just(Bson::MinKey),
        Just(Bson::MaxKey),
    ]
}

/// Scalars, arrays and nested documents (three levels deep at most).
fn value() -> impl Strategy<Value = Bson> {
    scalar().prop_recursive(3, 24, 5, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..5).prop_map(Bson::Array),
            proptest::collection::vec((key(), inner), 0..5)
                .prop_map(|fields| Bson::Document(fields.into_iter().collect())),
        ]
    })
}

fn document() -> impl Strategy<Value = Document> {
    proptest::collection::vec((key(), value()), 0..6).prop_map(|f| f.into_iter().collect())
}

/// What an `_id` can be: no array, no regular expression.
fn id() -> impl Strategy<Value = Bson> {
    value().prop_filter("a valid _id", |v| {
        !matches!(v, Bson::Array(_) | Bson::RegularExpression(_))
    })
}

/// A dotted field path an inline edit can set: no empty segment, no `$` at the start,
/// not `_id`.
fn path() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        key().prop_filter("a path segment", |s| !s.is_empty() && !s.contains('.')),
        1..4,
    )
    .prop_map(|segments| segments.join("."))
    .prop_filter("not _id", |p| p != "_id" && !p.starts_with("_id."))
}

fn namespace() -> impl Strategy<Value = (String, String)> {
    (
        "[a-z][a-z0-9_]{0,10}",
        text().prop_filter("a name", |n| !n.is_empty()),
    )
}

fn edits() -> impl Strategy<Value = Write> {
    prop_oneof![
        (
            namespace(),
            id(),
            path(),
            proptest::option::of(value()),
            proptest::option::of(value())
        )
            .prop_map(|((db, collection), id, path, value, original)| {
                Write::SetField {
                    db,
                    collection,
                    id,
                    path,
                    value,
                    original,
                }
            }),
        (namespace(), id(), document(), document()).prop_map(
            |((db, collection), id, mut original, mut replacement)| {
                original.insert("_id", id.clone());
                replacement.insert("_id", id.clone());
                Write::ReplaceDocument {
                    db,
                    collection,
                    id,
                    original,
                    replacement,
                }
            }
        ),
        (namespace(), document(), document()).prop_map(|((db, collection), filter, update)| {
            Write::UpdateMany {
                db,
                collection,
                filter,
                update,
            }
        }),
        (namespace(), document()).prop_map(|((db, collection), filter)| Write::DeleteMany {
            db,
            collection,
            filter,
        }),
        (namespace(), proptest::collection::vec(document(), 0..=3)).prop_map(
            |((db, collection), documents)| Write::InsertDocuments {
                db,
                collection,
                documents,
            }
        ),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_writes_read_back_from_their_shell_text(write in edits()) {
        let text = write.shell();
        let back = read_back(&write);
        prop_assert!(back.is_ok(), "{:?}\n{}", back, text);
        let back = back.unwrap();
        prop_assert_eq!(back.len(), 1, "{}", text);
        prop_assert!(
            same(&back[0], &write.canonical()),
            "text: {}\nwrite: {:?}\nread:  {:?}",
            text,
            write.canonical(),
            back[0]
        );
    }

    #[test]
    fn random_values_read_back_from_shell_text(v in value()) {
        use mongo_raabit_core::shell::{ShellStyle, parse_value, to_shell_string};
        for style in [ShellStyle::Compact, ShellStyle::Pretty(2)] {
            let text = to_shell_string(&v, style);
            let back = parse_value(&text);
            prop_assert!(back.is_ok(), "{:?}\n{}", back, text);
            let back = back.unwrap();
            prop_assert_eq!(format!("{:?}", back), format!("{:?}", v), "{}", text);
        }
    }
}
