//! Mongo Raabit's icon names, drawn with the kit's icon set (`archipel-icons`).
//!
//! [`Icon`] names what the views mean (collection, pipeline, …); [`Icon::glyph`] picks the
//! kit glyph that draws it. There is no artwork in this crate.

use crate::egui::{Color32, Painter, Rect, Response, Ui};
use archipel_sdk::kit::{self, Glyph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Server,
    Database,
    Collection,
    View,
    TimeSeries,
    Bucket,
    User,
    Folder,
    Play,
    Refresh,
    Plus,
    Minus,
    Edit,
    Copy,
    Trash,
    Search,
    Close,
    ChevronRight,
    ChevronDown,
    ChevronLeft,
    ChevronUp,
    Table,
    Tree,
    Json,
    Pipeline,
    Sql,
    Shell,
    Schema,
    Index,
    Pulse,
    Code,
    Explain,
    Import,
    Export,
    Plug,
    Unplug,
    Check,
    Warning,
    Filter,
}

impl Icon {
    /// The kit glyph that draws this icon.
    pub fn kit(self) -> kit::Icon {
        match self {
            Icon::Sql => kit::icons::Language::Sql.into(),
            other => other.glyph().into(),
        }
    }

    pub fn glyph(self) -> Glyph {
        match self {
            Icon::Server => Glyph::Server,
            Icon::Database => Glyph::Database,
            Icon::Collection => Glyph::Table,
            Icon::View => Glyph::Eye,
            Icon::TimeSeries => Glyph::Clock,
            Icon::Bucket => Glyph::Archive,
            Icon::User => Glyph::User,
            Icon::Folder => Glyph::Folder,
            Icon::Play => Glyph::Run,
            Icon::Refresh => Glyph::Refresh,
            Icon::Plus => Glyph::Plus,
            Icon::Minus => Glyph::Minus,
            Icon::Edit => Glyph::Edit,
            Icon::Copy => Glyph::Copy,
            Icon::Trash => Glyph::Delete,
            Icon::Search => Glyph::Search,
            Icon::Close => Glyph::Close,
            Icon::ChevronRight => Glyph::ChevronRight,
            Icon::ChevronDown => Glyph::ChevronDown,
            Icon::ChevronLeft => Glyph::ChevronLeft,
            Icon::ChevronUp => Glyph::ChevronUp,
            Icon::Table => Glyph::Table,
            Icon::Tree => Glyph::Structure,
            Icon::Json => Glyph::Json,
            Icon::Pipeline => Glyph::Layers,
            Icon::Sql => Glyph::Code,
            Icon::Shell => Glyph::Terminal,
            Icon::Schema => Glyph::Structure,
            Icon::Index => Glyph::Key,
            Icon::Pulse => Glyph::Lightning,
            Icon::Code => Glyph::Code,
            Icon::Explain => Glyph::Help,
            Icon::Import => Glyph::Download,
            Icon::Export => Glyph::Upload,
            Icon::Plug | Icon::Unplug => Glyph::Plug,
            Icon::Check => Glyph::Check,
            Icon::Warning => Glyph::Warning,
            Icon::Filter => Glyph::Filter,
        }
    }
}

impl From<Icon> for kit::Icon {
    fn from(icon: Icon) -> Self {
        icon.kit()
    }
}

/// Allocate a 16×16 icon and paint it.
pub fn icon(ui: &mut Ui, icon: Icon, color: Color32) -> Response {
    kit::icons::show(ui, icon.kit(), 16.0, color)
}

/// Paint `icon` centered in `rect`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    kit::icons::paint(painter, rect, icon.kit(), color);
}
