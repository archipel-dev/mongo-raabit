//! Connection tree: sessions → databases → collections, views, time series and GridFS.
//!
//! Drawn with the kit's `Tree`: the rows are flattened from the session state each frame,
//! the kit handles selection, ↑/↓/←/→, Enter and double-click, and reports what to toggle.

use crate::app::{Load, MongoRaabit, NodeKey, SessionId, SessionState};
use crate::egui::Ui;
use crate::icons::Icon;
use archipel_sdk::kit::Glyph;
use archipel_sdk::kit::behaviour::ContextMenu;
use archipel_sdk::kit::components::{
    Banner, EmptyState, IconButton, MenuItem, MenuSeparator, Severity, TextField, Tree, TreeRow,
};
use archipel_sdk::kit::tokens::size;
use mongo_raabit_core::connection::CollectionKind;
use mongo_raabit_core::format;

enum Action {
    Toggle(NodeKey),
    Select(NodeKey),
    Open(SessionId, String, String, CollectionKind),
    RefreshSession(SessionId),
    RefreshDatabase(SessionId, String),
    Disconnect(SessionId),
    Tool(SessionId, String, Option<String>, ToolKind),
}

#[derive(Clone, Copy)]
pub enum ToolKind {
    Aggregation,
    Sql,
    Schema,
    Indexes,
    Shell,
}

/// The flattened, visible rows of the tree and what each one stands for.
#[derive(Default)]
struct Flat {
    rows: Vec<TreeRow>,
    keys: Vec<NodeKey>,
    /// Open target of a collection row (GridFS buckets open their `.files`).
    collections: Vec<Option<(String, String, CollectionKind)>>,
    errors: Vec<String>,
}

impl Flat {
    fn push(
        &mut self,
        row: TreeRow,
        key: NodeKey,
        collection: Option<(String, String, CollectionKind)>,
    ) {
        self.rows.push(row);
        self.keys.push(key);
        self.collections.push(collection);
    }
}

fn collection_icon(kind: CollectionKind) -> Icon {
    match kind {
        CollectionKind::Collection | CollectionKind::System => Icon::Collection,
        CollectionKind::View => Icon::View,
        CollectionKind::TimeSeries => Icon::TimeSeries,
        CollectionKind::GridFsBucket => Icon::Bucket,
    }
}

fn flatten(app: &MongoRaabit) -> Flat {
    let filter = app.tree_filter.to_lowercase();
    let mut flat = Flat::default();
    for session in &app.sessions {
        let skey = NodeKey::Session(session.id);
        let expanded = app.expanded.contains(&skey);
        let info = match &session.state {
            SessionState::Connecting => "connecting…".to_owned(),
            SessionState::Connected(conn) if session.databases.is_loading() => {
                format!("v{} · loading…", conn.server().version)
            }
            SessionState::Connected(conn) => format!("v{}", conn.server().version),
            SessionState::Failed(err) => {
                flat.errors.push(format!("{}: {err}", session.profile.name));
                "failed".into()
            }
        };
        flat.push(
            TreeRow::new(session.profile.name.as_str(), 0)
                .icon(Icon::Server)
                .secondary(info)
                .parent(expanded),
            skey,
            None,
        );
        if !expanded {
            continue;
        }
        let databases = match &session.databases {
            Load::Ready(databases) => databases,
            Load::Failed(err) => {
                flat.errors.push(format!("{}: {err}", session.profile.name));
                continue;
            }
            _ => continue,
        };
        for db in databases {
            let dkey = NodeKey::Database(session.id, db.info.name.clone());
            let collections = db.collections.ready();
            let db_matches = filter.is_empty() || db.info.name.to_lowercase().contains(&filter);
            let child_matches = collections.is_some_and(|c| {
                c.iter()
                    .any(|c| c.info.name.to_lowercase().contains(&filter))
            });
            if !db_matches && !child_matches {
                continue;
            }
            let expanded = app.expanded.contains(&dkey) || (!filter.is_empty() && child_matches);
            let info = if db.collections.is_loading() {
                "loading…".to_owned()
            } else if db.info.empty {
                "empty".to_owned()
            } else {
                format::bytes(db.info.size_on_disk)
            };
            flat.push(
                TreeRow::new(db.info.name.as_str(), 1)
                    .icon(Icon::Database)
                    .secondary(info)
                    .parent(expanded),
                dkey,
                None,
            );
            if !expanded {
                continue;
            }
            let list = match &db.collections {
                Load::Ready(list) => list,
                Load::Failed(err) => {
                    flat.errors.push(format!("{}: {err}", db.info.name));
                    continue;
                }
                _ => continue,
            };
            let buckets: Vec<&str> = list
                .iter()
                .filter(|c| c.info.kind == CollectionKind::GridFsBucket)
                .map(|c| c.info.name.as_str())
                .collect();
            for entry in list {
                // GridFS `.files`/`.chunks` are represented by their bucket.
                let in_bucket = entry
                    .info
                    .name
                    .rsplit_once('.')
                    .is_some_and(|(bucket, suffix)| {
                        matches!(suffix, "files" | "chunks") && buckets.contains(&bucket)
                    });
                if in_bucket
                    || (!filter.is_empty()
                        && !db_matches
                        && !entry.info.name.to_lowercase().contains(&filter))
                {
                    continue;
                }
                let info = entry
                    .stats
                    .as_ref()
                    .map(|s| format::count(s.count))
                    .unwrap_or_else(|| match entry.info.kind {
                        CollectionKind::View => "view".into(),
                        CollectionKind::GridFsBucket => "GridFS".into(),
                        _ => String::new(),
                    });
                let open_name = if entry.info.kind == CollectionKind::GridFsBucket {
                    format!("{}.files", entry.info.name)
                } else {
                    entry.info.name.clone()
                };
                let mut row = TreeRow::new(entry.info.name.as_str(), 2)
                    .icon(collection_icon(entry.info.kind));
                if !info.is_empty() {
                    row = row.secondary(info);
                }
                flat.push(
                    row,
                    NodeKey::Collection(session.id, db.info.name.clone(), entry.info.name.clone()),
                    Some((db.info.name.clone(), open_name, entry.info.kind)),
                );
            }
        }
    }
    flat
}

/// The actions of the node at row `i`, as a menu on the header's "more" button.
fn node_menu(ui: &mut Ui, flat: &Flat, i: usize, actions: &mut Vec<Action>) {
    // A popup is as wide as the window; its items fill it unless told otherwise (see "Kit
    // gaps": menus should size to their items).
    ui.set_max_width(size::SUBMENU_WIDTH);
    let mut item = |ui: &mut Ui, label: &str, icon: Icon, action: Action| {
        if ui.add(MenuItem::new(label).icon(icon)).clicked() {
            actions.push(action);
            ui.close();
        }
    };
    match &flat.keys[i] {
        NodeKey::Session(id) => {
            item(ui, "Refresh", Icon::Refresh, Action::RefreshSession(*id));
            item(ui, "Disconnect", Icon::Unplug, Action::Disconnect(*id));
        }
        NodeKey::Database(id, db) => {
            item(
                ui,
                "Refresh",
                Icon::Refresh,
                Action::RefreshDatabase(*id, db.clone()),
            );
            ui.add(MenuSeparator);
            item(
                ui,
                "Query with SQL",
                Icon::Sql,
                Action::Tool(*id, db.clone(), None, ToolKind::Sql),
            );
            item(
                ui,
                "Query with Shell",
                Icon::Shell,
                Action::Tool(*id, db.clone(), None, ToolKind::Shell),
            );
        }
        NodeKey::Collection(id, db, name) => {
            if let Some((_, _, kind)) = &flat.collections[i] {
                item(
                    ui,
                    "Query",
                    Icon::Collection,
                    Action::Open(*id, db.clone(), name.clone(), *kind),
                );
            }
            ui.add(MenuSeparator);
            for (label, icon, kind) in [
                ("Query with SQL", Icon::Sql, ToolKind::Sql),
                ("Query with Shell", Icon::Shell, ToolKind::Shell),
                ("Aggregation", Icon::Pipeline, ToolKind::Aggregation),
                ("Structure: Schema", Icon::Schema, ToolKind::Schema),
                ("Structure: Indexes", Icon::Index, ToolKind::Indexes),
            ] {
                item(
                    ui,
                    label,
                    icon,
                    Action::Tool(*id, db.clone(), Some(name.clone()), kind),
                );
            }
            ui.add(MenuSeparator);
            if ui
                .add(MenuItem::new("Copy name").icon(Icon::Copy))
                .clicked()
            {
                ui.ctx().copy_text(name.clone());
                ui.close();
            }
        }
    }
}

/// Render the tree panel: a kit panel with the filter and the tree.
/// The header buttons of the Connections tool window, right to left: Connection Manager,
/// refresh, and the selected node's menu.
pub fn header_actions(app: &mut MongoRaabit, ui: &mut Ui) {
    let flat = flatten(app);
    let current = app
        .selected
        .as_ref()
        .and_then(|key| flat.keys.iter().position(|k| k == key));
    let session_ids: Vec<SessionId> = app.sessions.iter().map(|s| s.id).collect();
    let mut open_manager = false;
    let mut actions = Vec::new();
    let more = ui.add(
        IconButton::new(Glyph::More, "Actions for the selected item").enabled(current.is_some()),
    );
    if let Some(i) = current {
        crate::egui::Popup::menu(&more).show(|ui| {
            node_menu(ui, &flat, i, &mut actions);
        });
    }
    if ui
        .add(IconButton::new(Icon::Refresh, "Refresh all"))
        .clicked()
    {
        actions.extend(session_ids.iter().map(|id| Action::RefreshSession(*id)));
    }
    if ui
        .add(IconButton::new(Icon::Plug, "Connection Manager… (Ctrl+N)"))
        .clicked()
    {
        open_manager = true;
    }
    if open_manager {
        app.open_manager();
    }
    for action in actions {
        apply(app, action);
    }
}

/// The body of the Connections tool window: the filter and the tree of connections,
/// databases and collections, or a way to connect when there is none.
pub fn show(app: &mut MongoRaabit, ui: &mut Ui) {
    let flat = flatten(app);
    let mut selected = app
        .selected
        .as_ref()
        .and_then(|key| flat.keys.iter().position(|k| k == key));
    let has_sessions = !app.sessions.is_empty();
    let mut open_manager = false;
    let mut actions = Vec::new();
    ui.spacing_mut().item_spacing.y = size::TOOLBAR_GAP;
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        let filter = ui.add(
            TextField::search(&mut app.tree_filter)
                .name("Filter databases and collections")
                .placeholder("Filter databases and collections")
                .width((ui.available_width() - size::CONTENT_INSET).max(0.0)),
        );
        let _ = filter;
    });
    for error in &flat.errors {
        Banner::new(Severity::Error, error).show(ui);
    }
    if !has_sessions {
        let connect = EmptyState::new("No open connections")
            .icon(Icon::Plug)
            .action("Connect…")
            .show(ui);
        open_manager = connect.clicked();
    } else {
        // Right click opens the node's menu, as the header's "more" button does.
        let menu_actions = std::cell::RefCell::new(Vec::new());
        let (flat_ref, menu_ref) = (&flat, &menu_actions);
        let out = Tree::new("raabit-tree", &flat.rows, &mut selected)
            .name("Connections")
            .height(ui.available_height())
            .show_with(ui, |i, row| {
                row.with(ContextMenu::new(move |ui| {
                    node_menu(ui, flat_ref, i, &mut menu_ref.borrow_mut());
                }))
            });
        actions.extend(menu_actions.into_inner());
        if out.selection_changed
            && let Some(i) = selected
        {
            actions.push(Action::Select(flat.keys[i].clone()));
        }
        if let Some(i) = out.toggled {
            actions.push(Action::Toggle(flat.keys[i].clone()));
        }
        if let Some(i) = out.activated {
            match (&flat.collections[i], &flat.keys[i]) {
                (Some((db, name, kind)), NodeKey::Collection(session, ..)) => {
                    actions.push(Action::Open(*session, db.clone(), name.clone(), *kind));
                }
                _ => actions.push(Action::Toggle(flat.keys[i].clone())),
            }
        }
    }
    if open_manager {
        app.open_manager();
    }
    for action in actions {
        apply(app, action);
    }
}

fn apply(app: &mut MongoRaabit, action: Action) {
    match action {
        Action::Toggle(key) => {
            app.selected = Some(key.clone());
            if !app.expanded.remove(&key) {
                app.expanded.insert(key.clone());
                if let NodeKey::Database(session, db) = &key {
                    let needs_load = app
                        .session(*session)
                        .and_then(|s| s.databases.ready())
                        .and_then(|dbs| dbs.iter().find(|d| &d.info.name == db))
                        .is_some_and(|d| matches!(d.collections, Load::Idle | Load::Failed(_)));
                    if needs_load {
                        app.load_collections(*session, db);
                    }
                }
            }
        }
        Action::Select(key) => app.selected = Some(key),
        Action::Open(session, db, collection, kind) => {
            app.selected = Some(NodeKey::Collection(session, db.clone(), collection.clone()));
            app.open_collection(session, &db, &collection, kind);
        }
        Action::RefreshSession(id) => app.load_databases(id),
        Action::RefreshDatabase(id, db) => app.load_collections(id, &db),
        Action::Disconnect(id) => app.disconnect(id),
        Action::Tool(id, db, collection, kind) => {
            open_tool(app, id, &db, collection.as_deref(), kind)
        }
    }
}

/// Open a tool tab for a database or collection.
pub fn open_tool(
    app: &mut MongoRaabit,
    session: SessionId,
    db: &str,
    collection: Option<&str>,
    kind: ToolKind,
) {
    match (kind, collection) {
        (ToolKind::Sql, coll) => {
            app.open_sql(session, db, coll);
        }
        (ToolKind::Shell, coll) => {
            app.open_shell(session, db, coll);
        }
        (ToolKind::Aggregation, Some(coll)) => {
            app.open_aggregation(session, db, coll, None);
        }
        (ToolKind::Schema, Some(coll)) => {
            app.open_schema(session, db, coll);
        }
        (ToolKind::Indexes, Some(coll)) => {
            app.open_indexes(session, db, coll);
        }
        _ => {}
    }
}
