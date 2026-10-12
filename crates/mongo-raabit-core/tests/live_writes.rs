//! The guarded write path against a live server: previews count what a write touches, and
//! a read-only connection refuses every change before it reaches the server.

mod common;

use mongo_raabit_core::bson::{Bson, Document, doc};
use mongo_raabit_core::connection::NewIndex;
use mongo_raabit_core::query::Query;
use mongo_raabit_core::{Affected, Connection, RaabitError, Write};

async fn seed(conn: &Connection, db: &str) {
    conn.execute(Write::InsertDocuments {
        db: db.to_owned(),
        collection: "orders".into(),
        documents: (0..10)
            .map(|i| doc! {"_id": i, "status": if i % 2 == 0 { "open" } else { "done" }, "n": i})
            .collect(),
    })
    .await
    .unwrap();
}

async fn documents(conn: &Connection, db: &str) -> Vec<Document> {
    conn.find_page(db, "orders", &Query::default(), 0, 100)
        .await
        .unwrap()
        .documents
}

#[tokio::test]
async fn previews_count_what_a_write_touches() {
    let Some((conn, db)) = common::connect("write_preview").await else {
        return;
    };
    seed(&conn, &db).await;
    let ns = |db: &str| (db.to_owned(), "orders".to_owned());

    let (d, c) = ns(&db);
    let update = Write::UpdateMany {
        db: d,
        collection: c,
        filter: doc! {"status": "open"},
        update: doc! {"$set": {"flag": true}},
    };
    let preview = conn.preview(&update).await.unwrap();
    assert_eq!(preview.affected, Affected::Documents(5));
    assert_eq!(preview.shell, update.shell());
    assert_eq!(
        update.summary(&preview.affected),
        "5 documents will be modified"
    );

    let (d, c) = ns(&db);
    let delete_one = Write::DeleteOne {
        db: d,
        collection: c,
        filter: doc! {"status": "done"},
    };
    assert_eq!(
        conn.preview(&delete_one).await.unwrap().affected,
        Affected::Documents(1)
    );
    let (d, c) = ns(&db);
    let edit = Write::SetField {
        db: d,
        collection: c,
        id: Bson::Int32(404),
        path: "n".into(),
        value: Some(Bson::Int32(1)),
        original: Some(Bson::Int32(404)),
    };
    assert_eq!(
        conn.preview(&edit).await.unwrap().affected,
        Affected::Documents(0)
    );

    let (d, c) = ns(&db);
    let drop = Write::DropCollection {
        db: d,
        collection: c,
    };
    assert_eq!(
        conn.preview(&drop).await.unwrap().affected,
        Affected::Collection {
            name: "orders".into(),
            documents: 10
        }
    );
    assert_eq!(
        conn.preview(&Write::DropDatabase { db: db.clone() })
            .await
            .unwrap()
            .affected,
        Affected::Database {
            collections: 1,
            documents: 10
        }
    );
    // Previews change nothing.
    assert_eq!(documents(&conn, &db).await.len(), 10);
    assert!(
        documents(&conn, &db)
            .await
            .iter()
            .all(|d| !d.contains_key("flag"))
    );

    conn.execute(Write::DropDatabase { db: db.clone() })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_read_only_connection_changes_nothing() {
    let Some((conn, db)) = common::connect("write_read_only").await else {
        return;
    };
    let Some(read_only) = common::connect_read_only().await else {
        panic!("the server answered the first connection");
    };
    assert!(read_only.is_read_only());
    assert!(!conn.is_read_only());
    seed(&conn, &db).await;
    let before = documents(&conn, &db).await;

    let (d, c) = (db.clone(), "orders".to_owned());
    let writes = vec![
        Write::SetField {
            db: d.clone(),
            collection: c.clone(),
            id: Bson::Int32(1),
            path: "n".into(),
            value: Some(Bson::Int32(100)),
            original: Some(Bson::Int32(1)),
        },
        Write::InsertDocuments {
            db: d.clone(),
            collection: c.clone(),
            documents: vec![doc! {"x": 1}],
        },
        Write::DeleteMany {
            db: d.clone(),
            collection: c.clone(),
            filter: doc! {},
        },
        Write::CreateIndex {
            db: d.clone(),
            collection: c.clone(),
            index: NewIndex {
                keys: doc! {"n": 1},
                ..NewIndex::default()
            },
        },
        Write::DropCollection {
            db: d.clone(),
            collection: c.clone(),
        },
        Write::Aggregate {
            db: d.clone(),
            collection: c.clone(),
            pipeline: vec![doc! {"$out": "copy"}],
        },
        Write::DropDatabase { db: d.clone() },
    ];
    for write in writes {
        // The preview still works: it only reads.
        read_only.preview(&write).await.unwrap();
        let err = read_only.execute(write.clone()).await.unwrap_err();
        assert!(matches!(err, RaabitError::ReadOnly(_)), "{write:?}: {err}");
        assert!(err.to_string().contains(&write.title()), "{err}");
    }

    // Commands: reads run, anything else is refused.
    read_only
        .run_command(&db, doc! {"count": "orders"})
        .await
        .unwrap();
    let err = read_only
        .run_command(
            &db,
            doc! {"delete": "orders", "deletes": [{"q": {}, "limit": 0}]},
        )
        .await
        .unwrap_err();
    assert!(matches!(err, RaabitError::ReadOnly(_)), "{err}");

    // An aggregation through the query path cannot write either.
    let err = read_only
        .aggregate(
            &db,
            "orders",
            vec![doc! {"$merge": "copy"}],
            Default::default(),
            0,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, RaabitError::ReadOnly(_)), "{err}");

    // The built-in shell reads but does not write.
    let mut current = db.clone();
    let out = read_only
        .eval_builtin(
            &mut current,
            "db.orders.countDocuments()\ndb.orders.deleteMany({})",
        )
        .await;
    assert_eq!(out[0].result, Ok(Bson::Int64(10)));
    assert!(
        out[1].result.as_ref().unwrap_err().contains("read-only"),
        "{out:?}"
    );
    let out = read_only
        .eval_builtin(&mut current, "db.runCommand({dropDatabase: 1})")
        .await;
    assert!(
        out[0].result.as_ref().unwrap_err().contains("read-only"),
        "{out:?}"
    );

    // mongosh cannot be limited to reads: it is not started at all.
    let err = read_only
        .run_mongosh(
            std::path::Path::new("mongosh"),
            &db,
            "db.orders.drop()",
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, RaabitError::ReadOnly(_)), "{err}");

    assert_eq!(documents(&conn, &db).await, before, "nothing changed");
    let collections = conn.collections(&db).await.unwrap();
    assert_eq!(collections.len(), 1, "no $out or $merge target was created");
    assert_eq!(conn.indexes(&db, "orders").await.unwrap().len(), 1);

    conn.execute(Write::DropDatabase { db: db.clone() })
        .await
        .unwrap();
}
