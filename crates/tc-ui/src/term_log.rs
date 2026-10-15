//! "Log terminal output": one file per tab (`<folder>/<date>_<time>_<tab>.log`
//! or the "Log filename" pattern). "Log type": plain text (escape
//! sequences removed), everything the session sent, or plain text with a
//! timestamp on each line. Writes are buffered and flushed at most once a
//! second (and on close).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tc_core::settings::LogType;
use tc_term::AnsiStripper;

const FLUSH_EVERY: Duration = Duration::from_secs(1);

pub struct TermLog {
    path: PathBuf,
    file: std::io::BufWriter<std::fs::File>,
    strip: AnsiStripper,
    buf: Vec<u8>,
    last_flush: Instant,
    kind: LogType,
    /// The next byte starts a line (timestamped logs).
    line_start: bool,
}

/// The file name (without extension) from the "Log filename" pattern.
fn file_base(pattern: &str, title: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let pattern = match pattern.trim() {
        "" | "<default>" => "{date}_{time}_{session}",
        p => p,
    };
    let name = pattern
        .replace("{date}", &now.format("%Y%m%d").to_string())
        .replace("{time}", &now.format("%H%M%S").to_string())
        .replace("{session}", title);
    let name = name.strip_suffix(".log").unwrap_or(&name);
    safe_name(name)
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
    pub fn create(
        folder: &Path,
        title: &str,
        kind: LogType,
        pattern: &str,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(folder)?;
        let base = file_base(pattern, title, chrono::Local::now());
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
            kind,
            line_start: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Log terminal output. Errors stop logging (returned once).
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self.kind {
            LogType::All => self.file.write_all(bytes)?,
            LogType::Printable => {
                self.buf.clear();
                self.strip.feed(bytes, &mut self.buf);
                self.file.write_all(&self.buf)?;
            }
            LogType::Timestamped => {
                self.buf.clear();
                self.strip.feed(bytes, &mut self.buf);
                let stamp = chrono::Local::now()
                    .format("[%Y-%m-%d %H:%M:%S] ")
                    .to_string();
                for &b in &self.buf {
                    if self.line_start {
                        self.file.write_all(stamp.as_bytes())?;
                        self.line_start = false;
                    }
                    self.file.write_all(&[b])?;
                    if b == b'\n' {
                        self.line_start = true;
                    }
                }
            }
        }
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
        let mut a = TermLog::create(dir.path(), "root@web:~ (1)", LogType::Printable, "").unwrap();
        let b = TermLog::create(dir.path(), "root@web:~ (1)", LogType::Printable, "").unwrap();
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

    #[test]
    fn log_types_and_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut raw = TermLog::create(dir.path(), "web", LogType::All, "{session}-raw").unwrap();
        assert!(raw.path().ends_with("web-raw.log"));
        raw.write(b"\x1b[32mok\x1b[0m\r\n").unwrap();
        let path = raw.path().to_path_buf();
        drop(raw);
        assert_eq!(std::fs::read(path).unwrap(), b"\x1b[32mok\x1b[0m\r\n");

        let mut t = TermLog::create(dir.path(), "web", LogType::Timestamped, "t.log").unwrap();
        assert!(t.path().ends_with("t.log"));
        t.write(b"one\r\ntw").unwrap();
        t.write(b"o\r\n").unwrap();
        let path = t.path().to_path_buf();
        drop(t);
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with('[') && lines[0].ends_with("] one"));
        assert!(lines[1].ends_with("] two"), "one stamp per line: {text:?}");

        let now = chrono::Local::now();
        assert_eq!(file_base("../{session}", "a/b", now), "a_b");
    }
}
