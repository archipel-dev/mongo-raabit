//! The Query tool window in the right island, driven like a user: fields dragged with the
//! pointer onto Filter, Projection and Sort, sort rows dragged to reorder, the double-click,
//! Enter and context-menu alternatives, Run on the active tab, and switching tabs switching
//! the builder. Everything here is simulated pointer and keyboard input on the whole
//! window at 1280×800; nothing calls the builder directly.

mod common;

use archipel_sdk::kit::egui::{self, Key, Pos2, Rect, vec2};
use archipel_sdk::kit::{Theme, testing};
use archipel_sdk::testing::{Harness, Queryable};
use common::app_with_documents;
use mongo_raabit::MongoRaabit;
use mongo_raabit::app::{
    CenterView, CollectionTab, Load, QueryMode, ResultView, Tab, TabKind, ToolTab,
};
use mongo_raabit::core::bson::doc;
use mongo_raabit::core::builder::{Item, Operator, ProjectionMode, ValueType};
use mongo_raabit::core::connection::CollectionKind;
use mongo_raabit::core::query::Page;
use mongo_raabit::query_builder::{FILTER_HINT, PROJECTION_HINT, SORT_HINT};
use std::time::Duration;

const PEOPLE: u64 = 100;
const ORDERS: u64 = 101;

/// `shop.people` (active) and `shop.orders`, both in the visual builder.
fn two_tabs() -> MongoRaabit {
    let (mut app, _) = app_with_documents(ResultView::Table);
    app.collection_tab_mut(PEOPLE).unwrap().mode = QueryMode::Builder;
    let mut orders = CollectionTab::new(1, "shop", "orders", CollectionKind::Collection);
    orders.count = Load::Ready(2);
    orders.result = Load::Ready(Page {
        documents: vec![
            doc! {"_id": 1, "total": 9.5, "status": "open"},
            doc! {"_id": 2, "total": 12.0, "status": "paid"},
        ],
        page: 0,
        page_size: 50,
        has_more: false,
        elapsed: Duration::from_millis(3),
    });
    app.tabs.push(Tab {
        id: ORDERS,
        kind: TabKind::Collection(Box::new(orders)),
    });
    app.active_tab = 2;
    app
}

fn window<'a>(app: MongoRaabit) -> Harness<'a, MongoRaabit> {
    let mut harness = testing::harness_state_sized(
        Theme::dark(),
        vec2(1280.0, 1000.0),
        app,
        |ui, app: &mut MongoRaabit| app.ui(ui),
    );
    harness.run_ok();
    harness
}

fn zone(harness: &Harness<'_, MongoRaabit>, hint: &str) -> Rect {
    harness.get_by_label(hint).rect()
}

/// The left edge of the right island: where its drop zones start.
fn island_left(harness: &Harness<'_, MongoRaabit>) -> f32 {
    zone(harness, FILTER_HINT).left() - 40.0
}

/// The center of the node labeled `label` that lies inside `area` (labels repeat: the
/// table header, the field list and the zones all name the fields).
fn inside(harness: &Harness<'_, MongoRaabit>, label: &str, area: Rect) -> Pos2 {
    harness
        .get_all_by_label(label)
        .map(|n| n.rect())
        .find(|r| area.contains(r.center()))
        .unwrap_or_else(|| panic!("no {label:?} in {area:?}"))
        .center()
}

/// The row of `label` in the field list of the right island.
fn field(harness: &Harness<'_, MongoRaabit>, label: &str) -> Pos2 {
    let left = island_left(harness);
    let top = zone(harness, FILTER_HINT).top();
    let area = Rect::from_min_max(egui::pos2(left, 0.0), egui::pos2(1280.0, top));
    inside(harness, label, area)
}

/// Presses at `from`, moves to `to` in steps and lets go, as a mouse does.
fn drag(harness: &mut Harness<'_, MongoRaabit>, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    harness.run();
    harness.drag_at(from);
    harness.run();
    for step in 1..=10 {
        let t = step as f32 / 10.0;
        harness.hover_at(from + (to - from) * t);
        harness.run();
    }
    harness.drop_at(to);
    harness.run();
    harness.run();
}

fn double_click(harness: &mut Harness<'_, MongoRaabit>, at: Pos2) {
    harness.hover_at(at);
    harness.run();
    for _ in 0..2 {
        harness.drag_at(at);
        harness.step();
        harness.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    harness.run();
}

fn builder(
    harness: &mut Harness<'_, MongoRaabit>,
    tab: u64,
) -> mongo_raabit::core::builder::Builder {
    harness
        .state_mut()
        .collection_tab_mut(tab)
        .unwrap()
        .builder
        .model
        .clone()
}

fn conditions(model: &mongo_raabit::core::builder::Builder) -> Vec<(String, Operator, ValueType)> {
    model
        .filter
        .items
        .iter()
        .map(|item| match item {
            Item::Condition(c) => (c.field.clone(), c.operator, c.value_type),
            Item::Group(_) => panic!("a condition"),
        })
        .collect()
}

#[test]
fn fields_dragged_onto_each_zone_land_there() {
    let mut h = window(two_tabs());
    // People is the active tab here.
    h.get_by_label("people").click();
    h.run();
    let to = zone(&h, FILTER_HINT).center();
    let from = field(&h, "age");
    drag(&mut h, from, to);
    assert_eq!(
        conditions(&builder(&mut h, PEOPLE)),
        [("age".to_owned(), Operator::Equals, ValueType::Number)],
        "a number field filters on a number"
    );

    let from = field(&h, "name");
    let to = zone(&h, PROJECTION_HINT).center();
    drag(&mut h, from, to);
    let model = builder(&mut h, PEOPLE);
    assert_eq!(model.projection.fields, ["name"]);
    assert_eq!(model.projection.mode, ProjectionMode::Include);

    let from = field(&h, "score");
    let to = zone(&h, SORT_HINT).center();
    drag(&mut h, from, to);
    let from = field(&h, "name");
    let to = zone(&h, SORT_HINT).center();
    drag(&mut h, from, to);
    let model = builder(&mut h, PEOPLE);
    let sort: Vec<&str> = model.sort.iter().map(|k| k.field.as_str()).collect();
    assert_eq!(sort, ["score", "name"]);

    // The text the tab will run is the builder's.
    let tab = h.state_mut().collection_tab_mut(PEOPLE).unwrap();
    assert_eq!(tab.query.projection, "{ name: 1 }");
    assert_eq!(tab.query.sort, "{ score: 1, name: 1 }");
    assert_eq!(
        tab.query.filter, "",
        "the age condition waits for its value"
    );
}

#[test]
fn a_drop_outside_the_zones_adds_nothing() {
    let mut h = window(two_tabs());
    let from = field(&h, "status");
    let to = egui::pos2(from.x - 500.0, from.y);
    drag(&mut h, from, to);
    let model = builder(&mut h, ORDERS);
    assert!(model.filter.items.is_empty() && model.sort.is_empty());
    assert!(model.projection.fields.is_empty());
}

#[test]
fn sort_rows_drag_to_reorder_and_toggle_direction() {
    let mut h = window(two_tabs());
    for name in ["status", "total"] {
        let from = field(&h, name);
        let to = zone(&h, SORT_HINT).center();
        drag(&mut h, from, to);
    }
    let order = |h: &mut Harness<'_, MongoRaabit>| -> Vec<(String, bool)> {
        builder(h, ORDERS)
            .sort
            .iter()
            .map(|k| (k.field.clone(), k.descending))
            .collect()
    };
    assert_eq!(
        order(&mut h),
        [("status".to_owned(), false), ("total".to_owned(), false)]
    );
    // Drag "total" onto the "status" row: it becomes the first sort key.
    let area = zone(&h, SORT_HINT);
    let from = inside(&h, "total", area);
    let to = inside(&h, "status", area);
    drag(&mut h, from, to);
    assert_eq!(
        order(&mut h),
        [("total".to_owned(), false), ("status".to_owned(), false)]
    );
    // The keyboard and mouse alternative: the row's Move down button.
    h.get_by_label("Move total down").click();
    h.run();
    assert_eq!(
        order(&mut h),
        [("status".to_owned(), false), ("total".to_owned(), false)]
    );
    // Descending, from the direction toggle of the "total" row.
    let area = zone(&h, SORT_HINT);
    let descending = inside(
        &h,
        "Descending",
        Rect::from_min_max(
            egui::pos2(area.left(), inside(&h, "total", area).y - 10.0),
            egui::pos2(area.right(), inside(&h, "total", area).y + 10.0),
        ),
    );
    h.hover_at(descending);
    h.drag_at(descending);
    h.run();
    h.drop_at(descending);
    h.run();
    assert_eq!(
        order(&mut h),
        [("status".to_owned(), false), ("total".to_owned(), true)]
    );
    assert_eq!(
        h.state_mut().collection_tab_mut(ORDERS).unwrap().query.sort,
        "{ status: 1, total: -1 }"
    );
}

#[test]
fn double_click_enter_and_the_menu_add_fields_without_dragging() {
    let mut h = window(two_tabs());
    let at = field(&h, "status");
    double_click(&mut h, at);
    assert_eq!(
        conditions(&builder(&mut h, ORDERS)),
        [("status".to_owned(), Operator::Equals, ValueType::String)]
    );
    // Keyboard: select the row, then Enter.
    let at = field(&h, "total");
    h.hover_at(at);
    h.drag_at(at);
    h.run();
    h.drop_at(at);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(conditions(&builder(&mut h, ORDERS)).len(), 2);
    assert_eq!(conditions(&builder(&mut h, ORDERS))[1].0, "total");
    // Context menu: Add to Sort.
    let at = field(&h, "total");
    h.hover_at(at);
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    h.get_by_label("Add to Sort").click();
    h.run();
    let model = builder(&mut h, ORDERS);
    assert_eq!(model.sort.len(), 1);
    assert_eq!(model.sort[0].field, "total");
}

#[test]
fn run_applies_to_the_active_tab_and_tabs_keep_their_own_builder() {
    let mut h = window(two_tabs());
    // Orders: status, typed, then Run.
    let from = field(&h, "status");
    let to = zone(&h, FILTER_HINT).center();
    drag(&mut h, from, to);
    h.get_by_label("Value").click();
    h.run();
    h.get_by_label("Value").type_text("open");
    h.run();
    h.get_by_label("Run").click();
    h.run();
    {
        let app = h.state_mut();
        let orders = app.collection_tab_mut(ORDERS).unwrap();
        assert_eq!(orders.query.filter, "{ status: \"open\" }");
        assert_eq!(
            orders.ran.as_ref().map(|q| q.filter.clone()),
            Some(doc! {"status": "open"}),
            "Run ran the builder's query on this tab"
        );
        assert_eq!(orders.center, CenterView::Documents);
        let people = app.collection_tab_mut(PEOPLE).unwrap();
        assert!(people.ran.is_none(), "the other tab did not run");
        assert!(people.builder.model.filter.items.is_empty());
    }
    // Switch to People: the builder shows People's (empty) query and its fields.
    h.get_by_label("people").click();
    h.run();
    assert_eq!(h.state().active_tab, 1);
    assert!(
        h.query_by_label("open").is_none(),
        "orders' value is not shown"
    );
    let from = field(&h, "age");
    let to = zone(&h, SORT_HINT).center();
    drag(&mut h, from, to);
    assert_eq!(builder(&mut h, PEOPLE).sort.len(), 1);
    assert!(builder(&mut h, ORDERS).sort.is_empty());
    // And back: Orders still has its condition.
    h.get_by_label("orders").click();
    h.run();
    assert_eq!(h.get_by_label("Value").value().as_deref(), Some("open"));
}

#[test]
fn the_summary_and_edit_query_lead_to_the_builder() {
    let mut app = two_tabs();
    app.layout.right_hidden = true;
    let mut h = window(app);
    assert!(
        h.query_by_label(FILTER_HINT).is_none(),
        "the tool window is hidden"
    );
    h.get_by_label("Edit query").click();
    h.run();
    h.get_by_label(FILTER_HINT);
    assert_eq!(h.state().current_tool(), ToolTab::Query);
}

#[test]
fn without_a_collection_tab_the_island_says_how_to_get_one() {
    let (mut app, _) = app_with_documents(ResultView::Table);
    app.active_tab = 0;
    let h = window(app);
    h.get_by_label("No collection open");
    assert!(h.query_by_label(FILTER_HINT).is_none());
}

#[test]
fn text_mode_shows_the_same_query_and_visual_takes_it_back() {
    let mut h = window(two_tabs());
    let from = field(&h, "total");
    let to = zone(&h, SORT_HINT).center();
    drag(&mut h, from, to);
    h.get_by_label("Text").click();
    h.run();
    let text = |h: &Harness<'_, MongoRaabit>, label: &str| {
        h.get_by_role_and_label(testing::Role::TextInput, label)
            .value()
    };
    assert_eq!(text(&h, "Sort").as_deref(), Some("{ total: 1 }"));
    // A query the builder cannot show stays in text mode, with the note.
    h.get_by_role_and_label(testing::Role::TextInput, "Filter")
        .click();
    h.run();
    h.get_by_role_and_label(testing::Role::TextInput, "Filter")
        .type_text("{ $where: 'this.total > 1' }");
    h.run();
    h.get_by_label("Visual").click();
    h.run();
    let tab = h.state_mut().collection_tab_mut(ORDERS).unwrap();
    assert_eq!(tab.mode, QueryMode::Code);
    assert!(tab.builder.note.is_some());
    assert_eq!(tab.query.filter, "{ $where: 'this.total > 1' }");
}

#[test]
fn picking_the_aggregation_tool_shows_the_pipeline_output_in_the_center() {
    let mut h = window(two_tabs());
    h.get_by_label("Pipeline").click();
    h.run();
    let app = h.state_mut();
    assert_eq!(app.current_tool(), ToolTab::Aggregation);
    let tab = app.collection_tab_mut(ORDERS).unwrap();
    assert_eq!(tab.center, CenterView::Aggregation);
    assert!(tab.aggregation.is_some());
}

#[test]
#[ignore = "writes images for looking at the layout"]
fn dump_layout() {
    let dir = std::env::var("RAABIT_DUMP").unwrap();
    let mut h = window(two_tabs());
    h.render()
        .unwrap()
        .save(format!("{dir}/island-1.png"))
        .unwrap();
    let from = field(&h, "status");
    let to = zone(&h, FILTER_HINT).center();
    drag(&mut h, from, to);
    h.render()
        .unwrap()
        .save(format!("{dir}/island-2.png"))
        .unwrap();
    h.get_by_label("people").click();
    h.run();
    h.render()
        .unwrap()
        .save(format!("{dir}/island-3.png"))
        .unwrap();
    h.state_mut().layout.right_hidden = true;
    h.run();
    h.render()
        .unwrap()
        .save(format!("{dir}/island-4.png"))
        .unwrap();
}

#[test]
#[ignore = "prints the Tab order"]
fn dump_tab_order() {
    use archipel_sdk::kit::testing::NodeT;
    let (mut app, _) = app_with_documents(ResultView::Table);
    if std::env::var("RAABIT_BUILDER").is_ok() {
        app.collection_tab_mut(PEOPLE).unwrap().mode = QueryMode::Builder;
    }
    let mut h = window(app);
    for _ in 0..120 {
        h.key_press(Key::Tab);
        h.run_ok();
        let focused = h
            .root()
            .children_recursive()
            .find(|n| n.accesskit_node().is_focused())
            .map(|n| {
                format!(
                    "{:?} {:?}",
                    n.accesskit_node().role(),
                    n.accesskit_node().label()
                )
            });
        println!("{focused:?}");
    }
}
