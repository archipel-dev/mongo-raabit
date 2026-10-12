//! Small shared helpers over archipel-kit components.
//!
//! Buttons, icon buttons, segmented controls, tags, separators and dialogs are used straight
//! from `archipel_sdk::kit::components`; what remains here is layout glue (captions, form
//! rows, the query-bar field) and the code editor until the kit ships its own.

use crate::egui::{Align, CornerRadius, FontId, Id, Layout, Response, RichText, Stroke, Ui};
use crate::icons::Icon;
use crate::style::Palette;
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::{
    Dialog, DialogAction, Dropdown, FieldLabel, IconButton, MenuItem, TextField, TextFieldOutput,
};
use archipel_sdk::kit::tokens::{size, typography};

/// Section caption (small, muted).
pub fn caption(ui: &mut Ui, text: &str) {
    let color = Tone::Muted.resolve(ui.ctx());
    ui.label(RichText::new(text).size(typography::SMALL).color(color));
}

/// A label in the color of `tone`.
pub fn label(ui: &mut Ui, tone: Tone, text: impl Into<String>) -> Response {
    ui.label(RichText::new(text.into()).color(tone.resolve(ui.ctx())))
}

/// A small note (comment size) in the color of `tone`, wrapping at the available width.
pub fn note(ui: &mut Ui, tone: Tone, text: impl Into<String>) -> Response {
    let text = RichText::new(text.into())
        .size(typography::COMMENT)
        .color(tone.resolve(ui.ctx()));
    ui.add(crate::egui::Label::new(text).wrap())
}

/// A semibold title in the strong text color.
pub fn title(ui: &mut Ui, text: impl Into<String>) -> Response {
    let text = RichText::new(text.into())
        .font(typography::semibold(ui.ctx(), typography::TITLE))
        .color(Tone::Strong.resolve(ui.ctx()));
    ui.label(text)
}

/// Content with the view's standard inset on every side.
pub fn padded<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inset = size::CONTENT_INSET as i8;
    crate::egui::Frame::new()
        .inner_margin(crate::egui::Margin::symmetric(
            inset,
            size::SECTION_SPACING as i8,
        ))
        .show(ui, add)
        .inner
}

/// An option of a [`choice`]: compared by its value, shown as its label.
#[derive(Clone)]
struct Labeled<T> {
    value: T,
    label: String,
}

impl<T: PartialEq> PartialEq for Labeled<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T> std::fmt::Display for Labeled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// A kit [`Dropdown`] over `(value, label)` options, for values that have no `Display`.
/// `name` is its accessible name. `changed()` on the response tells whether the user
/// picked another option.
pub fn choice<'a, T: PartialEq + Clone>(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    name: &str,
    value: &mut T,
    options: impl IntoIterator<Item = (T, &'a str)>,
    width: f32,
) -> Response {
    let options: Vec<Labeled<T>> = options
        .into_iter()
        .map(|(value, label)| Labeled {
            value,
            label: label.to_owned(),
        })
        .collect();
    let mut current = options
        .iter()
        .find(|option| option.value == *value)
        .cloned()
        .unwrap_or_else(|| Labeled {
            value: value.clone(),
            label: "…".to_owned(),
        });
    let mut response = ui.add(
        Dropdown::new(id, &mut current, options)
            .label(name)
            .width(width),
    );
    if current.value != *value {
        *value = current.value;
        response.mark_changed();
    }
    response
}

/// A single-line monospace field for values and names. `name` is its accessible name; the
/// output tells when Enter was pressed in it.
pub fn value_field(
    ui: &mut Ui,
    name: &str,
    text: &mut String,
    hint: &str,
    width: f32,
    enabled: bool,
) -> TextFieldOutput {
    TextField::new(text)
        .name(name)
        .placeholder(hint)
        .monospace(true)
        .enabled(enabled)
        .width(width)
        .show(ui)
}

/// What happened in a [`number_field`] this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NumberEdit {
    /// The value was set to a number the user typed.
    pub changed: bool,
    /// The user left the field (or pressed Enter) after changing the value.
    pub committed: bool,
}

/// A whole-number field (the kit has no number input): edits `value` while the text parses.
pub fn number_field<N>(ui: &mut Ui, id: Id, name: &str, value: &mut N, width: f32) -> NumberEdit
where
    N: Copy + Default + PartialEq + std::fmt::Display + std::str::FromStr,
{
    let field_id = id.with("field");
    let buffer_id = id.with("text");
    let dirty_id = id.with("dirty");
    let editing = ui.memory(|m| m.has_focus(field_id));
    let mut text = if editing {
        ui.data(|d| d.get_temp::<String>(buffer_id))
            .unwrap_or_else(|| value.to_string())
    } else {
        value.to_string()
    };
    let response = TextField::new(&mut text)
        .id(field_id)
        .name(name)
        .monospace(true)
        .width(width)
        .show(ui)
        .response;
    let mut edit = NumberEdit::default();
    if response.changed()
        && let Some(parsed) = parse_number::<N>(&text)
        && parsed != *value
    {
        *value = parsed;
        edit.changed = true;
        ui.data_mut(|d| d.insert_temp(dirty_id, true));
    }
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(buffer_id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(buffer_id));
        if response.lost_focus() {
            edit.committed = ui
                .data_mut(|d| d.remove_temp::<bool>(dirty_id))
                .unwrap_or(false);
        }
    }
    edit
}

/// A number typed by the user: blank means the default (zero).
fn parse_number<N: Default + std::str::FromStr>(text: &str) -> Option<N> {
    let text = text.trim();
    if text.is_empty() {
        Some(N::default())
    } else {
        text.parse().ok()
    }
}

/// An editable name with a picker of `names` (fields seen in results, collections).
/// Typing filters the list; `empty_hint` is shown when there is nothing to pick yet.
/// Returns true when the text changed.
#[allow(clippy::too_many_arguments)]
pub fn name_picker(
    ui: &mut Ui,
    id: Id,
    name: &str,
    text: &mut String,
    names: &[String],
    hint: &str,
    empty_hint: &str,
    width: f32,
) -> bool {
    let mut changed = false;
    let field_width = (width - size::ICON_BUTTON - ui.spacing().item_spacing.x).max(40.0);
    changed |= TextField::new(text)
        .id(id.with("text"))
        .name(name)
        .placeholder(hint)
        .monospace(true)
        .width(field_width)
        .show(ui)
        .response
        .changed();
    let button = ui.add(IconButton::new(Icon::ChevronDown, format!("Pick {name}")));
    crate::egui::Popup::menu(&button)
        .id(id.with("popup"))
        .show(|ui| {
            ui.set_min_width(width);
            if names.is_empty() {
                label(ui, Tone::Muted, empty_hint);
                return;
            }
            archipel_sdk::kit::parts::scroll_vertical()
                .max_height(ui.ctx().content_rect().height() / 3.0)
                .show(ui, |ui| {
                    for picked in mongo_raabit_core::fields::filter_names(names, text) {
                        let item = MenuItem::new(picked.as_str())
                            .checked(picked.as_str() == text.as_str());
                        if ui.add(item).clicked() && picked.as_str() != text.as_str() {
                            text.clone_from(picked);
                            changed = true;
                        }
                    }
                });
        });
    changed
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbers_with_blank_as_zero() {
        assert_eq!(parse_number::<u64>("12"), Some(12));
        assert_eq!(parse_number::<u64>("  7 "), Some(7));
        assert_eq!(parse_number::<u64>(""), Some(0));
        assert_eq!(parse_number::<u64>("-1"), None);
        assert_eq!(parse_number::<u64>("1x"), None);
    }
}
