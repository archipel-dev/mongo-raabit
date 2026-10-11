//! Small themed widgets shared by the views.

use crate::icons::{self, Icon};
use crate::style::Palette;
use egui::{
    Color32, CornerRadius, FontId, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
    vec2,
};

/// A 26×24 icon button with a hover background and tooltip.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    icon_button_tinted(ui, icon, tooltip, None, true)
}

pub fn icon_button_tinted(
    ui: &mut Ui,
    icon: Icon,
    tooltip: &str,
    tint: Option<Color32>,
    enabled: bool,
) -> Response {
    let p = Palette::of(ui);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 24.0), sense);
    if enabled && (response.hovered() || response.has_focus()) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    let color = if !enabled {
        p.text_info.gamma_multiply(0.6)
    } else {
        tint.unwrap_or(p.text_secondary)
    };
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(16.0)),
        icon,
        color,
    );
    response.on_hover_text(tooltip)
}

/// Filled accent button with an optional leading icon.
pub fn primary_button(ui: &mut Ui, icon: Option<Icon>, label: &str) -> Response {
    let p = Palette::of(ui);
    button_impl(ui, icon, label, p.accent, Color32::WHITE, None)
}

/// Outlined neutral button.
pub fn secondary_button(ui: &mut Ui, icon: Option<Icon>, label: &str) -> Response {
    let p = Palette::of(ui);
    button_impl(ui, icon, label, p.raised, p.text, Some(p.border))
}

fn button_impl(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    fill: Color32,
    text: Color32,
    border: Option<Color32>,
) -> Response {
    let font = FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, text);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 24.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        fill.gamma_multiply(0.85)
    } else if response.hovered() {
        fill.gamma_multiply(1.12)
    } else {
        fill
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(7),
        fill,
        border.map_or(Stroke::NONE, |c| Stroke::new(1.0_f32, c)),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + 12.0;
    if let Some(icon) = icon {
        icons::paint(
            painter,
            Rect::from_center_size(egui::pos2(x + 7.0, rect.center().y), Vec2::splat(14.0)),
            icon,
            text,
        );
        x += icon_w;
    }
    painter.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
    response
}

/// Segmented control (e.g. Table | Tree | JSON). Returns true when changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, Icon, &str)],
) -> bool {
    let p = Palette::of(ui);
    let mut changed = false;
    let frame = egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(2));
    frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            for (option, icon, label) in options {
                let selected = *value == *option;
                let font = FontId::proportional(12.5);
                let color = if selected {
                    p.text_strong
                } else {
                    p.text_secondary
                };
                let galley = ui
                    .painter()
                    .layout_no_wrap((*label).to_owned(), font, color);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(galley.size().x + 34.0, 22.0), Sense::click());
                if selected {
                    ui.painter().rect(
                        rect,
                        CornerRadius::same(6),
                        if p.dark {
                            p.selection_inactive
                        } else {
                            p.island
                        },
                        Stroke::new(1.0_f32, p.border),
                        StrokeKind::Inside,
                    );
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(6), p.hover);
                }
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(
                        egui::pos2(rect.left() + 14.0, rect.center().y),
                        Vec2::splat(13.0),
                    ),
                    *icon,
                    if selected {
                        p.accent_text
                    } else {
                        p.text_secondary
                    },
                );
                ui.painter().galley(
                    egui::pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0),
                    galley,
                    color,
                );
                if response.clicked() && !selected {
                    *value = *option;
                    changed = true;
                }
            }
        });
    });
    changed
}

/// A labeled query-bar field: small caption on the left inside a rounded field.
pub fn query_field(
    ui: &mut Ui,
    caption: &str,
    text: &mut String,
    hint: &str,
    width: f32,
) -> Response {
    let p = Palette::of(ui);
    let mut response = None;
    egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(RichText::new(caption).size(11.5).color(p.text_info));
                let edit = egui::TextEdit::singleline(text)
                    .hint_text(RichText::new(hint).color(p.text_info))
                    .font(FontId::monospace(12.5))
                    .frame(false)
                    .desired_width(f32::INFINITY);
                response = Some(ui.add(edit));
            });
        });
    response.unwrap_or_else(|| ui.label(""))
}

/// A rounded badge (counts, types, states).
pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(12.0, 4.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), color.gamma_multiply(0.16));
    ui.painter()
        .galley(rect.min + vec2(6.0, 2.0), galley, color);
    response
}

/// Thin horizontal separator in the separator color.
pub fn separator(ui: &mut Ui) {
    let p = Palette::of(ui);
    let rect = ui.available_rect_before_wrap();
    let y = ui.cursor().top();
    ui.painter()
        .hline(rect.x_range(), y, Stroke::new(1.0_f32, p.separator));
    ui.add_space(1.0);
}

/// Section caption (small, secondary).
pub fn caption(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui);
    ui.label(RichText::new(text).size(11.5).color(p.text_info));
}

/// A labeled form row: right-aligned label column of `label_width` then the content.
pub fn form_row(ui: &mut Ui, label: &str, label_width: f32, add: impl FnOnce(&mut Ui)) {
    let p = Palette::of(ui);
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            vec2(label_width, 24.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(RichText::new(label).color(p.text_secondary));
            },
        );
        ui.add_space(8.0);
        add(ui);
    });
}

/// Dimmed hint text for text fields (explicit color beats the theme override).
pub fn hint(ui: &Ui, text: &str) -> RichText {
    RichText::new(text).color(Palette::of(ui).text_info)
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
    let mut layouter = |ui: &Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = egui::text::LayoutJob::default();
        crate::values::highlight_into(&mut job, buffer.as_str(), &p, font.clone());
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            if fill {
                ui.set_min_height(ui.available_height());
            }
            ui.add(
                egui::TextEdit::multiline(text)
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

/// Shared modal frame: title with icon and close button, then `body`.
/// Returns `false` when the user closed it (✕ or Esc).
pub fn modal(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    icon: Icon,
    size: Vec2,
    body: impl FnOnce(&mut Ui),
) -> bool {
    let p = Palette::for_dark(ctx.style().visuals.dark_mode);
    let mut open = true;
    let screen = ctx.content_rect();
    let size = vec2(
        size.x.min(screen.width() - 40.0),
        size.y.min(screen.height() - 40.0),
    );
    let frame = egui::Frame::new()
        .fill(p.island)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(0))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 120 } else { 40 }),
        });
    let response = egui::Modal::new(egui::Id::new(id))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(if p.dark { 110 } else { 60 }))
        .show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_max_height(size.y);
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 10,
                    top: 12,
                    bottom: 10,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        icons::icon(ui, icon, p.leaf);
                        ui.label(
                            RichText::new(title)
                                .size(15.0)
                                .strong()
                                .color(p.text_strong),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, Icon::Close, "Close (Esc)").clicked() {
                                open = false;
                            }
                        });
                    });
                });
            separator(ui);
            egui::Frame::new()
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.set_width(size.x - 32.0);
                    body(ui);
                });
        });
    !response.should_close() && open
}
