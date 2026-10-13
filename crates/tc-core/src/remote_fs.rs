//! Remote file browser vocabulary shared by SFTP (`tc-ssh`) and FTP
//! (`tc-net`): directory entries, requests and result events.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: Option<u64>,
    /// Unix mode bits (permissions part).
    pub permissions: Option<u32>,
    /// Owner and group: names when the server sends them, else numeric ids.
    pub owner: Option<String>,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsRequest {
    List(String),
    Download {
        remote: String,
        local: PathBuf,
    },
    Upload {
        local: PathBuf,
        remote: String,
    },
    Mkdir(String),
    /// Create an empty file (fails if it exists).
    CreateFile(String),
    Remove {
        path: String,
        is_dir: bool,
    },
    Rename {
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEvent {
    /// The browser is available; `home` is the initial directory.
    Ready {
        home: String,
    },
    Unavailable(String),
    Listing {
        id: u64,
        path: String,
        entries: Vec<RemoteEntry>,
    },
    Progress {
        id: u64,
        done: u64,
        total: Option<u64>,
    },
    /// A non-listing request finished.
    Done {
        id: u64,
    },
    Failed {
        id: u64,
        error: String,
    },
}

/// Join a remote directory and a name with `/` separators.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Parent of a remote path (`/` stays `/`).
pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => trimmed[..i].into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/home/u", "f.txt"), "/home/u/f.txt");
        assert_eq!(parent("/home/u/"), "/home");
        assert_eq!(parent("/home"), "/");
        assert_eq!(parent("/"), "/");
    }
}
