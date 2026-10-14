//! Session options and settings read when a tab starts.

use super::*;

/// What a key does at the "Session stopped" prompt.
pub(super) fn ended_key(bytes: &[u8], keys: &ExpertTerminal) -> Option<EndedAction> {
    if !tc_term::is_single_key(bytes) {
        return None; // a paste is not a key
    }
    if tc_term::key_name_matches(&keys.reconnect_key, bytes) {
        Some(EndedAction::Restart)
    } else if tc_term::key_name_matches(&keys.save_key, bytes) {
        Some(EndedAction::Save)
    } else if keys.exit_key == tc_core::settings::ANY_OTHER_KEY
        || tc_term::key_name_matches(&keys.exit_key, bytes)
    {
        Some(EndedAction::Close)
    } else {
        None
    }
}

/// How a reconnection key is named in the "Session stopped" message.
pub(super) fn key_label(name: &str) -> String {
    match name {
        tc_core::settings::ANY_OTHER_KEY => "any other key".into(),
        "RETURN" => "<Return>".into(),
        n if n.chars().count() == 1 => n.to_string(),
        n => format!("<{n}>"),
    }
}

/// 3_12 "Do not exit after command ends": run the command, then the
/// user's login shell in the same terminal.
pub(super) fn keep_open(command: &str, keep: bool) -> String {
    if keep {
        format!("{command}; exec \"${{SHELL:-/bin/sh}}\" -l")
    } else {
        command.to_string()
    }
}

pub fn agent_config(ssh: &tc_core::settings::SshSettings) -> tc_ssh::AgentConfig {
    tc_ssh::AgentConfig {
        system: ssh.use_system_agent,
        pageant: ssh.use_pageant,
        socket: None,
        keys: if ssh.internal_agent {
            ssh.agent_keys
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .map(PathBuf::from)
                .collect()
        } else {
            Vec::new()
        },
    }
}

/// A Delphi `TColor` (decimal, `0x00BBGGRR`); `-1` = none.
pub(super) fn delphi_color(value: &str) -> Option<[u8; 3]> {
    let v: i64 = value.trim().parse().ok()?;
    if !(0..=0x00ff_ffff).contains(&v) {
        return None;
    }
    let v = v as u32;
    Some([
        (v & 0xff) as u8,
        (v >> 8 & 0xff) as u8,
        (v >> 16 & 0xff) as u8,
    ])
}

/// 4_40 "Paste delay": 0 Auto, 1 none, then 10…100 ms, 200 ms, 360 ms.
pub(super) fn paste_delay(session: Option<&Session>) -> std::time::Duration {
    let index = session
        .and_then(|s| s.options.get("4_40"))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let ms = match index {
        2..=11 => (index - 1) * 10,
        12 => 200,
        13 => 360,
        _ => 0,
    };
    std::time::Duration::from_millis(ms)
}

/// The local X server for X11 forwarding: `$DISPLAY`, else XQuartz on
/// macOS (started on demand); with a notice for the terminal.
pub(super) fn x11_display() -> (Option<String>, Option<&'static str>) {
    if let Some(d) = tc_ssh::x11::default_display() {
        return (Some(d), None);
    }
    if crate::external::start_xquartz() {
        return (
            Some(":0".into()),
            Some("[90mStarting XQuartz for X11 forwarding …[0m"),
        );
    }
    (
        None,
        Some("[33mX11-forwarding: no X server (DISPLAY is not set).[0m"),
    )
}

/// 4_36 "Syntax highlighting" index of the session, when it sets one.
pub(super) fn session_syntax(session: Option<&Session>) -> Option<usize> {
    session
        .and_then(|s| s.options.get("4_36"))
        .and_then(|v| v.parse::<usize>().ok())
}

/// 4_6 "Term charset" (default UTF-8).
pub(super) fn term_charset(session: Option<&Session>) -> tc_term::TermCharset {
    let index = session
        .and_then(|s| s.options.get("4_6"))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(tc_term::charset::UTF8);
    tc_term::TermCharset::from_index(index)
}

/// Session 4_12 "Log terminal output" (4_13 folder, `<>` = the global
/// one), else the global log folder.
pub(super) fn log_folder(
    session: Option<&Session>,
    global: Option<&std::path::Path>,
) -> Option<PathBuf> {
    let opt = |id: &str| session.and_then(|s| s.options.get(id)).map(String::as_str);
    if opt("4_12").is_some_and(schema::is_true) {
        let folder = opt("4_13")
            .map(str::trim)
            .filter(|f| !f.is_empty() && *f != "<>");
        return folder
            .map(PathBuf::from)
            .or_else(|| global.map(PathBuf::from));
    }
    global.map(PathBuf::from)
}

pub(super) fn shell_kind(shell: LocalShell) -> tc_pty::ShellKind {
    match shell {
        LocalShell::Bash => tc_pty::ShellKind::Bash,
        LocalShell::Zsh => tc_pty::ShellKind::Zsh,
        LocalShell::Cmd => tc_pty::ShellKind::Cmd,
        LocalShell::PowerShell => tc_pty::ShellKind::PowerShell,
        LocalShell::Wsl => tc_pty::ShellKind::Wsl,
    }
}

/// The proxy of a Telnet (3_12 …) or VNC (3_14 …) session.
pub(super) fn session_proxy(session: Option<&Session>) -> Option<tc_ssh::proxy::Proxy> {
    let session = session?;
    let ids = match session.kind {
        SessionKind::Telnet { .. } => &tc_ssh::proxy::TELNET_IDS,
        SessionKind::Vnc { .. } => &tc_ssh::proxy::VNC_IDS,
        _ => return None,
    };
    tc_ssh::proxy::from_options_with(ids, &|id| session.options.get(id).cloned())
}
