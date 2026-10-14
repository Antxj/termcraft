//! Saved passwords, credentials and key passphrases ("Passwords settings").
//!
//! Secrets live either in the OS credential store (Windows Credential
//! Manager, macOS Keychain, Secret Service on Linux) or in an encrypted
//! configuration file unlocked with the Master Password ([`file_store`]).
//! A local *index of names* lets the dialog list entries without reading
//! any secret; it never contains secrets.

pub mod file_store;

pub use file_store::{EncryptedFile, MasterKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use zeroize::Zeroizing;

const SERVICE: &str = "TermCraft";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("credential store: {0}")]
    Store(String),
    #[error("password index {path}: {message}")]
    Index { path: String, message: String },
    #[error("the password vault is locked: enter the Master Password")]
    Locked,
    #[error("wrong Master Password")]
    WrongPassword,
    #[error("no Master Password is set")]
    NoMasterPassword,
    #[error("password vault is damaged: {0}")]
    Corrupt(String),
    #[error("encryption: {0}")]
    Crypto(String),
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

    /// Passphrase of a private key file ("Save SSH keys passphrases as well").
    pub fn key_passphrase(path: &str) -> Self {
        Self {
            protocol: "sshkey".into(),
            username: String::new(),
            server: path.into(),
        }
    }

    /// Key in the credential store, e.g. `ssh://root@10.0.0.1:22`.
    pub fn key(&self) -> String {
        format!("{}://{}@{}", self.protocol, self.username, self.server)
    }
}

/// "Choose where to save passwords".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    /// The OS credential store.
    #[default]
    Keychain,
    /// The encrypted configuration file (needs the Master Password).
    File,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    storage: Storage,
    #[serde(default, rename = "saved")]
    targets: BTreeSet<Target>,
    /// Named credentials: name -> username (the password is a secret).
    #[serde(default)]
    credentials: BTreeMap<String, String>,
}

fn credential_key(name: &str) -> String {
    format!("cred://{name}")
}

/// Keychain entry caching the Master Password key on this computer.
const CACHED_KEY: &str = "master-key://termcraft";

pub struct Vault {
    keychain: Box<dyn SecretStore>,
    file: EncryptedFile,
    index_path: PathBuf,
    index: Mutex<Index>,
}

impl Vault {
    /// `keychain` is the OS store (`MemoryStore` in tests). The encrypted
    /// file sits next to the index as `passwords.vault`.
    pub fn open(index_path: PathBuf, keychain: Box<dyn SecretStore>) -> Self {
        let index = std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            keychain,
            file: EncryptedFile::new(index_path.with_file_name("passwords.vault")),
            index_path,
            index: Mutex::new(index),
        }
    }

    fn store(&self) -> &dyn SecretStore {
        match self.storage() {
            Storage::Keychain => self.keychain.as_ref(),
            Storage::File => &self.file,
        }
    }

    pub fn storage(&self) -> Storage {
        self.index.lock().expect("lock").storage
    }

    pub fn has_master_password(&self) -> bool {
        self.file.exists()
    }

    /// Secrets cannot be read until the Master Password is entered.
    pub fn is_locked(&self) -> bool {
        self.storage() == Storage::File && !self.file.is_unlocked()
    }

    /// Unlock with the Master Password. With `remember`, the derived key is
    /// cached in the OS keychain so this computer unlocks by itself
    /// ("only on new account or new computer").
    pub fn unlock(&self, master: &str, remember: bool) -> Result<(), VaultError> {
        if !self.file.exists() {
            return Err(VaultError::NoMasterPassword);
        }
        self.file.unlock(master)?;
        self.remember_key(remember)
    }

    /// Unlock from the key cached on this computer, if any.
    pub fn try_cached_unlock(&self) -> bool {
        if self.file.is_unlocked() {
            return true;
        }
        let Ok(Some(hex_key)) = self.keychain.get(CACHED_KEY) else {
            return false;
        };
        let Some(key) = hex::decode(hex_key.trim())
            .ok()
            .and_then(|k| <[u8; 32]>::try_from(k).ok())
        else {
            return false;
        };
        self.file.unlock_with_key(Zeroizing::new(key)).is_ok()
    }

    /// Cache (or drop) the unlocked key in the OS keychain.
    pub fn remember_key(&self, remember: bool) -> Result<(), VaultError> {
        match (remember, self.file.key()) {
            (true, Some(key)) => self
                .keychain
                .set(CACHED_KEY, &Zeroizing::new(hex::encode(key.as_ref()))),
            (true, None) => Ok(()),
            (false, _) => self.keychain.delete(CACHED_KEY),
        }
    }

    pub fn lock(&self) {
        self.file.lock();
    }

    /// Check the Master Password ("Show passwords", export).
    pub fn verify_master(&self, master: &str) -> Result<bool, VaultError> {
        if !self.file.exists() {
            return Err(VaultError::NoMasterPassword);
        }
        self.file.verify(master)
    }

    /// Set or change the Master Password. Changing it needs the current one.
    pub fn set_master_password(&self, current: Option<&str>, new: &str) -> Result<(), VaultError> {
        if !self.file.exists() {
            return self.file.create(new);
        }
        if !self.file.is_unlocked() {
            self.file.unlock(current.ok_or(VaultError::Locked)?)?;
        } else if let Some(c) = current
            && !self.file.verify(c)?
        {
            return Err(VaultError::WrongPassword);
        }
        let cached = matches!(self.keychain.get(CACHED_KEY), Ok(Some(_)));
        self.file.change_master(new)?;
        self.remember_key(cached)
    }

    /// Remove the Master Password (keychain storage only).
    pub fn remove_master_password(&self, current: &str) -> Result<(), VaultError> {
        if self.storage() == Storage::File {
            return Err(VaultError::Store(
                "the configuration file storage needs a Master Password".into(),
            ));
        }
        if !self.verify_master(current)? {
            return Err(VaultError::WrongPassword);
        }
        self.keychain.delete(CACHED_KEY)?;
        self.file.remove()
    }

    /// Move every secret to `to`. The configuration file needs a Master
    /// Password and must be unlocked.
    pub fn set_storage(&self, to: Storage) -> Result<(), VaultError> {
        let from = self.storage();
        if from == to {
            return Ok(());
        }
        if !self.file.exists() {
            return Err(VaultError::NoMasterPassword);
        }
        if !self.file.is_unlocked() {
            return Err(VaultError::Locked);
        }
        let (src, dst): (&dyn SecretStore, &dyn SecretStore) = match to {
            Storage::File => (self.keychain.as_ref(), &self.file),
            Storage::Keychain => (&self.file, self.keychain.as_ref()),
        };
        let keys = self.secret_keys();
        for key in &keys {
            if let Some(secret) = src.get(key)? {
                dst.set(key, &Zeroizing::new(secret))?;
            }
        }
        {
            let mut index = self.index.lock().expect("lock");
            index.storage = to;
            self.write_index(&index)?;
        }
        for key in &keys {
            src.delete(key)?;
        }
        Ok(())
    }

    fn secret_keys(&self) -> Vec<String> {
        let index = self.index.lock().expect("lock");
        index
            .targets
            .iter()
            .map(Target::key)
            .chain(index.credentials.keys().map(|n| credential_key(n)))
            .collect()
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
        self.store().get(&target.key())
    }

    pub fn save(&self, target: &Target, secret: &str) -> Result<(), VaultError> {
        self.store().set(&target.key(), secret)?;
        let mut index = self.index.lock().expect("lock");
        index.targets.insert(target.clone());
        self.write_index(&index)
    }

    pub fn forget(&self, target: &Target) -> Result<(), VaultError> {
        self.store().delete(&target.key())?;
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

    /// Named credentials: (name, username), sorted by name.
    pub fn credentials(&self) -> Vec<(String, String)> {
        let index = self.index.lock().expect("lock");
        index
            .credentials
            .iter()
            .map(|(n, u)| (n.clone(), u.clone()))
            .collect()
    }

    /// Username and password of a named credential.
    pub fn credential(&self, name: &str) -> Result<Option<(String, String)>, VaultError> {
        let Some(user) = self
            .index
            .lock()
            .expect("lock")
            .credentials
            .get(name)
            .cloned()
        else {
            return Ok(None);
        };
        let password = self.store().get(&credential_key(name))?.unwrap_or_default();
        Ok(Some((user, password)))
    }

    /// Create or replace a named credential.
    pub fn save_credential(
        &self,
        name: &str,
        user: &str,
        password: &str,
    ) -> Result<(), VaultError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(VaultError::Store("a credential needs a name".into()));
        }
        self.store().set(&credential_key(name), password)?;
        let mut index = self.index.lock().expect("lock");
        index.credentials.insert(name.into(), user.into());
        self.write_index(&index)
    }

    pub fn forget_credential(&self, name: &str) -> Result<(), VaultError> {
        self.store().delete(&credential_key(name))?;
        let mut index = self.index.lock().expect("lock");
        index.credentials.remove(name);
        self.write_index(&index)
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

    /// Keychain store shared between two `Vault`s (a restart).
    #[derive(Clone, Default)]
    struct Shared(std::sync::Arc<MemoryStore>);

    impl SecretStore for Shared {
        fn get(&self, key: &str) -> Result<Option<String>, VaultError> {
            self.0.get(key)
        }
        fn set(&self, key: &str, secret: &str) -> Result<(), VaultError> {
            self.0.set(key, secret)
        }
        fn delete(&self, key: &str) -> Result<(), VaultError> {
            self.0.delete(key)
        }
    }

    #[test]
    fn move_secrets_to_the_configuration_file_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("saved_passwords.toml");
        let keychain = Shared::default();
        let vault = Vault::open(index.clone(), Box::new(keychain.clone()));
        let t = Target::ssh("root", "h", 22);
        vault.save(&t, "pw-1").unwrap();
        vault.save_credential("prod", "admin", "pw-2").unwrap();

        assert!(matches!(
            vault.set_storage(Storage::File),
            Err(VaultError::NoMasterPassword)
        ));
        vault.set_master_password(None, "master").unwrap();
        vault.set_storage(Storage::File).unwrap();
        assert_eq!(
            keychain.get(&t.key()).unwrap(),
            None,
            "moved out of the keychain"
        );
        assert_eq!(vault.get(&t).unwrap().as_deref(), Some("pw-1"));

        // Restart: locked until the Master Password is entered.
        let vault = Vault::open(index.clone(), Box::new(keychain.clone()));
        assert_eq!(vault.storage(), Storage::File);
        assert!(vault.is_locked());
        assert!(matches!(vault.get(&t), Err(VaultError::Locked)));
        assert!(!vault.try_cached_unlock());
        assert!(matches!(
            vault.unlock("bad", false),
            Err(VaultError::WrongPassword)
        ));
        vault.unlock("master", true).unwrap();
        assert_eq!(
            vault.credential("prod").unwrap(),
            Some(("admin".into(), "pw-2".into()))
        );

        // Restart on the same computer: the cached key unlocks it.
        let vault = Vault::open(index.clone(), Box::new(keychain.clone()));
        assert!(vault.try_cached_unlock());
        assert!(!vault.is_locked());

        vault
            .set_master_password(Some("master"), "master-2")
            .unwrap();
        assert!(vault.verify_master("master-2").unwrap());
        assert!(!vault.verify_master("master").unwrap());

        vault.set_storage(Storage::Keychain).unwrap();
        assert_eq!(keychain.get(&t.key()).unwrap().as_deref(), Some("pw-1"));
        assert_eq!(vault.credential("prod").unwrap().unwrap().1, "pw-2");
        vault.remove_master_password("master-2").unwrap();
        assert!(!vault.has_master_password());
        assert_eq!(keychain.get(CACHED_KEY).unwrap(), None);
    }

    #[test]
    fn credentials_and_key_passphrases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i.toml");
        let vault = Vault::open(path.clone(), Box::new(MemoryStore::default()));
        assert!(vault.save_credential("  ", "u", "p").is_err());
        vault.save_credential("lab", "root", "secret-lab").unwrap();
        vault
            .save(&Target::key_passphrase("/home/u/.ssh/id_ed25519"), "phrase")
            .unwrap();
        assert_eq!(vault.credentials(), vec![("lab".into(), "root".into())]);
        let index = std::fs::read_to_string(&path).unwrap();
        assert!(!index.contains("secret-lab") && !index.contains("phrase"));
        vault.forget_credential("lab").unwrap();
        assert_eq!(vault.credential("lab").unwrap(), None);
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
