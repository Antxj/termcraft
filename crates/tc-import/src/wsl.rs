//! "Import WSL sessions": one WSL session per installed distribution.

use crate::mobaxterm::ImportReport;
use tc_core::{Session, SessionKind};

/// Installed WSL distributions (`wsl.exe -l -q`), read once per run.
/// Empty when WSL is not installed or not on Windows.
pub fn distributions() -> &'static [String] {
    static LIST: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        if !cfg!(windows) {
            return Vec::new();
        }
        let mut cmd = std::process::Command::new("wsl.exe");
        cmd.args(["-l", "-q"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.output() {
            Ok(out) if out.status.success() => parse_list(&out.stdout),
            _ => Vec::new(),
        }
    })
}

/// `wsl -l -q` prints UTF-16LE (sometimes with a BOM), one name per line.
pub fn parse_list(bytes: &[u8]) -> Vec<String> {
    let utf16 = bytes.starts_with(&[0xff, 0xfe])
        || bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).all(|&b| b == 0);
    let text = if utf16 {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0'))
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

pub fn sessions(distros: &[String]) -> ImportReport {
    let sessions = distros
        .iter()
        .map(|d| {
            let mut s = Session::new(format!("WSL {d}"), SessionKind::Wsl { distro: d.clone() });
            s.folder = "WSL sessions".into();
            s
        })
        .collect();
    ImportReport {
        sessions,
        unsupported: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_utf16() {
        let utf16: Vec<u8> = "\u{feff}Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(parse_list(&utf16), ["Ubuntu", "Debian"]);
        assert_eq!(parse_list(b"Alpine\n"), ["Alpine"]);
        let r = sessions(&["Ubuntu".into()]);
        assert_eq!(r.sessions[0].name, "WSL Ubuntu");
    }
}
