//! RDP and VNC open in the system's own client for now (an embedded
//! IronRDP / VNC view is tracked in docs/PARITY.md).

use std::path::{Path, PathBuf};
use std::process::Command;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdp_file_has_address_and_user() {
        let f = rdp_file("srv.local", 3390, Some("admin"));
        assert!(f.contains("full address:s:srv.local:3390\r\n"));
        assert!(f.contains("username:s:admin\r\n"));
        assert!(!rdp_file("h", 3389, None).contains("username"));
    }
}
