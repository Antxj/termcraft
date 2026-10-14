//! FTP file browser backend (plain FTP; FTPS is tracked in docs/PARITY.md).
//! One worker thread owns the control connection and processes requests in
//! order, reporting with the same [`FsEvent`]s as the SFTP browser.

use crate::Notify;
use crossbeam_channel::{Receiver, Sender};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;
use suppaftp::list::{File, PosixPexQuery};
use suppaftp::types::FileType;
use suppaftp::{FtpStream, Mode};
use tc_core::remote_fs::{FsEvent, FsRequest, RemoteEntry};

#[derive(Debug, Clone)]
pub struct FtpParams {
    pub host: String,
    pub port: u16,
    /// Empty = anonymous.
    pub user: String,
    pub password: String,
    pub passive: bool,
    /// ASCII transfer type instead of binary.
    pub ascii: bool,
    pub start_dir: Option<String>,
}

pub struct FtpSession {
    requests: Sender<(u64, FsRequest)>,
    events: Receiver<FsEvent>,
    next: AtomicU64,
}

impl FtpSession {
    pub fn connect(params: FtpParams, notify: Notify) -> Self {
        let (req_tx, req_rx) = crossbeam_channel::unbounded::<(u64, FsRequest)>();
        let (ev_tx, events) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name(format!("ftp {}", params.host))
            .spawn(move || worker(params, req_rx, ev_tx, notify))
            .expect("spawn ftp thread");
        Self {
            requests: req_tx,
            events,
            next: AtomicU64::new(1),
        }
    }

    pub fn request(&self, request: FsRequest) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let _ = self.requests.send((id, request));
        id
    }

    pub fn events(&self) -> &Receiver<FsEvent> {
        &self.events
    }
}

/// TYPE A for "ASCII mode" transfers, TYPE I otherwise.
fn file_type(ascii: bool) -> FileType {
    if ascii {
        FileType::Ascii(suppaftp::types::FormatControl::Default)
    } else {
        FileType::Binary
    }
}

fn login(p: &FtpParams) -> Result<(FtpStream, String), String> {
    let mut ftp = FtpStream::connect((p.host.as_str(), p.port))
        .map_err(|e| format!("cannot connect to {}:{}: {e}", p.host, p.port))?;
    ftp.set_mode(if p.passive {
        Mode::Passive
    } else {
        Mode::Active
    });
    let (user, pass) = if p.user.trim().is_empty() {
        ("anonymous", "anonymous@")
    } else {
        (p.user.as_str(), p.password.as_str())
    };
    ftp.login(user, pass)
        .map_err(|e| format!("login failed: {e}"))?;
    ftp.transfer_type(file_type(p.ascii))
        .map_err(|e| e.to_string())?;
    if let Some(dir) = p.start_dir.as_deref().filter(|d| !d.trim().is_empty()) {
        ftp.cwd(dir.trim())
            .map_err(|e| format!("cannot open {dir}: {e}"))?;
    }
    let home = ftp.pwd().unwrap_or_else(|_| "/".into());
    Ok((ftp, home))
}

fn worker(
    params: FtpParams,
    requests: Receiver<(u64, FsRequest)>,
    events: Sender<FsEvent>,
    notify: Notify,
) {
    let send = |e: FsEvent| {
        let _ = events.send(e);
        notify();
    };
    let mut ftp = match login(&params) {
        Ok((ftp, home)) => {
            send(FsEvent::Ready { home });
            ftp
        }
        Err(e) => return send(FsEvent::Unavailable(e)),
    };
    for (id, request) in requests.iter() {
        let progress = |done: u64, total: Option<u64>| send(FsEvent::Progress { id, done, total });
        match execute(&mut ftp, id, request, progress) {
            Ok(event) => send(event),
            Err(error) => send(FsEvent::Failed { id, error }),
        }
    }
    let _ = ftp.quit();
}

const CHUNK: usize = 64 * 1024;

fn execute(
    ftp: &mut FtpStream,
    id: u64,
    request: FsRequest,
    progress: impl Fn(u64, Option<u64>),
) -> Result<FsEvent, String> {
    let err = |e: suppaftp::FtpError| e.to_string();
    match request {
        FsRequest::List(path) => {
            let lines = ftp.list(Some(&path)).map_err(err)?;
            let mut entries: Vec<RemoteEntry> = lines
                .iter()
                .filter_map(|l| l.parse::<File>().ok())
                .filter(|f| f.name() != "." && f.name() != "..")
                .map(|f| entry(&f))
                .collect();
            entries.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            Ok(FsEvent::Listing { id, path, entries })
        }
        FsRequest::Download {
            remote,
            local,
            ascii,
        } => {
            ftp.transfer_type(file_type(ascii)).map_err(err)?;
            let total = ftp.size(&remote).ok().map(|s| s as u64);
            let mut out =
                std::fs::File::create(&local).map_err(|e| format!("{}: {e}", local.display()))?;
            ftp.retr(&remote, |reader| {
                let mut buf = vec![0u8; CHUNK];
                let mut done = 0u64;
                loop {
                    let n = reader
                        .read(&mut buf)
                        .map_err(suppaftp::FtpError::ConnectionError)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n])
                        .map_err(suppaftp::FtpError::ConnectionError)?;
                    done += n as u64;
                    progress(done, total);
                }
                Ok(())
            })
            .map_err(err)?;
            Ok(FsEvent::Done { id })
        }
        FsRequest::Upload {
            local,
            remote,
            ascii,
        } => {
            ftp.transfer_type(file_type(ascii)).map_err(err)?;
            let file =
                std::fs::File::open(&local).map_err(|e| format!("{}: {e}", local.display()))?;
            let total = file.metadata().ok().map(|m| m.len());
            let mut reader = Counting {
                inner: file,
                done: 0,
                total,
                progress: &progress,
            };
            ftp.put_file(&remote, &mut reader).map_err(err)?;
            Ok(FsEvent::Done { id })
        }
        FsRequest::Mkdir(path) => {
            ftp.mkdir(&path).map_err(err)?;
            Ok(FsEvent::Done { id })
        }
        FsRequest::CreateFile(path) => {
            ftp.put_file(&path, &mut std::io::empty()).map_err(err)?;
            Ok(FsEvent::Done { id })
        }
        FsRequest::Remove { path, is_dir } => {
            if is_dir {
                ftp.rmdir(&path)
            } else {
                ftp.rm(&path)
            }
            .map_err(err)?;
            Ok(FsEvent::Done { id })
        }
        FsRequest::Rename { from, to } => {
            ftp.rename(&from, &to).map_err(err)?;
            Ok(FsEvent::Done { id })
        }
        FsRequest::SetPermissions { path, mode } => {
            if path.contains(['\r', '\n']) {
                return Err("invalid path".into());
            }
            ftp.site(format!("CHMOD {:o} {path}", mode & 0o7777))
                .map_err(err)?;
            Ok(FsEvent::Done { id })
        }
    }
}

/// Reader that reports upload progress.
struct Counting<'a, R, P: Fn(u64, Option<u64>)> {
    inner: R,
    done: u64,
    total: Option<u64>,
    progress: &'a P,
}

impl<R: Read, P: Fn(u64, Option<u64>)> Read for Counting<'_, R, P> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        (self.progress)(self.done, self.total);
        Ok(n)
    }
}

fn entry(f: &File) -> RemoteEntry {
    let mut perms = 0u32;
    for (shift, who) in [
        (6, PosixPexQuery::Owner),
        (3, PosixPexQuery::Group),
        (0, PosixPexQuery::Others),
    ] {
        let bits = u32::from(f.can_read(who)) << 2
            | u32::from(f.can_write(who)) << 1
            | u32::from(f.can_execute(who));
        perms |= bits << shift;
    }
    RemoteEntry {
        name: f.name().to_string(),
        is_dir: f.is_directory(),
        size: f.size() as u64,
        modified: f
            .modified()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
            .filter(|&secs| secs > 0),
        permissions: Some(perms),
        owner: f.uid().map(|u| u.to_string()),
        group: f.gid().map(|g| g.to_string()),
    }
}
