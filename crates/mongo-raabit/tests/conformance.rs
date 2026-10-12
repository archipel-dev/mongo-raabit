//! Kit conformance for every interactive element type the app chrome, the welcome tab, the
//! connection manager and the tree are built from, configured the way they are used there:
//! it renders and passes the audit in both themes, is found by its label, and reacts to
//! click, Tab, Enter and Space (disabled: to none of them).

use archipel_sdk::kit::components::{
    Button, Checkbox, Dropdown, IconButton, ListRow, MenuItem, Tab, TabBar, TextField, Tree,
    TreeRow,
};
use archipel_sdk::kit::egui::{Key, Response};
use archipel_sdk::kit::parts::Slot;
use archipel_sdk::kit::testing::{self, Queryable};
use archipel_sdk::kit::{Theme, Tone};
use mongo_raabit::icons::Icon;
use mongo_raabit::widgets;

#[test]
fn toolbar_icon_buttons() {
    for (icon, label) in [
        (Icon::Unplug, "Disconnect"),
        (Icon::Refresh, "Refresh"),
        (Icon::Plug, "Connection Manager… (Ctrl+N)"),
        (Icon::Plus, "New connection"),
        (Icon::Import, "Import from connection string"),
        (Icon::Save, "Save connection"),
        (Icon::Copy, "Duplicate"),
    ] {
        testing::conformance(label, |ui, enabled| {
            ui.add(IconButton::new(icon, label).enabled(enabled))
        })
        .run();
    }
}

#[test]
fn destructive_icon_button() {
    testing::conformance("Delete", |ui, enabled| {
        ui.add(
            IconButton::new(Icon::Trash, "Delete")
                .tone(Tone::Error)
                .enabled(enabled),
        )
    })
    .run();
}

#[test]
fn buttons() {
    testing::conformance("Connect…", |ui, enabled| {
        ui.add(
            Button::primary("Connect…")
                .icon(Icon::Plug)
                .enabled(enabled),
        )
    })
    .run();
}

#[test]
fn text_fields() {
    let mut filter = String::new();
    testing::conformance("Filter connections", |ui, enabled| {
        let response = ui.add(
            TextField::search(&mut filter)
                .placeholder("Filter connections")
                .enabled(enabled),
        );
        mongo_raabit::widgets::name_input(&response, "Filter connections", enabled);
        response
    })
    .no_click()
    .keys(&[])
    .run();
    let mut name = String::new();
    testing::conformance("Name", |ui, enabled| {
        ui.add(TextField::new(&mut name).label("Name").enabled(enabled))
    })
    .no_click()
    .keys(&[])
    .run();
    let mut password = String::new();
    testing::conformance("Password", |ui, enabled| {
        ui.add(
            TextField::new(&mut password)
                .label("Password")
                .password(true)
                .enabled(enabled),
        )
    })
    .no_click()
    .keys(&[])
    .run();
}

#[test]
fn query_field_takes_text() {
    let mut harness = testing::harness_state(Theme::dark(), String::new(), |ui, text| {
        widgets::query_field(ui, "Filter", text, "{ }", 240.0);
    });
    let input =
        harness.get_by_role_and_label(archipel_sdk::kit::testing::Role::TextInput, "Filter");
    input.focus();
    input.type_text("{a: 1}");
    harness.run();
    assert_eq!(harness.state(), "{a: 1}");
}

#[test]
fn dropdowns_and_checkboxes() {
    let mut kind = "Standalone".to_owned();
    testing::conformance("Connection type", |ui, enabled| {
        ui.add(
            Dropdown::new(
                "kind",
                &mut kind,
                ["Standalone".to_owned(), "Replica set".to_owned()],
            )
            .label("Connection type")
            .enabled(enabled),
        )
    })
    .run();
    let mut tls = false;
    testing::conformance("Use TLS/SSL", |ui, enabled| {
        ui.add_enabled(enabled, Checkbox::new(&mut tls, "Use TLS/SSL"))
    })
    .run();
}

#[test]
fn editor_tabs_select_and_close() {
    let mut harness = testing::harness_state(Theme::dark(), (0usize, Vec::new()), |ui, state| {
        let tabs = [
            Tab::new("Welcome").icon(Icon::Database).closable(true),
            Tab::new("orders · Query")
                .icon(Icon::Collection)
                .closable(true),
        ];
        let out = TabBar::new(&tabs, state.0).show(ui);
        if let Some(index) = out.selected {
            state.0 = index;
        }
        if let Some(index) = out.closed {
            state.1.push(index);
        }
    });
    harness.get_by_label("orders · Query").click();
    harness.run();
    assert_eq!(harness.state().0, 1);
    harness.get_by_label("Close orders · Query").click();
    harness.run();
    assert_eq!(harness.state().1, [1]);
}

#[test]
fn connection_manager_tabs_select_on_click() {
    let labels = [
        "Server",
        "Authentication",
        "TLS / SSL",
        "SSH Tunnel",
        "Advanced",
    ];
    let mut harness = testing::harness_state(Theme::dark(), 0usize, |ui, page| {
        let tabs: Vec<Tab> = labels.iter().map(|l| Tab::new(*l)).collect();
        if let Some(index) = TabBar::new(&tabs, *page).show(ui).selected {
            *page = index;
        }
    });
    harness.get_by_label("Advanced").click();
    harness.run();
    assert_eq!(*harness.state(), 4);
}

#[test]
fn saved_connection_rows() {
    testing::conformance("Local localhost:27017", |ui, enabled| {
        ListRow::new("Local")
            .icon(Icon::Server)
            .secondary("localhost:27017")
            .trailing(Slot::text("Team"))
            .enabled(enabled)
            .show(ui)
            .response
    })
    .run();
}

#[test]
fn tree_rows_select_and_toggle() {
    #[derive(Default)]
    struct State {
        selected: Option<usize>,
        open: bool,
    }
    let mut harness = testing::harness_state(Theme::dark(), State::default(), |ui, s| {
        let mut rows = vec![
            TreeRow::new("shop", 0)
                .icon(Icon::Database)
                .secondary("4.0 MB")
                .parent(s.open),
        ];
        if s.open {
            rows.push(
                TreeRow::new("orders", 1)
                    .icon(Icon::Collection)
                    .secondary("1200"),
            );
        }
        let out = Tree::new("t", &rows, &mut s.selected).show(ui);
        if out.toggled.is_some() {
            s.open = !s.open;
        }
    });
    harness.get_by_label("shop 4.0 MB").click();
    harness.run();
    assert_eq!(harness.state().selected, Some(0));
    harness.key_press(Key::ArrowRight);
    harness.run();
    assert!(harness.state().open, "→ expands the focused database");
    harness.key_press(Key::ArrowDown);
    harness.run();
    assert_eq!(
        harness.state().selected,
        Some(1),
        "↓ moves to the collection"
    );
    harness.key_press(Key::ArrowLeft);
    harness.run();
    assert_eq!(harness.state().selected, Some(0), "← moves to the parent");
}

#[test]
fn node_menu_items() {
    testing::conformance("Query with SQL", |ui, enabled| -> Response {
        ui.add(
            MenuItem::new("Query with SQL")
                .icon(Icon::Sql)
                .enabled(enabled),
        )
    })
    .run();
}
