//! Shell mode of the Query tab (IntelliShell equivalent): script editor with completion, console output
//! and a result tree. Uses the installed mongosh, or the built-in evaluator.

use crate::app::{MongoRaabit, TabId};
use crate::icons::{self, Icon};
use crate::results;
use crate::style::{self, Palette};
use crate::tools::{ShellEngine, shell_documents};
use crate::values;
use crate::widgets;
use egui::text::{CCursor, CCursorRange};
use egui::{CornerRadius, FontId, RichText, Stroke, Ui, vec2};
use mongo_raabit_core::shell_eval;

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let p = Palette::of(ui);
    let Some(tab) = app.shell_tab_mut(tab_id) else {
        return;
    };
    let mut run: Option<Option<String>> = None;
    egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 14, top: 10, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            icons::icon(ui, Icon::Shell, p.leaf);
            ui.label(RichText::new(&tab.db).size(15.0).strong().color(p.text_strong));
            match &tab.engine {
                ShellEngine::Detecting => {
                    ui.spinner();
                }
                ShellEngine::Mongosh { version, .. } if !tab.force_builtin => {
                    widgets::badge(ui, &format!("mongosh {version}"), p.leaf);
                }
                ShellEngine::Mongosh { .. } | ShellEngine::Builtin => {
                    widgets::badge(ui, "built-in shell", p.warning).on_hover_text(
                        "Evaluates show/use and db.<collection> commands with the driver. Install mongosh for full JavaScript.",
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if tab.running {
                    ui.spinner();
                } else if widgets::primary_button(ui, Some(Icon::Play), "Run").on_hover_text("Run script (Ctrl+Enter)").clicked() {
                    run = Some(None);
                }
                if matches!(tab.engine, ShellEngine::Mongosh { .. }) {
                    ui.checkbox(&mut tab.force_builtin, "Built-in");
                }
                if widgets::icon_button(ui, Icon::Trash, "Clear output").clicked() {
                    tab.history.clear();
                }
            });
        });
    });
    widgets::separator(ui);
    let editor_height = (ui.available_height() * 0.34).clamp(120.0, 300.0);
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 4,
        })
        .show(ui, |ui| {
            let response = egui::ScrollArea::vertical()
                .id_salt(("raabit-shell-editor", tab_id))
                .max_height(editor_height)
                .show(ui, |ui| {
                    widgets::code_editor(ui, &mut tab.script, 8, ("raabit-shell", tab_id))
                })
                .inner;
            // Completion for the token before the cursor.
            let ctx = ui.ctx().clone();
            let mut state = egui::TextEdit::load_state(&ctx, response.id);
            let cursor = state
                .as_ref()
                .and_then(|s| s.cursor.char_range())
                .map(|r| r.primary.index)
                .unwrap_or(tab.script.chars().count());
            let byte = tab
                .script
                .char_indices()
                .nth(cursor)
                .map_or(tab.script.len(), |(i, _)| i);
            let prefix = &tab.script[..byte];
            let completions = if response.has_focus() {
                shell_eval::complete(prefix, &tab.collections)
            } else {
                Vec::new()
            };
            let mut accept: Option<String> = None;
            if response.has_focus()
                && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
            {
                // Run the selection when there is one, otherwise the whole script.
                let selection = state
                    .as_ref()
                    .and_then(|s| s.cursor.char_range())
                    .filter(|r| !r.is_empty())
                    .map(|r| {
                        let [a, b] = r.sorted_cursors();
                        tab.script
                            .chars()
                            .skip(a.index)
                            .take(b.index - a.index)
                            .collect::<String>()
                    });
                run = Some(selection);
            }
            if !completions.is_empty()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space))
            {
                accept = completions.first().cloned();
            }
            ui.horizontal_wrapped(|ui| {
                ui.set_min_height(24.0);
                if completions.is_empty() {
                    widgets::caption(
                        ui,
                        "Ctrl+Enter runs the script or the selection · type db. for completions",
                    );
                } else {
                    widgets::caption(ui, "Ctrl+Space");
                    for completion in &completions {
                        let chip = egui::Button::new(
                            RichText::new(completion)
                                .font(FontId::monospace(12.0))
                                .color(p.text),
                        )
                        .fill(p.raised)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(12));
                        if ui.add(chip).clicked() {
                            accept = Some(completion.clone());
                        }
                    }
                }
            });
            if let Some(completion) = accept {
                // Replace the partial word before the cursor with the completion.
                let partial = prefix
                    .chars()
                    .rev()
                    .take_while(|c| {
                        c.is_alphanumeric()
                            || *c == '_'
                            || *c == ' ' && prefix.trim_start().starts_with("sh")
                    })
                    .count();
                let partial = if prefix.ends_with('.') {
                    0
                } else {
                    partial.min(prefix.chars().count())
                };
                let start_char = cursor - partial;
                let start_byte = tab
                    .script
                    .char_indices()
                    .nth(start_char)
                    .map_or(tab.script.len(), |(i, _)| i);
                tab.script.replace_range(start_byte..byte, &completion);
                let new_cursor = start_char + completion.chars().count();
                if let Some(state) = state.as_mut() {
                    state
                        .cursor
                        .set_char_range(Some(CCursorRange::one(CCursor::new(new_cursor))));
                }
                if let Some(state) = state {
                    state.store(&ctx, response.id);
                }
                response.request_focus();
            }
        });
    widgets::separator(ui);
    let last_value = tab.history.last().and_then(|e| e.value.clone());
    let height = ui.available_height();
    ui.horizontal_top(|ui| {
        let console_width = if last_value.is_some() {
            ui.available_width() * 0.48
        } else {
            ui.available_width()
        };
        ui.vertical(|ui| {
            ui.set_width(console_width);
            ui.set_height(height);
            egui::ScrollArea::vertical()
                .id_salt(("raabit-shell-console", tab_id))
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            if tab.history.is_empty() {
                                widgets::caption(ui, "Output appears here.");
                            }
                            for entry in &tab.history {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("›")
                                            .font(FontId::monospace(13.0))
                                            .color(p.accent_text),
                                    );
                                    let first = entry.script.lines().next().unwrap_or_default();
                                    let more = if entry.script.lines().count() > 1 {
                                        " …"
                                    } else {
                                        ""
                                    };
                                    ui.label(
                                        RichText::new(format!(
                                            "{}{more}",
                                            values::truncate(first, 90)
                                        ))
                                        .font(FontId::monospace(12.5))
                                        .color(p.text_strong),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} · {}",
                                                    entry.engine,
                                                    style::duration(entry.elapsed)
                                                ))
                                                .size(11.0)
                                                .color(p.text_info),
                                            );
                                            if !entry.ok {
                                                icons::icon(ui, Icon::Warning, p.error);
                                            }
                                        },
                                    );
                                });
                                let mut job = egui::text::LayoutJob::default();
                                if entry.ok {
                                    values::highlight_into(
                                        &mut job,
                                        &entry.output,
                                        &p,
                                        FontId::monospace(12.0),
                                    );
                                } else {
                                    job.append(
                                        &entry.output,
                                        0.0,
                                        egui::TextFormat {
                                            font_id: FontId::monospace(12.0),
                                            color: p.error,
                                            ..Default::default()
                                        },
                                    );
                                }
                                ui.add(egui::Label::new(job).selectable(true).wrap());
                                ui.add_space(10.0);
                            }
                        });
                });
        });
        if let Some(value) = &last_value {
            let (sep, _) = ui.allocate_exact_size(vec2(1.0, height), egui::Sense::hover());
            ui.painter().rect_filled(sep, 0.0, p.separator);
            ui.vertical(|ui| {
                ui.set_height(height);
                let docs = shell_documents(value);
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 12,
                        top: 8,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Result").strong());
                            widgets::badge(ui, &format!("{} items", docs.len()), p.accent_text);
                        });
                    });
                if let Some(results::TreeEvent::Toggle(key)) = results::tree(
                    ui,
                    results::TreeState {
                        id: egui::Id::new(("raabit-shell-tree", tab_id)),
                        open: &tab.open,
                        selected: None,
                    },
                    &docs,
                    1,
                ) && !tab.open.remove(&key)
                {
                    tab.open.insert(key);
                }
            });
        }
    });
    if let Some(selection) = run {
        app.run_shell(tab_id, selection);
    }
}
