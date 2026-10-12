//! Every write path against a live server (`docs/safety.md`): each sample of
//! `write::samples` runs on its own scratch database, and the whole database is compared
//! with the exact state the write must leave; a read-only connection refuses every one
//! and leaves the state as it was; the single-document guards refuse to overwrite a
//! document that changed after it was loaded; outcomes are checked against the preview.
//!
//! `write_contract.rs` fails when a `Write` variant has no arm in
//! `expected_after` below.

mod common;

use futures_util::FutureExt;
use mongo_raabit_core::bson::{Bson, Document, doc};
use mongo_raabit_core::connection::CollectionKind;
use mongo_raabit_core::query::Query;
use mongo_raabit_core::write::{CONFLICT, sample_seed, samples};
use mongo_raabit_core::{Affected, Connection, RaabitError, Write, WriteOutcome};
use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;

/// Run `body` on a scratch database whose name is unique to this test and process; the
/// database and its users are dropped afterwards, also when the test fails.
async fn with_scratch<F, Fut>(test: &str, body: F)
where
    F: FnOnce(Connection, String) -> Fut,
    Fut: Future<Output = ()>,
{
    let Some((conn, _)) = common::connect(test).await else {
        return;
    };
    let db = format!("raabit_wp_{test}_{}", std::process::id());
    let outcome = AssertUnwindSafe(body(conn.clone(), db.clone()))
        .catch_unwind()
        .await;
    drop_scratch(&conn, &db).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn drop_scratch(conn: &Connection, db: &str) {
    let _ = conn
        .execute(Write::RunCommand {
            db: db.to_owned(),
            command: doc! {"dropAllUsersFromDatabase": 1},
        })
        .await;
    let _ = conn
        .execute(Write::DropDatabase { db: db.to_owned() })
        .await;
}

/// `orders` with `sample_seed()`, its index `n_1`, and the user `reporter`.
async fn seed(conn: &Connection, db: &str) {
    drop_scratch(conn, db).await;
    let run = |write: Write| async move { conn.execute(write).await.unwrap() };
    run(Write::InsertDocuments {
        db: db.into(),
        collection: "orders".into(),
        documents: sample_seed(),
    })
    .await;
    run(Write::CreateIndex {
        db: db.into(),
        collection: "orders".into(),
        index: mongo_raabit_core::connection::NewIndex {
            keys: doc! {"n": 1},
            ..Default::default()
        },
    })
    .await;
    run(Write::CreateUser {
        db: db.into(),
        user: "reporter".into(),
        password: "pw".into(),
        roles: Vec::new(),
    })
    .await;
}

/// The indexes of a collection: name → (keys, sparse, hidden, unique).
type Indexes = BTreeMap<String, (Document, bool, bool, bool)>;

/// Everything a write can change in a database, read back from the server.
#[derive(Debug, Clone, PartialEq)]
struct State {
    exists: bool,
    /// Documents of every collection, sorted by `_id`.
    documents: BTreeMap<String, Vec<Document>>,
    /// `create` options of every collection.
    options: BTreeMap<String, Document>,
    /// Views: source and pipeline.
    views: BTreeMap<String, (String, Vec<Document>)>,
    /// Indexes of every collection: name → (keys, sparse, hidden, unique).
    indexes: BTreeMap<String, Indexes>,
    /// Users: name → role names.
    users: BTreeMap<String, Vec<String>>,
}

async fn state(conn: &Connection, db: &str) -> State {
    let exists = conn.databases().await.unwrap().iter().any(|d| d.name == db);
    let mut state = State {
        exists,
        documents: BTreeMap::new(),
        options: BTreeMap::new(),
        views: BTreeMap::new(),
        indexes: BTreeMap::new(),
        users: BTreeMap::new(),
    };
    for info in conn.collections(db).await.unwrap() {
        match info.kind {
            CollectionKind::View => {
                state.views.insert(
                    info.name.clone(),
                    (info.view_on.clone().unwrap_or_default(), info.pipeline),
                );
            }
            CollectionKind::Collection => {
                let mut documents = conn
                    .find_page(db, &info.name, &Query::default(), 0, 1000)
                    .await
                    .unwrap()
                    .documents;
                documents.sort_by_key(|d| format!("{:?}", d.get("_id")));
                state.documents.insert(info.name.clone(), documents);
                let mut options = info.options.clone();
                options.remove("uuid");
                state.options.insert(info.name.clone(), options);
                let indexes = conn
                    .indexes(db, &info.name)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|i| (i.name, (i.keys, i.sparse, i.hidden, i.unique)))
                    .collect();
                state.indexes.insert(info.name.clone(), indexes);
            }
            _ => {}
        }
    }
    for user in conn.users(db).await.unwrap() {
        state
            .users
            .insert(user.user, user.roles.into_iter().map(|r| r.role).collect());
    }
    state
}

fn orders(state: &mut State) -> &mut Vec<Document> {
    state.documents.get_mut("orders").expect("orders exists")
}

fn by_id(state: &mut State, id: i32) -> &mut Document {
    orders(state)
        .iter_mut()
        .find(|d| d.get("_id") == Some(&Bson::Int32(id)))
        .expect("document exists")
}

fn sort(documents: &mut [Document]) {
    documents.sort_by_key(|d| format!("{:?}", d.get("_id")));
}

/// The exact state `write` (a sample) leaves, from the seeded state `before` and what
/// `execute` reported. Every variant has an arm.
fn expected_after(write: &Write, before: &State, outcome: &WriteOutcome) -> State {
    let mut after = before.clone();
    let seed = sample_seed();
    match write {
        Write::InsertDocuments { documents, .. } => {
            assert_eq!(
                outcome,
                &WriteOutcome::Inserted(vec![Bson::Int32(10), Bson::Int32(11)])
            );
            orders(&mut after).extend(documents.iter().cloned());
            sort(orders(&mut after));
        }
        Write::CloneDocument { .. } => {
            let WriteOutcome::Inserted(ids) = outcome else {
                panic!("a clone reports its new _id: {outcome:?}");
            };
            assert!(matches!(ids.as_slice(), [Bson::ObjectId(_)]), "{ids:?}");
            let mut copy = doc! {"_id": ids[0].clone()};
            copy.extend(
                seed[0]
                    .iter()
                    .filter(|(k, _)| *k != "_id")
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
            orders(&mut after).push(copy);
            sort(orders(&mut after));
        }
        Write::ReplaceDocument { replacement, .. } => {
            assert_eq!(
                outcome,
                &WriteOutcome::Updated {
                    matched: 1,
                    modified: 1
                }
            );
            *by_id(&mut after, 1) = replacement.clone();
        }
        Write::SetField { .. } => {
            assert_eq!(
                outcome,
                &WriteOutcome::Updated {
                    matched: 1,
                    modified: 1
                }
            );
            by_id(&mut after, 2).insert("address", doc! {"city": "Bergen"});
        }
        Write::UpdateOne { .. } => {
            assert_eq!(
                outcome,
                &WriteOutcome::Updated {
                    matched: 1,
                    modified: 1
                }
            );
            by_id(&mut after, 3).insert("n", 13);
        }
        Write::UpdateMany { .. } => {
            assert_eq!(
                outcome,
                &WriteOutcome::Updated {
                    matched: 2,
                    modified: 2
                }
            );
            by_id(&mut after, 1).insert("flag", true);
            by_id(&mut after, 2).insert("flag", true);
        }
        Write::ReplaceOne { replacement, .. } => {
            assert_eq!(
                outcome,
                &WriteOutcome::Updated {
                    matched: 1,
                    modified: 1
                }
            );
            *by_id(&mut after, 2) = replacement.clone();
        }
        Write::DeleteDocuments { .. } => {
            assert_eq!(outcome, &WriteOutcome::Deleted(2));
            orders(&mut after).retain(|d| d.get("_id") == Some(&Bson::Int32(3)));
        }
        Write::DeleteOne { .. } => {
            assert_eq!(outcome, &WriteOutcome::Deleted(1));
            orders(&mut after).retain(|d| d.get("_id") != Some(&Bson::Int32(3)));
        }
        Write::DeleteMany { .. } => {
            assert_eq!(outcome, &WriteOutcome::Deleted(2));
            orders(&mut after).retain(|d| d.get_str("status") != Ok("open"));
        }
        Write::CreateDatabase { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after.documents.insert("fresh".into(), Vec::new());
            after.options.insert("fresh".into(), Document::new());
            after.indexes.insert(
                "fresh".into(),
                BTreeMap::from([("_id_".into(), (doc! {"_id": 1}, false, false, false))]),
            );
        }
        Write::DropDatabase { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after = State {
                exists: false,
                documents: BTreeMap::new(),
                options: BTreeMap::new(),
                views: BTreeMap::new(),
                indexes: BTreeMap::new(),
                // Users are not part of a database's data: dropping it keeps them.
                users: before.users.clone(),
            };
        }
        Write::CreateCollection { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after.documents.insert("logs".into(), Vec::new());
            after
                .options
                .insert("logs".into(), doc! {"capped": true, "size": 4096});
            after.indexes.insert(
                "logs".into(),
                BTreeMap::from([("_id_".into(), (doc! {"_id": 1}, false, false, false))]),
            );
        }
        Write::CreateView { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after.views.insert(
                "open_orders".into(),
                ("orders".into(), vec![doc! {"$match": {"status": "open"}}]),
            );
        }
        Write::DropCollection { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after.documents.remove("orders");
            after.options.remove("orders");
            after.indexes.remove("orders");
            // `orders` was its only collection: the database is gone with it.
            after.exists = false;
        }
        Write::RenameCollection { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            let moved = after.documents.remove("orders").unwrap();
            after.documents.insert("archive".into(), moved);
            let options = after.options.remove("orders").unwrap();
            after.options.insert("archive".into(), options);
            let indexes = after.indexes.remove("orders").unwrap();
            after.indexes.insert("archive".into(), indexes);
        }
        Write::CreateIndex { .. } => {
            assert_eq!(outcome, &WriteOutcome::IndexCreated("by_status".into()));
            after.indexes.get_mut("orders").unwrap().insert(
                "by_status".into(),
                (doc! {"status": 1, "n": -1}, true, false, false),
            );
        }
        Write::DropIndex { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after.indexes.get_mut("orders").unwrap().remove("n_1");
        }
        Write::SetIndexHidden { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after
                .indexes
                .get_mut("orders")
                .unwrap()
                .get_mut("n_1")
                .unwrap()
                .2 = true;
        }
        Write::CreateUser { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after.users.insert("auditor".into(), vec!["read".into()]);
        }
        Write::DropUser { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
            after.users.remove("reporter");
        }
        Write::KillOp { .. } => {
            assert_eq!(outcome, &WriteOutcome::Done);
        }
        Write::Import { .. } => {
            let WriteOutcome::Imported(report) = outcome else {
                panic!("an import reports: {outcome:?}");
            };
            assert_eq!((report.replaced + report.upserted, report.failed), (2, 0));
            *by_id(&mut after, 3) = doc! {"_id": 3, "status": "imported"};
            orders(&mut after).push(doc! {"_id": 20});
            sort(orders(&mut after));
        }
        Write::Aggregate { .. } => {
            assert!(
                matches!(outcome, WriteOutcome::Aggregated { documents, .. } if documents.is_empty())
            );
            let copied: Vec<Document> = seed[..2].to_vec();
            after.documents.insert("open_copy".into(), copied);
            after.options.insert("open_copy".into(), Document::new());
            after.indexes.insert(
                "open_copy".into(),
                BTreeMap::from([("_id_".into(), (doc! {"_id": 1}, false, false, false))]),
            );
        }
        Write::RunCommand { .. } => {
            assert!(
                matches!(outcome, WriteOutcome::Command(reply) if reply.get_f64("ok") == Ok(1.0))
            );
            after
                .options
                .insert("orders".into(), doc! {"validator": {"n": {"$gte": 0}}, "validationLevel": "strict", "validationAction": "error"});
        }
    }
    after
}

#[tokio::test]
async fn every_write_leaves_exactly_the_state_it_shows() {
    with_scratch("every", |conn, db| async move {
        for write in samples(&db) {
            seed(&conn, &db).await;
            let before = state(&conn, &db).await;
            let preview = conn.preview(&write).await.unwrap();
            assert_eq!(preview.shell, write.shell(), "the preview shows the write");
            let outcome = conn
                .execute(write.clone())
                .await
                .unwrap_or_else(|e| panic!("{}: {e}", write.kind()));
            let after = state(&conn, &db).await;
            let expected = expected_after(&write, &before, &outcome);
            assert_eq!(
                after,
                expected,
                "{}: the database after the write",
                write.kind()
            );
            assert_eq!(
                write.check_outcome(Some(&preview.affected), &outcome),
                None,
                "{}: nothing changed between preview and run",
                write.kind()
            );
        }
    })
    .await;
}

#[tokio::test]
async fn a_read_only_connection_refuses_every_write() {
    with_scratch("read_only", |conn, db| async move {
        let read_only = common::connect_read_only()
            .await
            .expect("the server answered the first connection");
        for write in samples(&db) {
            seed(&conn, &db).await;
            let before = state(&conn, &db).await;
            let err = read_only.execute(write.clone()).await.unwrap_err();
            assert!(
                matches!(err, RaabitError::ReadOnly(_)),
                "{}: {err}",
                write.kind()
            );
            assert_eq!(
                state(&conn, &db).await,
                before,
                "{}: a refused write changes nothing",
                write.kind()
            );
            // Previewing is allowed and changes nothing either.
            read_only.preview(&write).await.unwrap();
            assert_eq!(state(&conn, &db).await, before, "{}: preview", write.kind());
        }
    })
    .await;
}

/// The inline edit of document 2's `address.city` (loaded as "Oslo").
fn edit_city(db: &str) -> Write {
    samples(db).remove(3)
}

/// The Save of document 1 (loaded as `sample_seed()[0]`).
fn save_first(db: &str) -> Write {
    samples(db).remove(2)
}

async fn document(conn: &Connection, db: &str, id: i32) -> Option<Document> {
    let query = Query {
        filter: doc! {"_id": id},
        ..Query::default()
    };
    conn.find_page(db, "orders", &query, 0, 1)
        .await
        .unwrap()
        .documents
        .pop()
}

async fn out_of_band(conn: &Connection, db: &str, filter: Document, update: Document) {
    conn.execute(Write::UpdateOne {
        db: db.into(),
        collection: "orders".into(),
        filter,
        update,
    })
    .await
    .unwrap();
}

fn assert_conflict(result: mongo_raabit_core::Result<WriteOutcome>) {
    match result {
        Err(RaabitError::Conflict(message)) => assert_eq!(message, CONFLICT),
        other => panic!("expected the conflict error, got {other:?}"),
    }
}

#[tokio::test]
async fn an_edit_does_not_overwrite_a_value_changed_since_it_was_loaded() {
    with_scratch("edit_conflict", |conn, db| async move {
        seed(&conn, &db).await;
        // Someone else changes the city after the page was loaded.
        out_of_band(
            &conn,
            &db,
            doc! {"_id": 2},
            doc! {"$set": {"address.city": "Trondheim"}},
        )
        .await;
        let changed = document(&conn, &db, 2).await.unwrap();
        let edit = edit_city(&db);
        assert_eq!(
            conn.preview(&edit).await.unwrap().affected,
            Affected::Documents(0),
            "the confirmation already shows that nothing matches"
        );
        assert_conflict(conn.execute(edit.clone()).await);
        assert_eq!(
            document(&conn, &db, 2).await.unwrap(),
            changed,
            "not overwritten"
        );

        // A changed type of the same value is a change too.
        seed(&conn, &db).await;
        out_of_band(
            &conn,
            &db,
            doc! {"_id": 2},
            doc! {"$set": {"address.city": ["Oslo"]}},
        )
        .await;
        assert_conflict(conn.execute(edit.clone()).await);

        // A deleted document is not recreated.
        seed(&conn, &db).await;
        conn.execute(Write::DeleteOne {
            db: db.clone(),
            collection: "orders".into(),
            filter: doc! {"_id": 2},
        })
        .await
        .unwrap();
        assert_conflict(conn.execute(edit).await);
        assert_eq!(document(&conn, &db, 2).await, None);

        // A field that was missing must still be missing.
        seed(&conn, &db).await;
        let add = Write::SetField {
            db: db.clone(),
            collection: "orders".into(),
            id: Bson::Int32(3),
            path: "note".into(),
            value: Some(Bson::String("mine".into())),
            original: None,
        };
        out_of_band(
            &conn,
            &db,
            doc! {"_id": 3},
            doc! {"$set": {"note": "theirs"}},
        )
        .await;
        assert_conflict(conn.execute(add.clone()).await);
        assert_eq!(
            document(&conn, &db, 3).await.unwrap().get_str("note"),
            Ok("theirs")
        );
        out_of_band(&conn, &db, doc! {"_id": 3}, doc! {"$unset": {"note": ""}}).await;
        conn.execute(add).await.unwrap();
        assert_eq!(
            document(&conn, &db, 3).await.unwrap().get_str("note"),
            Ok("mine")
        );

        // A null that became missing is a change ({ f: null } would match both).
        seed(&conn, &db).await;
        out_of_band(
            &conn,
            &db,
            doc! {"_id": 3},
            doc! {"$set": {"note": Bson::Null}},
        )
        .await;
        let clear = Write::SetField {
            db: db.clone(),
            collection: "orders".into(),
            id: Bson::Int32(3),
            path: "note".into(),
            value: Some(Bson::String("set".into())),
            original: Some(Bson::Null),
        };
        out_of_band(&conn, &db, doc! {"_id": 3}, doc! {"$unset": {"note": ""}}).await;
        assert_conflict(conn.execute(clear).await);
        assert!(!document(&conn, &db, 3).await.unwrap().contains_key("note"));
    })
    .await;
}

#[tokio::test]
async fn a_save_does_not_overwrite_a_document_changed_since_it_was_loaded() {
    with_scratch("save_conflict", |conn, db| async move {
        // A field someone else added would be lost by the replacement.
        seed(&conn, &db).await;
        out_of_band(&conn, &db, doc! {"_id": 1}, doc! {"$set": {"added": true}}).await;
        let changed = document(&conn, &db, 1).await.unwrap();
        assert_eq!(
            conn.preview(&save_first(&db)).await.unwrap().affected,
            Affected::Documents(0)
        );
        assert_conflict(conn.execute(save_first(&db)).await);
        assert_eq!(document(&conn, &db, 1).await.unwrap(), changed);

        // A nested change, and a removed field.
        for update in [
            doc! {"$set": {"tags.1": "c"}},
            doc! {"$unset": {"n": ""}},
            doc! {"$set": {"n": 1.0}},
        ] {
            seed(&conn, &db).await;
            out_of_band(&conn, &db, doc! {"_id": 1}, update.clone()).await;
            let changed = document(&conn, &db, 1).await.unwrap();
            assert_conflict(conn.execute(save_first(&db)).await);
            assert_eq!(document(&conn, &db, 1).await.unwrap(), changed, "{update}");
        }

        // Deleted: not recreated.
        seed(&conn, &db).await;
        conn.execute(Write::DeleteDocuments {
            db: db.clone(),
            collection: "orders".into(),
            ids: vec![Bson::Int32(1)],
        })
        .await
        .unwrap();
        assert_conflict(conn.execute(save_first(&db)).await);
        assert_eq!(document(&conn, &db, 1).await, None);

        // Unchanged: saved.
        seed(&conn, &db).await;
        conn.execute(save_first(&db)).await.unwrap();
        assert_eq!(
            document(&conn, &db, 1).await.unwrap(),
            doc! {"_id": 1, "status": "done", "n": 1}
        );
    })
    .await;
}

#[tokio::test]
async fn outcomes_that_differ_from_the_preview_are_reported() {
    with_scratch("outcome", |conn, db| async move {
        seed(&conn, &db).await;
        let update = samples(&db).remove(5);
        let preview = conn.preview(&update).await.unwrap();
        assert_eq!(preview.affected, Affected::Documents(2));
        // Another open order arrives between the confirmation and the run.
        conn.execute(Write::InsertDocuments {
            db: db.clone(),
            collection: "orders".into(),
            documents: vec![doc! {"_id": 4, "status": "open"}],
        })
        .await
        .unwrap();
        let outcome = conn.execute(update.clone()).await.unwrap();
        assert_eq!(
            update.check_outcome(Some(&preview.affected), &outcome),
            Some(
                "Expected 2 documents, 3 were matched — the data changed between preview and run"
                    .into()
            )
        );

        // Fewer deleted than selected.
        seed(&conn, &db).await;
        let delete = samples(&db).remove(7);
        conn.execute(Write::DeleteOne {
            db: db.clone(),
            collection: "orders".into(),
            filter: doc! {"_id": 1},
        })
        .await
        .unwrap();
        let outcome = conn.execute(delete.clone()).await.unwrap();
        assert_eq!(outcome, WriteOutcome::Deleted(1));
        assert_eq!(
            delete.check_outcome(None, &outcome),
            Some("Deleted 1 of 2 selected documents: 1 no longer existed".into())
        );
    })
    .await;
}

/// The server's answer to a read that ran past `maxTimeMS` (JavaScript in a filter is
/// interrupted instead).
fn stopped_by_time_limit(err: &str) -> bool {
    err.contains("MaxTimeMSExpired") || err.contains("time limit") || err.contains("(Interrupted)")
}

#[tokio::test]
async fn reads_stop_at_the_query_timeout() {
    with_scratch("timeout", |conn, db| async move {
        seed(&conn, &db).await;
        conn.set_query_timeout(std::time::Duration::from_millis(1));
        assert_eq!(conn.query_timeout(), std::time::Duration::from_millis(1));
        // A query that sleeps on the server is stopped by maxTimeMS, not left running.
        let slow = Query {
            filter: doc! {"$expr": {"$function": {
                "body": "function() { sleep(200); return true; }",
                "args": [],
                "lang": "js",
            }}},
            ..Query::default()
        };
        let err = conn
            .find_page(&db, "orders", &slow, 0, 10)
            .await
            .unwrap_err()
            .to_string();
        assert!(stopped_by_time_limit(&err), "{err}");
        let err = conn
            .count(&db, "orders", &slow)
            .await
            .unwrap_err()
            .to_string();
        assert!(stopped_by_time_limit(&err), "{err}");
        // With the default limit the same query runs to its end: the time limit stopped it.
        conn.set_query_timeout(std::time::Duration::from_secs(60));
        assert_eq!(
            conn.find_page(&db, "orders", &slow, 0, 10)
                .await
                .unwrap()
                .documents
                .len(),
            3
        );
        assert_eq!(
            conn.find_page(&db, "orders", &Query::default(), 0, 10)
                .await
                .unwrap()
                .documents
                .len(),
            3
        );
    })
    .await;
}
