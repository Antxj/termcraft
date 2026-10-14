//! Shared sessions ("Share these sessions with my team"): read-only
//! session lists from a MobaXterm sessions file in a shared folder or on a
//! web server, shown under "Shared sessions" in the tree. They are read in
//! the background and never written to the user's own sessions.

use std::io::Read;
use tc_core::settings::{SharedBackend, SharedSessionsEntry};
use tc_core::{Session, SessionKind};

/// Largest shared sessions file read.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub sessions: Vec<Session>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct SharedSessions {
    pub groups: Vec<Group>,
    loading: Option<crossbeam_channel::Receiver<Vec<Group>>>,
}

fn read(entry: &SharedSessionsEntry) -> Result<String, String> {
    let location = entry.location.trim();
    let bytes = match entry.backend {
        SharedBackend::Folder => {
            let path = std::path::Path::new(location);
            let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
            if len > MAX_BYTES {
                return Err("the shared sessions file is too large".into());
            }
            std::fs::read(path).map_err(|e| e.to_string())?
        }
        SharedBackend::Http => {
            if !(location.starts_with("https://") || location.starts_with("http://")) {
                return Err("the URL must start with http:// or https://".into());
            }
            let mut resp = ureq::get(location).call().map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            resp.body_mut()
                .as_reader()
                .take(MAX_BYTES + 1)
                .read_to_end(&mut out)
                .map_err(|e| e.to_string())?;
            if out.len() as u64 > MAX_BYTES {
                return Err("the shared sessions file is too large".into());
            }
            out
        }
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The sessions of one entry. "File" sessions are left out: opening one
/// from someone else's list could run a program on this computer.
pub fn load(entry: &SharedSessionsEntry) -> Group {
    let name = if entry.name.trim().is_empty() {
        "Shared".to_string()
    } else {
        entry.name.trim().to_string()
    };
    match read(entry) {
        Ok(text) => Group {
            name,
            sessions: tc_import::mobaxterm::parse(&text)
                .sessions
                .into_iter()
                .filter(|s| !matches!(s.kind, SessionKind::File { .. }))
                .collect(),
            error: None,
        },
        Err(e) => Group {
            name,
            sessions: Vec::new(),
            error: Some(e),
        },
    }
}

impl SharedSessions {
    /// Read every entry again (in the background).
    pub fn refresh(
        &mut self,
        entries: &[SharedSessionsEntry],
        repaint: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) {
        if entries.is_empty() {
            self.groups.clear();
            self.loading = None;
            return;
        }
        let entries = entries.to_vec();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(entries.iter().map(load).collect());
            repaint();
        });
        self.loading = Some(rx);
    }

    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.loading
            && let Ok(groups) = rx.try_recv()
        {
            self.groups = groups;
            self.loading = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_a_shared_folder_file_without_file_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("team.mxtsessions");
        std::fs::write(
            &file,
            "[Bookmarks]\nSubRep=\nImgNum=42\nweb=#109#0%web.example%22%ops%%-1%-1%%%%%0%0%0%%%-1%0%0%0%%1080%%0%0%1#MobaFont#0# #-1\n\
             runme=#130#9%C:\\tools\\x.exe#MobaFont#0#\n",
        )
        .unwrap();
        let g = load(&SharedSessionsEntry {
            name: "Team".into(),
            backend: SharedBackend::Folder,
            location: file.display().to_string(),
        });
        assert_eq!(g.error, None);
        assert_eq!(g.name, "Team");
        let names: Vec<&str> = g.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["web"], "the File session is left out");
        let bad = load(&SharedSessionsEntry {
            name: String::new(),
            backend: SharedBackend::Http,
            location: "file:///etc/passwd".into(),
        });
        assert!(bad.error.is_some() && bad.name == "Shared");
    }
}
