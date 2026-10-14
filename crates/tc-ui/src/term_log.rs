//! "Log terminal output": plain-text logs, one file per tab
//! (`<folder>/<date>_<time>_<tab>.log`), escape sequences removed.
//! Writes are buffered and flushed at most once a second (and on close).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tc_term::AnsiStripper;

const FLUSH_EVERY: Duration = Duration::from_secs(1);

pub struct TermLog {
    path: PathBuf,
    file: std::io::BufWriter<std::fs::File>,
    strip: AnsiStripper,
    buf: Vec<u8>,
    last_flush: Instant,
}

/// A file name part from a tab title: letters, digits, `-`, `_`, `.`.
fn safe_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    // One "_" for each run of replaced characters.
    let mut collapsed = String::with_capacity(name.len());
    for c in name.chars() {
        if !(c == '_' && collapsed.ends_with('_')) {
            collapsed.push(c);
        }
    }
    let name = collapsed.trim_matches(['.', '_']);
    if name.is_empty() {
        "terminal".into()
    } else {
        name.into()
    }
}

impl TermLog {
    pub fn create(folder: &Path, title: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(folder)?;
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let base = format!("{stamp}_{}", safe_name(title));
        // Several tabs may open in the same second.
        let mut path = folder.join(format!("{base}.log"));
        let mut n = 2;
        while path.exists() {
            path = folder.join(format!("{base}_{n}.log"));
            n += 1;
        }
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: std::io::BufWriter::new(file),
            strip: AnsiStripper::default(),
            buf: Vec::new(),
            last_flush: Instant::now(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Log terminal output. Errors stop logging (returned once).
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.buf.clear();
        self.strip.feed(bytes, &mut self.buf);
        self.file.write_all(&self.buf)?;
        if self.last_flush.elapsed() >= FLUSH_EVERY {
            self.file.flush()?;
            self.last_flush = Instant::now();
        }
        Ok(())
    }
}

impl TermLog {
    /// "Flush log files frequently": push buffered output to the disk now.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.last_flush = Instant::now();
        self.file.flush()
    }
}

impl Drop for TermLog {
    fn drop(&mut self) {
        let _ = self.file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_plain_text_to_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = TermLog::create(dir.path(), "root@web:~ (1)").unwrap();
        let b = TermLog::create(dir.path(), "root@web:~ (1)").unwrap();
        assert_ne!(a.path(), b.path(), "same second, distinct files");
        assert!(
            a.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("root_web_1.log")
        );
        a.write(b"\x1b[32mok\x1b[0m\r\n").unwrap();
        let path = a.path().to_path_buf();
        drop(a);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "ok\n");
        assert_eq!(safe_name("../../etc"), "etc");
        assert_eq!(safe_name(""), "terminal");
    }
}
