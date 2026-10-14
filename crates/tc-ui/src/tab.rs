use crate::credentials::{Credentials, PromptAction, target_for};
use crate::folder_hook::{self, FolderHook};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tc_core::settings::{LocalShell, SavePasswords};
use tc_core::{Session, SessionKind, schema};
use tc_pty::{PtyEvent, PtySession};
use tc_ssh::{
    ConnectParams, RemoteEntry, Reply, SecretKind, SftpEvent, SftpRequest, SshEvent, SshSession,
};
use tc_term::Terminal;
use tc_vault::{Target, Vault};

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
}

pub fn agent_config(ssh: &tc_core::settings::SshSettings) -> tc_ssh::AgentConfig {
    tc_ssh::AgentConfig {
        system: ssh.use_system_agent,
        pageant: ssh.use_pageant,
        socket: None,
    }
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
    Ftp(tc_net::ftp::FtpSession),
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
    listing: Option<u64>,
}

pub enum BrowserPrompt {
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
    /// Latest "Remote monitoring" reading (`Some` while monitoring is on).
    pub monitor: Option<Box<tc_core::monitor::Sample>>,
    /// The user toggled "Remote monitoring" here; the app applies it to
    /// every SSH tab and saves the setting.
    pub monitoring_request: Option<bool>,
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
            monitor: None,
            monitoring_request: None,
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
        match tab.session.as_ref().map(|s| s.kind.clone()) {
            None => tab.spawn_local(None, env.local_shell),
            Some(SessionKind::LocalShell { program }) => {
                // 3_2 "Terminal shell": 0 = the configured default.
                let shell = tab
                    .session
                    .as_ref()
                    .and_then(|s| s.options.get("3_2"))
                    .and_then(|v| LocalShell::from_option(v))
                    .unwrap_or(env.local_shell);
                tab.spawn_local(program.as_deref(), shell)
            }
            Some(SessionKind::Ssh {
                user: Some(user), ..
            }) => tab.connect_ssh(user),
            Some(SessionKind::Ssh { user: None, .. }) => {
                tab.term.feed(b"login as: ");
                tab.line = Some(LineInput::Login(String::new()));
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
            Some(other) => {
                tab.message(&format!(
                    "\x1b[33m{} sessions are not implemented yet - see docs/PARITY.md\x1b[0m",
                    other.label()
                ));
                tab.status = Some("unsupported session type".into());
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

    fn message(&mut self, text: &str) {
        self.term.feed(format!("{text}\r\n").as_bytes());
    }

    fn spawn_local(&mut self, program: Option<&str>, shell: LocalShell) {
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
        match PtySession::spawn_command(&command, INITIAL_ROWS, INITIAL_COLS, self.repaint.clone())
        {
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
        // Network settings: 3_9 gateway host, 3_10 port, 3_11 login, 3_16 key.
        let gateway = opt("3_9")
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(|h| tc_ssh::Endpoint {
                host: h.to_string(),
                port: opt("3_10")
                    .and_then(|p| p.trim().parse().ok())
                    .unwrap_or(22),
                user: opt("3_11")
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .map_or_else(|| user.clone(), String::from),
                private_key: opt("3_16")
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(PathBuf::from),
            });
        let mut params = ConnectParams {
            host: host.clone(),
            port: *port,
            user,
            private_key: private_key.as_ref().map(PathBuf::from),
            gateway,
            known_hosts: self.known_hosts.clone(),
            compression: opt("3_7").is_some_and(schema::is_true),
            command: opt("3_8")
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(String::from),
            term: self
                .term_type
                .clone()
                .unwrap_or_else(|| "xterm-256color".into()),
            cols,
            rows,
            // 3_26 SSH-browser type: 0 = none.
            sftp: self.ssh_settings.browser && opt("3_26") != Some("0"),
            keepalive: (self.ssh_settings.keepalive_secs > 0)
                .then(|| std::time::Duration::from_secs(self.ssh_settings.keepalive_secs)),
            detect_login_shell: false,
            remote_monitoring: self.ssh_settings.remote_monitoring,
            agent: agent_config(&self.ssh_settings),
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

    fn start_ftp(&mut self, params: tc_net::ftp::FtpParams) {
        self.message(&format!(
            "\x1b[90mConnecting to {}:{} (FTP) …\x1b[0m",
            params.host, params.port
        ));
        self.sftp = SftpStatus::Connecting;
        self.backend = Backend::Ftp(tc_net::ftp::FtpSession::connect(
            params,
            self.repaint.clone(),
        ));
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
                        "\x1b[32mConnected. Browse files in the side panel ({home}).\x1b[0m"
                    ));
                    self.status = None;
                    self.after_login();
                }
                SftpEvent::Unavailable(e) => {
                    self.message(&format!("\x1b[31mFTP: {e}\x1b[0m"));
                    self.status = Some("connection failed".into());
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
                    let reply = self.term.feed(&bytes);
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
                    self.message(&format!(
                        "\r\n\x1b[90m[connection closed{why} - close the tab or reopen the session]\x1b[0m"
                    ));
                    break;
                }
            }
        }
        changed
    }

    fn pump_pty(&mut self) -> bool {
        let Backend::Pty(pty) = &mut self.backend else {
            return false;
        };
        let mut changed = false;
        let mut exited = false;
        while let Ok(event) = pty.events().try_recv() {
            changed = true;
            match event {
                PtyEvent::Output(bytes) => {
                    let reply = self.term.feed(&bytes);
                    if !reply.is_empty() {
                        let _ = pty.write(&reply);
                    }
                }
                PtyEvent::Exited => exited = true,
            }
        }
        if exited {
            self.backend = Backend::None;
            self.status = Some("process exited".into());
            self.message(
                "\r\n\x1b[90m[process exited - close the tab or reopen the session]\x1b[0m",
            );
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
            match event {
                SshEvent::Output(bytes) => {
                    let bytes = match &mut self.folder_hook {
                        Some(hook) => hook.filter(&bytes, Instant::now()),
                        None => bytes,
                    };
                    let reply = self.term.feed(&bytes);
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
                    self.folder_hook = None;
                    self.monitor = None;
                    self.backend = Backend::None;
                    self.sftp = SftpStatus::Off;
                    self.line = None;
                    self.host_key = None;
                    let code =
                        exit_status.map_or_else(String::new, |c| format!(" (exit status {c})"));
                    self.status = Some("session closed".into());
                    self.message(&format!(
                        "\r\n\x1b[90m[session closed{code} - close the tab or reopen the session]\x1b[0m"
                    ));
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
        let changed = match hook.poll(Instant::now()) {
            folder_hook::Action::None => false,
            folder_hook::Action::Type => {
                if let Backend::Ssh(ssh) = &self.backend {
                    ssh.write(FOLDER_TRACKING_HOOK.as_bytes());
                }
                false
            }
            folder_hook::Action::Feed(bytes) => {
                self.term.feed(&bytes);
                true
            }
        };
        let pending = !hook.is_done();
        if !pending {
            self.folder_hook = None;
        }
        if let SftpStatus::Ready(v) = &mut self.sftp {
            v.auto_hook_pending = pending;
        }
        changed
    }

    /// When the tab needs a frame even without new events (hook timers).
    pub fn wakeup(&self) -> Option<std::time::Duration> {
        self.folder_hook.as_ref()?.wakeup(Instant::now())
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
                if let Some(mut ftp) = self.pending_ftp.take() {
                    ftp.password = password;
                    self.start_ftp(ftp);
                } else if let Backend::Ssh(ssh) = &self.backend {
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
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.ops.remove(&id);
                    v.transfers.remove(&id);
                    let cwd = v.cwd.clone();
                    self.sftp_list(&cwd);
                }
            }
            SftpEvent::Failed { id, error } => {
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
        self.term.set_scrollback(0);
        if self.line.is_some() {
            self.line_input(bytes);
            return;
        }
        if let Some(hook) = &mut self.folder_hook {
            hook.on_user_input();
        }
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
                        Some(LineInput::Login(user)) if !user.is_empty() => self.connect_ssh(user),
                        Some(LineInput::Login(_)) => {
                            self.term.feed(b"login as: ");
                            self.line = Some(LineInput::Login(String::new()));
                        }
                        Some(LineInput::Secret(secret, kind, target)) => {
                            self.credentials.on_typed(kind, target, secret.clone());
                            if let Some(mut ftp) = self.pending_ftp.take() {
                                ftp.password = secret;
                                self.start_ftp(ftp);
                            } else if let Backend::Ssh(ssh) = &self.backend {
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

    pub fn is_alive(&self) -> bool {
        !matches!(self.backend, Backend::None) || self.line.is_some()
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
        let Ok(out) = std::process::Command::new("bash")
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
