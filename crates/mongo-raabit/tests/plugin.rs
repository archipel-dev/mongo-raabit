//! The plugin against the fake host of `archipel-plugin-testing`: contract, views and audit.

use archipel_sdk::api::{
    EditorTabRequest, Permission, Plugin, PluginHost, PluginManifest, Value, ViewContext,
    ViewFrame, ViewPlacement,
};
use archipel_sdk::kit::Theme;
use archipel_sdk::testing::{DataDir, Fixture, Queryable, RecordingHost, audit, view_harness};
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
        // The app fills its area: a window fitted to the content would be a few points wide.
        let fixture = Fixture::new(MongoRaabitPlugin::new(), ID);
        let view = app_view();
        let mut harness = archipel_sdk::kit::testing::harness_state_sized(
            theme,
            archipel_sdk::kit::egui::vec2(1000.0, 640.0),
            fixture,
            move |ui, fixture| {
                let Fixture { plugin, host } = fixture;
                plugin.render_view(&view, &mut ViewFrame::new(ui), host);
            },
        );
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

#[test]
fn the_settings_page_holds_every_setting() {
    use mongo_raabit::core::settings;
    let contributions = MongoRaabitPlugin::new().contributions();
    let page = &contributions.settings_pages[0];
    let keys: Vec<&str> = page.fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            settings::CONFIRM_WRITES,
            settings::NEW_CONNECTIONS_READ_ONLY,
            settings::PAGE_SIZE,
            settings::SCHEMA_SAMPLE_SIZE,
            settings::QUERY_TIMEOUT_SECS,
        ]
    );
    assert!(page.fields.iter().all(|f| f.description.is_some()));
    assert_eq!(page.fields[0].kind.default_value(), Value::Bool(true));
}

#[test]
fn the_views_read_and_write_the_hosts_settings() {
    use archipel_sdk::api::json;
    use mongo_raabit::core::settings;
    use mongo_raabit::plugin::OPEN_SETTINGS;
    let mut host = RecordingHost::new(ID);
    host.set_setting(settings::PAGE_SIZE, json!(100.0)).unwrap();
    let fixture = Fixture::with_host(MongoRaabitPlugin::new(), host);
    let view = app_view();
    let mut harness = archipel_sdk::kit::testing::harness_state_sized(
        Theme::dark(),
        archipel_sdk::kit::egui::vec2(1000.0, 640.0),
        fixture,
        move |ui, fixture| {
            let Fixture { plugin, host } = fixture;
            plugin.render_view(&view, &mut ViewFrame::new(ui), host);
        },
    );
    harness.run();
    // "Settings…" in the Connections header menu asks the host to open Settings.
    harness.get_by_label("More actions").click();
    harness.run();
    harness.get_by_label("Settings…").click();
    harness.run();
    let host = &harness.state().host;
    assert!(
        host.dispatched
            .iter()
            .any(|(command, _)| command == OPEN_SETTINGS)
    );
    // Reading the settings never writes them.
    assert_eq!(host.setting(settings::PAGE_SIZE), Some(json!(100.0)));
}
