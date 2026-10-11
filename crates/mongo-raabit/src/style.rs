//! Palette and small drawing helpers following the Jetthing "Islands" look.
//!
//! Colors are picked from the host's visuals (`dark_mode`), so the views follow the
//! workbench theme. Dark values match the Islands Dark tokens of `jetthing-ui`; light
//! values follow the IntelliJ Light theme facts (Apache-2.0 theme files).

use egui::{Color32, CornerRadius, FontId, Margin, Stroke, Ui};

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
    /// Leaf green used for brand accents (database icons, charts).
    pub leaf: Color32,
}

impl Palette {
    pub const DARK: Self = Self {
        dark: true,
        frame: hex(0x26282C),
        island: hex(0x191A1C),
        raised: hex(0x2B2D30),
        field: hex(0x1E1F22),
        border: hex(0x393B40),
        separator: hex(0x2B2D30),
        text: hex(0xD1D3D9),
        text_strong: hex(0xEDEEF0),
        text_secondary: hex(0x9FA2A8),
        text_info: hex(0x73767C),
        hover: Color32::from_rgba_premultiplied(0x17, 0x17, 0x17, 0x17),
        selection: hex(0x2A4371),
        selection_inactive: hex(0x2E3036),
        accent: hex(0x3871E1),
        accent_text: hex(0x6EA0FA),
        success: hex(0x6DB083),
        warning: hex(0xD59637),
        error: hex(0xF57E84),
        stripe: hex(0x1D1E21),
        key: hex(0xC77DBB),
        string: hex(0x6AAB73),
        number: hex(0x2AACB8),
        keyword: hex(0xCF8E6D),
        object_id: hex(0x56A8F5),
        date: hex(0xB3AE60),
        leaf: hex(0x5FB878),
    };

    pub const LIGHT: Self = Self {
        dark: false,
        frame: hex(0xF2F3F5),
        island: hex(0xFFFFFF),
        raised: hex(0xF7F8FA),
        field: hex(0xFFFFFF),
        border: hex(0xDFE1E5),
        separator: hex(0xEBECF0),
        text: hex(0x1E1F22),
        text_strong: hex(0x000000),
        text_secondary: hex(0x6C707E),
        text_info: hex(0x8C8F99),
        hover: Color32::from_rgba_premultiplied(0, 0, 0, 0x0D),
        selection: hex(0xD5E1FF),
        selection_inactive: hex(0xEBECF0),
        accent: hex(0x3574F0),
        accent_text: hex(0x2D5FD0),
        success: hex(0x208A3C),
        warning: hex(0xC27D04),
        error: hex(0xDB3B4B),
        stripe: hex(0xF9FAFB),
        key: hex(0x871094),
        string: hex(0x067D17),
        number: hex(0x1750EB),
        keyword: hex(0x0033B3),
        object_id: hex(0x00627A),
        date: hex(0x9E880D),
        leaf: hex(0x2E9447),
    };

    pub fn of(ui: &Ui) -> Self {
        Self::for_dark(ui.visuals().dark_mode)
    }

    pub fn for_dark(dark: bool) -> Self {
        if dark { Self::DARK } else { Self::LIGHT }
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

pub const RADIUS: u8 = 10;
pub const GAP: f32 = 2.0;

/// A rounded island panel.
pub fn island(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.island)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(0))
}

/// A subtle bordered card inside an island.
pub fn card(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
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
