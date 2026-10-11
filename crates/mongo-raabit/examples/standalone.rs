//! Standalone mongo-raabit window, used for development and screenshot evidence until
//! `jetthing --app dev.jetthing.mongo-raabit` hosts the plugin.
//!
//! ```text
//! cargo run -p mongo-raabit --example standalone -- [--light] [--uri mongodb://…]
//!     [--open db.collection] [--view table|tree|json] [--drill field] [--manager]
//! ```

use mongo_raabit::MongoRaabit;
use mongo_raabit::app::{NodeKey, ResultView, TabKind};
use mongo_raabit::core::profile::ConnectionProfile;

struct Args {
    light: bool,
    uri: Option<String>,
    open: Option<(String, String)>,
    view: ResultView,
    drill: Vec<String>,
    manager: bool,
    filter: Option<String>,
    demo: bool,
    dialog: Option<String>,
    edit_cell: Option<String>,
    tool: Option<String>,
    text: Option<String>,
    stage: Option<usize>,
}

fn parse_args() -> Args {
    let mut args = Args {
        light: false,
        uri: None,
        open: None,
        view: ResultView::Table,
        drill: Vec::new(),
        manager: false,
        filter: None,
        demo: false,
        dialog: None,
        edit_cell: None,
        tool: None,
        text: None,
        stage: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--light" => args.light = true,
            "--manager" => args.manager = true,
            "--demo-profiles" => args.demo = true,
            "--uri" => args.uri = it.next(),
            "--filter" => args.filter = it.next(),
            "--dialog" => args.dialog = it.next(),
            "--edit-cell" => args.edit_cell = it.next(),
            "--tool" => args.tool = it.next(),
            "--text" => args.text = it.next(),
            "--stage" => args.stage = it.next().and_then(|s| s.parse().ok()),
            "--drill" => args.drill.extend(it.next()),
            "--open" => {
                args.open = it.next().and_then(|v| {
                    v.split_once('.')
                        .map(|(db, coll)| (db.to_owned(), coll.to_owned()))
                });
            }
            "--view" => {
                args.view = match it.next().as_deref() {
                    Some("tree") => ResultView::Tree,
                    Some("json") => ResultView::Json,
                    _ => ResultView::Table,
                }
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    args
}

struct App {
    raabit: MongoRaabit,
    args: Args,
    scripted: bool,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.scripted {
            self.scripted = true;
            self.script();
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ctx, |ui| self.raabit.ui(ui));
    }
}

impl App {
    /// Drive the app into the requested state (blocking on background loads).
    fn script(&mut self) {
        let wait = |r: &mut MongoRaabit| r.wait_idle(std::time::Duration::from_secs(10));
        if let Some(uri) = self.args.uri.clone() {
            match ConnectionProfile::from_uri(&uri) {
                Ok((mut profile, _)) => {
                    profile.name = "Local MongoDB".into();
                    let id = self.raabit.connect(profile);
                    wait(&mut self.raabit);
                    if let Some((db, coll)) = self.args.open.clone() {
                        self.raabit
                            .expanded
                            .insert(NodeKey::Database(id, db.clone()));
                        self.raabit.load_collections(id, &db);
                        wait(&mut self.raabit);
                        self.raabit.open_collection(
                            id,
                            &db,
                            &coll,
                            mongo_raabit::core::connection::CollectionKind::Collection,
                        );
                        self.raabit.selected = Some(NodeKey::Collection(id, db, coll));
                        if let Some(filter) = self.args.filter.clone()
                            && let Some(tab_id) = self.raabit.active_tab_id()
                        {
                            if let Some(tab) = self.raabit.collection_tab_mut(tab_id) {
                                tab.query.filter = filter;
                            }
                            self.raabit.run_query(tab_id, true);
                        }
                        wait(&mut self.raabit);
                        let view = self.args.view;
                        let drill = self.args.drill.clone();
                        if let Some(tab) = self.raabit.tabs.last_mut()
                            && let TabKind::Collection(tab) = &mut tab.kind
                        {
                            tab.view = view;
                            tab.drill = drill;
                            if view == ResultView::Tree {
                                tab.open.insert("0".into());
                                tab.open.insert("0/address".into());
                                tab.open.insert("1".into());
                            }
                        }
                    }
                }
                Err(err) => eprintln!("invalid --uri: {err}"),
            }
        }
        if let Some(tab_id) = self.raabit.active_tab_id()
            && self.raabit.collection_tab_mut(tab_id).is_some()
        {
            if let Some(tab) = self.raabit.collection_tab_mut(tab_id) {
                tab.selected = Some(0);
            }
            if let Some(field) = self.args.edit_cell.clone()
                && let Some(doc) = self.raabit.selected_document(tab_id)
                && let Some(tab) = self.raabit.collection_tab_mut(tab_id)
            {
                let value = doc.get(&field).cloned();
                tab.editing = Some(mongo_raabit::app::CellEdit {
                    doc_index: 0,
                    path: field,
                    text: match &value {
                        Some(mongo_raabit::core::bson::Bson::String(s)) => s.clone(),
                        _ => String::new(),
                    },
                    original: value,
                    focus: true,
                });
            }
            match self.args.dialog.as_deref() {
                Some("edit") => self.raabit.edit_selected(tab_id),
                Some("insert") => self.raabit.add_document(tab_id),
                Some("delete") => self.raabit.request_delete(tab_id),
                Some("explain") => self.raabit.explain(tab_id),
                Some("code") => self.raabit.query_code(tab_id),
                _ => {}
            }
            wait(&mut self.raabit);
            if let Some(code) = std::env::var("RAABIT_CODE_LANGUAGE").ok()
                && let Some(mongo_raabit::dialogs::Dialog::Code(dialog)) = &mut self.raabit.dialog
            {
                dialog.language = mongo_raabit::core::codegen::Language::ALL
                    .into_iter()
                    .find(|l| format!("{l:?}").eq_ignore_ascii_case(&code));
            }
        }
        if let Some(tool) = self.args.tool.clone()
            && let Some(session) = self.raabit.sessions.first().map(|s| s.id)
            && let Some((db, coll)) = self.args.open.clone()
        {
            let text = self.args.text.clone();
            match tool.as_str() {
                "aggregate" => {
                    let pipeline = text
                        .and_then(|t| mongo_raabit::core::aggregate::Pipeline::from_text(&t).ok());
                    let id = self.raabit.open_aggregation(session, &db, &coll, pipeline);
                    if let Some(stage) = self.args.stage
                        && let Some(tab) = self.raabit.aggregation_tab_mut(id)
                    {
                        tab.selected = stage;
                    }
                    self.raabit.preview_stage(id);
                    wait(&mut self.raabit);
                    if std::env::var("RAABIT_RUN").is_ok() {
                        self.raabit.run_pipeline(id);
                    }
                }
                "sql" => {
                    if let Some(id) = self.raabit.open_sql(session, &db, Some(&coll)) {
                        if let Some(t) = text
                            && let Some(tab) = self.raabit.sql_tab_mut(id)
                        {
                            tab.sql = t;
                        }
                        self.raabit.run_sql(id);
                    }
                }
                "schema" => {
                    let id = self.raabit.open_schema(session, &db, &coll);
                    wait(&mut self.raabit);
                    if let Some(field) = text
                        && let Some(tab) = self.raabit.schema_tab_mut(id)
                        && let mongo_raabit::app::Load::Ready(report) = &tab.report
                    {
                        tab.selected = report.fields.iter().position(|f| f.path == field);
                    }
                }
                "indexes" => {
                    let id = self.raabit.open_indexes(session, &db, &coll);
                    if text.is_some()
                        && let Some(tab) = self.raabit.index_tab_mut(id)
                    {
                        tab.form_open = true;
                        tab.form.keys = "{ status: 1, createdAt: -1 }".into();
                        tab.form.partial = "{ status: { $ne: \"delivered\" } }".into();
                    }
                }
                "shell" => {
                    if let Some(id) = self.raabit.open_shell(session, &db, Some(&coll)) {
                        wait(&mut self.raabit);
                        if let Some(t) = text
                            && let Some(tab) = self.raabit.shell_tab_mut(id)
                        {
                            tab.script = t;
                            tab.force_builtin = std::env::var("RAABIT_BUILTIN").is_ok();
                        }
                        self.raabit.run_shell(id, None);
                    }
                }
                other => eprintln!("unknown --tool {other}"),
            }
            wait(&mut self.raabit);
        }
        if self.args.demo {
            for (uri, name, folder) in [
                ("mongodb://127.0.0.1:27017/raabit_shop", "Local MongoDB", ""),
                (
                    "mongodb://app:secret@db-eu-1:27017,db-eu-2:27017,db-eu-3:27017/shop?replicaSet=rs-eu&authSource=admin&tls=true",
                    "Shop (EU primary)",
                    "Production/EU",
                ),
                (
                    "mongodb://reporting@mongos-1:27017,mongos-2:27017/?readPreference=secondaryPreferred",
                    "Reporting cluster",
                    "Production/EU",
                ),
                (
                    "mongodb+srv://analyst:pw@cluster0.example.net/analytics",
                    "Analytics (Atlas)",
                    "Cloud",
                ),
                (
                    "mongodb://10.0.4.12:27017/",
                    "Staging via bastion",
                    "Staging",
                ),
            ] {
                if let Ok((mut profile, _)) = ConnectionProfile::from_uri(uri) {
                    profile.name = name.into();
                    profile.folder = folder.into();
                    if name.contains("bastion") {
                        profile.ssh = Some(mongo_raabit::core::profile::SshSettings {
                            host: "bastion.example.com".into(),
                            username: "ops".into(),
                            ..Default::default()
                        });
                    }
                    self.raabit.store.upsert(profile);
                }
            }
        }
        if self.args.manager {
            self.raabit.open_manager();
        }
    }
}

fn main() -> eframe::Result {
    let args = parse_args();
    let renderer = if std::env::var("JETTHING_RENDERER").is_ok_and(|v| v == "glow") {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mongo-raabit")
            .with_inner_size([1440.0, 900.0]),
        renderer,
        ..Default::default()
    };
    let light = args.light;
    eframe::run_native(
        "mongo-raabit",
        options,
        Box::new(move |cc| {
            jetthing_ui::install_theme(&cc.egui_ctx);
            if light {
                cc.egui_ctx.style_mut(|style| {
                    let p = mongo_raabit::style::Palette::LIGHT;
                    style.visuals = egui::Visuals::light();
                    style.visuals.override_text_color = Some(p.text);
                    style.visuals.panel_fill = p.frame;
                    style.visuals.window_fill = p.island;
                    style.visuals.extreme_bg_color = p.field;
                    style.visuals.selection.bg_fill = p.selection;
                    style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
                    style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
                    style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
                });
            }
            Ok(Box::new(App {
                raabit: MongoRaabit::new(None),
                args,
                scripted: false,
            }))
        }),
    )
}
