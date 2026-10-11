//! Structure tab: Schema Explorer and Index Manager of one collection, switched with
//! a segmented control.

use crate::app::{MongoRaabit, TabId};
use crate::icons::Icon;
use crate::tools::StructureSection;
use crate::widgets;
use egui::Ui;

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let Some(tab) = app.structure_tab_mut(tab_id) else {
        return;
    };
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 8,
            bottom: 0,
        })
        .show(ui, |ui| {
            widgets::segmented(
                ui,
                &mut tab.section,
                &[
                    (StructureSection::Schema, Icon::Schema, "Schema"),
                    (StructureSection::Indexes, Icon::Index, "Indexes"),
                ],
            );
        });
    match tab.section {
        StructureSection::Schema => crate::schema_view::show(app, tab_id, ui),
        StructureSection::Indexes => crate::index_view::show(app, tab_id, ui),
    }
}
