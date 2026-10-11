//! Query tab of a collection: the query written with the visual builder, as shell code,
//! as SQL or as a mongosh script, the paging toolbar and the Table / Tree / JSON result views.

use crate::app::{CollectionTab, Load, MongoRaabit, QueryMode, ResultView, TabId};
use crate::egui::{RichText, Ui};
use crate::icons::{self, Icon};
use crate::results::{self, DocCommand, TableEvent, TreeEvent};
use crate::style;
use crate::tree::ToolKind;
use crate::widgets;
use archipel_sdk::kit::components::{
    Banner, Button, Dropdown, EmptyState, IconButton, SegmentedControl, Separator, Severity,
    Spinner, Tag,
};
use archipel_sdk::kit::tokens::size;
use archipel_sdk::kit::{Glyph, Tone};
use mongo_raabit_core::connection::CollectionKind;

pub const PAGE_SIZES: [u64; 5] = [20, 50, 100, 250, 1000];

enum Action {
    Add,
    Doc(DocCommand, Option<usize>),
    CommitEdit,
    Explain,
    Code,
    Run { recount: bool },
    Page(u64),
    PageSize(u64),
}

pub fn show(app: &mut MongoRaabit, tab_id: TabId, ui: &mut Ui) {
    let server = {
        let Some(tab) = app.collection_tab_mut(tab_id) else {
            return;
        };
        tab.session
    };
    let server_name = app
        .session(server)
        .map(|s| s.profile.name.clone())
        .unwrap_or_default();
    let Some(tab) = app.collection_tab_mut(tab_id) else {
        return;
    };
    let mut actions = Vec::new();
    let mut tool = None;
    let mut sql = false;
    crate::egui::Frame::new()
        .inner_margin(crate::egui::Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 6,
        })
        .show(ui, |ui| {
            header(ui, tab, &server_name, &mut tool);
            ui.add_space(8.0);
            mode_switch(ui, tab);
            if matches!(tab.mode, QueryMode::Sql | QueryMode::Shell) {
                sql = true;
                return;
            }
            ui.add_space(6.0);
            match tab.mode {
                QueryMode::Builder => {
                    let fields = tab
                        .result
                        .ready()
                        .map(|page| crate::query_builder::field_names(&page.documents))
                        .unwrap_or_default();
                    match crate::query_builder::show(
                        ui,
                        &crate::style::Palette::of(ui),
                        &mut tab.builder,
                        &fields,
                    ) {
                        crate::query_builder::Event::None => {}
                        crate::query_builder::Event::Changed => {
                            tab.query = tab.builder.to_query_text();
                        }
                        crate::query_builder::Event::Run => {
                            tab.query = tab.builder.to_query_text();
                            actions.push(Action::Run { recount: true });
                        }
                    }
                }
                _ => query_bar(ui, tab, &mut actions),
            }
            if let Some(err) = tab.query_error.clone() {
                ui.add_space(4.0);
                let banner = Banner::new(Severity::Error, err).dismissible(true).show(ui);
                if banner.dismissed {
                    tab.query_error = None;
                }
            }
            ui.add_space(8.0);
            result_toolbar(ui, tab, &mut actions);
        });
    if sql {
        ui.add(Separator);
        if app.collection_tab_mut(tab_id).map(|t| t.mode) == Some(QueryMode::Shell) {
            app.init_shell(tab_id);
            crate::shell_view::show(app, tab_id, ui);
        } else {
            crate::sql_view::show(app, tab_id, ui);
        }
        if let Some(kind) = tool
            && let Some(tab) = app.collection_tab_mut(tab_id)
        {
            let (session, db, coll) = (tab.session, tab.db.clone(), tab.collection.clone());
            crate::tree::open_tool(app, session, &db, Some(&coll), kind);
        }
        return;
    }
    ui.add(Separator);
    result_area(ui, tab, &mut actions);
    if let Some(kind) = tool
        && let Some(tab) = app.collection_tab_mut(tab_id)
    {
        let (session, db, coll) = (tab.session, tab.db.clone(), tab.collection.clone());
        crate::tree::open_tool(app, session, &db, Some(&coll), kind);
    }
    for action in actions {
        match action {
            Action::Doc(command, index) => {
                if let Some(index) = index
                    && let Some(tab) = app.collection_tab_mut(tab_id)
                {
                    tab.selected = Some(index);
                }
                match command {
                    DocCommand::Edit => app.edit_selected(tab_id),
                    DocCommand::Clone => app.clone_selected(tab_id),
                    DocCommand::Delete => app.request_delete(tab_id),
                    DocCommand::CopyJson | DocCommand::CopyId => {
                        if let Some(doc) = app.selected_document(tab_id) {
                            let text = if command == DocCommand::CopyId {
                                doc.get("_id")
                                    .map(|id| {
                                        mongo_raabit_core::shell::to_shell_string(
                                            id,
                                            mongo_raabit_core::shell::ShellStyle::Compact,
                                        )
                                    })
                                    .unwrap_or_default()
                            } else {
                                mongo_raabit_core::shell::document_to_shell(
                                    &doc,
                                    mongo_raabit_core::shell::ShellStyle::Pretty(2),
                                )
                            };
                            ui.ctx().copy_text(text);
                            app.status = "Copied to clipboard".into();
                        }
                    }
                }
            }
            Action::Add => app.add_document(tab_id),
            Action::CommitEdit => app.commit_cell_edit(tab_id),
            Action::Explain => app.explain(tab_id),
            Action::Code => app.query_code(tab_id),
            Action::Run { recount } => {
                if let Some(tab) = app.collection_tab_mut(tab_id) {
                    tab.page = 0;
                    tab.drill.clear();
                    tab.selected = None;
                }
                app.run_query(tab_id, recount);
            }
            Action::Page(page) => {
                if let Some(tab) = app.collection_tab_mut(tab_id) {
                    tab.page = page;
                    tab.selected = None;
                }
                app.run_query(tab_id, false);
            }
            Action::PageSize(size) => {
                if let Some(tab) = app.collection_tab_mut(tab_id) {
                    tab.page_size = size;
                    tab.page = 0;
                }
                app.run_query(tab_id, false);
            }
        }
    }
}

fn tone_text(ui: &Ui, tone: Tone, text: impl Into<String>) -> RichText {
    RichText::new(text).color(tone.resolve(ui.ctx()))
}

fn header(ui: &mut Ui, tab: &CollectionTab, server: &str, tool: &mut Option<ToolKind>) {
    ui.horizontal(|ui| {
        let icon = match tab.kind {
            CollectionKind::View => Icon::View,
            CollectionKind::TimeSeries => Icon::TimeSeries,
            CollectionKind::GridFsBucket => Icon::Bucket,
            _ => Icon::Collection,
        };
        let accent = Tone::Accent.resolve(ui.ctx());
        let muted = Tone::Muted.resolve(ui.ctx());
        icons::icon(ui, icon, accent);
        ui.label(tone_text(ui, Tone::Muted, server));
        icons::icon(ui, Icon::ChevronRight, muted);
        ui.label(tone_text(ui, Tone::Secondary, &tab.db));
        icons::icon(ui, Icon::ChevronRight, muted);
        ui.label(
            tone_text(ui, Tone::Strong, &tab.collection)
                .size(15.0)
                .strong(),
        );
        if tab.kind == CollectionKind::View {
            ui.add(Tag::new("view").tone(Tone::Accent));
        }
        ui.with_layout(
            crate::egui::Layout::right_to_left(crate::egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (icon, tip, kind) in [
                    (
                        Icon::Schema,
                        "Structure (schema and indexes)",
                        ToolKind::Schema,
                    ),
                    (Icon::Pipeline, "Aggregation", ToolKind::Aggregation),
                ] {
                    if ui.add(IconButton::new(icon, tip)).clicked() {
                        *tool = Some(kind);
                    }
                }
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                match &tab.count {
                    Load::Ready(n) => {
                        ui.add(Tag::new(format!("{} documents", group(*n))).tone(Tone::Accent));
                    }
                    Load::Loading => {
                        ui.add(Spinner::new().label("Counting documents"));
                    }
                    Load::Failed(err) => {
                        ui.add(Tag::new("count failed").tone(Tone::Error))
                            .on_hover_text(err);
                    }
                    Load::Idle => {}
                }
            },
        );
    });
}

/// Builder | Code | SQL. Switching from Code back to the builder takes over the code
/// when the builder can represent it.
fn mode_switch(ui: &mut Ui, tab: &mut CollectionTab) {
    let before = tab.mode;
    ui.horizontal(|ui| {
        ui.add(SegmentedControl::new(
            &mut tab.mode,
            [
                (QueryMode::Builder, "Builder"),
                (QueryMode::Code, "Code"),
                (QueryMode::Sql, "SQL"),
                (QueryMode::Shell, "Shell"),
            ],
        ));
    });
    if before != QueryMode::Builder && tab.mode == QueryMode::Builder {
        tab.builder.take_over(&tab.query);
    }
}

fn query_bar(ui: &mut Ui, tab: &mut CollectionTab, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing = crate::egui::vec2(6.0, 6.0);
    let mut enter = false;
    ui.horizontal(|ui| {
        let width = ui.available_width() - 110.0;
        let r = widgets::query_field(
            ui,
            "Filter",
            &mut tab.query.filter,
            "{ field: \"value\" }",
            width,
        );
        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(crate::egui::Key::Enter));
        if ui
            .add(Button::primary("Run").icon(Icon::Play))
            .on_hover_text("Run query (Enter)")
            .clicked()
        {
            actions.push(Action::Run { recount: true });
        }
    });
    ui.horizontal(|ui| {
        let total = ui.available_width() - 6.0 * 5.0;
        let r = widgets::query_field(
            ui,
            "Projection",
            &mut tab.query.projection,
            "{ name: 1 }",
            total * 0.34,
        );
        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(crate::egui::Key::Enter));
        let r = widgets::query_field(ui, "Sort", &mut tab.query.sort, "{ _id: -1 }", total * 0.30);
        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(crate::egui::Key::Enter));
        let r = widgets::query_field(ui, "Skip", &mut tab.query.skip, "0", total * 0.11);
        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(crate::egui::Key::Enter));
        let r = widgets::query_field(ui, "Limit", &mut tab.query.limit, "0", total * 0.11);
        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(crate::egui::Key::Enter));
        if ui
            .add(IconButton::new(Icon::Close, "Reset query"))
            .clicked()
        {
            tab.query = Default::default();
            actions.push(Action::Run { recount: true });
        }
    });
    if enter {
        actions.push(Action::Run { recount: true });
    }
}

/// `1234567` → `1,234,567`.
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A page size as the dropdown shows it.
#[derive(Clone, Copy, PartialEq)]
struct PageSize(u64);

impl std::fmt::Display for PageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} / page", self.0)
    }
}

fn result_toolbar(ui: &mut Ui, tab: &mut CollectionTab, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.add(SegmentedControl::new(
            &mut tab.view,
            [
                (ResultView::Table, "Table"),
                (ResultView::Tree, "Tree"),
                (ResultView::Json, "JSON"),
            ],
        ));
        ui.add_space(6.0);
        let has_selection = tab.selected.is_some();
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        if ui
            .add(IconButton::new(Icon::Plus, "Add document"))
            .clicked()
        {
            actions.push(Action::Add);
        }
        if ui
            .add(
                IconButton::new(Icon::Edit, "Edit document (double-click a row)")
                    .enabled(has_selection),
            )
            .clicked()
        {
            actions.push(Action::Doc(DocCommand::Edit, None));
        }
        if ui
            .add(IconButton::new(Icon::Copy, "Clone document").enabled(has_selection))
            .clicked()
        {
            actions.push(Action::Doc(DocCommand::Clone, None));
        }
        if ui
            .add(IconButton::new(Icon::Json, "Copy document").enabled(has_selection))
            .clicked()
        {
            actions.push(Action::Doc(DocCommand::CopyJson, None));
        }
        if ui
            .add(
                IconButton::new(Icon::Trash, "Delete document")
                    .enabled(has_selection)
                    .tone(Tone::Error),
            )
            .clicked()
        {
            actions.push(Action::Doc(DocCommand::Delete, None));
        }
        ui.add_space(6.0);
        if ui
            .add(IconButton::new(Icon::Explain, "Explain query plan"))
            .clicked()
        {
            actions.push(Action::Explain);
        }
        if ui
            .add(IconButton::new(
                Icon::Code,
                "Query code (JS, Python, Java, C#, Go, Rust, PHP, Ruby)",
            ))
            .clicked()
        {
            actions.push(Action::Code);
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        if tab.view == ResultView::Table && !tab.drill.is_empty() {
            ui.add_space(8.0);
            if let Some(depth) = results::breadcrumb(ui, &tab.drill) {
                tab.drill.truncate(depth);
            }
        }
        ui.with_layout(
            crate::egui::Layout::right_to_left(crate::egui::Align::Center),
            |ui| paging(ui, tab, actions),
        );
    });
}

/// The paging bar, right to left: next, range, previous, page size, query time.
fn paging(ui: &mut Ui, tab: &mut CollectionTab, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
    let page = tab.result.ready();
    let has_more = page.is_some_and(|pg| pg.has_more);
    if ui
        .add(IconButton::new(Icon::ChevronRight, "Next page").enabled(has_more))
        .clicked()
    {
        actions.push(Action::Page(tab.page + 1));
    }
    let first = page.map(|pg| pg.first_index()).unwrap_or(0);
    let last = page
        .map(|pg| first + pg.documents.len() as u64)
        .unwrap_or(0)
        .saturating_sub(1);
    let total = match &tab.count {
        Load::Ready(n) => format!(" of {}", group(*n)),
        _ => String::new(),
    };
    let range = if first == 0 {
        "No documents".to_owned()
    } else {
        format!("{}–{}{}", group(first), group(last), total)
    };
    ui.label(tone_text(ui, Tone::Secondary, range));
    if ui
        .add(IconButton::new(Icon::ChevronLeft, "Previous page").enabled(tab.page > 0))
        .clicked()
    {
        actions.push(Action::Page(tab.page.saturating_sub(1)));
    }
    let mut chosen = PageSize(tab.page_size);
    ui.add(
        Dropdown::new(
            ("raabit-page-size", &tab.db, &tab.collection),
            &mut chosen,
            PAGE_SIZES.map(PageSize),
        )
        .label("Page size")
        .width(110.0),
    );
    if chosen.0 != tab.page_size {
        actions.push(Action::PageSize(chosen.0));
    }
    if tab.result.is_loading() {
        ui.add(Spinner::new().label("Loading page"));
    }
    if let Some(page) = page {
        ui.add_space(6.0);
        ui.label(tone_text(ui, Tone::Muted, style::duration(page.elapsed)))
            .on_hover_text("Query time of this page");
        icons::icon(ui, Icon::TimeSeries, Tone::Muted.resolve(ui.ctx()));
    }
}

/// The last page that was shown, kept while the next one loads so the view (and its
/// keyboard focus) does not flicker away on a refresh after an edit, a sort or paging.
#[derive(Clone)]
struct Snapshot {
    seq: u64,
    first: u64,
    docs: std::sync::Arc<Vec<mongo_raabit_core::bson::Document>>,
}

fn result_area(ui: &mut Ui, tab: &mut CollectionTab, actions: &mut Vec<Action>) {
    let snapshot_id = crate::egui::Id::new(("raabit-page", &tab.db, &tab.collection));
    // The page is lent to the view for this frame; background jobs only apply their
    // results between frames.
    let result = std::mem::replace(&mut tab.result, Load::Idle);
    match &result {
        Load::Idle => {}
        Load::Loading => {
            let stale: Option<Snapshot> = ui.data(|d| d.get_temp(snapshot_id));
            match stale {
                Some(snapshot) => documents(
                    ui,
                    tab,
                    &snapshot.docs,
                    snapshot.first,
                    snapshot.seq,
                    actions,
                ),
                None => {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.add(Spinner::new().label("Running query"));
                        ui.label(tone_text(ui, Tone::Secondary, "Running query…"));
                    });
                }
            }
        }
        Load::Failed(err) => {
            ui.data_mut(|d| d.remove::<Snapshot>(snapshot_id));
            ui.add_space(8.0);
            Banner::new(Severity::Error, err.clone()).show(ui);
        }
        Load::Ready(page) if page.documents.is_empty() => {
            ui.data_mut(|d| d.remove::<Snapshot>(snapshot_id));
            ui.add_space(24.0);
            EmptyState::new("No documents match the query")
                .icon(Glyph::Filter)
                .hint("Change the filter or reset the query.")
                .show(ui);
        }
        Load::Ready(page) => {
            let first = page.first_index();
            let stored: Option<Snapshot> = ui.data(|d| d.get_temp(snapshot_id));
            if stored.is_none_or(|s| s.seq != tab.seq) {
                let snapshot = Snapshot {
                    seq: tab.seq,
                    first,
                    docs: std::sync::Arc::new(page.documents.clone()),
                };
                ui.data_mut(|d| d.insert_temp(snapshot_id, snapshot));
            }
            documents(ui, tab, &page.documents, first, tab.seq, actions);
        }
    }
    tab.result = result;
}

/// The documents of one page in the chosen view; what the user did becomes `actions`.
fn documents(
    ui: &mut Ui,
    tab: &mut CollectionTab,
    docs: &[mongo_raabit_core::bson::Document],
    first: u64,
    version: u64,
    actions: &mut Vec<Action>,
) {
    match tab.view {
        ResultView::Table => match results::table(ui, tab, docs, first) {
            Some(TableEvent::Drill(field)) => tab.drill.push(field),
            Some(TableEvent::Select(i)) => tab.selected = Some(i),
            Some(TableEvent::Open(i)) => actions.push(Action::Doc(DocCommand::Edit, Some(i))),
            Some(TableEvent::Edit(edit)) => {
                tab.editing = Some(edit);
                actions.push(Action::CommitEdit);
            }
            Some(TableEvent::Sort { field, descending }) => {
                tab.query.sort = results::sort_text(&field, descending);
                actions.push(Action::Run { recount: false });
            }
            None => {}
        },
        ResultView::Tree => match results::tree_view(
            ui,
            results::TreeState {
                id: crate::egui::Id::new(("raabit-tree", &tab.db, &tab.collection)),
                open: &tab.open,
                selected: tab.selected,
            },
            docs,
            first,
            true,
        ) {
            Some(TreeEvent::Toggle(key)) => {
                if !tab.open.remove(&key) {
                    tab.open.insert(key);
                }
            }
            Some(TreeEvent::Select(i)) => tab.selected = Some(i),
            Some(TreeEvent::Open(i)) => actions.push(Action::Doc(DocCommand::Edit, Some(i))),
            Some(TreeEvent::Edit(edit)) => {
                tab.editing = Some(edit);
                actions.push(Action::CommitEdit);
            }
            None => {}
        },
        ResultView::Json => results::json_cached(
            ui,
            crate::egui::Id::new(("raabit-json", &tab.db, &tab.collection)),
            docs,
            first,
            Some(version),
        ),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn groups_digits() {
        assert_eq!(super::group(0), "0");
        assert_eq!(super::group(1200), "1,200");
        assert_eq!(super::group(1_234_567), "1,234,567");
    }
}
