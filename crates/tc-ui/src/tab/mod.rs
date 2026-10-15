//! One tab: a terminal (or a file browser) and the connection behind it.

mod connect;
mod files;
mod input;
mod options;
mod pump;
#[cfg(test)]
mod tests;

use crate::credentials::{Credentials, PromptAction, target_for};
use crate::folder_hook::{self, FolderHook};
pub use options::agent_config;
use options::*;
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
    /// Configuration › Security "Disable log".
    pub disable_log: bool,
    /// Configuration › Terminal "Log type" and "Log filename".
    pub log_type: tc_core::settings::LogType,
    pub log_name: String,
    /// Configuration › X11 "Display offset": the local display of X11
    /// forwarding on Windows.
    pub x_display: u16,
    /// Configuration › Security "Disable RDP \"reconnect\"".
    pub no_rdp_reconnect: bool,
    /// "Use Windows PATH" in the local Bash / Zsh.
    pub use_windows_path: bool,
    /// Configured terminal colors and cursor, if any.
    pub theme: Option<tc_term::Theme>,
    /// "Home directory location": `HOME` of the local Bash / Zsh.
    pub home: Option<PathBuf>,
}

/// Configuration › SSH "Default login": the user of sessions and jump
/// hosts that name none (else "login as:" asks).
fn default_login(ssh: &tc_core::settings::SshSettings) -> Option<String> {
    let user = ssh.default_login.trim();
    (!user.is_empty()).then(|| user.to_string())
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
    let mut hops = session
        .and_then(|s| {
            let ids = tc_core::jump_hosts::ids_for(s.kind.type_index())?;
            Some(tc_core::jump_hosts::parse(&s.options, ids))
        })
        .unwrap_or_default();
    // An "SSH forwarding" proxy is one more gateway, the first one.
    if let Some(p) = session_proxy(session)
        && p.kind == tc_ssh::proxy::ProxyKind::SshForward
    {
        hops.insert(
            0,
            tc_core::jump_hosts::JumpHost {
                host: p.host,
                port: p.port.to_string(),
                user: p.user.unwrap_or_default(),
                key: String::new(),
            },
        );
    }
    let session = session?;
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
    /// Session font (name / bold / fixed grid); `None` = global settings.
    pub font_name: Option<String>,
    pub font_bold: Option<bool>,
    pub fixed_grid: Option<Option<(u16, u16)>>,
    pub backspace_sends_ctrl_h: bool,
    pub paste_right_click: bool,
    ssh_settings: tc_core::settings::SshSettings,
    /// See [`TabEnv::x_display`].
    x_display: u16,
    no_rdp_reconnect: bool,
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
    /// "Force local line editing on": the line being typed.
    line_editor: tc_term::LineEditor,
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
    /// 4_38 "Execute macro at session start": typed once the session shows
    /// its first output.
    pub start_macro: Option<String>,
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
    /// Remote images to show in the viewer: (local, remote).
    pub open_in_viewer: Vec<(PathBuf, String)>,
    /// Local side of the SFTP / FTP session view.
    pub local_pane: Option<crate::ui::local_pane::LocalPane>,
    /// 4_36 "Syntax highlighting" (`None` = the global setting).
    pub syntax: Option<usize>,
    /// Telnet / RDP / VNC waiting for its tunnel through the jump hosts.
    via_gateway: Option<ViaGateway>,
    /// Telnet "Username": typed once at the server's login prompt.
    auto_login: Option<String>,
    /// That tunnel, kept open as long as the tab.
    gateway_tunnel: Option<SshSession>,
    /// Telnet / VNC through a session proxy: the loopback port it listens on.
    proxy_forward: Option<tc_ssh::proxy::LocalForward>,
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
            font_name: None,
            font_bold: None,
            fixed_grid: None,
            backspace_sends_ctrl_h: false,
            paste_right_click: env.paste_right_click,
            ssh_settings: env.ssh.clone(),
            x_display: env.x_display,
            no_rdp_reconnect: env.no_rdp_reconnect,
            term_type: None,
            pending_ftp: None,
            pending_s3: None,
            backend: Backend::None,
            status: None,
            host_key: None,
            sftp: SftpStatus::Off,
            sftp_just_ready: false,
            line: None,
            line_editor: tc_term::LineEditor::default(),
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
            auto_login: None,
            gateway_tunnel: None,
            proxy_forward: None,
            syntax: None,
            local_pane: None,
            open_in_editor: Vec::new(),
            open_in_diff: Vec::new(),
            open_in_viewer: Vec::new(),
            detached_maximized: false,
            wants_fullscreen: false,
            lock_title: true,
            expert: ExpertTerminal::default(),
            start_macro: None,
            bell_flash: None,
            next_paste: Instant::now(),
            log: None,
            find: None,
            cert_prompt: None,
            last_ftp: None,
            edits: crate::remote_edit::RemoteEdits::new(),
            upload_prompts: Default::default(),
            monitor: None,
            monitoring_request: None,
            mosh: None,
            sftp_sudo: false,
            sftp_reopen_cwd: None,
        };
        let settings = crate::term_settings::for_session(tab.session.as_ref(), env.theme.as_ref());
        tab.term.theme = settings.theme;
        tab.font_size = settings.font_px;
        tab.font_name = settings.font_name;
        tab.font_bold = settings.font_bold;
        tab.fixed_grid = settings.fixed_grid;
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
        if !env.disable_log
            && let Some(folder) = log_folder(tab.session.as_ref(), env.log_dir.as_deref())
        {
            match crate::term_log::TermLog::create(&folder, &tab.title, env.log_type, &env.log_name)
            {
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
            Some(SessionKind::Ssh { user: None, .. }) => match default_login(&env.ssh) {
                Some(user) => tab.connect_ssh(user),
                None => {
                    tab.term.feed(b"login as: ");
                    tab.line = Some(LineInput::Login(String::new()));
                }
            },
            // A session proxy (other than "SSH forwarding", which is a
            // gateway) is never skipped: through it, or not at all.
            Some(SessionKind::Telnet { .. } | SessionKind::Vnc { .. })
                if let Some(proxy) = session_proxy(tab.session.as_ref())
                    && proxy.kind != tc_ssh::proxy::ProxyKind::SshForward =>
            {
                tab.open_via_proxy(proxy);
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
                let mut options = crate::external::RdpOptions::from_options(&|id| {
                    tab.session
                        .as_ref()
                        .and_then(|s| s.options.get(id).cloned())
                });
                options.no_reconnect = tab.no_rdp_reconnect;
                let result =
                    crate::external::launch_rdp(&host, port, user.as_deref(), &options, &temp);
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
            Some(SessionKind::Sftp { user: None, .. }) => match default_login(&env.ssh) {
                Some(user) => tab.connect_sftp(user),
                None => {
                    tab.term.feed(b"login as: ");
                    tab.line = Some(LineInput::Login(String::new()));
                }
            },
            Some(SessionKind::Mosh {
                user: Some(user), ..
            }) => tab.connect_mosh(user),
            Some(SessionKind::Mosh { user: None, .. }) => match default_login(&env.ssh) {
                Some(user) => tab.connect_mosh(user),
                None => {
                    tab.term.feed(b"login as: ");
                    tab.line = Some(LineInput::Login(String::new()));
                }
            },
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

    /// Answer the pending host key question.
    pub fn answer_host_key(&mut self, accept: bool, remember: bool) {
        self.host_key = None;
        if let Backend::Ssh(ssh) = &self.backend {
            ssh.reply(Reply::HostKey { accept, remember });
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
    /// startup folder" (SFTP 3_16, FTP 3_21).
    pub fn local_pane_mut(&mut self) -> &mut crate::ui::local_pane::LocalPane {
        let start = self
            .session
            .as_ref()
            .and_then(|s| match s.kind {
                SessionKind::Sftp { .. } => s.options.get("3_16"),
                // FTP 3_21 "Local startup folder".
                SessionKind::Ftp { .. } => s.options.get("3_21"),
                _ => None,
            })
            .filter(|d| !d.trim().is_empty() && d.as_str() != "<>")
            .cloned();
        self.local_pane
            .get_or_insert_with(|| crate::ui::local_pane::LocalPane::new(start.as_deref()))
    }

    pub fn is_alive(&self) -> bool {
        !matches!(self.backend, Backend::None)
            || self.line.is_some()
            || self.gateway_tunnel.is_some()
            || self.proxy_forward.is_some()
    }
}
