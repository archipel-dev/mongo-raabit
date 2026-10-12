//! Index Manager, the Indexes tool window: list with sizes, usage and properties; create,
//! hide/unhide and drop. Every change is a `Write` through the confirmation
//! (`request_write`), refused on a read-only connection.
//!
//! The form's validation and how indexes are described live in
//! `mongo_raabit_core::indexes`; this module lays them out.

use crate::app::{Load, MongoRaabit, TabId};
use crate::egui::Ui;
use crate::icons::Icon;
use crate::tools::IndexTab;
use crate::widgets;
use crate::writes::READ_ONLY_TIP;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::{
    Banner, Button, Checkbox, FieldLabel, IconButton, ListRow, PropertyGrid, Severity, Spinner,
    Tag, TextField, TooltipExt,
};
use archipel_sdk::kit::parts::Surface;
use archipel_sdk::kit::tokens::size;
use mongo_raabit_core::connection::IndexInfo;
use mongo_raabit_core::format;
use mongo_raabit_core::indexes::{self, PRIMARY_INDEX};

enum Action {
    Create,
    Drop(String),
    Hide(String, bool),
    Refresh,
}

const TTL_WIDTH: f32 = 120.0;
const KEYS_TIP: &str = "1 / -1 for ascending / descending, \"text\", \"2dsphere\", \"hashed\"";
const HIDE_TIP: &str = "Hidden indexes are kept up to date but not used by the planner";

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let read_only = app
        .index_tab_mut(tab_id)
        .map(|t| t.session)
        .is_some_and(|s| app.session_read_only(s));
    let Some(tab) = app.index_tab_mut(tab_id) else {
        return;
    };
    let IndexTab {
        indexes,
        form,
        form_open,
        error,
        busy,
        confirm_drop,
        ..
    } = tab;
    let mut actions = Vec::new();
    if read_only {
        *form_open = false;
    }
    widgets::padded(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
            let new = ui.add(
                Button::primary("New Index")
                    .icon(Icon::Plus)
                    .enabled(!read_only),
            );
            let new = if read_only {
                new.tooltip(READ_ONLY_TIP)
            } else {
                new
            };
            if new.clicked() {
                *form_open = !*form_open;
            }
            if ui
                .add(IconButton::new(Icon::Refresh, "Refresh indexes"))
                .clicked()
            {
                actions.push(Action::Refresh);
            }
            if read_only {
                ui.add(Tag::new("Read-only").tone(Tone::Warning));
            }
            if *busy {
                ui.add(Spinner::new().label("Working"));
            }
        });
        if *form_open {
            ui.add_space(size::SECTION_SPACING);
            new_index_form(ui, form, form_open, &mut actions);
        }
        if let Some(err) = &*error {
            ui.add_space(size::SECTION_SPACING);
            Banner::new(Severity::Error, err).show(ui);
        }
    });
    match &*indexes {
        Load::Loading | Load::Idle => widgets::busy(ui, "Loading indexes…"),
        Load::Failed(err) => {
            widgets::padded(ui, |ui| {
                Banner::new(Severity::Error, err).show(ui);
            });
        }
        Load::Ready(list) => index_list(ui, tab_id, list, confirm_drop, read_only, &mut actions),
    }
    for action in actions {
        match action {
            Action::Create => app.create_index(tab_id),
            Action::Drop(name) => app.drop_index(tab_id, name),
            Action::Hide(name, hidden) => app.set_index_hidden(tab_id, name, hidden),
            Action::Refresh => app.load_indexes(tab_id),
        }
    }
}

/// The "New index" form on an island, one field under the other.
fn new_index_form(
    ui: &mut Ui,
    form: &mut mongo_raabit_core::indexes::IndexForm,
    open: &mut bool,
    actions: &mut Vec<Action>,
) {
    Surface::Island.frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let width = ui.available_width();
        ui.add(FieldLabel::new("Keys"));
        widgets::value_field(ui, "Keys", &mut form.keys, "{ email: 1 }", width, true)
            .response
            .tooltip(KEYS_TIP);
        ui.add(FieldLabel::new("Name"));
        ui.add(
            TextField::new(&mut form.name)
                .name("Name")
                .placeholder("generated from keys")
                .width(width),
        );
        ui.horizontal_wrapped(|ui| {
            ui.add(Checkbox::new(&mut form.unique, "Unique"));
            ui.add(Checkbox::new(&mut form.sparse, "Sparse"));
            ui.add(Checkbox::new(&mut form.hidden, "Hidden"));
        });
        ui.add(FieldLabel::new("TTL (seconds)"));
        ui.add(
            TextField::new(&mut form.ttl)
                .name("TTL (seconds)")
                .placeholder("none")
                .monospace(true)
                .width(TTL_WIDTH),
        );
        ui.add(FieldLabel::new("Partial filter"));
        widgets::value_field(
            ui,
            "Partial filter",
            &mut form.partial,
            "{ active: true }",
            width,
            true,
        );
        ui.add_space(size::SECTION_SPACING);
        ui.horizontal(|ui| {
            if ui
                .add(Button::primary("Create Index").icon(Icon::Check))
                .clicked()
            {
                actions.push(Action::Create);
            }
            if ui.add(Button::secondary("Cancel")).clicked() {
                *open = false;
            }
        });
    });
}

/// The totals, the indexes as rows, and the details and actions of the selected one.
fn index_list(
    ui: &mut Ui,
    tab_id: TabId,
    list: &[IndexInfo],
    confirm: &mut Option<String>,
    read_only: bool,
    actions: &mut Vec<Action>,
) {
    let mem_id = ui.id().with(("raabit-indexes-selected", tab_id));
    let mut selected: Option<String> = ui.data(|d| d.get_temp(mem_id)).unwrap_or(None);
    widgets::padded(ui, |ui| {
        let (count, bytes) = indexes::totals(list);
        widgets::label(
            ui,
            Tone::Secondary,
            format!("{count} indexes · {}", format::bytes(bytes)),
        );
    });
    for index in list {
        let row = ListRow::new(&index.name)
            .icon(Icon::Index)
            .icon_tone(if index.hidden {
                Tone::Muted
            } else {
                Tone::Warning
            })
            .label_tone(if index.hidden {
                Tone::Muted
            } else {
                Tone::Default
            })
            .secondary(indexes::keys_label(index))
            .selected(selected.as_deref() == Some(index.name.as_str()))
            .show(ui);
        if row.response.clicked() {
            selected = Some(index.name.clone());
            if confirm.as_deref() != selected.as_deref() {
                *confirm = None;
            }
        }
    }
    if let Some(index) = selected
        .as_ref()
        .and_then(|name| list.iter().find(|i| &i.name == name))
    {
        widgets::padded(ui, |ui| {
            PropertyGrid::new(("raabit-index-details", tab_id))
                .mono_row("Keys", indexes::keys_label(index))
                .row("Properties", indexes::properties_label(index))
                .row("Size", index.size.map(format::bytes).unwrap_or_default())
                .row("Usage", indexes::usage_label(index.ops))
                .show(ui);
            ui.add_space(size::SECTION_SPACING);
            let target = (index.name != PRIMARY_INDEX && !read_only).then_some(index);
            ui.horizontal(|ui| {
                let hidden = index.hidden;
                let label = if hidden { "Unhide" } else { "Hide" };
                if ui
                    .add(
                        Button::secondary(label)
                            .small(true)
                            .enabled(target.is_some()),
                    )
                    .tooltip(if read_only { READ_ONLY_TIP } else { HIDE_TIP })
                    .clicked()
                {
                    actions.push(Action::Hide(index.name.clone(), !hidden));
                }
                let confirming =
                    target.is_some() && confirm.as_deref() == Some(index.name.as_str());
                if confirming {
                    if ui.add(Button::danger("Confirm drop").small(true)).clicked() {
                        actions.push(Action::Drop(index.name.clone()));
                        *confirm = None;
                    }
                } else if ui
                    .add(
                        Button::secondary("Drop…")
                            .small(true)
                            .enabled(target.is_some()),
                    )
                    .tooltip(if read_only {
                        READ_ONLY_TIP
                    } else {
                        "Drop the selected index"
                    })
                    .clicked()
                {
                    *confirm = Some(index.name.clone());
                }
            });
        });
    } else if !list.is_empty() {
        widgets::padded(ui, |ui| {
            widgets::note(
                ui,
                Tone::Muted,
                "Select an index to see its details, hide or drop it.",
            );
        });
    }
    ui.data_mut(|d| d.insert_temp(mem_id, selected));
}
