//! Modal dialogs of the collection tab: document editor, delete confirmation, explain
//! plan and query code.

use crate::app::{Load, MongoRaabit, TabId};
use crate::egui::Ui;
use crate::icons::Icon;
use crate::widgets;
use archipel_sdk::kit::components::code::{ExtendedJson, JavaScript};
use archipel_sdk::kit::components::{
    Banner, Cell, Checkbox, CodeEditor, CodeView, Column, Dialog as KitDialog, DialogAction,
    Dropdown, Severity, Spinner, Tag, TreeRow, TreeTable,
};
use archipel_sdk::kit::tokens::size;
use archipel_sdk::kit::{Glyph, Tone, ValueKind};
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::codegen::{self, CodeRequest, Language};
use mongo_raabit_core::query::{Explain, PlanStage};
use mongo_raabit_core::shell::{self, ShellStyle};
use std::collections::HashSet;

/// Width of the document dialog.
const DOCUMENT_WIDTH: f32 = 760.0;

/// The share of the window a dialog may take (the kit's default), and the shortest a code
/// view gets.
const DIALOG_SHARE: f32 = 0.8;
const MIN_VIEW_HEIGHT: f32 = 120.0;

/// Height of a code view that fills a dialog without making its body scroll: `max` in a
/// tall window, less when the window is short. `reserve` is what the rest of the dialog
/// (title, buttons, other content) takes.
fn view_height(ctx: &crate::egui::Context, reserve: f32, max: f32) -> f32 {
    (ctx.content_rect().height() * DIALOG_SHARE - reserve).clamp(MIN_VIEW_HEIGHT, max)
}

#[derive(Debug, Clone, PartialEq)]
pub enum DocumentMode {
    /// Replace the document with this `_id`.
    Edit(Bson),
    Insert,
}

pub struct DocumentEditor {
    pub tab: TabId,
    pub mode: DocumentMode,
    pub text: String,
    pub error: Option<String>,
    pub saving: bool,
}

impl DocumentEditor {
    pub fn edit(tab: TabId, document: &Document) -> Self {
        Self {
            tab,
            mode: DocumentMode::Edit(document.get("_id").cloned().unwrap_or(Bson::Null)),
            text: shell::document_to_shell(document, ShellStyle::Pretty(2)),
            error: None,
            saving: false,
        }
    }

    pub fn insert(tab: TabId, template: Option<&Document>) -> Self {
        Self {
            tab,
            mode: DocumentMode::Insert,
            text: template
                .map(|d| shell::document_to_shell(d, ShellStyle::Pretty(2)))
                .unwrap_or_else(|| "{\n  \n}".into()),
            error: None,
            saving: false,
        }
    }

    /// Parse the editor text; errors carry line and column.
    pub fn parse(&self) -> Result<Document, String> {
        shell::parse_document(&self.text).map_err(|e| {
            let (line, col) = widgets::line_col(&self.text, e.offset);
            format!("{} (line {line}, column {col})", e.message)
        })
    }
}

pub struct ExplainDialog {
    pub title: String,
    pub result: Load<Explain>,
    pub show_raw: bool,
}

pub struct CodeDialog {
    pub title: String,
    pub request: CodeRequest,
    pub shell: String,
    /// `None` shows mongo shell code.
    pub language: Option<Language>,
}

pub struct ConfirmDelete {
    pub tab: TabId,
    pub ids: Vec<Bson>,
    pub busy: bool,
}

pub enum Dialog {
    Document(DocumentEditor),
    Explain(ExplainDialog),
    Code(CodeDialog),
    Delete(ConfirmDelete),
}

/// Render the open dialog, if any.
pub fn show(app: &mut MongoRaabit, ctx: &crate::egui::Context) {
    let Some(mut dialog) = app.dialog.take() else {
        return;
    };
    let keep = match &mut dialog {
        Dialog::Document(editor) => document_editor(app, ctx, editor),
        Dialog::Explain(explain) => explain_dialog(ctx, explain),
        Dialog::Code(code) => code_dialog(ctx, code),
        Dialog::Delete(confirm) => delete_dialog(app, ctx, confirm),
    };
    // A save/delete started above may already have replaced or closed the dialog.
    if keep && app.dialog.is_none() {
        app.dialog = Some(dialog);
    }
}

fn document_editor(
    app: &mut MongoRaabit,
    ctx: &crate::egui::Context,
    editor: &mut DocumentEditor,
) -> bool {
    let title = match &editor.mode {
        DocumentMode::Edit(id) => format!(
            "Edit Document  {}",
            mongo_raabit_core::documents::preview(id)
        ),
        DocumentMode::Insert => "Add Document".to_owned(),
    };
    let saving = editor.saving;
    let editor_height = view_height(ctx, 300.0, 380.0);
    let label = if editor.mode == DocumentMode::Insert {
        "Insert"
    } else {
        "Save"
    };
    let action = KitDialog::new(title)
        .icon(Icon::Edit.glyph())
        .width(DOCUMENT_WIDTH.min(ctx.content_rect().width() - 40.0))
        .content(|ui| {
            widgets::caption(
                ui,
                "Document in mongo shell syntax — ObjectId(), ISODate(), NumberLong() and Extended JSON are accepted",
            );
            ui.add(
                CodeEditor::new(&mut editor.text)
                    .id(crate::egui::Id::new("raabit-doc-text"))
                    .label("Document")
                    .highlighter(&ExtendedJson)
                    .line_numbers(true)
                    .tab_indents(false)
                    .height(editor_height),
            );
            match editor.parse() {
                Ok(doc) => {
                    Banner::new(
                        Severity::Success,
                        format!("Valid document · {} top-level fields", doc.len()),
                    )
                    .show(ui);
                }
                Err(err) => {
                    Banner::new(Severity::Error, err).show(ui);
                }
            }
            if let Some(err) = &editor.error {
                Banner::new(Severity::Error, err.clone()).show(ui);
            }
            if editor.saving {
                ui.add(Spinner::new().label("Saving"));
            }
        })
        .tertiary("Format")
        .secondary("Cancel")
        .primary(label)
        .primary_enabled(!saving)
        .show(ctx);
    match action {
        Some(DialogAction::Tertiary) => {
            if let Ok(doc) = editor.parse() {
                editor.text = shell::document_to_shell(&doc, ShellStyle::Pretty(2));
            }
            true
        }
        Some(DialogAction::Primary) => {
            match editor.parse() {
                Ok(doc) => {
                    editor.saving = true;
                    editor.error = None;
                    app.save_document(editor.tab, editor.mode.clone(), doc);
                }
                Err(err) => editor.error = Some(err),
            }
            true
        }
        Some(_) => false,
        None => true,
    }
}

fn delete_dialog(
    app: &mut MongoRaabit,
    ctx: &crate::egui::Context,
    confirm: &mut ConfirmDelete,
) -> bool {
    let n = confirm.ids.len();
    let title = if n == 1 {
        "Delete document?".to_owned()
    } else {
        format!("Delete {n} documents?")
    };
    let ids: Vec<String> = confirm
        .ids
        .iter()
        .take(5)
        .map(|id| format!("_id: {}", shell::to_shell_string(id, ShellStyle::Compact)))
        .collect();
    let busy = confirm.busy;
    let action = KitDialog::new(title)
        .icon(Glyph::Warning)
        .message("This cannot be undone.")
        .content(|ui| {
            for id in &ids {
                widgets::mono_label(ui, Tone::Secondary, id);
            }
            if n > 5 {
                widgets::label(ui, Tone::Muted, format!("… and {} more", n - 5));
            }
            if busy {
                ui.add(Spinner::new().label("Deleting"));
            }
        })
        .primary("Delete")
        .secondary("Cancel")
        .danger(true)
        .primary_enabled(!busy)
        .width(460.0)
        .show(ctx);
    match action {
        Some(DialogAction::Primary) => {
            confirm.busy = true;
            app.delete_documents(confirm.tab, confirm.ids.clone());
            true
        }
        Some(_) => false,
        None => true,
    }
}

fn explain_dialog(ctx: &crate::egui::Context, explain: &mut ExplainDialog) -> bool {
    let action = KitDialog::new(explain.title.as_str())
        .icon(Icon::Explain.glyph())
        .width(980.0_f32.min(ctx.content_rect().width() - 40.0))
        .secondary("Close")
        .content(|ui| match &explain.result {
            Load::Idle | Load::Loading => {
                ui.horizontal(|ui| {
                    ui.add(Spinner::new().label("Running explain"));
                    widgets::label(ui, Tone::Secondary, "Running explain…");
                });
            }
            Load::Failed(err) => {
                Banner::new(Severity::Error, err.clone()).show(ui);
            }
            Load::Ready(plan) => {
                ui.horizontal_wrapped(|ui| {
                    ui.add(Tag::new(format!("Returned {}", plan.n_returned)));
                    let wasteful = plan.docs_examined > plan.n_returned.max(1) * 10;
                    ui.add(
                        Tag::new(format!("Docs examined {}", plan.docs_examined)).tone(
                            if wasteful {
                                Tone::Warning
                            } else {
                                Tone::Default
                            },
                        ),
                    );
                    ui.add(Tag::new(format!("Keys examined {}", plan.keys_examined)));
                    ui.add(Tag::new(format!("Execution {} ms", plan.execution_ms)));
                    if plan.indexes_used.is_empty() {
                        ui.add(Tag::new("Index none").tone(Tone::Warning));
                    } else {
                        ui.add(
                            Tag::new(format!("Index {}", plan.indexes_used.join(", ")))
                                .tone(Tone::Success),
                        );
                    }
                });
                if plan.root.has_collection_scan() {
                    Banner::new(
                        Severity::Warning,
                        "The query scans the whole collection (COLLSCAN). An index on the filtered fields would avoid it.",
                    )
                    .show(ui);
                }
                widgets::caption(ui, "WINNING PLAN");
                plan_tree(ui, &plan.root);
                ui.add(Checkbox::new(
                    &mut explain.show_raw,
                    "Show raw explain output",
                ));
                if explain.show_raw {
                    let raw = shell::document_to_shell(&plan.raw, ShellStyle::Pretty(2));
                    CodeView::new(&raw)
                        .id(crate::egui::Id::new("raabit-explain-raw"))
                        .label("Raw explain output")
                        .highlighter(&ExtendedJson)
                        .folding(true)
                        .height(200.0)
                        .show(ui);
                }
            }
        })
        .show(ctx);
    action.is_none()
}

/// One stage of the plan, flattened for the tree table.
struct PlanRow<'a> {
    stage: &'a PlanStage,
    depth: usize,
    key: String,
}

fn flatten<'a>(
    stage: &'a PlanStage,
    depth: usize,
    key: String,
    collapsed: &HashSet<String>,
    out: &mut Vec<PlanRow<'a>>,
) {
    let open = !collapsed.contains(&key);
    out.push(PlanRow {
        stage,
        depth,
        key: key.clone(),
    });
    if open {
        for (i, child) in stage.children.iter().enumerate() {
            flatten(child, depth + 1, format!("{key}/{i}"), collapsed, out);
        }
    }
}

fn count(n: Option<u64>) -> String {
    n.map(|n| n.to_string()).unwrap_or_else(|| "—".into())
}

/// The stage tree with per-stage counts. A collection scan is flagged in its row.
fn plan_tree(ui: &mut Ui, root: &PlanStage) {
    let id = crate::egui::Id::new("raabit-explain-plan");
    let mut collapsed: HashSet<String> = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut columns: Vec<Column> = ui.data(|d| d.get_temp(id.with("cols"))).unwrap_or_else(|| {
        vec![
            Column::new("Stage", 280.0),
            Column::new("Index", 200.0).mono(),
            Column::new("Returned", 100.0).right(),
            Column::new("Docs", 100.0).right(),
            Column::new("Keys", 100.0).right(),
            Column::new("Time", 100.0).right(),
        ]
    });
    let mut rows = Vec::new();
    flatten(root, 0, "0".into(), &collapsed, &mut rows);
    let kit_rows: Vec<TreeRow> = rows
        .iter()
        .map(|r| {
            let mut row = TreeRow::new(r.stage.stage.as_str(), r.depth);
            if !r.stage.children.is_empty() {
                row = row.parent(!collapsed.contains(&r.key));
            }
            match r.stage.stage.as_str() {
                "COLLSCAN" => row.icon(Glyph::Warning),
                "IXSCAN" | "EXPRESS_IXSCAN" | "IDHACK" | "EXPRESS_CLUSTERED_IXSCAN" => {
                    row.icon(Icon::Index.kit())
                }
                _ => row,
            }
        })
        .collect();
    let mut selected = None;
    let height = (kit_rows.len() as f32 * size::ROW_HEIGHT + size::HEADER_HEIGHT + 8.0).min(300.0);
    let out = TreeTable::new(id, &mut columns, &kit_rows, &mut selected)
        .name("Winning plan")
        .height(height)
        .show(ui, |row, column| {
            let stage = rows[row].stage;
            let scan = stage.stage == "COLLSCAN";
            match column {
                1 => match (&stage.index_name, scan) {
                    (Some(index), _) => Cell::new(index.as_str()),
                    (None, true) => Cell::new("none (full scan)").tone(Tone::Warning),
                    (None, false) => Cell::new("—").muted(),
                },
                2 => Cell::new(count(stage.n_returned)).value_kind(ValueKind::Number),
                3 => Cell::new(count(stage.docs_examined)).value_kind(ValueKind::Number),
                4 => Cell::new(count(stage.keys_examined)).value_kind(ValueKind::Number),
                _ => Cell::new(stage.time_ms.map_or("—".into(), |ms| format!("{ms} ms"))).muted(),
            }
        });
    if let Some(row) = out.toggled {
        let key = rows[row].key.clone();
        if !collapsed.remove(&key) {
            collapsed.insert(key);
        }
    }
    ui.data_mut(|d| {
        d.insert_temp(id, collapsed);
        d.insert_temp(id.with("cols"), columns);
    });
}

/// A language choice of the code dialog: mongo shell or a driver language.
#[derive(Clone, Copy, PartialEq)]
struct Lang(Option<Language>);

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.map_or("mongo shell", Language::label))
    }
}

/// The code the dialog shows for its language.
fn code_text(code: &CodeDialog) -> String {
    match code.language {
        None => code.shell.clone(),
        Some(language) => codegen::generate(language, &code.request),
    }
}

fn code_dialog(ctx: &crate::egui::Context, code: &mut CodeDialog) -> bool {
    let title = code.title.clone();
    let height = view_height(ctx, 280.0, 420.0);
    let action = KitDialog::new(title)
        .icon(Icon::Code.glyph())
        .width(900.0_f32.min(ctx.content_rect().width() - 40.0))
        .content(|ui| {
            let mut options = vec![Lang(None)];
            options.extend(Language::ALL.map(|l| Lang(Some(l))));
            let mut chosen = Lang(code.language);
            ui.add(
                Dropdown::new("raabit-code-language", &mut chosen, options)
                    .label("Language")
                    .width(200.0),
            );
            code.language = chosen.0;
            let text = code_text(code);
            let revision = code.language.map_or(0, |l| {
                Language::ALL.iter().position(|x| *x == l).unwrap_or(0) as u64 + 1
            });
            CodeView::new(&text)
                .id(crate::egui::Id::new("raabit-code-view"))
                .label("Query code")
                .highlighter(&JavaScript)
                .line_numbers(true)
                .revision(revision)
                .height(height)
                .show(ui);
            widgets::label(ui, Tone::Muted, format!("{} lines", text.lines().count()));
        })
        .secondary("Close")
        .primary("Copy")
        .show(ctx);
    match action {
        // Copy leaves the dialog open, to copy another language too.
        Some(DialogAction::Primary) => {
            ctx.copy_text(code_text(code));
            true
        }
        Some(_) => false,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongo_raabit_core::bson::doc;

    #[test]
    fn editor_reports_line_and_column() {
        let mut editor = DocumentEditor::insert(1, None);
        editor.text = "{\n  a: 1,\n  b: ,\n}".into();
        let err = editor.parse().unwrap_err();
        assert!(err.ends_with("(line 3, column 6)"), "{err}");
        let editor = DocumentEditor::edit(1, &doc! {"_id": 5, "x": "y"});
        assert_eq!(editor.mode, DocumentMode::Edit(Bson::Int32(5)));
        assert_eq!(editor.parse().unwrap(), doc! {"_id": 5, "x": "y"});
    }
}
