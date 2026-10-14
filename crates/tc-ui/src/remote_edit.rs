//! Editing remote files: the SSH-browser downloads the file to a private
//! temporary folder, opens it, and when it changes on disk asks whether to
//! upload it back (reference "Open with MobaTextEditor" / "Open with
//! default program"). The folder is removed with the tab.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenWith {
    /// A text editor, never the file's default program (so a remote
    /// script or executable is not run).
    TextEditor,
    /// The default program for the file type (explicit user choice).
    DefaultProgram,
}

struct Edit {
    remote: String,
    local: PathBuf,
    how: OpenWith,
    /// SFTP request downloading it, until done.
    download: Option<u64>,
    /// Modification time when last opened / uploaded.
    seen: Option<SystemTime>,
}

/// A local copy that changed and may be uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub remote: String,
    pub local: PathBuf,
}

pub struct RemoteEdits {
    dir: PathBuf,
    edits: Vec<Edit>,
    last_check: Instant,
}

const CHECK_EVERY: Duration = Duration::from_secs(1);

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A file name without path parts or characters Windows refuses.
fn local_name(remote: &str) -> String {
    let name = remote.rsplit('/').next().unwrap_or("file");
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim_matches(['.', ' ']).to_string();
    if clean.is_empty() {
        "file".into()
    } else {
        clean
    }
}

impl RemoteEdits {
    pub fn new(tab_id: u64) -> Self {
        let dir = std::env::temp_dir()
            .join("termcraft-edit")
            .join(format!("{}-{tab_id}", std::process::id()));
        Self {
            dir,
            edits: Vec::new(),
            last_check: Instant::now(),
        }
    }

    /// Where to download `remote`; remember it until the download ends.
    pub fn start(&mut self, remote: &str, how: OpenWith) -> std::io::Result<PathBuf> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // One sub-folder per edit keeps same-named files apart.
        let sub = self.dir.join(self.edits.len().to_string());
        builder.create(&sub)?;
        let local = sub.join(local_name(remote));
        self.edits.push(Edit {
            remote: remote.to_string(),
            local: local.clone(),
            how,
            download: None,
            seen: None,
        });
        Ok(local)
    }

    /// The SFTP request id of the download started for `local`.
    pub fn set_download(&mut self, local: &Path, id: u64) {
        if let Some(e) = self.edits.iter_mut().find(|e| e.local == local) {
            e.download = Some(id);
        }
    }

    /// A download finished: the file to open, if it was one of ours.
    pub fn downloaded(&mut self, id: u64) -> Option<(PathBuf, OpenWith)> {
        let e = self.edits.iter_mut().find(|e| e.download == Some(id))?;
        e.download = None;
        e.seen = mtime(&e.local);
        Some((e.local.clone(), e.how))
    }

    /// A download failed: forget it.
    pub fn failed(&mut self, id: u64) {
        self.edits.retain(|e| e.download != Some(id));
    }

    /// Local copies changed since last time (checked once a second).
    pub fn changed(&mut self) -> Vec<Changed> {
        if self.edits.is_empty() || self.last_check.elapsed() < CHECK_EVERY {
            return Vec::new();
        }
        self.last_check = Instant::now();
        let mut out = Vec::new();
        for e in self.edits.iter_mut().filter(|e| e.download.is_none()) {
            let now = mtime(&e.local);
            if now.is_some() && now != e.seen {
                e.seen = now;
                out.push(Changed {
                    remote: e.remote.clone(),
                    local: e.local.clone(),
                });
            }
        }
        out
    }

    pub fn is_watching(&self) -> bool {
        !self.edits.is_empty()
    }
}

impl Drop for RemoteEdits {
    fn drop(&mut self) {
        if self.dir.starts_with(std::env::temp_dir()) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe() {
        assert_eq!(local_name("/etc/nginx/nginx.conf"), "nginx.conf");
        assert_eq!(local_name("/x/a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(local_name("/x/.."), "file");
        assert_eq!(local_name("/x/"), "file");
    }

    #[test]
    fn tracks_downloads_and_changes() {
        let mut edits = RemoteEdits::new(u64::MAX - 1);
        let local = edits.start("/srv/app.conf", OpenWith::TextEditor).unwrap();
        edits.set_download(&local, 7);
        assert_eq!(edits.downloaded(8), None);
        std::fs::write(&local, "a").unwrap();
        assert_eq!(
            edits.downloaded(7),
            Some((local.clone(), OpenWith::TextEditor))
        );
        edits.last_check = Instant::now() - CHECK_EVERY;
        assert!(edits.changed().is_empty(), "not modified yet");

        // Modify (bump the time explicitly: some file systems are coarse).
        std::fs::write(&local, "b").unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&local)
            .unwrap()
            .set_modified(later)
            .unwrap();
        edits.last_check = Instant::now() - CHECK_EVERY;
        let changed = edits.changed();
        assert_eq!(
            changed,
            [Changed {
                remote: "/srv/app.conf".into(),
                local: local.clone()
            }]
        );
        let dir = edits.dir.clone();
        drop(edits);
        assert!(!dir.exists(), "temporary copies are removed");
    }
}
