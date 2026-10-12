//! The app never saves over a connections file it could not read: it reports the problem,
//! keeps a backup, and holds changes in memory for the run.

use mongo_raabit::MongoRaabit;
use mongo_raabit::core::profile::ConnectionProfile;

#[test]
fn an_unreadable_connections_file_is_never_saved_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let contents = b"{ \"profiles\": [ { \"id\": ".to_vec();
    std::fs::write(&path, &contents).unwrap();
    let mut app = MongoRaabit::new(Some(path.clone()));
    assert!(
        app.status
            .starts_with("Could not read the saved connections"),
        "{}",
        app.status
    );
    assert_eq!(app.store_path, None, "saving is off for this run");
    app.store.upsert(ConnectionProfile::default());
    app.save_store();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        contents,
        "the file is untouched"
    );
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("connections.json.bak-")
        })
        .count();
    assert_eq!(backups, 1);
}

#[test]
fn a_readable_file_is_saved_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let mut app = MongoRaabit::new(Some(path.clone()));
    assert!(app.status.is_empty(), "{}", app.status);
    app.store.upsert(ConnectionProfile::default());
    app.save_store();
    app.store.upsert(ConnectionProfile::default());
    app.save_store();
    assert!(path.exists());
    assert!(dir.path().join("connections.json.bak").exists());
    assert!(!dir.path().join("connections.json.tmp").exists());
    assert_eq!(MongoRaabit::new(Some(path)).store.profiles.len(), 2);
}
