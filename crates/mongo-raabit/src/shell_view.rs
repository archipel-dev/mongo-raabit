//! Shell mode of the Query tab (IntelliShell equivalent): script editor with completion, console output
//! and a result tree. Uses the installed mongosh, or the built-in evaluator.
//!
//! Completion sites, result documents and the run summaries are computed in
//! `mongo_raabit_core`; this module lays them out.

use crate::app::{MongoRaabit, TabId};
use crate::egui::text::CCursorRange;
use crate::egui::{Id, RichText, Ui};
use crate::icons::{self, Icon};
use crate::results;
use crate::tools::{ShellEngine, ShellEntry, ShellTab, shell_documents};
use crate::widgets;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::code::{ExtendedJson, JavaScript};
use archipel_sdk::kit::components::{
    Button, Checkbox, CodeEditor, CodeView, Completion, Completions, EmptyState, IconButton,
    Spinner, SplitPane, Tag, Tooltip, TooltipExt,
};
use archipel_sdk::kit::parts::scroll_vertical;
use archipel_sdk::kit::tokens::size;
use mongo_raabit_core::format;
use mongo_raabit_core::shell::{char_slice, script_summary};
use mongo_raabit_core::shell_eval;
use std::collections::HashSet;

/// Default share of the height the editor gets.
const EDITOR_RATIO: f32 = 0.38;
const EDITOR_MIN_HEIGHT: f32 = size::CODE_LINE_HEIGHT * 5.0;
const CONSOLE_MIN_HEIGHT: f32 = size::CODE_LINE_HEIGHT * 4.0;
/// Default share of the width the console gets beside the result tree.
const CONSOLE_RATIO: f32 = 0.52;
const PANE_MIN_WIDTH: f32 = 200.0;
/// Characters of a script's first line shown in the console.
const SUMMARY_CHARS: usize = 90;
/// Most lines of one run's output shown before its block scrolls.
const OUTPUT_LINES: usize = 14;
const HINT: &str = "Ctrl+Enter runs the script or the selection · Ctrl+Space completes";
const BUILTIN_TIP: &str = "Evaluate with the built-in shell even though mongosh is installed";
const BUILTIN_ENGINE_TIP: &str = "Evaluates show/use and db.<collection> commands with the driver. Install mongosh for full JavaScript.";

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let Some(tab) = app.shell_tab_mut(tab_id) else {
        return;
    };
    let ShellTab {
        db,
        script,
        engine,
        force_builtin,
        history,
        running,
        open,
        collections,
        ..
    } = tab;
    let mut run: Option<Option<String>> = None;
    let builtin = *force_builtin;
    widgets::tool_header(
        ui,
        Icon::Shell,
        Tone::Accent,
        &[],
        db,
        Tag::new("Shell").tone(Tone::Accent),
        |ui| engine_tag(ui, engine, builtin),
        |ui| {
            if *running {
                ui.add(Spinner::new().label("Running"));
            } else if ui
                .add(Button::primary("Run").icon(Icon::Play))
                .tooltip_with(Tooltip::new("Run script").shortcut("Ctrl+Enter"))
                .clicked()
            {
                run = Some(None);
            }
            if matches!(engine, ShellEngine::Mongosh { .. }) {
                ui.add(Checkbox::new(force_builtin, "Built-in"))
                    .tooltip(BUILTIN_TIP);
            }
            if ui
                .add(IconButton::new(Icon::Trash, "Clear output"))
                .tooltip("Clear output")
                .clicked()
            {
                history.clear();
            }
        },
    );
    let ratio_id = ui.id().with(("raabit-shell-ratio", tab_id));
    let mut ratio = ui
        .data(|d| d.get_temp::<f32>(ratio_id))
        .unwrap_or(EDITOR_RATIO);
    SplitPane::vertical(("raabit-shell-body", tab_id), &mut ratio)
        .default_ratio(EDITOR_RATIO)
        .min_sizes(EDITOR_MIN_HEIGHT, CONSOLE_MIN_HEIGHT)
        .label("Resize console")
        .show(
            ui,
            |ui| {
                widgets::padded(ui, |ui| {
                    if let Some(selection) = editor(ui, tab_id, script, collections) {
                        run = Some(selection);
                    }
                });
            },
            |ui| output(ui, tab_id, history, open),
        );
    ui.data_mut(|d| d.insert_temp(ratio_id, ratio));
    if let Some(selection) = run {
        app.run_shell(tab_id, selection);
    }
}

/// Which engine runs the scripts.
fn engine_tag(ui: &mut Ui, engine: &ShellEngine, force_builtin: bool) {
    match engine {
        ShellEngine::Detecting => {
            ui.add(Spinner::new().label("Detecting mongosh"));
        }
        ShellEngine::Mongosh { version, .. } if !force_builtin => {
            ui.add(Tag::new(format!("mongosh {version}")).tone(Tone::Accent));
        }
        ShellEngine::Mongosh { .. } | ShellEngine::Builtin => {
            ui.add(Tag::new("built-in shell").tone(Tone::Warning))
                .tooltip(BUILTIN_ENGINE_TIP);
        }
    }
}

/// The script editor with completion. Returns what the user asked to run with Ctrl+Enter:
/// the selection, or `None` for the whole script.
fn editor(
    ui: &mut Ui,
    tab_id: TabId,
    script: &mut String,
    collections: &[String],
) -> Option<Option<String>> {
    let id = Id::new(("raabit-shell", tab_id));
    let completer = |text: &str, caret: usize| {
        let site = shell_eval::completions_at(text, caret, collections)?;
        Some(Completions {
            start: site.start,
            end: caret,
            items: site
                .items
                .into_iter()
                .map(|item| Completion::new(item.as_str(), item.as_str()))
                .collect(),
        })
    };
    let hint = crate::egui::vec2(0.0, size::COMMENT_LINE_HEIGHT + size::SECTION_SPACING);
    let height = (ui.available_height() - hint.y).max(size::CODE_LINE_HEIGHT * 3.0);
    ui.add(
        CodeEditor::new(script)
            .label("Script")
            .highlighter(&JavaScript)
            .completer(&completer)
            .id(id)
            .height(height),
    );
    widgets::caption(ui, HINT);
    let focused = ui.memory(|m| m.has_focus(id));
    if !focused || !ui.input(|i| i.modifiers.command && i.key_pressed(crate::egui::Key::Enter)) {
        return None;
    }
    // Run the selection when there is one, otherwise the whole script.
    let selection = crate::egui::TextEdit::load_state(ui.ctx(), id)
        .and_then(|state| state.cursor.char_range())
        .filter(|range: &CCursorRange| !range.is_empty())
        .map(|range| {
            let [from, to] = range.sorted_cursors();
            char_slice(script, from.index, to.index)
        });
    Some(selection)
}

/// The console, and beside it the tree of the last run's value.
fn output(ui: &mut Ui, tab_id: TabId, history: &[ShellEntry], open: &mut HashSet<String>) {
    let last_value = history.last().and_then(|e| e.value.clone());
    let Some(value) = last_value else {
        console(ui, tab_id, history);
        return;
    };
    let ratio_id = ui.id().with(("raabit-shell-io-ratio", tab_id));
    let mut ratio = ui
        .data(|d| d.get_temp::<f32>(ratio_id))
        .unwrap_or(CONSOLE_RATIO);
    SplitPane::horizontal(("raabit-shell-io", tab_id), &mut ratio)
        .default_ratio(CONSOLE_RATIO)
        .min_size(PANE_MIN_WIDTH)
        .label("Resize result")
        .show(
            ui,
            |ui| console(ui, tab_id, history),
            |ui| {
                let docs = shell_documents(&value);
                widgets::padded(ui, |ui| {
                    ui.horizontal(|ui| {
                        widgets::title(ui, "Result");
                        ui.add(Tag::new(format!("{} items", docs.len())).tone(Tone::Accent));
                    });
                });
                if let Some(results::TreeEvent::Toggle(key)) = results::tree(
                    ui,
                    results::TreeState {
                        id: Id::new(("raabit-shell-tree", tab_id)),
                        open,
                        selected: None,
                    },
                    &docs,
                    1,
                ) && !open.remove(&key)
                {
                    open.insert(key);
                }
            },
        );
    ui.data_mut(|d| d.insert_temp(ratio_id, ratio));
}

/// What every run printed, oldest first, following the newest.
fn console(ui: &mut Ui, tab_id: TabId, history: &[ShellEntry]) {
    if history.is_empty() {
        EmptyState::new("No output yet")
            .icon(Icon::Shell)
            .hint("Run a script with Ctrl+Enter.")
            .show(ui);
        return;
    }
    scroll_vertical()
        .id_salt(("raabit-shell-console", tab_id))
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            widgets::padded(ui, |ui| {
                for (index, entry) in history.iter().enumerate() {
                    entry_block(ui, tab_id, index, entry);
                }
            });
        });
}

/// One run: the script's first line, engine and time, and its output.
fn entry_block(ui: &mut Ui, tab_id: TabId, index: usize, entry: &ShellEntry) {
    let strong = Tone::Strong.resolve(ui.ctx());
    ui.horizontal(|ui| {
        widgets::label(ui, Tone::Accent, "›");
        ui.label(
            RichText::new(script_summary(&entry.script, SUMMARY_CHARS))
                .monospace()
                .color(strong),
        );
        ui.with_layout(
            crate::egui::Layout::right_to_left(crate::egui::Align::Center),
            |ui| {
                widgets::label(
                    ui,
                    Tone::Muted,
                    format!("{} · {}", entry.engine, format::duration(entry.elapsed)),
                );
                if !entry.ok {
                    icons::icon(ui, Icon::Warning, Tone::Error.resolve(ui.ctx()));
                }
            },
        );
    });
    if entry.ok {
        let lines = entry.output.lines().count().clamp(1, OUTPUT_LINES);
        CodeView::new(&entry.output)
            .label("Output")
            .highlighter(&ExtendedJson)
            .wrap(true)
            .id(Id::new(("raabit-shell-output", tab_id, index)))
            .revision(crate::egui::util::hash(&entry.output))
            .height(lines as f32 * size::CODE_LINE_HEIGHT + size::CODE_PADDING * 2.0)
            .show(ui);
    } else {
        let error = Tone::Error.resolve(ui.ctx());
        ui.add(
            crate::egui::Label::new(RichText::new(&entry.output).monospace().color(error))
                .selectable(true)
                .wrap(),
        );
    }
    ui.add_space(size::SECTION_SPACING);
}
