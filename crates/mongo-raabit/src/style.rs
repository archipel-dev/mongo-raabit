//! Palette and small drawing helpers.
//!
//! Surfaces, text and interaction colors come from the Archipel kit theme installed by the
//! host (`Theme::colors`). Only the BSON value colors and the chart slots below are domain
//! colors the kit has no token for (see "SDK gaps" in the port notes).

use crate::egui::{Color32, FontId, Ui};
use archipel_sdk::kit::{Theme, theme::Colors};

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub frame: Color32,
    pub island: Color32,
    pub raised: Color32,
    pub field: Color32,
    pub border: Color32,
    pub separator: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub text_secondary: Color32,
    pub text_info: Color32,
    pub hover: Color32,
    pub selection: Color32,
    pub selection_inactive: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub stripe: Color32,
    // BSON value colors.
    pub key: Color32,
    pub string: Color32,
    pub number: Color32,
    pub keyword: Color32,
    pub object_id: Color32,
    pub date: Color32,
}

impl Palette {
    fn from_colors(dark: bool, c: &Colors) -> Self {
        let (key, string, number, keyword, object_id, date) = if dark {
            (
                hex(0xC77DBB),
                hex(0x6AAB73),
                hex(0x2AACB8),
                hex(0xCF8E6D),
                hex(0x56A8F5),
                hex(0xB3AE60),
            )
        } else {
            (
                hex(0x871094),
                hex(0x067D17),
                hex(0x1750EB),
                hex(0x0033B3),
                hex(0x00627A),
                hex(0x9E880D),
            )
        };
        Self {
            dark,
            frame: c.frame,
            island: c.island,
            raised: c.popup,
            field: c.field,
            border: c.border,
            separator: c.separator,
            text: c.text,
            text_strong: c.text_strong,
            text_secondary: c.text_secondary,
            text_info: c.text_placeholder,
            hover: c.hover,
            selection: c.selection,
            selection_inactive: c.row_hover,
            accent: c.accent,
            accent_text: c.link,
            success: c.success,
            warning: c.warning,
            error: c.error,
            stripe: c.frame,
            key,
            string,
            number,
            keyword,
            object_id,
            date,
        }
    }

    /// The palette of the theme installed in `ui`'s context.
    pub fn of(ui: &Ui) -> Self {
        Self::of_ctx(ui.ctx())
    }

    pub fn of_ctx(ctx: &crate::egui::Context) -> Self {
        Self::from_colors(Theme::current(ctx).dark, &Theme::colors(ctx))
    }

    /// The palette of the default dark or light kit theme (tests, headless use).
    pub fn for_dark(dark: bool) -> Self {
        let theme = if dark { Theme::dark() } else { Theme::light() };
        Self::from_colors(dark, &theme.colors)
    }
}

/// Categorical chart slots (validated for CVD separation and contrast against the
/// island surfaces with the dataviz palette validator). Assigned in fixed order.
const CHART_DARK: [Color32; 8] = [
    hex(0x3987E5),
    hex(0xD95926),
    hex(0x199E70),
    hex(0xC98500),
    hex(0xD55181),
    hex(0x008300),
    hex(0x9085E9),
    hex(0xE66767),
];
const CHART_LIGHT: [Color32; 8] = [
    hex(0x2A78D6),
    hex(0xEB6834),
    hex(0x1BAF7A),
    hex(0xEDA100),
    hex(0xE87BA4),
    hex(0x008300),
    hex(0x6250D6),
    hex(0xE34948),
];

/// BSON types with a fixed chart slot; everything else is "Other" (neutral).
pub const CHART_TYPES: [&str; 8] = [
    "String", "Int32", "Double", "Date", "Boolean", "ObjectId", "Object", "Array",
];

impl Palette {
    /// Chart color of categorical slot `i` (0-based, fixed order).
    pub fn chart(&self, i: usize) -> Color32 {
        let slots = if self.dark { &CHART_DARK } else { &CHART_LIGHT };
        slots.get(i).copied().unwrap_or(self.text_info)
    }

    /// Chart color of a BSON type name; the color follows the type, never its rank.
    pub fn type_color(&self, type_name: &str) -> Color32 {
        CHART_TYPES
            .iter()
            .position(|t| *t == type_name)
            .map_or(self.text_info, |i| self.chart(i))
    }
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Human readable byte size (`1.2 MB`).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Compact count (`1.2k`).
pub fn count(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Elapsed time (`12 ms`, `1.4 s`).
pub fn duration(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        "<1 ms".into()
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(count(1200), "1200");
        assert_eq!(count(12_345), "12.3k");
        assert_eq!(duration(std::time::Duration::from_micros(2500)), "2 ms");
    }
}
