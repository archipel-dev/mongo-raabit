//! The standalone host: the [`PluginHost`] services the `raabit` window offers its plugin.

use archipel_sdk::api::{
    EditorTabRequest, Notification, Permission, PluginError, PluginHost, Value,
};
use std::path::{Path, PathBuf};

pub struct StandaloneHost {
    id: String,
    permissions: Vec<Permission>,
    data_dir: Option<PathBuf>,
    /// Latest status-bar message, shown by nothing yet: the views keep their own status bar.
    pub status: String,
    pub repaint: bool,
}

impl StandaloneHost {
    pub fn new(
        id: impl Into<String>,
        permissions: Vec<Permission>,
        data_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            id: id.into(),
            permissions,
            data_dir,
            status: String::new(),
            repaint: false,
        }
    }
}

/// Per-user directory of the app: `$XDG_DATA_HOME/archipel/raabit`, `~/.local/share/...`,
/// `%APPDATA%\archipel\raabit` or `~/Library/Application Support/archipel/raabit`.
pub fn default_data_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")))
    };
    base.map(|b| b.join("archipel").join("raabit"))
}

impl PluginHost for StandaloneHost {
    fn plugin_id(&self) -> &str {
        &self.id
    }

    fn project_root(&self) -> Option<&Path> {
        None
    }

    fn show_status(&mut self, message: &str) {
        self.status = message.into();
    }

    fn notify(&mut self, notification: Notification) {
        self.status = format!("{}: {}", notification.title, notification.body);
    }

    fn dispatch(&mut self, _command: &str, _params: Value) {}

    fn open_tool_window(&mut self, _id: &str) {}

    fn open_editor_tab(&mut self, _request: EditorTabRequest) {}

    fn setting(&self, _key: &str) -> Option<Value> {
        None
    }

    fn set_setting(&mut self, _key: &str, _value: Value) -> Result<(), PluginError> {
        Ok(())
    }

    fn data_dir(&self) -> Result<PathBuf, PluginError> {
        let dir = self
            .data_dir
            .clone()
            .ok_or_else(|| PluginError::Io(self.id.clone(), "no per-user data directory".into()))?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| PluginError::Io(self.id.clone(), e.to_string()))?;
        Ok(dir)
    }

    fn has_permission(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    fn refresh_contributions(&mut self) {}

    fn request_repaint(&mut self) {
        self.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_only_declared_permissions() {
        let host = StandaloneHost::new("x.y", vec![Permission::Network], None);
        assert!(host.require_permission(Permission::Network).is_ok());
        assert!(host.require_permission(Permission::Process).is_err());
        assert!(host.data_dir().is_err());
    }

    #[test]
    fn creates_the_data_dir_on_demand() {
        let dir = std::env::temp_dir().join(format!("raabit-host-test-{}", std::process::id()));
        let host = StandaloneHost::new("x.y", vec![], Some(dir.join("data")));
        assert!(host.data_dir().unwrap().is_dir());
        let _ = std::fs::remove_dir_all(dir);
    }
}
