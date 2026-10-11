//! Running scripts in the installed `mongosh` (discovered on `PATH`, never bundled).
//!
//! Each run is a fresh `mongosh --nodb` process that connects with `connect()` to the
//! URI passed in the environment, so credentials never appear on a command line.

use crate::connection::Connection;
use crate::error::{RaabitError, Result};
use crate::shell;
use mongodb::bson::Bson;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Output of one script run.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub elapsed: Duration,
    /// The printed result parsed back into BSON, when it is a document or an array.
    pub value: Option<Bson>,
    /// mongosh paginated the cursor (`Type "it" for more`).
    pub more: bool,
}

/// Locate `mongosh` on `PATH`.
pub fn find_mongosh() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "mongosh.exe"
    } else {
        "mongosh"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// `mongosh --version`, for the shell tab header.
pub async fn version(path: &std::path::Path) -> Option<String> {
    let output = tokio::process::Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Parse mongosh's printed result (shell syntax) into BSON.
pub fn parse_printed(stdout: &str) -> (Option<Bson>, bool) {
    let more_marker = "Type \"it\" for more";
    let more = stdout.trim_end().ends_with(more_marker);
    let text = stdout.trim_end().trim_end_matches(more_marker).trim_end();
    if text.is_empty() {
        return (None, more);
    }
    // The result is the last top-level block; earlier lines may be print() output.
    let starts: Vec<usize> = text
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .chain(std::iter::once(0))
        .filter(|&i| text[i..].starts_with('[') || text[i..].starts_with('{'))
        .collect();
    let mut candidates = starts;
    candidates.sort_unstable();
    for start in candidates {
        if let Ok(value @ (Bson::Document(_) | Bson::Array(_))) = shell::parse_value(&text[start..])
        {
            return (Some(value), more);
        }
    }
    (None, more)
}

impl Connection {
    /// Run `script` in mongosh against this connection's database `db`.
    pub async fn run_mongosh(
        &self,
        mongosh: &std::path::Path,
        db: &str,
        script: &str,
        timeout: Duration,
    ) -> Result<ShellOutput> {
        // Point the URI at `db` so `db` in the script is the selected database.
        let uri = with_database(self.connection_uri(), db);
        // The prelude shares line 1 with the script so error positions stay right.
        let code = format!("db = connect(process.env.RAABIT_MONGO_URI); {script}");
        let start = Instant::now();
        let child = tokio::process::Command::new(mongosh)
            .args(["--nodb", "--quiet", "--norc", "--eval", &code])
            .env("RAABIT_MONGO_URI", uri)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| RaabitError::Invalid(format!("could not start mongosh: {e}")))?;
        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| {
                RaabitError::Invalid(format!(
                    "mongosh did not finish within {}s",
                    timeout.as_secs()
                ))
            })??;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let (value, more) = parse_printed(&stdout);
        Ok(ShellOutput {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
            elapsed: start.elapsed(),
            stdout,
            value,
            more,
        })
    }
}

/// Replace the database path segment of a connection string.
pub fn with_database(uri: &str, db: &str) -> String {
    let (scheme, rest) = uri.split_once("://").unwrap_or(("mongodb", uri));
    let (before_query, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };
    let authority = before_query
        .split_once('/')
        .map_or(before_query, |(a, _)| a);
    let mut out = format!("{scheme}://{authority}/{db}");
    if let Some(query) = query {
        out.push('?');
        out.push_str(query);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_printed_results() {
        let out = "hello\n[\n  {\n    _id: ObjectId('6aca46613fe433f7bba2f154'),\n    price: Decimal128('89.90')\n  }\n]\n";
        let (value, more) = parse_printed(out);
        assert!(!more);
        let Some(Bson::Array(items)) = value else {
            panic!("array expected")
        };
        assert_eq!(items.len(), 1);
        let (value, more) = parse_printed("[\n  { a: 1 }\n]\nType \"it\" for more\n");
        assert!(more);
        assert!(value.is_some());
        assert_eq!(parse_printed("5\n").0, None);
        assert_eq!(parse_printed("").0, None);
    }

    #[test]
    fn rewrites_database() {
        assert_eq!(
            with_database("mongodb://u:p@h:1/old?tls=true", "new"),
            "mongodb://u:p@h:1/new?tls=true"
        );
        assert_eq!(with_database("mongodb://h", "x"), "mongodb://h/x");
        assert_eq!(
            with_database("mongodb+srv://c.example.net/", "y"),
            "mongodb+srv://c.example.net/y"
        );
    }
}
