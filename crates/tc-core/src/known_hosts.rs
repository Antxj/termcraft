//! Reading and editing the OpenSSH-format `known_hosts` file the SSH
//! client trusts ("Known hosts" in Configuration › SSH).

use base64::Engine as _;
use serde::Serialize;
use sha2::Digest as _;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnownHost {
    /// 1-based line in the file.
    pub line: usize,
    /// Host patterns, or `(hashed)` for HashKnownHosts entries.
    pub hosts: String,
    pub key_type: String,
    /// `SHA256:…` of the key.
    pub fingerprint: String,
    /// `@cert-authority` / `@revoked`, if any.
    pub marker: Option<String>,
}

fn fingerprint(key_b64: &str) -> Option<String> {
    let blob = base64::engine::general_purpose::STANDARD
        .decode(key_b64)
        .ok()?;
    let digest = sha2::Sha256::digest(&blob);
    let b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest);
    Some(format!("SHA256:{b64}"))
}

pub fn parse(text: &str) -> Vec<KnownHost> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let mut first = fields.next().unwrap_or_default();
        let marker = first.starts_with('@').then(|| first.to_string());
        if marker.is_some() {
            first = fields.next().unwrap_or_default();
        }
        let (Some(key_type), Some(key)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some(fingerprint) = fingerprint(key) else {
            continue;
        };
        let hosts = if first.starts_with("|1|") {
            "(hashed)".to_string()
        } else {
            first.to_string()
        };
        out.push(KnownHost {
            line: i + 1,
            hosts,
            key_type: key_type.to_string(),
            fingerprint,
            marker,
        });
    }
    out
}

pub fn load(path: &Path) -> std::io::Result<Vec<KnownHost>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(parse(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Remove line `line` if it still holds the key with `fingerprint` (so a
/// file changed in the meantime never loses the wrong entry).
pub fn remove(path: &Path, line: usize, fingerprint: &str) -> std::io::Result<bool> {
    let text = std::fs::read_to_string(path)?;
    let matches = parse(&text)
        .iter()
        .any(|h| h.line == line && h.fingerprint == fingerprint);
    if !matches {
        return Ok(false);
    }
    let mut kept: Vec<&str> = text
        .lines()
        .enumerate()
        .filter(|(i, _)| i + 1 != line)
        .map(|(_, l)| l)
        .collect();
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Append OpenSSH `known_hosts` lines that are not there yet (same hosts
/// and key). Returns how many were added.
pub fn add(path: &Path, lines: &[String]) -> std::io::Result<usize> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let key_of = |l: &str| {
        let mut f = l.split_whitespace();
        Some((
            f.next()?.to_string(),
            f.next()?.to_string(),
            f.next()?.to_string(),
        ))
    };
    let mut have: std::collections::HashSet<_> = text.lines().filter_map(key_of).collect();
    let mut out = text.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    let mut added = 0;
    for line in lines {
        let line = line.trim();
        // One entry per line, nothing that could add another one.
        if line.contains(['\n', '\r']) || parse(line).len() != 1 {
            continue;
        }
        if let Some(k) = key_of(line)
            && have.insert(k)
        {
            out.push_str(line);
            out.push('\n');
            added += 1;
        }
    }
    if added > 0 {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, out)?;
        std::fs::rename(&tmp, path)?;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real ed25519 public key blob (from a fixed seed).
    const KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIGb7Lw2OV5r3bOv0uTYkQ8fN0RZs8a/Fc8fd9NBbUq3m";

    #[test]
    fn parse_and_remove() {
        let text = format!(
            "# comment\n\n[h.test]:2222 ssh-ed25519 {KEY}\n|1|abc=|def= ssh-ed25519 {KEY}\n\
             @cert-authority *.corp ssh-ed25519 {KEY}\nbroken line\n"
        );
        let hosts = parse(&text);
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].line, 3);
        assert_eq!(hosts[0].hosts, "[h.test]:2222");
        assert!(hosts[0].fingerprint.starts_with("SHA256:"));
        assert_eq!(hosts[1].hosts, "(hashed)");
        assert_eq!(hosts[2].marker.as_deref(), Some("@cert-authority"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, &text).unwrap();
        assert!(
            !remove(&path, 3, "SHA256:other").unwrap(),
            "wrong key: kept"
        );
        assert!(remove(&path, 3, &hosts[0].fingerprint).unwrap());
        let left = load(&path).unwrap();
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|h| h.hosts != "[h.test]:2222"));
    }

    #[test]
    fn add_skips_duplicates_and_junk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/known_hosts");
        let line = format!("srv.test ssh-ed25519 {KEY}");
        let lines = [
            line.clone(),
            line.clone(),
            "not a key line".to_string(),
            format!("a.test ssh-ed25519 {KEY}\nb.test ssh-ed25519 {KEY}"),
        ];
        assert_eq!(add(&path, &lines).unwrap(), 1);
        assert_eq!(add(&path, &lines).unwrap(), 0, "already there");
        assert_eq!(load(&path).unwrap().len(), 1);
    }
}
