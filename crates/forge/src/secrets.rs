//! Storage of access tokens. Secrets never appear in `Debug` / `Display`
//! output of anything in this module.

use keyring_core::Entry;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock, PoisonError};

/// Keychain service name; the account id is the keychain user.
const SERVICE: &str = "dev.v1rus.gitwarp";

/// Why a secret could not be read or written; the text is shown in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// No usable OS secret store (no Secret Service on D-Bus, unsupported platform).
    Unavailable(String),
    Failed(String),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Unavailable(why) => write!(f, "No secret store available: {why}"),
            SecretError::Failed(why) => write!(f, "Secret store error: {why}"),
        }
    }
}

impl std::error::Error for SecretError {}

/// Keyed storage of tokens by account id.
pub trait SecretStore: Send + Sync {
    fn get(&self, account_id: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, account_id: &str, secret: &str) -> Result<(), SecretError>;
    /// Deleting a secret that does not exist is not an error.
    fn delete(&self, account_id: &str) -> Result<(), SecretError>;
}

/// In-process store for tests, or a fallback the app may choose when the OS
/// store is unavailable (secrets then live only until exit).
#[derive(Default)]
pub struct MemoryStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.secrets.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore")
            .field("entries", &self.lock().len())
            .finish()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, account_id: &str) -> Result<Option<String>, SecretError> {
        Ok(self.lock().get(account_id).cloned())
    }

    fn set(&self, account_id: &str, secret: &str) -> Result<(), SecretError> {
        self.lock()
            .insert(account_id.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, account_id: &str) -> Result<(), SecretError> {
        self.lock().remove(account_id);
        Ok(())
    }
}

/// The operating system's secret store (Secret Service on Linux, Keychain on
/// macOS, Credential Manager on Windows) through `keyring-core`.
#[derive(Debug, Clone, Copy)]
pub struct KeychainStore {
    _private: (),
}

impl KeychainStore {
    /// Picks the platform store and installs it as the `keyring-core` default
    /// (once per process). Fails with [`SecretError::Unavailable`] when the
    /// platform has no usable store.
    pub fn init() -> Result<KeychainStore, SecretError> {
        static INIT: OnceLock<Result<(), String>> = OnceLock::new();
        INIT.get_or_init(|| {
            platform_store()
                .map(keyring_core::set_default_store)
                .map_err(|e| e.to_string())
        })
        .clone()
        .map(|()| KeychainStore { _private: () })
        .map_err(SecretError::Unavailable)
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::api::CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::api::CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(windows)]
fn platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::api::CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(not(any(
    all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ),
    target_os = "macos",
    windows
)))]
fn platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::api::CredentialStore>> {
    Err(keyring_core::Error::NotSupportedByStore(
        "this platform has no secret store".to_string(),
    ))
}

fn map_error(error: keyring_core::Error) -> SecretError {
    match error {
        keyring_core::Error::NoStorageAccess(_) | keyring_core::Error::NoDefaultStore => {
            SecretError::Unavailable(error.to_string())
        }
        other => SecretError::Failed(other.to_string()),
    }
}

fn entry(account_id: &str) -> Result<Entry, SecretError> {
    Entry::new(SERVICE, account_id).map_err(map_error)
}

impl SecretStore for KeychainStore {
    fn get(&self, account_id: &str) -> Result<Option<String>, SecretError> {
        match entry(account_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_error(e)),
        }
    }

    fn set(&self, account_id: &str, secret: &str) -> Result<(), SecretError> {
        entry(account_id)?.set_password(secret).map_err(map_error)
    }

    fn delete(&self, account_id: &str) -> Result<(), SecretError> {
        match entry(account_id)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_error(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_get_set_delete() {
        let store = MemoryStore::new();
        assert_eq!(store.get("a").unwrap(), None);
        store.set("a", "tok1").unwrap();
        store.set("b", "tok2").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("tok1"));
        store.set("a", "tok3").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("tok3"));
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
        assert_eq!(store.get("b").unwrap().as_deref(), Some("tok2"));
        store.delete("missing").unwrap();
    }

    #[test]
    fn memory_store_debug_hides_secrets() {
        let store = MemoryStore::new();
        store.set("a", "super-secret").unwrap();
        let text = format!("{store:?}");
        assert!(!text.contains("super-secret"), "{text}");
    }

    #[test]
    #[ignore = "touches the real OS keychain"]
    fn keychain_round_trip() {
        let store = KeychainStore::init().expect("secret store available");
        let id = format!("gitwarp-test-{}", std::process::id());
        assert_eq!(store.get(&id).unwrap(), None);
        store.set(&id, "round-trip-secret").unwrap();
        assert_eq!(
            store.get(&id).unwrap().as_deref(),
            Some("round-trip-secret")
        );
        store.delete(&id).unwrap();
        assert_eq!(store.get(&id).unwrap(), None);
        store.delete(&id).unwrap();
    }
}
