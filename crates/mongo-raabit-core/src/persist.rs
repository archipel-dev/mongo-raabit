//! Files the app keeps on disk, written so that an update of the app, a crash or a full
//! disk never costs the user what they saved (see `docs/safety.md`):
//!
//! - [`write_atomic`]: the new content goes to `<name>.tmp`, is flushed to disk, the
//!   previous file is kept as `<name>.bak`, and only then is the new file renamed over the
//!   old one (an atomic replace), so the file is always either the old or the new
//!   content, never a mix;
//! - [`back_up_unreadable`]: a file that cannot be read is copied to
//!   `<name>.bak-<timestamp>` and left as it is; the caller then reports the problem and
//!   does not save over it.

use crate::error::Result;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// `path` with `suffix` appended to its file name (`connections.json` → `connections.json.tmp`).
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Replace the file at `path` with `contents` atomically, keeping the previous content
/// as `<name>.bak`.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = sibling(path, ".tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents)?;
        // On disk before it replaces anything: a crash leaves the old file or the new one.
        file.sync_all()?;
    }
    if path.exists() {
        std::fs::copy(path, sibling(path, ".bak"))?;
    }
    std::fs::rename(&tmp, path)?;
    sync_dir(path);
    Ok(())
}

/// Flush the directory entry of a rename (Unix; elsewhere the rename is enough).
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(dir) = std::fs::File::open(if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        })
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Copy a file that could not be read to `<name>.bak-<timestamp>` (UTC, e.g.
/// `connections.json.bak-20261012T143005Z`) and return where. The file itself is left
/// untouched.
pub fn back_up_unreadable(path: &Path) -> Result<PathBuf> {
    let now = mongodb::bson::DateTime::now()
        .try_to_rfc3339_string()
        .unwrap_or_default();
    // 2026-10-12T14:30:05.123Z → 20261012T143005Z
    let stamp: String = now
        .split('.')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut backup = sibling(path, &format!(".bak-{stamp}Z"));
    let mut n = 1;
    while backup.exists() {
        n += 1;
        backup = sibling(path, &format!(".bak-{stamp}Z-{n}"));
    }
    std::fs::copy(path, &backup)?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_replace_atomically_and_keep_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep/state.json");
        write_atomic(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        assert!(!sibling(&path, ".bak").exists(), "nothing to keep yet");
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(std::fs::read(sibling(&path, ".bak")).unwrap(), b"one");
        assert!(
            !sibling(&path, ".tmp").exists(),
            "the temporary file was renamed"
        );
    }

    #[test]
    fn unreadable_files_are_copied_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"{ broken").unwrap();
        let first = back_up_unreadable(&path).unwrap();
        let second = back_up_unreadable(&path).unwrap();
        assert_ne!(
            first, second,
            "a second backup does not overwrite the first"
        );
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("state.json.bak-"), "{name}");
        assert!(name.ends_with('Z'), "{name}");
        assert_eq!(std::fs::read(&first).unwrap(), b"{ broken");
        assert_eq!(std::fs::read(&path).unwrap(), b"{ broken", "left as it was");
    }
}
