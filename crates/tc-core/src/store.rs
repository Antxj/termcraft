use crate::{Session, SessionId, Settings};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid TOML in {path}: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },
    #[error("failed to serialize: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStore {
    /// Folders that exist even without sessions ("New folder").
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub folders: std::collections::BTreeSet<String>,
    #[serde(default, rename = "session")]
    pub sessions: Vec<Session>,
}

/// `a/b` from user input: trimmed parts, no empty parts.
pub fn normalize_folder(path: &str) -> String {
    path.split(['/', '\\'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn in_folder(path: &str, folder: &str) -> bool {
    path == folder || path.starts_with(&format!("{folder}/"))
}

impl SessionStore {
    /// Load from `path`; a missing file yields an empty store.
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        load_toml(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        save_toml(path, self)
    }

    pub fn get(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.name == name)
    }

    pub fn add(&mut self, session: Session) -> SessionId {
        let id = session.id;
        self.sessions.push(session);
        id
    }

    pub fn remove(&mut self, id: SessionId) -> Option<Session> {
        let idx = self.sessions.iter().position(|s| s.id == id)?;
        Some(self.sessions.remove(idx))
    }

    /// Every folder: the ones kept on their own and the ones sessions use,
    /// with their parents.
    pub fn all_folders(&self) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for f in self
            .folders
            .iter()
            .chain(self.sessions.iter().map(|s| &s.folder))
        {
            let mut acc = String::new();
            for part in f.split('/').filter(|p| !p.is_empty()) {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(part);
                out.insert(acc.clone());
            }
        }
        out
    }

    /// "New folder".
    pub fn add_folder(&mut self, path: &str) -> Option<String> {
        let path = normalize_folder(path);
        (!path.is_empty()).then(|| {
            self.folders.insert(path.clone());
            path
        })
    }

    /// Rename a folder: sub-folders and sessions inside move with it.
    pub fn rename_folder(&mut self, from: &str, to: &str) -> bool {
        let (from, to) = (normalize_folder(from), normalize_folder(to));
        if from.is_empty() || to.is_empty() || in_folder(&to, &from) && to != from {
            return false;
        }
        let moved = |p: &str| format!("{to}{}", &p[from.len()..]);
        let folders: Vec<String> = self.folders.iter().cloned().collect();
        self.folders = folders
            .into_iter()
            .map(|f| if in_folder(&f, &from) { moved(&f) } else { f })
            .collect();
        self.folders.insert(to.clone());
        for s in &mut self.sessions {
            if in_folder(&s.folder, &from) {
                s.folder = moved(&s.folder);
            }
        }
        true
    }

    /// Delete a folder and everything in it; returns the sessions removed.
    pub fn remove_folder(&mut self, path: &str) -> usize {
        let path = normalize_folder(path);
        if path.is_empty() {
            return 0;
        }
        self.folders.retain(|f| !in_folder(f, &path));
        let before = self.sessions.len();
        self.sessions.retain(|s| !in_folder(&s.folder, &path));
        before - self.sessions.len()
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        load_toml(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        save_toml(path, self)
    }
}

pub(crate) fn load_toml<T: Default + for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<T, StoreError> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|source| StoreError::Parse {
            path: path.display().to_string(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(source) => Err(StoreError::Io {
            path: path.display().to_string(),
            source,
        }),
    }
}

/// Write atomically: serialize to a sibling temp file, then rename over.
pub(crate) fn save_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let io_err = |source| StoreError::Io {
        path: path.display().to_string(),
        source,
    };
    let text = toml::to_string_pretty(value)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(io_err)?;
    std::fs::rename(&tmp, path).map_err(io_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionKind;

    #[test]
    fn roundtrip_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.toml");

        let mut store = SessionStore::default();
        let mut web = Session::new(
            "web-01",
            SessionKind::Ssh {
                host: "10.0.0.5".into(),
                port: 2222,
                user: Some("deploy".into()),
                private_key: None,
                x11_forwarding: true,
            },
        );
        web.folder = "prod/web".into();
        store.add(web);
        store.add(Session::new(
            "local",
            SessionKind::LocalShell { program: None },
        ));
        store.save(&path).unwrap();

        let loaded = SessionStore::load(&path).unwrap();
        assert_eq!(loaded, store);
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::load(&dir.path().join("nope.toml")).unwrap();
        assert!(store.sessions.is_empty());
    }

    #[test]
    fn ssh_port_defaults_to_22() {
        let store: SessionStore = toml::from_str(
            r#"
            [[session]]
            name = "box"
            type = "ssh"
            host = "example.org"
            "#,
        )
        .unwrap();
        match &store.sessions[0].kind {
            SessionKind::Ssh { port, .. } => assert_eq!(*port, 22),
            other => panic!("unexpected kind {other:?}"),
        }
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;
    use crate::SessionKind;

    fn session(name: &str, folder: &str) -> Session {
        let mut s = Session::new(name, SessionKind::LocalShell { program: None });
        s.folder = folder.into();
        s
    }

    #[test]
    fn folders_add_rename_remove() {
        let mut store = SessionStore::default();
        store.add(session("a", "Prod/Web"));
        store.add(session("b", "Prod"));
        store.add(session("c", "Lab"));
        assert_eq!(store.add_folder(" Empty / Sub "), Some("Empty/Sub".into()));
        assert_eq!(store.add_folder("  /  "), None);
        let all: Vec<_> = store.all_folders().into_iter().collect();
        assert_eq!(all, ["Empty", "Empty/Sub", "Lab", "Prod", "Prod/Web"]);

        assert!(
            !store.rename_folder("Prod", "Prod/Inside"),
            "not into itself"
        );
        assert!(store.rename_folder("Prod", "Clients/ACME"));
        assert_eq!(store.find_by_name("a").unwrap().folder, "Clients/ACME/Web");
        assert_eq!(store.find_by_name("b").unwrap().folder, "Clients/ACME");
        assert_eq!(store.find_by_name("c").unwrap().folder, "Lab");

        assert_eq!(store.remove_folder("Clients"), 2);
        assert_eq!(store.sessions.len(), 1);
        assert!(!store.all_folders().contains("Clients/ACME"));
        // "Prodx" is not inside "Prod".
        store.add(session("d", "Labx"));
        assert_eq!(store.remove_folder("Lab"), 1);
        assert!(store.find_by_name("d").is_some());
    }
}
