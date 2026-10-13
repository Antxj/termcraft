//! Saved keyboard macros: what was typed into a terminal while recording,
//! replayed into any terminal later.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MacroId(pub uuid::Uuid);

impl MacroId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for MacroId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MacroId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Macro {
    #[serde(default)]
    pub id: MacroId,
    pub name: String,
    /// Exactly what the terminal received (text, `\r`, escape sequences).
    pub keys: String,
}

impl Macro {
    pub fn new(name: impl Into<String>, keys: impl Into<String>) -> Self {
        Self {
            id: MacroId::new(),
            name: name.into(),
            keys: keys.into(),
        }
    }

    /// Human-readable form: control characters shown as `^X` / `<Esc>`.
    pub fn preview(&self, max: usize) -> String {
        let mut out = String::new();
        for ch in self.keys.chars() {
            match ch {
                '\r' | '\n' => out.push('⏎'),
                '\x1b' => out.push_str("<Esc>"),
                '\t' => out.push_str("<Tab>"),
                c if (c as u32) < 0x20 => {
                    out.push('^');
                    out.push(char::from(b'@' + c as u8));
                }
                c => out.push(c),
            }
            if out.chars().count() >= max {
                out.push('…');
                break;
            }
        }
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroStore {
    #[serde(default, rename = "macro")]
    pub macros: Vec<Macro>,
}

impl MacroStore {
    pub fn load(path: &Path) -> Result<Self, crate::StoreError> {
        crate::store::load_toml(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), crate::StoreError> {
        crate::store::save_toml(path, self)
    }

    pub fn get(&self, id: MacroId) -> Option<&Macro> {
        self.macros.iter().find(|m| m.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_control_characters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("macros.toml");
        let store = MacroStore {
            macros: vec![Macro::new("update", "sudo apt update\r\x1b[A\x03")],
        };
        store.save(&path).unwrap();
        assert_eq!(MacroStore::load(&path).unwrap(), store);
        assert_eq!(store.macros[0].preview(80), "sudo apt update⏎<Esc>[A^C");
        assert_eq!(Macro::new("x", "abcdef").preview(3), "abc…");
    }
}
