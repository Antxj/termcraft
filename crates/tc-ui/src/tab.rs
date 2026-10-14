use crate::credentials::{Credentials, PromptAction, target_for};
use crate::folder_hook::{self, FolderHook};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tc_core::settings::{ExpertTerminal, LocalShell, SavePasswords};
use tc_core::{Session, SessionKind, schema};
use tc_pty::{PtyEvent, PtySession};
use tc_ssh::{
    ConnectParams, RemoteEntry, Reply, SecretKind, SftpEvent, SftpRequest, SshEvent, SshSession,
};
use tc_term::Terminal;
use tc_vault::{Target, Vault};

/// A Mosh session between "SSH started" and "mosh-client running".
struct MoshStart {
    /// Server output so far (holds the session key: never shown).
    output: Vec<u8>,
    ip: std::net::IpAddr,
    client: PathBuf,
    prediction: &'static str,
}

/// What every tab needs from the app.
pub struct TabEnv {
    pub scrollback: usize,
    pub known_hosts: PathBuf,
    pub repaint: Arc<dyn Fn() + Send + Sync>,
    pub vault: Arc<Vault>,
    pub save_passwords: SavePasswords,
    pub ssh: tc_core::settings::SshSettings,
    /// Global "Backspace sends ^H" (local terminals).
    pub backspace_ctrl_h: bool,
    pub paste_right_click: bool,
    /// "Local terminal environment".
    pub local_shell: tc_core::settings::LocalShell,
    /// "Save SSH keys passphrases as well".
    pub save_key_passphrases: bool,
    /// Configuration › Terminal "Log terminal output to the following
    /// directory" (sessions can log on their own: 4_12 / 4_13).
    pub log_dir: Option<PathBuf>,
    /// "Use Windows PATH" in the local Bash / Zsh.
    pub use_windows_path: bool,
    /// "Home directory location": `HOME` of the local Bash / Zsh.
    pub home: Option<PathBuf>,
}

/// Keys of the "Session stopped" prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedAction {
    /// Return: close the tab.
    Close,
    /// R: open the same session again in this tab.
    Restart,
    /// S: save the terminal text to a file.
    Save,
}

/// What a key does at the "Session stopped" prompt.
fn ended_key(bytes: &[u8], keys: &ExpertTerminal) -> Option<EndedAction> {
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
fn key_label(name: &str) -> String {
    match name {
        tc_core::settings::ANY_OTHER_KEY => "any other key".into(),
        "RETURN" => "<Return>".into(),
        n if n.chars().count() == 1 => n.to_string(),
        n => format!("<{n}>"),
    }
}

/// A Telnet / RDP / VNC session through SSH jump hosts: a local tunnel
/// to the end server is opened first, then the session uses it.
struct ViaGateway {
    target: GatewayTarget,
    host: String,
    port: u16,
    hops: Vec<tc_core::jump_hosts::JumpHost>,
}

enum GatewayTarget {
    Telnet,
    Rdp(Option<String>),
    Vnc,
}

/// The jump hosts of a Telnet / RDP / VNC session, if it has any.
fn via_gateway(session: Option<&Session>) -> Option<ViaGateway> {
    let session = session?;
    let ids = tc_core::jump_hosts::ids_for(session.kind.type_index())?;
    let hops = tc_core::jump_hosts::parse(&session.options, ids);
    if hops.is_empty() {
        return None;
    }
    let (target, host, port) = match &session.kind {
        SessionKind::Telnet { host, port } => (GatewayTarget::Telnet, host, *port),
        SessionKind::Rdp { host, port, user } => (GatewayTarget::Rdp(user.clone()), host, *port),
        SessionKind::Vnc { host, port } => (GatewayTarget::Vnc, host, *port),
        _ => return None,
    };
    Some(ViaGateway {
        target,
        host: host.clone(),
        port,
        hops,
    })
}

/// An FTPS certificate that did not verify.
pub struct CertPrompt {
    pub fingerprint: String,
    pub reason: String,
}

/// "Find in terminal" bar of a tab.
#[derive(Default)]
pub struct FindBar {
    pub text: String,
    pub match_case: bool,
    /// Where the last match started (the next search goes above it).
    pub last: Option<tc_term::Point>,
    pub not_found: bool,
    /// Give the text box focus on the next frame.
    pub focus: bool,
}

/// 3_12 "Do not exit after command ends": run the command, then the
/// user's login shell in the same terminal.
fn keep_open(command: &str, keep: bool) -> String {
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
fn delphi_color(value: &str) -> Option<[u8; 3]> {
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
fn paste_delay(session: Option<&Session>) -> std::time::Duration {
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
fn x11_display() -> (Option<String>, Option<&'static str>) {
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
fn session_syntax(session: Option<&Session>) -> Option<usize> {
    session
        .and_then(|s| s.options.get("4_36"))
        .and_then(|v| v.parse::<usize>().ok())
}

/// 4_6 "Term charset" (default UTF-8).
fn term_charset(session: Option<&Session>) -> tc_term::TermCharset {
    let index = session
        .and_then(|s| s.options.get("4_6"))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(tc_term::charset::UTF8);
    tc_term::TermCharset::from_index(index)
}

/// Session 4_12 "Log terminal output" (4_13 folder, `<>` = the global
/// one), else the global log folder.
fn log_folder(session: Option<&Session>, global: Option<&std::path::Path>) -> Option<PathBuf> {
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

fn shell_kind(shell: LocalShell) -> tc_pty::ShellKind {
    match shell {
        LocalShell::Bash => tc_pty::ShellKind::Bash,
        LocalShell::Zsh => tc_pty::ShellKind::Zsh,
        LocalShell::Cmd => tc_pty::ShellKind::Cmd,
        LocalShell::PowerShell => tc_pty::ShellKind::PowerShell,
        LocalShell::Wsl => tc_pty::ShellKind::Wsl,
    }
}

type StreamResult = Result<Box<dyn tc_net::Stream>, String>;

enum Backend {
    None,
    Pty(PtySession),
    Ssh(SshSession),
    /// Telnet / serial being opened in the background.
    Connecting(crossbeam_channel::Receiver<StreamResult>),
    Stream(Box<dyn tc_net::Stream>),
    /// FTP session: the browser is the UI, the terminal shows status.
    /// FTP / S3 file browser.
    Ftp(Box<dyn tc_net::FsBackend>),
}

/// State of the SSH-browser (SFTP side panel) for one tab.
#[derive(Default)]
pub struct SftpView {
    pub cwd: String,
    pub path_input: String,
    pub entries: Vec<RemoteEntry>,
    pub selected: BTreeSet<String>,
    pub show_hidden: bool,
    pub error: Option<String>,
    /// In-flight operations: id → label.
    pub ops: HashMap<u64, String>,
    /// Transfers: id → (label, bytes done, total).
    pub transfers: HashMap<u64, (String, u64, Option<u64>)>,
    pub prompt: Option<BrowserPrompt>,
    /// "Follow terminal folder": browse to directories the shell reports.
    pub follow: bool,
    /// The shell has reported its folder at least once (OSC 7).
    pub reports_seen: bool,
    /// The prompt hook is about to be typed automatically.
    pub auto_hook_pending: bool,
    /// "ASCII mode": convert line endings in transfers.
    pub ascii: bool,
    /// "Use SUDO for SSH-browser" is on / offered (SSH only).
    pub sudo: bool,
    pub sudo_supported: bool,
    /// Two-pane session view: downloads go straight to this local folder.
    pub local_target: Option<PathBuf>,
    listing: Option<u64>,
}

pub enum BrowserPrompt {
    /// "Permissions": mode bits being edited.
    Permissions {
        path: String,
        mode: u32,
    },
    /// "Properties" of an entry.
    Properties(RemoteEntry),
    /// "Use SUDO for SSH-browser" needs the sudo password (never saved).
    SudoPassword(zeroize::Zeroizing<String>),
    NewFolder(String),
    NewFile(String),
    Rename {
        from: String,
        to: String,
    },
    ConfirmDelete(Vec<(String, bool)>),
}

pub enum SftpStatus {
    Off,
    Connecting,
    Ready(Box<SftpView>),
    Unavailable(String),
}

/// A line typed into the terminal and handled locally (PuTTY-style
/// "login as:" and password prompts).
enum LineInput {
    Login(String),
    Secret(String, SecretKind, Target),
}

pub struct Tab {
    pub id: u64,
    pub title: String,
    /// Session this tab was opened from (`None` = default local shell).
    pub session: Option<Session>,
    pub term: Terminal,
    /// Font size from the session (px); `None` = global setting.
    pub font_size: Option<f32>,
    pub backspace_sends_ctrl_h: bool,
    pub paste_right_click: bool,
    ssh_settings: tc_core::settings::SshSettings,
    term_type: Option<String>,
    /// FTP connection waiting for its password prompt.
    pending_ftp: Option<tc_net::ftp::FtpParams>,
    /// S3 login waiting for its secret access key.
    pending_s3: Option<tc_net::s3::S3Params>,
    backend: Backend,
    pub status: Option<String>,
    /// SSH host key waiting for the user's decision.
    pub host_key: Option<Box<tc_ssh::HostKeyInfo>>,
    pub sftp: SftpStatus,
    /// Set once when the SSH-browser becomes ready (the app shows the Sftp tab).
    pub sftp_just_ready: bool,
    line: Option<LineInput>,
    known_hosts: PathBuf,
    repaint: Arc<dyn Fn() + Send + Sync>,
    credentials: Credentials,
    /// "Do you want to save this password?" questions waiting for an answer.
    pub offers: std::collections::VecDeque<(Target, String)>,
    /// Automatic "Follow terminal folder" for bash / zsh logins.
    folder_hook: Option<FolderHook>,
    /// A multi-line paste waiting for the user's OK.
    pub pending_paste: Option<String>,
    /// "Set tab color" (session 7_1 "Custom tab color" to start with).
    pub color: Option<[u8; 3]>,
    /// "Pin/unpin this tab": kept by the "Close all…" commands.
    pub pinned: bool,
    /// MultiExec: this terminal receives what is typed in the others.
    pub multiexec_on: bool,
    /// Output arrived while another tab was shown.
    pub activity: bool,
    /// "Detach tab": shown in its own window.
    pub detached: bool,
    /// The session stopped: keys choose what happens to the tab.
    ended: bool,
    /// What the user chose after the session stopped (the app applies it).
    pub ended_action: Option<EndedAction>,
    /// Lines of a paste still to send (4_40 "Paste delay").
    paste_queue: std::collections::VecDeque<Vec<u8>>,
    paste_delay: std::time::Duration,
    next_paste: Instant,
    /// "Expert settings" (reconnection keys, log flushing), from the
    /// configuration.
    pub expert: ExpertTerminal,
    /// "Visual" bell: flash the terminal until then.
    pub bell_flash: Option<Instant>,
    /// Bookmark 5_1 "Maximized/detached/fullscreen tab": open maximized
    /// when detached, or ask the app for fullscreen (taken once).
    pub detached_maximized: bool,
    pub wants_fullscreen: bool,
    /// Bookmark 4_15 "Lock terminal title" (default on): titles set by
    /// programs are ignored.
    lock_title: bool,
    /// Remote copies to open in the built-in text editor: (local, remote).
    pub open_in_editor: Vec<(PathBuf, String)>,
    /// Remote copies to compare in the Diff tool.
    pub open_in_diff: Vec<PathBuf>,
    /// Local side of the SFTP / FTP session view.
    pub local_pane: Option<crate::ui::local_pane::LocalPane>,
    /// 4_36 "Syntax highlighting" (`None` = the global setting).
    pub syntax: Option<usize>,
    /// Telnet / RDP / VNC waiting for its tunnel through the jump hosts.
    via_gateway: Option<ViaGateway>,
    /// That tunnel, kept open as long as the tab.
    gateway_tunnel: Option<SshSession>,
    /// 4_6 "Term charset": remote bytes ↔ the emulator's UTF-8.
    charset: tc_term::TermCharset,
    /// "Log terminal output".
    log: Option<crate::term_log::TermLog>,
    /// "Find in terminal".
    pub find: Option<FindBar>,
    /// FTPS certificate waiting for "trust it?".
    pub cert_prompt: Option<CertPrompt>,
    /// Parameters of the last FTP connection (to reconnect after trust).
    last_ftp: Option<tc_net::ftp::FtpParams>,
    /// Remote files opened in a local program, watched for changes.
    edits: crate::remote_edit::RemoteEdits,
    /// Edited files waiting for "upload to the server?".
    pub upload_prompts: std::collections::VecDeque<crate::remote_edit::Changed>,
    /// Latest "Remote monitoring" reading (`Some` while monitoring is on).
    pub monitor: Option<Box<tc_core::monitor::Sample>>,
    /// The user toggled "Remote monitoring" here; the app applies it to
    /// every SSH tab and saves the setting.
    pub monitoring_request: Option<bool>,
    /// Mosh: `mosh-server` being started over SSH.
    mosh: Option<MoshStart>,
    /// The SSH-browser runs through sudo.
    sftp_sudo: bool,
    /// Folder to return to once the browser has been reopened (sudo switch).
    sftp_reopen_cwd: Option<String>,
}

/// Prompt hook for bash and zsh that reports `$PWD` with OSC 7. Typed into
/// the shell after login when `$SHELL` is bash or zsh (see `folder_hook`),
/// or from the SSH-browser button; the leading space keeps it out of bash
/// history when `HISTCONTROL` ignores spaces.
pub const FOLDER_TRACKING_HOOK: &str = concat!(
    r#" if [ -n "$ZSH_VERSION" ]; then _tc_osc7() { printf '\033]7;file://%s%s\007' "$HOST" "$PWD"; };"#,
    r#" precmd_functions+=(_tc_osc7);"#,
    r#" elif [ -n "$BASH_VERSION" ]; then"#,
    r#" PROMPT_COMMAND='printf "\033]7;file://%s%s\007" "$HOSTNAME" "$PWD"'"${PROMPT_COMMAND:+;$PROMPT_COMMAND}"; fi"#,
    "\r",
);

const INITIAL_ROWS: u16 = 24;
const INITIAL_COLS: u16 = 80;

impl Tab {
    pub fn open(id: u64, session: Option<Session>, env: &TabEnv) -> Self {
        let title = session
            .as_ref()
            .map_or_else(|| "Local terminal".to_string(), |s| s.name.clone());
        let mut tab = Self {
            id,
            title,
            session,
            term: Terminal::new(INITIAL_ROWS, INITIAL_COLS, env.scrollback),
            font_size: None,
            backspace_sends_ctrl_h: false,
            paste_right_click: env.paste_right_click,
            ssh_settings: env.ssh.clone(),
            term_type: None,
            pending_ftp: None,
            pending_s3: None,
            backend: Backend::None,
            status: None,
            host_key: None,
            sftp: SftpStatus::Off,
            sftp_just_ready: false,
            line: None,
            known_hosts: env.known_hosts.clone(),
            repaint: env.repaint.clone(),
            credentials: Credentials::new(
                env.vault.clone(),
                env.save_passwords,
                env.save_key_passphrases,
            ),
            offers: Default::default(),
            folder_hook: None,
            pending_paste: None,
            color: None,
            pinned: false,
            multiexec_on: true,
            activity: false,
            detached: false,
            ended: false,
            ended_action: None,
            paste_queue: Default::default(),
            paste_delay: std::time::Duration::ZERO,
            charset: tc_term::TermCharset::default(),
            via_gateway: None,
            gateway_tunnel: None,
            syntax: None,
            local_pane: None,
            open_in_editor: Vec::new(),
            open_in_diff: Vec::new(),
            detached_maximized: false,
            wants_fullscreen: false,
            lock_title: true,
            expert: ExpertTerminal::default(),
            bell_flash: None,
            next_paste: Instant::now(),
            log: None,
            find: None,
            cert_prompt: None,
            last_ftp: None,
            edits: crate::remote_edit::RemoteEdits::new(id),
            upload_prompts: Default::default(),
            monitor: None,
            monitoring_request: None,
            mosh: None,
            sftp_sudo: false,
            sftp_reopen_cwd: None,
        };
        let settings = crate::term_settings::for_session(tab.session.as_ref());
        tab.term.theme = settings.theme;
        tab.font_size = settings.font_px;
        tab.backspace_sends_ctrl_h = if tab.session.is_some() {
            settings.backspace_sends_ctrl_h
        } else {
            env.backspace_ctrl_h
        };
        tab.term_type = settings.term_type;
        tab.paste_delay = paste_delay(tab.session.as_ref());
        tab.charset = term_charset(tab.session.as_ref());
        tab.syntax = session_syntax(tab.session.as_ref());
        let bookmark = |id: &str| {
            tab.session
                .as_ref()
                .and_then(|s| s.options.get(id))
                .cloned()
        };
        // 5_1: 0 normal, 1 detached, 2 maximized detached, 3 fullscreen.
        match bookmark("5_1").as_deref() {
            Some("1") => tab.detached = true,
            Some("2") => (tab.detached, tab.detached_maximized) = (true, true),
            Some("3") => tab.wants_fullscreen = true,
            _ => {}
        }
        tab.lock_title = bookmark("4_15").is_none_or(|v| schema::is_true(&v));
        tab.color = tab
            .session
            .as_ref()
            .and_then(|s| s.options.get("7_1"))
            .and_then(|v| delphi_color(v));
        if let Some(folder) = log_folder(tab.session.as_ref(), env.log_dir.as_deref()) {
            match crate::term_log::TermLog::create(&folder, &tab.title) {
                Ok(log) => tab.log = Some(log),
                Err(e) => tab.message(&format!(
                    "\x1b[33mcannot log to {}: {e}\x1b[0m",
                    folder.display()
                )),
            }
        }
        match tab.session.as_ref().map(|s| s.kind.clone()) {
            None => tab.spawn_local(None, env.local_shell, env),
            Some(SessionKind::LocalShell { program }) => {
                // 3_2 "Terminal shell": 0 = the configured default.
                let shell = tab
                    .session
                    .as_ref()
                    .and_then(|s| s.options.get("3_2"))
                    .and_then(|v| LocalShell::from_option(v))
                    .unwrap_or(env.local_shell);
                tab.spawn_local(program.as_deref(), shell, env)
            }
            Some(SessionKind::Ssh {
                user: Some(user), ..
            }) => tab.connect_ssh(user),
            Some(SessionKind::Ssh { user: None, .. }) => {
                tab.term.feed(b"login as: ");
                tab.line = Some(LineInput::Login(String::new()));
            }
            Some(
                SessionKind::Telnet { .. } | SessionKind::Rdp { .. } | SessionKind::Vnc { .. },
            ) if let Some(via) = via_gateway(tab.session.as_ref()) => {
                tab.start_via_gateway(via, None);
            }
            Some(SessionKind::Telnet { host, port }) => tab.open_telnet(host, port),
            Some(SessionKind::Serial { port_name, baud }) => tab.open_serial(port_name, baud),
            Some(SessionKind::Ftp { host, port, user }) => tab.open_ftp(host, port, user),
            Some(SessionKind::Rdp { host, port, user }) => {
                let temp = std::env::temp_dir().join("termcraft-rdp");
                let result = crate::external::launch_rdp(&host, port, user.as_deref(), &temp);
                tab.external_result("RDP", &host, port, result);
            }
            Some(SessionKind::Vnc { host, port }) => {
                let result = crate::external::launch_vnc(&host, port);
                tab.external_result("VNC", &host, port, result);
            }
            Some(SessionKind::Rsh { host, user }) => tab.open_rlogin(host, user),
            Some(SessionKind::Sftp {
                user: Some(user), ..
            }) => tab.connect_sftp(user),
            Some(SessionKind::Sftp { user: None, .. }) => {
                tab.term.feed(b"login as: ");
                tab.line = Some(LineInput::Login(String::new()));
            }
            Some(SessionKind::Mosh {
                user: Some(user), ..
            }) => tab.connect_mosh(user),
            Some(SessionKind::Mosh { user: None, .. }) => {
                tab.term.feed(b"login as: ");
                tab.line = Some(LineInput::Login(String::new()));
            }
            Some(SessionKind::Wsl { distro }) => tab.open_wsl(&distro),
            Some(SessionKind::AwsS3 { key_id }) => tab.open_s3(key_id),
            Some(SessionKind::Xdmcp { host }) => tab.open_xdmcp(&host),
            Some(SessionKind::File { path }) => {
                let result = crate::external::open_path(&path);
                tab.opened_externally("File", &path, result);
            }
            Some(SessionKind::Browser { url }) => {
                let result = crate::external::open_url(&url);
                tab.opened_externally("Browser", &url, result);
            }
        }
        tab
    }

    fn external_result(
        &mut self,
        kind: &str,
        host: &str,
        port: u16,
        result: Result<String, String>,
    ) {
        match result {
            Ok(client) => {
                self.message(&format!(
                    "\x1b[32m{kind} session to {host}:{port} opened in {client}.\x1b[0m"
                ));
                self.message("\x1b[90mAn embedded viewer is planned (see docs/PARITY.md).\x1b[0m");
                self.status = Some(format!("{kind} opened externally"));
            }
            Err(e) => {
                self.message(&format!("\x1b[31m{kind}: {e}\x1b[0m"));
                self.status = Some(format!("{kind} unavailable"));
            }
        }
    }

    /// "File" / "Browser" sessions: the target opened in another program.
    fn opened_externally(&mut self, kind: &str, target: &str, result: Result<String, String>) {
        match result {
            Ok(app) => {
                self.message(&format!("\x1b[32m{target} opened in {app}.\x1b[0m"));
                self.status = Some(format!("{kind} opened externally"));
            }
            Err(e) => {
                self.message(&format!("\x1b[31m{kind}: {e}\x1b[0m"));
                self.status = Some(format!("{kind} unavailable"));
            }
        }
    }

    /// Mosh: start `mosh-server` over SSH (3_4 SSH port, 3_9 key, 3_5 UDP
    /// port, 3_6 charset, 3_7 prediction, 3_8 server path).
    fn connect_mosh(&mut self, user: String) {
        let Some(Session {
            kind: SessionKind::Mosh { host, ssh_port, .. },
            options,
            ..
        }) = &self.session
        else {
            return;
        };
        let (host, ssh_port) = (host.clone(), *ssh_port);
        let opt = |id: &str| options.get(id).cloned().unwrap_or_default();
        let locale = schema::type_params(12)
            .iter()
            .find(|p| p.id == "3_6")
            .and_then(|p| match p.widget {
                schema::Widget::Choice(items) => {
                    items.get(opt("3_6").parse::<usize>().unwrap_or(0)).copied()
                }
                _ => None,
            })
            .unwrap_or("auto");
        let server = crate::mosh::server_command(&opt("3_8"), &opt("3_5"), locale);
        let key = Some(opt("3_9"))
            .filter(|k| !k.trim().is_empty())
            .map(PathBuf::from);
        let prediction = crate::mosh::prediction(&opt("3_7"));
        let Some(client) = crate::mosh::find_client() else {
            self.message(
                "\x1b[31mmosh-client was not found in PATH. Install Mosh (https://mosh.org) \
                 to use Mosh sessions.\x1b[0m",
            );
            self.status = Some("Mosh unavailable".into());
            return;
        };
        let server = match server {
            Ok(cmd) => cmd,
            Err(e) => {
                self.message(&format!("\x1b[31mMosh: {e}\x1b[0m"));
                return;
            }
        };
        use std::net::ToSocketAddrs;
        let Some(ip) = (host.as_str(), ssh_port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .map(|a| a.ip())
        else {
            self.message(&format!("\x1b[31mMosh: cannot resolve {host}\x1b[0m"));
            return;
        };
        self.message(&format!(
            "\x1b[90mStarting mosh-server on {host} over SSH …\x1b[0m"
        ));
        let (rows, cols) = self.term.size();
        self.credentials.reset();
        let (user, credential_error) = self.credentials.use_named(&user, &host, ssh_port);
        if let Some(e) = credential_error {
            self.message(&format!("\x1b[33m{e}\x1b[0m"));
        }
        let params = ConnectParams {
            host,
            port: ssh_port,
            user,
            private_key: key,
            gateways: Vec::new(),
            known_hosts: self.known_hosts.clone(),
            compression: false,
            command: Some(server),
            term: "xterm-256color".into(),
            cols,
            rows,
            sftp: false,
            browser_protocol: Default::default(),
            keepalive: None,
            detect_login_shell: false,
            remote_monitoring: false,
            agent: agent_config(&self.ssh_settings),
            shell: true,
            proxy: None,
            agent_forwarding: false,
            x11_display: None,
        };
        self.mosh = Some(MoshStart {
            output: Vec::new(),
            ip,
            client,
            prediction,
        });
        self.backend = Backend::Ssh(SshSession::connect(params, self.repaint.clone()));
    }

    /// Mosh: collect the server's answer; once it has the session key,
    /// leave SSH and run `mosh-client`.
    fn mosh_output(&mut self, bytes: &[u8]) {
        let Some(m) = &mut self.mosh else {
            return;
        };
        m.output.extend_from_slice(bytes);
        if m.output.len() > 64 * 1024 {
            m.output.drain(..32 * 1024);
        }
        if let Some(connect) = crate::mosh::parse_connect(&m.output) {
            let m = self.mosh.take().expect("checked");
            let command = crate::mosh::client_command(&m.client, m.ip, &connect, m.prediction);
            self.message(&format!(
                "\x1b[90mmosh-server is listening on UDP {}; starting mosh-client …\x1b[0m",
                connect.port
            ));
            // mosh-server keeps running on its own; SSH is no longer needed.
            self.backend = Backend::None;
            self.spawn_command(&command);
        }
    }

    /// SFTP session: SSH with only the file browser (no terminal); the
    /// tab shows the connection status.
    fn connect_sftp(&mut self, user: String) {
        let Some(Session {
            kind: SessionKind::Sftp { host, port, .. },
            ..
        }) = &self.session
        else {
            return;
        };
        let (host, port) = (host.clone(), *port);
        self.credentials.reset();
        let (user, credential_error) = self.credentials.use_named(&user, &host, port);
        if let Some(e) = credential_error {
            self.message(&format!("\x1b[33m{e}\x1b[0m"));
        }
        self.message(&format!(
            "\x1b[90mConnecting to {user}@{host}:{port} (SFTP) …\x1b[0m"
        ));
        let (rows, cols) = self.term.size();
        let params = ConnectParams {
            host,
            port,
            user,
            private_key: None,
            gateways: Vec::new(),
            known_hosts: self.known_hosts.clone(),
            compression: false,
            command: None,
            term: "xterm-256color".into(),
            cols,
            rows,
            sftp: true,
            browser_protocol: Default::default(),
            keepalive: (self.ssh_settings.keepalive_secs > 0)
                .then(|| std::time::Duration::from_secs(self.ssh_settings.keepalive_secs)),
            detect_login_shell: false,
            remote_monitoring: false,
            agent: agent_config(&self.ssh_settings),
            shell: false,
            proxy: None,
            agent_forwarding: false,
            x11_display: None,
        };
        self.sftp = SftpStatus::Connecting;
        self.backend = Backend::Ssh(SshSession::connect(params, self.repaint.clone()));
    }

    /// "Rsh" session over rlogin (TCP 513).
    fn open_rlogin(&mut self, host: String, user: Option<String>) {
        self.message(&format!(
            "\x1b[90mConnecting to {host} (rlogin, not encrypted) …\x1b[0m"
        ));
        let (rows, cols) = self.term.size();
        let term = self.term_type.clone().unwrap_or_else(|| "xterm".into());
        let repaint = self.repaint.clone();
        self.open_in_background(move || {
            tc_net::rlogin::Rlogin::connect(
                &host,
                tc_net::rlogin::DEFAULT_PORT,
                user.as_deref().unwrap_or(""),
                &term,
                rows,
                cols,
                repaint,
            )
            .map(|r| Box::new(r) as Box<dyn tc_net::Stream>)
            .map_err(|e| format!("cannot connect to {host}: {e}"))
        });
    }

    /// WSL session: 3_6 user, 3_3 startup directory.
    fn open_wsl(&mut self, distro: &str) {
        let opt = |id: &str| {
            self.session
                .as_ref()
                .and_then(|s| s.options.get(id))
                .cloned()
        };
        let (user, dir) = (opt("3_6"), opt("3_3"));
        match tc_pty::shells::wsl_command(distro, user.as_deref(), dir.as_deref()) {
            Some(command) => self.spawn_command(&command),
            None => {
                self.message("\x1b[31mWSL is not installed (wsl.exe not found).\x1b[0m");
                self.status = Some("WSL unavailable".into());
            }
        }
    }

    /// Output from the program: decoded from the session charset,
    /// logged, then shown.
    fn output(&mut self, bytes: &[u8]) -> Vec<u8> {
        let decoded = self.charset.decode(bytes);
        let bytes: &[u8] = &decoded;
        let flush = self.expert.flush_logs;
        if let Some(log) = &mut self.log
            && let Err(e) = log
                .write(bytes)
                .and_then(|()| if flush { log.flush() } else { Ok(()) })
        {
            let path = log.path().display().to_string();
            self.log = None;
            self.message(&format!("\x1b[33mlogging to {path} stopped: {e}\x1b[0m"));
        }
        self.term.feed(bytes)
    }

    /// Paste text: line endings as Enter, bracketed when the program asks,
    /// line by line when the session has a paste delay.
    pub fn paste(&mut self, text: &str) {
        let bytes = crate::term_view::paste_bytes(self, text);
        if self.paste_delay.is_zero() {
            self.send(&bytes);
            return;
        }
        for line in bytes.split_inclusive(|&b| b == b'\r') {
            self.paste_queue.push_back(line.to_vec());
        }
        self.next_paste = Instant::now();
        self.pump_paste();
    }

    fn pump_paste(&mut self) {
        while !self.paste_queue.is_empty() && Instant::now() >= self.next_paste {
            let line = self.paste_queue.pop_front().expect("checked");
            self.send(&line);
            self.next_paste = Instant::now() + self.paste_delay;
        }
    }

    /// The program or connection ended: say so and offer the next steps,
    /// like the reference. Local terminals without a session just stop.
    fn session_ended(&mut self, detail: &str) {
        self.message(&format!("\r\n\x1b[90m[{detail}]\x1b[0m"));
        self.message("\x1b[1mSession stopped\x1b[0m");
        let keys = &self.expert;
        let lines = [
            format!("    - Press {} to exit tab", key_label(&keys.exit_key)),
            format!(
                "    - Press {} to restart session",
                key_label(&keys.reconnect_key)
            ),
            format!(
                "    - Press {} to save terminal output to file",
                key_label(&keys.save_key)
            ),
        ];
        for line in lines {
            self.message(&line);
        }
        self.ended = true;
        // 1_1 "Reconnection message at session end" off: just close.
        let message = self
            .session
            .as_ref()
            .and_then(|s| s.options.get("1_1"))
            .is_none_or(|v| schema::is_true(v));
        if !message {
            self.ended_action = Some(EndedAction::Close);
        }
    }

    fn message(&mut self, text: &str) {
        self.term.feed(format!("{text}\r\n").as_bytes());
    }

    fn spawn_local(&mut self, program: Option<&str>, shell: LocalShell, env: &TabEnv) {
        let windows_path = env.use_windows_path;
        let command = match program {
            Some(p) => tc_pty::ShellCommand {
                program: p.to_string(),
                args: Vec::new(),
                env: Vec::new(),
            },
            None => match tc_pty::shells::locate(shell_kind(shell)) {
                Some(c) => c,
                None => {
                    self.message(&format!(
                        "\x1b[33m{} is not installed on this machine; using the system shell.\x1b[0m",
                        shell.label()
                    ));
                    if shell == LocalShell::Bash && cfg!(windows) {
                        self.message(
                            "\x1b[90mInstall Git for Windows or MSYS2 to get Bash.\x1b[0m",
                        );
                    }
                    tc_pty::ShellCommand {
                        program: tc_pty::default_shell(),
                        args: Vec::new(),
                        env: Vec::new(),
                    }
                }
            },
        };
        let mut command = command;
        // Git for Windows and MSYS2 read MSYS2_PATH_TYPE in /etc/profile.
        if cfg!(windows) && program.is_none() && matches!(shell, LocalShell::Bash | LocalShell::Zsh)
        {
            let kind = if windows_path { "inherit" } else { "minimal" };
            command.env.push(("MSYS2_PATH_TYPE".into(), kind.into()));
        }
        // "Home directory location" (Bash / Zsh read HOME).
        if program.is_none()
            && matches!(shell, LocalShell::Bash | LocalShell::Zsh)
            && let Some(home) = &env.home
        {
            command
                .env
                .push(("HOME".into(), home.display().to_string()));
        }
        self.spawn_command(&command);
    }

    fn spawn_command(&mut self, command: &tc_pty::ShellCommand) {
        match PtySession::spawn_command(command, INITIAL_ROWS, INITIAL_COLS, self.repaint.clone()) {
            Ok(pty) => self.backend = Backend::Pty(pty),
            Err(e) => {
                self.message(&format!("\x1b[31mfailed to start: {e:#}\x1b[0m"));
                self.status = Some("failed to start".into());
            }
        }
    }

    fn connect_ssh(&mut self, user: String) {
        let Some(Session {
            kind:
                SessionKind::Ssh {
                    host,
                    port,
                    private_key,
                    x11_forwarding,
                    ..
                },
            options,
            ..
        }) = &self.session
        else {
            return;
        };
        // Schema ids: 3_7 Compression, 3_8 Execute command.
        let opt = |id: &str| options.get(id).map(String::as_str);
        let (rows, cols) = self.term.size();
        self.credentials.reset();
        let (user, credential_error) = self.credentials.use_named(&user, host, *port);
        // Network settings: the SSH jump hosts (3_9 host, 3_10 port,
        // 3_11 login, 3_16 key), one or several.
        let gateways = tc_core::jump_hosts::parse(options, tc_core::jump_hosts::SSH)
            .into_iter()
            .map(|hop| tc_ssh::Endpoint {
                port: hop.port_number(),
                user: if hop.user.is_empty() {
                    user.clone()
                } else {
                    hop.user
                },
                private_key: (!hop.key.is_empty()).then(|| PathBuf::from(hop.key)),
                host: hop.host,
            })
            .collect();
        let x11 = if *x11_forwarding {
            x11_display()
        } else {
            (None, None)
        };
        let mut params = ConnectParams {
            host: host.clone(),
            port: *port,
            user,
            private_key: private_key.as_ref().map(PathBuf::from),
            gateways,
            known_hosts: self.known_hosts.clone(),
            compression: opt("3_7").is_some_and(schema::is_true),
            command: opt("3_8")
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(|c| keep_open(c, opt("3_12").is_some_and(schema::is_true))),
            term: self
                .term_type
                .clone()
                .unwrap_or_else(|| "xterm-256color".into()),
            cols,
            rows,
            // 3_26 SSH-browser type: 0 = none.
            sftp: self.ssh_settings.browser && opt("3_26") != Some("0"),
            // 3_26: 2 "SCP (enhanced speed)", 3 "SCP (normal speed)".
            browser_protocol: match opt("3_26") {
                Some("2") => tc_ssh::BrowserProtocol::Scp { enhanced: true },
                Some("3") => tc_ssh::BrowserProtocol::Scp { enhanced: false },
                _ => tc_ssh::BrowserProtocol::Sftp,
            },
            keepalive: (self.ssh_settings.keepalive_secs > 0)
                .then(|| std::time::Duration::from_secs(self.ssh_settings.keepalive_secs)),
            detect_login_shell: false,
            remote_monitoring: self.ssh_settings.remote_monitoring,
            // 3_34 "Attempt SSH agent authentication" (default on).
            agent: if opt("3_34").is_none_or(schema::is_true) {
                agent_config(&self.ssh_settings)
            } else {
                tc_ssh::AgentConfig::default()
            },
            shell: true,
            proxy: tc_ssh::proxy::from_options(&|id| opt(id).map(String::from)),
            agent_forwarding: opt("3_35").is_some_and(schema::is_true),
            x11_display: x11.0,
        };
        if params.remote_monitoring {
            self.monitor = Some(Box::default());
        }
        // 3_18 "Try to follow SSH path in browser" (default on).
        params.detect_login_shell =
            params.sftp && params.command.is_none() && opt("3_18").is_none_or(schema::is_true);
        self.folder_hook = params.detect_login_shell.then(FolderHook::new);
        if let Some(e) = credential_error {
            self.message(&format!("\x1b[33m{e}\x1b[0m"));
        }
        if params.sftp {
            self.sftp = SftpStatus::Connecting;
        }
        self.backend = Backend::Ssh(SshSession::connect(params, self.repaint.clone()));
    }

    /// Drain backend events into the emulator. Returns true if anything changed.
    pub fn pump(&mut self) -> bool {
        self.pump_paste();
        if let Some(title) = self.term.take_title()
            && !self.lock_title
            && !title.is_empty()
        {
            self.title = title;
        }
        self.upload_prompts.extend(self.edits.changed());
        match &mut self.backend {
            Backend::None => false,
            Backend::Pty(_) => self.pump_pty(),
            Backend::Ssh(_) => self.pump_ssh(),
            Backend::Connecting(_) | Backend::Stream(_) => self.pump_stream(),
            Backend::Ftp(_) => self.pump_ftp(),
        }
    }

    /// Open a connection off the UI thread (TCP connect can take seconds).
    fn open_in_background(&mut self, open: impl FnOnce() -> StreamResult + Send + 'static) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open());
            repaint();
        });
        self.backend = Backend::Connecting(rx);
    }

    /// Open the local tunnel to the end server through the jump hosts.
    /// Hops without a login ask for one first ("login as:").
    fn start_via_gateway(&mut self, via: ViaGateway, login: Option<String>) {
        let needs_login = via.hops.iter().any(|h| h.user.trim().is_empty());
        let login = match login {
            Some(l) => l,
            None if needs_login => {
                self.message("\x1b[90mLogin for the SSH gateway:\x1b[0m");
                self.term.feed(b"login as: ");
                self.line = Some(LineInput::Login(String::new()));
                self.via_gateway = Some(via);
                return;
            }
            None => String::new(),
        };
        let mut endpoints: Vec<tc_ssh::Endpoint> = via
            .hops
            .iter()
            .map(|h| tc_ssh::Endpoint {
                host: h.host.clone(),
                port: h.port_number(),
                user: if h.user.trim().is_empty() {
                    login.clone()
                } else {
                    h.user.trim().to_string()
                },
                private_key: (!h.key.is_empty()).then(|| PathBuf::from(&h.key)),
            })
            .collect();
        let Some(last) = endpoints.pop() else {
            return;
        };
        self.message(&format!(
            "\x1b[90mOpening a tunnel to {}:{} through {} …\x1b[0m",
            via.host,
            via.port,
            tc_core::jump_hosts::summary(&via.hops)
        ));
        let params = tc_ssh::tunnel::TunnelParams {
            ssh: last,
            gateways: endpoints,
            known_hosts: self.known_hosts.clone(),
            spec: tc_ssh::tunnel::TunnelSpec::Local {
                bind: "127.0.0.1".into(),
                port: 0,
                dest_host: via.host.clone(),
                dest_port: via.port,
            },
            agent: agent_config(&self.ssh_settings),
        };
        self.credentials.reset();
        self.via_gateway = Some(via);
        self.backend = Backend::Ssh(SshSession::tunnel(params, self.repaint.clone()));
    }

    /// The tunnel listens: keep it, and start the session through it.
    fn gateway_ready(&mut self, local_port: u16) {
        let Some(via) = self.via_gateway.take() else {
            return;
        };
        if let Backend::Ssh(tunnel) = std::mem::replace(&mut self.backend, Backend::None) {
            self.gateway_tunnel = Some(tunnel);
        }
        let local = "127.0.0.1";
        let external = !matches!(via.target, GatewayTarget::Telnet);
        match via.target {
            GatewayTarget::Telnet => self.open_telnet(local.into(), local_port),
            GatewayTarget::Rdp(user) => {
                let temp = std::env::temp_dir().join("termcraft-rdp");
                let result = crate::external::launch_rdp(local, local_port, user.as_deref(), &temp);
                self.external_result("RDP", &via.host, via.port, result);
            }
            GatewayTarget::Vnc => {
                let result = crate::external::launch_vnc(local, local_port);
                self.external_result("VNC", &via.host, via.port, result);
            }
        }
        if external {
            self.message(&format!(
                "\x1b[90mThe tunnel stays open on {local}:{local_port} while this tab is \
                 open.\x1b[0m"
            ));
        }
    }

    fn open_telnet(&mut self, host: String, port: u16) {
        self.message(&format!(
            "\x1b[90mConnecting to {host}:{port} (Telnet) …\x1b[0m"
        ));
        let (rows, cols) = self.term.size();
        let term = self.term_type.clone().unwrap_or_else(|| "xterm".into());
        let repaint = self.repaint.clone();
        self.open_in_background(move || {
            tc_net::telnet::Telnet::connect(&host, port, &term, rows, cols, repaint)
                .map(|t| Box::new(t) as Box<dyn tc_net::Stream>)
                .map_err(|e| format!("{host}:{port}: {e}"))
        });
    }

    fn open_serial(&mut self, port: String, baud: u32) {
        // Advanced Serial settings: 3_4 data bits, 3_5 stop bits, 3_6 parity, 3_7 flow.
        let session = self.session.clone();
        let idx = |id: &str, default: u8| -> u8 {
            let param = tc_core::schema::type_params(8).iter().find(|p| p.id == id);
            match (session.as_ref(), param) {
                (Some(s), Some(p)) => s.option(p).parse().unwrap_or(default),
                _ => default,
            }
        };
        let config = tc_net::serial::SerialConfig::from_indices(
            &port,
            baud,
            idx("3_4", 3),
            idx("3_5", 0),
            idx("3_6", 0),
            idx("3_7", 1),
        );
        self.message(&format!("\x1b[90mOpening {port} at {baud} baud …\x1b[0m"));
        let repaint = self.repaint.clone();
        self.open_in_background(move || {
            let config = config?;
            tc_net::serial::Serial::open(&config, repaint)
                .map(|s| Box::new(s) as Box<dyn tc_net::Stream>)
        });
    }

    fn open_ftp(&mut self, host: String, port: u16, user: Option<String>) {
        // Advanced Ftp settings: 3_5 passive, 3_9 ASCII, 3_6 remote startup folder.
        let session = self.session.clone();
        let opt = |id: &str| -> Option<String> {
            let param = tc_core::schema::type_params(6)
                .iter()
                .find(|p| p.id == id)?;
            Some(session.as_ref()?.option(param).to_string())
        };
        let params = tc_net::ftp::FtpParams {
            host: host.clone(),
            port,
            user: user.clone().unwrap_or_default(),
            password: String::new(),
            passive: opt("3_5").is_none_or(|v| schema::is_true(&v)),
            ascii: opt("3_9").is_some_and(|v| schema::is_true(&v)),
            start_dir: opt("3_6").filter(|d| !d.trim().is_empty()),
            // 3_7 FTPS, 3_20 implicit, 3_19 TLS 1.2, 3_22 TLS 1.3.
            tls: opt("3_7")
                .is_some_and(|v| schema::is_true(&v))
                .then(|| tc_net::ftp::FtpsParams {
                    implicit: opt("3_20").is_some_and(|v| schema::is_true(&v)),
                    versions: tc_net::ftps::TlsVersions {
                        tls12: opt("3_19").is_none_or(|v| schema::is_true(&v)),
                        tls13: opt("3_22").is_some_and(|v| schema::is_true(&v)),
                    },
                    pinned: tc_net::ftps::trusted(&self.ftps_certificates(), &host, port),
                }),
        };
        match user.filter(|u| !u.is_empty()) {
            // Anonymous login needs no password.
            None => self.start_ftp(params),
            Some(user) => {
                self.pending_ftp = Some(params);
                let target = Target {
                    protocol: "ftp".into(),
                    username: user.clone(),
                    server: format!("{host}:{port}"),
                };
                let prompt = format!("{user}@{host}'s FTP password:");
                self.on_secret_needed(prompt, SecretKind::Password, target);
            }
        }
    }

    /// A password (typed or saved) for a pending FTP / S3 login; given back
    /// when no such login waits (it is then for the SSH connection).
    fn pending_login(&mut self, secret: String) -> Option<String> {
        if let Some(mut ftp) = self.pending_ftp.take() {
            ftp.password = secret;
            self.start_ftp(ftp);
            return None;
        }
        if let Some(mut s3) = self.pending_s3.take() {
            s3.secret = zeroize::Zeroizing::new(secret);
            self.start_s3(s3);
            return None;
        }
        Some(secret)
    }

    /// XDMCP through an X server (3_5 "Specify server to connect to" with
    /// 3_2, else broadcast; 3_3 sender IP, 3_12 screen, 3_14 / 3_15 size).
    fn open_xdmcp(&mut self, host: &str) {
        let opt = |id: &str| {
            self.session
                .as_ref()
                .and_then(|s| s.options.get(id))
                .cloned()
        };
        let query = opt("3_5").is_some_and(|v| schema::is_true(&v)) || !host.trim().is_empty();
        let num = |id: &str, default: u32| {
            opt(id)
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(default)
        };
        let from = opt("3_3");
        let request = crate::external::Xdmcp {
            host: (query && !host.trim().is_empty()).then_some(host.trim()),
            from: from.as_deref(),
            display: u16::try_from(num("3_12", 1)).unwrap_or(1),
            size: (num("3_14", 1024), num("3_15", 768)),
        };
        let shown = request.host.unwrap_or("any server (broadcast)").to_string();
        let result = crate::external::launch_xdmcp(&request);
        match result {
            Ok(server) => {
                self.message(&format!(
                    "\x1b[32mXDMCP session to {shown} opened in {server} (display :{}).\x1b[0m",
                    request.display
                ));
                self.status = Some("XDMCP opened externally".into());
            }
            Err(e) => {
                self.message(&format!("\x1b[31mXDMCP: {e}\x1b[0m"));
                self.status = Some("XDMCP unavailable".into());
            }
        }
    }

    /// "Aws S3": the secret access key is asked for (or taken from the
    /// password store), then the buckets are listed.
    fn open_s3(&mut self, key_id: String) {
        let key_id = key_id.trim().to_string();
        if key_id.is_empty() {
            self.message("\x1b[31mAws S3: no Key ID in this session\x1b[0m");
            self.status = Some("no Key ID".into());
            return;
        }
        self.pending_s3 = Some(tc_net::s3::S3Params {
            key_id: key_id.clone(),
            secret: zeroize::Zeroizing::new(String::new()),
            region: "us-east-1".into(),
            start_path: None,
            endpoint: None,
        });
        let target = Target {
            protocol: "s3".into(),
            username: key_id.clone(),
            server: "s3.amazonaws.com".into(),
        };
        self.on_secret_needed(
            format!("Secret access key for {key_id}:"),
            SecretKind::Password,
            target,
        );
    }

    fn start_s3(&mut self, params: tc_net::s3::S3Params) {
        self.message("\x1b[90mConnecting to Amazon S3 …\x1b[0m");
        self.sftp = SftpStatus::Connecting;
        self.backend = Backend::Ftp(Box::new(tc_net::s3::S3Session::connect(
            params,
            self.repaint.clone(),
        )));
    }

    /// FTPS certificates the user trusted (next to known_hosts).
    fn ftps_certificates(&self) -> PathBuf {
        self.known_hosts.with_file_name("ftps_certificates")
    }

    /// FTPS certificate question answered: reconnect trusting it (and
    /// remember it unless "Connect once").
    pub fn answer_certificate(&mut self, accept: bool, remember: bool) {
        let Some(prompt) = self.cert_prompt.take() else {
            return;
        };
        let Some(mut params) = self.last_ftp.take() else {
            return;
        };
        if !accept {
            self.message("\x1b[90m[connection cancelled]\x1b[0m");
            return;
        }
        if remember
            && let Err(e) = tc_net::ftps::trust(
                &self.ftps_certificates(),
                &params.host,
                params.port,
                &prompt.fingerprint,
            )
        {
            self.message(&format!(
                "\x1b[33mcould not save the certificate: {e}\x1b[0m"
            ));
        }
        if let Some(tls) = &mut params.tls {
            tls.pinned = Some(prompt.fingerprint);
        }
        self.start_ftp(params);
    }

    fn start_ftp(&mut self, params: tc_net::ftp::FtpParams) {
        self.message(&format!(
            "\x1b[90mConnecting to {}:{} ({}) …\x1b[0m",
            params.host,
            params.port,
            if params.tls.is_some() { "FTPS" } else { "FTP" }
        ));
        self.last_ftp = Some(params.clone());
        self.sftp = SftpStatus::Connecting;
        self.backend = Backend::Ftp(Box::new(tc_net::ftp::FtpSession::connect(
            params,
            self.repaint.clone(),
        )));
    }

    fn pump_ftp(&mut self) -> bool {
        let Backend::Ftp(ftp) = &self.backend else {
            return false;
        };
        let events: Vec<SftpEvent> = ftp.events().try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            match &event {
                SftpEvent::Ready { home } => {
                    self.message(&format!(
                        "\x1b[32mConnected ({home}). Drag files between the local and remote panes to transfer them.\x1b[0m"
                    ));
                    self.status = None;
                    self.after_login();
                }
                SftpEvent::Unavailable(e) => {
                    let kind = self.session.as_ref().map_or("FTP", |s| s.kind.label());
                    self.message(&format!("\x1b[31m{kind}: {e}\x1b[0m"));
                    self.status = Some("connection failed".into());
                }
                SftpEvent::UntrustedCertificate {
                    fingerprint,
                    reason,
                } => {
                    self.message(&format!(
                        "\x1b[33mThe server certificate is not trusted: {reason}\x1b[0m"
                    ));
                    self.cert_prompt = Some(CertPrompt {
                        fingerprint: fingerprint.clone(),
                        reason: reason.clone(),
                    });
                }
                SftpEvent::Failed { error, .. } => self.message(&format!("\x1b[33m{error}\x1b[0m")),
                _ => {}
            }
            let unavailable = matches!(event, SftpEvent::Unavailable(_));
            self.on_sftp(event);
            if unavailable {
                self.backend = Backend::None;
            }
        }
        changed
    }

    /// Send a browser request over SFTP or FTP.
    fn fs_request(&self, request: SftpRequest) -> Option<u64> {
        match &self.backend {
            Backend::Ssh(ssh) => Some(ssh.sftp(request)),
            Backend::Ftp(ftp) => Some(ftp.request(request)),
            _ => None,
        }
    }

    fn pump_stream(&mut self) -> bool {
        if let Backend::Connecting(rx) = &self.backend {
            match rx.try_recv() {
                Ok(Ok(stream)) => {
                    self.backend = Backend::Stream(stream);
                    self.status = None;
                }
                Ok(Err(e)) => {
                    self.backend = Backend::None;
                    self.status = Some("connection failed".into());
                    self.message(&format!("\x1b[31m{e}\x1b[0m"));
                    return true;
                }
                Err(_) => return false,
            }
        }
        let Backend::Stream(stream) = &mut self.backend else {
            return false;
        };
        let events: Vec<tc_net::StreamEvent> = stream.events().try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            match event {
                tc_net::StreamEvent::Output(bytes) => {
                    let reply = self.output(&bytes);
                    if !reply.is_empty()
                        && let Backend::Stream(stream) = &mut self.backend
                    {
                        let _ = stream.write(&reply);
                    }
                }
                tc_net::StreamEvent::Closed(reason) => {
                    self.backend = Backend::None;
                    self.status = Some("connection closed".into());
                    let why = reason.map(|r| format!(": {r}")).unwrap_or_default();
                    self.session_ended(&format!("connection closed{why}"));
                    break;
                }
            }
        }
        changed
    }

    fn pump_pty(&mut self) -> bool {
        let Backend::Pty(pty) = &self.backend else {
            return false;
        };
        let events: Vec<PtyEvent> = pty.events().try_iter().collect();
        let changed = !events.is_empty();
        let mut exited = false;
        for event in events {
            match event {
                PtyEvent::Output(bytes) => {
                    let reply = self.output(&bytes);
                    if !reply.is_empty()
                        && let Backend::Pty(pty) = &mut self.backend
                    {
                        let _ = pty.write(&reply);
                    }
                }
                PtyEvent::Exited => exited = true,
            }
        }
        if exited {
            self.backend = Backend::None;
            self.status = Some("process exited".into());
            self.session_ended("process exited");
        }
        changed
    }

    fn pump_ssh(&mut self) -> bool {
        let Backend::Ssh(ssh) = &self.backend else {
            return false;
        };
        let events: Vec<SshEvent> = ssh.events().try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            if !matches!(self.backend, Backend::Ssh(_)) {
                break; // Mosh switched to mosh-client
            }
            match event {
                SshEvent::Output(bytes) if self.mosh.is_some() => {
                    self.mosh_output(&bytes);
                }
                SshEvent::Output(bytes) => {
                    let bytes = match &mut self.folder_hook {
                        Some(hook) => hook.filter(&bytes, Instant::now()),
                        None => bytes,
                    };
                    let reply = self.output(&bytes);
                    if !reply.is_empty()
                        && let Backend::Ssh(ssh) = &self.backend
                    {
                        ssh.write(&reply);
                    }
                    if let Some(dir) = self.term.take_cwd_report() {
                        self.on_cwd_report(dir);
                    }
                }
                SshEvent::Info(text) => self.message(&format!("\x1b[90m{text}\x1b[0m")),
                SshEvent::HostKey(info) => self.host_key = Some(info),
                SshEvent::SecretNeeded {
                    prompt,
                    kind,
                    user,
                    host,
                    port,
                    key_path,
                } => {
                    let target = target_for(kind, &user, &host, port, key_path.as_deref());
                    self.on_secret_needed(prompt, kind, target)
                }
                SshEvent::Connected => {
                    self.status = None;
                    self.after_login();
                }
                SshEvent::Listening(message) if self.via_gateway.is_some() => {
                    match tc_ssh::tunnel::listening_port(&message) {
                        Some(port) => self.gateway_ready(port),
                        None => self.message(&format!("\x1b[31mtunnel: {message}\x1b[0m")),
                    }
                }
                SshEvent::Error(e) => self.message(&format!("\x1b[31m{e}\x1b[0m")),
                SshEvent::Sftp(e) => self.on_sftp(e),
                SshEvent::Listening(_) => {}
                SshEvent::Monitor(sample) => {
                    if self.monitor.is_some() {
                        self.monitor = Some(sample);
                    }
                }
                SshEvent::LoginShell(shell) => {
                    if let Some(hook) = &mut self.folder_hook {
                        hook.on_login_shell(&shell);
                    }
                }
                SshEvent::Closed { exit_status } => {
                    if let Some(m) = self.mosh.take() {
                        let text = crate::mosh::without_key(&m.output);
                        self.message(&format!(
                            "\x1b[31mmosh-server did not start{}\x1b[0m",
                            if text.trim().is_empty() {
                                String::new()
                            } else {
                                format!(":\r\n{text}")
                            }
                        ));
                    }
                    self.folder_hook = None;
                    self.monitor = None;
                    self.backend = Backend::None;
                    self.sftp = SftpStatus::Off;
                    self.line = None;
                    self.host_key = None;
                    let code =
                        exit_status.map_or_else(String::new, |c| format!(" (exit status {c})"));
                    self.status = Some("session closed".into());
                    self.session_ended(&format!("session closed{code}"));
                }
            }
        }
        changed | self.poll_folder_hook()
    }

    /// Type the prompt hook when it is due, or show output held back.
    fn poll_folder_hook(&mut self) -> bool {
        let Some(hook) = &mut self.folder_hook else {
            return false;
        };
        let action = hook.poll(Instant::now());
        let pending = !hook.is_done();
        let changed = match action {
            folder_hook::Action::None => false,
            folder_hook::Action::Type => {
                if let Backend::Ssh(ssh) = &self.backend {
                    ssh.write(FOLDER_TRACKING_HOOK.as_bytes());
                }
                false
            }
            folder_hook::Action::Feed(bytes) => {
                self.output(&bytes);
                true
            }
        };
        if !pending {
            self.folder_hook = None;
        }
        if let SftpStatus::Ready(v) = &mut self.sftp {
            v.auto_hook_pending = pending;
        }
        changed
    }

    /// When the tab needs a frame even without new events (hook and paste
    /// timers).
    pub fn wakeup(&self) -> Option<std::time::Duration> {
        let now = Instant::now();
        let paste =
            (!self.paste_queue.is_empty()).then(|| self.next_paste.saturating_duration_since(now));
        let hook = self.folder_hook.as_ref().and_then(|h| h.wakeup(now));
        // Watch edited files once a second.
        let hook = match (hook, self.edits.is_watching()) {
            (Some(h), true) => Some(h.min(std::time::Duration::from_secs(1))),
            (None, true) => Some(std::time::Duration::from_secs(1)),
            (h, false) => h,
        };
        match (paste, hook) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Auto-fill a saved password once per target; otherwise prompt in the
    /// terminal like PuTTY.
    fn on_secret_needed(&mut self, prompt: String, kind: SecretKind, target: Target) {
        match self.credentials.on_prompt(kind, &target) {
            PromptAction::AutoFill(password) => {
                self.message(&format!(
                    "\x1b[90mUsing saved password for {}\x1b[0m",
                    target.key()
                ));
                if let Some(password) = self.pending_login(password)
                    && let Backend::Ssh(ssh) = &self.backend
                {
                    ssh.reply(Reply::Secret(password));
                }
            }
            PromptAction::Ask { saved_rejected } => {
                if saved_rejected {
                    self.message("\x1b[33mThe saved password was rejected.\x1b[0m");
                }
                self.term.feed(format!("{prompt} ").as_bytes());
                self.line = Some(LineInput::Secret(String::new(), kind, target));
            }
        }
    }

    /// After a successful login, save typed passwords (Always) or queue the
    /// "Do you want to save this password?" questions (Ask).
    fn after_login(&mut self) {
        let (offers, errors) = self.credentials.after_login();
        self.offers.extend(offers);
        for e in errors {
            self.message(&format!("\x1b[31m{e}\x1b[0m"));
        }
    }

    fn on_sftp(&mut self, event: SftpEvent) {
        match event {
            SftpEvent::Ready { home } => {
                if matches!(
                    self.session.as_ref().map(|s| &s.kind),
                    Some(SessionKind::Sftp { .. })
                ) && self.sftp_reopen_cwd.is_none()
                {
                    self.message(&format!(
                        "\x1b[32mConnected ({home}). Drag files between the local and remote panes to transfer them.\x1b[0m"
                    ));
                    self.status = None;
                }
                // 3_18 "Try to follow SSH path in browser" (default on).
                let follow = self
                    .session
                    .as_ref()
                    .and_then(|s| s.options.get("3_18"))
                    .is_none_or(|v| schema::is_true(v));
                let session = self.session.as_ref();
                let is_ftp = matches!(session.map(|s| &s.kind), Some(SessionKind::Ftp { .. }));
                // FTP 3_9 "ASCII mode".
                let ascii = is_ftp
                    && session
                        .and_then(|s| s.options.get("3_9"))
                        .is_some_and(|v| schema::is_true(v));
                self.sftp = SftpStatus::Ready(Box::new(SftpView {
                    cwd: home.clone(),
                    path_input: home.clone(),
                    follow,
                    ascii,
                    sudo: self.sftp_sudo,
                    sudo_supported: matches!(self.backend, Backend::Ssh(_)),
                    ..SftpView::default()
                }));
                // A sudo switch keeps the folder; a new login starts at home.
                match self.sftp_reopen_cwd.take() {
                    Some(cwd) => self.sftp_list(&cwd),
                    None => {
                        self.sftp_just_ready = true;
                        self.sftp_list(&home);
                    }
                }
            }
            SftpEvent::Unavailable(e) => self.sftp = SftpStatus::Unavailable(e),
            // Handled by pump_ftp (the question); the browser waits.
            SftpEvent::UntrustedCertificate { reason, .. } => {
                self.sftp = SftpStatus::Unavailable(format!("certificate not trusted: {reason}"));
            }
            SftpEvent::SudoPasswordNeeded => {
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.prompt = Some(BrowserPrompt::SudoPassword(Default::default()));
                }
            }
            SftpEvent::SudoFailed(e) => {
                self.sftp_sudo = !self.sftp_sudo;
                self.sftp_reopen_cwd = None;
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.sudo = self.sftp_sudo;
                    v.error = Some(e);
                }
            }
            SftpEvent::Listing { id, path, entries } => {
                if let SftpStatus::Ready(v) = &mut self.sftp
                    && v.listing == Some(id)
                {
                    v.listing = None;
                    v.path_input = path.clone();
                    v.cwd = path;
                    v.entries = entries;
                    v.selected.clear();
                    v.error = None;
                }
            }
            SftpEvent::Progress { id, done, total } => {
                if let SftpStatus::Ready(v) = &mut self.sftp
                    && let Some(t) = v.transfers.get_mut(&id)
                {
                    t.1 = done;
                    t.2 = total;
                }
            }
            SftpEvent::Done { id } => {
                if let Some((local, how)) = self.edits.downloaded(id) {
                    use crate::remote_edit::OpenWith;
                    let opened = match how {
                        // The built-in editor (the app opens it).
                        OpenWith::TextEditor => {
                            let remote = self.edits.remote_of(&local).unwrap_or_default();
                            self.open_in_editor.push((local.clone(), remote));
                            Ok(String::new())
                        }
                        OpenWith::DefaultProgram => {
                            crate::external::open_path(&local.display().to_string())
                        }
                        OpenWith::Compare => {
                            self.open_in_diff.push(local.clone());
                            Ok(String::new())
                        }
                    };
                    if let (Err(e), SftpStatus::Ready(v)) = (opened, &mut self.sftp) {
                        v.error = Some(e);
                    }
                }
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.ops.remove(&id);
                    v.transfers.remove(&id);
                    let cwd = v.cwd.clone();
                    self.sftp_list(&cwd);
                }
            }
            SftpEvent::Failed { id, error } => {
                self.edits.failed(id);
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    let label = v.ops.remove(&id);
                    v.transfers.remove(&id);
                    if v.listing == Some(id) {
                        v.listing = None;
                    }
                    v.error = Some(match label {
                        Some(l) => format!("{l}: {error}"),
                        None => error,
                    });
                }
            }
        }
    }

    fn on_cwd_report(&mut self, dir: String) {
        let SftpStatus::Ready(v) = &mut self.sftp else {
            return;
        };
        v.reports_seen = true;
        if v.follow && dir != v.cwd {
            self.sftp_list(&dir);
        }
    }

    /// Type the OSC 7 prompt hook into the remote shell.
    pub fn install_folder_tracking(&mut self) {
        if let Backend::Ssh(ssh) = &self.backend {
            ssh.write(FOLDER_TRACKING_HOOK.as_bytes());
        }
    }

    /// Start or stop "Remote monitoring" for this tab (SSH only).
    pub fn set_monitoring(&mut self, on: bool) {
        self.ssh_settings.remote_monitoring = on;
        let Backend::Ssh(ssh) = &self.backend else {
            return;
        };
        if on == self.monitor.is_some() {
            return;
        }
        self.monitor = on.then(Box::default);
        ssh.set_monitoring(on);
    }

    /// "Use SUDO for SSH-browser": reopen the browser elevated or not. The
    /// password, when sudo asks for one, goes straight to the server.
    pub fn sftp_sudo(&mut self, on: bool, password: Option<String>) {
        let Backend::Ssh(ssh) = &self.backend else {
            return;
        };
        if let SftpStatus::Ready(v) = &mut self.sftp {
            v.sudo = on;
            v.error = None;
            self.sftp_reopen_cwd = Some(v.cwd.clone());
        }
        self.sftp_sudo = on;
        ssh.sftp_sudo(on, password);
    }

    /// "Open with default text editor" / "Open with default program":
    /// download to a private folder, open, then watch for changes.
    pub fn sftp_edit(&mut self, remote: &str, how: crate::remote_edit::OpenWith) {
        let local = match self.edits.start(remote, how) {
            Ok(l) => l,
            Err(e) => {
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.error = Some(format!("cannot open {remote}: {e}"));
                }
                return;
            }
        };
        let request = SftpRequest::Download {
            remote: remote.to_string(),
            local: local.clone(),
            ascii: false,
        };
        if let Some(id) = self.fs_request(request) {
            self.edits.set_download(&local, id);
            if let SftpStatus::Ready(v) = &mut self.sftp {
                v.ops.insert(id, format!("Open {remote}"));
            }
        }
    }

    /// Upload an edited copy back ("upload to the server?" answered yes).
    pub fn upload_edited(&mut self, changed: &crate::remote_edit::Changed) {
        self.sftp_op(SftpRequest::Upload {
            local: changed.local.clone(),
            remote: changed.remote.clone(),
            ascii: false,
        });
    }

    /// Browse to a remote directory.
    pub fn sftp_list(&mut self, path: &str) {
        if !matches!(self.sftp, SftpStatus::Ready(_)) {
            return;
        }
        let id = self.fs_request(SftpRequest::List(path.to_string()));
        if let SftpStatus::Ready(v) = &mut self.sftp {
            v.listing = id;
        }
    }

    /// Start a mutating SFTP operation (refreshes the listing when done).
    pub fn sftp_op(&mut self, request: SftpRequest) {
        if !matches!(self.sftp, SftpStatus::Ready(_)) {
            return;
        }
        let (label, transfer) = match &request {
            SftpRequest::Download { remote, .. } => (format!("Download {remote}"), true),
            SftpRequest::Upload { local, .. } => (format!("Upload {}", local.display()), true),
            SftpRequest::Mkdir(p) => (format!("Create folder {p}"), false),
            SftpRequest::CreateFile(p) => (format!("Create file {p}"), false),
            SftpRequest::Remove { path, .. } => (format!("Delete {path}"), false),
            SftpRequest::Rename { from, .. } => (format!("Rename {from}"), false),
            SftpRequest::List(p) => (format!("List {p}"), false),
            SftpRequest::SetPermissions { path, mode } => {
                (format!("Set permissions {mode:o} on {path}"), false)
            }
        };
        let Some(id) = self.fs_request(request) else {
            return;
        };
        let SftpStatus::Ready(v) = &mut self.sftp else {
            return;
        };
        if transfer {
            v.transfers.insert(id, (label.clone(), 0, None));
        }
        v.ops.insert(id, label);
    }

    /// Answer the pending host key question.
    pub fn answer_host_key(&mut self, accept: bool, remember: bool) {
        self.host_key = None;
        if let Backend::Ssh(ssh) = &self.backend {
            ssh.reply(Reply::HostKey { accept, remember });
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        if self.ended {
            if let Some(action) = ended_key(bytes, &self.expert) {
                self.ended_action = Some(action);
            }
            return;
        }
        self.term.set_scrollback(0);
        if self.line.is_some() {
            self.line_input(bytes);
            return;
        }
        if let Some(hook) = &mut self.folder_hook {
            hook.on_user_input();
        }
        if self.expert.local_echo == Some(true) {
            self.local_echo(bytes);
        }
        let bytes: &[u8] = &self.charset.encode(bytes);
        match &mut self.backend {
            Backend::None => {}
            Backend::Pty(pty) => {
                let _ = pty.write(bytes);
            }
            Backend::Ssh(ssh) => ssh.write(bytes),
            Backend::Stream(stream) => {
                let _ = stream.write(bytes);
            }
            Backend::Connecting(_) | Backend::Ftp(_) => {}
        }
    }

    /// "Force local echo on": show what is typed without waiting for the
    /// remote side (control keys and escape sequences are not shown).
    fn local_echo(&mut self, bytes: &[u8]) {
        if bytes.first() == Some(&0x1b) {
            return;
        }
        let mut shown = Vec::with_capacity(bytes.len());
        for &b in bytes {
            match b {
                b'\r' => shown.extend_from_slice(b"\r\n"),
                0x7f | 0x08 => shown.extend_from_slice(b"\x08 \x08"),
                b'\t' => shown.push(b),
                b if b < 0x20 => {}
                b => shown.push(b),
            }
        }
        if !shown.is_empty() {
            self.term.feed(&shown);
        }
    }

    fn line_input(&mut self, bytes: &[u8]) {
        for ch in String::from_utf8_lossy(bytes).chars() {
            let Some(line) = &mut self.line else { return };
            let (buffer, echo) = match line {
                LineInput::Login(b) => (b, true),
                LineInput::Secret(b, ..) => (b, false),
            };
            match ch {
                '\r' | '\n' => {
                    self.term.feed(b"\r\n");
                    match self.line.take() {
                        Some(LineInput::Login(user))
                            if !user.is_empty() && self.via_gateway.is_some() =>
                        {
                            let via = self.via_gateway.take().expect("checked");
                            self.start_via_gateway(via, Some(user));
                        }
                        Some(LineInput::Login(user)) if !user.is_empty() => {
                            match self.session.as_ref().map(|s| &s.kind) {
                                Some(SessionKind::Mosh { .. }) => self.connect_mosh(user),
                                Some(SessionKind::Sftp { .. }) => self.connect_sftp(user),
                                _ => self.connect_ssh(user),
                            }
                        }
                        Some(LineInput::Login(_)) => {
                            self.term.feed(b"login as: ");
                            self.line = Some(LineInput::Login(String::new()));
                        }
                        Some(LineInput::Secret(secret, kind, target)) => {
                            self.credentials.on_typed(kind, target, secret.clone());
                            if let Some(secret) = self.pending_login(secret)
                                && let Backend::Ssh(ssh) = &self.backend
                            {
                                ssh.reply(Reply::Secret(secret));
                            }
                        }
                        None => {}
                    }
                }
                '\u{7f}' | '\u{8}' => {
                    if buffer.pop().is_some() && echo {
                        self.term.feed(b"\x08 \x08");
                    }
                }
                // Ctrl+C / Ctrl+D abort the prompt.
                '\u{3}' | '\u{4}' => {
                    self.term.feed(b"^C\r\n");
                    self.line = None;
                    match &self.backend {
                        Backend::Ssh(ssh) => ssh.reply(Reply::Cancel),
                        _ => self.message("\x1b[90m[cancelled]\x1b[0m"),
                    }
                }
                c if !c.is_control() => {
                    buffer.push(c);
                    if echo {
                        let mut tmp = [0u8; 4];
                        self.term.feed(c.encode_utf8(&mut tmp).as_bytes());
                    }
                }
                _ => {}
            }
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.term.size() == (rows, cols) {
            return;
        }
        self.term.resize(rows, cols);
        match &self.backend {
            Backend::None => {}
            Backend::Pty(pty) => {
                let _ = pty.resize(rows, cols);
            }
            Backend::Ssh(ssh) => ssh.resize(rows, cols),
            Backend::Stream(_) | Backend::Connecting(_) | Backend::Ftp(_) => {}
        }
        if let Backend::Stream(stream) = &mut self.backend {
            stream.resize(rows, cols);
        }
    }

    /// A hidden (password-like) prompt is waiting for input.
    pub fn awaiting_secret(&self) -> bool {
        matches!(self.line, Some(LineInput::Secret(..)))
    }

    /// SFTP and FTP sessions show the two-pane file view.
    pub fn is_file_session(&self) -> bool {
        matches!(
            self.session.as_ref().map(|s| &s.kind),
            Some(SessionKind::Sftp { .. } | SessionKind::Ftp { .. } | SessionKind::AwsS3 { .. })
        )
    }

    /// The local pane, created on first use in the session's "Local
    /// startup folder" (SFTP 3_16).
    pub fn local_pane_mut(&mut self) -> &mut crate::ui::local_pane::LocalPane {
        let start = self
            .session
            .as_ref()
            .filter(|s| matches!(s.kind, SessionKind::Sftp { .. }))
            .and_then(|s| s.options.get("3_16"))
            .cloned();
        self.local_pane
            .get_or_insert_with(|| crate::ui::local_pane::LocalPane::new(start.as_deref()))
    }

    pub fn is_alive(&self) -> bool {
        !matches!(self.backend, Backend::None)
            || self.line.is_some()
            || self.gateway_tunnel.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Pump a tab until `done` holds (answering the host key prompt), or panic.
    fn pump_until(tab: &mut Tab, mut done: impl FnMut(&mut Tab) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            tab.pump();
            if tab.host_key.is_some() {
                tab.answer_host_key(true, false);
            }
            if done(tab) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timeout; screen:\n{}", tab.term.contents());
    }

    fn dev_session(port: u16) -> Session {
        let mut s = Session::new(
            "dev",
            SessionKind::Ssh {
                host: "127.0.0.1".into(),
                port,
                user: Some(tc_devserver::DEV_USER.into()),
                private_key: None,
                x11_forwarding: false,
            },
        );
        s.options
            .insert("3_8".into(), "echo saved-password-test".into());
        s.options.insert("3_26".into(), "0".into());
        s
    }

    #[test]
    fn term_charset_choices_match_the_schema() {
        let param = schema::terminal_params()
            .iter()
            .find(|p| p.id == "4_6")
            .expect("4_6");
        let schema::Widget::Choice(choices) = param.widget else {
            panic!("4_6 is a choice");
        };
        assert_eq!(choices, tc_term::charset::NAMES);
        assert_eq!(param.default, tc_term::charset::UTF8.to_string());

        let mut s = Session::new(
            "x",
            SessionKind::Telnet {
                host: "h".into(),
                port: 23,
            },
        );
        assert!(term_charset(Some(&s)).is_utf8());
        s.options.insert("4_6".into(), "0".into());
        assert!(!term_charset(Some(&s)).is_utf8());
    }

    #[test]
    fn paste_delay_and_log_folder_from_session_options() {
        let mut s = Session::new(
            "x",
            SessionKind::Telnet {
                host: "h".into(),
                port: 23,
            },
        );
        assert_eq!(paste_delay(Some(&s)), Duration::ZERO);
        s.options.insert("4_40".into(), "4".into());
        assert_eq!(paste_delay(Some(&s)), Duration::from_millis(30));
        s.options.insert("4_40".into(), "13".into());
        assert_eq!(paste_delay(Some(&s)), Duration::from_millis(360));

        let global = std::path::Path::new("/logs");
        assert_eq!(log_folder(Some(&s), None), None);
        assert_eq!(log_folder(None, Some(global)), Some(PathBuf::from("/logs")));
        s.options.insert("4_12".into(), "-1".into());
        assert_eq!(
            log_folder(Some(&s), Some(global)),
            Some(PathBuf::from("/logs"))
        );
        s.options.insert("4_13".into(), "/session-logs".into());
        assert_eq!(
            log_folder(Some(&s), Some(global)),
            Some(PathBuf::from("/session-logs"))
        );
    }

    #[test]
    fn delphi_colors() {
        assert_eq!(delphi_color("255"), Some([255, 0, 0]), "red is 0x0000FF");
        assert_eq!(delphi_color("16711680"), Some([0, 0, 255]));
        assert_eq!(delphi_color("-1"), None);
        assert_eq!(delphi_color("x"), None);
    }

    #[test]
    fn session_stopped_keys() {
        let keys = ExpertTerminal::default();
        assert_eq!(ended_key(b"\r", &keys), Some(EndedAction::Close));
        assert_eq!(ended_key(b"R", &keys), Some(EndedAction::Restart));
        assert_eq!(ended_key(b"s", &keys), Some(EndedAction::Save));
        assert_eq!(
            ended_key(b"x", &keys),
            Some(EndedAction::Close),
            "any other key"
        );
        assert_eq!(ended_key(b"rr", &keys), None, "a paste is not a key");
        let keys = ExpertTerminal {
            exit_key: "RETURN".into(),
            reconnect_key: "F5".into(),
            ..ExpertTerminal::default()
        };
        assert_eq!(ended_key(b"x", &keys), None);
        assert_eq!(ended_key(b"\x1b[15~", &keys), Some(EndedAction::Restart));
        assert_eq!(key_label("F5"), "<F5>");
        assert_eq!(key_label("Any other key"), "any other key");
    }

    #[test]
    fn keep_open_runs_the_login_shell_after_the_command() {
        assert_eq!(keep_open("top", false), "top");
        assert_eq!(keep_open("top", true), "top; exec \"${SHELL:-/bin/sh}\" -l");
    }

    #[test]
    fn telnet_tab_shows_server_output() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            // IAC WILL ECHO, then a banner.
            s.write_all(&[255, 251, 1]).unwrap();
            s.write_all(b"Welcome to telnet-test\r\n").unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
        let dir = tempfile::tempdir().unwrap();
        let env = TabEnv {
            scrollback: 100,
            known_hosts: dir.path().join("kh"),
            repaint: Arc::new(|| {}),
            vault: Arc::new(Vault::open(
                dir.path().join("i.toml"),
                Box::new(tc_vault::MemoryStore::default()),
            )),
            save_passwords: SavePasswords::Ask,
            ssh: Default::default(),
            backspace_ctrl_h: false,
            paste_right_click: true,
            local_shell: Default::default(),
            save_key_passphrases: false,
            log_dir: None,
            use_windows_path: true,
            home: None,
        };
        let session = Session::new(
            "t",
            SessionKind::Telnet {
                host: "127.0.0.1".into(),
                port,
            },
        );
        let mut tab = Tab::open(1, Some(session), &env);
        pump_until(&mut tab, |t| {
            t.term.contents().contains("Welcome to telnet-test")
        });
        pump_until(&mut tab, |t| {
            t.term.contents().contains("connection closed")
        });
    }

    /// Bookmark 1_1 off: no "Session stopped" prompt, the tab closes.
    #[test]
    fn no_reconnection_message_closes_the_tab() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || drop(listener.accept()));
        let dir = tempfile::tempdir().unwrap();
        let env = TabEnv {
            scrollback: 100,
            known_hosts: dir.path().join("kh"),
            repaint: Arc::new(|| {}),
            vault: Arc::new(Vault::open(
                dir.path().join("i.toml"),
                Box::new(tc_vault::MemoryStore::default()),
            )),
            save_passwords: SavePasswords::Ask,
            ssh: Default::default(),
            backspace_ctrl_h: false,
            paste_right_click: true,
            local_shell: Default::default(),
            save_key_passphrases: false,
            log_dir: None,
            use_windows_path: true,
            home: None,
        };
        let mut session = Session::new(
            "t",
            SessionKind::Telnet {
                host: "127.0.0.1".into(),
                port,
            },
        );
        session.options.insert("1_1".into(), "0".into());
        session.options.insert("5_1".into(), "2".into());
        let mut tab = Tab::open(1, Some(session), &env);
        assert!(
            tab.detached && tab.detached_maximized,
            "5_1 maximized detached"
        );
        pump_until(&mut tab, |t| t.ended_action.is_some());
        assert_eq!(tab.ended_action, Some(EndedAction::Close));
    }

    /// End to end: a Telnet session whose jump host has no login asks for
    /// it, opens the tunnel through the gateway, then talks Telnet over it.
    #[test]
    fn telnet_through_an_ssh_gateway() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let telnet_port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"Welcome through the gateway\r\n").unwrap();
            std::thread::sleep(Duration::from_millis(500));
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (gw, fut) = rt
            .block_on(tc_devserver::bind(0, tc_devserver::DevConfig::default()))
            .unwrap();
        rt.spawn(fut);
        let dir = tempfile::tempdir().unwrap();
        let env = TabEnv {
            scrollback: 100,
            known_hosts: dir.path().join("kh"),
            repaint: Arc::new(|| {}),
            vault: Arc::new(Vault::open(
                dir.path().join("i.toml"),
                Box::new(tc_vault::MemoryStore::default()),
            )),
            save_passwords: SavePasswords::Never,
            ssh: Default::default(),
            backspace_ctrl_h: false,
            paste_right_click: true,
            local_shell: Default::default(),
            save_key_passphrases: false,
            log_dir: None,
            use_windows_path: true,
            home: None,
        };
        let mut session = Session::new(
            "t",
            SessionKind::Telnet {
                host: "127.0.0.1".into(),
                port: telnet_port,
            },
        );
        // Telnet Network settings: 3_7 gateway host, 3_8 port.
        session.options.insert("3_7".into(), "127.0.0.1".into());
        session.options.insert("3_8".into(), gw.port().to_string());
        let mut tab = Tab::open(1, Some(session), &env);
        pump_until(&mut tab, |t| t.term.contents().contains("login as:"));
        tab.send(format!("{}\r", tc_devserver::DEV_USER).as_bytes());
        pump_until(&mut tab, |t| t.term.contents().contains("password:"));
        tab.send(format!("{}\r", tc_devserver::DEV_PASSWORD).as_bytes());
        pump_until(&mut tab, |t| {
            t.term.contents().contains("Welcome through the gateway")
        });
        assert!(tab.gateway_tunnel.is_some());
    }

    /// End to end: the login shell is bash, so the hook is typed after the
    /// first prompt, the browser follows `cd`, and the hook never shows.
    /// Skipped when bash is not installed.
    #[test]
    fn folder_tracking_is_automatic_and_hidden_for_bash() {
        let Some(mut bash) = tc_pty::shells::locate(tc_pty::ShellKind::Bash) else {
            return;
        };
        bash.args = vec!["--norc".into(), "--noprofile".into(), "-i".into()];
        bash.env.push(("PS1".into(), "tc$ ".into()));
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let config = tc_devserver::DevConfig {
            shell: Some(bash),
            login_shell: Some("/usr/bin/bash".into()),
            sftp_root: root.path().to_path_buf(),
            ..Default::default()
        };
        let (addr, fut) = rt.block_on(tc_devserver::bind(0, config)).unwrap();
        rt.spawn(fut);
        let dir = tempfile::tempdir().unwrap();
        let env = TabEnv {
            scrollback: 100,
            known_hosts: dir.path().join("known_hosts"),
            repaint: Arc::new(|| {}),
            vault: Arc::new(Vault::open(
                dir.path().join("index.toml"),
                Box::new(tc_vault::MemoryStore::default()),
            )),
            save_passwords: SavePasswords::Never,
            ssh: Default::default(),
            backspace_ctrl_h: false,
            paste_right_click: true,
            local_shell: Default::default(),
            save_key_passphrases: false,
            log_dir: None,
            use_windows_path: true,
            home: None,
        };
        let mut session = dev_session(addr.port());
        session.options.remove("3_8");
        session.options.remove("3_26");
        let mut tab = Tab::open(1, Some(session), &env);
        pump_until(&mut tab, |t| t.term.contents().contains("password:"));
        tab.send(format!("{}\r", tc_devserver::DEV_PASSWORD).as_bytes());
        pump_until(
            &mut tab,
            |t| matches!(&t.sftp, SftpStatus::Ready(v) if v.reports_seen),
        );
        pump_until(&mut tab, |t| t.term.contents().contains("tc$ "));
        let screen = tab.term.contents();
        assert!(!screen.contains("PROMPT_COMMAND"), "{screen}");
    }

    #[test]
    fn saved_password_offer_and_autofill() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (addr, fut) = rt
            .block_on(tc_devserver::bind(0, tc_devserver::DevConfig::default()))
            .unwrap();
        rt.spawn(fut);
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(Vault::open(
            dir.path().join("index.toml"),
            Box::new(tc_vault::MemoryStore::default()),
        ));
        let env = TabEnv {
            scrollback: 100,
            known_hosts: dir.path().join("known_hosts"),
            repaint: Arc::new(|| {}),
            vault: vault.clone(),
            save_passwords: SavePasswords::Ask,
            ssh: Default::default(),
            backspace_ctrl_h: false,
            paste_right_click: true,
            local_shell: Default::default(),
            save_key_passphrases: false,
            log_dir: None,
            use_windows_path: true,
            home: None,
        };

        // 1st connection: no saved password -> prompt in the terminal.
        let mut tab = Tab::open(1, Some(dev_session(addr.port())), &env);
        pump_until(&mut tab, |t| t.term.contents().contains("password:"));
        tab.send(format!("{}\r", tc_devserver::DEV_PASSWORD).as_bytes());
        pump_until(&mut tab, |t| !t.offers.is_empty());
        let (target, password) = tab.offers.pop_front().unwrap();
        assert_eq!(
            target,
            Target::ssh(tc_devserver::DEV_USER, "127.0.0.1", addr.port())
        );
        vault.save(&target, &password).unwrap();
        pump_until(&mut tab, |t| !t.is_alive());
        assert!(tab.term.contents().contains("saved-password-test"));

        // 2nd connection: the saved password is used, nothing is typed.
        let mut tab = Tab::open(2, Some(dev_session(addr.port())), &env);
        pump_until(&mut tab, |t| !t.is_alive());
        let screen = tab.term.contents();
        assert!(screen.contains("Using saved password"), "{screen}");
        assert!(!screen.contains("password: "), "{screen}");
        assert!(screen.contains("saved-password-test"), "{screen}");
        assert!(tab.offers.is_empty());
    }

    /// Run the hook in a real bash (skipped when bash is not installed) and
    /// check that the prompt command emits an OSC 7 report for `$PWD`.
    #[test]
    fn folder_tracking_hook_reports_pwd_in_bash() {
        let hook = FOLDER_TRACKING_HOOK.trim_end_matches('\r');
        let script = format!("{hook}\ncd /tmp && eval \"$PROMPT_COMMAND\"");
        // Like the app: Git Bash / MSYS2 on Windows, never the WSL launcher.
        let Some(bash) = tc_pty::shells::locate(tc_pty::ShellKind::Bash) else {
            eprintln!("bash not available; skipping");
            return;
        };
        let Ok(out) = std::process::Command::new(&bash.program)
            .args(["--norc", "--noprofile", "-c", &script])
            .output()
        else {
            eprintln!("bash not available; skipping");
            return;
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{stdout} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.starts_with("\x1b]7;file://"), "{stdout:?}");
        assert!(stdout.ends_with("/tmp\x07"), "{stdout:?}");
    }
}
