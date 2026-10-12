//! The right tool window: the tools that act on the active collection tab, as content
//! tabs (Query, Aggregation, Indexes). What they produce shows in the center island; with
//! no collection tab active the window says how to get one.

use crate::app::{CenterView, Load, MongoRaabit, QueryMode, TabId, ToolTab};
use crate::egui::Ui;
use crate::query_builder::{self, Event, FieldList};
use crate::widgets;
use archipel_sdk::kit::components::EmptyState;
use archipel_sdk::kit::{Glyph, Tone};
use mongo_raabit_core::fields::query_fields;
use mongo_raabit_core::format;
use std::sync::Arc;

/// Title of the right tool window (and of its stripe button).
pub const TITLE: &str = "Collection";

/// The fields of a tab as last computed, and what they were computed from.
#[derive(Clone, Default)]
pub struct FieldsCache {
    key: Option<(u64, u64)>,
    list: Arc<FieldList>,
}

pub fn show(app: &mut MongoRaabit, ui: &mut Ui) {
    let Some(tab_id) = app.active_collection_tab() else {
        ui.add_space(archipel_sdk::kit::tokens::size::FORM_GROUP_SPACING);
        EmptyState::new("No collection open")
            .icon(Glyph::Table)
            .hint("Open a collection in Connections to query it, build a pipeline or manage its indexes.")
            .show(ui);
        return;
    };
    context_line(app, tab_id, ui);
    tool_switch(app, ui);
    match app.current_tool() {
        ToolTab::Query => query(app, tab_id, ui),
        ToolTab::Aggregation => {
            app.ensure_aggregation(tab_id);
            crate::aggregation_view::tool(app, tab_id, ui);
        }
        ToolTab::Indexes => {
            app.ensure_structure(tab_id);
            crate::index_view::show(app, tab_id, ui);
        }
    }
}

/// The tool window as a host's tool window (the IDE's right anchor): the same tools for
/// the active collection tab of the Mongo Raabit editor tab.
pub fn show_in_host(app: &mut MongoRaabit, ui: &mut Ui) {
    show(app, ui);
}

/// Query | Pipeline | Indexes: which tool the window shows.
fn tool_switch(app: &mut MongoRaabit, ui: &mut Ui) {
    let mut tool = app.current_tool();
    crate::egui::Frame::new()
        .inner_margin(crate::egui::Margin {
            left: archipel_sdk::kit::tokens::size::CONTENT_INSET as i8,
            right: archipel_sdk::kit::tokens::size::CONTENT_INSET as i8,
            top: archipel_sdk::kit::tokens::size::SECTION_SPACING as i8,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.add(archipel_sdk::kit::components::SegmentedControl::new(
                &mut tool,
                ToolTab::ALL.iter().copied().zip(ToolTab::TITLES),
            ));
        });
    if tool != app.current_tool() {
        app.show_tool(tool);
        app.tool_picked(tool);
    }
}

/// Which tab the tools act on: `database.collection`, muted, above the tool.
fn context_line(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let Some(tab) = app.collection_tab_mut(tab_id) else {
        return;
    };
    let text = format!("{}.{}", tab.db, tab.collection);
    crate::egui::Frame::new()
        .inner_margin(crate::egui::Margin {
            left: archipel_sdk::kit::tokens::size::CONTENT_INSET as i8,
            right: archipel_sdk::kit::tokens::size::CONTENT_INSET as i8,
            top: archipel_sdk::kit::tokens::size::SECTION_SPACING as i8,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                crate::icons::icon(
                    ui,
                    crate::icons::Icon::Collection,
                    Tone::Muted.resolve(ui.ctx()),
                );
                widgets::label(ui, Tone::Secondary, text);
            });
        });
}

/// The fields the builder lists for a tab: from its schema sample when there is one, else
/// from the documents of its current page. Computed again only when either changes.
pub fn field_list(app: &mut MongoRaabit, tab_id: TabId) -> Arc<FieldList> {
    let Some(tab) = app.collection_tab_mut(tab_id) else {
        return Arc::default();
    };
    let schema = tab.structure.as_ref().map(|s| &s.schema);
    let generation = schema.map_or(0, |s| s.generation);
    let key = (generation, tab.seq);
    if tab.fields.key == Some(key) {
        return tab.fields.list.clone();
    }
    let sampling = schema.is_some_and(|s| s.report.is_loading());
    let list = match schema.and_then(|s| s.report.ready()) {
        Some(report) if !report.fields.is_empty() => FieldList {
            fields: query_fields(report),
            source: format!("{} sampled documents", format::group(report.sampled)),
            loading: false,
        },
        _ => {
            let page = tab.result.ready().map(|p| p.documents.as_slice());
            let fields = page
                .map(|docs| query_fields(&mongo_raabit_core::schema::analyze(docs)))
                .unwrap_or_default();
            FieldList {
                loading: sampling && fields.is_empty(),
                fields,
                source: "the documents of this page".into(),
            }
        }
    };
    // While the page or the sample loads, keep what was there and look again next frame.
    if !matches!(tab.result, Load::Loading) && !sampling {
        tab.fields.key = Some(key);
    }
    tab.fields.list = Arc::new(list);
    tab.fields.list.clone()
}

fn query(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    app.ensure_structure(tab_id);
    let fields = field_list(app, tab_id);
    let Some(tab) = app.collection_tab_mut(tab_id) else {
        return;
    };
    let event = query_builder::show(ui, &mut tab.builder, &mut tab.mode, &mut tab.query, &fields);
    match event {
        Event::None => {}
        Event::Changed => tab.query = tab.builder.to_query_text(),
        Event::Run => {
            if tab.mode == QueryMode::Builder {
                tab.query = tab.builder.to_query_text();
            }
            tab.page = 0;
            tab.drill.clear();
            tab.selected = None;
            tab.center = CenterView::Documents;
            app.run_query(tab_id, true);
        }
    }
}
