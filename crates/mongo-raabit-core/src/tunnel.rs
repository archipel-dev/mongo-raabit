//! SSH tunnels through the system `ssh` client (`ssh -N -L`).
//!
//! Using the installed OpenSSH client keeps host-key verification, agents, `~/.ssh/config`
//! and hardware keys working exactly as the user configured them. The executable is
//! discovered on `PATH` at runtime; when it is missing the error says so.

use crate::error::{RaabitError, Result};
use crate::profile::{HostPort, SshAuth, SshSettings};
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A running local port forward; the `ssh` process is stopped on drop.
#[derive(Debug)]
pub struct SshTunnel {
    child: Child,
    local: HostPort,
    _askpass: Option<tempfile_guard::TempScript>,
}

impl SshTunnel {
    /// Forward a free local port to `remote` through `settings`.
    pub fn open(
        settings: &SshSettings,
        remote: &HostPort,
        password: Option<&str>,
        timeout: Duration,
    ) -> Result<Self> {
        let ssh = find_ssh().ok_or_else(|| {
            RaabitError::Tunnel(
                "no `ssh` executable on PATH; install the OpenSSH client to use SSH tunnels".into(),
            )
        })?;
        let local_port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|addr| addr.port())?;
        let local = HostPort::new("127.0.0.1", local_port);
        let mut command = Command::new(ssh);
        command
            .args(tunnel_args(settings, remote, local_port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let askpass = match (&settings.auth, password) {
            (SshAuth::Password, Some(password)) => {
                let script = tempfile_guard::TempScript::askpass()?;
                command
                    .env("SSH_ASKPASS", script.path())
                    .env("SSH_ASKPASS_REQUIRE", "force")
                    .env("RAABIT_SSH_SECRET", password)
                    .env(
                        "DISPLAY",
                        std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()),
                    );
                Some(script)
            }
            (SshAuth::Password, None) => {
                return Err(RaabitError::Tunnel("SSH password is not set".into()));
            }
            _ => None,
        };
        let mut child = command
            .spawn()
            .map_err(|e| RaabitError::Tunnel(format!("could not start ssh: {e}")))?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = child.try_wait()? {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                let detail = stderr.trim();
                return Err(RaabitError::Tunnel(if detail.is_empty() {
                    format!("ssh exited with {status}")
                } else {
                    detail.to_owned()
                }));
            }
            if TcpStream::connect_timeout(
                &([127, 0, 0, 1], local_port).into(),
                Duration::from_millis(100),
            )
            .is_ok()
            {
                return Ok(Self {
                    child,
                    local,
                    _askpass: askpass,
                });
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RaabitError::Tunnel(format!(
                    "timed out after {}s waiting for the forward to {}@{}",
                    timeout.as_secs(),
                    settings.username,
                    settings.host
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Local endpoint the driver should connect to.
    pub fn local(&self) -> &HostPort {
        &self.local
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Arguments passed to `ssh` (exposed for tests and diagnostics).
pub fn tunnel_args(settings: &SshSettings, remote: &HostPort, local_port: u16) -> Vec<String> {
    let mut args = vec![
        "-N".to_owned(),
        "-T".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
        "-o".to_owned(),
        "ServerAliveInterval=30".to_owned(),
        "-o".to_owned(),
        "StrictHostKeyChecking=accept-new".to_owned(),
        "-L".to_owned(),
        format!(
            "127.0.0.1:{local_port}:{}:{}",
            if remote.host.contains(':') {
                format!("[{}]", remote.host)
            } else {
                remote.host.clone()
            },
            remote.port
        ),
        "-p".to_owned(),
        settings.port.to_string(),
    ];
    match &settings.auth {
        SshAuth::PrivateKey { path } => {
            args.extend([
                "-i".to_owned(),
                path.display().to_string(),
                "-o".to_owned(),
                "BatchMode=yes".to_owned(),
            ]);
        }
        SshAuth::Agent => args.extend(["-o".to_owned(), "BatchMode=yes".to_owned()]),
        SshAuth::Password => args.extend([
            "-o".to_owned(),
            "PreferredAuthentications=password,keyboard-interactive".to_owned(),
            "-o".to_owned(),
            "NumberOfPasswordPrompts=1".to_owned(),
        ]),
    }
    args.push("-l".to_owned());
    args.push(settings.username.clone());
    args.push("--".to_owned());
    args.push(settings.host.clone());
    args
}

/// Locate `ssh` on `PATH`.
pub fn find_ssh() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

mod tempfile_guard {
    use std::path::{Path, PathBuf};

    /// A short-lived askpass helper that prints the secret from the environment, so
    /// the password never appears on a command line.
    #[derive(Debug)]
    pub struct TempScript {
        path: PathBuf,
    }

    impl TempScript {
        pub fn askpass() -> std::io::Result<Self> {
            let unique = mongodb::bson::oid::ObjectId::new().to_hex();
            let (name, body) = if cfg!(windows) {
                (
                    format!("raabit-askpass-{unique}.cmd"),
                    "@echo off\r\necho %RAABIT_SSH_SECRET%\r\n",
                )
            } else {
                (
                    format!("raabit-askpass-{unique}.sh"),
                    "#!/bin/sh\nprintf '%s\\n' \"$RAABIT_SSH_SECRET\"\n",
                )
            };
            let path = std::env::temp_dir().join(name);
            std::fs::write(&path, body)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(Self { path })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempScript {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_forward_arguments() {
        let settings = SshSettings {
            host: "bastion.example".into(),
            port: 2222,
            username: "ops".into(),
            auth: SshAuth::PrivateKey {
                path: "/keys/id_ed25519".into(),
            },
        };
        let args = tunnel_args(&settings, &HostPort::new("10.0.0.5", 27017), 40001);
        let joined = args.join(" ");
        assert!(joined.contains("-L 127.0.0.1:40001:10.0.0.5:27017"));
        assert!(joined.contains("-p 2222"));
        assert!(joined.contains("-i /keys/id_ed25519"));
        assert!(joined.ends_with("-l ops -- bastion.example"));
    }

    #[test]
    fn password_auth_requires_secret() {
        let settings = SshSettings {
            host: "h".into(),
            username: "u".into(),
            auth: SshAuth::Password,
            ..SshSettings::default()
        };
        let err = SshTunnel::open(
            &settings,
            &HostPort::new("db", 27017),
            None,
            Duration::from_secs(1),
        );
        assert!(err.is_err());
    }
}
