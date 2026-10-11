//! Shared helpers for tests that need a running `mongod`.
//!
//! Set `MONGO_RAABIT_TEST_URI` to point at a server (default `mongodb://127.0.0.1:27017`).
//! When none is reachable the tests print a skip message and pass, so CI without
//! MongoDB stays green; with a server they exercise the real driver paths.

#![allow(dead_code)]

use mongo_raabit_core::Connection;
use mongo_raabit_core::profile::ConnectionProfile;
use mongo_raabit_core::secrets::MemorySecretStore;

pub fn test_uri() -> String {
    std::env::var("MONGO_RAABIT_TEST_URI").unwrap_or_else(|_| "mongodb://127.0.0.1:27017".into())
}

/// Connect to the test server, or `None` (with a message) when it is unavailable.
pub async fn connect(test: &str) -> Option<(Connection, String)> {
    let (mut profile, password) = ConnectionProfile::from_uri(&test_uri()).ok()?;
    profile.connect_timeout_ms = Some(1500);
    let secrets = MemorySecretStore::default();
    if let Some(password) = password {
        use mongo_raabit_core::secrets::SecretStore;
        secrets.set(&profile.password_key(), &password).ok()?;
    }
    match Connection::connect(&profile, &secrets).await {
        Ok(connection) => {
            let db = format!("raabit_test_{test}");
            let _ = connection.drop_database(&db).await;
            Some((connection, db))
        }
        Err(err) => {
            eprintln!(
                "skipping {test}: no MongoDB server at {} ({err}); set MONGO_RAABIT_TEST_URI",
                test_uri()
            );
            None
        }
    }
}
