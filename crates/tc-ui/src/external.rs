//! Sessions that open in another program: RDP and VNC in the system's own
//! client (an embedded IronRDP / VNC view is tracked in docs/PARITY.md),
//! "File" and "Browser" sessions with the default application.
//!
//! Values from sessions are checked before they reach a command line or
//! a `.rdp` file: no control characters (no injected `.rdp` lines) and no
//! leading `-` (no injected options).

use std::path::{Path, PathBuf};
use std::process::Command;

/// A host name or user that is safe to pass to another program.
fn check_value(what: &str, value: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err(format!("{what} contains control characters"));
    }
    if value.starts_with('-') {
        return Err(format!("{what} must not start with '-'"));
    }
    if what == "host" && (value.is_empty() || value.contains(char::is_whitespace)) {
        return Err("invalid host name".into());
    }
    Ok(())
}

/// Contents of a minimal `.rdp` file for mstsc.
pub fn rdp_file(host: &str, port: u16, user: Option<&str>) -> String {
    let mut s = format!("full address:s:{host}:{port}\r\nprompt for credentials:i:1\r\n");
    if let Some(u) = user.filter(|u| !u.is_empty()) {
        s.push_str(&format!("username:s:{u}\r\n"));
    }
    s
}

fn find_in_path(exe: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

fn spawn(program: &Path, args: &[String]) -> Result<(), String> {
    Command::new(program)
        .args(args)
        .spawn()
        .map(drop)
        .map_err(|e| format!("{}: {e}", program.display()))
}

/// Start the platform RDP client; returns a description of what ran.
pub fn launch_rdp(
    host: &str,
    port: u16,
    user: Option<&str>,
    temp_dir: &Path,
) -> Result<String, String> {
    check_value("host", host)?;
    if let Some(u) = user {
        check_value("user name", u)?;
    }
    if cfg!(windows) {
        std::fs::create_dir_all(temp_dir).map_err(|e| e.to_string())?;
        let file = temp_dir.join(format!("{host}-{port}.rdp").replace([':', '/', '\\'], "_"));
        std::fs::write(&file, rdp_file(host, port, user)).map_err(|e| e.to_string())?;
        spawn(Path::new("mstsc.exe"), &[file.display().to_string()])?;
        return Ok("Windows Remote Desktop (mstsc)".into());
    }
    if cfg!(target_os = "macos") {
        let mut url = format!("rdp://full%20address=s:{host}:{port}");
        if let Some(u) = user.filter(|u| !u.is_empty()) {
            url.push_str(&format!("&username=s:{u}"));
        }
        spawn(Path::new("open"), &[url])?;
        return Ok("the default RDP app".into());
    }
    if let Some(p) = find_in_path("xfreerdp3").or_else(|| find_in_path("xfreerdp")) {
        let mut args = vec![format!("/v:{host}:{port}")];
        if let Some(u) = user.filter(|u| !u.is_empty()) {
            args.push(format!("/u:{u}"));
        }
        spawn(&p, &args)?;
        return Ok("FreeRDP".into());
    }
    if let Some(p) = find_in_path("remmina") {
        spawn(&p, &["-c".into(), format!("rdp://{host}:{port}")])?;
        return Ok("Remmina".into());
    }
    Err("no RDP client found (install FreeRDP or Remmina)".into())
}

/// Start the platform VNC viewer; returns a description of what ran.
pub fn launch_vnc(host: &str, port: u16) -> Result<String, String> {
    check_value("host", host)?;
    if cfg!(target_os = "macos") {
        spawn(Path::new("open"), &[format!("vnc://{host}:{port}")])?;
        return Ok("Screen Sharing".into());
    }
    let candidates: &[&str] = if cfg!(windows) {
        &["vncviewer.exe", "tvnviewer.exe", "uvnc_viewer.exe"]
    } else {
        &["vncviewer", "xtigervncviewer", "remmina"]
    };
    for exe in candidates {
        if let Some(p) = find_in_path(exe) {
            let args = if exe.starts_with("remmina") {
                vec!["-c".into(), format!("vnc://{host}:{port}")]
            } else {
                vec![format!("{host}::{port}")]
            };
            spawn(&p, &args)?;
            return Ok((*exe).to_string());
        }
    }
    Err("no VNC viewer found in PATH (e.g. TigerVNC or TightVNC)".into())
}

/// The system "open" command: Explorer, `open` or `xdg-open`. They take the
/// target as one argument, without a shell, so nothing in it is executed.
fn opener() -> &'static str {
    if cfg!(windows) {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// "File" session: open a local file or folder with its default application.
pub fn open_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    check_value("path", path)?;
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("{path} does not exist"));
    }
    // An absolute path, so a file named like an option cannot be one.
    let full = std::fs::canonicalize(p).map_err(|e| e.to_string())?;
    let full = full.display().to_string();
    // Windows `canonicalize` adds a `\\?\` prefix Explorer does not take.
    let full = full
        .strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(full);
    spawn(Path::new(opener()), &[full])?;
    Ok("the default application".into())
}

/// Open a local file in a text editor (never its default program, so a
/// downloaded script or executable is not run): Notepad on Windows,
/// TextEdit / the default text editor on macOS, a known GUI editor on Linux.
pub fn open_in_text_editor(path: &Path) -> Result<String, String> {
    let file = path.display().to_string();
    if cfg!(windows) {
        spawn(Path::new("notepad.exe"), &[file])?;
        return Ok("Notepad".into());
    }
    if cfg!(target_os = "macos") {
        spawn(Path::new("open"), &["-t".into(), file])?;
        return Ok("the default text editor".into());
    }
    for exe in [
        "gnome-text-editor",
        "gedit",
        "kate",
        "kwrite",
        "mousepad",
        "xed",
        "pluma",
        "geany",
    ] {
        if let Some(p) = find_in_path(exe) {
            spawn(&p, &[file])?;
            return Ok(exe.to_string());
        }
    }
    Err("no graphical text editor found (install gedit, kate or mousepad)".into())
}

/// Only web addresses: no other URL scheme (`file:`, app protocol
/// handlers…) is handed to the system.
pub fn web_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    check_value("URL", url)?;
    if url.contains(char::is_whitespace) || url.is_empty() {
        return Err("invalid URL".into());
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(url.to_string())
    } else if lower.contains("://") || lower.contains(':') && !lower.contains('.') {
        Err("only http:// and https:// addresses are opened".into())
    } else {
        Ok(format!("https://{url}"))
    }
}

/// "Browser" session: open the page in the default browser.
pub fn open_url(url: &str) -> Result<String, String> {
    let url = web_url(url)?;
    spawn(Path::new(opener()), &[url])?;
    Ok("the default browser".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_checked() {
        assert!(check_value("host", "srv.local").is_ok());
        assert!(check_value("host", "-oProxyCommand=x").is_err());
        assert!(check_value("host", "a\r\nalternate shell:s:cmd").is_err());
        assert!(check_value("host", "two words").is_err());
        assert!(launch_vnc("-via evil", 5900).is_err());
    }

    #[test]
    fn only_web_urls() {
        assert_eq!(web_url("example.com").unwrap(), "https://example.com");
        assert_eq!(web_url("http://x.test/a?b").unwrap(), "http://x.test/a?b");
        assert!(web_url("file:///etc/passwd").is_err());
        assert!(web_url("ms-settings:privacy").is_err());
        assert!(web_url("javascript:alert(1)").is_err());
        assert!(web_url("-x").is_err());
    }

    #[test]
    fn rdp_file_has_address_and_user() {
        let f = rdp_file("srv.local", 3390, Some("admin"));
        assert!(f.contains("full address:s:srv.local:3390\r\n"));
        assert!(f.contains("username:s:admin\r\n"));
        assert!(!rdp_file("h", 3389, None).contains("username"));
    }
}
