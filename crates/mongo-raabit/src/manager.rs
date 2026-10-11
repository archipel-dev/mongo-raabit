//! Connection Manager: saved connections in folders, an editor for server, auth, TLS,
//! SSH and advanced settings, URI import and Test Connection.

use crate::app::{Load, MongoRaabit};
use crate::egui::{ScrollArea, Ui};
use crate::icons::Icon;
use crate::style;
use crate::widgets;
use archipel_sdk::kit::components::{
    Banner, Button, Checkbox, Dialog, DialogAction, Dropdown, IconButton, ListRow, Severity,
    Spinner, Tab, TabBar, TextField,
};
use archipel_sdk::kit::tokens::size;
use archipel_sdk::kit::{Glyph, Tone};
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
    pub const ALL: [(Self, &'static str); 5] = [
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

/// Dialog width, and the share of the window height the form area may use.
const WIDTH: f32 = 880.0;
const LIST_WIDTH: f32 = 250.0;

/// Render the dialog while `app.manager` is set; closing clears it.
pub fn show(app: &mut MongoRaabit, ctx: &crate::egui::Context) {
    let Some(mut manager) = app.manager.take() else {
        return;
    };
    let width = WIDTH.min(ctx.content_rect().width() - 40.0);
    // What is left of the window once the title, the result banner and the buttons are in.
    let body_height = (ctx.content_rect().height() - 300.0).clamp(240.0, 440.0);
    let valid = manager.profile().is_ok();
    let action = Dialog::new("Connection Manager")
        .icon(Glyph::Database)
        .width(width)
        .primary("Connect")
        .primary_enabled(valid)
        .secondary("Save")
        .content(|ui| content(app, &mut manager, ui, width, body_height))
        .show(ctx);
    let mut open = !manager.close;
    match action {
        Some(DialogAction::Primary) => {
            if save(app, &mut manager)
                && let Some(profile) = app.store.get(&manager.draft.id).cloned()
            {
                app.connect(profile);
                open = false;
            }
        }
        Some(DialogAction::Secondary) => {
            save(app, &mut manager);
        }
        Some(DialogAction::Dismissed) => open = false,
        None => {}
    }
    // The dialog may have been replaced while its content ran (e.g. by `connect`).
    if open && app.manager.is_none() {
        app.manager = Some(manager);
    }
}

/// Store the form's profile (and its secrets); reports whether it was valid.
fn save(app: &mut MongoRaabit, m: &mut ConnectionManager) -> bool {
    match m.profile() {
        Ok(profile) => {
            store_secrets(app, m, &profile);
            app.store.upsert(profile.clone());
            app.save_store();
            m.selected = Some(profile.id.clone());
            m.draft = profile;
            m.error = None;
            true
        }
        Err(err) => {
            m.error = Some(err);
            false
        }
    }
}

fn content(app: &mut MongoRaabit, m: &mut ConnectionManager, ui: &mut Ui, width: f32, height: f32) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = size::CONTENT_INSET;
        ui.vertical(|ui| {
            ui.set_width(LIST_WIDTH);
            connection_list(app, m, ui, height);
        });
        ui.add(archipel_sdk::kit::components::Separator);
        ui.vertical(|ui| {
            let form_width = width - LIST_WIDTH - 3.0 * size::CONTENT_INSET - 40.0;
            ui.set_width(form_width);
            editor(m, ui, height);
        });
    });
    ui.add(archipel_sdk::kit::components::Separator);
    ui.horizontal(|ui| {
        if ui
            .add(Button::secondary("Test Connection").icon(Icon::Pulse))
            .clicked()
        {
            test_connection(app, m);
        }
        if ui.add(Button::ghost("Close")).clicked() {
            m.close = true;
        }
    });
}

fn connection_list(app: &mut MongoRaabit, m: &mut ConnectionManager, ui: &mut Ui, height: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        if ui
            .add(IconButton::new(Icon::Plus, "New connection"))
            .clicked()
        {
            let mut fresh = ConnectionManager::from_profile(ConnectionProfile::default());
            fresh.filter = std::mem::take(&mut m.filter);
            *m = fresh;
        }
        if ui
            .add(IconButton::new(
                Icon::Import,
                "Import from connection string",
            ))
            .clicked()
        {
            m.uri_import = Some(String::new());
        }
        let has_selection = m.selected.is_some();
        if ui
            .add(IconButton::new(Icon::Copy, "Duplicate").enabled(has_selection))
            .clicked()
            && let Some(id) = m.selected.clone()
            && let Some(new_id) = app.store.duplicate(&id)
        {
            app.save_store();
            if let Some(profile) = app.store.get(&new_id).cloned() {
                m.select(&profile, app.secrets.as_ref());
            }
        }
        if ui
            .add(
                IconButton::new(Icon::Trash, "Delete")
                    .tone(Tone::Error)
                    .enabled(has_selection),
            )
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
    ui.add(TextField::search(&mut m.filter).placeholder("Filter connections"));
    let filter = m.filter.to_lowercase();
    let mut clicked: Option<ConnectionProfile> = None;
    let mut connect: Option<ConnectionProfile> = None;
    ScrollArea::vertical()
        .id_salt("raabit-manager-list")
        .max_height(height - 70.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
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
                    ListRow::new(folder.rsplit('/').next().unwrap_or(&folder))
                        .icon(Icon::Folder)
                        .depth(depth - 1)
                        .enabled(false)
                        .show(ui);
                }
                for profile in profiles {
                    let row = ListRow::new(profile.name.as_str())
                        .icon(Icon::Database)
                        .secondary(crate::values::truncate(&profile.summary(), 30))
                        .depth(depth)
                        .selected(m.selected.as_deref() == Some(profile.id.as_str()))
                        .show(ui);
                    if row.response.double_clicked() {
                        connect = Some(profile.clone());
                    } else if row.response.clicked() {
                        clicked = Some(profile.clone());
                    }
                }
            }
            if app.store.profiles.is_empty() {
                widgets::caption(ui, "No saved connections");
                widgets::caption(ui, "Create one with + or import a URI");
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

fn tab_strip(ui: &mut Ui, m: &mut ConnectionManager) {
    let tabs: Vec<Tab> = Page::ALL
        .iter()
        .map(|(_, label)| Tab::new(*label))
        .collect();
    let current = Page::ALL
        .iter()
        .position(|(p, _)| *p == m.page)
        .unwrap_or(0);
    if let Some(index) = TabBar::new(&tabs, current).show(ui).selected {
        m.page = Page::ALL[index].0;
    }
}

fn text(ui: &mut Ui, label: &str, value: &mut String, placeholder: &str) {
    ui.add(TextField::new(value).label(label).placeholder(placeholder));
}

/// A dropdown over `all` with a visible label; `name` gives each option its text.
fn pick<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    label: &str,
    value: &mut T,
    all: &[T],
    name: impl Fn(T) -> String,
) {
    widgets::caption(ui, label);
    let mut selected = name(*value);
    let options: Vec<String> = all.iter().map(|v| name(*v)).collect();
    if ui
        .add(
            Dropdown::new(id, &mut selected, options)
                .label(label)
                .width(240.0),
        )
        .changed()
        && let Some(v) = all.iter().find(|v| name(**v) == selected)
    {
        *value = *v;
    }
}

fn editor(m: &mut ConnectionManager, ui: &mut Ui, height: f32) {
    if m.uri_import.is_some() {
        import_form(m, ui);
        return;
    }
    ScrollArea::vertical()
        .id_salt("raabit-manager-form")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = size::SECTION_SPACING;
            text(ui, "Name", &mut m.draft.name, "");
            text(ui, "Folder", &mut m.draft.folder, "e.g. Production/EU");
            tab_strip(ui, m);
            match m.page {
                Page::Server => server_page(m, ui),
                Page::Authentication => auth_page(m, ui),
                Page::Tls => {
                    ui.add(Checkbox::new(&mut m.draft.tls.enabled, "Use TLS/SSL"));
                    if m.draft.tls.enabled {
                        text(ui, "CA file", &mut m.ca_file, "/path/to/ca.pem (optional)");
                        text(
                            ui,
                            "Client certificate",
                            &mut m.cert_file,
                            "PEM with key, for X.509",
                        );
                        ui.add(Checkbox::new(
                            &mut m.draft.tls.allow_invalid_certificates,
                            "Allow invalid certificates",
                        ));
                        ui.add(Checkbox::new(
                            &mut m.draft.tls.allow_invalid_hostnames,
                            "Allow invalid host names",
                        ));
                    }
                }
                Page::Ssh => ssh_page(m, ui),
                Page::Advanced => {
                    text(ui, "Default database", &mut m.draft.default_database, "");
                    text(
                        ui,
                        "Application name",
                        &mut m.draft.app_name,
                        "mongo-raabit",
                    );
                    text(ui, "Timeout (ms)", &mut m.timeout, "10000");
                }
            }
            let mut uri = match m.profile() {
                Ok(profile) => profile.display_uri(),
                Err(err) => format!("⚠ {err}"),
            };
            ui.add(
                TextField::new(&mut uri)
                    .label("Connection string")
                    .monospace(true)
                    .enabled(false),
            );
        });
    if let Some(err) = &m.error {
        Banner::new(Severity::Error, err).show(ui);
    }
    match &m.test {
        Load::Idle => {}
        Load::Loading => {
            ui.add(Spinner::new().label("Testing connection…"));
        }
        Load::Ready(info) => {
            Banner::new(
                Severity::Success,
                format!(
                    "Connected: MongoDB {} · {} · {}",
                    info.version,
                    info.topology,
                    style::duration(info.round_trip)
                ),
            )
            .show(ui);
        }
        Load::Failed(err) => {
            Banner::new(Severity::Error, err).show(ui);
        }
    }
}

fn import_form(m: &mut ConnectionManager, ui: &mut Ui) {
    let mut import = false;
    let mut cancel = false;
    if let Some(uri) = &mut m.uri_import {
        ui.spacing_mut().item_spacing.y = size::SECTION_SPACING;
        ui.add(
            TextField::new(uri)
                .label("Import from connection string")
                .placeholder("mongodb+srv://user:password@cluster0.example.net/")
                .monospace(true),
        );
        ui.horizontal(|ui| {
            import = ui
                .add(Button::primary("Import").icon(Icon::Import))
                .clicked();
            cancel = ui.add(Button::secondary("Cancel")).clicked();
        });
    }
    if let Some(err) = &m.error {
        Banner::new(Severity::Error, err).show(ui);
    }
    if import {
        let parsed = m.uri_import.as_deref().map(ConnectionProfile::from_uri);
        match parsed {
            Some(Ok((profile, password))) => {
                let mut next = ConnectionManager::from_profile(profile);
                next.password = password.unwrap_or_default();
                next.filter = std::mem::take(&mut m.filter);
                *m = next;
            }
            Some(Err(err)) => m.error = Some(err.to_string()),
            None => {}
        }
    } else if cancel {
        m.uri_import = None;
        m.error = None;
    }
}

fn server_page(m: &mut ConnectionManager, ui: &mut Ui) {
    pick(
        ui,
        "raabit-target",
        "Connection type",
        &mut m.target_kind,
        &TargetKind::ALL,
        |k| k.label().to_owned(),
    );
    match m.target_kind {
        TargetKind::Standalone => {
            ui.horizontal_top(|ui| {
                ui.add(TextField::new(&mut m.host).label("Host").width(240.0));
                let error = m
                    .port
                    .trim()
                    .parse::<u16>()
                    .is_err()
                    .then_some("Enter a port from 0 to 65535");
                ui.add(
                    TextField::new(&mut m.port)
                        .label("Port")
                        .error(error)
                        .width(100.0),
                );
            });
        }
        TargetKind::Srv => text(ui, "Cluster host", &mut m.host, "cluster0.example.net"),
        TargetKind::ReplicaSet | TargetKind::Sharded => {
            let label = if m.target_kind == TargetKind::ReplicaSet {
                "Members"
            } else {
                "mongos routers"
            };
            widgets::caption(ui, label);
            // The kit has no multi-line text field yet (see "Kit gaps").
            ui.add(
                crate::egui::TextEdit::multiline(&mut m.hosts_text)
                    .hint_text(widgets::hint(ui, "host1:27017\nhost2:27017"))
                    .font(crate::egui::TextStyle::Monospace)
                    .desired_rows(4)
                    .desired_width(300.0),
            );
            if m.target_kind == TargetKind::ReplicaSet {
                text(ui, "Replica set name", &mut m.set_name, "rs0");
            }
        }
    }
    pick(
        ui,
        "raabit-readpref",
        "Read preference",
        &mut m.draft.read_preference,
        &ReadPreference::ALL,
        |p| p.as_str().to_owned(),
    );
}

fn auth_page(m: &mut ConnectionManager, ui: &mut Ui) {
    pick(
        ui,
        "raabit-auth",
        "Mechanism",
        &mut m.draft.auth.mechanism,
        &AuthMechanism::ALL,
        |a| a.label().to_owned(),
    );
    if m.draft.auth.mechanism != AuthMechanism::None {
        text(ui, "User name", &mut m.draft.auth.username, "");
        if m.draft.auth.mechanism.needs_password() {
            ui.add(
                TextField::new(&mut m.password)
                    .label("Password")
                    .password(true),
            );
        }
        text(
            ui,
            "Auth database",
            &mut m.draft.auth.auth_database,
            "admin",
        );
        widgets::caption(
            ui,
            "Passwords are kept in the secret store, never in the connections file.",
        );
    }
}

fn ssh_page(m: &mut ConnectionManager, ui: &mut Ui) {
    ui.add(Checkbox::new(
        &mut m.ssh_enabled,
        "Connect through an SSH tunnel",
    ));
    if !m.ssh_enabled {
        return;
    }
    ui.horizontal_top(|ui| {
        ui.add(
            TextField::new(&mut m.ssh.host)
                .label("SSH server")
                .placeholder("bastion.example.com")
                .width(240.0),
        );
        let mut port = m.ssh.port.to_string();
        if ui
            .add(TextField::new(&mut port).label("Port").width(100.0))
            .changed()
        {
            m.ssh.port = port.trim().parse().unwrap_or(m.ssh.port);
        }
    });
    text(ui, "SSH user", &mut m.ssh.username, "");
    #[derive(Clone, Copy, PartialEq)]
    enum AuthKind {
        Agent,
        Key,
        Password,
    }
    let mut kind = match m.ssh.auth {
        SshAuth::Agent => AuthKind::Agent,
        SshAuth::PrivateKey { .. } => AuthKind::Key,
        SshAuth::Password => AuthKind::Password,
    };
    pick(
        ui,
        "raabit-ssh-auth",
        "Authentication",
        &mut kind,
        &[AuthKind::Agent, AuthKind::Key, AuthKind::Password],
        |k| {
            match k {
                AuthKind::Agent => "SSH agent / default keys",
                AuthKind::Key => "Private key",
                AuthKind::Password => "Password",
            }
            .to_owned()
        },
    );
    m.ssh.auth = match kind {
        AuthKind::Agent => SshAuth::Agent,
        AuthKind::Key => SshAuth::PrivateKey {
            path: m.ssh_key.clone().into(),
        },
        AuthKind::Password => SshAuth::Password,
    };
    match m.ssh.auth {
        SshAuth::PrivateKey { .. } => text(ui, "Private key", &mut m.ssh_key, "~/.ssh/id_ed25519"),
        SshAuth::Password => {
            ui.add(
                TextField::new(&mut m.ssh_password)
                    .label("SSH password")
                    .password(true),
            );
        }
        SshAuth::Agent => {}
    }
    widgets::caption(
        ui,
        &match mongo_raabit_core::tunnel::find_ssh() {
            Some(path) => format!("Uses the OpenSSH client at {}", path.display()),
            None => {
                "No `ssh` executable on PATH — install the OpenSSH client to use tunnels.".into()
            }
        },
    );
}

fn test_connection(app: &mut MongoRaabit, m: &mut ConnectionManager) {
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
