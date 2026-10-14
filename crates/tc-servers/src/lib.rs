//! Network services of the "Servers" window (reference `TFORMSERVICES`):
//! small file servers for sharing files with devices on the network.
//!
//! - [`http`]: static files (GET / HEAD), optional directory listings.
//! - [`tftp`]: RFC 1350 reads and writes, with the block size option.
//!
//! Every server keeps requests inside its root folder, limits what it
//! accepts, logs to the "Server output" and can stop by itself after a
//! delay ("Stop server after").

pub mod http;
pub mod tftp;

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A running server: its log, and a way to stop it.
pub struct ServerHandle {
    stop: Arc<AtomicBool>,
    log: crossbeam_channel::Receiver<String>,
    pub addr: std::net::SocketAddr,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl ServerHandle {
    /// New "Server output" lines.
    pub fn log(&self) -> impl Iterator<Item = String> + '_ {
        self.log.try_iter()
    }

    pub fn is_running(&self) -> bool {
        !self.stop.load(Ordering::Relaxed) && self.threads.iter().any(|t| !t.is_finished())
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What a server thread shares: the stop flag, the deadline and the log.
#[derive(Clone)]
pub(crate) struct Ctx {
    stop: Arc<AtomicBool>,
    deadline: Option<Instant>,
    log: crossbeam_channel::Sender<String>,
}

impl Ctx {
    pub(crate) fn new(
        stop_after: Option<Duration>,
    ) -> (Self, crossbeam_channel::Receiver<String>, Arc<AtomicBool>) {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = crossbeam_channel::bounded(1024);
        let ctx = Self {
            stop: stop.clone(),
            deadline: stop_after.map(|d| Instant::now() + d),
            log: tx,
        };
        (ctx, rx, stop)
    }

    /// Stop requested, or "Stop server after" reached.
    pub(crate) fn stopping(&self) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return true;
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            self.stop.store(true, Ordering::Relaxed);
            self.say("Server stopped (time limit reached)".into());
            return true;
        }
        false
    }

    /// Log a line (dropped when the window does not read the log).
    pub(crate) fn say(&self, line: String) {
        let _ = self.log.try_send(line);
    }
}

pub(crate) fn handle(
    ctx: Ctx,
    log: crossbeam_channel::Receiver<String>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
    threads: Vec<std::thread::JoinHandle<()>>,
) -> ServerHandle {
    drop(ctx);
    ServerHandle {
        stop,
        log,
        addr,
        threads,
    }
}

/// The file a request names, inside `root`: `None` when the name leaves
/// the root (`..`, absolute paths, drive letters, symlinks pointing out).
/// `must_exist`: reads need an existing file; writes may create one.
pub fn resolve(root: &Path, requested: &str, must_exist: bool) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let mut path = root.clone();
    for part in requested.split(['/', '\\']) {
        match Path::new(part).components().next() {
            None => {}
            Some(Component::Normal(p)) if Path::new(part).components().count() == 1 => {
                path.push(p);
            }
            Some(Component::CurDir) => {}
            // "..", "C:", "/" and anything odd: refused.
            _ => return None,
        }
    }
    if must_exist {
        let real = path.canonicalize().ok()?;
        return real.starts_with(&root).then_some(real);
    }
    // A new file: its folder must exist and be inside the root.
    let parent = path.parent()?.canonicalize().ok()?;
    if !parent.starts_with(&root) {
        return None;
    }
    if let Ok(real) = path.canonicalize()
        && !real.starts_with(&root)
    {
        return None; // an existing symlink pointing out
    }
    Some(parent.join(path.file_name()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_the_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/a.txt"), b"x").unwrap();
        let root = dir.path();
        assert!(resolve(root, "/sub/a.txt", true).is_some());
        assert!(resolve(root, "sub\\a.txt", true).is_some());
        assert!(resolve(root, "./sub/./a.txt", true).is_some());
        assert!(resolve(root, "", true).is_some(), "the root itself");
        assert!(resolve(root, "../x", true).is_none());
        assert!(resolve(root, "sub/../../x", false).is_none());
        assert!(resolve(root, "C:/Windows/win.ini", true).is_none());
        assert!(resolve(root, "sub/missing", true).is_none());
        assert!(resolve(root, "sub/new.bin", false).is_some(), "a new file");
        assert!(resolve(root, "nodir/new.bin", false).is_none());
    }
}
