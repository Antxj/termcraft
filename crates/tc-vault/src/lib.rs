//! Saved passwords. Secrets live only in the OS credential store (Windows
//! Credential Manager, macOS Keychain, Secret Service on Linux); TermCraft
//! keeps a local *index of names* so the "Passwords settings" dialog can
//! list and delete entries — never the secrets themselves.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

const SERVICE: &str = "TermCraft";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("credential store: {0}")]
    Store(String),
    #[error("password index {path}: {message}")]
    Index { path: String, message: String },
}

/// Where secrets are kept. `OsKeychain` in the app, `MemoryStore` in tests.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, VaultError>;
    fn set(&self, key: &str, secret: &str) -> Result<(), VaultError>;
    fn delete(&self, key: &str) -> Result<(), VaultError>;
}

pub struct OsKeychain;

impl SecretStore for OsKeychain {
    fn get(&self, key: &str) -> Result<Option<String>, VaultError> {
        match entry(key)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(VaultError::Store(e.to_string())),
        }
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), VaultError> {
        entry(key)?
            .set_password(secret)
            .map_err(|e| VaultError::Store(e.to_string()))
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        match entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(VaultError::Store(e.to_string())),
        }
    }
}

fn entry(key: &str) -> Result<keyring::Entry, VaultError> {
    keyring::Entry::new(SERVICE, key).map_err(|e| VaultError::Store(e.to_string()))
}

#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>, VaultError> {
        Ok(self.0.lock().expect("lock").get(key).cloned())
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), VaultError> {
        self.0
            .lock()
            .expect("lock")
            .insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.0.lock().expect("lock").remove(key);
        Ok(())
    }
}

/// One saved credential, identified without its secret.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Target {
    pub protocol: String,
    pub username: String,
    /// `host:port`.
    pub server: String,
}

impl Target {
    pub fn ssh(user: &str, host: &str, port: u16) -> Self {
        Self {
            protocol: "ssh".into(),
            username: user.into(),
            server: format!("{host}:{port}"),
        }
    }

    /// Key in the credential store, e.g. `ssh://root@10.0.0.1:22`.
    pub fn key(&self) -> String {
        format!("{}://{}@{}", self.protocol, self.username, self.server)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default, rename = "saved")]
    targets: BTreeSet<Target>,
}

pub struct Vault {
    store: Box<dyn SecretStore>,
    index_path: PathBuf,
    index: Mutex<Index>,
}

impl Vault {
    pub fn open(index_path: PathBuf, store: Box<dyn SecretStore>) -> Self {
        let index = std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            store,
            index_path,
            index: Mutex::new(index),
        }
    }

    pub fn list(&self) -> Vec<Target> {
        self.index
            .lock()
            .expect("lock")
            .targets
            .iter()
            .cloned()
            .collect()
    }

    pub fn get(&self, target: &Target) -> Result<Option<String>, VaultError> {
        self.store.get(&target.key())
    }

    pub fn save(&self, target: &Target, secret: &str) -> Result<(), VaultError> {
        self.store.set(&target.key(), secret)?;
        let mut index = self.index.lock().expect("lock");
        index.targets.insert(target.clone());
        self.write_index(&index)
    }

    pub fn forget(&self, target: &Target) -> Result<(), VaultError> {
        self.store.delete(&target.key())?;
        let mut index = self.index.lock().expect("lock");
        index.targets.remove(target);
        self.write_index(&index)
    }

    pub fn forget_all(&self) -> Result<(), VaultError> {
        for t in self.list() {
            self.forget(&t)?;
        }
        Ok(())
    }

    fn write_index(&self, index: &Index) -> Result<(), VaultError> {
        let err = |message: String| VaultError::Index {
            path: self.index_path.display().to_string(),
            message,
        };
        if let Some(dir) = self.index_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| err(e.to_string()))?;
        }
        let text = toml::to_string_pretty(index).map_err(|e| err(e.to_string()))?;
        std::fs::write(&self.index_path, text).map_err(|e| err(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_get_forget_and_index_has_no_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved_passwords.toml");
        let vault = Vault::open(path.clone(), Box::new(MemoryStore::default()));
        let t = Target::ssh("root", "10.0.0.1", 22);
        assert_eq!(t.key(), "ssh://root@10.0.0.1:22");

        vault.save(&t, "s3cret-value").unwrap();
        assert_eq!(vault.get(&t).unwrap().as_deref(), Some("s3cret-value"));
        assert_eq!(vault.list(), vec![t.clone()]);
        let index = std::fs::read_to_string(&path).unwrap();
        assert!(index.contains("10.0.0.1:22"));
        assert!(
            !index.contains("s3cret-value"),
            "index must not contain secrets"
        );

        // Reopen: the index survives, the secret comes from the store.
        let store = MemoryStore::default();
        store.set(&t.key(), "s3cret-value").unwrap();
        let vault = Vault::open(path.clone(), Box::new(store));
        assert_eq!(vault.list(), vec![t.clone()]);

        vault.forget(&t).unwrap();
        assert!(vault.list().is_empty());
        assert_eq!(vault.get(&t).unwrap(), None);
    }

    /// Round-trip through the real OS store (run with `--ignored`; CI Linux
    /// runners have no Secret Service).
    #[test]
    #[ignore]
    fn os_keychain_roundtrip() {
        let key = "test://termcraft-selftest@localhost:0";
        OsKeychain.set(key, "temporary-test-value").unwrap();
        assert_eq!(
            OsKeychain.get(key).unwrap().as_deref(),
            Some("temporary-test-value")
        );
        OsKeychain.delete(key).unwrap();
        assert_eq!(OsKeychain.get(key).unwrap(), None);
        OsKeychain.delete(key).unwrap(); // deleting twice is fine
    }

    #[test]
    fn forget_all() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path().join("i.toml"), Box::new(MemoryStore::default()));
        vault.save(&Target::ssh("a", "h1", 22), "1").unwrap();
        vault.save(&Target::ssh("b", "h2", 2222), "2").unwrap();
        vault.forget_all().unwrap();
        assert!(vault.list().is_empty());
    }
}
