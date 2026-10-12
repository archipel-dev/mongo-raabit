//! The editor tab of a collection in the center island: what the collection holds, in
//! one of its views. Documents shows the result of the tab's query (Table / Tree / JSON,
//! paging, count and query time) under a one-line summary of the query; the query itself
//! is built in the Query tool window on the right. Aggregation shows the output of the
//! pipeline of the Aggregation tool window, Schema the sampled schema, SQL and Shell their
//! consoles.

use crate::app::{CenterView, CollectionTab, Load, MongoRaabit, ResultView, TabId, ToolTab};
use crate::egui::{RichText, Ui};
use crate::icons::{self, Icon};
use crate::results::{self, DocCommand, TableEvent, TreeEvent};
use crate::widgets;
use crate::writes::READ_ONLY_TIP_LOWER;
use archipel_sdk::kit::components::{
    Banner, Dropdown, EmptyState, IconButton, SegmentedControl, Separator, Severity, Spinner, Tag,
    TooltipExt,
};
use archipel_sdk::kit::tokens::{size, typography};
use archipel_sdk::kit::{Glyph, Tone};
use mongo_raabit_core::connection::CollectionKind;
use mongo_raabit_core::format;

const PAGE_SIZE_WIDTH: f32 = 110.0;

pub const PAGE_SIZES: [u64; 5] = [20, 50, 100, 250, 1000];

enum Action {
    Add,
    Select(usize),
    Doc(DocCommand, Option<usize>),
    CommitEdit,
    Explain,
    Code,
    Run { recount: bool },
    Reset,
    EditQuery,
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
    let read_only = app.session_read_only(server);
    let Some(tab) = app.collection_tab_mut(tab_id) else {
        return;
    };
    header(ui, tab, &server_name, read_only);
    match tab.center {
        CenterView::Documents => documents_view(app, tab_id, read_only, ui),
        CenterView::Aggregation => {
            if app.ensure_aggregation(tab_id) {
                app.preview_stage(tab_id);
            }
            crate::aggregation_view::output(app, tab_id, ui);
        }
        CenterView::Schema => {
            app.ensure_structure(tab_id);
            crate::schema_view::show(app, tab_id, ui);
        }
        CenterView::Sql => crate::sql_view::show(app, tab_id, ui),
        CenterView::Shell => {
            app.init_shell(tab_id);
            crate::shell_view::show(app, tab_id, ui);
        }
    }
}

/// The documents of the tab's query: its summary, the result toolbar and the results.
fn documents_view(app: &mut MongoRaabit, tab_id: TabId, read_only: bool, ui: &mut Ui) {
    let Some(tab) = app.collection_tab_mut(tab_id) else {
        return;
    };
    let mut actions = Vec::new();
    widgets::padded(ui, |ui| {
        query_summary(ui, tab, &mut actions);
        if let Some(err) = tab.query_error.clone() {
            ui.add_space(size::ISLAND_GAP);
            let banner = Banner::new(Severity::Error, err).dismissible(true).show(ui);
            if banner.dismissed {
                tab.query_error = None;
            }
        }
        ui.add_space(size::SECTION_SPACING);
        result_toolbar(ui, tab, read_only, &mut actions);
    });
    ui.add(Separator);
    result_area(ui, tab, read_only, &mut actions);
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
            Action::Select(index) => {
                if let Some(tab) = app.collection_tab_mut(tab_id) {
                    tab.selected = Some(index);
                }
            }
            Action::CommitEdit => app.commit_cell_edit(tab_id),
            Action::Explain => app.explain(tab_id),
            Action::Code => app.query_code(tab_id),
            Action::EditQuery => app.show_tool(ToolTab::Query),
            Action::Reset => {
                if let Some(tab) = app.collection_tab_mut(tab_id) {
                    tab.builder.reset();
                    tab.query = Default::default();
                }
                app.run_documents(tab_id, true);
            }
            Action::Run { recount } => app.run_documents(tab_id, recount),
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

impl MongoRaabit {
    /// Runs the tab's query from its first page (after the query changed).
    pub fn run_documents(&mut self, tab_id: TabId, recount: bool) {
        if let Some(tab) = self.collection_tab_mut(tab_id) {
            tab.page = 0;
            tab.drill.clear();
            tab.selected = None;
        }
        self.run_query(tab_id, recount);
    }
}

/// One line saying which query the documents answer, read-only, with the way to edit it
/// in the Query tool window, and its reset.
fn query_summary(ui: &mut Ui, tab: &CollectionTab, actions: &mut Vec<Action>) {
    let text = tab.query.preview();
    let pending = match (tab.query.parse(), &tab.ran) {
        (Ok(query), Some(ran)) => &query != ran,
        (Ok(_), None) => false,
        (Err(_), _) => true,
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        icons::icon(ui, Icon::Filter, Tone::Muted.resolve(ui.ctx()));
        ui.with_layout(
            crate::egui::Layout::right_to_left(crate::egui::Align::Center),
            |ui| {
                if ui
                    .add(IconButton::new(Icon::Close, "Reset query"))
                    .tooltip("Clear the query and show every document")
                    .clicked()
                {
                    actions.push(Action::Reset);
                }
                if ui
                    .add(IconButton::new(Icon::Edit, "Edit query"))
                    .tooltip("Edit the query in the Query tool window")
                    .clicked()
                {
                    actions.push(Action::EditQuery);
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
                if ui
                    .add(IconButton::new(Icon::Explain, "Explain query plan"))
                    .clicked()
                {
                    actions.push(Action::Explain);
                }
                if let Some(page) = tab.result.ready() {
                    icons::icon(ui, Icon::TimeSeries, Tone::Muted.resolve(ui.ctx()));
                    ui.label(tone_text(ui, Tone::Muted, format::duration(page.elapsed)))
                        .on_hover_text("Query time of this page");
                }
                count_tag(ui, &tab.count);
                if pending {
                    ui.add(Tag::new("Not run").tone(Tone::Warning))
                        .tooltip("The query was changed: Run it in the Query tool window");
                }
                let rich = RichText::new(&text)
                    .font(crate::egui::FontId::monospace(typography::BODY))
                    .color(Tone::Secondary.resolve(ui.ctx()));
                ui.with_layout(
                    crate::egui::Layout::left_to_right(crate::egui::Align::Center),
                    |ui| {
                        ui.add(crate::egui::Label::new(rich).truncate())
                            .tooltip(format!("Query: {text}"));
                    },
                );
            },
        );
    });
}

fn tone_text(ui: &Ui, tone: Tone, text: impl Into<String>) -> RichText {
    RichText::new(text).color(tone.resolve(ui.ctx()))
}

/// Why a read-only connection's write actions are disabled.
const READ_ONLY_HINT: &str =
    "Read-only connection: edits, inserts, deletes and index changes are blocked";

/// The document count of the header.
fn count_tag(ui: &mut Ui, count: &Load<u64>) {
    match count {
        Load::Ready(n) => {
            ui.add(Tag::new(format!("{} documents", format::group(*n))).tone(Tone::Accent));
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
}

/// The tab's header: the switch between its views, and where the collection lives and
/// what it is.
fn header(ui: &mut Ui, tab: &mut CollectionTab, server: &str, read_only: bool) {
    let (icon, kind) = match tab.kind {
        CollectionKind::View => (Icon::View, "View"),
        CollectionKind::TimeSeries => (Icon::TimeSeries, "Time series"),
        CollectionKind::GridFsBucket => (Icon::Bucket, "Bucket"),
        CollectionKind::System => (Icon::Collection, "System"),
        CollectionKind::Collection => (Icon::Collection, "Collection"),
    };
    let tag_tone = if tab.kind == CollectionKind::View {
        Tone::Accent
    } else {
        Tone::Secondary
    };
    widgets::padded(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(SegmentedControl::new(
                &mut tab.center,
                [
                    (CenterView::Documents, "Documents"),
                    (CenterView::Aggregation, "Aggregation"),
                    (CenterView::Schema, "Schema"),
                    (CenterView::Sql, "SQL"),
                    (CenterView::Shell, "Shell"),
                ],
            ));
        });
        ui.horizontal(|ui| {
            ui.add(Tag::new(kind).tone(tag_tone));
            if read_only {
                ui.add(Tag::new("Read-only").tone(Tone::Warning))
                    .tooltip(READ_ONLY_HINT);
            }
            widgets::label(ui, Tone::Muted, tab.db.as_str())
                .on_hover_text(format!("{server} › {}.{}", tab.db, tab.collection));
            icons::icon(ui, icon, Tone::Muted.resolve(ui.ctx()));
        });
    });
    ui.add(Separator);
}

/// The page sizes offered: the usual ones and the tab's own (from Settings).
fn page_sizes(current: u64) -> Vec<PageSize> {
    let mut sizes = PAGE_SIZES.to_vec();
    if !sizes.contains(&current) {
        sizes.push(current);
        sizes.sort_unstable();
    }
    sizes.into_iter().map(PageSize).collect()
}

/// A page size as the dropdown shows it.
#[derive(Clone, Copy, PartialEq)]
struct PageSize(u64);

impl std::fmt::Display for PageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} / page", self.0)
    }
}

/// A write action of the toolbar: on a read-only connection disabled, its tooltip (the
/// label) saying why. The kit's icon buttons take no separate disabled reason (see "Kit
/// gaps"), so the reason joins the label.
fn write_button(icon: Icon, label: &str, enabled: bool, read_only: bool) -> IconButton {
    if read_only {
        IconButton::new(icon, format!("{label} ({READ_ONLY_TIP_LOWER})")).enabled(false)
    } else {
        IconButton::new(icon, label).enabled(enabled)
    }
}

fn result_toolbar(
    ui: &mut Ui,
    tab: &mut CollectionTab,
    read_only: bool,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.add(SegmentedControl::new(
            &mut tab.view,
            [
                (ResultView::Table, "Table"),
                (ResultView::Tree, "Tree"),
                (ResultView::Json, "JSON"),
            ],
        ));
        ui.add_space(size::SECONDARY_GAP);
        let has_selection = tab.selected.is_some();
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        if ui
            .add(write_button(Icon::Plus, "Add document", true, read_only))
            .clicked()
        {
            actions.push(Action::Add);
        }
        if ui
            .add(write_button(
                Icon::Edit,
                "Edit document (double-click a row)",
                has_selection,
                read_only,
            ))
            .clicked()
        {
            actions.push(Action::Doc(DocCommand::Edit, None));
        }
        if ui
            .add(write_button(
                Icon::Copy,
                "Clone document",
                has_selection,
                read_only,
            ))
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
                write_button(Icon::Trash, "Delete document", has_selection, read_only)
                    .tone(Tone::Error),
            )
            .clicked()
        {
            actions.push(Action::Doc(DocCommand::Delete, None));
        }
        ui.with_layout(
            crate::egui::Layout::right_to_left(crate::egui::Align::Center),
            |ui| paging(ui, tab, actions),
        );
    });
    if tab.view == ResultView::Table && !tab.drill.is_empty() {
        ui.add_space(size::FORM_ROW_GAP);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = size::FORM_LABEL_GAP;
            if let Some(depth) = results::breadcrumb(ui, &tab.drill) {
                tab.drill.truncate(depth);
            }
        });
    }
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
    let range = format::page_range(
        page.map_or(0, |pg| pg.first_index()),
        page.map_or(0, |pg| pg.documents.len() as u64),
        match &tab.count {
            Load::Ready(n) => Some(*n),
            _ => None,
        },
    );
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
            page_sizes(tab.page_size),
        )
        .label("Page size")
        .width(PAGE_SIZE_WIDTH),
    );
    if chosen.0 != tab.page_size {
        actions.push(Action::PageSize(chosen.0));
    }
    if tab.result.is_loading() {
        ui.add(Spinner::new().label("Loading page"));
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

fn result_area(ui: &mut Ui, tab: &mut CollectionTab, read_only: bool, actions: &mut Vec<Action>) {
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
                    read_only,
                    actions,
                ),
                None => {
                    ui.add_space(size::FORM_GROUP_SPACING * 2.0);
                    ui.vertical_centered(|ui| {
                        ui.add(Spinner::new().label("Running query"));
                        ui.label(tone_text(ui, Tone::Secondary, "Running query…"));
                    });
                }
            }
        }
        Load::Failed(err) => {
            ui.data_mut(|d| d.remove::<Snapshot>(snapshot_id));
            ui.add_space(size::SECONDARY_GAP);
            Banner::new(Severity::Error, err.clone()).show(ui);
        }
        Load::Ready(page) if page.documents.is_empty() => {
            ui.data_mut(|d| d.remove::<Snapshot>(snapshot_id));
            ui.add_space(size::FORM_GROUP_SPACING);
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
            documents(ui, tab, &page.documents, first, tab.seq, read_only, actions);
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
    read_only: bool,
    actions: &mut Vec<Action>,
) {
    // A read-only connection shows the documents; it cannot open them for editing.
    let open = |i: usize| {
        if read_only {
            Action::Select(i)
        } else {
            Action::Doc(DocCommand::Edit, Some(i))
        }
    };
    match tab.view {
        ResultView::Table => match results::table(ui, tab, docs, first, !read_only) {
            Some(TableEvent::Drill(field)) => tab.drill.push(field),
            Some(TableEvent::Select(i)) => tab.selected = Some(i),
            Some(TableEvent::Open(i)) => actions.push(open(i)),
            Some(TableEvent::Edit(edit)) => {
                tab.editing = Some(edit);
                actions.push(Action::CommitEdit);
            }
            Some(TableEvent::Sort { field, descending }) => {
                tab.query.sort = mongo_raabit_core::documents::sort_text(&field, descending);
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
            !read_only,
        ) {
            Some(TreeEvent::Toggle(key)) => {
                if !tab.open.remove(&key) {
                    tab.open.insert(key);
                }
            }
            Some(TreeEvent::Select(i)) => tab.selected = Some(i),
            Some(TreeEvent::Open(i)) => actions.push(open(i)),
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
