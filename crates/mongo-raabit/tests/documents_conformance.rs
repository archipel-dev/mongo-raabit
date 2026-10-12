//! Kit conformance for every interactive element type of the documents views, configured
//! the way they are used there, plus behaviour tests of the table, the tree and the
//! round trip of an inline edit (against a live server when one is reachable).

mod common;

use archipel_sdk::kit::components::code::JavaScript;
use archipel_sdk::kit::components::{
    Banner, Checkbox, CodeEditor, Dropdown, IconButton, SegmentedControl, Severity,
};
use archipel_sdk::kit::egui::{Key, Modifiers, vec2};
use archipel_sdk::kit::testing::{self, Queryable};
use archipel_sdk::kit::{Theme, Tone};
use common::documents;
use mongo_raabit::MongoRaabit;
use mongo_raabit::app::{CollectionTab, Load, ResultView, SessionState};
use mongo_raabit::core::bson::{Bson, Document, doc};
use mongo_raabit::core::connection::CollectionKind;
use mongo_raabit::core::profile::ConnectionProfile;
use mongo_raabit::icons::Icon;
use mongo_raabit::results::{self, TableEvent, TreeEvent, TreeState};
use std::collections::HashSet;
use std::time::Duration;

#[test]
fn toolbar_icon_buttons() {
    for (icon, label) in [
        (Icon::Plus, "Add document"),
        (Icon::Edit, "Edit document (double-click a row)"),
        (Icon::Copy, "Clone document"),
        (Icon::Json, "Copy document"),
        (Icon::Explain, "Explain query plan"),
        (
            Icon::Code,
            "Query code (JS, Python, Java, C#, Go, Rust, PHP, Ruby)",
        ),
        (Icon::ChevronRight, "Next page"),
        (Icon::ChevronLeft, "Previous page"),
    ] {
        testing::conformance(label, |ui, enabled| {
            ui.add(IconButton::new(icon, label).enabled(enabled))
        })
        .run();
    }
    testing::conformance("Delete document", |ui, enabled| {
        ui.add(
            IconButton::new(Icon::Trash, "Delete document")
                .tone(Tone::Error)
                .enabled(enabled),
        )
    })
    .run();
}

#[test]
fn dropdowns_checkbox_and_breadcrumb() {
    let mut size = "50 / page".to_owned();
    testing::conformance("Page size", |ui, enabled| {
        ui.add(
            Dropdown::new(
                "page-size",
                &mut size,
                ["20 / page".to_owned(), "50 / page".to_owned()],
            )
            .label("Page size")
            .enabled(enabled),
        )
    })
    .run();
    let mut language = "mongo shell".to_owned();
    testing::conformance("Language", |ui, enabled| {
        ui.add(
            Dropdown::new(
                "language",
                &mut language,
                ["mongo shell".to_owned(), "Python".to_owned()],
            )
            .label("Language")
            .enabled(enabled),
        )
    })
    .run();
    let mut raw = false;
    testing::conformance("Show raw explain output", |ui, enabled| {
        ui.add_enabled(enabled, Checkbox::new(&mut raw, "Show raw explain output"))
    })
    .run();
}

#[test]
fn breadcrumb_links_go_back() {
    let mut harness = testing::harness_state(
        Theme::dark(),
        (vec!["address".to_owned(), "geo".to_owned()], None),
        |ui, (drill, clicked)| {
            if let Some(depth) = results::breadcrumb(ui, drill) {
                *clicked = Some(depth);
            }
        },
    );
    harness.get_by_label("address").click();
    harness.run();
    assert_eq!(harness.state().1, Some(1));
    harness.get_by_label("documents").click();
    harness.run();
    assert_eq!(harness.state().1, Some(0));
}

#[test]
fn banners_and_the_document_editor_field() {
    let harness = testing::harness(Theme::dark(), |ui| {
        Banner::new(Severity::Error, "expected a value (line 1, column 6)")
            .dismissible(true)
            .show(ui);
    });
    harness.get_by_label_contains("line 1, column 6");
    let mut text = String::from("{ }");
    testing::conformance("Document", |ui, enabled| {
        ui.add(
            CodeEditor::new(&mut text)
                .label("Document")
                .highlighter(&JavaScript)
                .tab_indents(false)
                .enabled(enabled)
                .height(120.0),
        )
    })
    .no_click()
    .keys(&[])
    .run();
}

#[test]
fn the_view_switcher_changes_the_view() {
    let mut harness = testing::harness_state(Theme::dark(), ResultView::Table, |ui, view| {
        ui.add(SegmentedControl::new(
            view,
            [
                (ResultView::Table, "Table"),
                (ResultView::Tree, "Tree"),
                (ResultView::Json, "JSON"),
            ],
        ));
    });
    harness.get_by_label("Tree").click();
    harness.run();
    assert_eq!(*harness.state(), ResultView::Tree);
    harness.get_by_label("JSON").click();
    harness.run();
    assert_eq!(*harness.state(), ResultView::Json);
}

struct TableHarness {
    tab: CollectionTab,
    docs: Vec<Document>,
    events: Vec<TableEvent>,
}

fn table_harness<'a>() -> testing::Harness<'a, TableHarness> {
    let tab = CollectionTab::new(1, "shop", "people", CollectionKind::Collection);
    let state = TableHarness {
        tab,
        docs: documents(),
        events: Vec::new(),
    };
    let mut harness = testing::harness_state(Theme::dark(), state, |ui, s: &mut TableHarness| {
        if let Some(event) = results::table(ui, &mut s.tab, &s.docs, 1, true) {
            if let TableEvent::Select(i) = &event {
                s.tab.selected = Some(*i);
            }
            s.events.push(event);
        }
    });
    harness.set_size(vec2(1280.0, 600.0));
    harness.run();
    harness
}

#[test]
fn a_table_cell_edit_reports_the_set_update() {
    let mut harness = table_harness();
    harness
        .get_by_role_and_label(testing::Role::Table, "Documents")
        .focus();
    harness.key_press(Key::ArrowDown);
    harness.run();
    assert!(matches!(
        harness.state().events.last(),
        Some(TableEvent::Select(0))
    ));
    harness.key_press(Key::F2);
    harness.run();
    let field = harness.get_by_label("Edit name");
    field.focus();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Edit name").type_text("Grace");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    let Some(TableEvent::Edit(edit)) = harness.state().events.last() else {
        panic!("no edit reported");
    };
    assert_eq!(edit.path, "name");
    assert_eq!(edit.text, "Grace");
    assert_eq!(edit.doc_index, 0);
    assert_eq!(edit.original, Some(Bson::String("Person 0".into())));
}

#[test]
fn escape_cancels_a_table_cell_edit() {
    let mut harness = table_harness();
    harness
        .get_by_role_and_label(testing::Role::Table, "Documents")
        .focus();
    harness.key_press(Key::ArrowDown);
    harness.run();
    harness.key_press(Key::F2);
    harness.run();
    harness.get_by_label("Edit name").type_text("x");
    harness.key_press(Key::Escape);
    harness.run();
    assert!(
        !harness
            .state()
            .events
            .iter()
            .any(|e| matches!(e, TableEvent::Edit(_)))
    );
    assert!(harness.query_by_label("Edit name").is_none());
}

#[test]
fn a_header_click_sorts_by_that_field() {
    let mut harness = table_harness();
    harness.get_by_label("age").click();
    harness.run();
    let Some(TableEvent::Sort { field, descending }) = harness.state().events.last() else {
        panic!("no sort reported");
    };
    assert_eq!((field.as_str(), *descending), ("age", false));
    assert_eq!(
        mongo_raabit_core::documents::sort_text("age", false),
        "{ age: 1 }"
    );
    assert_eq!(
        mongo_raabit_core::documents::sort_text("a.b", true),
        "{ \"a.b\": -1 }"
    );
}

struct TreeHarness {
    open: HashSet<String>,
    selected: Option<usize>,
    docs: Vec<Document>,
    events: Vec<TreeEvent>,
}

fn tree_harness<'a>() -> testing::Harness<'a, TreeHarness> {
    let state = TreeHarness {
        open: HashSet::new(),
        selected: None,
        docs: documents(),
        events: Vec::new(),
    };
    let mut harness = testing::harness_state(Theme::dark(), state, |ui, s: &mut TreeHarness| {
        let event = results::tree_view(
            ui,
            TreeState {
                id: archipel_sdk::kit::egui::Id::new("tree"),
                open: &s.open,
                selected: s.selected,
            },
            &s.docs,
            1,
            true,
        );
        match event {
            Some(TreeEvent::Toggle(key)) => {
                if !s.open.remove(&key) {
                    s.open.insert(key);
                }
            }
            Some(TreeEvent::Select(i)) => s.selected = Some(i),
            Some(other) => s.events.push(other),
            None => {}
        }
    });
    harness.set_size(vec2(1280.0, 600.0));
    harness.run();
    harness
}

#[test]
fn tree_arrows_expand_and_collapse() {
    let mut harness = tree_harness();
    harness.get_by_label("Documents tree").focus();
    harness.key_press(Key::ArrowDown);
    harness.run();
    harness.key_press(Key::ArrowRight);
    harness.run();
    assert!(harness.state().open.contains("0"), "→ expands the document");
    harness.key_press(Key::ArrowLeft);
    harness.run();
    assert!(!harness.state().open.contains("0"), "← collapses it");
}

#[test]
fn a_tree_leaf_edit_reports_the_set_update() {
    let mut harness = tree_harness();
    harness.get_by_label("Documents tree").focus();
    harness.key_press(Key::ArrowDown);
    harness.run();
    harness.key_press(Key::ArrowRight);
    harness.run();
    // (0) id → _id → name
    harness.key_press(Key::ArrowDown);
    harness.key_press(Key::ArrowDown);
    harness.run();
    harness.key_press(Key::F2);
    harness.run();
    harness.get_by_label("Edit Value").focus();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Edit Value").type_text("Grace");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    let Some(TreeEvent::Edit(edit)) = harness.state().events.last() else {
        panic!("no edit reported");
    };
    assert_eq!((edit.path.as_str(), edit.text.as_str()), ("name", "Grace"));
    assert_eq!(edit.doc_index, 0);
}

fn uri() -> String {
    std::env::var("MONGO_RAABIT_TEST_URI").unwrap_or_else(|_| "mongodb://127.0.0.1:27017".into())
}

/// Connect, seed `db.people` and open it in the given view; `None` without a server.
fn live(
    db: &str,
    view: ResultView,
) -> Option<(
    testing::Harness<'static, MongoRaabit>,
    mongo_raabit::core::Connection,
    tokio::runtime::Runtime,
)> {
    let (mut profile, _) = ConnectionProfile::from_uri(&uri()).unwrap();
    profile.connect_timeout_ms = Some(1500);
    let mut app = MongoRaabit::new(None);
    // These flows exercise the writes; the confirmation has its own tests (confirm.rs).
    app.settings.confirm_writes = false;
    let session = app.connect(profile);
    app.wait_idle(Duration::from_secs(10));
    if let SessionState::Failed(err) = &app.session(session).unwrap().state {
        eprintln!(
            "skipping live edit test: no MongoDB server at {} ({err})",
            uri()
        );
        return None;
    }
    let conn = app.session(session).unwrap().connection()?.clone();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let _ = conn
            .execute(mongo_raabit::core::Write::DropDatabase { db: db.to_owned() })
            .await;
        conn.execute(mongo_raabit::core::Write::InsertDocuments {
            db: db.to_owned(),
            collection: "people".to_owned(),
            documents: vec![
                doc! {"_id": 1, "name": "Ada", "age": 36, "address": {"city": "London"}},
                doc! {"_id": 2, "name": "Linus", "age": 54},
            ],
        })
        .await
        .unwrap();
    });
    app.open_collection(session, db, "people", CollectionKind::Collection);
    app.wait_idle(Duration::from_secs(10));
    let tab_id = app.active_tab_id().unwrap();
    let tab = app.collection_tab_mut(tab_id).unwrap();
    tab.view = view;
    tab.mode = mongo_raabit::app::QueryMode::Code;
    assert!(matches!(tab.result, Load::Ready(_)));
    let mut harness =
        testing::harness_state(Theme::dark(), app, |ui, app: &mut MongoRaabit| app.ui(ui));
    harness.set_size(vec2(1280.0, 800.0));
    harness.run();
    Some((harness, conn, rt))
}

fn find(
    rt: &tokio::runtime::Runtime,
    conn: &mongo_raabit::core::Connection,
    db: &str,
    id: i32,
) -> Document {
    rt.block_on(async {
        let query = mongo_raabit::core::query::Query {
            filter: doc! {"_id": id},
            ..Default::default()
        };
        conn.find_page(db, "people", &query, 0, 1)
            .await
            .unwrap()
            .documents
            .remove(0)
    })
}

fn selected_document(harness: &mut testing::Harness<'_, MongoRaabit>) -> Option<usize> {
    let app = harness.state_mut();
    let tab = app.active_tab_id()?;
    app.collection_tab_mut(tab)?.selected
}

fn settle(harness: &mut testing::Harness<'_, MongoRaabit>) {
    harness.state_mut().wait_idle(Duration::from_secs(10));
    harness.run();
}

#[test]
fn editing_a_table_cell_updates_the_server() {
    let db = "raabit_test_doc_table_edit";
    let Some((mut harness, conn, rt)) = live(db, ResultView::Table) else {
        return;
    };
    harness
        .get_by_role_and_label(testing::Role::Table, "Documents")
        .focus();
    harness.key_press(Key::ArrowDown);
    harness.run();
    // The first editable column of the row is `name`.
    harness.key_press(Key::F2);
    harness.run();
    harness.get_by_label("Edit name").focus();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Edit name").type_text("Ada Lovelace");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    settle(&mut harness);
    assert_eq!(find(&rt, &conn, db, 1).get_str("name"), Ok("Ada Lovelace"));
    // The view shows the stored value again and keeps the keyboard focus: ↓ moves on.
    harness.get_by_label_contains("Ada Lovelace");
    harness.key_press(Key::ArrowDown);
    harness.run();
    assert_eq!(selected_document(&mut harness), Some(1));
    rt.block_on(async {
        conn.execute(mongo_raabit::core::Write::DropDatabase { db: db.to_owned() })
            .await
            .unwrap()
    });
}

#[test]
fn editing_a_tree_value_updates_the_server() {
    let db = "raabit_test_doc_tree_edit";
    let Some((mut harness, conn, rt)) = live(db, ResultView::Tree) else {
        return;
    };
    harness.get_by_label("Documents tree").focus();
    harness.key_press(Key::ArrowDown);
    harness.run();
    harness.key_press(Key::ArrowRight);
    harness.run();
    // (1) 1 → _id → name → age
    for _ in 0..3 {
        harness.key_press(Key::ArrowDown);
    }
    harness.run();
    harness.key_press(Key::F2);
    harness.run();
    harness.get_by_label("Edit Value").focus();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Edit Value").type_text("37");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    settle(&mut harness);
    let stored = find(&rt, &conn, db, 1);
    assert_eq!(stored.get("age"), Some(&Bson::Int32(37)), "{stored:?}");
    rt.block_on(async {
        conn.execute(mongo_raabit::core::Write::DropDatabase { db: db.to_owned() })
            .await
            .unwrap()
    });
}
