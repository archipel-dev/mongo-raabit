mod common;

use mongo_raabit_core::bson::{Bson, doc};
use mongo_raabit_core::connection::{CollectionKind, NewIndex, RoleRef};
use mongo_raabit_core::query::QueryText;

#[tokio::test]
async fn metadata_and_server_info() {
    let Some((conn, db)) = common::connect("metadata").await else {
        return;
    };
    assert!(!conn.server().version.is_empty());
    assert!(!conn.server().topology.is_empty());

    conn.create_database(&db, "people").await.unwrap();
    conn.collection(&db, "people")
        .insert_many((0..50).map(|i| doc! {"n": i, "name": format!("p{i}")}))
        .await
        .unwrap();
    conn.create_view(
        &db,
        "adults",
        "people",
        vec![doc! {"$match": {"n": {"$gte": 18}}}],
    )
    .await
    .unwrap();
    conn.create_collection(&db, "fs.files", doc! {})
        .await
        .unwrap();
    conn.create_collection(&db, "fs.chunks", doc! {})
        .await
        .unwrap();

    let dbs = conn.databases().await.unwrap();
    assert!(dbs.iter().any(|d| d.name == db));

    let collections = conn.collections(&db).await.unwrap();
    let kind = |name: &str| collections.iter().find(|c| c.name == name).map(|c| c.kind);
    assert_eq!(kind("people"), Some(CollectionKind::Collection));
    assert_eq!(kind("adults"), Some(CollectionKind::View));
    assert_eq!(kind("fs"), Some(CollectionKind::GridFsBucket));
    let view = collections.iter().find(|c| c.name == "adults").unwrap();
    assert_eq!(view.view_on.as_deref(), Some("people"));
    assert_eq!(view.pipeline.len(), 1);

    let stats = conn.collection_stats(&db, "people").await.unwrap();
    assert_eq!(stats.count, 50);
    assert!(stats.size > 0);

    conn.rename_collection(&db, "people", &db, "humans", false)
        .await
        .unwrap();
    assert!(
        conn.collections(&db)
            .await
            .unwrap()
            .iter()
            .any(|c| c.name == "humans")
    );
    assert!(
        conn.create_collection(&db, "system.bad", doc! {})
            .await
            .is_err()
    );

    let status = conn.server_status().await.unwrap();
    assert!(status.contains_key("uptime"));
    let ops = conn.current_ops(false).await.unwrap();
    assert!(ops.iter().all(|op| !op.op.is_empty()));

    conn.drop_database(&db).await.unwrap();
}

#[tokio::test]
async fn index_manager() {
    let Some((conn, db)) = common::connect("indexes").await else {
        return;
    };
    conn.collection(&db, "orders")
        .insert_many((0..20).map(|i| doc! {"total": i, "sku": format!("s{i}")}))
        .await
        .unwrap();
    let name = conn
        .create_index(
            &db,
            "orders",
            NewIndex {
                keys: doc! {"sku": 1},
                unique: true,
                ..NewIndex::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(name, "sku_1");
    conn.create_index(
        &db,
        "orders",
        NewIndex {
            keys: doc! {"total": -1},
            name: Some("by_total".into()),
            partial_filter: Some(doc! {"total": {"$gt": 5}}),
            ..NewIndex::default()
        },
    )
    .await
    .unwrap();
    conn.set_index_hidden(&db, "orders", "by_total", true)
        .await
        .unwrap();
    let indexes = conn.indexes(&db, "orders").await.unwrap();
    let sku = indexes.iter().find(|i| i.name == "sku_1").unwrap();
    assert!(sku.unique);
    assert!(sku.size.unwrap_or(0) > 0);
    let by_total = indexes.iter().find(|i| i.name == "by_total").unwrap();
    assert!(by_total.hidden);
    assert!(by_total.partial_filter.is_some());
    assert!(conn.drop_index(&db, "orders", "_id_").await.is_err());
    conn.drop_index(&db, "orders", "by_total").await.unwrap();
    assert_eq!(conn.indexes(&db, "orders").await.unwrap().len(), 2);
    conn.drop_database(&db).await.unwrap();
}

#[tokio::test]
async fn query_paging_crud_and_explain() {
    let Some((conn, db)) = common::connect("query").await else {
        return;
    };
    let ids = conn
        .insert_documents(
            &db,
            "items",
            (0..95)
                .map(|i| doc! {"i": i, "even": i % 2 == 0, "nested": {"v": i * 10}})
                .collect(),
        )
        .await
        .unwrap();
    assert_eq!(ids.len(), 95);

    let query = QueryText {
        filter: "{even: true}".into(),
        projection: "{i: 1, _id: 0}".into(),
        sort: "{i: -1}".into(),
        ..QueryText::default()
    }
    .parse()
    .unwrap();
    assert_eq!(conn.count(&db, "items", &query).await.unwrap(), 48);
    let first = conn.find_page(&db, "items", &query, 0, 20).await.unwrap();
    assert_eq!(first.documents.len(), 20);
    assert!(first.has_more);
    assert_eq!(first.documents[0], doc! {"i": 94});
    let last = conn.find_page(&db, "items", &query, 2, 20).await.unwrap();
    assert_eq!(last.documents.len(), 8);
    assert!(!last.has_more);
    assert_eq!(last.first_index(), 41);

    let limited = QueryText {
        limit: "25".into(),
        skip: "3".into(),
        ..QueryText::default()
    }
    .parse()
    .unwrap();
    assert_eq!(conn.count(&db, "items", &limited).await.unwrap(), 25);
    let page = conn.find_page(&db, "items", &limited, 1, 20).await.unwrap();
    assert_eq!(page.documents.len(), 5);
    assert!(!page.has_more);

    // Explain: collection scan first, index scan after creating an index.
    let by_i = QueryText {
        filter: "{i: {$gte: 90}}".into(),
        ..QueryText::default()
    }
    .parse()
    .unwrap();
    let explain = conn.explain(&db, "items", &by_i).await.unwrap();
    assert!(explain.root.has_collection_scan());
    assert_eq!(explain.n_returned, 5);
    conn.create_index(
        &db,
        "items",
        NewIndex {
            keys: doc! {"i": 1},
            ..NewIndex::default()
        },
    )
    .await
    .unwrap();
    let explain = conn.explain(&db, "items", &by_i).await.unwrap();
    assert_eq!(explain.indexes_used, ["i_1"]);
    assert!(!explain.root.has_collection_scan());

    // CRUD by _id.
    let id = ids[0].clone();
    conn.set_field(
        &db,
        "items",
        &id,
        "nested.v",
        Some(Bson::String("edited".into())),
    )
    .await
    .unwrap();
    conn.set_field(&db, "items", &id, "even", None)
        .await
        .unwrap();
    assert!(
        conn.set_field(&db, "items", &id, "_id", None)
            .await
            .is_err()
    );
    let edited = conn
        .collection(&db, "items")
        .find_one(doc! {"_id": id.clone()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        edited.get_document("nested").unwrap().get_str("v").unwrap(),
        "edited"
    );
    assert!(!edited.contains_key("even"));

    let clone_id = conn.clone_document(&db, "items", &edited).await.unwrap();
    assert_ne!(clone_id, id);
    let mut replacement = edited.clone();
    replacement.insert("i", -1);
    conn.replace_document(&db, "items", &id, replacement)
        .await
        .unwrap();
    let mut wrong = edited.clone();
    wrong.insert("_id", 12345);
    assert!(
        conn.replace_document(&db, "items", &id, wrong)
            .await
            .is_err()
    );
    assert_eq!(
        conn.delete_documents(&db, "items", &[id.clone(), clone_id])
            .await
            .unwrap(),
        2
    );
    assert!(
        conn.replace_document(&db, "items", &id, doc! {"x": 1})
            .await
            .is_err()
    );
    conn.drop_database(&db).await.unwrap();
}

#[tokio::test]
async fn users_and_roles() {
    let Some((conn, db)) = common::connect("users").await else {
        return;
    };
    conn.create_user(
        &db,
        "reporter",
        "s3cret-pass",
        &[RoleRef {
            role: "read".into(),
            db: db.clone(),
        }],
    )
    .await
    .unwrap();
    let users = conn.users(&db).await.unwrap();
    let reporter = users.iter().find(|u| u.user == "reporter").unwrap();
    assert_eq!(reporter.roles[0].role, "read");
    assert!(!reporter.mechanisms.is_empty());
    let roles = conn.roles(&db, true).await.unwrap();
    assert!(roles.iter().any(|r| r.role == "readWrite" && r.is_builtin));
    conn.drop_user(&db, "reporter").await.unwrap();
    assert!(conn.users(&db).await.unwrap().is_empty());
    conn.drop_database(&db).await.unwrap();
}

#[tokio::test]
async fn connection_errors_are_actionable() {
    use mongo_raabit_core::Connection;
    use mongo_raabit_core::profile::ConnectionProfile;
    use mongo_raabit_core::secrets::MemorySecretStore;
    let (mut profile, _) = ConnectionProfile::from_uri("mongodb://127.0.0.1:1/").unwrap();
    profile.connect_timeout_ms = Some(300);
    let err = Connection::connect(&profile, &MemorySecretStore::default())
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("MongoDB error"), "{err}");
}

#[tokio::test]
async fn builtin_shell_and_mongosh() {
    let Some((conn, db)) = common::connect("shell").await else {
        return;
    };
    let mut current = db.clone();
    let out = conn
        .eval_builtin(
            &mut current,
            "db.items.insertMany([{n: 1}, {n: 2}, {n: 3}])\ndb.items.find({n: {$gte: 2}}, {_id: 0}).sort({n: -1}).limit(1)\ndb.items.countDocuments()",
        )
        .await;
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(
        out[1].result,
        Ok(Bson::Array(vec![Bson::Document(doc! {"n": 3})]))
    );
    assert_eq!(out[2].result, Ok(Bson::Int64(3)));
    let out = conn
        .eval_builtin(
            &mut current,
            "show collections\ndb.items.nope()\ndb.items.drop()",
        )
        .await;
    assert_eq!(out.len(), 2, "evaluation stops at the first error");
    assert!(
        out[1]
            .result
            .as_ref()
            .unwrap_err()
            .contains("install mongosh")
    );
    let out = conn.eval_builtin(&mut current, "use other_db").await;
    assert_eq!(current, "other_db");
    assert!(out[0].result.is_ok());

    if let Some(mongosh) = mongo_raabit_core::mongosh::find_mongosh() {
        let output = conn
            .run_mongosh(
                &mongosh,
                &db,
                "print('hi'); db.items.find({}, {_id: 0}).sort({n: 1}).toArray()",
                std::time::Duration::from_secs(30),
            )
            .await
            .unwrap();
        assert!(output.success, "{output:?}");
        assert!(output.stdout.starts_with("hi"));
        let Some(Bson::Array(items)) = output.value else {
            panic!("{output:?}")
        };
        assert_eq!(items.len(), 3);
        let failed = conn
            .run_mongosh(
                &mongosh,
                &db,
                "this is not js(",
                std::time::Duration::from_secs(30),
            )
            .await
            .unwrap();
        assert!(!failed.success);
        assert!(failed.stderr.contains("SyntaxError") || failed.stdout.contains("SyntaxError"));
    } else {
        eprintln!("mongosh not on PATH; skipping the mongosh half of builtin_shell_and_mongosh");
    }
    conn.drop_database(&db).await.unwrap();
}
