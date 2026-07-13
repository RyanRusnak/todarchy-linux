// keystore.rs — per-project symmetric-key storage.
//
// Production uses the OS keyring via the `keyring` crate, which on Linux
// talks to libsecret (gnome-keyring / KWallet / kde-wallet through the
// secret-service D-Bus API). The service name is identical to the iOS
// app's keychain namespace (`com.todarchy.app.shared-keys`), so if a
// Linux + macOS install happen to coexist on the same hardware (rare
// but possible in container/VM setups) they look at the same logical
// vault.
//
// Tests use the in-memory implementation so CI runs without needing a
// session D-Bus.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::cryptobox::KEY_BYTES;

/// Keyring service name. Matches `kSecAttrService` on iOS so the two
/// platforms agree on where a project's key lives logically.
pub const SERVICE: &str = "com.todarchy.app.shared-keys";

#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error("keyring backend error: {0}")]
    Backend(String),
    #[error("stored key was not 32 bytes (got {0})")]
    WrongLength(usize),
}

pub trait KeyStore: Send + Sync {
    fn save(&self, project_id: &str, key: &[u8; KEY_BYTES]) -> Result<(), KeyStoreError>;
    fn load(&self, project_id: &str) -> Result<Option<[u8; KEY_BYTES]>, KeyStoreError>;
    fn delete(&self, project_id: &str) -> Result<(), KeyStoreError>;
}

// MARK: - libsecret-backed implementation

/// Wraps `keyring::Entry`. One Entry per (service, account) pair; we
/// construct on demand rather than caching so the secret-service
/// connection stays managed by the crate.
pub struct LibSecretKeyStore {
    service: String,
}

impl LibSecretKeyStore {
    pub fn new() -> Self {
        Self { service: SERVICE.to_string() }
    }

    fn entry(&self, project_id: &str) -> Result<keyring::Entry, KeyStoreError> {
        keyring::Entry::new(&self.service, project_id)
            .map_err(|e| KeyStoreError::Backend(e.to_string()))
    }
}

impl Default for LibSecretKeyStore {
    fn default() -> Self { Self::new() }
}

impl KeyStore for LibSecretKeyStore {
    fn save(&self, project_id: &str, key: &[u8; KEY_BYTES]) -> Result<(), KeyStoreError> {
        let entry = self.entry(project_id)?;
        entry
            .set_secret(key)
            .map_err(|e| KeyStoreError::Backend(e.to_string()))
    }

    fn load(&self, project_id: &str) -> Result<Option<[u8; KEY_BYTES]>, KeyStoreError> {
        let entry = self.entry(project_id)?;
        match entry.get_secret() {
            Ok(bytes) => {
                if bytes.len() != KEY_BYTES {
                    return Err(KeyStoreError::WrongLength(bytes.len()));
                }
                let mut out = [0u8; KEY_BYTES];
                out.copy_from_slice(&bytes);
                Ok(Some(out))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(KeyStoreError::Backend(e.to_string())),
        }
    }

    fn delete(&self, project_id: &str) -> Result<(), KeyStoreError> {
        let entry = self.entry(project_id)?;
        match entry.delete_credential() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KeyStoreError::Backend(e.to_string())),
        }
    }
}

// MARK: - In-memory test implementation

/// Thread-safe in-memory store for tests. Doesn't touch D-Bus, so CI
/// passes without a session bus / running secret service.
pub struct InMemoryKeyStore {
    storage: Mutex<HashMap<String, [u8; KEY_BYTES]>>,
}

impl InMemoryKeyStore {
    pub fn new() -> Self {
        Self { storage: Mutex::new(HashMap::new()) }
    }
}

impl Default for InMemoryKeyStore {
    fn default() -> Self { Self::new() }
}

impl KeyStore for InMemoryKeyStore {
    fn save(&self, project_id: &str, key: &[u8; KEY_BYTES]) -> Result<(), KeyStoreError> {
        self.storage.lock().unwrap().insert(project_id.to_string(), *key);
        Ok(())
    }

    fn load(&self, project_id: &str) -> Result<Option<[u8; KEY_BYTES]>, KeyStoreError> {
        Ok(self.storage.lock().unwrap().get(project_id).copied())
    }

    fn delete(&self, project_id: &str) -> Result<(), KeyStoreError> {
        self.storage.lock().unwrap().remove(project_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_round_trips_a_key() {
        let store = InMemoryKeyStore::new();
        let key = [7u8; KEY_BYTES];
        store.save("p1", &key).unwrap();
        assert_eq!(store.load("p1").unwrap(), Some(key));
    }

    #[test]
    fn in_memory_returns_none_when_absent() {
        let store = InMemoryKeyStore::new();
        assert_eq!(store.load("does-not-exist").unwrap(), None);
    }

    #[test]
    fn in_memory_overwrites_on_resave() {
        let store = InMemoryKeyStore::new();
        store.save("p1", &[1u8; KEY_BYTES]).unwrap();
        store.save("p1", &[2u8; KEY_BYTES]).unwrap();
        assert_eq!(store.load("p1").unwrap(), Some([2u8; KEY_BYTES]));
    }

    #[test]
    fn in_memory_delete_removes_entry() {
        let store = InMemoryKeyStore::new();
        store.save("p1", &[3u8; KEY_BYTES]).unwrap();
        store.delete("p1").unwrap();
        assert_eq!(store.load("p1").unwrap(), None);
    }

    #[test]
    fn in_memory_delete_is_idempotent() {
        let store = InMemoryKeyStore::new();
        // Never saved, but delete shouldn't error — matches the
        // libsecret impl's "NoEntry = success" behavior so callers can
        // treat "ensure forgotten" as a single no-question-asked call.
        store.delete("never-existed").unwrap();
    }
}
