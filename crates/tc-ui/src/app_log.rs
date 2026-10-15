//! The TermCraft log file (Help › "View TermCraft log file"): status bar
//! messages, start-up and crashes, one timestamped line each. The file is
//! kept under 1 MB: a bigger one is moved to `termcraft.log.old` at start.
//! Messages are the ones the status bar shows; no passwords go there.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_SIZE: u64 = 1024 * 1024;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Start logging to `path` (rotating a file that grew too big).
pub fn init(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_SIZE) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = FILE.lock() {
        *f = Some(path.to_path_buf());
    }
}

/// The line written for `message` at `now`.
fn line(now: chrono::DateTime<chrono::Local>, message: &str) -> String {
    let one_line: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(2000)
        .collect();
    format!("{} {}\n", now.format("%Y-%m-%d %H:%M:%S"), one_line.trim())
}

/// Append one line (nothing before [`init`]; errors are ignored).
pub fn write(message: &str) {
    if message.trim().is_empty() {
        return;
    }
    let Some(path) = FILE.lock().ok().and_then(|f| f.clone()) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line(chrono::Local::now(), message).as_bytes());
    }
}

/// The current log file.
pub fn path() -> Option<PathBuf> {
    FILE.lock().ok().and_then(|f| f.clone())
}

/// Crashes go to the log too (then to the default report).
pub fn log_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(&format!("crash: {info}"));
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_per_message() {
        let now = chrono::Local::now();
        let l = line(now, "first\nsecond\r\n");
        assert!(l.ends_with("first second\n"));
        assert_eq!(l.matches('\n').count(), 1);
    }
}
