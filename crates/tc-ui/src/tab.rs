use crate::credentials::{Credentials, PromptAction};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tc_core::settings::SavePasswords;
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
}

enum Backend {
    None,
    Pty(PtySession),
    Ssh(SshSession),
}

/// An unknown SSH host key waiting for the user's decision.
pub struct HostKeyPrompt {
    pub host: String,
    pub algorithm: String,
    pub fingerprint: String,
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
    listing: Option<u64>,
}

pub enum BrowserPrompt {
    NewFolder(String),
    NewFile(String),
    Rename { from: String, to: String },
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
    term_type: Option<String>,
    backend: Backend,
    pub status: Option<String>,
    pub host_key: Option<HostKeyPrompt>,
    pub sftp: SftpStatus,
    /// Set once when the SSH-browser becomes ready (the app shows the Sftp tab).
    pub sftp_just_ready: bool,
    line: Option<LineInput>,
    known_hosts: PathBuf,
    repaint: Arc<dyn Fn() + Send + Sync>,
    credentials: Credentials,
    /// "Do you want to save this password?" questions waiting for an answer.
    pub offers: std::collections::VecDeque<(Target, String)>,
}

/// Prompt hook for bash and zsh that reports `$PWD` with OSC 7. Typed into
/// the shell only when the user asks; the leading space keeps it out of
/// bash history when `HISTCONTROL` ignores spaces.
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
            term_type: None,
            backend: Backend::None,
            status: None,
            host_key: None,
            sftp: SftpStatus::Off,
            sftp_just_ready: false,
            line: None,
            known_hosts: env.known_hosts.clone(),
            repaint: env.repaint.clone(),
            credentials: Credentials::new(env.vault.clone(), env.save_passwords),
            offers: Default::default(),
        };
        let settings = crate::term_settings::for_session(tab.session.as_ref());
        tab.term.theme = settings.theme;
        tab.font_size = settings.font_px;
        tab.backspace_sends_ctrl_h = settings.backspace_sends_ctrl_h;
        tab.term_type = settings.term_type;
        match tab.session.as_ref().map(|s| s.kind.clone()) {
            None => tab.spawn_local(None),
            Some(SessionKind::LocalShell { program }) => tab.spawn_local(program.as_deref()),
            Some(SessionKind::Ssh {
                user: Some(user), ..
            }) => tab.connect_ssh(user),
            Some(SessionKind::Ssh { user: None, .. }) => {
                tab.term.feed(b"login as: ");
                tab.line = Some(LineInput::Login(String::new()));
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

    fn message(&mut self, text: &str) {
        self.term.feed(format!("{text}\r\n").as_bytes());
    }

    fn spawn_local(&mut self, program: Option<&str>) {
        match PtySession::spawn(
            program,
            &[],
            INITIAL_ROWS,
            INITIAL_COLS,
            self.repaint.clone(),
        ) {
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
        let params = ConnectParams {
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
            sftp: opt("3_26") != Some("0"),
        };
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
        }
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
                SshEvent::UnknownHostKey {
                    host,
                    port,
                    algorithm,
                    fingerprint,
                } => {
                    self.host_key = Some(HostKeyPrompt {
                        host: format!("{host}:{port}"),
                        algorithm,
                        fingerprint,
                    });
                }
                SshEvent::SecretNeeded {
                    prompt,
                    kind,
                    user,
                    host,
                    port,
                } => self.on_secret_needed(prompt, kind, Target::ssh(&user, &host, port)),
                SshEvent::Connected => {
                    self.status = None;
                    self.after_login();
                }
                SshEvent::Error(e) => self.message(&format!("\x1b[31m{e}\x1b[0m")),
                SshEvent::Sftp(e) => self.on_sftp(e),
                SshEvent::Listening(_) => {}
                SshEvent::Closed { exit_status } => {
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
        changed
    }

    /// Auto-fill a saved password once per target; otherwise prompt in the
    /// terminal like PuTTY.
    fn on_secret_needed(&mut self, prompt: String, kind: SecretKind, target: Target) {
        match self.credentials.on_prompt(kind, &target) {
            PromptAction::AutoFill(password) => {
                if let Backend::Ssh(ssh) = &self.backend {
                    ssh.reply(Reply::Secret(password));
                }
                self.message(&format!(
                    "\x1b[90mUsing saved password for {}\x1b[0m",
                    target.key()
                ));
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
                self.sftp = SftpStatus::Ready(Box::new(SftpView {
                    cwd: home.clone(),
                    path_input: home.clone(),
                    follow,
                    ..SftpView::default()
                }));
                self.sftp_just_ready = true;
                self.sftp_list(&home);
            }
            SftpEvent::Unavailable(e) => self.sftp = SftpStatus::Unavailable(e),
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

    /// Browse to a remote directory.
    pub fn sftp_list(&mut self, path: &str) {
        if let (Backend::Ssh(ssh), SftpStatus::Ready(v)) = (&self.backend, &mut self.sftp) {
            v.listing = Some(ssh.sftp(SftpRequest::List(path.to_string())));
        }
    }

    /// Start a mutating SFTP operation (refreshes the listing when done).
    pub fn sftp_op(&mut self, request: SftpRequest) {
        let (Backend::Ssh(ssh), SftpStatus::Ready(v)) = (&self.backend, &mut self.sftp) else {
            return;
        };
        let (label, transfer) = match &request {
            SftpRequest::Download { remote, .. } => (format!("Download {remote}"), true),
            SftpRequest::Upload { local, .. } => (format!("Upload {}", local.display()), true),
            SftpRequest::Mkdir(p) => (format!("Create folder {p}"), false),
            SftpRequest::CreateFile(p) => (format!("Create file {p}"), false),
            SftpRequest::Remove { path, .. } => (format!("Delete {path}"), false),
            SftpRequest::Rename { from, .. } => (format!("Rename {from}"), false),
            SftpRequest::List(p) => (format!("List {p}"), false),
        };
        let id = ssh.sftp(request);
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
        match &mut self.backend {
            Backend::None => {}
            Backend::Pty(pty) => {
                let _ = pty.write(bytes);
            }
            Backend::Ssh(ssh) => ssh.write(bytes),
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
                            if let Backend::Ssh(ssh) = &self.backend {
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
