mod common;

use mongo_raabit_core::bson::{Document, doc};
use mongo_raabit_core::query::Query;
use mongo_raabit_core::transfer::{
    ExportFormat, ExportSource, Exporter, ImportMode, JsonMode, read_bson, read_json,
};
use std::sync::atomic::AtomicBool;

#[tokio::test]
async fn export_then_import_round_trip() {
    let Some((conn, db)) = common::connect("transfer").await else {
        return;
    };
    let docs: Vec<Document> = (0..2500)
        .map(|i| doc! {"_id": i, "name": format!("n{i}"), "nested": {"v": i % 7}})
        .collect();
    conn.insert_documents(&db, "source", docs.clone())
        .await
        .unwrap();
    let cancel = AtomicBool::new(false);
    let source = ExportSource::Query(Query {
        sort: Some(doc! {"_id": 1}),
        ..Query::default()
    });

    for format in [
        ExportFormat::Bson,
        ExportFormat::JsonLines {
            mode: JsonMode::Canonical,
        },
    ] {
        let mut exporter = Exporter::new(Vec::new(), format.clone()).unwrap();
        let count = conn
            .export(&db, "source", &source, &mut exporter, &cancel, &|_| {})
            .await
            .unwrap();
        assert_eq!(count, 2500);
        let (bytes, _) = exporter.into_inner().unwrap();
        let mut back = Vec::new();
        let sink = |d| {
            back.push(d);
            Ok(())
        };
        if format == ExportFormat::Bson {
            read_bson(bytes.as_slice(), sink).unwrap();
        } else {
            read_json(bytes.as_slice(), sink).unwrap();
        }
        assert_eq!(back, docs);

        let report = conn
            .import(
                &db,
                "target",
                back.clone(),
                ImportMode::DropAndInsert,
                &cancel,
                &|_| {},
            )
            .await
            .unwrap();
        assert_eq!(report.inserted, 2500);
        assert_eq!(report.failed, 0);
    }

    // Insert again: every _id collides, failures are reported but not fatal.
    let mut again = docs[..10].to_vec();
    again.push(doc! {"_id": 99_999, "name": "new"});
    let report = conn
        .import(
            &db,
            "target",
            again.clone(),
            ImportMode::Insert,
            &cancel,
            &|_| {},
        )
        .await
        .unwrap();
    assert_eq!(report.inserted, 1);
    assert_eq!(report.failed, 10);
    assert!(report.failures[0].1.contains("duplicate key"));

    // Upsert replaces existing documents.
    again[0].insert("name", "changed");
    let report = conn
        .import(&db, "target", again, ImportMode::Upsert, &cancel, &|_| {})
        .await
        .unwrap();
    assert_eq!(report.replaced, 11);
    let changed = conn
        .collection(&db, "target")
        .find_one(doc! {"_id": 0})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.get_str("name").unwrap(), "changed");

    // Aggregation export.
    let mut exporter = Exporter::new(
        Vec::new(),
        ExportFormat::JsonArray {
            mode: JsonMode::Relaxed,
            pretty: false,
        },
    )
    .unwrap();
    let count = conn
        .export(
            &db,
            "source",
            &ExportSource::Aggregate(vec![doc! {"$group": {"_id": "$nested.v"}}]),
            &mut exporter,
            &cancel,
            &|_| {},
        )
        .await
        .unwrap();
    assert_eq!(count, 7);
    conn.drop_database(&db).await.unwrap();
}
