//! Connection profiles: what the Connection Manager edits and persists.
//!
//! A profile never stores secrets. Passwords live in a [`crate::secrets::SecretStore`]
//! under keys derived from the profile id; [`ConnectionProfile::connection_string`]
//! takes them as arguments.

use crate::error::{RaabitError, Result};
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 27017;

/// `host:port` of one server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

impl HostPort {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    /// Parse `host`, `host:port` or `[ipv6]:port`.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RaabitError::InvalidProfile("empty host".into()));
        }
        let (host, port) = if let Some(rest) = text.strip_prefix('[') {
            let (host, after) = rest
                .split_once(']')
                .ok_or_else(|| RaabitError::InvalidProfile(format!("invalid host '{text}'")))?;
            (host, after.strip_prefix(':'))
        } else {
            match text.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (text, None),
            }
        };
        let port = match port {
            Some(port) => port.parse::<u16>().map_err(|_| {
                RaabitError::InvalidProfile(format!("invalid port '{port}' in '{text}'"))
            })?,
            None => DEFAULT_PORT,
        };
        if host.is_empty() || host.contains(['/', '@', '?', ' ']) {
            return Err(RaabitError::InvalidProfile(format!(
                "invalid host '{text}'"
            )));
        }
        Ok(Self::new(host, port))
    }
}

impl std::fmt::Display for HostPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// Deployment topology the profile connects to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Standalone {
        server: HostPort,
    },
    ReplicaSet {
        members: Vec<HostPort>,
        set_name: String,
    },
    Sharded {
        routers: Vec<HostPort>,
    },
    /// DNS seed list (`mongodb+srv://`).
    Srv {
        host: String,
    },
}

impl Default for Target {
    fn default() -> Self {
        Self::Standalone {
            server: HostPort::new("localhost", DEFAULT_PORT),
        }
    }
}

impl Target {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Standalone { .. } => "Standalone",
            Self::ReplicaSet { .. } => "Replica set",
            Self::Sharded { .. } => "Sharded cluster",
            Self::Srv { .. } => "DNS seed list (SRV)",
        }
    }

    fn hosts(&self) -> Vec<HostPort> {
        match self {
            Self::Standalone { server } => vec![server.clone()],
            Self::ReplicaSet { members, .. } => members.clone(),
            Self::Sharded { routers } => routers.clone(),
            Self::Srv { .. } => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMechanism {
    #[default]
    None,
    /// Let the server negotiate SCRAM-SHA-256 / SCRAM-SHA-1.
    Default,
    ScramSha256,
    ScramSha1,
    X509,
    /// LDAP via SASL PLAIN.
    Ldap,
    /// Kerberos via GSSAPI.
    Kerberos,
    Aws,
}

impl AuthMechanism {
    pub const ALL: [Self; 8] = [
        Self::None,
        Self::Default,
        Self::ScramSha256,
        Self::ScramSha1,
        Self::X509,
        Self::Ldap,
        Self::Kerberos,
        Self::Aws,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Default => "Default (SCRAM)",
            Self::ScramSha256 => "SCRAM-SHA-256",
            Self::ScramSha1 => "SCRAM-SHA-1",
            Self::X509 => "X.509",
            Self::Ldap => "LDAP (PLAIN)",
            Self::Kerberos => "Kerberos (GSSAPI)",
            Self::Aws => "AWS IAM",
        }
    }

    fn uri_name(self) -> Option<&'static str> {
        match self {
            Self::None | Self::Default => None,
            Self::ScramSha256 => Some("SCRAM-SHA-256"),
            Self::ScramSha1 => Some("SCRAM-SHA-1"),
            Self::X509 => Some("MONGODB-X509"),
            Self::Ldap => Some("PLAIN"),
            Self::Kerberos => Some("GSSAPI"),
            Self::Aws => Some("MONGODB-AWS"),
        }
    }

    fn from_uri_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_uppercase().as_str() {
            "SCRAM-SHA-256" => Self::ScramSha256,
            "SCRAM-SHA-1" => Self::ScramSha1,
            "MONGODB-X509" => Self::X509,
            "PLAIN" => Self::Ldap,
            "GSSAPI" => Self::Kerberos,
            "MONGODB-AWS" => Self::Aws,
            _ => return None,
        })
    }

    /// Whether the mechanism uses a password from the secret store.
    pub fn needs_password(self) -> bool {
        matches!(
            self,
            Self::Default | Self::ScramSha256 | Self::ScramSha1 | Self::Ldap
        )
    }

    /// Database the credentials are defined in when the profile leaves it blank.
    fn default_source(self) -> Option<&'static str> {
        match self {
            Self::X509 | Self::Kerberos | Self::Aws | Self::Ldap => Some("$external"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSettings {
    pub mechanism: AuthMechanism,
    #[serde(default)]
    pub username: String,
    /// Authentication database (`authSource`); blank means the mechanism default.
    #[serde(default)]
    pub auth_database: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsSettings {
    pub enabled: bool,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    /// PEM containing the client certificate and key (X.509 authentication).
    #[serde(default)]
    pub certificate_key_file: Option<PathBuf>,
    #[serde(default)]
    pub allow_invalid_certificates: bool,
    #[serde(default)]
    pub allow_invalid_hostnames: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SshAuth {
    /// Keys offered by the running SSH agent or `~/.ssh` defaults.
    Agent,
    PrivateKey {
        path: PathBuf,
    },
    /// Password kept in the secret store.
    Password,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
}

impl Default for SshSettings {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            username: String::new(),
            auth: SshAuth::Agent,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReadPreference {
    #[default]
    Primary,
    PrimaryPreferred,
    Secondary,
    SecondaryPreferred,
    Nearest,
}

impl ReadPreference {
    pub const ALL: [Self; 5] = [
        Self::Primary,
        Self::PrimaryPreferred,
        Self::Secondary,
        Self::SecondaryPreferred,
        Self::Nearest,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::PrimaryPreferred => "primaryPreferred",
            Self::Secondary => "secondary",
            Self::SecondaryPreferred => "secondaryPreferred",
            Self::Nearest => "nearest",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str().eq_ignore_ascii_case(text))
    }
}

/// One saved connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    /// Slash-separated folder path in the Connection Manager, e.g. `Production/EU`.
    #[serde(default)]
    pub folder: String,
    pub target: Target,
    #[serde(default)]
    pub auth: AuthSettings,
    #[serde(default)]
    pub tls: TlsSettings,
    #[serde(default)]
    pub ssh: Option<SshSettings>,
    #[serde(default)]
    pub read_preference: ReadPreference,
    #[serde(default)]
    pub default_database: String,
    #[serde(default)]
    pub app_name: String,
    #[serde(default)]
    pub connect_timeout_ms: Option<u64>,
    /// URI options this model does not know, preserved verbatim on round trips.
    #[serde(default)]
    pub extra_options: Vec<(String, String)>,
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            id: mongodb::bson::oid::ObjectId::new().to_hex(),
            name: "New Connection".into(),
            folder: String::new(),
            target: Target::default(),
            auth: AuthSettings::default(),
            tls: TlsSettings::default(),
            ssh: None,
            read_preference: ReadPreference::default(),
            default_database: String::new(),
            app_name: String::new(),
            connect_timeout_ms: None,
            extra_options: Vec::new(),
        }
    }
}

/// Characters escaped in user names, passwords and option values.
const USERINFO: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'/')
    .add(b':')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b'+')
    .add(b',')
    .add(b'=');

fn encode(text: &str) -> String {
    utf8_percent_encode(text, USERINFO).to_string()
}

fn decode(text: &str) -> Result<String> {
    percent_decode_str(text)
        .decode_utf8()
        .map(|s| s.into_owned())
        .map_err(|_| RaabitError::InvalidProfile("connection string is not valid UTF-8".into()))
}

impl ConnectionProfile {
    /// Secret-store key for the database password.
    pub fn password_key(&self) -> String {
        format!("mongo-raabit/{}/password", self.id)
    }

    /// Secret-store key for the SSH password.
    pub fn ssh_password_key(&self) -> String {
        format!("mongo-raabit/{}/ssh-password", self.id)
    }

    /// Short description for the connection tree tooltip and manager list.
    pub fn summary(&self) -> String {
        match &self.target {
            Target::Srv { host } => format!("mongodb+srv://{host}"),
            target => target
                .hosts()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// Check the profile for mistakes the connection would only report later.
    pub fn validate(&self) -> Result<()> {
        let fail = |m: &str| Err(RaabitError::InvalidProfile(m.to_owned()));
        if self.name.trim().is_empty() {
            return fail("the connection needs a name");
        }
        match &self.target {
            Target::Standalone { server } if server.host.trim().is_empty() => {
                return fail("server host is empty");
            }
            Target::ReplicaSet { members, set_name } => {
                if members.is_empty() {
                    return fail("a replica set needs at least one member");
                }
                if set_name.trim().is_empty() {
                    return fail("replica set name is empty");
                }
            }
            Target::Sharded { routers } if routers.is_empty() => {
                return fail("a sharded cluster needs at least one mongos router");
            }
            Target::Srv { host } if host.trim().is_empty() || host.contains(':') => {
                return fail("SRV host must be a DNS name without a port");
            }
            _ => {}
        }
        if self.auth.mechanism != AuthMechanism::None
            && self.auth.mechanism != AuthMechanism::X509
            && self.auth.mechanism != AuthMechanism::Aws
            && self.auth.username.trim().is_empty()
        {
            return fail("authentication needs a user name");
        }
        if self.auth.mechanism == AuthMechanism::X509 && !self.tls.enabled {
            return fail("X.509 authentication requires TLS");
        }
        if let Some(ssh) = &self.ssh {
            if ssh.host.trim().is_empty() {
                return fail("SSH host is empty");
            }
            if ssh.username.trim().is_empty() {
                return fail("SSH user name is empty");
            }
            if matches!(self.target, Target::Srv { .. }) {
                return fail("SSH tunnels need explicit hosts; SRV seed lists are not supported");
            }
        }
        Ok(())
    }

    /// Build the driver connection string.
    ///
    /// `tunnel` replaces the hosts with a local forwarded endpoint (and forces a direct
    /// connection, since only one server is reachable through the tunnel).
    pub fn connection_string(
        &self,
        password: Option<&str>,
        tunnel: Option<&HostPort>,
    ) -> Result<String> {
        self.validate()?;
        let mut uri = String::from(match (&self.target, tunnel) {
            (Target::Srv { .. }, None) => "mongodb+srv://",
            _ => "mongodb://",
        });
        let mechanism = self.auth.mechanism;
        if mechanism != AuthMechanism::None && !self.auth.username.is_empty() {
            uri.push_str(&encode(&self.auth.username));
            if mechanism.needs_password()
                && let Some(password) = password
            {
                uri.push(':');
                uri.push_str(&encode(password));
            }
            uri.push('@');
        }
        match (tunnel, &self.target) {
            (Some(local), _) => uri.push_str(&local.to_string()),
            (None, Target::Srv { host }) => uri.push_str(host),
            (None, target) => uri.push_str(
                &target
                    .hosts()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        }
        uri.push('/');
        uri.push_str(&encode(&self.default_database));

        let mut options: Vec<(String, String)> = Vec::new();
        let mut opt = |k: &str, v: String| options.push((k.to_owned(), v));
        if let (Target::ReplicaSet { set_name, .. }, None) = (&self.target, tunnel) {
            opt("replicaSet", set_name.clone());
        }
        if tunnel.is_some() {
            opt("directConnection", "true".into());
        }
        if mechanism != AuthMechanism::None {
            if let Some(name) = mechanism.uri_name() {
                opt("authMechanism", name.into());
            }
            let source = if self.auth.auth_database.is_empty() {
                mechanism.default_source().map(str::to_owned)
            } else {
                Some(self.auth.auth_database.clone())
            };
            if let Some(source) = source {
                opt("authSource", source);
            }
        }
        if self.tls.enabled {
            opt("tls", "true".into());
            if let Some(ca) = &self.tls.ca_file {
                opt("tlsCAFile", ca.display().to_string());
            }
            if let Some(cert) = &self.tls.certificate_key_file {
                opt("tlsCertificateKeyFile", cert.display().to_string());
            }
            if self.tls.allow_invalid_certificates {
                opt("tlsAllowInvalidCertificates", "true".into());
            }
            if self.tls.allow_invalid_hostnames {
                opt("tlsAllowInvalidHostnames", "true".into());
            }
        } else if matches!(self.target, Target::Srv { .. }) && tunnel.is_none() {
            // SRV implies TLS unless switched off explicitly.
            opt("tls", "false".into());
        }
        if self.read_preference != ReadPreference::Primary {
            opt("readPreference", self.read_preference.as_str().into());
        }
        if !self.app_name.is_empty() {
            opt("appName", self.app_name.clone());
        }
        if let Some(timeout) = self.connect_timeout_ms {
            opt("connectTimeoutMS", timeout.to_string());
            opt("serverSelectionTimeoutMS", timeout.to_string());
        }
        options.extend(self.extra_options.iter().cloned());
        if !options.is_empty() {
            uri.push('?');
            let query = options
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            uri.push_str(&query);
        }
        Ok(uri)
    }

    /// The connection string with the password masked, for display and logs.
    pub fn display_uri(&self) -> String {
        let password = self.auth.mechanism.needs_password().then_some("****");
        self.connection_string(password, None)
            .unwrap_or_else(|err| err.to_string())
    }

    /// Import a `mongodb://` or `mongodb+srv://` connection string. Returns the profile
    /// and the password, which the caller should move into the secret store.
    pub fn from_uri(uri: &str) -> Result<(Self, Option<String>)> {
        let uri = uri.trim();
        let invalid = |m: String| RaabitError::InvalidProfile(m);
        let (srv, rest) = if let Some(rest) = uri.strip_prefix("mongodb+srv://") {
            (true, rest)
        } else if let Some(rest) = uri.strip_prefix("mongodb://") {
            (false, rest)
        } else {
            return Err(invalid(
                "connection string must start with mongodb:// or mongodb+srv://".into(),
            ));
        };
        let (before_query, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = before_query.split_once('/').unwrap_or((before_query, ""));
        let (userinfo, hosts) = match authority.rsplit_once('@') {
            Some((user, hosts)) => (Some(user), hosts),
            None => (None, authority),
        };
        let mut profile = Self {
            default_database: decode(path)?,
            ..Self::default()
        };
        let mut password = None;
        if let Some(userinfo) = userinfo {
            let (user, pass) = match userinfo.split_once(':') {
                Some((user, pass)) => (user, Some(pass)),
                None => (userinfo, None),
            };
            profile.auth.username = decode(user)?;
            password = pass.map(decode).transpose()?;
            profile.auth.mechanism = AuthMechanism::Default;
        }

        let mut set_name = None;
        let mut direct = false;
        let mut tls_explicit = None;
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = decode(key)?;
            let value = decode(value)?;
            let flag = value.eq_ignore_ascii_case("true");
            match key.to_ascii_lowercase().as_str() {
                "replicaset" => set_name = Some(value),
                "directconnection" => direct = flag,
                "authsource" => profile.auth.auth_database = value,
                "authmechanism" => {
                    profile.auth.mechanism = AuthMechanism::from_uri_name(&value)
                        .ok_or_else(|| invalid(format!("unsupported authMechanism '{value}'")))?;
                }
                "tls" | "ssl" => tls_explicit = Some(flag),
                "tlscafile" | "sslcafile" => profile.tls.ca_file = Some(PathBuf::from(value)),
                "tlscertificatekeyfile" | "sslpemkeyfile" => {
                    profile.tls.certificate_key_file = Some(PathBuf::from(value));
                }
                "tlsallowinvalidcertificates" | "sslallowinvalidcertificates" => {
                    profile.tls.allow_invalid_certificates = flag;
                }
                "tlsallowinvalidhostnames" | "sslallowinvalidhostnames" => {
                    profile.tls.allow_invalid_hostnames = flag;
                }
                "tlsinsecure" => {
                    profile.tls.allow_invalid_certificates = flag;
                    profile.tls.allow_invalid_hostnames = flag;
                }
                "readpreference" => {
                    profile.read_preference = ReadPreference::parse(&value)
                        .ok_or_else(|| invalid(format!("unknown readPreference '{value}'")))?;
                }
                "appname" => profile.app_name = value,
                "connecttimeoutms" => {
                    profile.connect_timeout_ms = value.parse().ok();
                }
                "serverselectiontimeoutms" => {}
                _ => profile.extra_options.push((key, value)),
            }
        }
        if direct {
            profile
                .extra_options
                .push(("directConnection".into(), "true".into()));
        }
        profile.tls.enabled = tls_explicit.unwrap_or(srv);

        if srv {
            if hosts.contains(',') || hosts.contains(':') {
                return Err(invalid(
                    "mongodb+srv:// takes a single host name without a port".into(),
                ));
            }
            profile.target = Target::Srv {
                host: hosts.to_owned(),
            };
        } else {
            let servers = hosts
                .split(',')
                .map(HostPort::parse)
                .collect::<Result<Vec<_>>>()?;
            profile.target = match (set_name, servers.len()) {
                (Some(set_name), _) => Target::ReplicaSet {
                    members: servers,
                    set_name,
                },
                (None, 1) => Target::Standalone {
                    server: servers
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| HostPort::new("localhost", DEFAULT_PORT)),
                },
                (None, _) => Target::Sharded { routers: servers },
            };
        }
        profile.name = profile.summary();
        Ok((profile, password))
    }
}

/// Saved profiles, persisted as JSON. Folders are implied by the profiles' `folder`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileStore {
    #[serde(default)]
    pub profiles: Vec<ConnectionProfile>,
    /// Empty folders the user created explicitly.
    #[serde(default)]
    pub folders: Vec<String>,
}

impl ProfileStore {
    /// Default location: `$XDG_CONFIG_HOME/jetthing/mongo-raabit/connections.json`
    /// (or the platform equivalent).
    pub fn default_path() -> Option<PathBuf> {
        let base = if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join("Library/Application Support"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        };
        base.map(|base| {
            base.join("jetthing")
                .join("mongo-raabit")
                .join("connections.json")
        })
    }

    /// Load the store; a missing file is an empty store.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| RaabitError::Serde(format!("{}: {e}", path.display()))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err.into()),
        }
    }

    /// Save atomically (write to a temporary file, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|e| RaabitError::Serde(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ConnectionProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Insert or replace by id.
    pub fn upsert(&mut self, profile: ConnectionProfile) {
        match self.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<ConnectionProfile> {
        let index = self.profiles.iter().position(|p| p.id == id)?;
        Some(self.profiles.remove(index))
    }

    /// Copy a profile under a new id and "(copy)" name.
    pub fn duplicate(&mut self, id: &str) -> Option<String> {
        let mut copy = self.get(id)?.clone();
        copy.id = mongodb::bson::oid::ObjectId::new().to_hex();
        copy.name.push_str(" (copy)");
        let new_id = copy.id.clone();
        self.profiles.push(copy);
        Some(new_id)
    }

    /// All folder paths (explicit and implied), sorted, including parents.
    pub fn folder_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        for folder in self
            .folders
            .iter()
            .chain(self.profiles.iter().map(|p| &p.folder))
        {
            let mut prefix = String::new();
            for part in folder.split('/').filter(|p| !p.is_empty()) {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                if !paths.contains(&prefix) {
                    paths.push(prefix.clone());
                }
            }
        }
        paths.sort_by_key(|p| p.to_lowercase());
        paths
    }

    /// Profiles directly inside `folder` ("" for the root), sorted by name.
    pub fn profiles_in(&self, folder: &str) -> Vec<&ConnectionProfile> {
        let mut list: Vec<_> = self
            .profiles
            .iter()
            .filter(|p| p.folder.trim_matches('/') == folder)
            .collect();
        list.sort_by_key(|p| p.name.to_lowercase());
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_replica_set_uri_with_options() {
        let (profile, password) = ConnectionProfile::from_uri(
            "mongodb://al%40ice:p%3Ass@db1:27018,db2/sales?replicaSet=rs0&authSource=admin\
             &tls=true&readPreference=secondaryPreferred&appName=raabit&retryWrites=false",
        )
        .unwrap();
        assert_eq!(profile.auth.username, "al@ice");
        assert_eq!(password.as_deref(), Some("p:ss"));
        assert_eq!(profile.default_database, "sales");
        assert_eq!(
            profile.target,
            Target::ReplicaSet {
                members: vec![HostPort::new("db1", 27018), HostPort::new("db2", 27017)],
                set_name: "rs0".into()
            }
        );
        assert!(profile.tls.enabled);
        assert_eq!(profile.read_preference, ReadPreference::SecondaryPreferred);
        assert_eq!(
            profile.extra_options,
            vec![("retryWrites".to_string(), "false".to_string())]
        );
        let uri = profile
            .connection_string(password.as_deref(), None)
            .unwrap();
        let (again, again_password) = ConnectionProfile::from_uri(&uri).unwrap();
        assert_eq!(again.target, profile.target);
        assert_eq!(again.auth, profile.auth);
        assert_eq!(again_password, password);
        assert_eq!(again.extra_options, profile.extra_options);
    }

    #[test]
    fn srv_defaults_to_tls_and_masks_password() {
        let (mut profile, _) =
            ConnectionProfile::from_uri("mongodb+srv://u:secret@cluster0.example.net/").unwrap();
        assert_eq!(
            profile.target,
            Target::Srv {
                host: "cluster0.example.net".into()
            }
        );
        assert!(profile.tls.enabled);
        assert!(!profile.display_uri().contains("secret"));
        assert!(profile.display_uri().contains("u:****@"));
        profile.tls.enabled = false;
        assert!(
            profile
                .connection_string(None, None)
                .unwrap()
                .contains("tls=false")
        );
        assert!(ConnectionProfile::from_uri("mongodb+srv://a:1/").is_err());
    }

    #[test]
    fn tunnel_rewrites_hosts() {
        let profile = ConnectionProfile {
            target: Target::ReplicaSet {
                members: vec![HostPort::new("10.0.0.5", 27017)],
                set_name: "rs".into(),
            },
            ssh: Some(SshSettings {
                host: "bastion".into(),
                username: "ops".into(),
                ..SshSettings::default()
            }),
            ..ConnectionProfile::default()
        };
        let uri = profile
            .connection_string(None, Some(&HostPort::new("127.0.0.1", 40000)))
            .unwrap();
        assert_eq!(uri, "mongodb://127.0.0.1:40000/?directConnection=true");
    }

    #[test]
    fn validation_catches_mistakes() {
        let mut profile = ConnectionProfile::default();
        profile.auth.mechanism = AuthMechanism::ScramSha256;
        assert!(profile.validate().is_err());
        profile.auth.username = "bob".into();
        assert!(profile.validate().is_ok());
        profile.auth.mechanism = AuthMechanism::X509;
        assert!(profile.validate().is_err());
        assert!(ConnectionProfile::from_uri("http://x").is_err());
        assert!(ConnectionProfile::from_uri("mongodb://h:99999").is_err());
        assert!(ConnectionProfile::from_uri("mongodb://h/?authMechanism=NOPE").is_err());
        let (x509, _) =
            ConnectionProfile::from_uri("mongodb://CN=me@h/?authMechanism=MONGODB-X509&tls=true")
                .unwrap();
        assert!(
            x509.connection_string(None, None)
                .unwrap()
                .contains("authSource=$external")
        );
    }

    #[test]
    fn store_round_trip_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/connections.json");
        let mut store = ProfileStore::load(&path).unwrap();
        assert!(store.profiles.is_empty());
        let mut a = ConnectionProfile {
            name: "b-local".into(),
            folder: "Prod/EU".into(),
            ..ConnectionProfile::default()
        };
        store.upsert(a.clone());
        a.name = "a-local".into();
        store.upsert(a.clone());
        assert_eq!(store.profiles.len(), 1);
        let copy = store.duplicate(&a.id).unwrap();
        assert_ne!(copy, a.id);
        store.folders.push("Dev".into());
        store.save(&path).unwrap();
        let loaded = ProfileStore::load(&path).unwrap();
        assert_eq!(loaded, store);
        assert_eq!(loaded.folder_paths(), ["Dev", "Prod", "Prod/EU"]);
        assert_eq!(loaded.profiles_in("Prod/EU").len(), 2);
        assert_eq!(loaded.profiles_in("Prod/EU")[0].name, "a-local");
        assert!(store.remove(&a.id).is_some());
    }
}
