//! Write confirmation and read-only connections in the views: every write goes through
//! `MongoRaabit::request_write`, which asks first (the query and what it affects) when
//! "Confirm changes" is on, runs at once when it is off, and refuses on a read-only
//! connection. These run without a server; `app_flow.rs` covers the real writes.

mod common;

use archipel_sdk::kit::egui::{Key, vec2};
use archipel_sdk::kit::testing::NodeExt;
use archipel_sdk::kit::{Theme, testing};
use archipel_sdk::testing::{Harness, Queryable, audit};
use common::app_with_documents;
use mongo_raabit::MongoRaabit;
use mongo_raabit::app::{CellEdit, Load, ResultView, Session, SessionState};
use mongo_raabit::core::bson::doc;
use mongo_raabit::core::profile::ConnectionProfile;
use mongo_raabit::core::{Write, settings};
use mongo_raabit::dialogs::{ConfirmWrite, Confirmed, Dialog};
use mongo_raabit::writes::{HostRequest, PendingWrite, WriteOrigin};

fn dialog<'a>(theme: Theme, app: MongoRaabit) -> Harness<'a, MongoRaabit> {
    let mut harness = testing::harness_state_sized(
        theme,
        vec2(1280.0, 800.0),
        app,
        |ui, app: &mut MongoRaabit| mongo_raabit::dialogs::show(app, ui.ctx()),
    );
    harness.run_ok();
    harness
}

/// An inline edit of the first document's `name`, committed.
fn edit_name(app: &mut MongoRaabit, tab: u64) {
    app.collection_tab_mut(tab).unwrap().editing = Some(CellEdit {
        doc_index: 0,
        path: "name".into(),
        text: "Ada".into(),
        original: Some("Person 0".into()),
        focus: false,
    });
    app.commit_cell_edit(tab);
}

fn confirmation(app: &MongoRaabit) -> &ConfirmWrite {
    match &app.dialog {
        Some(Dialog::ConfirmWrite(confirm)) => confirm,
        _ => panic!("no write confirmation is open"),
    }
}

#[test]
fn an_inline_edit_asks_first_with_its_set_query() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    assert!(app.settings.confirm_writes, "on by default");
    edit_name(&mut app, tab);
    let confirm = confirmation(&app);
    assert_eq!(confirm.title, "Update 1 document in shop.people");
    assert_eq!(
        confirm.shell,
        "db.getCollection(\"people\").updateOne({\n  _id: ObjectId(\"000000000000000000000000\")\n}, {\n  $set: {\n    name: \"Ada\"\n  }\n})"
    );
    assert!(!confirm.destructive);
    let Confirmed::Write(pending) = &confirm.action else {
        panic!("a write");
    };
    assert_eq!(pending.origin, WriteOrigin::Cell(tab));
    // Nothing ran: no status, the edit waits for Run.
    assert!(app.status.is_empty(), "{}", app.status);

    let mut harness = dialog(Theme::dark(), app);
    harness.get_by_label("Update 1 document in shop.people");
    harness.get_by_label("Query to run");
    harness.get_by_label("Run");
    harness.get_by_label("Don't ask again");
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(harness.state().dialog.is_none());
    assert!(harness.state().status.is_empty(), "cancel runs nothing");
}

#[test]
fn run_runs_the_write() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    edit_name(&mut app, tab);
    let mut harness = dialog(Theme::dark(), app);
    harness.get_by_label("Run").click();
    harness.run();
    // No server in this test: the write was sent and failed for want of a connection.
    assert!(harness.state().dialog.is_none());
    assert_eq!(harness.state().status, "Update failed: not connected");
}

#[test]
fn escape_cancels_and_enter_runs() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    edit_name(&mut app, tab);
    let mut harness = dialog(Theme::light(), app);
    harness.key_press(Key::Escape);
    harness.run();
    assert!(harness.state().dialog.is_none());
    assert!(harness.state().status.is_empty());

    edit_name(harness.state_mut(), tab);
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    assert!(harness.state().dialog.is_none());
    assert_eq!(harness.state().status, "Update failed: not connected");
}

#[test]
fn dont_ask_again_turns_the_confirmation_off() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    edit_name(&mut app, tab);
    let mut harness = dialog(Theme::dark(), app);
    harness.get_by_label("Don't ask again").click();
    harness.run();
    harness.get_by_label("Run").click();
    harness.run();
    let app = harness.state_mut();
    assert!(!app.settings.confirm_writes);
    assert_eq!(
        app.host_requests,
        [HostRequest::SetSetting(
            settings::CONFIRM_WRITES,
            serde_json::Value::Bool(false)
        )]
    );
    // The next write runs without asking.
    app.status.clear();
    edit_name(app, tab);
    assert!(app.dialog.is_none());
    assert_eq!(app.status, "Update failed: not connected");
}

#[test]
fn without_confirmation_writes_run_directly() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    app.settings.confirm_writes = false;
    edit_name(&mut app, tab);
    assert!(app.dialog.is_none());
    assert_eq!(app.status, "Update failed: not connected");
    // Deletes still ask, in the plain delete dialog.
    app.collection_tab_mut(tab).unwrap().selected = Some(0);
    app.request_delete(tab);
    assert!(matches!(app.dialog, Some(Dialog::Delete(_))));
}

#[test]
fn a_delete_is_confirmed_as_destructive() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    app.collection_tab_mut(tab).unwrap().selected = Some(1);
    app.request_delete(tab);
    let confirm = confirmation(&app);
    assert!(confirm.destructive);
    assert_eq!(confirm.title, "Delete 1 document from shop.people");
    assert_eq!(
        confirm.shell,
        "db.getCollection(\"people\").deleteOne({ _id: ObjectId(\"010101010101010101010101\") })"
    );
}

#[test]
fn cancelling_a_save_returns_to_the_editor() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    app.collection_tab_mut(tab).unwrap().selected = Some(0);
    app.edit_selected(tab);
    let mut harness = dialog(Theme::dark(), app);
    harness.get_by_label("Save").click();
    harness.run();
    let confirm = confirmation(harness.state());
    assert!(confirm.destructive, "a replace overwrites the document");
    assert!(
        confirm
            .shell
            .starts_with("db.getCollection(\"people\").replaceOne({\n  _id: ObjectId("),
        "{}",
        confirm.shell
    );
    harness.get_by_label("Cancel").click();
    harness.run();
    let Some(Dialog::Document(editor)) = &harness.state().dialog else {
        panic!("the editor is back");
    };
    assert!(!editor.saving);
}

#[test]
fn the_confirmation_passes_the_audit() {
    for theme in [Theme::dark(), Theme::light()] {
        for destructive in [false, true] {
            let (mut app, tab) = app_with_documents(ResultView::Table);
            if destructive {
                app.collection_tab_mut(tab).unwrap().selected = Some(0);
                app.request_delete(tab);
            } else {
                edit_name(&mut app, tab);
            }
            if let Some(Dialog::ConfirmWrite(confirm)) = &mut app.dialog {
                confirm.summary = Load::Ready("1 document will be modified".into());
            }
            let mut harness = dialog(theme.clone(), app);
            harness.get_by_label("Run");
            audit(&mut harness).assert_clean();
        }
    }
}

#[test]
fn a_shell_script_that_writes_is_confirmed() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    app.init_shell(tab);
    assert!(app.confirm_script(tab, "shop", "db.people.deleteMany({})"));
    let confirm = confirmation(&app);
    assert!(confirm.destructive);
    assert_eq!(confirm.shell, "db.people.deleteMany({})");
    assert_eq!(
        confirm.summary,
        Load::Ready("The script calls deleteMany: it can modify data".into())
    );
    app.dialog = None;
    assert!(!app.confirm_script(tab, "shop", "db.people.find({})"));
    app.settings.confirm_writes = false;
    assert!(!app.confirm_script(tab, "shop", "db.people.deleteMany({})"));
}

/// The documents tab on a read-only session (still connecting, so no server is needed).
fn read_only_app() -> (MongoRaabit, u64) {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    let (mut profile, _) = ConnectionProfile::from_uri("mongodb://localhost:27017").unwrap();
    profile.read_only = true;
    app.sessions.push(Session {
        id: 1,
        profile,
        state: SessionState::Connecting,
        databases: Load::Idle,
    });
    (app, tab)
}

#[test]
fn a_read_only_connection_disables_write_actions() {
    let (mut app, tab) = read_only_app();
    app.collection_tab_mut(tab).unwrap().selected = Some(0);
    assert!(app.session_read_only(1));
    let mut harness = testing::harness_state_sized(
        Theme::dark(),
        vec2(1280.0, 800.0),
        app,
        |ui, app: &mut MongoRaabit| app.ui(ui),
    );
    harness.run_ok();
    for label in [
        "Add document",
        "Edit document (double-click a row)",
        "Clone document",
        "Delete document",
    ] {
        let label = format!("{label} (read-only connection)");
        assert!(harness.get_by_label(&label).is_disabled(), "{label}");
    }
    // Shown, not hidden; reading stays possible.
    assert!(!harness.get_by_label("Copy document").is_disabled());
    harness.get_by_label("Read-only");
    audit(&mut harness).assert_clean();

    // The guard behind the buttons: a write asked for anyway runs nothing.
    let app = harness.state_mut();
    app.request_write(PendingWrite {
        session: 1,
        write: Write::DeleteMany {
            db: "shop".into(),
            collection: "people".into(),
            filter: doc! {},
        },
        origin: WriteOrigin::Delete(tab),
    });
    assert!(app.dialog.is_none());
    assert_eq!(
        app.status,
        "Read-only connection: \"Delete documents from shop.people\" was not run"
    );
}

#[test]
fn the_open_read_only_setting_applies_to_new_sessions() {
    let (mut app, _) = app_with_documents(ResultView::Table);
    let (profile, _) = ConnectionProfile::from_uri("mongodb://localhost:27017").unwrap();
    app.sessions.push(Session {
        id: 1,
        profile,
        state: SessionState::Connecting,
        databases: Load::Idle,
    });
    assert!(!app.session_read_only(1));
    app.settings.new_connections_read_only = true;
    assert!(app.session_read_only(1));
}
