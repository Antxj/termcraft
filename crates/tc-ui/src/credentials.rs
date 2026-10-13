//! Saved-password policy for one SSH connection (shell tab or tunnel):
//! auto-fill a saved password once per target (gateway and server are
//! separate targets), remember what the user typed, and after a successful
//! login save it (Always) or offer to (Ask).

use std::collections::HashSet;
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

pub struct Credentials {
    vault: Arc<Vault>,
    mode: SavePasswords,
    tried: HashSet<Target>,
    typed: Vec<(Target, String)>,
}

impl Credentials {
    pub fn new(vault: Arc<Vault>, mode: SavePasswords) -> Self {
        Self {
            vault,
            mode,
            tried: HashSet::new(),
            typed: Vec::new(),
        }
    }

    /// Forget per-connection state (new connection attempt).
    pub fn reset(&mut self) {
        self.tried.clear();
        self.typed.clear();
    }

    pub fn on_prompt(&mut self, kind: SecretKind, target: &Target) -> PromptAction {
        if kind != SecretKind::Password || self.mode == SavePasswords::Never {
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
        if kind == SecretKind::Password {
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
        let mut c = Credentials::new(v, SavePasswords::Ask);

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
        let mut c = Credentials::new(v.clone(), SavePasswords::Never);
        assert!(matches!(
            c.on_prompt(SecretKind::Password, &t),
            PromptAction::Ask { .. }
        ));
        let mut c = Credentials::new(v, SavePasswords::Always);
        assert!(matches!(
            c.on_prompt(SecretKind::KeyPassphrase, &t),
            PromptAction::Ask { .. }
        ));
        c.on_typed(SecretKind::KeyPassphrase, t, "pp".into());
        assert_eq!(c.after_login().0.len(), 0, "passphrases are never offered");
    }
}
