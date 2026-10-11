//! Connection Manager: saved connections in folders, an editor for server, auth, TLS,
//! SSH and advanced settings, URI import and Test Connection.

use crate::app::{Load, MongoRaabit};
use crate::icons::Icon;
use crate::style::{self, Palette};
use crate::widgets;
use egui::{Align2, Color32, CornerRadius, FontId, RichText, Stroke, Ui, vec2};
use mongo_raabit_core::Connection;
use mongo_raabit_core::connection::ServerInfo;
use mongo_raabit_core::profile::{
    AuthMechanism, ConnectionProfile, HostPort, ProfileStore, ReadPreference, SshAuth, SshSettings,
    Target,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Server,
    Authentication,
    Tls,
    Ssh,
    Advanced,
}

impl Page {
    const ALL: [(Self, &'static str); 5] = [
        (Self::Server, "Server"),
        (Self::Authentication, "Authentication"),
        (Self::Tls, "TLS / SSL"),
        (Self::Ssh, "SSH Tunnel"),
        (Self::Advanced, "Advanced"),
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Standalone,
    ReplicaSet,
    Sharded,
    Srv,
}

impl TargetKind {
    const ALL: [Self; 4] = [Self::Standalone, Self::ReplicaSet, Self::Sharded, Self::Srv];

    fn label(self) -> &'static str {
        match self {
            Self::Standalone => "Standalone",
            Self::ReplicaSet => "Replica set",
            Self::Sharded => "Sharded cluster",
            Self::Srv => "DNS seed list (SRV)",
        }
    }
}

pub struct ConnectionManager {
    pub selected: Option<String>,
    pub draft: ConnectionProfile,
    pub page: Page,
    target_kind: TargetKind,
    host: String,
    port: String,
    hosts_text: String,
    set_name: String,
    password: String,
    ssh_enabled: bool,
    ssh: SshSettings,
    ssh_key: String,
    ssh_password: String,
    ca_file: String,
    cert_file: String,
    timeout: String,
    pub uri_import: Option<String>,
    pub test: Load<ServerInfo>,
    pub error: Option<String>,
    filter: String,
    close: bool,
}

impl ConnectionManager {
    pub fn new(store: &ProfileStore) -> Self {
        let first = store
            .profiles_in("")
            .into_iter()
            .chain(
                store
                    .folder_paths()
                    .iter()
                    .flat_map(|f| store.profiles_in(f)),
            )
            .next()
            .cloned();
        let mut manager = Self::from_profile(first.clone().unwrap_or_default());
        manager.selected = first.map(|p| p.id);
        manager
    }

    fn from_profile(profile: ConnectionProfile) -> Self {
        let mut m = Self {
            selected: None,
            page: Page::Server,
            target_kind: TargetKind::Standalone,
            host: "localhost".into(),
            port: "27017".into(),
            hosts_text: String::new(),
            set_name: String::new(),
            password: String::new(),
            ssh_enabled: profile.ssh.is_some(),
            ssh: profile.ssh.clone().unwrap_or_default(),
            ssh_key: String::new(),
            ssh_password: String::new(),
            ca_file: profile
                .tls
                .ca_file
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            cert_file: profile
                .tls
                .certificate_key_file
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            timeout: profile
                .connect_timeout_ms
                .map(|t| t.to_string())
                .unwrap_or_default(),
            uri_import: None,
            test: Load::Idle,
            error: None,
            filter: String::new(),
            close: false,
            draft: profile,
        };
        if let SshAuth::PrivateKey { path } = &m.ssh.auth {
            m.ssh_key = path.display().to_string();
        }
        match &m.draft.target {
            Target::Standalone { server } => {
                m.host = server.host.clone();
                m.port = server.port.to_string();
            }
            Target::ReplicaSet { members, set_name } => {
                m.target_kind = TargetKind::ReplicaSet;
                m.hosts_text = members
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                m.set_name = set_name.clone();
            }
            Target::Sharded { routers } => {
                m.target_kind = TargetKind::Sharded;
                m.hosts_text = routers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            Target::Srv { host } => {
                m.target_kind = TargetKind::Srv;
                m.host = host.clone();
            }
        }
        m
    }

    /// Build the profile from the form fields.
    pub fn profile(&self) -> Result<ConnectionProfile, String> {
        let mut profile = self.draft.clone();
        let hosts = || -> Result<Vec<HostPort>, String> {
            let list = self
                .hosts_text
                .split(['\n', ','])
                .filter(|l| !l.trim().is_empty())
                .map(HostPort::parse)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            if list.is_empty() {
                Err("add at least one host:port".into())
            } else {
                Ok(list)
            }
        };
        profile.target = match self.target_kind {
            TargetKind::Standalone => {
                let port = self
                    .port
                    .trim()
                    .parse::<u16>()
                    .map_err(|_| format!("invalid port '{}'", self.port))?;
                Target::Standalone {
                    server: HostPort::new(self.host.trim(), port),
                }
            }
            TargetKind::ReplicaSet => Target::ReplicaSet {
                members: hosts()?,
                set_name: self.set_name.trim().to_owned(),
            },
            TargetKind::Sharded => Target::Sharded { routers: hosts()? },
            TargetKind::Srv => Target::Srv {
                host: self.host.trim().to_owned(),
            },
        };
        profile.tls.ca_file = (!self.ca_file.trim().is_empty()).then(|| self.ca_file.trim().into());
        profile.tls.certificate_key_file =
            (!self.cert_file.trim().is_empty()).then(|| self.cert_file.trim().into());
        profile.connect_timeout_ms = if self.timeout.trim().is_empty() {
            None
        } else {
            Some(
                self.timeout
                    .trim()
                    .parse()
                    .map_err(|_| "timeout must be milliseconds".to_owned())?,
            )
        };
        profile.ssh = if self.ssh_enabled {
            let mut ssh = self.ssh.clone();
            if let SshAuth::PrivateKey { .. } = ssh.auth {
                ssh.auth = SshAuth::PrivateKey {
                    path: self.ssh_key.trim().into(),
                };
            }
            Some(ssh)
        } else {
            None
        };
        profile.validate().map_err(|e| e.to_string())?;
        Ok(profile)
    }

    fn select(
        &mut self,
        profile: &ConnectionProfile,
        app_secrets: &dyn mongo_raabit_core::secrets::SecretStore,
    ) {
        let mut next = Self::from_profile(profile.clone());
        next.selected = Some(profile.id.clone());
        next.password = app_secrets
            .get(&profile.password_key())
            .ok()
            .flatten()
            .unwrap_or_default();
        next.ssh_password = app_secrets
            .get(&profile.ssh_password_key())
            .ok()
            .flatten()
            .unwrap_or_default();
        next.filter = std::mem::take(&mut self.filter);
        *self = next;
    }
}

/// Render the dialog while `app.manager` is set; closing clears it.
pub fn show(app: &mut MongoRaabit, ctx: &egui::Context) {
    let Some(mut manager) = app.manager.take() else {
        return;
    };
    let p = Palette::for_dark(ctx.style().visuals.dark_mode);
    let mut open = true;
    let screen = ctx.content_rect();
    let size = vec2(
        880.0_f32.min(screen.width() - 40.0),
        600.0_f32.min(screen.height() - 40.0),
    );
    let frame = egui::Frame::new()
        .fill(p.island)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(12))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 120 } else { 40 }),
        });
    let response = egui::Modal::new(egui::Id::new("raabit-manager"))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(if p.dark { 110 } else { 60 }))
        .show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            dialog(app, &mut manager, ui, &p, size, &mut open);
        });
    if response.should_close() {
        open = false;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    if open {
        app.manager = Some(manager);
    }
}

fn dialog(
    app: &mut MongoRaabit,
    m: &mut ConnectionManager,
    ui: &mut Ui,
    p: &Palette,
    size: egui::Vec2,
    open: &mut bool,
) {
    // Title bar.
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 16,
            right: 10,
            top: 12,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                crate::icons::icon(ui, Icon::Leaf, p.leaf);
                ui.label(
                    RichText::new("Connection Manager")
                        .size(15.0)
                        .strong()
                        .color(p.text_strong),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::Close, "Close (Esc)").clicked() {
                        *open = false;
                    }
                });
            });
        });
    widgets::separator(ui);
    let body_height = size.y - 44.0 - 58.0;
    ui.allocate_ui_with_layout(
        vec2(size.x, body_height),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // ---- left: saved connections
            egui::Frame::new()
                .fill(p.raised)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(250.0);
                    ui.set_height(body_height - 20.0);
                    ui.vertical(|ui| connection_list(app, m, ui, p));
                });
            let (sep, _) = ui.allocate_exact_size(vec2(1.0, body_height), egui::Sense::hover());
            ui.painter().rect_filled(sep, 0.0, p.separator);
            // ---- right: editor
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 18,
                    right: 18,
                    top: 12,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    let width = size.x - 271.0 - 36.0;
                    ui.set_width(width);
                    ui.set_height(body_height - 20.0);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        editor(app, m, ui, p, body_height - 20.0);
                    });
                });
        },
    );
    widgets::separator(ui);
    footer(app, m, ui, p, open);
}

fn connection_list(app: &mut MongoRaabit, m: &mut ConnectionManager, ui: &mut Ui, p: &Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if widgets::icon_button(ui, Icon::Plus, "New connection").clicked() {
            let mut fresh = ConnectionManager::from_profile(ConnectionProfile::default());
            fresh.filter = std::mem::take(&mut m.filter);
            *m = fresh;
        }
        if widgets::icon_button(ui, Icon::Import, "Import from connection string").clicked() {
            m.uri_import = Some(String::new());
        }
        let has_selection = m.selected.is_some();
        if widgets::icon_button_tinted(ui, Icon::Copy, "Duplicate", None, has_selection).clicked()
            && let Some(id) = m.selected.clone()
            && let Some(new_id) = app.store.duplicate(&id)
        {
            app.save_store();
            if let Some(profile) = app.store.get(&new_id).cloned() {
                m.select(&profile, app.secrets.as_ref());
            }
        }
        if widgets::icon_button_tinted(ui, Icon::Trash, "Delete", Some(p.error), has_selection)
            .clicked()
            && let Some(id) = m.selected.clone()
        {
            if let Some(profile) = app.store.remove(&id) {
                let _ = app.secrets.delete(&profile.password_key());
                let _ = app.secrets.delete(&profile.ssh_password_key());
            }
            app.save_store();
            *m = ConnectionManager::new(&app.store);
        }
    });
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                crate::icons::icon(ui, Icon::Search, p.text_info);
                ui.add(
                    egui::TextEdit::singleline(&mut m.filter)
                        .hint_text(widgets::hint(ui, "Filter connections"))
                        .frame(false)
                        .desired_width(f32::INFINITY),
                );
            });
        });
    ui.add_space(8.0);
    let filter = m.filter.to_lowercase();
    let mut clicked: Option<ConnectionProfile> = None;
    let mut connect: Option<ConnectionProfile> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut folders = vec![String::new()];
            folders.extend(app.store.folder_paths());
            for folder in folders {
                let profiles: Vec<&ConnectionProfile> = app
                    .store
                    .profiles_in(&folder)
                    .into_iter()
                    .filter(|pr| {
                        filter.is_empty()
                            || pr.name.to_lowercase().contains(&filter)
                            || pr.summary().to_lowercase().contains(&filter)
                    })
                    .collect();
                if profiles.is_empty() && !folder.is_empty() && !filter.is_empty() {
                    continue;
                }
                let depth = if folder.is_empty() {
                    0
                } else {
                    folder.matches('/').count() + 1
                };
                if !folder.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0 + 12.0 * (depth as f32 - 1.0));
                        crate::icons::icon(ui, Icon::Folder, p.text_secondary);
                        ui.label(
                            RichText::new(folder.rsplit('/').next().unwrap_or(&folder))
                                .color(p.text_secondary),
                        );
                    });
                }
                for profile in profiles {
                    let selected = m.selected.as_deref() == Some(profile.id.as_str());
                    let (rect, response) = ui.allocate_exact_size(
                        vec2(ui.available_width(), 40.0),
                        egui::Sense::click(),
                    );
                    if selected {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(7), p.selection);
                    } else if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(7), p.hover);
                    }
                    let x = rect.left() + 10.0 + 12.0 * depth as f32;
                    crate::icons::paint(
                        ui.painter(),
                        egui::Rect::from_center_size(
                            egui::pos2(x + 8.0, rect.center().y),
                            vec2(16.0, 16.0),
                        ),
                        Icon::Leaf,
                        p.leaf,
                    );
                    ui.painter().text(
                        egui::pos2(x + 24.0, rect.top() + 12.0),
                        Align2::LEFT_CENTER,
                        &profile.name,
                        FontId::proportional(13.0),
                        p.text_strong,
                    );
                    ui.painter().text(
                        egui::pos2(x + 24.0, rect.top() + 28.0),
                        Align2::LEFT_CENTER,
                        crate::values::truncate(&profile.summary(), 30),
                        FontId::proportional(11.0),
                        p.text_info,
                    );
                    if response.double_clicked() {
                        connect = Some(profile.clone());
                    } else if response.clicked() {
                        clicked = Some(profile.clone());
                    }
                }
            }
            if app.store.profiles.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No saved connections").color(p.text_secondary));
                    ui.label(
                        RichText::new("Create one with + or import a URI")
                            .size(11.5)
                            .color(p.text_info),
                    );
                });
            }
        });
    if let Some(profile) = clicked {
        m.select(&profile, app.secrets.as_ref());
    }
    if let Some(profile) = connect {
        m.select(&profile, app.secrets.as_ref());
        app.connect(profile);
        m.close = true;
    }
}

fn tab_strip(ui: &mut Ui, m: &mut ConnectionManager, p: &Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        for (page, label) in Page::ALL {
            let selected = m.page == page;
            let color = if selected {
                p.text_strong
            } else {
                p.text_secondary
            };
            let galley =
                ui.painter()
                    .layout_no_wrap(label.to_owned(), FontId::proportional(13.0), color);
            let (rect, response) =
                ui.allocate_exact_size(galley.size() + vec2(0.0, 12.0), egui::Sense::click());
            ui.painter().galley(rect.min, galley, color);
            if selected {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(rect.left(), rect.bottom() - 2.5),
                        vec2(rect.width(), 2.5),
                    ),
                    CornerRadius::same(1),
                    p.accent,
                );
            } else if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                m.page = page;
            }
        }
    });
    widgets::separator(ui);
}

fn text_row(ui: &mut Ui, label: &str, value: &mut String, hint: &str, width: f32) {
    widgets::form_row(ui, label, 130.0, |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(width),
        );
    });
}

fn editor(app: &mut MongoRaabit, m: &mut ConnectionManager, ui: &mut Ui, p: &Palette, height: f32) {
    if let Some(uri) = &mut m.uri_import {
        let mut import = false;
        let mut cancel = false;
        style::card(p).show(ui, |ui| {
            ui.label(RichText::new("Import from connection string").strong());
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::singleline(uri)
                    .hint_text(widgets::hint(
                        ui,
                        "mongodb+srv://user:password@cluster0.example.net/",
                    ))
                    .font(FontId::monospace(12.5))
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                import = widgets::primary_button(ui, Some(Icon::Import), "Import").clicked();
                cancel = widgets::secondary_button(ui, None, "Cancel").clicked();
            });
        });
        if import {
            match ConnectionProfile::from_uri(uri) {
                Ok((profile, password)) => {
                    let mut next = ConnectionManager::from_profile(profile);
                    next.password = password.unwrap_or_default();
                    *m = next;
                }
                Err(err) => m.error = Some(err.to_string()),
            }
        } else if cancel {
            m.uri_import = None;
        }
        ui.add_space(10.0);
    }
    let footer_height =
        96.0 + if matches!(m.test, Load::Idle) {
            0.0
        } else {
            36.0
        } + if m.error.is_some() { 36.0 } else { 0.0 };
    egui::ScrollArea::vertical()
        .id_salt("raabit-manager-form")
        .max_height((height - footer_height - ui.min_rect().height()).max(120.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
    widgets::form_row(ui, "Name", 130.0, |ui| {
                ui.add(egui::TextEdit::singleline(&mut m.draft.name).desired_width(300.0));
            });
            widgets::form_row(ui, "Folder", 130.0, |ui| {
                ui.add(egui::TextEdit::singleline(&mut m.draft.folder).hint_text(widgets::hint(ui, "e.g. Production/EU")).desired_width(300.0));
            });
            ui.add_space(8.0);
            tab_strip(ui, m, p);
            ui.add_space(8.0);
            match m.page {
                Page::Server => {
                    widgets::form_row(ui, "Connection type", 130.0, |ui| {
                        egui::ComboBox::from_id_salt("raabit-target")
                            .selected_text(m.target_kind.label())
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for kind in TargetKind::ALL {
                                    ui.selectable_value(&mut m.target_kind, kind, kind.label());
                                }
                            });
                    });
                    match m.target_kind {
                        TargetKind::Standalone => {
                            widgets::form_row(ui, "Server", 130.0, |ui| {
                                ui.add(egui::TextEdit::singleline(&mut m.host).desired_width(220.0));
                                ui.label(RichText::new(":").color(p.text_info));
                                ui.add(egui::TextEdit::singleline(&mut m.port).desired_width(64.0));
                            });
                        }
                        TargetKind::Srv => text_row(ui, "Cluster host", &mut m.host, "cluster0.example.net", 300.0),
                        TargetKind::ReplicaSet | TargetKind::Sharded => {
                            let label = if m.target_kind == TargetKind::ReplicaSet { "Members" } else { "mongos routers" };
                            widgets::form_row(ui, label, 130.0, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut m.hosts_text)
                                        .hint_text(widgets::hint(ui, "host1:27017\nhost2:27017"))
                                        .font(FontId::monospace(12.5))
                                        .desired_rows(4)
                                        .desired_width(300.0),
                                );
                            });
                            if m.target_kind == TargetKind::ReplicaSet {
                                text_row(ui, "Replica set name", &mut m.set_name, "rs0", 220.0);
                            }
                        }
                    }
                    widgets::form_row(ui, "Read preference", 130.0, |ui| {
                        egui::ComboBox::from_id_salt("raabit-readpref")
                            .selected_text(m.draft.read_preference.as_str())
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for pref in ReadPreference::ALL {
                                    ui.selectable_value(&mut m.draft.read_preference, pref, pref.as_str());
                                }
                            });
                    });
                }
                Page::Authentication => {
                    widgets::form_row(ui, "Mechanism", 130.0, |ui| {
                        egui::ComboBox::from_id_salt("raabit-auth")
                            .selected_text(m.draft.auth.mechanism.label())
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for mech in AuthMechanism::ALL {
                                    ui.selectable_value(&mut m.draft.auth.mechanism, mech, mech.label());
                                }
                            });
                    });
                    if m.draft.auth.mechanism != AuthMechanism::None {
                        text_row(ui, "User name", &mut m.draft.auth.username, "", 260.0);
                        if m.draft.auth.mechanism.needs_password() {
                            widgets::form_row(ui, "Password", 130.0, |ui| {
                                ui.add(egui::TextEdit::singleline(&mut m.password).password(true).desired_width(260.0));
                            });
                        }
                        text_row(ui, "Auth database", &mut m.draft.auth.auth_database, "admin", 260.0);
                        ui.add_space(6.0);
                        hint(ui, p, "Passwords are kept in the secret store, never in the connections file.");
                    }
                }
                Page::Tls => {
                    widgets::form_row(ui, "", 130.0, |ui| {
                        ui.checkbox(&mut m.draft.tls.enabled, "Use TLS/SSL");
                    });
                    if m.draft.tls.enabled {
                        text_row(ui, "CA file", &mut m.ca_file, "/path/to/ca.pem (optional)", 320.0);
                        text_row(ui, "Client certificate", &mut m.cert_file, "PEM with key, for X.509", 320.0);
                        widgets::form_row(ui, "", 130.0, |ui| {
                            ui.checkbox(&mut m.draft.tls.allow_invalid_certificates, "Allow invalid certificates");
                        });
                        widgets::form_row(ui, "", 130.0, |ui| {
                            ui.checkbox(&mut m.draft.tls.allow_invalid_hostnames, "Allow invalid host names");
                        });
                    }
                }
                Page::Ssh => {
                    widgets::form_row(ui, "", 130.0, |ui| {
                        ui.checkbox(&mut m.ssh_enabled, "Connect through an SSH tunnel");
                    });
                    if m.ssh_enabled {
                        widgets::form_row(ui, "SSH server", 130.0, |ui| {
                            ui.add(egui::TextEdit::singleline(&mut m.ssh.host).hint_text(widgets::hint(ui, "bastion.example.com")).desired_width(220.0));
                            ui.label(RichText::new(":").color(p.text_info));
                            let mut port = m.ssh.port.to_string();
                            if ui.add(egui::TextEdit::singleline(&mut port).desired_width(48.0)).changed() {
                                m.ssh.port = port.trim().parse().unwrap_or(m.ssh.port);
                            }
                        });
                        text_row(ui, "SSH user", &mut m.ssh.username, "", 220.0);
                        widgets::form_row(ui, "Authentication", 130.0, |ui| {
                            let label = match m.ssh.auth {
                                SshAuth::Agent => "SSH agent / default keys",
                                SshAuth::PrivateKey { .. } => "Private key",
                                SshAuth::Password => "Password",
                            };
                            egui::ComboBox::from_id_salt("raabit-ssh-auth").selected_text(label).width(220.0).show_ui(ui, |ui| {
                                if ui.selectable_label(matches!(m.ssh.auth, SshAuth::Agent), "SSH agent / default keys").clicked() {
                                    m.ssh.auth = SshAuth::Agent;
                                }
                                if ui.selectable_label(matches!(m.ssh.auth, SshAuth::PrivateKey { .. }), "Private key").clicked() {
                                    m.ssh.auth = SshAuth::PrivateKey { path: m.ssh_key.clone().into() };
                                }
                                if ui.selectable_label(matches!(m.ssh.auth, SshAuth::Password), "Password").clicked() {
                                    m.ssh.auth = SshAuth::Password;
                                }
                            });
                        });
                        match m.ssh.auth {
                            SshAuth::PrivateKey { .. } => text_row(ui, "Private key", &mut m.ssh_key, "~/.ssh/id_ed25519", 300.0),
                            SshAuth::Password => widgets::form_row(ui, "SSH password", 130.0, |ui| {
                                ui.add(egui::TextEdit::singleline(&mut m.ssh_password).password(true).desired_width(220.0));
                            }),
                            SshAuth::Agent => {}
                        }
                        ui.add_space(6.0);
                        let ssh_found = mongo_raabit_core::tunnel::find_ssh();
                        hint(
                            ui,
                            p,
                            &match ssh_found {
                                Some(path) => format!("Uses the OpenSSH client at {}", path.display()),
                                None => "No `ssh` executable on PATH — install the OpenSSH client to use tunnels.".into(),
                            },
                        );
                    }
                }
                Page::Advanced => {
                    text_row(ui, "Default database", &mut m.draft.default_database, "", 220.0);
                    text_row(ui, "Application name", &mut m.draft.app_name, "mongo-raabit", 220.0);
                    text_row(ui, "Timeout (ms)", &mut m.timeout, "10000", 120.0);
                }
            }
        });
    // Connection string preview and test result below the form.
    ui.add_space(6.0);
    widgets::caption(ui, "Connection string");
    let uri = match m.profile() {
        Ok(profile) => profile.display_uri(),
        Err(err) => format!("⚠ {err}"),
    };
    egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(uri)
                        .font(FontId::monospace(12.0))
                        .color(p.text_secondary),
                )
                .truncate(),
            );
        });
    ui.add_space(6.0);
    if let Some(err) = &m.error {
        banner(ui, p, p.error, Icon::Warning, err);
    }
    match &m.test {
        Load::Idle => {}
        Load::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Testing connection…").color(p.text_secondary));
            });
        }
        Load::Ready(info) => banner(
            ui,
            p,
            p.success,
            Icon::Check,
            &format!(
                "Connected: MongoDB {} · {} · {}",
                info.version,
                info.topology,
                style::duration(info.round_trip)
            ),
        ),
        Load::Failed(err) => banner(ui, p, p.error, Icon::Warning, err),
    }
    let _ = app;
}

fn hint(ui: &mut Ui, p: &Palette, text: &str) {
    widgets::form_row(ui, "", 130.0, |ui| {
        ui.label(RichText::new(text).size(11.5).color(p.text_info));
    });
}

fn banner(ui: &mut Ui, p: &Palette, color: Color32, icon: Icon, text: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(if p.dark { 0.14 } else { 0.10 }))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                crate::icons::icon(ui, icon, color);
                ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
            });
        });
    ui.add_space(4.0);
}

fn store_secrets(app: &MongoRaabit, m: &ConnectionManager, profile: &ConnectionProfile) {
    let set = |key: String, value: &str| {
        let _ = if value.is_empty() {
            app.secrets.delete(&key)
        } else {
            app.secrets.set(&key, value)
        };
    };
    set(profile.password_key(), &m.password);
    set(profile.ssh_password_key(), &m.ssh_password);
}

fn footer(
    app: &mut MongoRaabit,
    m: &mut ConnectionManager,
    ui: &mut Ui,
    p: &Palette,
    open: &mut bool,
) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if widgets::secondary_button(ui, Some(Icon::Pulse), "Test Connection").clicked() {
                    match m.profile() {
                        Ok(profile) => {
                            store_secrets(app, m, &profile);
                            m.test = Load::Loading;
                            m.error = None;
                            let secrets = app.secrets.clone();
                            let id = profile.id.clone();
                            app.jobs.spawn(
                                async move { Connection::test(&profile, secrets.as_ref()).await },
                                move |app: &mut MongoRaabit, result| {
                                    if let Some(manager) = &mut app.manager
                                        && manager.draft.id == id
                                    {
                                        manager.test = match result {
                                            Ok(info) => Load::Ready(info),
                                            Err(err) => Load::Failed(err.to_string()),
                                        };
                                    }
                                },
                            );
                        }
                        Err(err) => m.error = Some(err),
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let connect =
                        widgets::primary_button(ui, Some(Icon::Plug), "Connect").clicked();
                    let save = widgets::secondary_button(ui, None, "Save").clicked();
                    if connect || save {
                        match m.profile() {
                            Ok(profile) => {
                                store_secrets(app, m, &profile);
                                app.store.upsert(profile.clone());
                                app.save_store();
                                m.selected = Some(profile.id.clone());
                                m.draft = profile.clone();
                                m.error = None;
                                if connect {
                                    app.connect(profile);
                                    *open = false;
                                }
                            }
                            Err(err) => m.error = Some(err),
                        }
                    }
                    let _ = p;
                });
            });
        });
    if m.close {
        *open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_round_trips_profiles() {
        let (profile, _) = ConnectionProfile::from_uri(
            "mongodb://u@a:1,b:2/?replicaSet=rs0&tls=true&readPreference=nearest",
        )
        .unwrap();
        let m = ConnectionManager::from_profile(profile.clone());
        assert_eq!(m.target_kind, TargetKind::ReplicaSet);
        assert_eq!(m.hosts_text, "a:1\nb:2");
        let rebuilt = m.profile().unwrap();
        assert_eq!(rebuilt.target, profile.target);
        assert_eq!(rebuilt.read_preference, ReadPreference::Nearest);

        let mut bad = ConnectionManager::from_profile(ConnectionProfile::default());
        bad.port = "x".into();
        assert!(bad.profile().unwrap_err().contains("invalid port"));
        bad.port = "27017".into();
        bad.ssh_enabled = true;
        assert!(bad.profile().is_err());
    }
}
