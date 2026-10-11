//! The plugin against the fake host of `archipel-plugin-testing`: contract, views and audit.

use archipel_sdk::api::{
    EditorTabRequest, Permission, Plugin, PluginHost, PluginManifest, Value, ViewContext,
    ViewPlacement,
};
use archipel_sdk::kit::Theme;
use archipel_sdk::testing::{DataDir, Fixture, RecordingHost, audit, view_harness};
use mongo_raabit::MongoRaabitPlugin;
use mongo_raabit::plugin::{DATABASES_VIEW, DATABASES_WINDOW, OPEN, SHOW_DATABASES, VIEW};

const ID: &str = "dev.archipel.mongo-raabit";

fn app_view() -> ViewContext {
    ViewContext {
        view: VIEW.into(),
        placement: ViewPlacement::App,
    }
}

#[test]
fn manifest_is_valid_and_declares_its_permissions() {
    let plugin = MongoRaabitPlugin::new();
    let manifest: &PluginManifest = plugin.manifest();
    assert_eq!(manifest.id, ID);
    assert_eq!(manifest.vendor, "Archipel");
    assert_eq!(manifest.api_version, archipel_sdk::API_VERSION);
    for permission in [
        Permission::Network,
        Permission::Process,
        Permission::UserFiles,
        Permission::Clipboard,
    ] {
        assert!(manifest.permissions.contains(&permission), "{permission:?}");
    }
    assert_eq!(manifest.app.as_ref().map(|a| a.view.as_str()), Some(VIEW));
    assert!(plugin.contributions().problems().is_empty());
}

#[test]
fn commands_open_the_tab_and_the_tool_window() {
    let mut plugin = MongoRaabitPlugin::new();
    let contributions = plugin.contributions();
    assert_eq!(contributions.tool_windows[0].id, DATABASES_WINDOW);
    assert_eq!(contributions.tool_windows[0].view, DATABASES_VIEW);
    let mut host = RecordingHost::new(ID);
    plugin.execute(OPEN, Value::Null, &mut host).unwrap();
    plugin
        .execute(SHOW_DATABASES, Value::Null, &mut host)
        .unwrap();
    assert_eq!(host.editor_tabs, [EditorTabRequest::new(VIEW, "MongoDB")]);
    assert_eq!(host.tool_windows, [DATABASES_WINDOW]);
    assert!(
        plugin
            .execute("mongo.nope", Value::Null, &mut host)
            .is_err()
    );
}

#[test]
fn main_screen_passes_the_kit_audit_in_both_themes() {
    for theme in [Theme::dark(), Theme::light()] {
        let fixture = Fixture::new(MongoRaabitPlugin::new(), ID);
        let mut harness = view_harness(theme, fixture, app_view());
        harness.run();
        audit(&mut harness).assert_clean();
    }
}

#[test]
fn the_databases_tool_window_renders() {
    let view = ViewContext {
        view: DATABASES_VIEW.into(),
        placement: ViewPlacement::ToolWindow {
            id: DATABASES_WINDOW.into(),
        },
    };
    let mut harness = view_harness(
        Theme::dark(),
        Fixture::new(MongoRaabitPlugin::new(), ID),
        view,
    );
    harness.run();
    harness.run();
}

#[test]
fn connections_are_stored_in_the_hosts_data_dir() {
    let dir = DataDir::create().unwrap();
    let mut host = RecordingHost::new(ID);
    dir.attach(&mut host);
    assert!(host.data_dir().unwrap().is_dir());
    let mut harness = view_harness(
        Theme::dark(),
        Fixture::with_host(MongoRaabitPlugin::new(), host),
        app_view(),
    );
    harness.run();
}
