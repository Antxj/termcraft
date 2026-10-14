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
    /// `ascii`: "ASCII mode", convert line endings to the local ones.
    Download {
        remote: String,
        local: PathBuf,
        ascii: bool,
    },
    /// `ascii`: "ASCII mode", send Unix line endings.
    Upload {
        local: PathBuf,
        remote: String,
        ascii: bool,
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
    /// "Permissions": set the mode bits (`0o755`…).
    SetPermissions {
        path: String,
        mode: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEvent {
    /// The browser is available; `home` is the initial directory.
    Ready {
        home: String,
    },
    Unavailable(String),
    /// "Use SUDO for SSH-browser" needs the user's sudo password (SFTP only).
    SudoPasswordNeeded,
    /// Switching to or from sudo failed; the previous browser stays usable.
    SudoFailed(String),
    /// FTPS: the server certificate did not verify; the user may trust
    /// this fingerprint and reconnect.
    UntrustedCertificate {
        fingerprint: String,
        reason: String,
    },
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

/// Line-ending conversion for "ASCII mode" transfers, chunk by chunk
/// (a CR at the end of one chunk and its LF in the next stay together).
#[derive(Debug, Clone, Copy)]
pub struct EolConverter {
    to_crlf: bool,
    /// Upload: a CR held back until we know whether LF follows.
    /// Download: the previous byte was CR.
    cr: bool,
}

impl EolConverter {
    /// CRLF -> LF (to a Unix server).
    pub fn to_lf() -> Self {
        Self {
            to_crlf: false,
            cr: false,
        }
    }

    /// LF -> CRLF.
    pub fn to_crlf() -> Self {
        Self {
            to_crlf: true,
            cr: false,
        }
    }

    /// Remote -> local: CRLF on Windows, unchanged elsewhere.
    pub fn to_local() -> Option<Self> {
        cfg!(windows).then(Self::to_crlf)
    }

    pub fn convert(&mut self, input: &[u8], out: &mut Vec<u8>) {
        out.reserve(input.len() + input.len() / 16);
        for &b in input {
            if self.to_crlf {
                if b == b'\n' && !self.cr {
                    out.push(b'\r');
                }
                out.push(b);
                self.cr = b == b'\r';
            } else {
                if self.cr {
                    self.cr = false;
                    if b != b'\n' {
                        out.push(b'\r');
                    }
                }
                if b == b'\r' {
                    self.cr = true;
                } else {
                    out.push(b);
                }
            }
        }
    }

    /// Flush what is held back at the end of the file.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        if !self.to_crlf && std::mem::take(&mut self.cr) {
            out.push(b'\r');
        }
    }
}

#[cfg(test)]
mod eol_tests {
    use super::EolConverter;

    fn run(mut c: EolConverter, chunks: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in chunks {
            c.convert(chunk, &mut out);
        }
        c.finish(&mut out);
        out
    }

    #[test]
    fn crlf_to_lf_across_chunks() {
        assert_eq!(
            run(EolConverter::to_lf(), &[b"a\r\nb\r", b"\nc\r"]),
            b"a\nb\nc\r"
        );
        assert_eq!(run(EolConverter::to_lf(), &[b"x\ry"]), b"x\ry");
    }

    #[test]
    fn lf_to_crlf_keeps_existing_pairs() {
        assert_eq!(
            run(EolConverter::to_crlf(), &[b"a\nb\r", b"\nc\n"]),
            b"a\r\nb\r\nc\r\n"
        );
    }
}
