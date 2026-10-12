//! The dialogs act through their own buttons (title, body and a fixed button row): what
//! each button does, with the keyboard where the kit gives it one.

mod common;

use archipel_sdk::kit::egui::vec2;
use archipel_sdk::kit::{Theme, testing};
use archipel_sdk::testing::{Harness, Queryable};
use common::app_with_documents;
use mongo_raabit::MongoRaabit;
use mongo_raabit::app::{Load, ResultView};
use mongo_raabit::core::codegen::{CodeRequest, Operation};
use mongo_raabit::core::query::Query;
use mongo_raabit::dialogs::{CodeDialog, Dialog, DocumentEditor, ExplainDialog};

fn dialog<'a>(app: MongoRaabit) -> Harness<'a, MongoRaabit> {
    let mut harness = testing::harness_state_sized(
        Theme::dark(),
        vec2(1280.0, 800.0),
        app,
        |ui, app: &mut MongoRaabit| mongo_raabit::dialogs::show(app, ui.ctx()),
    );
    harness.run_ok();
    harness
}

fn manager<'a>(app: MongoRaabit) -> Harness<'a, MongoRaabit> {
    let mut harness = testing::harness_state_sized(
        Theme::dark(),
        vec2(1280.0, 800.0),
        app,
        |ui, app: &mut MongoRaabit| mongo_raabit::manager::show(app, ui.ctx()),
    );
    harness.run_ok();
    harness
}

#[test]
fn the_document_editor_formats_and_cancels_from_its_buttons() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    let mut editor = DocumentEditor::insert(tab, None);
    editor.text = "{ a:1,   b:[1,2] }".into();
    app.dialog = Some(Dialog::Document(editor));
    let mut harness = dialog(app);
    harness.get_by_label("Format").click();
    harness.run();
    let Some(Dialog::Document(editor)) = &harness.state().dialog else {
        panic!("the dialog stays open after Format");
    };
    assert_eq!(editor.text, "{\n  a: 1,\n  b: [\n    1,\n    2\n  ]\n}");
    harness.get_by_label("Insert");
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(harness.state().dialog.is_none());
}

#[test]
fn saving_an_invalid_document_keeps_the_dialog_and_says_why() {
    let (mut app, tab) = app_with_documents(ResultView::Table);
    let mut editor = DocumentEditor::edit(tab, &mongo_raabit::core::bson::doc! {"_id": 1});
    editor.text = "{ a: , }".into();
    app.dialog = Some(Dialog::Document(editor));
    let mut harness = dialog(app);
    harness.get_by_label("Save").click();
    harness.run();
    let Some(Dialog::Document(editor)) = &harness.state().dialog else {
        panic!("the dialog stays open");
    };
    assert!(editor.error.is_some());
    assert!(!editor.saving);
}

#[test]
fn the_code_dialog_copies_and_closes_from_its_buttons() {
    let (mut app, _) = app_with_documents(ResultView::Table);
    app.dialog = Some(Dialog::Code(CodeDialog {
        title: "Query code  shop.people".into(),
        request: CodeRequest {
            uri: "mongodb://localhost:27017".into(),
            database: "shop".into(),
            collection: "people".into(),
            operation: Operation::Find(Query::default()),
        },
        shell: "db.people.find({})".into(),
        language: None,
    }));
    let mut harness = dialog(app);
    harness.get_by_label("Copy").click();
    harness.run();
    assert!(harness.state().dialog.is_some(), "Copy keeps the dialog");
    harness.get_by_label("Close").click();
    harness.run();
    assert!(harness.state().dialog.is_none());
}

#[test]
fn the_explain_dialog_closes_from_its_button() {
    let (mut app, _) = app_with_documents(ResultView::Table);
    app.dialog = Some(Dialog::Explain(ExplainDialog {
        title: "Explain  shop.people".into(),
        result: Load::Loading,
        show_raw: false,
    }));
    let mut harness = dialog(app);
    harness.get_by_label("Close").click();
    harness.run();
    assert!(harness.state().dialog.is_none());
}

#[test]
fn the_manager_has_its_actions_in_the_button_row() {
    let mut app = MongoRaabit::new(None);
    app.open_manager();
    let mut harness = manager(app);
    harness.get_by_label("Test Connection");
    harness.get_by_label("Connect");
    harness.get_by_label("Close").click();
    harness.run();
    assert!(harness.state().manager.is_none());
}

#[test]
fn the_import_panel_cancels_from_the_button_row() {
    let mut app = MongoRaabit::new(None);
    app.open_manager();
    let mut harness = manager(app);
    harness
        .get_by_label("Import from connection string")
        .click();
    harness.run();
    // The row now offers Import and Cancel instead of Connect and Close.
    harness.get_by_label("Import");
    assert!(harness.query_by_label("Connect").is_none());
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(
        harness
            .state()
            .manager
            .as_ref()
            .unwrap()
            .uri_import
            .is_none()
    );
    harness.get_by_label("Connect");
}

#[test]
fn the_import_panel_imports_from_the_button_row() {
    let mut app = MongoRaabit::new(None);
    app.open_manager();
    let mut harness = manager(app);
    harness
        .get_by_label("Import from connection string")
        .click();
    harness.run();
    let field = harness.get_by_label("Connection string to import");
    field.focus();
    field.type_text("mongodb://u:pw@db1:27017,db2:27018/?replicaSet=rs0");
    harness.run();
    harness.get_by_label("Import").click();
    harness.run();
    let manager = harness.state().manager.as_ref().unwrap();
    assert!(manager.uri_import.is_none());
    assert_eq!(manager.form.set_name, "rs0");
    assert_eq!(manager.form.password, "pw");
}

#[test]
fn an_import_error_is_shown_and_keeps_the_panel() {
    let mut app = MongoRaabit::new(None);
    app.open_manager();
    let mut harness = manager(app);
    harness
        .get_by_label("Import from connection string")
        .click();
    harness.run();
    let field = harness.get_by_label("Connection string to import");
    field.focus();
    field.type_text("http://nope");
    harness.run();
    harness.get_by_label("Import").click();
    harness.run();
    let manager = harness.state().manager.as_ref().unwrap();
    assert!(manager.uri_import.is_some());
    assert!(manager.error.is_some());
}
