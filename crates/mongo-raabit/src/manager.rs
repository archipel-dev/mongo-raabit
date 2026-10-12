//! Connection Manager: saved connections in folders, an editor for server, auth, TLS,
//! SSH and advanced settings, URI import and Test Connection.
//!
//! The form itself (fields, validation, profile mapping) is
//! [`mongo_raabit_core::connection_form`]; this module renders it and wires the dialog.

use crate::app::{Load, MongoRaabit};
use crate::egui::{Align, Layout, Ui};
use crate::icons::Icon;
use crate::widgets;
use archipel_sdk::kit::components::{
    Banner, Checkbox, Dialog, DialogAction, FieldLabel, Form, FormUi, IconButton, NumberField,
    Severity, Spinner, Tab, TabBar, TextField, Tree, TreeRow,
};
use archipel_sdk::kit::tokens::size;
use archipel_sdk::kit::{Glyph, Tone};
use mongo_raabit_core::Connection;
use mongo_raabit_core::connection::ServerInfo;
use mongo_raabit_core::connection_form::{
    ConnectionForm, ListEntry, SshAuthKind, TargetKind, first_profile, list_entries,
};
use mongo_raabit_core::profile::{AuthMechanism, ConnectionProfile, ReadPreference, SshAuth};
use mongo_raabit_core::secrets::SecretStore;
use mongo_raabit_core::status;
use std::collections::HashSet;

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

pub struct ConnectionManager {
    pub selected: Option<String>,
    /// The profile being edited, as the form shows it.
    pub form: ConnectionForm,
    pub page: Page,
    /// The connection string being typed in the import panel, while it is open.
    pub uri_import: Option<String>,
    /// Focus the import field on the next frame (the panel was just opened).
    focus_import: bool,
    pub test: Load<ServerInfo>,
    pub error: Option<String>,
    filter: String,
    /// Folders of the list that are closed.
    collapsed: HashSet<String>,
    close: bool,
}

impl ConnectionManager {
    pub fn new(store: &mongo_raabit_core::profile::ProfileStore) -> Self {
        let first = first_profile(store);
        let mut manager = Self::from_form(ConnectionForm::from_profile(
            first.clone().unwrap_or_default(),
        ));
        manager.selected = first.map(|p| p.id);
        manager
    }

    fn from_form(form: ConnectionForm) -> Self {
        Self {
            selected: None,
            form,
            page: Page::Server,
            uri_import: None,
            focus_import: false,
            test: Load::Idle,
            error: None,
            filter: String::new(),
            collapsed: HashSet::new(),
            close: false,
        }
    }

    /// Replace the editor by `next`, keeping what belongs to the list.
    fn replace(&mut self, mut next: Self) {
        next.filter = std::mem::take(&mut self.filter);
        next.collapsed = std::mem::take(&mut self.collapsed);
        *self = next;
    }

    fn select(&mut self, profile: &ConnectionProfile, secrets: &dyn SecretStore) {
        let mut next = Self::from_form(ConnectionForm::from_stored(profile, secrets));
        next.selected = Some(profile.id.clone());
        self.replace(next);
    }

    /// Build the profile from the form fields.
    pub fn profile(&self) -> Result<ConnectionProfile, String> {
        self.form.profile()
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
    let importing = manager.uri_import.is_some();
    let dialog = Dialog::new("Connection Manager")
        .icon(Glyph::Database)
        .width(width);
    // The import panel has its own pair of buttons; Esc leaves it before the dialog.
    let dialog = if importing {
        let ready = manager
            .uri_import
            .as_deref()
            .is_some_and(|uri| !uri.trim().is_empty());
        dialog
            .primary("Import")
            .primary_enabled(ready)
            .secondary("Cancel")
    } else {
        dialog
            .primary("Connect")
            .primary_enabled(manager.profile().is_ok())
            .secondary("Close")
            .tertiary("Test Connection")
    };
    let action = dialog
        .content(|ui| content(app, &mut manager, ui, width, body_height))
        .show(ctx);
    let mut open = !manager.close;
    match action {
        Some(DialogAction::Primary) if importing => import(&mut manager),
        Some(DialogAction::Primary) => {
            if save(app, &mut manager)
                && let Some(profile) = app.store.get(&manager.form.draft.id).cloned()
            {
                app.connect(profile);
                open = false;
            }
        }
        Some(DialogAction::Tertiary) => test_connection(app, &mut manager),
        Some(DialogAction::Secondary | DialogAction::Dismissed) if importing => {
            manager.uri_import = None;
            manager.error = None;
        }
        Some(DialogAction::Secondary | DialogAction::Dismissed) => open = false,
        Some(_) | None => {}
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
            m.form.store_secrets(&profile, app.secrets.as_ref());
            app.store.upsert(profile.clone());
            app.save_store();
            m.selected = Some(profile.id.clone());
            m.form.draft = profile;
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
        ui.vertical(|ui| {
            let form_width = (width - LIST_WIDTH - 3.0 * size::CONTENT_INSET - 40.0).max(160.0);
            ui.set_width(form_width);
            editor(m, ui, height);
        });
    });
}

fn connection_list(app: &mut MongoRaabit, m: &mut ConnectionManager, ui: &mut Ui, height: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        if ui
            .add(IconButton::new(Icon::Plus, "New connection"))
            .clicked()
        {
            m.replace(ConnectionManager::from_form(ConnectionForm::default()));
        }
        if ui
            .add(IconButton::new(
                Icon::Import,
                "Import from connection string",
            ))
            .clicked()
        {
            m.uri_import = Some(String::new());
            m.focus_import = true;
        }
        if ui
            .add(IconButton::new(Icon::Save, "Save connection"))
            .clicked()
        {
            save(app, m);
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
            m.replace(ConnectionManager::new(&app.store));
        }
    });
    ui.add(
        TextField::search(&mut m.filter)
            .name("Filter connections")
            .placeholder("Filter connections"),
    );
    if app.store.profiles.is_empty() {
        widgets::caption(ui, "No saved connections");
        widgets::caption(ui, "Create one with + or import a URI");
        return;
    }
    let folders = app.store.folder_paths();
    let entries = list_entries(&app.store, &folders, &m.filter, &m.collapsed);
    let rows: Vec<TreeRow> = entries
        .iter()
        .map(|entry| match entry {
            ListEntry::Folder { path, name, depth } => TreeRow::new(*name, *depth)
                .icon(Icon::Folder)
                .parent(!m.collapsed.contains(*path)),
            ListEntry::Profile { profile, depth } => TreeRow::new(profile.name.as_str(), *depth)
                .icon(Icon::Database)
                .secondary(profile.summary()),
        })
        .collect();
    let mut selected = entries.iter().position(|entry| {
        matches!(entry, ListEntry::Profile { profile, .. } if m.selected.as_deref() == Some(profile.id.as_str()))
    });
    let out = Tree::new("raabit-manager-list", &rows, &mut selected)
        .name("Saved connections")
        .height(height - 70.0)
        .show(ui);
    let profile_at = |i: usize| match &entries[i] {
        ListEntry::Profile { profile, .. } => Some((*profile).clone()),
        ListEntry::Folder { .. } => None,
    };
    if let Some(i) = out.toggled
        && let ListEntry::Folder { path, .. } = &entries[i]
        && !m.collapsed.remove(*path)
    {
        m.collapsed.insert((*path).to_owned());
    }
    if out.selection_changed
        && let Some(profile) = selected.and_then(profile_at)
        && m.selected.as_deref() != Some(profile.id.as_str())
    {
        m.select(&profile, app.secrets.as_ref());
    }
    if let Some(i) = out.activated {
        match profile_at(i) {
            Some(profile) => {
                m.select(&profile, app.secrets.as_ref());
                app.connect(profile);
                m.close = true;
            }
            None => {
                if let ListEntry::Folder { path, .. } = &entries[i]
                    && !m.collapsed.remove(*path)
                {
                    m.collapsed.insert((*path).to_owned());
                }
            }
        }
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

/// A form row `Label:` with one text field named `Label`.
fn text(form: &mut FormUi, label: &str, value: &mut String, placeholder: &str) {
    form.row(&format!("{label}:"), |ui| {
        ui.add(TextField::new(value).name(label).placeholder(placeholder));
    });
}

/// A form row with a dropdown over `(value, label)` options.
fn pick<T: PartialEq + Clone>(
    form: &mut FormUi,
    id: &str,
    label: &str,
    value: &mut T,
    options: impl IntoIterator<Item = (T, &'static str)>,
) {
    form.row(&format!("{label}:"), |ui| {
        widgets::choice(ui, id, label, value, options, 240.0);
    });
}

/// A form row with a field and, at the right, a port.
fn with_port(
    form: &mut FormUi,
    label: &str,
    value: &mut String,
    placeholder: &str,
    port: &mut u16,
) {
    form.row(&format!("{label}:"), |ui| {
        // Right to left: the port keeps its width and the field takes what is left.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(NumberField::new(port).name("Port"));
            ui.add(FieldLabel::new("Port:"));
            ui.add_space(size::FORM_CONTROL_SPACE);
            ui.add(TextField::new(value).name(label).placeholder(placeholder));
        });
    });
}

/// What the Read-only checkbox of a connection blocks.
const READ_ONLY_COMMENT: &str = "Blocks every change to the database: edits, inserts, deletes, index and collection changes, writing shell commands";

fn editor(m: &mut ConnectionManager, ui: &mut Ui, height: f32) {
    if m.uri_import.is_some() {
        import_form(m, ui);
        return;
    }
    archipel_sdk::kit::parts::scroll_vertical()
        .id_salt("raabit-manager-form")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Form::new().show(ui, |form| {
                text(form, "Name", &mut m.form.draft.name, "");
                text(
                    form,
                    "Folder",
                    &mut m.form.draft.folder,
                    "e.g. Production/EU",
                );
                form.add(
                    Checkbox::new(&mut m.form.draft.read_only, "Read-only")
                        .comment(READ_ONLY_COMMENT),
                );
                tab_strip(form.ui(), m);
                match m.page {
                    Page::Server => server_page(form, &mut m.form),
                    Page::Authentication => auth_page(form, &mut m.form),
                    Page::Tls => tls_page(form, &mut m.form),
                    Page::Ssh => ssh_page(form, &mut m.form),
                    Page::Advanced => advanced_page(form, &mut m.form),
                }
                form.space();
                let (mut uri, error) = match m.form.uri_preview() {
                    Ok(uri) => (uri, None),
                    Err(err) => (String::new(), Some(err)),
                };
                form.row("Connection string:", |ui| {
                    ui.add(
                        TextField::new(&mut uri)
                            .name("Connection string")
                            .monospace(true)
                            .error(error.as_deref())
                            .enabled(false),
                    );
                });
            });
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
            Banner::new(Severity::Success, status::test_result(info)).show(ui);
        }
        Load::Failed(err) => {
            Banner::new(Severity::Error, err).show(ui);
        }
    }
}

/// The import panel: the dialog's Import and Cancel buttons act on it.
fn import_form(m: &mut ConnectionManager, ui: &mut Ui) {
    let id = crate::egui::Id::new("raabit-manager-import");
    if let Some(uri) = &mut m.uri_import {
        if m.focus_import {
            // Ask until the field has it: the click that opened the panel may still hold it.
            if ui.memory(|memory| memory.has_focus(id)) {
                m.focus_import = false;
            } else {
                ui.memory_mut(|memory| memory.request_focus(id));
                ui.ctx().request_repaint();
            }
        }
        Form::new().show(ui, |form| {
            form.row("Connection string:", |ui| {
                ui.add(
                    TextField::new(uri)
                        .id(id)
                        .name("Connection string to import")
                        .placeholder("mongodb+srv://user:password@cluster0.example.net/")
                        .monospace(true),
                );
            });
        });
    }
    if let Some(err) = &m.error {
        Banner::new(Severity::Error, err).show(ui);
    }
}

fn import(m: &mut ConnectionManager) {
    let Some(uri) = m.uri_import.as_deref() else {
        return;
    };
    match ConnectionForm::from_uri(uri) {
        Ok(form) => m.replace(ConnectionManager::from_form(form)),
        Err(err) => m.error = Some(err),
    }
}

fn server_page(form: &mut FormUi, f: &mut ConnectionForm) {
    pick(
        form,
        "raabit-target",
        "Connection type",
        &mut f.target_kind,
        TargetKind::ALL.map(|k| (k, k.label())),
    );
    match f.target_kind {
        TargetKind::Standalone => with_port(form, "Host", &mut f.host, "", &mut f.port),
        TargetKind::Srv => text(form, "Cluster host", &mut f.host, "cluster0.example.net"),
        TargetKind::ReplicaSet | TargetKind::Sharded => {
            let label = if f.target_kind == TargetKind::ReplicaSet {
                "Members"
            } else {
                "mongos routers"
            };
            form.ui().label(format!("{label}:"));
            form.ui().add(
                TextField::new(&mut f.hosts_text)
                    .name(label)
                    .multiline(4)
                    .monospace(true)
                    .placeholder("host1:27017\nhost2:27017"),
            );
            if f.target_kind == TargetKind::ReplicaSet {
                text(form, "Replica set name", &mut f.set_name, "rs0");
            }
        }
    }
    pick(
        form,
        "raabit-readpref",
        "Read preference",
        &mut f.draft.read_preference,
        ReadPreference::ALL.map(|p| (p, p.as_str())),
    );
}

fn auth_page(form: &mut FormUi, f: &mut ConnectionForm) {
    pick(
        form,
        "raabit-auth",
        "Mechanism",
        &mut f.draft.auth.mechanism,
        AuthMechanism::ALL.map(|a| (a, a.label())),
    );
    if f.draft.auth.mechanism == AuthMechanism::None {
        return;
    }
    text(form, "User name", &mut f.draft.auth.username, "");
    if f.draft.auth.mechanism.needs_password() {
        form.row("Password:", |ui| {
            ui.add(
                TextField::new(&mut f.password)
                    .name("Password")
                    .password(true),
            );
        });
    }
    text(
        form,
        "Auth database",
        &mut f.draft.auth.auth_database,
        "admin",
    );
    widgets::note(
        form.ui(),
        Tone::Muted,
        "Passwords are kept in the secret store, never in the connections file.",
    );
}

fn tls_page(form: &mut FormUi, f: &mut ConnectionForm) {
    form.add(Checkbox::new(&mut f.draft.tls.enabled, "Use TLS/SSL"));
    if !f.draft.tls.enabled {
        return;
    }
    form.indent(|form| {
        text(
            form,
            "CA file",
            &mut f.ca_file,
            "/path/to/ca.pem (optional)",
        );
        text(
            form,
            "Client certificate",
            &mut f.cert_file,
            "PEM with key, for X.509",
        );
        form.add(Checkbox::new(
            &mut f.draft.tls.allow_invalid_certificates,
            "Allow invalid certificates",
        ));
        form.add(Checkbox::new(
            &mut f.draft.tls.allow_invalid_hostnames,
            "Allow invalid host names",
        ));
    });
}

fn advanced_page(form: &mut FormUi, f: &mut ConnectionForm) {
    text(form, "Default database", &mut f.draft.default_database, "");
    text(
        form,
        "Application name",
        &mut f.draft.app_name,
        "mongo-raabit",
    );
    // Empty means the driver default, which an integer field cannot say (see "Kit gaps").
    let error = f.timeout_error();
    form.row("Timeout (ms):", |ui| {
        ui.add(
            TextField::new(&mut f.timeout)
                .name("Timeout (ms)")
                .placeholder("10000")
                .error(error),
        );
    });
}

fn ssh_page(form: &mut FormUi, f: &mut ConnectionForm) {
    form.add(Checkbox::new(
        &mut f.ssh_enabled,
        "Connect through an SSH tunnel",
    ));
    if !f.ssh_enabled {
        return;
    }
    form.indent(|form| {
        with_port(
            form,
            "SSH server",
            &mut f.ssh.host,
            "bastion.example.com",
            &mut f.ssh.port,
        );
        text(form, "SSH user", &mut f.ssh.username, "");
        let mut kind = f.ssh_auth_kind();
        pick(
            form,
            "raabit-ssh-auth",
            "Authentication",
            &mut kind,
            SshAuthKind::ALL.map(|k| (k, k.label())),
        );
        f.set_ssh_auth_kind(kind);
        match f.ssh.auth {
            SshAuth::PrivateKey { .. } => {
                text(form, "Private key", &mut f.ssh_key, "~/.ssh/id_ed25519");
            }
            SshAuth::Password => {
                form.row("SSH password:", |ui| {
                    ui.add(
                        TextField::new(&mut f.ssh_password)
                            .name("SSH password")
                            .password(true),
                    );
                });
            }
            SshAuth::Agent => {}
        }
        widgets::note(
            form.ui(),
            Tone::Muted,
            match mongo_raabit_core::tunnel::find_ssh() {
                Some(path) => format!("Uses the OpenSSH client at {}", path.display()),
                None => "No `ssh` executable on PATH — install the OpenSSH client to use tunnels."
                    .into(),
            },
        );
    });
}

fn test_connection(app: &mut MongoRaabit, m: &mut ConnectionManager) {
    match m.profile() {
        Ok(profile) => {
            m.form.store_secrets(&profile, app.secrets.as_ref());
            m.test = Load::Loading;
            m.error = None;
            let secrets = app.secrets.clone();
            let id = profile.id.clone();
            app.jobs.spawn(
                async move { Connection::test(&profile, secrets.as_ref()).await },
                move |app: &mut MongoRaabit, result| {
                    if let Some(manager) = &mut app.manager
                        && manager.form.draft.id == id
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
