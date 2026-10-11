//! Welcome tab: quick actions and saved connections.

use crate::app::MongoRaabit;
use crate::egui::{Align, Layout, Ui};
use crate::icons::Icon;
use archipel_sdk::kit::components::{Button, EmptyState, ListRow, SectionHeader};
use archipel_sdk::kit::parts::Slot;
use archipel_sdk::kit::tokens::size;
use mongo_raabit_core::profile::ConnectionProfile;

/// Width of the saved-connections list.
const LIST_WIDTH: f32 = 560.0;

pub fn show(app: &mut MongoRaabit, ui: &mut Ui) {
    let mut connect: Option<ConnectionProfile> = None;
    archipel_sdk::kit::parts::scroll_vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let new = EmptyState::new("Mongo Raabit")
                .icon(Icon::Database)
                .hint("Browse, query and shape your MongoDB data")
                .action("New Connection")
                .show(ui);
            if new.clicked() {
                app.open_manager();
            }
            ui.vertical_centered(|ui| {
                if ui
                    .add(Button::secondary("Connect to localhost").icon(Icon::Plug))
                    .clicked()
                {
                    // The default profile is a standalone server on localhost:27017.
                    connect = Some(ConnectionProfile {
                        name: "localhost".into(),
                        ..ConnectionProfile::default()
                    });
                }
            });
            let saved: Vec<ConnectionProfile> = app.store.profiles.clone();
            if saved.is_empty() {
                return;
            }
            ui.add_space(size::SECTION_SPACING * 2.0);
            let width = ui.available_width().min(LIST_WIDTH);
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.allocate_ui(crate::egui::vec2(width, 0.0), |ui| {
                    ui.set_width(width);
                    ui.add(SectionHeader::new("Saved connections"));
                    for profile in &saved {
                        let mut row = ListRow::new(profile.name.as_str())
                            .icon(Icon::Server)
                            .secondary(profile.display_uri());
                        if !profile.folder.is_empty() {
                            row = row.trailing(Slot::text(profile.folder.clone()));
                        }
                        if row.show(ui).response.clicked() {
                            connect = Some(profile.clone());
                        }
                    }
                });
            });
        });
    if let Some(profile) = connect {
        app.connect(profile);
    }
}
