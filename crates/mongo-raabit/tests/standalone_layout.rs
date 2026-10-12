//! Geometry contract for the existing mouse-only Xvfb performance driver.
use archipel_sdk::app::{App, AppOptions, SecretBackend, ThemeMode};
use archipel_sdk::kit::egui::pos2;
use archipel_sdk::kit::testing::{self, Queryable};
use mongo_raabit::MongoRaabitPlugin;

#[test]
fn perf_driver_targets_match_the_standalone_islands() {
    let root = archipel_sdk::testing::DataDir::create().unwrap();
    let app = App::new(
        MongoRaabitPlugin::new(),
        AppOptions {
            dirs_root: Some(root.path().to_path_buf()),
            secrets: SecretBackend::File,
            theme: ThemeMode::Dark,
            ..AppOptions::default()
        },
    )
    .unwrap();
    let mut h = testing::builder()
        .with_size(archipel_sdk::kit::egui::vec2(1280.0, 800.0))
        .build_state(|ctx, app: &mut App| app.update(ctx), app);
    h.run_ok();
    let connect = h.get_by_label("Connect to localhost").rect();
    assert!(
        connect.contains(pos2(562.0, 253.0)),
        "welcome target: {connect:?}"
    );
    let open = h.get_by_label("Connect…").rect();
    assert!(open.contains(pos2(177.0, 217.0)), "dialog opener: {open:?}");
    h.get_by_label("Connect…").click();
    h.run_ok();
    let close = h.get_by_label("Close").rect();
    assert!(
        close.contains(pos2(958.0, 644.0)),
        "dialog close: {close:?}"
    );
}
