//! The Connection Manager's editor, without the UI: a connection profile as the text and
//! choices a form edits, back to a validated profile, the list of saved connections as
//! rows, and the secrets that belong to a profile.

use crate::profile::{ConnectionProfile, HostPort, ProfileStore, SshAuth, SshSettings, Target};
use crate::secrets::SecretStore;

/// The shape of the server a profile points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Standalone,
    ReplicaSet,
    Sharded,
    Srv,
}

impl TargetKind {
    pub const ALL: [Self; 4] = [Self::Standalone, Self::ReplicaSet, Self::Sharded, Self::Srv];

    pub fn label(self) -> &'static str {
        match self {
            Self::Standalone => "Standalone",
            Self::ReplicaSet => "Replica set",
            Self::Sharded => "Sharded cluster",
            Self::Srv => "DNS seed list (SRV)",
        }
    }
}

/// How the SSH tunnel authenticates, without the key path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAuthKind {
    Agent,
    Key,
    Password,
}

impl SshAuthKind {
    pub const ALL: [Self; 3] = [Self::Agent, Self::Key, Self::Password];

    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "SSH agent / default keys",
            Self::Key => "Private key",
            Self::Password => "Password",
        }
    }
}

/// A profile being edited: the fields as the form shows them.
#[derive(Debug, Clone)]
pub struct ConnectionForm {
    /// The profile the form started from; the fields below are applied on top of it.
    pub draft: ConnectionProfile,
    pub target_kind: TargetKind,
    pub host: String,
    pub port: u16,
    /// Members or routers, one `host:port` per line.
    pub hosts_text: String,
    pub set_name: String,
    pub password: String,
    pub ssh_enabled: bool,
    pub ssh: SshSettings,
    pub ssh_key: String,
    pub ssh_password: String,
    pub ca_file: String,
    pub cert_file: String,
    /// Connect timeout in milliseconds; empty = driver default.
    pub timeout: String,
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self::from_profile(ConnectionProfile::default())
    }
}

fn hosts_text<T: ToString>(hosts: &[T]) -> String {
    hosts
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

impl ConnectionForm {
    pub fn from_profile(profile: ConnectionProfile) -> Self {
        let mut form = Self {
            target_kind: TargetKind::Standalone,
            host: "localhost".into(),
            port: 27017,
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
            draft: profile,
        };
        if let SshAuth::PrivateKey { path } = &form.ssh.auth {
            form.ssh_key = path.display().to_string();
        }
        match &form.draft.target {
            Target::Standalone { server } => {
                form.host = server.host.clone();
                form.port = server.port;
            }
            Target::ReplicaSet { members, set_name } => {
                form.target_kind = TargetKind::ReplicaSet;
                form.hosts_text = hosts_text(members);
                form.set_name = set_name.clone();
            }
            Target::Sharded { routers } => {
                form.target_kind = TargetKind::Sharded;
                form.hosts_text = hosts_text(routers);
            }
            Target::Srv { host } => {
                form.target_kind = TargetKind::Srv;
                form.host = host.clone();
            }
        }
        form
    }

    /// The form of a connection string (the password goes into the form's password field).
    pub fn from_uri(uri: &str) -> Result<Self, String> {
        let (profile, password) = ConnectionProfile::from_uri(uri).map_err(|e| e.to_string())?;
        let mut form = Self::from_profile(profile);
        form.password = password.unwrap_or_default();
        Ok(form)
    }

    /// The form of a stored profile, with its passwords read from `secrets`.
    pub fn from_stored(profile: &ConnectionProfile, secrets: &dyn SecretStore) -> Self {
        let mut form = Self::from_profile(profile.clone());
        form.password = secrets
            .get(&profile.password_key())
            .ok()
            .flatten()
            .unwrap_or_default();
        form.ssh_password = secrets
            .get(&profile.ssh_password_key())
            .ok()
            .flatten()
            .unwrap_or_default();
        form
    }

    /// Write (or, when blank, delete) the passwords of `profile` in `secrets`.
    pub fn store_secrets(&self, profile: &ConnectionProfile, secrets: &dyn SecretStore) {
        let set = |key: String, value: &str| {
            let _ = if value.is_empty() {
                secrets.delete(&key)
            } else {
                secrets.set(&key, value)
            };
        };
        set(profile.password_key(), &self.password);
        set(profile.ssh_password_key(), &self.ssh_password);
    }

    fn hosts(&self) -> Result<Vec<HostPort>, String> {
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
    }

    /// Why the timeout text is not a number of milliseconds.
    pub fn timeout_error(&self) -> Option<&'static str> {
        let text = self.timeout.trim();
        (!text.is_empty() && text.parse::<u64>().is_err())
            .then_some("Enter the timeout in milliseconds")
    }

    /// Build the profile from the form fields, validated.
    pub fn profile(&self) -> Result<ConnectionProfile, String> {
        let mut profile = self.draft.clone();
        profile.target = match self.target_kind {
            TargetKind::Standalone => Target::Standalone {
                server: HostPort::new(self.host.trim(), self.port),
            },
            TargetKind::ReplicaSet => Target::ReplicaSet {
                members: self.hosts()?,
                set_name: self.set_name.trim().to_owned(),
            },
            TargetKind::Sharded => Target::Sharded {
                routers: self.hosts()?,
            },
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

    /// The connection string the form describes (password masked), or what is wrong.
    pub fn uri_preview(&self) -> Result<String, String> {
        self.profile().map(|profile| profile.display_uri())
    }

    pub fn ssh_auth_kind(&self) -> SshAuthKind {
        match self.ssh.auth {
            SshAuth::Agent => SshAuthKind::Agent,
            SshAuth::PrivateKey { .. } => SshAuthKind::Key,
            SshAuth::Password => SshAuthKind::Password,
        }
    }

    pub fn set_ssh_auth_kind(&mut self, kind: SshAuthKind) {
        self.ssh.auth = match kind {
            SshAuthKind::Agent => SshAuth::Agent,
            SshAuthKind::Key => SshAuth::PrivateKey {
                path: self.ssh_key.clone().into(),
            },
            SshAuthKind::Password => SshAuth::Password,
        };
    }
}

/// One row of the saved-connections list.
#[derive(Debug, Clone, PartialEq)]
pub enum ListEntry<'a> {
    Folder {
        path: &'a str,
        name: &'a str,
        depth: usize,
    },
    Profile {
        profile: &'a ConnectionProfile,
        depth: usize,
    },
}

/// The saved connections as the manager's list shows them: profiles of the root, then each
/// folder followed by its profiles, those matching `filter` (name or address) only, and
/// nothing below the folders in `collapsed`. While filtering, folders without a match are
/// left out.
pub fn list_entries<'a>(
    store: &'a ProfileStore,
    folder_paths: &'a [String],
    filter: &str,
    collapsed: &std::collections::HashSet<String>,
) -> Vec<ListEntry<'a>> {
    let filter = filter.to_lowercase();
    let matches = |p: &ConnectionProfile| {
        filter.is_empty()
            || p.name.to_lowercase().contains(&filter)
            || p.summary().to_lowercase().contains(&filter)
    };
    let mut entries = Vec::new();
    let mut folders = vec![""];
    folders.extend(folder_paths.iter().map(String::as_str));
    for folder in folders {
        // Below a collapsed folder, at any depth.
        if collapsed.iter().any(|c| {
            folder
                .strip_prefix(c.as_str())
                .is_some_and(|r| r.starts_with('/'))
        }) {
            continue;
        }
        let profiles: Vec<&ConnectionProfile> = store
            .profiles_in(folder)
            .into_iter()
            .filter(|p| matches(p))
            .collect();
        // While filtering, a folder stays when something inside it, at any depth, matches.
        if !folder.is_empty()
            && !filter.is_empty()
            && !store.profiles.iter().any(|p| {
                let inside = p.folder.trim_matches('/');
                (inside == folder
                    || inside
                        .strip_prefix(folder)
                        .is_some_and(|rest| rest.starts_with('/')))
                    && matches(p)
            })
        {
            continue;
        }
        let depth = if folder.is_empty() {
            0
        } else {
            folder.matches('/').count() + 1
        };
        if !folder.is_empty() {
            entries.push(ListEntry::Folder {
                path: folder,
                name: folder.rsplit('/').next().unwrap_or(folder),
                depth: depth - 1,
            });
            if collapsed.contains(folder) {
                continue;
            }
        }
        entries.extend(
            profiles
                .into_iter()
                .map(|profile| ListEntry::Profile { profile, depth }),
        );
    }
    entries
}

/// The first profile the manager selects: a root profile, else the first folder's.
pub fn first_profile(store: &ProfileStore) -> Option<ConnectionProfile> {
    store
        .profiles_in("")
        .into_iter()
        .chain(
            store
                .folder_paths()
                .iter()
                .flat_map(|f| store.profiles_in(f)),
        )
        .next()
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ReadPreference;
    use crate::secrets::MemorySecretStore;
    use std::collections::HashSet;

    fn blank() -> ConnectionForm {
        ConnectionForm::from_profile(ConnectionProfile::default())
    }

    #[test]
    fn form_round_trips_profiles() {
        let (profile, _) = ConnectionProfile::from_uri(
            "mongodb://u@a:1,b:2/?replicaSet=rs0&tls=true&readPreference=nearest",
        )
        .unwrap();
        let form = ConnectionForm::from_profile(profile.clone());
        assert_eq!(form.target_kind, TargetKind::ReplicaSet);
        assert_eq!(form.hosts_text, "a:1\nb:2");
        let rebuilt = form.profile().unwrap();
        assert_eq!(rebuilt.target, profile.target);
        assert_eq!(rebuilt.read_preference, ReadPreference::Nearest);

        let mut bad = blank();
        bad.ssh_enabled = true;
        assert!(bad.profile().is_err());
        bad.ssh_enabled = false;
        assert!(bad.profile().is_ok());
    }

    #[test]
    fn standalone_form_uses_host_and_port() {
        let mut form = blank();
        assert_eq!(form.host, "localhost");
        assert_eq!(form.port, 27017);
        form.host = " db.example ".into();
        form.port = 1234;
        assert_eq!(
            form.profile().unwrap().target,
            Target::Standalone {
                server: HostPort::new("db.example", 1234)
            }
        );
        assert!(form.uri_preview().unwrap().contains("db.example:1234"));
    }

    #[test]
    fn host_lists_need_a_member() {
        let mut form = blank();
        form.target_kind = TargetKind::Sharded;
        assert_eq!(form.profile().unwrap_err(), "add at least one host:port");
        form.hosts_text = "a:1, b:2\n\nc:3".into();
        let Target::Sharded { routers } = form.profile().unwrap().target else {
            panic!("sharded");
        };
        assert_eq!(routers.len(), 3);
        form.hosts_text = "a:x".into();
        assert!(form.profile().is_err());
    }

    #[test]
    fn timeout_is_optional_milliseconds() {
        let mut form = blank();
        assert_eq!(form.profile().unwrap().connect_timeout_ms, None);
        form.timeout = " 2500 ".into();
        assert_eq!(form.profile().unwrap().connect_timeout_ms, Some(2500));
        assert_eq!(form.timeout_error(), None);
        form.timeout = "soon".into();
        assert!(form.timeout_error().is_some());
        assert_eq!(form.profile().unwrap_err(), "timeout must be milliseconds");
    }

    #[test]
    fn ssh_auth_kind_keeps_the_key_path() {
        let mut form = blank();
        form.ssh_enabled = true;
        form.ssh.host = "bastion".into();
        form.ssh.username = "me".into();
        form.ssh_key = "~/.ssh/id".into();
        form.set_ssh_auth_kind(SshAuthKind::Key);
        assert_eq!(form.ssh_auth_kind(), SshAuthKind::Key);
        form.ssh_key = " ~/.ssh/other ".into();
        let ssh = form.profile().unwrap().ssh.unwrap();
        assert_eq!(
            ssh.auth,
            SshAuth::PrivateKey {
                path: "~/.ssh/other".into()
            }
        );
        form.set_ssh_auth_kind(SshAuthKind::Password);
        assert_eq!(form.ssh_auth_kind(), SshAuthKind::Password);
    }

    #[test]
    fn imports_a_uri_with_its_password() {
        let form = ConnectionForm::from_uri("mongodb://me:secret@h:1/").unwrap();
        assert_eq!(form.password, "secret");
        assert_eq!(form.host, "h");
        assert_eq!(form.port, 1);
        assert!(ConnectionForm::from_uri("http://nope").is_err());
    }

    #[test]
    fn secrets_follow_the_form() {
        let secrets = MemorySecretStore::default();
        let mut form = blank();
        form.password = "pw".into();
        let profile = form.profile().unwrap();
        form.store_secrets(&profile, &secrets);
        let loaded = ConnectionForm::from_stored(&profile, &secrets);
        assert_eq!(loaded.password, "pw");
        assert_eq!(loaded.ssh_password, "");
        form.password.clear();
        form.store_secrets(&profile, &secrets);
        assert_eq!(secrets.get(&profile.password_key()).unwrap(), None);
    }

    fn store() -> ProfileStore {
        let mut store = ProfileStore::default();
        for (name, folder) in [
            ("Local", ""),
            ("Prod EU", "Production/EU"),
            ("Prod US", "Production"),
            ("Dev", "Dev"),
        ] {
            store.upsert(ConnectionProfile {
                name: name.into(),
                folder: folder.into(),
                ..ConnectionProfile::default()
            });
        }
        store
    }

    fn names(entries: &[ListEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                ListEntry::Folder { name, depth, .. } => format!("{depth}/{name}/"),
                ListEntry::Profile { profile, depth } => format!("{depth}:{}", profile.name),
            })
            .collect()
    }

    #[test]
    fn list_entries_group_by_folder() {
        let store = store();
        let folders = store.folder_paths();
        let all = list_entries(&store, &folders, "", &HashSet::new());
        assert_eq!(
            names(&all),
            [
                "0:Local",
                "0/Dev/",
                "1:Dev",
                "0/Production/",
                "1:Prod US",
                "1/EU/",
                "2:Prod EU"
            ]
        );
        let filtered = list_entries(&store, &folders, "eu", &HashSet::new());
        assert_eq!(names(&filtered), ["0/Production/", "1/EU/", "2:Prod EU"]);
        let collapsed = HashSet::from(["Production".to_owned()]);
        let hidden = list_entries(&store, &folders, "", &collapsed);
        assert_eq!(
            names(&hidden),
            ["0:Local", "0/Dev/", "1:Dev", "0/Production/"]
        );
    }

    #[test]
    fn first_profile_prefers_the_root() {
        assert_eq!(first_profile(&store()).unwrap().name, "Local");
        assert!(first_profile(&ProfileStore::default()).is_none());
    }
}
