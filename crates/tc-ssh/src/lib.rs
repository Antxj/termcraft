//! Native SSH client built on `russh`.
//!
//! [`SshSession::connect`] (interactive shell) and [`SshSession::tunnel`]
//! (port forwarding) start a background thread with its own tokio runtime
//! and return immediately. The UI reads [`SshEvent`]s from a channel and
//! answers questions (unknown host key, password, key passphrase) with
//! [`SshSession::reply`]. Input and resizes are queued the same way, so the
//! UI thread never blocks on the network.
//!
//! Both kinds of session can go through an SSH gateway (jump host).

pub mod sftp;
pub mod tunnel;

pub use sftp::{RemoteEntry, SftpEvent, SftpRequest};
pub use tunnel::{TunnelParams, TunnelSpec};

use crossbeam_channel::{Receiver, Sender};
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, MethodKind};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc};

/// One SSH server to log in to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub private_key: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub private_key: Option<PathBuf>,
    /// SSH gateway (jump host) to go through, if any.
    pub gateway: Option<Endpoint>,
    /// OpenSSH-format known_hosts file used for host key verification.
    pub known_hosts: PathBuf,
    pub compression: bool,
    /// Run this command instead of an interactive shell.
    pub command: Option<String>,
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    /// Open the SFTP "SSH-browser" channel after login.
    pub sftp: bool,
    /// Send SSH keepalives at this interval.
    pub keepalive: Option<std::time::Duration>,
    /// Ask for the login shell on a separate exec channel and report it as
    /// `SshEvent::LoginShell` ("Follow terminal folder").
    pub detect_login_shell: bool,
    /// "Remote monitoring": report `SshEvent::Monitor` samples.
    pub remote_monitoring: bool,
}

impl ConnectParams {
    fn endpoint(&self) -> Endpoint {
        Endpoint {
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            private_key: self.private_key.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SshEvent {
    /// Terminal output (stdout and stderr).
    Output(Vec<u8>),
    /// Status line for the terminal ("Connecting…", "Authenticated").
    Info(String),
    /// The server key is not in known_hosts, or differs from the saved
    /// one; answer with `Reply::HostKey`.
    HostKey(Box<HostKeyInfo>),
    /// A secret is needed (password, keyboard-interactive or key
    /// passphrase) for `user@host:port`; answer with `Reply::Secret`.
    SecretNeeded {
        prompt: String,
        kind: SecretKind,
        user: String,
        host: String,
        port: u16,
        /// The key file, for `SecretKind::KeyPassphrase`.
        key_path: Option<String>,
    },
    Connected,
    /// A tunnel is accepting connections on this address.
    Listening(String),
    Error(String),
    Closed {
        exit_status: Option<u32>,
    },
    Sftp(SftpEvent),
    /// The account's login shell (`$SHELL`), empty if it could not be read.
    LoginShell(String),
    /// "Remote monitoring" reading (every 2 s while enabled).
    Monitor(Box<tc_core::monitor::Sample>),
}

/// Command run on a separate exec channel to find the login shell.
pub const LOGIN_SHELL_PROBE: &str = "echo \"$SHELL\"";

/// A server host key waiting for the user's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyInfo {
    pub host: String,
    pub port: u16,
    /// e.g. `ssh-ed25519`.
    pub algorithm: String,
    /// `SHA256:…` (base64, as OpenSSH prints it).
    pub sha256: String,
    /// Legacy `aa:bb:…` MD5 fingerprint.
    pub md5: String,
    /// The full public key, OpenSSH one-line format.
    pub public_key: String,
    /// known_hosts line of the *different* key saved for this host.
    pub changed_line: Option<usize>,
}

impl HostKeyInfo {
    fn new(host: &str, port: u16, key: &PublicKey, changed_line: Option<usize>) -> Self {
        let blob = key.to_bytes().unwrap_or_default();
        let md5 = md5::compute(&blob)
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":");
        Self {
            host: host.to_string(),
            port,
            algorithm: key.algorithm().to_string(),
            sha256: key.fingerprint(HashAlg::Sha256).to_string(),
            md5,
            public_key: key.to_openssh().unwrap_or_default(),
            changed_line,
        }
    }

    /// "You can verify the expected host key by pasting it below": does
    /// `text` (a fingerprint, with or without its prefix, or the public
    /// key) match this key?
    pub fn matches(&self, text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return false;
        }
        let sha = self.sha256.trim_start_matches("SHA256:");
        let t_sha = t.trim_start_matches("SHA256:");
        let md5 = t.trim_start_matches("MD5:").to_ascii_lowercase();
        let key_body = |k: &str| k.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        t_sha == sha || md5 == self.md5 || key_body(t) == key_body(&self.public_key)
    }
}

/// Remove line `line` (1-based) from a known_hosts file.
fn forget_known_host_line(path: &std::path::Path, line: usize) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let kept: Vec<&str> = text
        .lines()
        .enumerate()
        .filter(|(i, _)| i + 1 != line)
        .map(|(_, l)| l)
        .collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    std::fs::write(path, out)
}

/// What a `SecretNeeded` prompt is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// Login password ("password" auth method). Can be saved and auto-filled.
    Password,
    /// Passphrase of a private key file.
    KeyPassphrase,
    /// A keyboard-interactive (PAM / OTP) prompt.
    Challenge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    HostKey { accept: bool, remember: bool },
    Secret(String),
    Cancel,
}

enum Input {
    Data(Vec<u8>),
    Resize(u16, u16),
    Sftp(u64, SftpRequest),
    /// Start or stop "Remote monitoring".
    Monitoring(bool),
    /// Reopen the SSH-browser with or without sudo.
    SftpSudo {
        on: bool,
        password: Option<zeroize::Zeroizing<String>>,
    },
    Close,
}

pub struct SshSession {
    input: mpsc::UnboundedSender<Input>,
    replies: mpsc::UnboundedSender<Reply>,
    events: Receiver<SshEvent>,
    next_request: AtomicU64,
}

impl SshSession {
    /// Interactive shell (or `command`) with an optional SFTP browser.
    pub fn connect(params: ConnectParams, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let name = format!("ssh {}", params.host);
        Self::spawn(name, notify, move |input, replies, events| {
            run_shell(params, input, replies, events)
        })
    }

    /// Port forwarding session (no terminal).
    pub fn tunnel(params: TunnelParams, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let name = format!("tunnel {}", params.ssh.host);
        Self::spawn(name, notify, move |input, replies, events| async move {
            tunnel::run(params, input, replies, events)
                .await
                .map(|()| None)
        })
    }

    fn spawn<F, Fut>(name: String, notify: Arc<dyn Fn() + Send + Sync>, body: F) -> Self
    where
        F: FnOnce(mpsc::UnboundedReceiver<Input>, Replies, Events) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<u32>, String>>,
    {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (reply_tx, reply_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = crossbeam_channel::unbounded();
        let events_out = Events {
            tx: event_tx,
            notify,
        };
        std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => return events_out.send(SshEvent::Error(e.to_string())),
                };
                let replies = Arc::new(Mutex::new(reply_rx));
                let result = runtime.block_on(body(input_rx, replies, events_out.clone()));
                match result {
                    Ok(exit_status) => events_out.send(SshEvent::Closed { exit_status }),
                    Err(e) => {
                        events_out.send(SshEvent::Error(e));
                        events_out.send(SshEvent::Closed { exit_status: None });
                    }
                }
            })
            .expect("spawn ssh thread");
        Self {
            input: input_tx,
            replies: reply_tx,
            events,
            next_request: AtomicU64::new(1),
        }
    }

    pub fn events(&self) -> &Receiver<SshEvent> {
        &self.events
    }

    pub fn write(&self, bytes: &[u8]) {
        let _ = self.input.send(Input::Data(bytes.to_vec()));
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        let _ = self.input.send(Input::Resize(cols, rows));
    }

    /// Queue an SFTP request; the returned id tags its result events.
    pub fn sftp(&self, request: SftpRequest) -> u64 {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let _ = self.input.send(Input::Sftp(id, request));
        id
    }

    /// Start or stop "Remote monitoring" on this connection.
    pub fn set_monitoring(&self, on: bool) {
        let _ = self.input.send(Input::Monitoring(on));
    }

    /// "Use SUDO for SSH-browser": reopen the browser elevated (or not).
    /// Answers with `SftpEvent::Ready`, `SudoPasswordNeeded` or `SudoFailed`.
    pub fn sftp_sudo(&self, on: bool, password: Option<String>) {
        let _ = self.input.send(Input::SftpSudo {
            on,
            password: password.map(zeroize::Zeroizing::new),
        });
    }

    pub fn reply(&self, reply: Reply) {
        let _ = self.replies.send(reply);
    }

    pub fn close(&self) {
        let _ = self.input.send(Input::Close);
        let _ = self.replies.send(Reply::Cancel);
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Clone)]
pub(crate) struct Events {
    tx: Sender<SshEvent>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Events {
    pub(crate) fn send(&self, event: SshEvent) {
        let _ = self.tx.send(event);
        (self.notify)();
    }
}

pub(crate) type Replies = Arc<Mutex<mpsc::UnboundedReceiver<Reply>>>;

async fn next_reply(replies: &Replies) -> Reply {
    replies.lock().await.recv().await.unwrap_or(Reply::Cancel)
}

async fn ask_secret(
    events: &Events,
    replies: &Replies,
    endpoint: &Endpoint,
    prompt: String,
    kind: SecretKind,
) -> Option<String> {
    events.send(SshEvent::SecretNeeded {
        prompt,
        kind,
        user: endpoint.user.clone(),
        host: endpoint.host.clone(),
        port: endpoint.port,
        key_path: (kind == SecretKind::KeyPassphrase)
            .then(|| {
                endpoint
                    .private_key
                    .as_ref()
                    .map(|p| p.display().to_string())
            })
            .flatten(),
    });
    loop {
        match next_reply(replies).await {
            Reply::Secret(s) => return Some(s),
            Reply::Cancel => return None,
            Reply::HostKey { .. } => {}
        }
    }
}

/// Remote forwards: port bound on the server → local `host:port` to reach.
pub(crate) type RemoteForwards = Arc<std::sync::Mutex<HashMap<u32, (String, u16)>>>;

pub(crate) struct Handler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    events: Events,
    replies: Replies,
    forwards: RemoteForwards,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            self.events.send(SshEvent::Error(
                "SSH host certificates are not supported yet".into(),
            ));
            return Ok(false);
        };
        match keys::check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
            Ok(true) => Ok(true),
            // A different key is saved: warn, and only go on if the user
            // explicitly accepts (PuTTY "POTENTIAL SECURITY BREACH").
            Err(keys::Error::KeyChanged { line }) => Ok(self.ask_host_key(key, Some(line)).await),
            // Not recorded yet (or no known_hosts file): ask the user.
            Ok(false) | Err(_) => Ok(self.ask_host_key(key, None).await),
        }
    }

    /// A connection arrived on a remote forward: connect locally and bridge.
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let target = self
            .forwards
            .lock()
            .expect("lock")
            .get(&connected_port)
            .cloned();
        let Some((host, port)) = target else {
            drop(reply); // rejects
            return Ok(());
        };
        reply.accept().await;
        let events = self.events.clone();
        let from = originator_address.to_string();
        tokio::spawn(async move {
            match tokio::net::TcpStream::connect((host.as_str(), port)).await {
                Ok(mut tcp) => {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                }
                Err(e) => events.send(SshEvent::Info(format!(
                    "remote forward from {from}: cannot reach {host}:{port}: {e}"
                ))),
            }
        });
        Ok(())
    }
}

impl Handler {
    async fn ask_host_key(&mut self, key: &PublicKey, changed_line: Option<usize>) -> bool {
        let info = HostKeyInfo::new(&self.host, self.port, key, changed_line);
        self.events.send(SshEvent::HostKey(Box::new(info)));
        loop {
            match next_reply(&self.replies).await {
                Reply::HostKey { accept, remember } => {
                    if accept && remember {
                        if let Some(dir) = self.known_hosts.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        // Replace the old key rather than adding a second one.
                        if let Some(line) = changed_line
                            && let Err(e) = forget_known_host_line(&self.known_hosts, line)
                        {
                            self.events
                                .send(SshEvent::Info(format!("could not update known_hosts: {e}")));
                            return false;
                        }
                        if let Err(e) = keys::known_hosts::learn_known_hosts_path(
                            &self.host,
                            self.port,
                            key,
                            &self.known_hosts,
                        ) {
                            self.events
                                .send(SshEvent::Info(format!("could not save host key: {e}")));
                        }
                    }
                    return accept;
                }
                Reply::Cancel => return false,
                Reply::Secret(_) => {}
            }
        }
    }
}

/// An authenticated connection; the gateway session (if any) is kept alive
/// for as long as the target session is used.
pub(crate) struct Connection {
    pub(crate) handle: client::Handle<Handler>,
    pub(crate) forwards: RemoteForwards,
    _gateway: Option<client::Handle<Handler>>,
}

pub(crate) struct ConnectOptions<'a> {
    pub target: &'a Endpoint,
    pub gateway: Option<&'a Endpoint>,
    pub known_hosts: &'a std::path::Path,
    pub compression: bool,
    pub keepalive: Option<std::time::Duration>,
}

fn client_config(compression: bool, keepalive: Option<std::time::Duration>) -> Arc<client::Config> {
    let mut config = client::Config {
        keepalive_interval: keepalive,
        ..client::Config::default()
    };
    if compression {
        config.preferred.compression = std::borrow::Cow::Borrowed(&[
            russh::compression::ZLIB_LEGACY,
            russh::compression::ZLIB,
            russh::compression::NONE,
        ]);
    }
    Arc::new(config)
}

/// Connect (directly or through the gateway) and authenticate.
pub(crate) async fn establish(
    opts: ConnectOptions<'_>,
    replies: &Replies,
    events: &Events,
) -> Result<Connection, String> {
    let forwards: RemoteForwards = Arc::default();
    let handler = |e: &Endpoint| Handler {
        host: e.host.clone(),
        port: e.port,
        known_hosts: opts.known_hosts.to_path_buf(),
        events: events.clone(),
        replies: replies.clone(),
        forwards: forwards.clone(),
    };
    let target = opts.target;
    let config = client_config(opts.compression, opts.keepalive);

    let (mut handle, gateway) = match opts.gateway {
        None => {
            events.send(SshEvent::Info(format!(
                "Connecting to {}:{} …",
                target.host, target.port
            )));
            let handle =
                client::connect(config, (target.host.as_str(), target.port), handler(target))
                    .await
                    .map_err(|e| format!("connection failed: {e}"))?;
            (handle, None)
        }
        Some(gw) => {
            events.send(SshEvent::Info(format!(
                "Connecting to SSH gateway {}:{} …",
                gw.host, gw.port
            )));
            let mut gw_handle = client::connect(
                client_config(false, opts.keepalive),
                (gw.host.as_str(), gw.port),
                handler(gw),
            )
            .await
            .map_err(|e| format!("gateway connection failed: {e}"))?;
            authenticate(&mut gw_handle, gw, replies, events).await?;
            events.send(SshEvent::Info(format!(
                "Connecting to {}:{} through the gateway …",
                target.host, target.port
            )));
            let channel = gw_handle
                .channel_open_direct_tcpip(
                    target.host.clone(),
                    u32::from(target.port),
                    "127.0.0.1",
                    0,
                )
                .await
                .map_err(|e| {
                    format!("gateway cannot reach {}:{}: {e}", target.host, target.port)
                })?;
            let handle = client::connect_stream(config, channel.into_stream(), handler(target))
                .await
                .map_err(|e| format!("connection through gateway failed: {e}"))?;
            (handle, Some(gw_handle))
        }
    };
    authenticate(&mut handle, target, replies, events).await?;
    Ok(Connection {
        handle,
        forwards,
        _gateway: gateway,
    })
}

type SftpOpened = (Arc<russh_sftp::client::SftpSession>, String);

async fn run_shell(
    params: ConnectParams,
    mut input: mpsc::UnboundedReceiver<Input>,
    replies: Replies,
    events: Events,
) -> Result<Option<u32>, String> {
    let target = params.endpoint();
    let conn = establish(
        ConnectOptions {
            target: &target,
            gateway: params.gateway.as_ref(),
            known_hosts: &params.known_hosts,
            compression: params.compression,
            keepalive: params.keepalive,
        },
        &replies,
        &events,
    )
    .await?;
    let session = &conn.handle;
    events.send(SshEvent::Connected);

    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("cannot open session channel: {e}"))?;
    channel
        .request_pty(
            false,
            &params.term,
            u32::from(params.cols),
            u32::from(params.rows),
            0,
            0,
            &[],
        )
        .await
        .map_err(|e| format!("pty request failed: {e}"))?;
    match &params.command {
        Some(cmd) => channel.exec(true, cmd.as_str()).await,
        None => channel.request_shell(true).await,
    }
    .map_err(|e| format!("cannot start shell: {e}"))?;

    let open_sftp = async {
        if !params.sftp {
            return None;
        }
        match sftp::open(session).await {
            Ok((sftp, home)) => {
                events.send(SshEvent::Sftp(SftpEvent::Ready { home }));
                Some(sftp)
            }
            Err(e) => {
                events.send(SshEvent::Sftp(SftpEvent::Unavailable(e)));
                None
            }
        }
    };
    let probe = async {
        if params.detect_login_shell && params.command.is_none() {
            let shell = login_shell(session).await.unwrap_or_default();
            events.send(SshEvent::LoginShell(shell));
        }
    };
    let (mut sftp, ()) = tokio::join!(open_sftp, probe);
    // A sudo switch in progress; the old browser keeps working meanwhile.
    type Reopen<'a> = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SftpOpened, sftp::SudoError>> + Send + 'a>,
    >;
    let mut reopening: Option<Reopen<'_>> = None;
    let mut monitor = if params.remote_monitoring {
        open_monitor(session).await
    } else {
        None
    };

    let mut exit_status = None;
    loop {
        tokio::select! {
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    events.send(SshEvent::Output(data.to_vec()));
                }
                Some(ChannelMsg::ExitStatus { exit_status: code }) => exit_status = Some(code),
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            cmd = input.recv() => match cmd {
                Some(Input::Data(bytes)) => {
                    if channel.data(&bytes[..]).await.is_err() {
                        break;
                    }
                }
                Some(Input::Resize(cols, rows)) => {
                    let _ = channel.window_change(u32::from(cols), u32::from(rows), 0, 0).await;
                }
                Some(Input::Sftp(id, request)) => match &sftp {
                    Some(s) => {
                        tokio::spawn(sftp::execute(s.clone(), id, request, events.clone()));
                    }
                    None => events.send(SshEvent::Sftp(SftpEvent::Failed {
                        id,
                        error: "SFTP is not available on this connection".into(),
                    })),
                },
                Some(Input::Monitoring(on)) => {
                    if !on {
                        if let Some((ch, _)) = monitor.take() {
                            let _ = ch.close().await;
                        }
                    } else if monitor.is_none() {
                        monitor = open_monitor(session).await;
                    }
                }
                Some(Input::SftpSudo { on, password }) => {
                    reopening = Some(Box::pin(async move {
                        if on {
                            sftp::open_sudo(session, password.as_deref().map(String::as_str)).await
                        } else {
                            sftp::open(session).await.map_err(sftp::SudoError::Failed)
                        }
                    }));
                }
                Some(Input::Close) | None => break,
            },
            msg = async { monitor.as_mut().expect("guarded").0.wait().await }, if monitor.is_some() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        let (_, parser) = monitor.as_mut().expect("guarded");
                        for sample in parser.feed(&data, std::time::Instant::now()) {
                            events.send(SshEvent::Monitor(Box::new(sample)));
                        }
                    }
                    Some(ChannelMsg::Eof | ChannelMsg::Close) | None => monitor = None,
                    Some(_) => {}
                }
            },
            opened = async { reopening.as_mut().expect("guarded").await }, if reopening.is_some() => {
                reopening = None;
                match opened {
                    Ok((s, home)) => {
                        sftp = Some(s);
                        events.send(SshEvent::Sftp(SftpEvent::Ready { home }));
                    }
                    Err(sftp::SudoError::PasswordNeeded) => {
                        events.send(SshEvent::Sftp(SftpEvent::SudoPasswordNeeded));
                    }
                    Err(sftp::SudoError::Failed(e)) => {
                        events.send(SshEvent::Sftp(SftpEvent::SudoFailed(e)));
                    }
                }
            },
        }
    }
    let _ = session
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    Ok(exit_status)
}

/// Start the "Remote monitoring" script on its own channel (under `sh`,
/// whatever the login shell is).
async fn open_monitor(
    session: &client::Handle<Handler>,
) -> Option<(russh::Channel<client::Msg>, tc_core::monitor::MonitorParser)> {
    let channel = session.channel_open_session().await.ok()?;
    let script = tc_core::monitor::SCRIPT.replace('\'', "'\\''");
    channel.exec(true, format!("sh -c '{script}'")).await.ok()?;
    Some((channel, Default::default()))
}

/// Run [`LOGIN_SHELL_PROBE`] on its own channel; the last non-empty line.
async fn login_shell(session: &client::Handle<Handler>) -> Option<String> {
    let mut channel = session.channel_open_session().await.ok()?;
    channel.exec(true, LOGIN_SHELL_PROBE).await.ok()?;
    let mut out = Vec::new();
    let read = async {
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.extend_from_slice(&data),
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
            if out.len() > 4096 {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), read).await;
    let _ = channel.close().await;
    String::from_utf8_lossy(&out)
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(String::from)
}

async fn authenticate(
    session: &mut client::Handle<Handler>,
    endpoint: &Endpoint,
    replies: &Replies,
    events: &Events,
) -> Result<(), String> {
    let user = endpoint.user.clone();
    let err = |e: russh::Error| format!("authentication error: {e}");

    if let Some(path) = &endpoint.private_key {
        let key = match keys::load_secret_key(path, None) {
            Ok(k) => Some(k),
            Err(keys::Error::KeyIsEncrypted) => {
                let prompt = format!("Passphrase for key {}:", path.display());
                match ask_secret(events, replies, endpoint, prompt, SecretKind::KeyPassphrase).await
                {
                    Some(pass) => keys::load_secret_key(path, Some(&pass))
                        .map_err(|e| format!("cannot load key {}: {e}", path.display()))
                        .map(Some)?,
                    None => return Err("cancelled".into()),
                }
            }
            Err(e) => {
                events.send(SshEvent::Info(format!(
                    "cannot load key {}: {e}",
                    path.display()
                )));
                None
            }
        };
        if let Some(key) = key {
            let hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(err)?
                .flatten();
            let result = session
                .authenticate_publickey(
                    user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
                .map_err(err)?;
            if result.success() {
                return Ok(());
            }
            events.send(SshEvent::Info("Server refused our key".into()));
        }
    }

    for _attempt in 0..3 {
        let prompt = format!("{}@{}'s password:", user, endpoint.host);
        let Some(password) =
            ask_secret(events, replies, endpoint, prompt, SecretKind::Password).await
        else {
            return Err("cancelled".into());
        };
        match session
            .authenticate_password(user.clone(), password.clone())
            .await
            .map_err(err)?
        {
            russh::client::AuthResult::Success => return Ok(()),
            russh::client::AuthResult::Failure {
                remaining_methods, ..
            } => {
                if remaining_methods.contains(&MethodKind::KeyboardInteractive)
                    && keyboard_interactive(session, endpoint, Some(password), replies, events)
                        .await?
                {
                    return Ok(());
                }
                events.send(SshEvent::Output(b"Access denied\r\n".to_vec()));
            }
        }
    }
    Err("authentication failed".into())
}

/// Keyboard-interactive (PAM-style) login; the first hidden prompt reuses
/// the password the user already typed.
async fn keyboard_interactive(
    session: &mut client::Handle<Handler>,
    endpoint: &Endpoint,
    mut password: Option<String>,
    replies: &Replies,
    events: &Events,
) -> Result<bool, String> {
    let err = |e: russh::Error| format!("authentication error: {e}");
    let mut response = session
        .authenticate_keyboard_interactive_start(endpoint.user.as_str(), None)
        .await
        .map_err(err)?;
    loop {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let mut answers = Vec::with_capacity(prompts.len());
                for p in prompts {
                    let answer = match (p.echo, password.take()) {
                        (false, Some(pw)) => pw,
                        _ => match ask_secret(
                            events,
                            replies,
                            endpoint,
                            p.prompt.clone(),
                            SecretKind::Challenge,
                        )
                        .await
                        {
                            Some(s) => s,
                            None => return Err("cancelled".into()),
                        },
                    };
                    answers.push(answer);
                }
                response = session
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(err)?;
            }
        }
    }
}

#[cfg(test)]
mod host_key_tests {
    use super::*;

    fn info() -> HostKeyInfo {
        let key = russh::keys::PrivateKey::from(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]),
        );
        HostKeyInfo::new("h", 22, key.public_key(), None)
    }

    #[test]
    fn fingerprints_and_paste_check() {
        let i = info();
        assert!(i.sha256.starts_with("SHA256:"));
        assert_eq!(i.md5.split(':').count(), 16);
        assert!(i.public_key.starts_with("ssh-ed25519 "));
        assert!(i.matches(&i.sha256));
        assert!(i.matches(i.sha256.trim_start_matches("SHA256:")));
        assert!(i.matches(&i.md5.to_uppercase()));
        assert!(i.matches(&format!("{} comment", i.public_key)));
        assert!(!i.matches("SHA256:AAAA"));
        assert!(!i.matches("  "));
    }

    #[test]
    fn forget_line_keeps_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("known_hosts");
        std::fs::write(&p, "a key1\nb key2\nc key3\n").unwrap();
        forget_known_host_line(&p, 2).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a key1\nc key3\n");
    }
}
