//! Query tab of a collection: the query written with the visual builder, as shell code,
//! as SQL or as a mongosh script, the paging toolbar and the Table / Tree / JSON result views.

use crate::app::{CollectionTab, Load, MongoRaabit, QueryMode, ResultView, TabId};
use crate::icons::{self, Icon};
use crate::results::{self, DocCommand, TableEvent, TreeEvent};
use crate::style::{self, Palette};
use crate::tree::ToolKind;
use crate::widgets;
use egui::{CornerRadius, RichText, Stroke, Ui};
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
    let p = Palette::of(ui);
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
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 6,
        })
        .show(ui, |ui| {
            header(ui, &p, tab, &server_name, &mut tool);
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
                    match crate::query_builder::show(ui, &p, &mut tab.builder, &fields) {
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
                _ => query_bar(ui, &p, tab, &mut actions),
            }
            if let Some(err) = &tab.query_error {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    icons::icon(ui, Icon::Warning, p.error);
                    ui.label(RichText::new(err).color(p.error));
                });
            }
            ui.add_space(8.0);
            result_toolbar(ui, &p, tab, &mut actions);
        });
    if sql {
        widgets::separator(ui);
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
    widgets::separator(ui);
    result_area(ui, &p, tab, &mut actions);
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

fn header(
    ui: &mut Ui,
    p: &Palette,
    tab: &CollectionTab,
    server: &str,
    tool: &mut Option<ToolKind>,
) {
    ui.horizontal(|ui| {
        let icon = match tab.kind {
            CollectionKind::View => Icon::View,
            CollectionKind::TimeSeries => Icon::TimeSeries,
            CollectionKind::GridFsBucket => Icon::Bucket,
            _ => Icon::Collection,
        };
        icons::icon(ui, icon, p.leaf);
        ui.label(RichText::new(server).color(p.text_info));
        icons::icon(ui, Icon::ChevronRight, p.text_info);
        ui.label(RichText::new(&tab.db).color(p.text_secondary));
        icons::icon(ui, Icon::ChevronRight, p.text_info);
        ui.label(
            RichText::new(&tab.collection)
                .size(15.0)
                .strong()
                .color(p.text_strong),
        );
        if tab.kind == CollectionKind::View {
            widgets::badge(ui, "view", p.keyword);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (icon, tip, kind) in [
                (
                    Icon::Schema,
                    "Structure (schema and indexes)",
                    ToolKind::Schema,
                ),
                (Icon::Pipeline, "Aggregation", ToolKind::Aggregation),
            ] {
                if widgets::icon_button(ui, icon, tip).clicked() {
                    *tool = Some(kind);
                }
            }
            ui.add_space(10.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            match &tab.count {
                Load::Ready(n) => {
                    widgets::badge(ui, &format!("{} documents", group(*n)), p.accent_text);
                }
                Load::Loading => {
                    ui.spinner();
                }
                Load::Failed(err) => {
                    widgets::badge(ui, "count failed", p.error).on_hover_text(err);
                }
                Load::Idle => {}
            }
        });
    });
}

/// Builder | Code | SQL. Switching from Code back to the builder takes over the code
/// when the builder can represent it.
fn mode_switch(ui: &mut Ui, tab: &mut CollectionTab) {
    let before = tab.mode;
    ui.horizontal(|ui| {
        widgets::segmented(
            ui,
            &mut tab.mode,
            &[
                (QueryMode::Builder, Icon::Filter, "Builder"),
                (QueryMode::Code, Icon::Code, "Code"),
                (QueryMode::Sql, Icon::Sql, "SQL"),
                (QueryMode::Shell, Icon::Shell, "Shell"),
            ],
        );
    });
    if before != QueryMode::Builder && tab.mode == QueryMode::Builder {
        tab.builder.take_over(&tab.query);
    }
}

fn query_bar(ui: &mut Ui, p: &Palette, tab: &mut CollectionTab, actions: &mut Vec<Action>) {
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
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
                enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if widgets::primary_button(ui, Some(Icon::Play), "Run")
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
                enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let r = widgets::query_field(
                    ui,
                    "Sort",
                    &mut tab.query.sort,
                    "{ _id: -1 }",
                    total * 0.30,
                );
                enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let r = widgets::query_field(ui, "Skip", &mut tab.query.skip, "0", total * 0.11);
                enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let r = widgets::query_field(ui, "Limit", &mut tab.query.limit, "0", total * 0.11);
                enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if widgets::icon_button(ui, Icon::Close, "Reset query").clicked() {
                    tab.query = Default::default();
                    actions.push(Action::Run { recount: true });
                }
            });
            if enter {
                actions.push(Action::Run { recount: true });
            }
        });
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

fn result_toolbar(ui: &mut Ui, p: &Palette, tab: &mut CollectionTab, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        widgets::segmented(
            ui,
            &mut tab.view,
            &[
                (ResultView::Table, Icon::Table, "Table"),
                (ResultView::Tree, Icon::Tree, "Tree"),
                (ResultView::Json, Icon::Json, "JSON"),
            ],
        );
        ui.add_space(6.0);
        let has_selection = tab.selected.is_some();
        ui.spacing_mut().item_spacing.x = 0.0;
        if widgets::icon_button(ui, Icon::Plus, "Add document").clicked() {
            actions.push(Action::Add);
        }
        if widgets::icon_button_tinted(
            ui,
            Icon::Edit,
            "Edit document (double-click a row)",
            None,
            has_selection,
        )
        .clicked()
        {
            actions.push(Action::Doc(DocCommand::Edit, None));
        }
        if widgets::icon_button_tinted(ui, Icon::Copy, "Clone document", None, has_selection)
            .clicked()
        {
            actions.push(Action::Doc(DocCommand::Clone, None));
        }
        if widgets::icon_button_tinted(
            ui,
            Icon::Trash,
            "Delete document",
            Some(p.error),
            has_selection,
        )
        .clicked()
        {
            actions.push(Action::Doc(DocCommand::Delete, None));
        }
        ui.add_space(6.0);
        if widgets::icon_button(ui, Icon::Explain, "Explain query plan").clicked() {
            actions.push(Action::Explain);
        }
        if widgets::icon_button(
            ui,
            Icon::Code,
            "Query code (JS, Python, Java, C#, Go, Rust, PHP, Ruby)",
        )
        .clicked()
        {
            actions.push(Action::Code);
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        if tab.view == ResultView::Table && !tab.drill.is_empty() {
            ui.add_space(8.0);
            if let Some(depth) = results::breadcrumb(ui, p, &tab.drill) {
                tab.drill.truncate(depth);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let page = tab.result.ready();
            let has_more = page.is_some_and(|pg| pg.has_more);
            if widgets::icon_button_tinted(ui, Icon::ChevronRight, "Next page", None, has_more)
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
                format!("{} – {}{}", group(first), group(last), total)
            };
            ui.label(RichText::new(range).color(p.text_secondary));
            if widgets::icon_button_tinted(
                ui,
                Icon::ChevronLeft,
                "Previous page",
                None,
                tab.page > 0,
            )
            .clicked()
            {
                actions.push(Action::Page(tab.page.saturating_sub(1)));
            }
            let mut size = tab.page_size;
            egui::ComboBox::from_id_salt(("raabit-page-size", &tab.db, &tab.collection))
                .selected_text(format!("{size} / page"))
                .width(96.0)
                .show_ui(ui, |ui| {
                    for option in PAGE_SIZES {
                        ui.selectable_value(&mut size, option, format!("{option} / page"));
                    }
                });
            if size != tab.page_size {
                actions.push(Action::PageSize(size));
            }
            if let Some(page) = page {
                ui.add_space(6.0);
                ui.label(RichText::new(style::duration(page.elapsed)).color(p.text_info))
                    .on_hover_text("Query time of this page");
                icons::icon(ui, Icon::TimeSeries, p.text_info);
            }
        });
    });
}

fn result_area(ui: &mut Ui, p: &Palette, tab: &mut CollectionTab, actions: &mut Vec<Action>) {
    match &tab.result {
        Load::Idle => {}
        Load::Loading => {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label(RichText::new("Running query…").color(p.text_secondary));
            });
        }
        Load::Failed(err) => {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                icons::icon(ui, Icon::Warning, p.error);
                ui.add(egui::Label::new(RichText::new(err).color(p.error)).wrap());
            });
        }
        Load::Ready(page) => {
            if page.documents.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    icons::icon(ui, Icon::Filter, p.text_info);
                    ui.add_space(4.0);
                    ui.label(RichText::new("No documents match the query").color(p.text_secondary));
                });
                return;
            }
            let docs = page.documents.clone();
            let first = page.first_index();
            match tab.view {
                ResultView::Table => match results::table(ui, tab, &docs, first) {
                    Some(TableEvent::Drill(field)) => {
                        tab.drill.push(field);
                    }
                    Some(TableEvent::Select(i)) => tab.selected = Some(i),
                    Some(TableEvent::Open(i)) => {
                        actions.push(Action::Doc(DocCommand::Edit, Some(i)))
                    }
                    Some(TableEvent::Command(command, i)) => {
                        actions.push(Action::Doc(command, Some(i)))
                    }
                    Some(TableEvent::CommitEdit) => actions.push(Action::CommitEdit),
                    None => {}
                },
                ResultView::Tree => match results::tree(
                    ui,
                    results::TreeState {
                        id: egui::Id::new(("raabit-tree", &tab.db, &tab.collection)),
                        open: &tab.open,
                        selected: tab.selected,
                    },
                    &docs,
                    first,
                ) {
                    Some(TreeEvent::Toggle(key)) => {
                        if !tab.open.remove(&key) {
                            tab.open.insert(key);
                        }
                    }
                    Some(TreeEvent::Select(i)) => tab.selected = Some(i),
                    Some(TreeEvent::Open(i)) => {
                        actions.push(Action::Doc(DocCommand::Edit, Some(i)))
                    }
                    Some(TreeEvent::Command(command, i)) => {
                        actions.push(Action::Doc(command, Some(i)))
                    }
                    None => {}
                },
                ResultView::Json => results::json(ui, &docs, first),
            }
        }
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
