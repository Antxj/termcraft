//! OpenSSH client configuration (`~/.ssh/config`): one SSH session per
//! concrete `Host` alias (patterns with `*`, `?` or `!` are skipped).
//! `HostName`, `User`, `Port`, `IdentityFile` and a single-hop
//! `ProxyJump` (→ the session's SSH gateway) are used.

use crate::mobaxterm::ImportReport;
use tc_core::{Session, SessionKind};

#[derive(Default)]
struct Block {
    aliases: Vec<String>,
    host_name: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity: Option<String>,
    jump: Option<String>,
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|h| format!("{h}/{rest}"))
            .unwrap_or_else(|_| path.to_string()),
        None => path.to_string(),
    }
}

fn unquote(v: &str) -> &str {
    v.trim().trim_matches('"')
}

pub fn parse(text: &str) -> ImportReport {
    let mut blocks: Vec<Block> = Vec::new();
    let mut in_match = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // "Key Value" or "Key=Value".
        let (key, value) = match line.find(|c: char| c == '=' || c.is_whitespace()) {
            Some(i) => (
                &line[..i],
                line[i + 1..].trim_start_matches(['=', ' ', '\t']),
            ),
            None => (line, ""),
        };
        let key = key.to_ascii_lowercase();
        if key == "host" {
            in_match = false;
            blocks.push(Block {
                aliases: value
                    .split_whitespace()
                    .map(|a| unquote(a).to_string())
                    .collect(),
                ..Block::default()
            });
            continue;
        }
        if key == "match" {
            in_match = true; // conditional blocks are not imported
            continue;
        }
        let Some(b) = blocks.last_mut().filter(|_| !in_match) else {
            continue;
        };
        // First value wins, like in OpenSSH.
        let v = unquote(value).to_string();
        match key.as_str() {
            "hostname" => {
                b.host_name.get_or_insert(v);
            }
            "user" => {
                b.user.get_or_insert(v);
            }
            "port" => {
                if b.port.is_none() {
                    b.port = v.parse().ok().filter(|&p| p > 0);
                }
            }
            "identityfile" => {
                b.identity.get_or_insert(expand_home(&v));
            }
            "proxyjump" => {
                b.jump.get_or_insert(v);
            }
            _ => {}
        }
    }

    let mut report = ImportReport::default();
    for b in blocks {
        for alias in &b.aliases {
            if alias.contains(['*', '?', '!']) {
                continue;
            }
            let host = b.host_name.clone().unwrap_or_else(|| alias.clone());
            let mut session = Session::new(
                alias.clone(),
                SessionKind::Ssh {
                    host,
                    port: b.port.unwrap_or(22),
                    user: b.user.clone(),
                    private_key: b.identity.clone(),
                    x11_forwarding: false,
                },
            );
            session.folder = "OpenSSH config".into();
            // Single hop only: "[user@]host[:port]"; "none" disables it.
            if let Some(jump) = b
                .jump
                .as_deref()
                .filter(|j| !j.contains(',') && *j != "none")
            {
                let (user, rest) = match jump.rsplit_once('@') {
                    Some((u, r)) => (Some(u), r),
                    None => (None, jump),
                };
                let (gw_host, gw_port) = match rest.rsplit_once(':') {
                    Some((h, p)) if p.parse::<u16>().is_ok() => (h, p),
                    _ => (rest, "22"),
                };
                session.options.insert("3_9".into(), gw_host.into());
                session.options.insert("3_10".into(), gw_port.into());
                if let Some(u) = user {
                    session.options.insert("3_11".into(), u.into());
                }
            }
            report.sessions.push(session);
        }
    }
    report
}

/// `~/.ssh/config` of this user.
pub fn default_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(std::path::Path::new(&home).join(".ssh").join("config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_jump() {
        let r = parse(
            "# comment\n\
             Host *\n  ServerAliveInterval 30\n\
             Host web web2\n  HostName 10.0.0.7\n  User deploy\n  Port=2222\n  ProxyJump admin@bastion:2200\n\
             Match host x\n  User nobody\n\
             Host db\n  HostName db.internal\n  IdentityFile \"/keys/db key\"\n  ProxyJump a,b\n",
        );
        let names: Vec<_> = r.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["web", "web2", "db"]);
        let web = &r.sessions[0];
        assert_eq!(
            web.kind,
            SessionKind::Ssh {
                host: "10.0.0.7".into(),
                port: 2222,
                user: Some("deploy".into()),
                private_key: None,
                x11_forwarding: false,
            }
        );
        assert_eq!(web.options.get("3_9").map(String::as_str), Some("bastion"));
        assert_eq!(web.options.get("3_10").map(String::as_str), Some("2200"));
        assert_eq!(web.options.get("3_11").map(String::as_str), Some("admin"));
        let db = &r.sessions[2];
        assert!(
            matches!(&db.kind, SessionKind::Ssh { private_key: Some(k), .. } if k == "/keys/db key")
        );
        assert!(!db.options.contains_key("3_9"), "multi-hop is not imported");
    }
}
