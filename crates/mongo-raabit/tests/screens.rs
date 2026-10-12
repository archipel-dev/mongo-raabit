//! Kit audit of every screen of the chrome (welcome, connection manager, tree, main
//! window) at 1280×800, in both themes: named and large enough targets, no overlaps, Tab
//! reaches everything.

use archipel_sdk::kit::egui::vec2;
use archipel_sdk::kit::{Theme, testing};
use archipel_sdk::testing::{Harness, Queryable, audit};
use mongo_raabit::MongoRaabit;
use mongo_raabit::app::{CollectionEntry, DatabaseEntry, Load, NodeKey, Session, SessionState};
use mongo_raabit::core::bson::Document;
use mongo_raabit::core::connection::{
    CollectionInfo, CollectionKind, CollectionStats, DatabaseInfo,
};
use mongo_raabit::core::profile::ConnectionProfile;
use mongo_raabit::manager::Page;

fn themes() -> [Theme; 2] {
    [Theme::dark(), Theme::light()]
}

fn screen<'a>(theme: Theme, app: MongoRaabit) -> Harness<'a, MongoRaabit> {
    let mut harness = testing::harness_state_sized(
        theme,
        vec2(1280.0, 800.0),
        app,
        |ui, app: &mut MongoRaabit| app.ui(ui),
    );
    harness.run_ok();
    harness
}

fn saved(app: &mut MongoRaabit) {
    for (name, uri) in [
        ("Local", "mongodb://localhost:27017"),
        (
            "Staging",
            "mongodb://staging.example.net:27017/?replicaSet=rs0",
        ),
    ] {
        let (mut profile, _) = ConnectionProfile::from_uri(uri).unwrap();
        profile.name = name.into();
        if name == "Staging" {
            profile.folder = "Team/EU".into();
        }
        app.store.upsert(profile);
    }
}

fn collection(name: &str, kind: CollectionKind, count: u64) -> CollectionEntry {
    CollectionEntry {
        info: CollectionInfo {
            name: name.into(),
            kind,
            view_on: None,
            pipeline: Vec::new(),
            options: Document::new(),
        },
        stats: Some(CollectionStats {
            count,
            ..CollectionStats::default()
        }),
    }
}

/// An app with one session whose databases and collections are already loaded.
fn with_tree() -> MongoRaabit {
    let mut app = MongoRaabit::new(None);
    saved(&mut app);
    let profile = app.store.profiles[0].clone();
    let databases = vec![
        DatabaseEntry {
            info: DatabaseInfo {
                name: "shop".into(),
                size_on_disk: 4 << 20,
                empty: false,
            },
            collections: Load::Ready(vec![
                collection("orders", CollectionKind::Collection, 1200),
                collection("customers", CollectionKind::Collection, 85),
                collection("daily_totals", CollectionKind::View, 0),
            ]),
        },
        DatabaseEntry {
            info: DatabaseInfo {
                name: "admin".into(),
                size_on_disk: 0,
                empty: true,
            },
            collections: Load::Idle,
        },
    ];
    app.sessions.push(Session {
        id: 1,
        profile,
        state: SessionState::Connecting,
        databases: Load::Ready(databases),
    });
    app.expanded.insert(NodeKey::Session(1));
    app.expanded.insert(NodeKey::Database(1, "shop".into()));
    app.selected = Some(NodeKey::Collection(1, "shop".into(), "orders".into()));
    app
}

#[test]
fn welcome_passes_the_audit() {
    for theme in themes() {
        let mut app = MongoRaabit::new(None);
        saved(&mut app);
        let mut harness = screen(theme, app);
        harness.get_by_label("New Connection");
        harness.get_by_label("Connect to localhost");
        audit(&mut harness).assert_clean();
    }
}

#[test]
fn welcome_without_saved_connections_passes_the_audit() {
    for theme in themes() {
        let mut harness = screen(theme, MongoRaabit::new(None));
        audit(&mut harness).assert_clean();
    }
}

#[test]
fn every_connection_manager_tab_passes_the_audit() {
    for theme in themes() {
        for (page, label) in Page::ALL {
            let mut app = MongoRaabit::new(None);
            saved(&mut app);
            app.open_manager();
            {
                let manager = app.manager.as_mut().unwrap();
                manager.page = page;
                // Show the optional fields of the page.
                manager.form.draft.tls.enabled = true;
                manager.form.draft.auth.mechanism =
                    mongo_raabit::core::profile::AuthMechanism::ScramSha256;
            }
            // The dialog on its own: behind a modal, the window is not reachable anyway.
            let mut harness =
                testing::harness_state(theme.clone(), app, |ui, app: &mut MongoRaabit| {
                    mongo_raabit::manager::show(app, ui.ctx())
                });
            harness.set_size(vec2(1280.0, 800.0));
            harness.run_ok();
            harness.get_by_label("Connection Manager");
            let _ = label;
            audit(&mut harness).assert_clean();
        }
    }
}

#[test]
fn the_tree_passes_the_audit() {
    for theme in themes() {
        let mut harness = screen(theme, with_tree());
        harness.get_by_role_and_label(testing::Role::Tree, "Connections");
        audit(&mut harness).assert_clean();
    }
}

#[test]
fn the_main_window_passes_the_audit() {
    for theme in themes() {
        let mut harness = screen(theme, with_tree());
        harness.get_by_label("Welcome");
        audit(&mut harness).assert_clean();
    }
}
