//! Links that open TermCraft (`termcraft://` URL handler, and `ssh://`
//! links passed on the command line). A link can only open a saved
//! session or connect to a host: nothing in it is run.

/// What a link asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlTarget {
    /// `termcraft://session/<name>`: a saved session, by name.
    Session(String),
    /// `ssh://[user@]host[:port]` or `termcraft://ssh/[user@]host[:port]`:
    /// a quick connection (`[user@]host[:port]`).
    Connect(String),
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A user / host / port that is safe to pass on: letters, digits and the
/// usual punctuation, no leading `-`.
fn connect_target(s: &str) -> Option<String> {
    let s = s.trim_end_matches('/');
    let (user, rest) = match s.rsplit_once('@') {
        Some((u, r)) => (Some(u), r),
        None => (None, s),
    };
    if let Some(u) = user
        && (u.is_empty()
            || u.starts_with('-')
            || !u
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)))
    {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') || h.ends_with(']') => (h, Some(p)),
        _ => (rest, None),
    };
    let host_ok = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_[]:".contains(c));
    if !host_ok {
        return None;
    }
    if let Some(p) = port
        && p.parse::<u16>().ok().filter(|&p| p > 0).is_none()
    {
        return None;
    }
    Some(s.to_string())
}

/// Parse a link; `None` for anything else.
pub fn parse(url: &str) -> Option<UrlTarget> {
    let url = url.trim();
    if url.len() > 2048 || url.chars().any(char::is_control) {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("ssh://") {
        return connect_target(&percent_decode(&url[6..])?).map(UrlTarget::Connect);
    }
    let rest = lower
        .starts_with("termcraft://")
        .then(|| &url["termcraft://".len()..])?;
    let (what, arg) = rest.split_once('/')?;
    let arg = percent_decode(arg.trim_end_matches('/'))?;
    match what.to_ascii_lowercase().as_str() {
        "session" | "bookmark" if !arg.trim().is_empty() => Some(UrlTarget::Session(arg)),
        "ssh" => connect_target(&arg).map(UrlTarget::Connect),
        _ => None,
    }
}

/// Linux desktop entry that hands `termcraft://` links to the program.
pub fn desktop_entry(exe: &std::path::Path) -> String {
    let exe = exe.display().to_string().replace(['"', '`', '$', '\\'], "");
    format!(
        "[Desktop Entry]\nType=Application\nName=TermCraft (links)\nExec=\"{exe}\" %u\n\
         NoDisplay=true\nMimeType=x-scheme-handler/termcraft;\n"
    )
}

/// Make the system open `termcraft://` links with `exe` (current user
/// only). Returns what was done.
pub fn register(exe: &std::path::Path) -> Result<String, String> {
    if cfg!(target_os = "macos") {
        return Err("on macOS the TermCraft.app bundle declares the termcraft:// links".into());
    }
    if cfg!(windows) {
        let exe = exe.display().to_string();
        if exe.contains('"') {
            return Err("unexpected program path".into());
        }
        let key = r"HKCU\Software\Classes\termcraft";
        let command = format!("\"{exe}\" \"%1\"");
        let steps: [&[&str]; 3] = [
            &["add", key, "/ve", "/d", "URL:TermCraft", "/f"],
            &["add", key, "/v", "URL Protocol", "/d", "", "/f"],
            &[
                "add",
                &format!(r"{key}\shell\open\command"),
                "/ve",
                "/d",
                &command,
                "/f",
            ],
        ];
        for args in steps {
            let ok = std::process::Command::new("reg")
                .args(args)
                .output()
                .map_err(|e| e.to_string())?
                .status
                .success();
            if !ok {
                return Err("reg.exe refused the change".into());
            }
        }
        return Ok("termcraft:// links now open TermCraft".into());
    }
    let apps = directories::BaseDirs::new()
        .ok_or("no home folder")?
        .data_dir()
        .join("applications");
    std::fs::create_dir_all(&apps).map_err(|e| e.to_string())?;
    std::fs::write(apps.join("termcraft-url.desktop"), desktop_entry(exe))
        .map_err(|e| e.to_string())?;
    let _ = std::process::Command::new("xdg-mime")
        .args([
            "default",
            "termcraft-url.desktop",
            "x-scheme-handler/termcraft",
        ])
        .status();
    Ok("termcraft:// links now open TermCraft".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        assert_eq!(
            parse("termcraft://session/web%2001"),
            Some(UrlTarget::Session("web 01".into()))
        );
        assert_eq!(
            parse("ssh://ops@10.0.0.5:2222"),
            Some(UrlTarget::Connect("ops@10.0.0.5:2222".into()))
        );
        assert_eq!(
            parse("TermCraft://ssh/host.lan/"),
            Some(UrlTarget::Connect("host.lan".into()))
        );
        assert_eq!(
            parse("ssh://[::1]:22"),
            Some(UrlTarget::Connect("[::1]:22".into()))
        );
        for bad in [
            "ssh://-oProxyCommand=x@h",
            "ssh://h;rm -rf",
            "ssh://h:99999",
            "termcraft://exec/ls",
            "termcraft://session/",
            "http://example.com",
            "ssh://a\u{7}@b",
            "ssh://%2Dx",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        let entry = desktop_entry(std::path::Path::new("/opt/tc/term$craft"));
        assert!(entry.contains("Exec=\"/opt/tc/termcraft\" %u\n"));
        assert!(entry.contains("MimeType=x-scheme-handler/termcraft;"));
    }
}
