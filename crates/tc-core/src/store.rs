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
    #[serde(default, rename = "session")]
    pub sessions: Vec<Session>,
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
