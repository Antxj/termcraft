//! Parser for MobaXterm bookmark sections, as found in exported
//! `.mxtsessions` files and in `MobaXterm.ini`.
//!
//! ```text
//! [Bookmarks_1]
//! SubRep=Prod\Web
//! ImgNum=41
//! web01=#109#0%10.0.0.1%22%root%%-1%-1%...#MobaFont%10%...#0# #-1
//! ```
//!
//! Field layout comes from the reference inventory
//! (`docs/inventory/session-types.md`): parameter `A_B` = split the value on
//! `#`, take part A (1-based), split on `%`, take field B (1-based).
//! Part 2 is only the **icon**; field `3_1` is the **session type index**.

use serde::Serialize;
use tc_core::{Session, SessionKind};

/// Session type indices (`[1_N]` sections of the reference parameter table).
const TYPE_NAMES: [&str; 15] = [
    "SSH", "Telnet", "Rsh", "Xdmcp", "RDP", "VNC", "FTP", "SFTP", "Serial", "File", "Shell",
    "Browser", "Mosh", "AwsS3", "WSL",
];

#[derive(Debug, Default)]
pub struct ImportReport {
    pub sessions: Vec<Session>,
    pub unsupported: Vec<Unsupported>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unsupported {
    pub name: String,
    pub folder: String,
    /// Reference type name (e.g. "Mosh") or the raw index if unknown.
    pub session_type: String,
    pub reason: &'static str,
}

pub fn parse(text: &str) -> ImportReport {
    let mut report = ImportReport::default();
    let mut in_bookmarks = false;
    let mut folder = String::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_bookmarks = section == "Bookmarks" || section.starts_with("Bookmarks_");
            folder.clear();
            continue;
        }
        if !in_bookmarks {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "SubRep" => folder = value.trim().replace('\\', "/"),
            "ImgNum" => {}
            name => parse_bookmark(name.trim(), value.trim(), &folder, &mut report),
        }
    }
    report
}

/// A bookmark value addressed by reference parameter ids.
struct Bookmark<'a> {
    parts: Vec<Vec<&'a str>>,
}

impl<'a> Bookmark<'a> {
    fn new(value: &'a str) -> Option<Self> {
        if !value.starts_with('#') {
            return None;
        }
        let parts = value.split('#').map(|p| p.split('%').collect()).collect();
        Some(Self { parts })
    }

    /// Field `A_B` (both 1-based); empty strings become `None`.
    fn get(&self, a: usize, b: usize) -> Option<&'a str> {
        let v = self.parts.get(a - 1)?.get(b - 1)?.trim();
        (!v.is_empty()).then_some(v)
    }

    fn string(&self, a: usize, b: usize) -> Option<String> {
        self.get(a, b).map(String::from)
    }

    fn port(&self, a: usize, b: usize, default: u16) -> u16 {
        self.get(a, b)
            .and_then(|p| p.parse().ok())
            .unwrap_or(default)
    }

    /// Delphi booleans: `-1` = true, `0` = false.
    fn flag(&self, a: usize, b: usize, default: bool) -> bool {
        match self.get(a, b) {
            Some("-1") | Some("1") => true,
            Some("0") => false,
            _ => default,
        }
    }
}

fn parse_bookmark(name: &str, value: &str, folder: &str, report: &mut ImportReport) {
    let unsupported = |session_type: String, reason| Unsupported {
        name: name.into(),
        folder: folder.into(),
        session_type,
        reason,
    };
    let Some(b) = Bookmark::new(value) else {
        report
            .unsupported
            .push(unsupported(String::new(), "unrecognized bookmark value"));
        return;
    };
    let Some(index) = b.get(3, 1).and_then(|t| t.parse::<usize>().ok()) else {
        report
            .unsupported
            .push(unsupported(String::new(), "missing session type"));
        return;
    };
    let type_name = TYPE_NAMES
        .get(index)
        .map_or_else(|| format!("#{index}"), |n| (*n).to_string());

    let host = b.string(3, 2);
    let kind = match (index, host) {
        (0, Some(host)) => SessionKind::Ssh {
            host,
            port: b.port(3, 3, 22),
            user: b.string(3, 4),
            private_key: b.string(3, 15),
            x11_forwarding: b.flag(3, 6, true),
        },
        (1, Some(host)) => SessionKind::Telnet {
            host,
            port: b.port(3, 3, 23),
        },
        (4, Some(host)) => SessionKind::Rdp {
            host,
            port: b.port(3, 3, 3389),
            user: b.string(3, 4),
        },
        (5, Some(host)) => SessionKind::Vnc {
            host,
            port: b.port(3, 3, 5900),
        },
        (6, Some(host)) => SessionKind::Ftp {
            host,
            port: b.port(3, 3, 21),
            user: b.string(3, 4),
        },
        (7, Some(host)) => SessionKind::Sftp {
            host,
            port: b.port(3, 3, 22),
            user: b.string(3, 4),
        },
        (8, _) => match b.string(3, 9) {
            Some(port_name) => SessionKind::Serial {
                port_name,
                baud: b
                    .get(3, 3)
                    .and_then(|s| s.parse().ok())
                    .filter(|&baud| baud >= 50)
                    .unwrap_or(115_200),
            },
            None => {
                report
                    .unsupported
                    .push(unsupported(type_name, "serial bookmark without port"));
                return;
            }
        },
        (10, _) => SessionKind::LocalShell { program: None },
        (2, Some(host)) => SessionKind::Rsh {
            host,
            user: b.string(3, 3),
        },
        (12, Some(host)) => SessionKind::Mosh {
            host,
            user: b.string(3, 3),
            ssh_port: b.port(3, 4, 22),
        },
        (9, Some(path)) => SessionKind::File { path },
        (11, Some(url)) => SessionKind::Browser { url },
        (14, distro) => SessionKind::Wsl {
            distro: distro.unwrap_or_else(|| "Default".into()),
        },
        (0 | 1 | 2 | 4 | 5 | 6 | 7 | 9 | 11 | 12, None) => {
            report
                .unsupported
                .push(unsupported(type_name, "bookmark without host"));
            return;
        }
        _ => {
            report
                .unsupported
                .push(unsupported(type_name, "session type not supported yet"));
            return;
        }
    };

    let mut session = Session::new(name, kind);
    session.folder = folder.into();
    copy_options(&b, index, &mut session);
    report.sessions.push(session);
}

/// Keep every schema parameter present in the bookmark (lossless import);
/// typed fields are skipped and defaults are dropped by `set_option`.
fn copy_options(b: &Bookmark<'_>, type_index: usize, session: &mut Session) {
    use tc_core::schema;
    let typed = session.kind.typed_ids();
    let params = schema::type_params(type_index)
        .iter()
        .chain(schema::network_params(type_index))
        .chain(schema::terminal_params())
        .chain(schema::bookmark_params());
    for param in params {
        if typed.contains(&param.id) {
            continue;
        }
        let Some((part, field)) = param.id.split_once('_') else {
            continue;
        };
        let (Ok(part), Ok(field)) = (part.parse(), field.parse()) else {
            continue;
        };
        if let Some(value) = b.get(part, field) {
            session.set_option(param, value.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[Misc]
LeftDelimChars=
[Bookmarks]
SubRep=
ImgNum=42
root box=#109#0%192.168.1.10%22%root%%-1%-1%%%%%0%0%0%%%-1%0%0%0%%1080%%0%0%1#MobaFont%10%0%0%-1%15#0# #-1
custom icon ssh=#42#0%10.9.9.9%%%%0#MobaFont#0#
[Bookmarks_1]
SubRep=Prod\\Web
ImgNum=41
web01=#109#0%web01.example.com%2222%%%-1%-1%%%%%%%%C:\\keys\\id_ed25519#MobaFont%10#0#
desk=#91#4%10.1.1.5%3389%admin%-1%0#MobaFont#0#
router=#98#1%192.168.0.1%23#MobaFont#0#
console=#131#8%%115200%3%0%0%1%%COM3#MobaFont#0#
mosh box=#145#12%h%u#MobaFont#0#
broken=not a bookmark
";

    fn by_name<'a>(r: &'a ImportReport, n: &str) -> &'a Session {
        r.sessions.iter().find(|s| s.name == n).unwrap()
    }

    #[test]
    fn ssh_fields_and_folders() {
        let r = parse(SAMPLE);
        assert_eq!(
            by_name(&r, "root box").kind,
            SessionKind::Ssh {
                host: "192.168.1.10".into(),
                port: 22,
                user: Some("root".into()),
                private_key: None,
                x11_forwarding: true,
            }
        );
        let web = by_name(&r, "web01");
        assert_eq!(web.folder, "Prod/Web");
        match &web.kind {
            SessionKind::Ssh {
                port,
                user,
                private_key,
                ..
            } => {
                assert_eq!(*port, 2222);
                assert_eq!(*user, None);
                assert_eq!(private_key.as_deref(), Some("C:\\keys\\id_ed25519"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn type_comes_from_field_3_1_not_icon() {
        let r = parse(SAMPLE);
        match &by_name(&r, "custom icon ssh").kind {
            SessionKind::Ssh {
                host,
                x11_forwarding,
                ..
            } => {
                assert_eq!(host, "10.9.9.9");
                assert!(!x11_forwarding);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn other_types() {
        let r = parse(SAMPLE);
        assert_eq!(
            by_name(&r, "desk").kind,
            SessionKind::Rdp {
                host: "10.1.1.5".into(),
                port: 3389,
                user: Some("admin".into())
            }
        );
        assert_eq!(
            by_name(&r, "router").kind,
            SessionKind::Telnet {
                host: "192.168.0.1".into(),
                port: 23
            }
        );
        assert_eq!(
            by_name(&r, "console").kind,
            SessionKind::Serial {
                port_name: "COM3".into(),
                baud: 115_200
            }
        );
    }

    #[test]
    fn reports_unsupported_and_garbage() {
        let r = parse(SAMPLE);
        let got: Vec<_> = r
            .unsupported
            .iter()
            .map(|u| (u.name.as_str(), u.session_type.as_str()))
            .collect();
        assert_eq!(got, [("broken", "")]);
        assert_eq!(
            by_name(&r, "mosh box").kind,
            SessionKind::Mosh {
                host: "h".into(),
                user: Some("u".into()),
                ssh_port: 22
            }
        );
    }

    #[test]
    fn keeps_non_default_options() {
        let r = parse("[Bookmarks]\nbox=#109#0%h%22%u%%-1%-1%%%%%0%0%0%%%-1#MobaFont%12#0#\n");
        let s = &r.sessions[0];
        // 3_7 compression = -1 differs from default 0 → kept; 4_1 font name kept.
        assert_eq!(s.options.get("3_7").map(String::as_str), Some("-1"));
        assert_eq!(s.options.get("4_2").map(String::as_str), Some("12"));
        // typed fields never duplicated into options
        assert!(!s.options.contains_key("3_2"));
        assert!(!s.options.contains_key("3_6"));
    }

    #[test]
    fn ignores_non_bookmark_sections() {
        let r = parse("[Misc]\nfoo=#109#0%h%22%u#\n");
        assert!(r.sessions.is_empty());
    }
}
