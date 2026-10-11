//! Small shared helpers over archipel-kit components.
//!
//! Buttons, icon buttons, segmented controls, tags, separators and dialogs are used straight
//! from `archipel_sdk::kit::components`; what remains here is layout glue (captions, form
//! rows, the query-bar field) and the code editor until the kit ships its own.

use crate::egui::{Align, CornerRadius, FontId, Layout, Response, RichText, Stroke, Ui};
use crate::icons::Icon;
use crate::style::Palette;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::{Dialog, DialogAction, FieldLabel, TextField};
use archipel_sdk::kit::tokens::{size, typography};

/// Section caption (small, muted).
pub fn caption(ui: &mut Ui, text: &str) {
    let color = Tone::Muted.resolve(ui.ctx());
    ui.label(RichText::new(text).size(typography::SMALL).color(color));
}

/// A labeled form row: right-aligned label column of `label_width` then the content.
pub fn form_row(ui: &mut Ui, label: &str, label_width: f32, add: impl FnOnce(&mut Ui)) {
    let color = Tone::Secondary.resolve(ui.ctx());
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            crate::egui::vec2(label_width, size::CONTROL_HEIGHT),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.label(RichText::new(label).color(color));
            },
        );
        ui.add_space(size::SECONDARY_GAP);
        add(ui);
    });
}

/// Give an input that has only a placeholder an accessible name (the kit's search field
/// has none; see "Kit gaps").
pub fn name_input(response: &Response, name: &str, enabled: bool) {
    response.widget_info(|| {
        crate::egui::WidgetInfo::labeled(crate::egui::WidgetType::TextEdit, enabled, name)
    });
}

/// Muted hint text.
pub fn hint(ui: &Ui, text: &str) -> RichText {
    RichText::new(text).color(Tone::Muted.resolve(ui.ctx()))
}

/// A query-bar field: a muted caption followed by a monospace kit text field.
pub fn query_field(
    ui: &mut Ui,
    caption: &str,
    text: &mut String,
    hint: &str,
    width: f32,
) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::FORM_LABEL_GAP;
        let label = ui.add(FieldLabel::new(caption));
        let remaining = (width - label.rect.width() - size::FORM_LABEL_GAP).max(40.0);
        ui.add(
            TextField::new(text)
                .name(caption)
                .placeholder(hint)
                .monospace(true)
                .width(remaining),
        )
    })
    .inner
}

/// Multiline code editor with mongo shell syntax coloring.
pub fn code_editor(
    ui: &mut Ui,
    text: &mut String,
    rows: usize,
    id: impl std::hash::Hash,
) -> Response {
    code_editor_with(ui, text, rows, id, false)
}

/// Like [`code_editor`]; `fill` grows it to the available height.
pub fn code_editor_with(
    ui: &mut Ui,
    text: &mut String,
    rows: usize,
    id: impl std::hash::Hash,
    fill: bool,
) -> Response {
    let p = Palette::of(ui);
    let font = FontId::monospace(12.5);
    let mut layouter = |ui: &Ui, buffer: &dyn crate::egui::TextBuffer, wrap_width: f32| {
        let mut job = crate::egui::text::LayoutJob::default();
        crate::values::highlight_into(&mut job, buffer.as_str(), &p, font.clone());
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    crate::egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(crate::egui::Margin::same(8))
        .show(ui, |ui| {
            if fill {
                ui.set_min_height(ui.available_height());
            }
            ui.add(
                crate::egui::TextEdit::multiline(text)
                    .id_salt(id)
                    .code_editor()
                    .frame(false)
                    .desired_rows(rows)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter),
            )
        })
        .inner
}

/// 1-based line and column of a byte offset.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..text.floor_char_boundary(offset)];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// A kit [`Dialog`] around `body`, for dialogs that lay out their own buttons.
/// Returns `false` once the user dismissed it (Esc or a click outside).
pub fn modal(
    ctx: &crate::egui::Context,
    title: &str,
    icon: Icon,
    width: f32,
    body: impl FnOnce(&mut Ui),
) -> bool {
    let width = width.min(ctx.content_rect().width() - 40.0);
    let action = Dialog::new(title)
        .icon(icon.glyph())
        .width(width)
        .content(body)
        .show(ctx);
    !matches!(action, Some(DialogAction::Dismissed))
}
