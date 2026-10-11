//! Connection tree: sessions → databases → collections, views, time series and GridFS.

use crate::app::{Load, MongoRaabit, NodeKey, SessionId, SessionState};
use crate::icons::{self, Icon};
use crate::style::{self, Palette};
use crate::widgets;
use egui::{Align2, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mongo_raabit_core::connection::CollectionKind;

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

const ROW: f32 = 24.0;
const INDENT: f32 = 16.0;

struct Row<'a> {
    depth: usize,
    icon: Icon,
    icon_color: egui::Color32,
    label: &'a str,
    info: String,
    expandable: bool,
    expanded: bool,
    selected: bool,
    dim: bool,
    loading: bool,
}

fn row(ui: &mut Ui, p: &Palette, r: Row<'_>) -> (egui::Response, bool) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
    let painter = ui.painter();
    if r.selected {
        painter.rect_filled(
            rect.shrink2(vec2(4.0, 1.0)),
            CornerRadius::same(6),
            p.selection,
        );
    } else if response.hovered() {
        painter.rect_filled(rect.shrink2(vec2(4.0, 1.0)), CornerRadius::same(6), p.hover);
    }
    let x = rect.left() + 8.0 + r.depth as f32 * INDENT;
    let chevron = Rect::from_center_size(pos2(x + 6.0, rect.center().y), vec2(12.0, 12.0));
    let mut toggled = false;
    if r.expandable {
        icons::paint(
            painter,
            chevron,
            if r.expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            p.text_info,
        );
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && pos.x < chevron.right() + 2.0
        {
            toggled = true;
        }
    }
    icons::paint(
        painter,
        Rect::from_center_size(pos2(x + 22.0, rect.center().y), vec2(15.0, 15.0)),
        r.icon,
        r.icon_color,
    );
    let text_color = if r.dim { p.text_info } else { p.text };
    let info_galley =
        painter.layout_no_wrap(r.info.clone(), FontId::proportional(11.5), p.text_info);
    let label_max = (rect.right() - 12.0 - info_galley.size().x - 8.0) - (x + 34.0);
    let label = crate::values::truncate(r.label, ((label_max / 7.0).max(4.0)) as usize);
    painter.text(
        pos2(x + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        text_color,
    );
    if r.loading {
        let c = pos2(rect.right() - 16.0, rect.center().y);
        let t = ui.input(|i| i.time) as f32;
        painter.add(egui::Shape::line(
            (0..=12)
                .map(|i| {
                    let a = t * 6.0 + i as f32 * 0.4;
                    c + vec2(a.cos(), a.sin()) * 5.0
                })
                .collect(),
            Stroke::new(1.5_f32, p.accent),
        ));
        ui.ctx().request_repaint();
    } else {
        painter.galley(
            pos2(
                rect.right() - 12.0 - info_galley.size().x,
                rect.center().y - info_galley.size().y / 2.0,
            ),
            info_galley,
            p.text_info,
        );
    }
    (response, toggled)
}

fn collection_icon(kind: CollectionKind) -> Icon {
    match kind {
        CollectionKind::Collection | CollectionKind::System => Icon::Collection,
        CollectionKind::View => Icon::View,
        CollectionKind::TimeSeries => Icon::TimeSeries,
        CollectionKind::GridFsBucket => Icon::Bucket,
    }
}

/// Render the tree panel content.
pub fn show(app: &mut MongoRaabit, ui: &mut Ui) {
    let p = Palette::of(ui);
    let mut actions = Vec::new();
    // Header.
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 6,
            top: 8,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Connections").strong().color(p.text_strong));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if widgets::icon_button(ui, Icon::Refresh, "Refresh all").clicked() {
                        for s in &app.sessions {
                            actions.push(Action::RefreshSession(s.id));
                        }
                    }
                    if widgets::icon_button_tinted(
                        ui,
                        Icon::Plug,
                        "Connection Manager… (Ctrl+N)",
                        Some(p.leaf),
                        true,
                    )
                    .clicked()
                    {
                        app.open_manager();
                    }
                });
            });
        });
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 0))
        .show(ui, |ui| {
            egui::Frame::new()
                .fill(p.field)
                .stroke(Stroke::new(1.0_f32, p.border))
                .corner_radius(CornerRadius::same(7))
                .inner_margin(egui::Margin::symmetric(8, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        icons::icon(ui, Icon::Search, p.text_info);
                        ui.add(
                            egui::TextEdit::singleline(&mut app.tree_filter)
                                .hint_text(widgets::hint(ui, "Filter databases and collections"))
                                .frame(false)
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
        });
    ui.add_space(6.0);
    let filter = app.tree_filter.to_lowercase();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if app.sessions.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    icons::icon(ui, Icon::Leaf, p.leaf.gamma_multiply(0.7));
                    ui.add_space(6.0);
                    ui.label(RichText::new("No open connections").color(p.text_secondary));
                    ui.add_space(8.0);
                    if widgets::primary_button(ui, Some(Icon::Plug), "Connect…").clicked() {
                        app.open_manager();
                    }
                });
            }
            for session in &app.sessions {
                let skey = NodeKey::Session(session.id);
                let expanded = app.expanded.contains(&skey);
                let (icon_color, info, loading) = match &session.state {
                    SessionState::Connecting => (p.warning, String::new(), true),
                    SessionState::Connected(conn) => {
                        (p.leaf, format!("v{}", conn.server().version), false)
                    }
                    SessionState::Failed(_) => (p.error, "failed".into(), false),
                };
                let (response, toggled) = row(
                    ui,
                    &p,
                    Row {
                        depth: 0,
                        icon: Icon::Server,
                        icon_color,
                        label: &session.profile.name,
                        info,
                        expandable: true,
                        expanded,
                        selected: app.selected.as_ref() == Some(&skey),
                        dim: false,
                        loading: loading || session.databases.is_loading(),
                    },
                );
                let response = response.on_hover_text(session.profile.summary());
                if toggled || response.double_clicked() {
                    actions.push(Action::Toggle(skey.clone()));
                } else if response.clicked() {
                    actions.push(Action::Select(skey.clone()));
                }
                response.context_menu(|ui| {
                    if ui.button("Refresh").clicked() {
                        actions.push(Action::RefreshSession(session.id));
                        ui.close();
                    }
                    if ui.button("Disconnect").clicked() {
                        actions.push(Action::Disconnect(session.id));
                        ui.close();
                    }
                });
                if !expanded {
                    continue;
                }
                if let SessionState::Failed(err) = &session.state {
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 40,
                            right: 8,
                            top: 2,
                            bottom: 6,
                        })
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(err).size(11.5).color(p.error))
                                    .wrap(),
                            );
                        });
                }
                let Load::Ready(databases) = &session.databases else {
                    if let Load::Failed(err) = &session.databases {
                        egui::Frame::new()
                            .inner_margin(egui::Margin {
                                left: 40,
                                right: 8,
                                top: 2,
                                bottom: 6,
                            })
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(err).size(11.5).color(p.error))
                                        .wrap(),
                                );
                            });
                    }
                    continue;
                };
                for db in databases {
                    let dkey = NodeKey::Database(session.id, db.info.name.clone());
                    let collections = db.collections.ready();
                    let db_matches =
                        filter.is_empty() || db.info.name.to_lowercase().contains(&filter);
                    let child_matches = collections.is_some_and(|c| {
                        c.iter()
                            .any(|c| c.info.name.to_lowercase().contains(&filter))
                    });
                    if !db_matches && !child_matches {
                        continue;
                    }
                    let expanded =
                        app.expanded.contains(&dkey) || (!filter.is_empty() && child_matches);
                    let (response, toggled) = row(
                        ui,
                        &p,
                        Row {
                            depth: 1,
                            icon: Icon::Database,
                            icon_color: p.accent_text,
                            label: &db.info.name,
                            info: if db.info.empty {
                                "empty".into()
                            } else {
                                style::bytes(db.info.size_on_disk)
                            },
                            expandable: true,
                            expanded,
                            selected: app.selected.as_ref() == Some(&dkey),
                            dim: matches!(db.info.name.as_str(), "admin" | "config" | "local"),
                            loading: db.collections.is_loading(),
                        },
                    );
                    if toggled || response.double_clicked() {
                        actions.push(Action::Toggle(dkey.clone()));
                    } else if response.clicked() {
                        actions.push(Action::Select(dkey.clone()));
                    }
                    response.context_menu(|ui| {
                        if ui.button("Refresh").clicked() {
                            actions.push(Action::RefreshDatabase(session.id, db.info.name.clone()));
                            ui.close();
                        }
                    });
                    if !expanded {
                        continue;
                    }
                    match &db.collections {
                        Load::Ready(list) => {
                            if list.is_empty() {
                                egui::Frame::new()
                                    .inner_margin(egui::Margin {
                                        left: 64,
                                        right: 8,
                                        top: 2,
                                        bottom: 4,
                                    })
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new("No collections")
                                                .size(11.5)
                                                .color(p.text_info),
                                        );
                                    });
                            }
                            let buckets: Vec<&str> = list
                                .iter()
                                .filter(|c| c.info.kind == CollectionKind::GridFsBucket)
                                .map(|c| c.info.name.as_str())
                                .collect();
                            for entry in list {
                                // GridFS `.files`/`.chunks` are represented by their bucket.
                                let in_bucket = entry.info.name.rsplit_once('.').is_some_and(
                                    |(bucket, suffix)| {
                                        matches!(suffix, "files" | "chunks")
                                            && buckets.contains(&bucket)
                                    },
                                );
                                if in_bucket {
                                    continue;
                                }
                                if !filter.is_empty()
                                    && !db_matches
                                    && !entry.info.name.to_lowercase().contains(&filter)
                                {
                                    continue;
                                }
                                let ckey = NodeKey::Collection(
                                    session.id,
                                    db.info.name.clone(),
                                    entry.info.name.clone(),
                                );
                                let info = entry
                                    .stats
                                    .as_ref()
                                    .map(|s| style::count(s.count))
                                    .unwrap_or_else(|| match entry.info.kind {
                                        CollectionKind::View => "view".into(),
                                        CollectionKind::GridFsBucket => "GridFS".into(),
                                        _ => String::new(),
                                    });
                                let (response, _) = row(
                                    ui,
                                    &p,
                                    Row {
                                        depth: 2,
                                        icon: collection_icon(entry.info.kind),
                                        icon_color: match entry.info.kind {
                                            CollectionKind::View => p.keyword,
                                            CollectionKind::TimeSeries => p.date,
                                            CollectionKind::GridFsBucket => p.object_id,
                                            _ => p.text_secondary,
                                        },
                                        label: &entry.info.name,
                                        info,
                                        expandable: false,
                                        expanded: false,
                                        selected: app.selected.as_ref() == Some(&ckey),
                                        dim: entry.info.kind == CollectionKind::System,
                                        loading: false,
                                    },
                                );
                                let response = response.on_hover_ui(|ui| {
                                    ui.label(RichText::new(&entry.info.name).strong());
                                    if let Some(stats) = &entry.stats {
                                        ui.label(format!(
                                            "{} documents · {} · avg {}",
                                            stats.count,
                                            style::bytes(stats.size),
                                            style::bytes(stats.avg_obj_size)
                                        ));
                                        ui.label(format!(
                                            "{} indexes · {}",
                                            stats.index_count,
                                            style::bytes(stats.total_index_size)
                                        ));
                                    }
                                    if let Some(source) = &entry.info.view_on {
                                        ui.label(format!(
                                            "View on {source} · {} stages",
                                            entry.info.pipeline.len()
                                        ));
                                    }
                                });
                                if response.double_clicked() {
                                    let open_name =
                                        if entry.info.kind == CollectionKind::GridFsBucket {
                                            format!("{}.files", entry.info.name)
                                        } else {
                                            entry.info.name.clone()
                                        };
                                    actions.push(Action::Open(
                                        session.id,
                                        db.info.name.clone(),
                                        open_name,
                                        entry.info.kind,
                                    ));
                                } else if response.clicked() {
                                    actions.push(Action::Select(ckey.clone()));
                                }
                                response.context_menu(|ui| {
                                    if ui.button("Query").clicked() {
                                        actions.push(Action::Open(
                                            session.id,
                                            db.info.name.clone(),
                                            entry.info.name.clone(),
                                            entry.info.kind,
                                        ));
                                        ui.close();
                                    }
                                    ui.separator();
                                    for (label, kind) in [
                                        ("Query with SQL", ToolKind::Sql),
                                        ("Query with Shell", ToolKind::Shell),
                                        ("Aggregation", ToolKind::Aggregation),
                                        ("Structure: Schema", ToolKind::Schema),
                                        ("Structure: Indexes", ToolKind::Indexes),
                                    ] {
                                        if ui.button(label).clicked() {
                                            actions.push(Action::Tool(
                                                session.id,
                                                db.info.name.clone(),
                                                Some(entry.info.name.clone()),
                                                kind,
                                            ));
                                            ui.close();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("Copy name").clicked() {
                                        ui.ctx().copy_text(entry.info.name.clone());
                                        ui.close();
                                    }
                                });
                            }
                        }
                        Load::Failed(err) => {
                            egui::Frame::new()
                                .inner_margin(egui::Margin {
                                    left: 64,
                                    right: 8,
                                    top: 2,
                                    bottom: 4,
                                })
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(err).size(11.5).color(p.error),
                                        )
                                        .wrap(),
                                    );
                                });
                        }
                        _ => {}
                    }
                }
            }
        });
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
