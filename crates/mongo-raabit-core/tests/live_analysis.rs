mod common;

use mongo_raabit_core::aggregate::{Pipeline, RunOptions, Stage};
use mongo_raabit_core::bson::{Bson, Document, doc};
use mongo_raabit_core::schema::SampleMethod;
use mongo_raabit_core::sql::{SqlTranslation, translate};

async fn seed(conn: &mongo_raabit_core::Connection, db: &str) {
    let customers: Vec<Document> = (0..10)
        .map(|i| doc! {"_id": i, "name": format!("c{i}"), "tier": if i % 2 == 0 { "pro" } else { "free" }})
        .collect();
    conn.insert_documents(db, "customers", customers)
        .await
        .unwrap();
    let orders: Vec<Document> = (0..40)
        .map(|i| {
            doc! {
                "customerId": i % 10,
                "status": (if i % 2 == 0 { "placed" } else { "shipped" }),
                "total": f64::from(i) * 2.5,
                "items": (0..(i % 3 + 1)).map(|k| doc! {"sku": format!("s{k}"), "qty": k + 1}).collect::<Vec<_>>(),
            }
        })
        .collect();
    conn.insert_documents(db, "orders", orders).await.unwrap();
}

#[tokio::test]
async fn aggregation_stage_preview() {
    let Some((conn, db)) = common::connect("aggregate").await else {
        return;
    };
    seed(&conn, &db).await;
    let mut pipeline = Pipeline::from_text(
        "[{$match: {status: 'shipped'}}, {$group: {_id: '$customerId', spent: {$sum: '$total'}}}, \
         {$sort: {spent: -1}}, {$out: 'report'}]",
    )
    .unwrap();
    let preview = conn
        .preview_stage(&db, "orders", &pipeline, 1, 3, RunOptions::default())
        .await
        .unwrap();
    assert_eq!(preview.input.len(), 3);
    assert!(
        preview
            .input
            .iter()
            .all(|d| d.get_str("status") == Ok("shipped"))
    );
    assert_eq!(preview.output.len(), 3);
    assert!(preview.output.iter().all(|d| d.contains_key("spent")));

    // Disabled stages pass input through.
    pipeline.stages[0].enabled = false;
    let preview = conn
        .preview_stage(&db, "orders", &pipeline, 0, 100, RunOptions::default())
        .await
        .unwrap();
    assert_eq!(preview.input.len(), 40);
    assert_eq!(preview.output, preview.input);

    // $out is never executed by a preview.
    let preview = conn
        .preview_stage(&db, "orders", &pipeline, 3, 5, RunOptions::default())
        .await
        .unwrap();
    assert!(preview.skipped_reason.is_some());
    assert!(
        !conn
            .collections(&db)
            .await
            .unwrap()
            .iter()
            .any(|c| c.name == "report")
    );

    // Running the full pipeline writes it; input sampling limits the input.
    let docs = pipeline.documents(None).unwrap();
    conn.aggregate(&db, "orders", docs, RunOptions::default(), 0)
        .await
        .unwrap();
    assert_eq!(
        conn.collection(&db, "report")
            .count_documents(doc! {})
            .await
            .unwrap(),
        10
    );
    let sampled = Pipeline {
        stages: vec![Stage {
            operator: "$count".into(),
            body: "'n'".into(),
            enabled: true,
        }],
    };
    let (rows, _) = conn
        .aggregate(
            &db,
            "orders",
            sampled.documents(None).unwrap(),
            RunOptions {
                input_sample: 7,
                ..RunOptions::default()
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(rows, vec![doc! {"n": 7}]);
    conn.drop_database(&db).await.unwrap();
}

#[tokio::test]
async fn schema_sampling() {
    let Some((conn, db)) = common::connect("schema").await else {
        return;
    };
    seed(&conn, &db).await;
    for method in [
        SampleMethod::Random,
        SampleMethod::First,
        SampleMethod::Last,
    ] {
        let report = conn
            .sample_schema(&db, "orders", 25, method, doc! {})
            .await
            .unwrap();
        assert_eq!(report.sampled, 25);
        let total = report.fields.iter().find(|f| f.path == "total").unwrap();
        assert_eq!(total.primary_type(), Some("Double"));
        assert_eq!(total.probability(), 1.0);
        assert!(report.fields.iter().any(|f| f.path == "items[].sku"));
    }
    let filtered = conn
        .sample_schema(
            &db,
            "orders",
            100,
            SampleMethod::First,
            doc! {"status": "placed"},
        )
        .await
        .unwrap();
    assert_eq!(filtered.sampled, 20);
    conn.drop_database(&db).await.unwrap();
}

#[tokio::test]
async fn sql_translations_run_on_the_server() {
    let Some((conn, db)) = common::connect("sql").await else {
        return;
    };
    seed(&conn, &db).await;
    let run = |sql: &'static str| {
        let conn = conn.clone();
        let db = db.clone();
        async move {
            let translation = translate(sql).unwrap();
            let (rows, _) = conn
                .aggregate(
                    &db,
                    translation.collection(),
                    translation.pipeline(),
                    RunOptions::default(),
                    0,
                )
                .await
                .unwrap();
            (translation, rows)
        }
    };

    let (t, rows) =
        run("SELECT name FROM customers WHERE tier = 'pro' ORDER BY name LIMIT 2").await;
    assert!(matches!(t, SqlTranslation::Find { .. }));
    assert_eq!(rows, vec![doc! {"name": "c0"}, doc! {"name": "c2"}]);

    let (_, rows) = run(
        "SELECT status, COUNT(*) AS n, SUM(total) AS revenue FROM orders GROUP BY status \
         HAVING COUNT(*) >= 20 ORDER BY status",
    )
    .await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get_str("status").unwrap(), "placed");
    assert_eq!(rows[0].get_i32("n").unwrap(), 20);

    let (_, rows) = run(
        "SELECT c.name AS customer, SUM(o.total) AS spent FROM orders o \
         JOIN customers c ON o.customerId = c._id WHERE c.tier = 'free' \
         GROUP BY c.name ORDER BY spent DESC LIMIT 1",
    )
    .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_str("customer").unwrap(), "c9");
    assert!(matches!(rows[0].get("spent"), Some(Bson::Double(v)) if *v > 0.0));
    conn.drop_database(&db).await.unwrap();
}
