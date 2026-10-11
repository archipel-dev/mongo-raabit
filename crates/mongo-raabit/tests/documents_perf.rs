//! Frame time of the documents views with a full page (50 documents). The 16 ms budget is
//! asserted in release builds (`cargo test --release -p mongo-raabit --test
//! documents_perf`); debug builds only print the numbers.

mod common;

use archipel_sdk::kit::egui::vec2;
use archipel_sdk::kit::{Theme, testing};
use common::app_with_documents;
use mongo_raabit::MongoRaabit;
use mongo_raabit::app::ResultView;
use std::time::Instant;

fn average_frame_ms(view: ResultView) -> f64 {
    let (mut app, tab) = app_with_documents(view);
    if view == ResultView::Tree {
        let tab = app.collection_tab_mut(tab).unwrap();
        tab.open.extend(["0".to_owned(), "0/address".to_owned()]);
    }
    let mut harness =
        testing::harness_state(Theme::dark(), app, |ui, app: &mut MongoRaabit| app.ui(ui));
    harness.set_size(vec2(1280.0, 800.0));
    // Warm up: fonts, caches, the first layout.
    for _ in 0..5 {
        harness.run();
    }
    let frames = 60;
    let start = Instant::now();
    for _ in 0..frames {
        harness.step();
    }
    start.elapsed().as_secs_f64() * 1000.0 / frames as f64
}

#[test]
fn fifty_rows_fit_one_frame() {
    for (view, name) in [
        (ResultView::Table, "table"),
        (ResultView::Tree, "tree"),
        (ResultView::Json, "json"),
    ] {
        let ms = average_frame_ms(view);
        println!("{name}: {ms:.2} ms per frame (50 documents, whole window)");
        if !cfg!(debug_assertions) {
            assert!(ms < 16.0, "{name} took {ms:.2} ms per frame");
        }
    }
}
