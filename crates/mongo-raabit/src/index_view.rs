//! Index Manager: list with sizes, usage and properties; create, hide/unhide and drop.
//!
//! The form's validation and how indexes are described live in
//! `mongo_raabit_core::indexes`; this module lays them out.

use crate::app::{Load, MongoRaabit, TabId};
use crate::egui::{Id, Ui};
use crate::icons::Icon;
use crate::tools::IndexTab;
use crate::widgets;
use crate::writes::READ_ONLY_TIP;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::{
    Banner, Button, Cell, Checkbox, Column, Form, IconButton, Severity, Spinner, Table, Tag,
    TextField, TooltipExt,
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

const KEYS_WIDTH: f32 = 360.0;
const TTL_WIDTH: f32 = 120.0;
const KEYS_TIP: &str = "1 / -1 for ascending / descending, \"text\", \"2dsphere\", \"hashed\"";
const HIDE_TIP: &str = "Hidden indexes are kept up to date but not used by the planner";

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let server = app
        .index_tab_mut(tab_id)
        .map(|t| t.session)
        .and_then(|s| app.session(s))
        .map(|s| s.profile.name.clone())
        .unwrap_or_default();
    let read_only = app
        .index_tab_mut(tab_id)
        .map(|t| t.session)
        .is_some_and(|s| app.session_read_only(s));
    let Some(tab) = app.index_tab_mut(tab_id) else {
        return;
    };
    let IndexTab {
        db,
        collection,
        indexes,
        form,
        form_open,
        error,
        busy,
        confirm_drop,
        ..
    } = tab;
    let mut actions = Vec::new();
    widgets::tool_header(
        ui,
        Icon::Index,
        Tone::Warning,
        &[server.as_str(), db.as_str()],
        collection,
        Tag::new("Indexes").tone(Tone::Warning),
        |ui| {
            if read_only {
                ui.add(Tag::new("Read-only").tone(Tone::Warning));
            }
            if *busy {
                ui.add(Spinner::new().label("Working"));
            }
        },
        |ui| {
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
            if ui.add(IconButton::new(Icon::Refresh, "Refresh")).clicked() {
                actions.push(Action::Refresh);
            }
        },
    );
    if read_only {
        *form_open = false;
    }
    if *form_open || error.is_some() {
        widgets::padded(ui, |ui| {
            if *form_open {
                new_index_form(ui, form, form_open, &mut actions);
            }
            if let Some(err) = &*error {
                Banner::new(Severity::Error, err).show(ui);
            }
        });
    }
    match &*indexes {
        Load::Loading | Load::Idle => widgets::busy(ui, "Loading indexes…"),
        Load::Failed(err) => {
            widgets::padded(ui, |ui| {
                Banner::new(Severity::Error, err).show(ui);
            });
        }
        Load::Ready(list) => {
            widgets::padded(ui, |ui| {
                index_table(ui, tab_id, list, confirm_drop, read_only, &mut actions);
            });
        }
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

/// The "New index" form on an island.
fn new_index_form(
    ui: &mut Ui,
    form: &mut mongo_raabit_core::indexes::IndexForm,
    open: &mut bool,
    actions: &mut Vec<Action>,
) {
    Surface::Island.frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        Form::new().show(ui, |rows| {
            rows.row("Keys:", |ui| {
                widgets::value_field(ui, "Keys", &mut form.keys, "{ email: 1 }", KEYS_WIDTH, true)
                    .response
                    .tooltip(KEYS_TIP);
            });
            rows.row("Name:", |ui| {
                ui.add(
                    TextField::new(&mut form.name)
                        .name("Name")
                        .placeholder("generated from keys")
                        .width(KEYS_WIDTH),
                );
            });
            rows.row("Options:", |ui| {
                ui.add(Checkbox::new(&mut form.unique, "Unique"));
                ui.add(Checkbox::new(&mut form.sparse, "Sparse"));
                ui.add(Checkbox::new(&mut form.hidden, "Hidden"));
            });
            rows.row("TTL (seconds):", |ui| {
                ui.add(
                    TextField::new(&mut form.ttl)
                        .name("TTL (seconds)")
                        .placeholder("none")
                        .monospace(true)
                        .width(TTL_WIDTH),
                );
            });
            rows.row("Partial filter:", |ui| {
                widgets::value_field(
                    ui,
                    "Partial filter",
                    &mut form.partial,
                    "{ active: true }",
                    KEYS_WIDTH,
                    true,
                );
            });
            rows.fields(|ui| {
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
    });
}

/// Column widths and the selected index of one table, kept between frames.
#[derive(Clone, Default)]
struct TableMemory {
    columns: Vec<Column>,
    selected: Option<String>,
}

/// The totals line, the actions on the selected index and the table of indexes.
fn index_table(
    ui: &mut Ui,
    tab_id: TabId,
    list: &[IndexInfo],
    confirm: &mut Option<String>,
    read_only: bool,
    actions: &mut Vec<Action>,
) {
    let mem_id = Id::new(("raabit-indexes", tab_id, "memory"));
    let mut mem: TableMemory = ui.data(|d| d.get_temp(mem_id)).unwrap_or_default();
    if mem.columns.is_empty() {
        let width = (ui.available_width() - size::CONTENT_INSET * 2.0).max(480.0);
        mem.columns = vec![
            Column::new("Name", (width * 0.20).round()).mono(),
            Column::new("Keys", (width * 0.26).round()).mono(),
            Column::new("Properties", (width * 0.28).round()),
            Column::new("Size", (width * 0.10).round()).right(),
            Column::new("Usage", (width * 0.16).round()).right(),
        ];
    }
    let mut position = mem
        .selected
        .as_ref()
        .and_then(|name| list.iter().position(|i| &i.name == name));
    let selected = position.map(|p| &list[p]);

    ui.horizontal(|ui| {
        let (count, bytes) = indexes::totals(list);
        widgets::label(
            ui,
            Tone::Secondary,
            format!("{count} indexes · {}", format::bytes(bytes)),
        );
        ui.with_layout(
            crate::egui::Layout::right_to_left(crate::egui::Align::Center),
            |ui| {
                let target = selected.filter(|i| i.name != PRIMARY_INDEX && !read_only);
                let confirming =
                    target.is_some_and(|i| confirm.as_deref() == Some(i.name.as_str()));
                if confirming {
                    let drop = Button::danger("Confirm drop").small(true);
                    if ui.add(drop).clicked()
                        && let Some(index) = target
                    {
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
                    *confirm = target.map(|i| i.name.clone());
                }
                let hidden = target.is_some_and(|i| i.hidden);
                let label = if hidden { "Unhide" } else { "Hide" };
                if ui
                    .add(
                        Button::secondary(label)
                            .small(true)
                            .enabled(target.is_some()),
                    )
                    .tooltip(if read_only { READ_ONLY_TIP } else { HIDE_TIP })
                    .clicked()
                    && let Some(index) = target
                {
                    actions.push(Action::Hide(index.name.clone(), !index.hidden));
                }
            },
        );
    });

    let before = position;
    let height = (ui.available_height() - size::ROW_HEIGHT).max(size::ROW_HEIGHT * 4.0);
    Table::new(
        ("raabit-indexes", tab_id),
        &mut mem.columns,
        list.len(),
        &mut position,
    )
    .name("Indexes")
    .height(height)
    .show(ui, |row, column| {
        let index = &list[row];
        let cell = match column {
            0 => Cell::new(&index.name).icon(Icon::Index),
            1 => Cell::new(indexes::keys_label(index)),
            2 => Cell::new(indexes::properties_label(index)),
            3 => Cell::new(index.size.map(format::bytes).unwrap_or_default()),
            _ => Cell::new(indexes::usage_label(index.ops)),
        };
        if index.hidden {
            cell.tone(Tone::Muted)
        } else if column == 0 {
            cell.tone(Tone::Strong)
        } else {
            cell.muted()
        }
    });
    if position != before {
        mem.selected = position.map(|p| list[p].name.clone());
        if confirm.as_deref() != mem.selected.as_deref() {
            *confirm = None;
        }
    }
    ui.data_mut(|d| d.insert_temp(mem_id, mem));
}
