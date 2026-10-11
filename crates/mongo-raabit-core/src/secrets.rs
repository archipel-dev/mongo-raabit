//! Secret storage abstraction. Profiles reference secrets by key; the host decides
//! where they live (OS keychain in the IDE, memory in tests).

use crate::error::Result;
use std::collections::HashMap;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, secret: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// Process-local store; secrets are lost on exit.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.lock().get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<()> {
        self.lock().insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.lock().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip() {
        let store = MemorySecretStore::default();
        assert_eq!(store.get("k").unwrap(), None);
        store.set("k", "v").unwrap();
        assert_eq!(store.get("k").unwrap().as_deref(), Some("v"));
        store.delete("k").unwrap();
        assert_eq!(store.get("k").unwrap(), None);
    }
}
