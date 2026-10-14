//! "Automatically backup TermCraft configuration file": at start-up the
//! configuration files are copied to `backups/<unix time>/` next to them,
//! unless they did not change since the last backup. Only the newest
//! [`KEEP`] backups are kept.

use std::path::{Path, PathBuf};

pub const KEEP: usize = 10;

fn backups_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("backups")
}

/// Back up `files` (those that exist). Returns the new backup folder, or
/// `None` when nothing changed since the newest one.
pub fn run(config_dir: &Path, files: &[PathBuf]) -> std::io::Result<Option<PathBuf>> {
    let present: Vec<&PathBuf> = files.iter().filter(|f| f.is_file()).collect();
    if present.is_empty() {
        return Ok(None);
    }
    let root = backups_dir(config_dir);
    let mut existing = list(&root)?;
    if let Some(latest) = existing.last()
        && present.iter().all(|f| {
            let name = f.file_name().unwrap_or_default();
            std::fs::read(f).ok() == std::fs::read(latest.join(name)).ok()
        })
    {
        return Ok(None);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Never reuse a name: after the newest one of this second.
    let n = existing
        .last()
        .and_then(|d| key(d))
        .filter(|(s, _)| *s == secs)
        .map_or(0, |(_, n)| n + 1);
    let dir = if n == 0 {
        root.join(secs.to_string())
    } else {
        root.join(format!("{secs}-{n}"))
    };
    std::fs::create_dir_all(&dir)?;
    for f in &present {
        std::fs::copy(f, dir.join(f.file_name().unwrap_or_default()))?;
    }
    existing.push(dir.clone());
    while existing.len() > KEEP {
        let old = existing.remove(0);
        // Only folders we created (digits and '-').
        if old
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit() || c == '-'))
        {
            std::fs::remove_dir_all(old)?;
        }
    }
    Ok(Some(dir))
}

/// `(seconds, n)` of a "<secs>" / "<secs>-<n>" folder.
fn key(dir: &Path) -> Option<(u64, u64)> {
    let name = dir.file_name()?.to_str()?;
    let (secs, n) = name.split_once('-').unwrap_or((name, "0"));
    Some((secs.parse().ok()?, n.parse().ok()?))
}

/// Backup folders, oldest first.
fn list(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    // "<secs>" or "<secs>-<n>": order by both numbers.
    let mut dirs: Vec<((u64, u64), PathBuf)> = match std::fs::read_dir(root) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| Some((key(&e.path())?, e.path())))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    dirs.sort();
    Ok(dirs.into_iter().map(|(_, p)| p).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_up_changes_only_and_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.toml");
        std::fs::write(&file, "a = 1").unwrap();
        let files = [file.clone(), dir.path().join("missing.toml")];
        let first = run(dir.path(), &files).unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(first.join("settings.toml")).unwrap(),
            "a = 1"
        );
        assert_eq!(
            run(dir.path(), &files).unwrap(),
            None,
            "unchanged: no new backup"
        );
        for i in 0..(KEEP + 3) {
            std::fs::write(&file, format!("a = {i}")).unwrap();
            run(dir.path(), &files).unwrap().unwrap();
        }
        let left = list(&backups_dir(dir.path())).unwrap();
        assert_eq!(left.len(), KEEP);
        let newest = std::fs::read_to_string(left.last().unwrap().join("settings.toml")).unwrap();
        assert_eq!(newest, format!("a = {}", KEEP + 2));
    }
}
