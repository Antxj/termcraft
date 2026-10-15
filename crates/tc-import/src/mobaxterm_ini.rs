//! "Import MobaXterm settings": the global settings and the trusted SSH host
//! keys of a `MobaXterm.ini` (sessions are read by [`crate::mobaxterm`]).
//!
//! Only settings whose meaning is clear are applied; each one is listed in
//! the report, and everything else is left as it is. Saved passwords are
//! never read: MobaXterm encrypts them with its own scheme.
//!
//! Host keys are stored PuTTY-style (`ssh-ed25519@22:host` = `0x…,0x…`) and
//! become OpenSSH `known_hosts` lines. When the file also has the key's
//! SHA-256 fingerprint (`sha256-ssh-ed25519@22:host`), the converted key
//! must match it, or it is dropped.

use base64::Engine as _;
use sha2::Digest as _;
use std::collections::BTreeMap;
use tc_core::Settings;
use tc_core::settings::{
    BellType, LocalShell, MasterPrompt, PopupPosition, SavePasswords, TerminalColors, ToolbarStyle,
};

/// `[Section]` → key → value, in file order of sections (later wins).
pub type Ini = BTreeMap<String, BTreeMap<String, String>>;

/// A `MobaXterm.ini` to offer at first start: next to TermCraft (a portable
/// MobaXterm unpacked in the same folder), else the one of an installed
/// MobaXterm (`%APPDATA%\MobaXterm` on Windows). It is only ever read.
pub fn find(program_dir: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = program_dir
        .map(|d| d.join("MobaXterm.ini"))
        .into_iter()
        .collect();
    if cfg!(windows)
        && let Some(appdata) = std::env::var_os("APPDATA")
    {
        candidates.push(std::path::Path::new(&appdata).join(r"MobaXterm\MobaXterm.ini"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn parse(text: &str) -> Ini {
    let mut ini = Ini::new();
    let mut section: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = Some(name.to_string());
            ini.entry(name.to_string()).or_default();
            continue;
        }
        if let (Some(s), Some((k, v))) = (&section, line.split_once('=')) {
            ini.entry(s.clone())
                .or_default()
                .insert(k.trim().to_string(), v.to_string());
        }
    }
    ini
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SettingsReport {
    /// What was applied, as `Section.Key`.
    pub applied: Vec<String>,
    /// Settings found but deliberately left alone, and why.
    pub not_applied: Vec<(String, &'static str)>,
}

struct Apply<'a> {
    ini: &'a Ini,
    report: SettingsReport,
}

impl Apply<'_> {
    fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.ini.get(section)?.get(key).map(|v| v.trim())
    }

    fn note(&mut self, section: &str, key: &str) {
        self.report.applied.push(format!("{section}.{key}"));
    }

    fn leave(&mut self, section: &str, key: &str, why: &'static str) {
        if self.get(section, key).is_some() {
            self.report
                .not_applied
                .push((format!("{section}.{key}"), why));
        }
    }

    /// A `0` / `1` value.
    fn flag(&mut self, section: &str, key: &str, set: impl FnOnce(bool)) {
        let value = match self.get(section, key) {
            Some("1") => true,
            Some("0") => false,
            _ => return,
        };
        set(value);
        self.note(section, key);
    }

    /// An integer in `range`.
    fn number<T: TryFrom<i64>>(
        &mut self,
        section: &str,
        key: &str,
        range: std::ops::RangeInclusive<i64>,
        set: impl FnOnce(T),
    ) {
        let Some(n) = self.get(section, key).and_then(|v| v.parse::<i64>().ok()) else {
            return;
        };
        if !range.contains(&n) {
            return;
        }
        if let Ok(v) = T::try_from(n) {
            set(v);
            self.note(section, key);
        }
    }
}

fn rgb(value: &str) -> Option<[u8; 3]> {
    let mut parts = value.split(',').map(|p| p.trim().parse::<u8>());
    let c = [
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ];
    parts.next().is_none().then_some(c)
}

/// MobaXterm's own fonts are not available elsewhere: the built-in one.
fn font_name(name: &str) -> String {
    match name.trim() {
        "MobaFont" | "MobaFira" | "MobaFanta" => String::new(),
        other => other.to_string(),
    }
}

/// "Local terminal environment" names (the reference lists them as text).
fn local_shell(name: &str) -> Option<LocalShell> {
    let n = name.to_ascii_lowercase();
    Some(if n.starts_with("bash") {
        LocalShell::Bash
    } else if n.starts_with("zsh") {
        LocalShell::Zsh
    } else if n.contains("powershell") {
        LocalShell::PowerShell
    } else if n == "cmd" || n.starts_with("cmd ") || n.contains("command prompt") {
        LocalShell::Cmd
    } else if n.contains("wsl") || n.contains("ubuntu") || n.contains("debian") {
        LocalShell::Wsl
    } else {
        return None;
    })
}

/// Apply the global settings of `ini` to `s`.
pub fn apply_settings(ini: &Ini, s: &mut Settings) -> SettingsReport {
    let mut a = Apply {
        ini,
        report: SettingsReport::default(),
    };
    let t = &mut s.terminal;

    // [Font]
    if let Some(name) = a.get("Font", "Font2") {
        t.font_name = font_name(name);
        a.note("Font", "Font2");
    }
    a.flag("Font", "FontIsBold2", |v| t.font_bold = v);
    // Points, as the session font size (1 pt = 4/3 px).
    a.number("Font", "FontHeight2", 4..=72, |pt: u16| {
        t.font_size = f32::from(pt) * 4.0 / 3.0;
    });
    a.flag("Font", "LockRowsCols", |v| t.fixed_grid = v);
    a.number("Font", "LockedCols", 20..=500, |v: u16| t.fixed_cols = v);
    a.number("Font", "LockedRows", 5..=200, |v: u16| t.fixed_rows = v);

    // [Colors] + [Cursor]: all 19 colors, or none.
    const ANSI: [&str; 8] = [
        "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
    ];
    let color = |key: &str| a.get("Colors", key).and_then(rgb);
    let mut ansi = [[0u8; 3]; 16];
    let mut complete = true;
    for (i, name) in ANSI.iter().enumerate() {
        match (color(name), color(&format!("Bold{name}"))) {
            (Some(n), Some(b)) => {
                ansi[i] = n;
                ansi[i + 8] = b;
            }
            _ => complete = false,
        }
    }
    if let (true, Some(fg), Some(bg), Some(cursor)) = (
        complete,
        color("ForegroundColour"),
        color("BackgroundColour"),
        color("CursorColour"),
    ) {
        let cursor_type = a
            .get("Cursor", "CursorType")
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|v| *v <= 2)
            .unwrap_or(0);
        t.colors = Some(TerminalColors {
            foreground: fg,
            background: bg,
            cursor,
            ansi,
            cursor_type,
            cursor_blink: a.get("Cursor", "CursorBlinks") == Some("1"),
        });
        a.note("Colors", "*");
        a.note("Cursor", "*");
    }
    a.number("Colors", "SyntaxType", 0..=5, |v: usize| {
        t.syntax_highlighting = v
    });

    // [Misc]
    a.flag("Misc", "BackupIni", |v| s.general.auto_backup = v);
    a.flag("Misc", "WinPath", |v| t.use_windows_path = v);
    a.flag("Misc", "ConfirmExit", |v| t.confirm_exit = v);
    a.flag("Misc", "Scrollbar", |v| t.expert.scrollbar = v);
    a.flag("Misc", "WarnBeforePasteMultipleLines", |v| {
        t.warn_multiline_paste = v;
    });
    a.flag("Misc", "BackspaceSendsBS", |v| t.backspace_ctrl_h = v);
    a.number("Misc", "BellType", 0..=3, |v: u8| {
        t.expert.bell = [
            BellType::None,
            BellType::Sound,
            BellType::Visual,
            BellType::Speaker,
        ][usize::from(v)];
    });
    if let Some(shell) = a.get("Misc", "LocalShell").and_then(local_shell) {
        t.local_shell = shell;
        a.note("Misc", "LocalShell");
    }
    let save = match a.get("Misc", "StorePasswords") {
        Some("Always") => Some(SavePasswords::Always),
        Some("Never") => Some(SavePasswords::Never),
        Some("Ask") => Some(SavePasswords::Ask),
        _ => None,
    };
    if let Some(save) = save {
        s.passwords.save = save;
        a.note("Misc", "StorePasswords");
    }
    a.flag("Misc", "StoreSSHKeysPassphrases", |v| {
        s.passwords.save_key_passphrases = v;
    });
    a.number("Misc", "WhenToPromptForMasterPassword", 0..=2, |v: u8| {
        s.passwords.master_prompt = if v == 0 {
            MasterPrompt::NewComputer
        } else {
            MasterPrompt::EveryStartup
        };
    });
    // MobaXterm's home is the one of its own Cygwin; TermCraft's local
    // terminals use the user's home, which a "<Temp directory>" would hide.
    a.leave(
        "Misc",
        "HomeDir",
        "the local terminal keeps your own home folder (MobaXterm's is its built-in Cygwin one)",
    );
    a.leave(
        "Misc",
        "SkinSat",
        "MobaXterm's skin saturation is on another scale than TermCraft's",
    );
    a.number("Misc", "TransTerm", 0..=80, |v: u8| s.view.transparency = v);
    a.number("Misc", "TransDetachedTabs", 0..=80, |v: u8| {
        s.view.detached_transparency = v;
    });

    a.flag("Misc", "AllowMultiInstances", |v| {
        s.general.multiple_instances = v
    });
    let startup = match a.get("Misc", "DefaultHome") {
        Some("<Home>") => Some(tc_core::settings::StartupTab::Home),
        Some("<Home (pinned)>") => Some(tc_core::settings::StartupTab::HomePinned),
        Some("<Terminal>") => Some(tc_core::settings::StartupTab::Terminal),
        _ => None,
    };
    if let Some(tab) = startup {
        s.general.startup_tab = tab;
        a.note("Misc", "DefaultHome");
    }
    a.number("Misc", "NewTabPosition", 0..=2, |v: usize| {
        s.general.new_tab_position = tc_core::settings::NewTabPosition::ALL[v];
    });

    // [SSH]
    let ssh = &mut s.ssh;
    a.flag("SSH", "RemoteMonitoring", |v| ssh.remote_monitoring = v);
    a.flag("SSH", "DisplaySSHBanner", |v| ssh.show_banner = v);
    a.flag("SSH", "AutoStartSSHGUI", |v| ssh.browser = v);
    a.flag("SSH", "UseExternalPageant", |v| ssh.use_pageant = v);
    a.flag("SSH", "UseInternalMobAgent", |v| ssh.internal_agent = v);
    a.flag("SSH", "UseExternalWindowsAgent", |v| {
        ssh.use_system_agent = v
    });
    a.flag("SSH", "SSHKeepAlive2", |v| {
        ssh.keepalive_secs = if v { ssh.keepalive_secs.max(60) } else { 0 };
    });

    // [Display]
    let view = &mut s.view;
    a.flag("Display", "SidebarRight", |v| view.sidebar_right = v);
    a.flag("Display", "VisibleTabNum", |v| view.show_tab_numbers = v);
    a.flag("Display", "VisibleTabClose", |v| view.show_close_button = v);
    a.flag("Display", "RoundedTabs", |v| view.rounded_tabs = v);
    a.flag("Display", "DisableQuickConnect", |v| {
        view.no_quick_connect_without_sidebar = v;
    });
    // "Menus and buttons": show menu bar / show buttons bar / show both.
    a.number("Display", "MenuAndButtons", 0..=2, |v: u8| {
        view.show_menu_bar = v != 1;
        view.show_toolbar = v != 0;
    });
    a.number("Display", "BtnType2", 0..=2, |v: u8| {
        view.toolbar_style = [
            ToolbarStyle::Small,
            ToolbarStyle::Standard,
            ToolbarStyle::StandardWithCaptions,
        ][usize::from(v)];
    });

    // [PopupConsole]
    let popup = &mut s.popup;
    a.number("PopupConsole", "Transparency", 0..=90, |v: u8| {
        popup.transparency = v
    });
    a.number("PopupConsole", "Size", 10..=100, |v: u8| popup.size = v);
    a.number("PopupConsole", "Position", 0..=4, |v: u8| {
        popup.position = [
            PopupPosition::Left,
            PopupPosition::Top,
            PopupPosition::Right,
            PopupPosition::Bottom,
            PopupPosition::Floating,
        ][usize::from(v)];
    });
    a.flag("PopupConsole", "Sizeable", |v| popup.sizeable = v);
    a.flag("PopupConsole", "BackgroundInsteadOfClose", |v| {
        popup.hide_instead_of_close = v;
    });
    a.report
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HostKeys {
    /// OpenSSH `known_hosts` lines.
    pub lines: Vec<String>,
    /// Keys checked against the fingerprint the file had for them.
    pub verified: usize,
    /// Entries not understood, or whose key did not match its fingerprint.
    pub skipped: usize,
}

fn hex_int(s: &str) -> Option<Vec<u8>> {
    let h = s.trim().strip_prefix("0x")?;
    if h.is_empty() || h.len() > 2048 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let h = if h.len() % 2 == 1 {
        format!("0{h}")
    } else {
        h.to_string()
    };
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok())
        .collect()
}

fn put_string(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

/// SSH `mpint`: big-endian, no leading zeros, a zero byte when the top
/// bit is set.
fn put_mpint(out: &mut Vec<u8>, be: &[u8]) {
    let start = be.iter().position(|&b| b != 0).unwrap_or(be.len());
    let mut v = be[start..].to_vec();
    if v.first().is_some_and(|b| b & 0x80 != 0) {
        v.insert(0, 0);
    }
    put_string(out, &v);
}

/// Left-pad a big-endian number to `len` bytes.
fn fixed(be: &[u8], len: usize) -> Option<Vec<u8>> {
    let start = be.iter().position(|&b| b != 0).unwrap_or(be.len());
    let v = &be[start..];
    (v.len() <= len).then(|| {
        let mut out = vec![0; len - v.len()];
        out.extend_from_slice(v);
        out
    })
}

/// The OpenSSH key blob of a PuTTY host key value.
fn key_blob(kind: &str, value: &str) -> Option<(String, Vec<u8>)> {
    let parts: Vec<&str> = value.split(',').collect();
    let mut blob = Vec::new();
    match kind {
        "ssh-ed25519" => {
            let [x, y] = parts.as_slice() else {
                return None;
            };
            let (x, y) = (hex_int(x)?, fixed(&hex_int(y)?, 32)?);
            // The point is y little-endian, with x's parity in the top bit.
            let mut key: Vec<u8> = y.iter().rev().copied().collect();
            if key[31] & 0x80 != 0 {
                return None;
            }
            if x.last().is_some_and(|b| b & 1 == 1) {
                key[31] |= 0x80;
            }
            put_string(&mut blob, b"ssh-ed25519");
            put_string(&mut blob, &key);
            Some(("ssh-ed25519".into(), blob))
        }
        "rsa2" => {
            let [e, n] = parts.as_slice() else {
                return None;
            };
            put_string(&mut blob, b"ssh-rsa");
            put_mpint(&mut blob, &hex_int(e)?);
            put_mpint(&mut blob, &hex_int(n)?);
            Some(("ssh-rsa".into(), blob))
        }
        k if k.starts_with("ecdsa-sha2-nistp") => {
            let [curve, x, y] = parts.as_slice() else {
                return None;
            };
            let size = match *curve {
                "nistp256" => 32,
                "nistp384" => 48,
                "nistp521" => 66,
                _ => return None,
            };
            if k != format!("ecdsa-sha2-{curve}") {
                return None;
            }
            let mut point = vec![4u8];
            point.extend(fixed(&hex_int(x)?, size)?);
            point.extend(fixed(&hex_int(y)?, size)?);
            put_string(&mut blob, k.as_bytes());
            put_string(&mut blob, curve.as_bytes());
            put_string(&mut blob, &point);
            Some((k.to_string(), blob))
        }
        _ => None,
    }
}

/// A host name or address that is safe in `known_hosts`.
fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_:".contains(c))
}

/// The trusted host keys of `ini` (`[SSH_Hostkeys]`).
pub fn host_keys(ini: &Ini) -> HostKeys {
    let mut out = HostKeys::default();
    let Some(section) = ini.get("SSH_Hostkeys") else {
        return out;
    };
    for (name, value) in section {
        if name.starts_with("sha256-") {
            continue;
        }
        let Some((kind, rest)) = name.split_once('@') else {
            out.skipped += 1;
            continue;
        };
        let Some((port, host)) = rest.split_once(':') else {
            out.skipped += 1;
            continue;
        };
        let (Ok(port), true) = (port.parse::<u16>(), host_ok(host)) else {
            out.skipped += 1;
            continue;
        };
        let Some((key_type, blob)) = key_blob(kind, value) else {
            out.skipped += 1;
            continue;
        };
        // The fingerprint the file has for this key, if any.
        if let Some(fp) = section.get(&format!("sha256-{name}")) {
            let digest = sha2::Sha256::digest(&blob);
            let ours: Vec<String> = digest.iter().map(|b| format!("{b:02x}")).collect();
            if !ours.join(":").eq_ignore_ascii_case(fp.trim()) {
                out.skipped += 1;
                continue;
            }
            out.verified += 1;
        }
        let pattern = if port == 22 {
            host.to_string()
        } else {
            format!("[{host}]:{port}")
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        out.lines.push(format!("{pattern} {key_type} {b64}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ed25519 key in OpenSSH and PuTTY form (seed 7…7, made with
    /// ed25519-dalek; x / y computed from the compressed point offline).
    fn putty_ed25519(blob_key: &[u8; 32]) -> String {
        // y = the 255 low bits little-endian; x only matters for parity
        // here, so a number with the right parity stands in for it.
        let mut y = *blob_key;
        let odd = y[31] & 0x80 != 0;
        y[31] &= 0x7f;
        let y_be: String = y.iter().rev().map(|b| format!("{b:02x}")).collect();
        format!(
            "0x{},0x{}",
            if odd { "3" } else { "2" },
            y_be.trim_start_matches('0')
        )
    }

    #[test]
    fn ed25519_round_trip_and_fingerprint_check() {
        let key: [u8; 32] = std::array::from_fn(|i| (i as u8).wrapping_mul(37) | 0x80);
        let value = putty_ed25519(&key);
        let (kind, blob) = key_blob("ssh-ed25519", &value).unwrap();
        assert_eq!(kind, "ssh-ed25519");
        assert_eq!(&blob[blob.len() - 32..], &key);

        let fp: Vec<String> = sha2::Sha256::digest(&blob)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let text = format!(
            "[SSH_Hostkeys]\r\nssh-ed25519@22:srv.test={value}\r\nsha256-ssh-ed25519@22:srv.test={}\r\n\
             ssh-ed25519@2222:10.0.0.9={value}\r\nssh-ed25519@22:bad.test={value}\r\n\
             sha256-ssh-ed25519@22:bad.test=00:11\r\nssh-ed25519@22:x;rm={value}\r\nweird=1\r\n",
            fp.join(":")
        );
        let keys = host_keys(&parse(&text));
        assert_eq!(keys.verified, 1);
        assert_eq!(keys.skipped, 3, "{keys:?}");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        assert_eq!(
            keys.lines,
            [
                format!("[10.0.0.9]:2222 ssh-ed25519 {b64}"),
                format!("srv.test ssh-ed25519 {b64}"),
            ]
        );
        // The lines are valid known_hosts entries.
        assert_eq!(tc_core::known_hosts::parse(&keys.lines.join("\n")).len(), 2);
    }

    /// Check a real `MobaXterm.ini` without changing or showing it: only
    /// counts are printed.
    /// `TC_MOBAXTERM_INI=path cargo test -p tc-import real_ini -- --ignored --nocapture`
    #[test]
    #[ignore = "needs TC_MOBAXTERM_INI"]
    fn real_ini() {
        let path = std::env::var_os("TC_MOBAXTERM_INI").expect("TC_MOBAXTERM_INI");
        let text = crate::read_text_lossy(std::path::Path::new(&path)).unwrap();
        let ini = parse(&text);
        let keys = host_keys(&ini);
        let fingerprints = ini
            .get("SSH_Hostkeys")
            .map_or(0, |s| s.keys().filter(|k| k.starts_with("sha256-")).count());
        println!(
            "host keys: {} converted, {} verified against {} fingerprints, {} skipped",
            keys.lines.len(),
            keys.verified,
            fingerprints,
            keys.skipped
        );
        let mut s = Settings::default();
        let report = apply_settings(&ini, &mut s);
        println!("settings applied: {}", report.applied.join(", "));
        println!("left alone: {:?}", report.not_applied);
        println!(
            "sessions: {}",
            crate::mobaxterm::parse(&text).sessions.len()
        );
    }

    #[test]
    fn find_next_to_the_program() {
        let dir = tempfile::tempdir().unwrap();
        // (On Windows an installed MobaXterm may also be found.)
        let none = find(Some(dir.path()));
        assert!(none.is_none() || !none.unwrap().starts_with(dir.path()));
        std::fs::write(dir.path().join("MobaXterm.ini"), "[Misc]\r\n").unwrap();
        assert_eq!(
            find(Some(dir.path())),
            Some(dir.path().join("MobaXterm.ini"))
        );
    }

    #[test]
    fn rsa_and_ecdsa_blobs() {
        let (k, blob) = key_blob("rsa2", "0x10001,0xc0ffee").unwrap();
        assert_eq!(k, "ssh-rsa");
        let mut want = Vec::new();
        put_string(&mut want, b"ssh-rsa");
        put_string(&mut want, &[1, 0, 1]);
        put_string(&mut want, &[0, 0xc0, 0xff, 0xee]);
        assert_eq!(blob, want);
        let (k, blob) = key_blob("ecdsa-sha2-nistp256", "nistp256,0x1,0x2").unwrap();
        assert_eq!(k, "ecdsa-sha2-nistp256");
        assert_eq!(blob.len(), 4 + 19 + 4 + 8 + 4 + 65);
        assert!(key_blob("ecdsa-sha2-nistp256", "nistp384,0x1,0x2").is_none());
        assert!(key_blob("ssh-dss", "0x1,0x2,0x3,0x4").is_none());
        assert!(key_blob("ssh-ed25519", "0x1").is_none());
    }

    const SAMPLE: &str = "[Misc]\r\nBackupIni=0\r\nWinPath=0\r\nBellType=2\r\nLocalShell=Bash (64 bit)\r\n\
        StorePasswords=Never\r\nHomeDir=<Temp directory>\r\nSkinSat=50\r\nTransTerm=999\r\n\
        [SSH]\r\nRemoteMonitoring=0\r\nSSHKeepAlive2=1\r\n\
        [Display]\r\nMenuAndButtons=1\r\nBtnType2=0\r\nRoundedTabs=0\r\n\
        [Font]\r\nFont2=Consolas\r\nFontIsBold2=1\r\nFontHeight2=12\r\nLockRowsCols=1\r\nLockedCols=100\r\nLockedRows=30\r\n\
        [Cursor]\r\nCursorType=1\r\nCursorBlinks=1\r\n\
        [PopupConsole]\r\nPosition=1\r\nSize=50\r\n\
        [Passwords]\r\nsecret=do-not-touch\r\n";

    fn with_colors() -> String {
        let mut text = SAMPLE.to_string();
        text.push_str("[Colors]\r\n");
        for (i, name) in [
            "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
        ]
        .iter()
        .enumerate()
        {
            text.push_str(&format!("{name}={i},0,0\r\nBold{name}={i},1,1\r\n"));
        }
        text.push_str("ForegroundColour=1,2,3\r\nBackgroundColour=4,5,6\r\nCursorColour=7,8,9\r\n");
        text
    }

    #[test]
    fn settings_are_applied_and_reported() {
        let mut s = Settings::default();
        let report = apply_settings(&parse(&with_colors()), &mut s);
        let t = &s.terminal;
        assert!(!s.general.auto_backup && !t.use_windows_path);
        assert_eq!(t.expert.bell, BellType::Visual);
        assert_eq!(t.local_shell, LocalShell::Bash);
        assert_eq!(s.passwords.save, SavePasswords::Never);
        assert_eq!(s.general.home_location, Default::default(), "own home kept");
        assert_eq!(s.view.saturation, Settings::default().view.saturation);
        let left: Vec<&str> = report.not_applied.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(left, ["Misc.HomeDir", "Misc.SkinSat"]);
        assert_eq!(
            s.view.transparency,
            Settings::default().view.transparency,
            "out of range"
        );
        assert!(!s.ssh.remote_monitoring);
        assert!(s.ssh.keepalive_secs >= 60);
        assert!(!s.view.show_menu_bar && s.view.show_toolbar);
        assert_eq!(s.view.toolbar_style, ToolbarStyle::Small);
        assert_eq!(t.font_name, "Consolas");
        assert!(t.font_bold && t.fixed_grid);
        assert_eq!((t.fixed_cols, t.fixed_rows), (100, 30));
        assert_eq!(t.font_size, 16.0);
        assert_eq!(s.popup.position, PopupPosition::Top);
        assert_eq!(s.popup.size, 50);
        let colors = t.colors.as_ref().unwrap();
        assert_eq!(colors.foreground, [1, 2, 3]);
        assert_eq!(colors.ansi[1], [1, 0, 0]);
        assert_eq!(colors.ansi[9], [1, 1, 1]);
        assert_eq!((colors.cursor_type, colors.cursor_blink), (1, true));
        assert!(report.applied.contains(&"Font.Font2".to_string()));
        assert!(report.applied.iter().all(|k| !k.starts_with("Passwords")));
        assert!(!report.applied.contains(&"Misc.TransTerm".to_string()));
    }

    #[test]
    fn partial_colors_and_moba_fonts_are_left_alone() {
        let mut s = Settings::default();
        let text = format!("{SAMPLE}[Colors]\r\nRed=1,2,3\r\n").replace("Consolas", "MobaFont");
        apply_settings(&parse(&text), &mut s);
        assert!(s.terminal.colors.is_none(), "incomplete palette ignored");
        assert_eq!(s.terminal.font_name, "", "MobaFont → built-in font");
    }
}
