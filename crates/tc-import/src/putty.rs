//! "Import PuTTY sessions": PuTTY keeps sessions in the Windows registry
//! (`HKCU\Software\SimonTatham\PuTTY\Sessions\<name>`) and, on Unix, in
//! `~/.putty/sessions/<name>` files (`Key=Value` lines). Session names are
//! URL-encoded in both places.

use crate::mobaxterm::{ImportReport, Unsupported};
use std::collections::BTreeMap;
use tc_core::{Session, SessionKind};

const REG_KEY: &str = r"HKCU\Software\SimonTatham\PuTTY\Sessions";

/// `%20` → space (PuTTY's name encoding).
fn decode_name(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = name
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse `reg query HKCU\Software\SimonTatham\PuTTY\Sessions /s` output.
pub fn parse_reg_query(text: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut sessions = Vec::new();
    let mut current: Option<(String, BTreeMap<String, String>)> = None;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if let Some(rest) =
            trimmed.strip_prefix("HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\")
        {
            sessions.extend(current.take());
            current = Some((decode_name(rest), BTreeMap::new()));
            continue;
        }
        let Some((_, values)) = current.as_mut() else {
            continue;
        };
        // "    Name    REG_TYPE    value" (the value may contain spaces).
        let t = trimmed.trim_start();
        let Some(reg) = t.find("    REG_") else {
            continue;
        };
        let name = t[..reg].trim();
        let after = &t[reg + 4..];
        let (kind, value) = after.split_once("    ").unwrap_or((after, ""));
        let value = if kind == "REG_DWORD" {
            value
                .trim()
                .strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .map_or_else(|| value.trim().to_string(), |v| v.to_string())
        } else {
            value.to_string()
        };
        values.insert(name.to_string(), value);
    }
    sessions.extend(current);
    sessions
}

/// A `~/.putty/sessions/<name>` file.
pub fn parse_session_file(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
        .collect()
}

/// Turn PuTTY settings into sessions; unknown protocols are reported.
pub fn to_report(sessions: Vec<(String, BTreeMap<String, String>)>) -> ImportReport {
    let mut report = ImportReport::default();
    for (name, v) in sessions {
        if name == "Default Settings" {
            continue;
        }
        let get = |k: &str| {
            v.get(k)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let port = |default: u16| {
            get("PortNumber")
                .and_then(|p| p.parse().ok())
                .filter(|&p| p > 0)
                .unwrap_or(default)
        };
        let host = get("HostName");
        let protocol = get("Protocol").unwrap_or_else(|| "ssh".into());
        let kind = match (protocol.as_str(), host) {
            ("ssh", Some(host)) => SessionKind::Ssh {
                host,
                port: port(22),
                user: get("UserName"),
                private_key: get("PublicKeyFile"),
                x11_forwarding: get("X11Forward").as_deref() == Some("1"),
            },
            ("telnet", Some(host)) => SessionKind::Telnet {
                host,
                port: port(23),
            },
            ("rlogin", Some(host)) => SessionKind::Rsh {
                host,
                user: get("UserName"),
            },
            ("serial", _) => match get("SerialLine") {
                Some(port_name) => SessionKind::Serial {
                    port_name,
                    baud: get("SerialSpeed")
                        .and_then(|b| b.parse().ok())
                        .filter(|&b| b >= 50)
                        .unwrap_or(9600),
                },
                None => {
                    report
                        .unsupported
                        .push(unsupported(&name, "Serial", "no serial line"));
                    continue;
                }
            },
            (p, None) if p != "serial" => {
                report
                    .unsupported
                    .push(unsupported(&name, p, "no host name"));
                continue;
            }
            (p, _) => {
                report
                    .unsupported
                    .push(unsupported(&name, p, "protocol not supported"));
                continue;
            }
        };
        let mut session = Session::new(name, kind);
        session.folder = "PuTTY sessions".into();
        report.sessions.push(session);
    }
    report
}

fn unsupported(name: &str, session_type: &str, reason: &'static str) -> Unsupported {
    Unsupported {
        name: name.into(),
        folder: String::new(),
        session_type: session_type.into(),
        reason,
    }
}

/// Read this user's PuTTY sessions.
pub fn load() -> Result<ImportReport, String> {
    if cfg!(windows) {
        let mut cmd = std::process::Command::new("reg");
        cmd.args(["query", REG_KEY, "/s"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let out = cmd.output().map_err(|e| format!("reg: {e}"))?;
        if !out.status.success() {
            return Err("no PuTTY sessions found".into());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        return Ok(to_report(parse_reg_query(&text)));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let dir = std::path::Path::new(&home).join(".putty").join("sessions");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let name = decode_name(&entry.file_name().to_string_lossy());
        if let Ok(text) = crate::read_text_lossy(&entry.path()) {
            sessions.push((name, parse_session_file(&text)));
        }
    }
    Ok(to_report(sessions))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REG: &str = r"
HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\Default%20Settings
    HostName    REG_SZ

HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\web%20server
    HostName    REG_SZ    10.0.0.5
    PortNumber    REG_DWORD    0x8ae
    Protocol    REG_SZ    ssh
    UserName    REG_SZ    deploy
    PublicKeyFile    REG_SZ    C:\Users\me\My Keys\web.ppk

HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\switch
    HostName    REG_SZ    192.168.1.2
    Protocol    REG_SZ    telnet
    PortNumber    REG_DWORD    0x17

HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\console
    Protocol    REG_SZ    serial
    SerialLine    REG_SZ    COM4
    SerialSpeed    REG_DWORD    0x1c200

HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\raw%20thing
    HostName    REG_SZ    h
    Protocol    REG_SZ    raw
";

    #[test]
    fn registry_export() {
        let r = to_report(parse_reg_query(REG));
        let names: Vec<_> = r.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["web server", "switch", "console"]);
        assert_eq!(
            r.sessions[0].kind,
            SessionKind::Ssh {
                host: "10.0.0.5".into(),
                port: 2222,
                user: Some("deploy".into()),
                private_key: Some(r"C:\Users\me\My Keys\web.ppk".into()),
                x11_forwarding: false,
            }
        );
        assert_eq!(
            r.sessions[2].kind,
            SessionKind::Serial {
                port_name: "COM4".into(),
                baud: 115_200
            }
        );
        assert_eq!(r.unsupported.len(), 1);
        assert_eq!(r.unsupported[0].name, "raw thing");
    }

    #[test]
    fn unix_session_file() {
        let v = parse_session_file("HostName=box\nPortNumber=2200\nProtocol=ssh\nUserName=me\n");
        let r = to_report(vec![("box".into(), v)]);
        assert!(matches!(
            &r.sessions[0].kind,
            SessionKind::Ssh { port: 2200, .. }
        ));
        assert_eq!(decode_name("a%20b%25c"), "a b%c");
    }
}
