//! Shared fixtures of the documents tests: an app with one collection tab whose page is
//! already loaded, so the views render without a server.
#![allow(dead_code)]

use mongo_raabit::MongoRaabit;
use mongo_raabit::app::{CollectionTab, Load, QueryMode, ResultView, Tab, TabId, TabKind};
use mongo_raabit::core::bson::{Document, doc, oid::ObjectId};
use mongo_raabit::core::connection::CollectionKind;
use mongo_raabit::core::query::Page;
use std::time::Duration;

/// A page's worth of documents with every value kind and nested values.
pub fn documents() -> Vec<Document> {
    (0..50)
        .map(|i| {
            doc! {
                "_id": ObjectId::from_bytes([i as u8; 12]),
                "name": format!("Person {i}"),
                "age": 20 + i,
                "score": i as f64 * 1.5,
                "active": i % 2 == 0,
                "note": mongo_raabit::core::bson::Bson::Null,
                "joined": mongo_raabit::core::bson::DateTime::from_millis(1_700_000_000_000 + i as i64 * 86_400_000),
                "address": { "city": "Oslo", "zip": "0150", "geo": { "lat": 59.9, "lng": 10.7 } },
                "tags": ["a", "b", "c"],
            }
        })
        .collect()
}

/// An app with a collection tab `shop.people` showing `documents()`.
pub fn app_with_documents(view: ResultView) -> (MongoRaabit, TabId) {
    let mut app = MongoRaabit::new(None);
    let mut tab = CollectionTab::new(1, "shop", "people", CollectionKind::Collection);
    tab.view = view;
    // The query bar, not the builder (which another port owns).
    tab.mode = QueryMode::Code;
    tab.count = Load::Ready(240);
    tab.result = Load::Ready(Page {
        documents: documents(),
        page: 0,
        page_size: 50,
        has_more: true,
        elapsed: Duration::from_millis(12),
    });
    app.tabs.push(Tab {
        id: 100,
        kind: TabKind::Collection(Box::new(tab)),
    });
    app.active_tab = app.tabs.len() - 1;
    (app, 100)
}
