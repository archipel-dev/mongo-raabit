//! Modal dialogs of the collection tab: document editor, delete confirmation, explain
//! plan and query code.

use crate::app::{Load, MongoRaabit, TabId};
use crate::icons::{self, Icon};
use crate::style::{self, Palette};
use crate::widgets;
use egui::{CornerRadius, FontId, RichText, Stroke, Ui, vec2};
use mongo_raabit_core::bson::{Bson, Document};
use mongo_raabit_core::codegen::{self, CodeRequest, Language};
use mongo_raabit_core::query::{Explain, PlanStage};
use mongo_raabit_core::shell::{self, ShellStyle};

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
pub fn show(app: &mut MongoRaabit, ctx: &egui::Context) {
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
    ctx: &egui::Context,
    editor: &mut DocumentEditor,
) -> bool {
    let p = Palette::for_dark(ctx.style().visuals.dark_mode);
    let title = match &editor.mode {
        DocumentMode::Edit(id) => format!("Edit Document  {}", crate::values::preview(id)),
        DocumentMode::Insert => "Add Document".to_owned(),
    };
    let mut save = false;
    let mut cancel = false;
    let open = widgets::modal(
        ctx,
        "raabit-doc-editor",
        &title,
        Icon::Edit,
        vec2(760.0, 640.0),
        |ui| {
            widgets::caption(
                ui,
                "Document in mongo shell syntax — ObjectId(), ISODate(), NumberLong() and Extended JSON are accepted",
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(440.0)
                .show(ui, |ui| {
                    widgets::code_editor(ui, &mut editor.text, 22, "raabit-doc-text");
                });
            ui.add_space(8.0);
            match editor.parse() {
                Ok(doc) => {
                    ui.horizontal(|ui| {
                        icons::icon(ui, Icon::Check, p.success);
                        ui.label(
                            RichText::new(format!(
                                "Valid document · {} top-level fields",
                                doc.len()
                            ))
                            .color(p.success),
                        );
                    });
                }
                Err(err) => {
                    ui.horizontal(|ui| {
                        icons::icon(ui, Icon::Warning, p.error);
                        ui.label(RichText::new(err).color(p.error));
                    });
                }
            }
            if let Some(err) = &editor.error {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(err).color(p.error)).wrap());
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::secondary_button(ui, Some(Icon::Json), "Format").clicked()
                    && let Ok(doc) = editor.parse()
                {
                    editor.text = shell::document_to_shell(&doc, ShellStyle::Pretty(2));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if editor.mode == DocumentMode::Insert {
                        "Insert"
                    } else {
                        "Save"
                    };
                    if editor.saving {
                        ui.spinner();
                    } else if widgets::primary_button(ui, Some(Icon::Check), label).clicked()
                        || ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
                    {
                        save = true;
                    }
                    if widgets::secondary_button(ui, None, "Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        },
    );
    if save {
        match editor.parse() {
            Ok(doc) => {
                editor.saving = true;
                editor.error = None;
                app.save_document(editor.tab, editor.mode.clone(), doc);
            }
            Err(err) => editor.error = Some(err),
        }
    }
    open && !cancel
}

fn delete_dialog(app: &mut MongoRaabit, ctx: &egui::Context, confirm: &mut ConfirmDelete) -> bool {
    let p = Palette::for_dark(ctx.style().visuals.dark_mode);
    let mut cancel = false;
    let mut delete = false;
    let n = confirm.ids.len();
    let title = if n == 1 {
        "Delete document?".to_owned()
    } else {
        format!("Delete {n} documents?")
    };
    let open = widgets::modal(
        ctx,
        "raabit-delete",
        &title,
        Icon::Trash,
        vec2(460.0, 260.0),
        |ui| {
            ui.label("This cannot be undone.");
            ui.add_space(6.0);
            for id in confirm.ids.iter().take(5) {
                ui.label(
                    RichText::new(format!(
                        "_id: {}",
                        shell::to_shell_string(id, ShellStyle::Compact)
                    ))
                    .font(FontId::monospace(12.0))
                    .color(p.text_secondary),
                );
            }
            if n > 5 {
                ui.label(RichText::new(format!("… and {} more", n - 5)).color(p.text_info));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if confirm.busy {
                        ui.spinner();
                    } else {
                        let r =
                            egui::Button::new(RichText::new("Delete").color(egui::Color32::WHITE))
                                .fill(p.error)
                                .corner_radius(CornerRadius::same(7))
                                .min_size(vec2(84.0, 28.0));
                        delete = ui.add(r).clicked();
                    }
                    cancel = widgets::secondary_button(ui, None, "Cancel").clicked();
                });
            });
        },
    );
    if delete {
        confirm.busy = true;
        app.delete_documents(confirm.tab, confirm.ids.clone());
    }
    open && !cancel
}

fn explain_dialog(ctx: &egui::Context, explain: &mut ExplainDialog) -> bool {
    let p = Palette::for_dark(ctx.style().visuals.dark_mode);
    widgets::modal(
        ctx,
        "raabit-explain",
        &explain.title,
        Icon::Explain,
        vec2(980.0, 640.0),
        |ui| match &explain.result {
            Load::Idle | Load::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Running explain…");
                });
            }
            Load::Failed(err) => {
                ui.add(egui::Label::new(RichText::new(err).color(p.error)).wrap());
            }
            Load::Ready(plan) => {
                ui.horizontal(|ui| {
                    stat(
                        ui,
                        &p,
                        "Returned",
                        &plan.n_returned.to_string(),
                        p.text_strong,
                    );
                    stat(
                        ui,
                        &p,
                        "Docs examined",
                        &plan.docs_examined.to_string(),
                        if plan.docs_examined > plan.n_returned.max(1) * 10 {
                            p.warning
                        } else {
                            p.text_strong
                        },
                    );
                    stat(
                        ui,
                        &p,
                        "Keys examined",
                        &plan.keys_examined.to_string(),
                        p.text_strong,
                    );
                    stat(
                        ui,
                        &p,
                        "Execution",
                        &format!("{} ms", plan.execution_ms),
                        p.text_strong,
                    );
                    let (index, color) = if plan.indexes_used.is_empty() {
                        ("none".to_owned(), p.warning)
                    } else {
                        (plan.indexes_used.join(", "), p.success)
                    };
                    stat(ui, &p, "Index", &index, color);
                });
                ui.add_space(10.0);
                if plan.root.has_collection_scan() {
                    egui::Frame::new()
                    .fill(p.warning.gamma_multiply(0.14))
                    .corner_radius(CornerRadius::same(7))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            icons::icon(ui, Icon::Warning, p.warning);
                            ui.label(RichText::new("The query scans the whole collection (COLLSCAN). An index on the filtered fields would avoid it.").color(p.warning));
                        });
                    });
                    ui.add_space(10.0);
                }
                widgets::caption(ui, "WINNING PLAN — data flows left to right");
                ui.add_space(6.0);
                egui::ScrollArea::both()
                    .max_height(300.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        plan_tree(ui, &p, &plan.root);
                    });
                ui.add_space(8.0);
                ui.checkbox(&mut explain.show_raw, "Show raw explain output");
                if explain.show_raw {
                    let mut raw = shell::document_to_shell(&plan.raw, ShellStyle::Pretty(2));
                    egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut raw)
                                    .font(FontId::monospace(11.5))
                                    .desired_width(f32::INFINITY)
                                    .interactive(false),
                            );
                        });
                }
            }
        },
    )
}

fn stat(ui: &mut Ui, p: &Palette, label: &str, value: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_min_width(120.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(11.5).color(p.text_info));
                ui.label(RichText::new(value).size(18.0).strong().color(color));
            });
        });
}

/// Children (inputs) to the left, then an arrow, then the stage card.
fn plan_tree(ui: &mut Ui, p: &Palette, stage: &PlanStage) {
    ui.horizontal(|ui| {
        if !stage.children.is_empty() {
            ui.vertical(|ui| {
                for child in &stage.children {
                    plan_tree(ui, p, child);
                }
            });
            let (rect, _) = ui.allocate_exact_size(vec2(28.0, 20.0), egui::Sense::hover());
            let y = rect.center().y;
            ui.painter().arrow(
                egui::pos2(rect.left() + 2.0, y),
                vec2(22.0, 0.0),
                Stroke::new(1.5_f32, p.text_info),
            );
        }
        let accent = match stage.stage.as_str() {
            "COLLSCAN" => p.warning,
            "IXSCAN" | "EXPRESS_IXSCAN" | "IDHACK" | "EXPRESS_CLUSTERED_IXSCAN" => p.success,
            _ => p.accent_text,
        };
        egui::Frame::new()
            .fill(p.raised)
            .stroke(Stroke::new(1.0_f32, accent.gamma_multiply(0.7)))
            .corner_radius(CornerRadius::same(9))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(150.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&stage.stage).strong().color(accent));
                    if let Some(index) = &stage.index_name {
                        ui.label(
                            RichText::new(format!("index {index}"))
                                .size(11.5)
                                .color(p.text_secondary),
                        );
                    }
                    let mut facts = Vec::new();
                    if let Some(n) = stage.n_returned {
                        facts.push(format!("{n} returned"));
                    }
                    if let Some(n) = stage.docs_examined {
                        facts.push(format!("{n} docs"));
                    }
                    if let Some(n) = stage.keys_examined {
                        facts.push(format!("{n} keys"));
                    }
                    if let Some(ms) = stage.time_ms {
                        facts.push(format!("~{ms} ms"));
                    }
                    if !facts.is_empty() {
                        ui.label(
                            RichText::new(facts.join(" · "))
                                .size(11.5)
                                .color(p.text_info),
                        );
                    }
                });
            });
    });
}

fn code_dialog(ctx: &egui::Context, code: &mut CodeDialog) -> bool {
    let p = Palette::for_dark(ctx.style().visuals.dark_mode);
    widgets::modal(
        ctx,
        "raabit-code",
        &code.title,
        Icon::Code,
        vec2(900.0, 660.0),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let chip = |ui: &mut Ui, selected: bool, label: &str| -> bool {
                    let text = RichText::new(label).color(if selected {
                        p.text_strong
                    } else {
                        p.text_secondary
                    });
                    let button = egui::Button::new(text)
                        .fill(if selected { p.selection } else { p.raised })
                        .stroke(Stroke::new(
                            1.0_f32,
                            if selected { p.accent } else { p.border },
                        ))
                        .corner_radius(CornerRadius::same(14));
                    ui.add(button).clicked()
                };
                if chip(ui, code.language.is_none(), "mongo shell") {
                    code.language = None;
                }
                for language in Language::ALL {
                    if chip(ui, code.language == Some(language), language.label()) {
                        code.language = Some(language);
                    }
                }
            });
            ui.add_space(10.0);
            let text = match code.language {
                None => code.shell.clone(),
                Some(language) => codegen::generate(language, &code.request),
            };
            egui::Frame::new()
                .fill(p.field)
                .stroke(Stroke::new(1.0_f32, p.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .max_height(440.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut job = egui::text::LayoutJob::default();
                            crate::values::highlight_into(
                                &mut job,
                                &text,
                                &p,
                                FontId::monospace(12.5),
                            );
                            ui.add(egui::Label::new(job).selectable(true).extend());
                        });
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} lines", text.lines().count())).color(p.text_info),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, Some(Icon::Copy), "Copy").clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                });
            });
            let _ = style::RADIUS;
        },
    )
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
