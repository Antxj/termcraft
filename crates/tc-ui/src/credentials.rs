//! Saved-password policy for one SSH connection (shell tab or tunnel):
//! auto-fill a saved password once per target (gateway and server are
//! separate targets), remember what the user typed, and after a successful
//! login save it (Always) or offer to (Ask).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tc_core::settings::SavePasswords;
use tc_ssh::SecretKind;
use tc_vault::{Target, Vault};

pub enum PromptAction {
    /// Answer with this saved password, without showing a prompt.
    AutoFill(String),
    /// Ask the user; `saved_rejected` = a saved password was just refused.
    Ask { saved_rejected: bool },
}

/// What a prompt's secret is saved under: the server login, or the key file.
pub fn target_for(
    kind: SecretKind,
    user: &str,
    host: &str,
    port: u16,
    key_path: Option<&str>,
) -> Target {
    match (kind, key_path) {
        (SecretKind::KeyPassphrase, Some(path)) => Target::key_passphrase(path),
        _ => Target::ssh(user, host, port),
    }
}

pub struct Credentials {
    vault: Arc<Vault>,
    mode: SavePasswords,
    /// "Save SSH keys passphrases as well".
    save_key_passphrases: bool,
    tried: HashSet<Target>,
    typed: Vec<(Target, String)>,
    /// Passwords of named credentials used by this connection.
    preset: HashMap<Target, String>,
}

impl Credentials {
    pub fn new(vault: Arc<Vault>, mode: SavePasswords, save_key_passphrases: bool) -> Self {
        Self {
            vault,
            mode,
            save_key_passphrases,
            tried: HashSet::new(),
            typed: Vec::new(),
            preset: HashMap::new(),
        }
    }

    /// Forget per-connection state (new connection attempt).
    pub fn reset(&mut self) {
        self.tried.clear();
        self.typed.clear();
        self.preset.clear();
    }

    /// `[name]` in the username field logs in with that named credential.
    /// Returns the username to use and an error to show, if any.
    pub fn use_named(&mut self, user: &str, host: &str, port: u16) -> (String, Option<String>) {
        self.use_named_as("ssh", user, host, port)
    }

    /// [`Self::use_named`] for another protocol ("rdp" …).
    pub fn use_named_as(
        &mut self,
        protocol: &str,
        user: &str,
        host: &str,
        port: u16,
    ) -> (String, Option<String>) {
        let Some(name) = user.strip_prefix('[').and_then(|u| u.strip_suffix(']')) else {
            return (user.to_string(), None);
        };
        match self.vault.credential(name) {
            Ok(Some((username, password))) => {
                let mut target = Target::ssh(&username, host, port);
                target.protocol = protocol.to_string();
                self.preset.insert(target, password);
                (username, None)
            }
            Ok(None) => (
                user.to_string(),
                Some(format!("there is no credential named \"{name}\"")),
            ),
            Err(e) => (
                user.to_string(),
                Some(format!("credential \"{name}\": {e}")),
            ),
        }
    }

    fn saves(&self, kind: SecretKind) -> bool {
        self.mode != SavePasswords::Never
            && match kind {
                SecretKind::Password => true,
                SecretKind::KeyPassphrase => self.save_key_passphrases,
                SecretKind::Challenge => false,
            }
    }

    pub fn on_prompt(&mut self, kind: SecretKind, target: &Target) -> PromptAction {
        if kind == SecretKind::Password
            && let Some(password) = self.preset.remove(target)
        {
            self.tried.insert(target.clone());
            return PromptAction::AutoFill(password);
        }
        if !self.saves(kind) {
            return PromptAction::Ask {
                saved_rejected: false,
            };
        }
        if self.tried.insert(target.clone()) {
            if let Ok(Some(saved)) = self.vault.get(target) {
                return PromptAction::AutoFill(saved);
            }
            return PromptAction::Ask {
                saved_rejected: false,
            };
        }
        let typed_before = self.typed.iter().any(|(t, _)| t == target);
        PromptAction::Ask {
            saved_rejected: !typed_before && self.vault.get(target).ok().flatten().is_some(),
        }
    }

    pub fn on_typed(&mut self, kind: SecretKind, target: Target, secret: String) {
        if self.saves(kind) {
            self.typed.retain(|(t, _)| *t != target);
            self.typed.push((target, secret));
        }
    }

    /// Login succeeded: returns the passwords to offer for saving (Ask) or
    /// saves them (Always). Errors are returned as messages.
    pub fn after_login(&mut self) -> (Vec<(Target, String)>, Vec<String>) {
        let typed = std::mem::take(&mut self.typed);
        match self.mode {
            SavePasswords::Never => (Vec::new(), Vec::new()),
            SavePasswords::Ask => (typed, Vec::new()),
            SavePasswords::Always => {
                let errors = typed
                    .iter()
                    .filter_map(|(t, p)| self.vault.save(t, p).err())
                    .map(|e| format!("could not save password: {e}"))
                    .collect();
                (Vec::new(), errors)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_credential_fills_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let v = Arc::new(Vault::open(
            dir.path().join("i.toml"),
            Box::new(tc_vault::MemoryStore::default()),
        ));
        v.save_credential("lab", "admin", "lab-pw").unwrap();
        let mut c = Credentials::new(v, SavePasswords::Never, false);
        assert_eq!(c.use_named("root", "h", 22), ("root".into(), None));
        let (user, err) = c.use_named("[missing]", "h", 22);
        assert_eq!(user, "[missing]");
        assert!(err.is_some());
        assert_eq!(c.use_named("[lab]", "h", 22), ("admin".into(), None));
        let t = Target::ssh("admin", "h", 22);
        assert!(matches!(
            c.on_prompt(SecretKind::Password, &t),
            PromptAction::AutoFill(p) if p == "lab-pw"
        ));
        assert!(matches!(
            c.on_prompt(SecretKind::Password, &t),
            PromptAction::Ask { .. }
        ));
    }

    fn vault() -> (tempfile::TempDir, Arc<Vault>) {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(
            dir.path().join("i.toml"),
            Box::new(tc_vault::MemoryStore::default()),
        );
        (dir, Arc::new(v))
    }

    #[test]
    fn autofill_once_then_ask_and_offer() {
        let (_d, v) = vault();
        let gw = Target::ssh("u", "gw", 22);
        let srv = Target::ssh("u", "srv", 22);
        v.save(&gw, "gw-pass").unwrap();
        let mut c = Credentials::new(v, SavePasswords::Ask, false);

        assert!(
            matches!(c.on_prompt(SecretKind::Password, &gw), PromptAction::AutoFill(p) if p == "gw-pass")
        );
        // second prompt for the same target: saved one was rejected
        assert!(matches!(
            c.on_prompt(SecretKind::Password, &gw),
            PromptAction::Ask {
                saved_rejected: true
            }
        ));
        c.on_typed(SecretKind::Password, gw.clone(), "new-gw".into());
        // the server has nothing saved
        assert!(matches!(
            c.on_prompt(SecretKind::Password, &srv),
            PromptAction::Ask {
                saved_rejected: false
            }
        ));
        c.on_typed(SecretKind::Password, srv.clone(), "srv-pass".into());
        let (offers, errors) = c.after_login();
        assert!(errors.is_empty());
        assert_eq!(
            offers,
            vec![(gw, "new-gw".into()), (srv, "srv-pass".into())]
        );
    }

    #[test]
    fn never_and_non_password_prompts() {
        let (_d, v) = vault();
        let t = Target::ssh("u", "h", 22);
        v.save(&t, "x").unwrap();
        let mut c = Credentials::new(v.clone(), SavePasswords::Never, false);
        assert!(matches!(
            c.on_prompt(SecretKind::Password, &t),
            PromptAction::Ask { .. }
        ));
        let mut c = Credentials::new(v, SavePasswords::Always, false);
        assert!(matches!(
            c.on_prompt(SecretKind::KeyPassphrase, &t),
            PromptAction::Ask { .. }
        ));
        c.on_typed(SecretKind::KeyPassphrase, t, "pp".into());
        assert_eq!(c.after_login().0.len(), 0, "passphrases are never offered");
    }
}
