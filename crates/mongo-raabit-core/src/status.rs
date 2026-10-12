//! The sentences the app says about what it is doing: the status line, results of
//! background jobs, and the connection string handed to code generation.

use crate::connection::ServerInfo;
use crate::format;
use crate::profile::ConnectionProfile;

pub fn connecting(name: &str) -> String {
    format!("Connecting to {name}…")
}

pub fn connected(name: &str, server: &ServerInfo) -> String {
    format!(
        "Connected to {name} — MongoDB {} ({})",
        server.version, server.topology
    )
}

pub fn connection_failed(name: &str) -> String {
    format!("Connection to {name} failed")
}

pub fn disconnected(name: &str) -> String {
    format!("Disconnected from {name}")
}

/// What Test Connection found.
pub fn test_result(info: &ServerInfo) -> String {
    format!(
        "Connected: MongoDB {} · {} · {}",
        info.version,
        info.topology,
        format::duration(info.round_trip)
    )
}

pub fn deleted_documents(n: u64) -> String {
    format!("Deleted {n} document{}", if n == 1 { "" } else { "s" })
}

/// What the status bar says: connections, the last message, whether a job runs.
pub fn status_line(connected: usize, message: &str, working: bool) -> String {
    let mut line = if connected > 0 {
        format!("{connected} connected")
    } else {
        "Not connected".to_owned()
    };
    if !message.is_empty() {
        line.push_str(" · ");
        line.push_str(message);
    }
    if working {
        line.push_str(" · Working…");
    }
    line
}

/// The connection string of generated code: the profile's, without a real password.
pub fn code_uri(profile: Option<&ConnectionProfile>) -> String {
    profile
        .map(|p| p.display_uri().replace(":****@", ":<password>@"))
        .unwrap_or_else(|| "mongodb://localhost:27017".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn server() -> ServerInfo {
        ServerInfo {
            version: "7.0.1".into(),
            topology: "Standalone".into(),
            is_writable_primary: true,
            max_wire_version: 21,
            round_trip: Duration::from_millis(12),
        }
    }

    #[test]
    fn connection_messages() {
        assert_eq!(connecting("local"), "Connecting to local…");
        assert_eq!(
            connected("local", &server()),
            "Connected to local — MongoDB 7.0.1 (Standalone)"
        );
        assert_eq!(connection_failed("local"), "Connection to local failed");
        assert_eq!(disconnected("local"), "Disconnected from local");
        assert_eq!(
            test_result(&server()),
            "Connected: MongoDB 7.0.1 · Standalone · 12 ms"
        );
    }

    #[test]
    fn counts_are_pluralized() {
        assert_eq!(deleted_documents(1), "Deleted 1 document");
        assert_eq!(deleted_documents(3), "Deleted 3 documents");
    }

    #[test]
    fn status_line_joins_the_parts() {
        assert_eq!(status_line(0, "", false), "Not connected");
        assert_eq!(
            status_line(2, "Saved", true),
            "2 connected · Saved · Working…"
        );
    }

    #[test]
    fn code_never_carries_the_password() {
        let (profile, _) = ConnectionProfile::from_uri("mongodb://me:secret@h:1/").unwrap();
        let uri = code_uri(Some(&profile));
        assert!(uri.contains(":<password>@"), "{uri}");
        assert!(!uri.contains("secret"));
        assert_eq!(code_uri(None), "mongodb://localhost:27017");
    }
}
