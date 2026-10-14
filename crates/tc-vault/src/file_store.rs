//! "Configuration file" password storage: every secret in one file,
//! encrypted with a key derived from the Master Password (Argon2id, then
//! AES-256-GCM). The file is locked until `unlock`. With the OS keychain
//! storage the same file, holding no secrets, verifies the Master Password.

use crate::{SecretStore, VaultError};
use aes_gcm::aead::{Aead, Generate, Key, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use zeroize::{Zeroize, Zeroizing};

const VERSION: u32 = 1;

/// Argon2id cost (OWASP minimum: 19 MiB, 2 passes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct KdfParams {
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

const DEFAULT_KDF: KdfParams = KdfParams {
    m_cost: 19 * 1024,
    t_cost: 2,
    p_cost: 1,
};

/// On-disk format. `salt`, `nonce` and `data` are hex.
#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    kdf: String,
    #[serde(flatten)]
    params: KdfParams,
    salt: String,
    nonce: String,
    data: String,
}

impl Envelope {
    /// Header fields bound to the ciphertext as associated data.
    fn aad(&self) -> Vec<u8> {
        format!(
            "termcraft-vault/{}/{}/{}/{}/{}/{}",
            self.version,
            self.kdf,
            self.params.m_cost,
            self.params.t_cost,
            self.params.p_cost,
            self.salt
        )
        .into_bytes()
    }
}

pub type MasterKey = Zeroizing<[u8; 32]>;

struct Open {
    key: MasterKey,
    salt: Vec<u8>,
    params: KdfParams,
    secrets: BTreeMap<String, String>,
}

impl Drop for Open {
    fn drop(&mut self) {
        for v in self.secrets.values_mut() {
            v.zeroize();
        }
    }
}

pub struct EncryptedFile {
    path: PathBuf,
    open: Mutex<Option<Open>>,
}

fn derive(password: &str, salt: &[u8], params: KdfParams) -> Result<MasterKey, VaultError> {
    let p = argon2::Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32))
        .map_err(|e| VaultError::Crypto(e.to_string()))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, p);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| VaultError::Crypto(e.to_string()))?;
    Ok(key)
}

fn random_salt() -> Vec<u8> {
    // 32 random bytes from the OS generator.
    Key::<Aes256Gcm>::generate().to_vec()
}

fn unhex(field: &str, value: &str) -> Result<Vec<u8>, VaultError> {
    hex::decode(value).map_err(|_| VaultError::Corrupt(format!("bad {field}")))
}

impl EncryptedFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            open: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    pub fn is_unlocked(&self) -> bool {
        self.open.lock().expect("lock").is_some()
    }

    pub fn lock(&self) {
        *self.open.lock().expect("lock") = None;
    }

    /// Start a new, empty file protected by `master` (replaces any file).
    pub fn create(&self, master: &str) -> Result<(), VaultError> {
        let salt = random_salt();
        let open = Open {
            key: derive(master, &salt, DEFAULT_KDF)?,
            salt,
            params: DEFAULT_KDF,
            secrets: BTreeMap::new(),
        };
        self.write(&open)?;
        *self.open.lock().expect("lock") = Some(open);
        Ok(())
    }

    pub fn unlock(&self, master: &str) -> Result<(), VaultError> {
        let env = self.read_envelope()?;
        let salt = unhex("salt", &env.salt)?;
        let key = derive(master, &salt, env.params)?;
        self.unlock_inner(env, key)
    }

    /// Unlock with a key from [`Self::key`] (cached in the OS keychain).
    pub fn unlock_with_key(&self, key: MasterKey) -> Result<(), VaultError> {
        let env = self.read_envelope()?;
        self.unlock_inner(env, key)
    }

    /// Check `master` without changing the lock state.
    pub fn verify(&self, master: &str) -> Result<bool, VaultError> {
        let env = self.read_envelope()?;
        let salt = unhex("salt", &env.salt)?;
        let key = derive(master, &salt, env.params)?;
        match decrypt(&env, &key) {
            Ok(_) => Ok(true),
            Err(VaultError::WrongPassword) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The derived key of the unlocked file.
    pub fn key(&self) -> Option<MasterKey> {
        self.open
            .lock()
            .expect("lock")
            .as_ref()
            .map(|o| o.key.clone())
    }

    /// Re-encrypt under a new Master Password (the file must be unlocked).
    pub fn change_master(&self, new_master: &str) -> Result<(), VaultError> {
        let mut guard = self.open.lock().expect("lock");
        let open = guard.as_mut().ok_or(VaultError::Locked)?;
        let salt = random_salt();
        open.key = derive(new_master, &salt, DEFAULT_KDF)?;
        open.salt = salt;
        open.params = DEFAULT_KDF;
        self.write(open)
    }

    /// Delete the file (and forget the key).
    pub fn remove(&self) -> Result<(), VaultError> {
        self.lock();
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(self.file_err(e)),
        }
    }

    /// Names of the stored secrets (the file must be unlocked).
    pub fn keys(&self) -> Result<Vec<String>, VaultError> {
        let guard = self.open.lock().expect("lock");
        let open = guard.as_ref().ok_or(VaultError::Locked)?;
        Ok(open.secrets.keys().cloned().collect())
    }

    fn unlock_inner(&self, env: Envelope, key: MasterKey) -> Result<(), VaultError> {
        let plain = decrypt(&env, &key)?;
        let text = std::str::from_utf8(&plain)
            .map_err(|_| VaultError::Corrupt("contents are not UTF-8".into()))?;
        let secrets: BTreeMap<String, String> =
            toml::from_str(text).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        *self.open.lock().expect("lock") = Some(Open {
            key,
            salt: unhex("salt", &env.salt)?,
            params: env.params,
            secrets,
        });
        Ok(())
    }

    fn read_envelope(&self) -> Result<Envelope, VaultError> {
        let text = std::fs::read_to_string(&self.path).map_err(|e| self.file_err(e))?;
        let env: Envelope =
            toml::from_str(&text).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        if env.version != VERSION || env.kdf != "argon2id" {
            return Err(VaultError::Corrupt(format!(
                "unsupported format {} / {}",
                env.version, env.kdf
            )));
        }
        Ok(env)
    }

    fn write(&self, open: &Open) -> Result<(), VaultError> {
        let plain = Zeroizing::new(
            toml::to_string(&open.secrets).map_err(|e| VaultError::Corrupt(e.to_string()))?,
        );
        let nonce = Nonce::generate();
        let mut env = Envelope {
            version: VERSION,
            kdf: "argon2id".into(),
            params: open.params,
            salt: hex::encode(&open.salt),
            nonce: hex::encode(nonce.as_slice()),
            data: String::new(),
        };
        let cipher = Aes256Gcm::new_from_slice(open.key.as_ref())
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        let aad = env.aad();
        let data = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plain.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        env.data = hex::encode(data);
        let text = toml::to_string_pretty(&env).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| self.file_err(e))?;
        }
        // Write a sibling file, then rename over the old one.
        let tmp = self.path.with_extension("vault.tmp");
        std::fs::write(&tmp, text).map_err(|e| self.file_err(e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| self.file_err(e))
    }

    fn file_err(&self, e: std::io::Error) -> VaultError {
        VaultError::Index {
            path: self.path.display().to_string(),
            message: e.to_string(),
        }
    }
}

fn decrypt(env: &Envelope, key: &MasterKey) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let nonce = unhex("nonce", &env.nonce)?;
    let nonce = Nonce::try_from(nonce.as_slice())
        .map_err(|_| VaultError::Corrupt("bad nonce length".into()))?;
    let data = unhex("data", &env.data)?;
    let cipher =
        Aes256Gcm::new_from_slice(key.as_ref()).map_err(|e| VaultError::Crypto(e.to_string()))?;
    let aad = env.aad();
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &data,
                aad: &aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| VaultError::WrongPassword)
}

impl SecretStore for EncryptedFile {
    fn get(&self, key: &str) -> Result<Option<String>, VaultError> {
        let guard = self.open.lock().expect("lock");
        let open = guard.as_ref().ok_or(VaultError::Locked)?;
        Ok(open.secrets.get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), VaultError> {
        let mut guard = self.open.lock().expect("lock");
        let open = guard.as_mut().ok_or(VaultError::Locked)?;
        open.secrets.insert(key.into(), secret.into());
        self.write(open)
    }

    fn delete(&self, key: &str) -> Result<(), VaultError> {
        let mut guard = self.open.lock().expect("lock");
        let open = guard.as_mut().ok_or(VaultError::Locked)?;
        if open.secrets.remove(key).is_some() {
            self.write(open)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_unlock_change_and_wrong_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("passwords.vault");
        let f = EncryptedFile::new(path.clone());
        assert!(!f.exists());
        assert!(matches!(f.get("k"), Err(VaultError::Locked)));

        f.create("correct horse").unwrap();
        f.set("ssh://u@h:22", "top-secret-value").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("top-secret-value"));
        assert!(!text.contains("ssh://u@h:22"), "names are encrypted too");

        let g = EncryptedFile::new(path.clone());
        assert!(matches!(g.unlock("wrong"), Err(VaultError::WrongPassword)));
        assert!(!g.is_unlocked());
        assert!(g.verify("correct horse").unwrap());
        assert!(!g.verify("nope").unwrap());
        g.unlock("correct horse").unwrap();
        assert_eq!(
            g.get("ssh://u@h:22").unwrap().as_deref(),
            Some("top-secret-value")
        );

        g.change_master("battery staple").unwrap();
        let h = EncryptedFile::new(path.clone());
        assert!(matches!(
            h.unlock("correct horse"),
            Err(VaultError::WrongPassword)
        ));
        h.unlock("battery staple").unwrap();
        let key = h.key().unwrap();
        h.delete("ssh://u@h:22").unwrap();
        h.lock();
        assert!(matches!(h.get("x"), Err(VaultError::Locked)));
        h.unlock_with_key(key).unwrap();
        assert_eq!(h.get("ssh://u@h:22").unwrap(), None);

        h.remove().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn tampered_header_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("passwords.vault");
        let f = EncryptedFile::new(path.clone());
        f.create("pw").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("t_cost = 2", "t_cost = 1")).unwrap();
        let g = EncryptedFile::new(path);
        assert!(g.unlock("pw").is_err());
    }
}
