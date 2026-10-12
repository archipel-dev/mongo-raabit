//! What the app saves survives its updates (`docs/safety.md`): the connection file of
//! every format version loads (fixtures in `tests/fixtures/`, one per version, never
//! changed once released), fields written by a newer version are kept and saved back, a
//! file that cannot be read is backed up and never saved over, and saving is atomic and
//! keeps the previous file.

use mongo_raabit_core::persist::sibling;
use mongo_raabit_core::profile::{
    AuthMechanism, ProfileStore, ReadPreference, STORE_VERSION, Target,
};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A copy of a fixture in a temporary directory (tests never write to the fixtures).
fn copy_of(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    std::fs::copy(fixture(name), &path).unwrap();
    (dir, path)
}

/// The content every fixture holds, whatever its version.
fn assert_fixture_content(store: &ProfileStore) {
    let names: Vec<&str> = store.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Sales (prod)", "Local", "Atlas \"cluster0\""]);
    let sales = &store.profiles[0];
    assert_eq!(sales.id, "65f000000000000000000001");
    assert_eq!(sales.folder, "Production/EU");
    assert!(sales.read_only);
    assert!(sales.tls.enabled);
    assert_eq!(sales.auth.username, "al@ice");
    assert_eq!(sales.auth.auth_database, "admin");
    assert_eq!(sales.read_preference, ReadPreference::SecondaryPreferred);
    assert_eq!(sales.connect_timeout_ms, Some(5000));
    assert_eq!(
        sales.extra_options,
        [("retryWrites".to_owned(), "false".to_owned())]
    );
    assert!(
        matches!(&sales.target, Target::ReplicaSet { set_name, members } if set_name == "rs0" && members.len() == 2)
    );
    let local = &store.profiles[1];
    assert_eq!(local.auth.mechanism, AuthMechanism::None);
    assert_eq!(local.ssh.as_ref().unwrap().host, "bastion.example.net");
    assert!(
        matches!(&store.profiles[2].target, Target::Srv { host } if host == "cluster0.example.net")
    );
    assert_eq!(store.folders, ["Staging"]);
}

#[test]
fn every_released_format_loads() {
    for name in ["connections-v0.json", "connections-v1.json"] {
        let store = ProfileStore::load(&fixture(name)).unwrap();
        assert_eq!(store.version, STORE_VERSION, "{name} is migrated");
        assert_fixture_content(&store);
        assert!(store.unknown.is_empty(), "{name}");
    }
    assert_eq!(
        ProfileStore::load(&fixture("connections-v0.json")).unwrap(),
        ProfileStore::load(&fixture("connections-v1.json")).unwrap(),
        "a file from before versions is the same store"
    );
}

#[test]
fn an_old_file_is_saved_in_the_current_format_without_losing_anything() {
    let (_dir, path) = copy_of("connections-v0.json");
    let opened = ProfileStore::open(&path);
    assert!(opened.writable && opened.problem.is_none());
    opened.store.save(&path).unwrap();
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], STORE_VERSION);
    let reloaded = ProfileStore::load(&path).unwrap();
    assert_eq!(reloaded, opened.store);
    assert_fixture_content(&reloaded);
    // The file as it was is kept next to it.
    assert_eq!(
        std::fs::read(sibling(&path, ".bak")).unwrap(),
        std::fs::read(fixture("connections-v0.json")).unwrap()
    );
}

#[test]
fn fields_of_a_newer_version_are_kept_and_saved_back() {
    let (_dir, path) = copy_of("connections-from-newer-version.json");
    let opened = ProfileStore::open(&path);
    assert!(opened.writable, "{:?}", opened.problem);
    let mut store = opened.store;
    assert_eq!(store.version, 2, "a newer version is not downgraded");
    assert_fixture_content(&store);
    // A change made with this version...
    store.profiles[1].name = "Local (renamed)".into();
    store.save(&path).unwrap();
    // ...keeps everything the newer version wrote.
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("connections-from-newer-version.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["workspaces"], original["workspaces"]);
    assert_eq!(saved["profiles"][0]["color"], "teal");
    assert_eq!(
        saved["profiles"][0]["auth"]["oidc_issuer"],
        "https://login.example.net"
    );
    assert_eq!(saved["profiles"][0]["tls"]["min_version"], "1.3");
    assert_eq!(
        saved["profiles"][1]["ssh"]["jump_hosts"],
        original["profiles"][1]["ssh"]["jump_hosts"]
    );
    assert_eq!(saved["profiles"][1]["name"], "Local (renamed)");
    let mut expected = original.clone();
    expected["profiles"][1]["name"] = "Local (renamed)".into();
    assert_eq!(saved, expected, "nothing else changed");
}

/// The files next to `path` whose names start with `<name>.bak-`.
fn backups(path: &Path) -> Vec<PathBuf> {
    let prefix = format!("{}.bak-", path.file_name().unwrap().to_string_lossy());
    let mut list: Vec<PathBuf> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&prefix)
        })
        .collect();
    list.sort();
    list
}

#[test]
fn a_file_that_cannot_be_read_is_backed_up_and_never_saved_over() {
    for (name, contents) in [
        (
            "truncated",
            b"{\n  \"version\": 1,\n  \"profiles\": [\n    { \"id\": \"a\"".to_vec(),
        ),
        ("not json", b"\x00\x01garbage".to_vec()),
        (
            "unknown variant",
            std::fs::read(fixture("connections-unknown-mechanism.json")).unwrap(),
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(&path, &contents).unwrap();
        let opened = ProfileStore::open(&path);
        assert!(!opened.writable, "{name}");
        let problem = opened.problem.unwrap();
        assert!(
            problem.contains("Could not read the saved connections"),
            "{name}: {problem}"
        );
        assert!(problem.contains("not saved"), "{name}: {problem}");
        assert!(opened.store.profiles.is_empty());
        let backups = backups(&path);
        assert_eq!(backups.len(), 1, "{name}: {backups:?}");
        assert!(
            problem.contains(&backups[0].display().to_string()),
            "{problem}"
        );
        assert_eq!(std::fs::read(&backups[0]).unwrap(), contents, "{name}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            contents,
            "{name}: left as it was"
        );
    }
}

#[test]
fn a_missing_file_is_an_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let opened = ProfileStore::open(&dir.path().join("connections.json"));
    assert!(opened.writable && opened.problem.is_none());
    assert_eq!(opened.store, ProfileStore::default());
    assert_eq!(opened.store.version, STORE_VERSION);
}

#[test]
fn saving_is_atomic_and_keeps_the_previous_file() {
    let (_dir, path) = copy_of("connections-v1.json");
    let mut store = ProfileStore::load(&path).unwrap();
    store.folders.push("Archive".into());
    store.save(&path).unwrap();
    assert!(
        !sibling(&path, ".tmp").exists(),
        "the temporary file was renamed"
    );
    assert_eq!(
        std::fs::read(sibling(&path, ".bak")).unwrap(),
        std::fs::read(fixture("connections-v1.json")).unwrap()
    );
    assert_eq!(ProfileStore::load(&path).unwrap(), store);
    // A save that cannot complete leaves the file as it was: a directory where the
    // temporary file must go makes the write fail before anything is replaced.
    std::fs::create_dir(sibling(&path, ".tmp")).unwrap();
    let before = std::fs::read(&path).unwrap();
    store.folders.push("Never".into());
    assert!(store.save(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
