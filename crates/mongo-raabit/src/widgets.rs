//! Small shared helpers over archipel-kit components.
//!
//! Buttons, icon buttons, segmented controls, tags, separators and dialogs are used straight
//! from `archipel_sdk::kit::components`; what remains here is layout glue (captions, form
//! rows, the query-bar field) and the SQL highlighter.

use crate::egui::{Layout, Response, RichText, Ui};
use crate::icons::{self, Icon};
use archipel_sdk::kit::Tone;
use archipel_sdk::kit::components::code::{Highlighter, Span, TokenKind};
use archipel_sdk::kit::components::{
    Dropdown, FieldLabel, Separator, Spinner, Tag, TextField, TextFieldOutput,
};
use archipel_sdk::kit::tokens::{size, typography};
use mongo_raabit_core::sql_syntax::{SqlToken, tokenize_line};

/// Section caption (small, muted).
pub fn caption(ui: &mut Ui, text: &str) {
    let color = Tone::Muted.resolve(ui.ctx());
    ui.label(RichText::new(text).size(typography::SMALL).color(color));
}

/// A label in the color of `tone`.
pub fn label(ui: &mut Ui, tone: Tone, text: impl Into<String>) -> Response {
    ui.label(RichText::new(text.into()).color(tone.resolve(ui.ctx())))
}

/// A monospace label in the color of `tone`.
pub fn mono_label(ui: &mut Ui, tone: Tone, text: impl Into<String>) -> Response {
    ui.label(
        RichText::new(text.into())
            .monospace()
            .color(tone.resolve(ui.ctx())),
    )
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

/// The header of a tool tab: the tab's icon, the `trail` of server and database (muted),
/// the `title`, a `tag` naming the tool, `status` content after the tag (a spinner), and
/// `actions` laid out right to left at the far end; a separator closes it.
#[allow(clippy::too_many_arguments)]
pub fn tool_header(
    ui: &mut Ui,
    icon: Icon,
    icon_tone: Tone,
    trail: &[&str],
    title: &str,
    tag: Tag,
    status: impl FnOnce(&mut Ui),
    actions: impl FnOnce(&mut Ui),
) {
    padded(ui, |ui| {
        ui.horizontal(|ui| {
            icons::icon(ui, icon, icon_tone.resolve(ui.ctx()));
            let muted = Tone::Muted.resolve(ui.ctx());
            for (i, crumb) in trail.iter().enumerate() {
                label(
                    ui,
                    if i == 0 { Tone::Muted } else { Tone::Secondary },
                    *crumb,
                );
                icons::icon(ui, Icon::ChevronRight, muted);
            }
            self::title(ui, title);
            ui.add(tag);
            status(ui);
            ui.with_layout(Layout::right_to_left(crate::egui::Align::Center), actions);
        });
    });
    ui.add(Separator);
}

/// A centered spinner with a caption, for a view that is loading.
pub fn busy(ui: &mut Ui, text: &str) {
    ui.add_space(size::FORM_GROUP_SPACING * 2.0);
    ui.vertical_centered(|ui| {
        ui.add(Spinner::new().label(text));
        label(ui, Tone::Secondary, text);
    });
}

/// SQL coloring for the kit's code views and editors.
pub struct SqlHighlighter;

impl Highlighter for SqlHighlighter {
    fn highlight_line(&self, line: &str, _state: &mut u32, out: &mut Vec<Span>) {
        out.extend(tokenize_line(line).into_iter().map(|(range, token)| Span {
            start: range.start,
            end: range.end,
            kind: match token {
                SqlToken::Keyword => TokenKind::Keyword,
                SqlToken::String => TokenKind::String,
                SqlToken::QuotedName => TokenKind::Property,
                SqlToken::Number => TokenKind::Number,
                SqlToken::Comment => TokenKind::Comment,
                SqlToken::Punctuation => TokenKind::Punctuation,
            },
        }));
    }

    fn stateless(&self) -> bool {
        true
    }
}

/// 1-based line and column of a byte offset.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..text.floor_char_boundary(offset)];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}
