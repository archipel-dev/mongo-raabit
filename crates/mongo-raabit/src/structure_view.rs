//! Structure tab: Schema Explorer and Index Manager of one collection, switched with
//! a segmented control.

use crate::app::{MongoRaabit, TabId};
use crate::egui::Ui;
use crate::tools::StructureSection;
use archipel_sdk::kit::components::SegmentedControl;

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let Some(tab) = app.structure_tab_mut(tab_id) else {
        return;
    };
    crate::egui::Frame::new()
        .inner_margin(crate::egui::Margin {
            left: 14,
            right: 14,
            top: 8,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.add(SegmentedControl::new(
                &mut tab.section,
                [
                    (StructureSection::Schema, "Schema"),
                    (StructureSection::Indexes, "Indexes"),
                ],
            ));
        });
    match tab.section {
        StructureSection::Schema => crate::schema_view::show(app, tab_id, ui),
        StructureSection::Indexes => crate::index_view::show(app, tab_id, ui),
    }
}
