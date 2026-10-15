//! "Import sessions from a CSV file": one session per row. The first row
//! names the columns (any order, case ignored): `name`, `type` (SSH,
//! Telnet, RDP … as in the session settings), `host`, `port`, `username`
//! (or `user`) and `folder`. Only `type` and `host` are required; a row
//! without a name is named after its host. Fields may be quoted ("a, b"),
//! with "" for a quote; `,` or `;` separates them.

use crate::mobaxterm::{ImportReport, Unsupported};
use tc_core::schema::{self, Param};
use tc_core::{Session, SessionKind};

/// Split one line into fields.
fn fields(line: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => out.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    out.push(field);
    out.iter().map(|f| f.trim().to_string()).collect()
}

/// The basic parameter of a type whose label is one of `labels`.
fn basic_param(type_index: usize, labels: &[&str]) -> Option<&'static Param> {
    schema::type_params(type_index)
        .iter()
        .find(|p| labels.iter().any(|l| p.label.eq_ignore_ascii_case(l)))
}

fn set(session: &mut Session, param: Option<&'static Param>, value: &str) -> Result<(), String> {
    let (Some(p), false) = (param, value.trim().is_empty()) else {
        return Ok(());
    };
    if session.kind.typed_ids().contains(&p.id) {
        session.kind.typed_set(p.id, value)
    } else {
        session
            .options
            .insert(p.id.to_string(), value.trim().to_string());
        Ok(())
    }
}

pub fn parse(text: &str) -> Result<ImportReport, String> {
    let mut lines = text
        .lines()
        .map(|l| l.trim_start_matches('\u{feff}'))
        .filter(|l| !l.trim().is_empty());
    let header = lines.next().ok_or("the file is empty")?;
    let sep = if header.contains(';') && !header.contains(',') {
        ';'
    } else {
        ','
    };
    let columns: Vec<String> = fields(header, sep)
        .into_iter()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let column = |names: &[&str]| columns.iter().position(|c| names.contains(&c.as_str()));
    let (Some(type_col), Some(host_col)) = (column(&["type"]), column(&["host", "hostname"]))
    else {
        return Err("the first row must name the columns, with at least type and host".into());
    };
    let name_col = column(&["name", "session"]);
    let port_col = column(&["port"]);
    let user_col = column(&["username", "user", "login"]);
    let folder_col = column(&["folder"]);

    let mut report = ImportReport::default();
    for line in lines {
        let row = fields(line, sep);
        let get = |col: Option<usize>| col.and_then(|c| row.get(c)).map_or("", String::as_str);
        let type_name = get(Some(type_col));
        let host = get(Some(host_col));
        let name = Some(get(name_col))
            .filter(|n| !n.is_empty())
            .unwrap_or(host);
        let folder = get(folder_col).replace('\\', "/");
        let skip = |reason| Unsupported {
            name: name.to_string(),
            folder: folder.clone(),
            session_type: type_name.to_string(),
            reason,
        };
        let Some(index) = schema::SESSION_TYPES
            .iter()
            .position(|t| t.eq_ignore_ascii_case(type_name))
        else {
            report.unsupported.push(skip("unknown session type"));
            continue;
        };
        let Some(kind) = SessionKind::default_for(index) else {
            report.unsupported.push(skip("unknown session type"));
            continue;
        };
        if host.is_empty() {
            report.unsupported.push(skip("no host"));
            continue;
        }
        let mut session = Session::new(name, kind);
        session.folder = folder.trim_matches('/').to_string();
        let host_param = basic_param(
            index,
            &[
                "Remote host",
                "Serial port",
                "File/folder to open",
                "URL",
                "Distribution",
                "Key ID",
            ],
        );
        let result = set(&mut session, host_param, host)
            .and_then(|()| {
                set(
                    &mut session,
                    basic_param(index, &["Port", "SSH port", "Speed"]),
                    get(port_col),
                )
            })
            .and_then(|()| {
                set(
                    &mut session,
                    basic_param(index, &["Username"]),
                    get(user_col),
                )
            });
        match result {
            Ok(()) => report.sessions.push(session),
            Err(_) => report.unsupported.push(skip("invalid port or value")),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_become_sessions() {
        let text = "\u{feff}Name,Type,Host,Port,Username,Folder\n\
                    web,SSH,web.lan,2222,root,prod/web\n\
                    \"db, primary\",ssh,db.lan,,,\n\
                    desk,RDP,10.0.0.5,3390,me,\n\
                    console,Serial,COM3,9600,,\n\
                    odd,Gopher,x,,,\n\
                    ,Telnet,router,,,\n";
        let r = parse(text).unwrap();
        assert_eq!(r.sessions.len(), 5);
        assert_eq!(r.unsupported.len(), 1);
        let web = &r.sessions[0];
        assert_eq!(web.folder, "prod/web");
        assert!(matches!(
            &web.kind,
            SessionKind::Ssh { host, port: 2222, user: Some(u), .. } if host == "web.lan" && u == "root"
        ));
        assert_eq!(r.sessions[1].name, "db, primary");
        assert!(matches!(
            &r.sessions[2].kind,
            SessionKind::Rdp { port: 3390, .. }
        ));
        assert!(matches!(
            &r.sessions[3].kind,
            SessionKind::Serial { port_name, baud: 9600 } if port_name == "COM3"
        ));
        assert_eq!(r.sessions[4].name, "router", "named after the host");
    }

    #[test]
    fn semicolons_and_errors() {
        let r = parse("type;host\nSSH;a.lan\n").unwrap();
        assert_eq!(r.sessions.len(), 1);
        assert!(
            parse("name,port\nx,22\n").is_err(),
            "no type / host columns"
        );
        assert!(parse("").is_err());
        let bad = parse("type,host,port\nSSH,a,notaport\n").unwrap();
        assert_eq!(bad.unsupported.len(), 1);
    }
}
