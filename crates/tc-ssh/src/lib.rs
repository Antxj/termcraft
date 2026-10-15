//! Native SSH client built on `russh`.
//!
//! [`SshSession::connect`] (interactive shell) and [`SshSession::tunnel`]
//! (port forwarding) start a background thread with its own tokio runtime
//! and return immediately. The UI reads [`SshEvent`]s from a channel and
//! answers questions (unknown host key, password, key passphrase) with
//! [`SshSession::reply`]. Input and resizes are queued the same way, so the
//! UI thread never blocks on the network.
//!
//! Both kinds of session can go through one or several SSH gateways
//! (jump hosts).

pub mod agent;
pub mod keygen;
pub mod proxy;
pub mod scp;
pub mod sftp;
pub mod tunnel;
pub mod x11;

pub use agent::AgentConfig;
pub use russh::keys::PrivateKey;
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
    /// SSH gateways (jump hosts) to go through, in order.
    pub gateways: Vec<Endpoint>,
    /// OpenSSH-format known_hosts file used for host key verification.
    pub known_hosts: PathBuf,
    pub compression: bool,
    /// Run this command instead of an interactive shell.
    pub command: Option<String>,
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    /// Open the "SSH-browser" after login.
    pub sftp: bool,
    /// 3_26 "SSH-browser type": SFTP (falls back to SCP when the server
    /// has no SFTP subsystem) or SCP.
    pub browser_protocol: BrowserProtocol,
    /// Send SSH keepalives at this interval.
    pub keepalive: Option<std::time::Duration>,
    /// Ask for the login shell on a separate exec channel and report it as
    /// `SshEvent::LoginShell` ("Follow terminal folder").
    pub detect_login_shell: bool,
    /// "Remote monitoring": report `SshEvent::Monitor` samples.
    pub remote_monitoring: bool,
    /// SSH agents to try for public-key login.
    pub agent: AgentConfig,
    /// Open a terminal (shell or command). `false` = SFTP session: only
    /// the file browser.
    pub shell: bool,
    /// Proxy for the first hop (session "Network settings").
    pub proxy: Option<proxy::Proxy>,
    /// 3_35 "Allow agent forwarding".
    pub agent_forwarding: bool,
    /// 3_6 "X11-Forwarding": the local display to forward to.
    pub x11_display: Option<String>,
    /// 3_32 "Disconnect if authentication succeeds trivially": a server
    /// that lets us in without any password, signature or answer is left.
    pub refuse_trivial_auth: bool,
    /// "Display SSH banner".
    pub show_banner: bool,
    /// "Preserve files dates during SSH-browser transfers" (SFTP).
    pub preserve_dates: bool,
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
    /// "Allow agent forwarding": where forwarded agent channels go. `None`
    /// = such channels are refused.
    agent_forward: Option<AgentConfig>,
    /// X11 forwarding: `None` = X11 channels are refused.
    x11: Option<Arc<x11::X11State>>,
    /// "Display SSH banner".
    banner: bool,
}

impl client::Handler for Handler {
    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if self.banner {
            // Shown as text: no control sequences from the server here.
            let text: String = banner
                .chars()
                .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                .take(8192)
                .collect();
            let text = text.trim_end().replace('\n', "\r\n");
            self.events
                .send(SshEvent::Output(format!("{text}\r\n").into_bytes()));
        }
        Ok(())
    }

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

    /// A program on the server opens an X11 window: bridge it to the local
    /// X server after checking the fake cookie.
    async fn server_channel_open_x11(
        &mut self,
        channel: russh::Channel<client::Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let Some(state) = self.x11.clone() else {
            drop(reply); // refuses
            return Ok(());
        };
        reply.accept().await;
        let events = self.events.clone();
        tokio::spawn(async move {
            if let Err(e) = x11::bridge(channel.into_stream(), &state).await {
                events.send(SshEvent::Info(format!("X11: {e}")));
            }
        });
        Ok(())
    }

    /// The server opens an agent channel (ssh / git there use our keys):
    /// bridge it to the local agent, but only when the session asked for
    /// agent forwarding.
    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let Some(config) = self.agent_forward.clone() else {
            drop(reply); // refuses
            return Ok(());
        };
        reply.accept().await;
        let events = self.events.clone();
        tokio::spawn(async move {
            match agent::connect_raw(&config).await {
                Some(mut local) => {
                    let mut remote = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut remote, &mut local).await;
                }
                None => events.send(SshEvent::Info(
                    "agent forwarding: no local SSH agent is running".into(),
                )),
            }
        });
        Ok(())
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

/// 3_26 "SSH-browser type" (besides "None").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserProtocol {
    #[default]
    Sftp,
    /// "SCP (enhanced speed)" (bigger blocks) / "SCP (normal speed)".
    Scp { enhanced: bool },
}

/// The open SSH-browser.
enum Browser {
    Sftp(Arc<russh_sftp::client::SftpSession>),
    Scp { chunk: usize },
}

const SCP_CHUNK_ENHANCED: usize = 256 * 1024;
const SCP_CHUNK_NORMAL: usize = 32 * 1024;

/// An authenticated connection; the gateway sessions (if any) are kept
/// alive for as long as the target session is used.
pub(crate) struct Connection {
    pub(crate) handle: client::Handle<Handler>,
    pub(crate) forwards: RemoteForwards,
    _gateways: Vec<client::Handle<Handler>>,
}

pub(crate) struct ConnectOptions<'a> {
    pub target: &'a Endpoint,
    pub gateways: &'a [Endpoint],
    pub known_hosts: &'a std::path::Path,
    pub compression: bool,
    pub keepalive: Option<std::time::Duration>,
    pub agent: AgentConfig,
    pub proxy: Option<&'a proxy::Proxy>,
    /// Bridge forwarded agent channels of the target server.
    pub agent_forward: bool,
    /// X11 forwarding state of the target connection.
    pub x11: Option<Arc<x11::X11State>>,
    /// See [`ConnectParams::refuse_trivial_auth`].
    pub refuse_trivial_auth: bool,
    /// Show the target server's banner before login.
    pub show_banner: bool,
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

/// TCP to the first SSH server, directly or through the session proxy.
async fn first_hop(
    config: Arc<client::Config>,
    to: &Endpoint,
    handler: Handler,
    proxy: Option<&proxy::Proxy>,
    replies: &Replies,
    events: &Events,
) -> Result<client::Handle<Handler>, String> {
    let Some(p) = proxy else {
        return client::connect(config, (to.host.as_str(), to.port), handler)
            .await
            .map_err(|e| e.to_string());
    };
    events.send(SshEvent::Info(format!(
        "Connecting through proxy {}:{} …",
        p.host, p.port
    )));
    let proxy_ep = Endpoint {
        host: p.host.clone(),
        port: p.port,
        user: p.user.clone().unwrap_or_default(),
        private_key: None,
    };
    let (events2, replies2) = (events.clone(), replies.clone());
    let mut ask = move || -> proxy::PasswordFuture {
        let (e, r, ep) = (events2.clone(), replies2.clone(), proxy_ep.clone());
        Box::pin(async move {
            let prompt = format!("Password for proxy {}@{}:", ep.user, ep.host);
            ask_secret(&e, &r, &ep, prompt, SecretKind::Password).await
        })
    };
    let stream = proxy::connect(p, &to.host, to.port, &mut ask)
        .await
        .map_err(|e| e.to_string())?;
    client::connect_stream(config, stream, handler)
        .await
        .map_err(|e| e.to_string())
}

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
        agent_forward: None,
        x11: None,
        banner: false,
    };
    let target_handler = |e: &Endpoint| Handler {
        banner: opts.show_banner,
        agent_forward: opts.agent_forward.then(|| opts.agent.clone()),
        x11: opts.x11.clone(),
        ..handler(e)
    };
    let target = opts.target;
    let config = client_config(opts.compression, opts.keepalive);

    // Each gateway is reached through the previous one (the first one
    // directly or through the proxy), then the target through the last.
    let mut chain: Vec<client::Handle<Handler>> = Vec::with_capacity(opts.gateways.len() + 1);
    // An SSH server as proxy is the very first hop; with "SSH command" the
    // next hop goes through that command instead of a forwarding channel.
    let (ssh_proxy, proxy) = match opts.proxy {
        Some(p)
            if matches!(
                p.kind,
                proxy::ProxyKind::SshForward | proxy::ProxyKind::SshCommand(_)
            ) =>
        {
            (Some(p), None)
        }
        other => (None, other),
    };
    let mut via_command = None;
    if let Some(p) = ssh_proxy {
        let ep = Endpoint {
            host: p.host.clone(),
            port: p.port,
            user: p.user.clone().unwrap_or_default(),
            private_key: None,
        };
        events.send(SshEvent::Info(format!(
            "Connecting to SSH proxy {}:{} …",
            ep.host, ep.port
        )));
        let mut proxy_handle = open_hop(
            None,
            None,
            client_config(false, opts.keepalive),
            &ep,
            handler(&ep),
            None,
            replies,
            events,
        )
        .await
        .map_err(|e| format!("SSH proxy connection failed: {e}"))?;
        authenticate(
            &mut proxy_handle,
            &ep,
            &opts.agent,
            opts.refuse_trivial_auth,
            replies,
            events,
        )
        .await?;
        chain.push(proxy_handle);
        if let proxy::ProxyKind::SshCommand(cmd) = &p.kind {
            via_command = Some(cmd.as_str());
        }
    }
    let count = opts.gateways.len();
    for (i, gw) in opts.gateways.iter().enumerate() {
        let which = if count > 1 {
            format!("SSH gateway {}/{count}", i + 1)
        } else {
            "SSH gateway".to_string()
        };
        events.send(SshEvent::Info(format!(
            "Connecting to {which} {}:{} …",
            gw.host, gw.port
        )));
        let mut gw_handle = open_hop(
            chain.last(),
            via_command.take(),
            client_config(false, opts.keepalive),
            gw,
            handler(gw),
            proxy,
            replies,
            events,
        )
        .await
        .map_err(|e| format!("{which} connection failed: {e}"))?;
        authenticate(
            &mut gw_handle,
            gw,
            &opts.agent,
            opts.refuse_trivial_auth,
            replies,
            events,
        )
        .await?;
        chain.push(gw_handle);
    }
    events.send(SshEvent::Info(if chain.is_empty() {
        format!("Connecting to {}:{} …", target.host, target.port)
    } else {
        format!(
            "Connecting to {}:{} through the gateway …",
            target.host, target.port
        )
    }));
    let mut handle = open_hop(
        chain.last(),
        via_command.take(),
        config,
        target,
        target_handler(target),
        proxy,
        replies,
        events,
    )
    .await
    .map_err(|e| {
        if chain.is_empty() {
            format!("connection failed: {e}")
        } else {
            format!("connection through gateway failed: {e}")
        }
    })?;
    authenticate(
        &mut handle,
        target,
        &opts.agent,
        opts.refuse_trivial_auth,
        replies,
        events,
    )
    .await?;
    Ok(Connection {
        handle,
        forwards,
        _gateways: chain,
    })
}

/// A host / port that is safe in a command run on an SSH proxy.
fn command_safe(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_:[]".contains(c))
}

/// One SSH connection: the first one directly (or through the proxy),
/// the next ones through a direct-tcpip channel of the previous gateway
/// (or, after an "SSH command" proxy, through that command's channel).
#[allow(clippy::too_many_arguments)]
async fn open_hop(
    via: Option<&client::Handle<Handler>>,
    command: Option<&str>,
    config: Arc<client::Config>,
    to: &Endpoint,
    handler: Handler,
    proxy: Option<&proxy::Proxy>,
    replies: &Replies,
    events: &Events,
) -> Result<client::Handle<Handler>, String> {
    let Some(gateway) = via else {
        return first_hop(config, to, handler, proxy, replies, events).await;
    };
    if let Some(template) = command {
        if !command_safe(&to.host) {
            return Err(format!(
                "{:?} cannot be passed to the proxy command",
                to.host
            ));
        }
        let line = proxy::substitute(template, &to.host, to.port, Some(&to.user));
        let channel = gateway
            .channel_open_session()
            .await
            .map_err(|e| format!("SSH proxy: {e}"))?;
        channel
            .exec(true, line.as_bytes())
            .await
            .map_err(|e| format!("SSH proxy command {line:?}: {e}"))?;
        return client::connect_stream(config, channel.into_stream(), handler)
            .await
            .map_err(|e| e.to_string());
    }
    let channel = gateway
        .channel_open_direct_tcpip(to.host.clone(), u32::from(to.port), "127.0.0.1", 0)
        .await
        .map_err(|e| format!("gateway cannot reach {}:{}: {e}", to.host, to.port))?;
    client::connect_stream(config, channel.into_stream(), handler)
        .await
        .map_err(|e| e.to_string())
}

type SftpOpened = (Arc<russh_sftp::client::SftpSession>, String);

async fn run_shell(
    params: ConnectParams,
    mut input: mpsc::UnboundedReceiver<Input>,
    replies: Replies,
    events: Events,
) -> Result<Option<u32>, String> {
    let x11 = params
        .x11_display
        .as_deref()
        .filter(|_| params.shell)
        .and_then(x11::X11State::new)
        .map(Arc::new);
    let target = params.endpoint();
    let conn = establish(
        ConnectOptions {
            target: &target,
            gateways: &params.gateways,
            known_hosts: &params.known_hosts,
            compression: params.compression,
            keepalive: params.keepalive,
            agent: params.agent.clone(),
            proxy: params.proxy.as_ref(),
            agent_forward: params.agent_forwarding,
            x11: x11.clone(),
            refuse_trivial_auth: params.refuse_trivial_auth,
            show_banner: params.show_banner,
        },
        &replies,
        &events,
    )
    .await?;
    let session = &conn.handle;
    events.send(SshEvent::Connected);

    let mut channel = if params.shell {
        let channel = session
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
        if params.agent_forwarding {
            let _ = channel.agent_forward(false).await;
        }
        if let Some(x) = &x11 {
            let _ = channel
                .request_x11(false, false, x.protocol(), x.fake_hex(), x.display.screen)
                .await;
        }
        match &params.command {
            Some(cmd) => channel.exec(true, cmd.as_str()).await,
            None => channel.request_shell(true).await,
        }
        .map_err(|e| format!("cannot start shell: {e}"))?;
        Some(channel)
    } else {
        None
    };

    let open_sftp = async {
        if !params.sftp {
            return None;
        }
        let scp = |enhanced: bool| async move {
            let chunk = if enhanced {
                SCP_CHUNK_ENHANCED
            } else {
                SCP_CHUNK_NORMAL
            };
            scp::open(session)
                .await
                .map(|home| (Browser::Scp { chunk }, home))
        };
        let opened = match params.browser_protocol {
            BrowserProtocol::Scp { enhanced } => scp(enhanced).await,
            BrowserProtocol::Sftp => match sftp::open(session).await {
                Ok((s, home)) => Ok((Browser::Sftp(s), home)),
                // No SFTP subsystem: browse with SCP instead.
                Err(e) => match scp(true).await {
                    Ok(found) => {
                        events.send(SshEvent::Info(format!("{e}; the browser uses SCP")));
                        Ok(found)
                    }
                    Err(_) => Err(e),
                },
            },
        };
        match opened {
            Ok((browser, home)) => {
                events.send(SshEvent::Sftp(SftpEvent::Ready { home }));
                Some(browser)
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
    if channel.is_none() && sftp.is_none() {
        // An SFTP session without SFTP has nothing to show.
        let _ = session
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
        return Ok(None);
    }
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
    // Servers (OpenSSH among them) send EOF first, then the exit status:
    // after EOF, wait a little for it.
    let mut eof = false;
    loop {
        tokio::select! {
            msg = async { channel.as_mut().expect("guarded").wait().await }, if channel.is_some() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    events.send(SshEvent::Output(data.to_vec()));
                }
                Some(ChannelMsg::ExitStatus { exit_status: code }) => {
                    exit_status = Some(code);
                    if eof {
                        break;
                    }
                }
                Some(ChannelMsg::Eof) => {
                    eof = true;
                    if exit_status.is_some() {
                        break;
                    }
                }
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            () = tokio::time::sleep(std::time::Duration::from_secs(2)), if eof => break,
            cmd = input.recv() => match cmd {
                Some(Input::Data(bytes)) => {
                    if let Some(channel) = &channel
                        && channel.data(&bytes[..]).await.is_err()
                    {
                        break;
                    }
                }
                Some(Input::Resize(cols, rows)) => {
                    if let Some(channel) = &channel {
                        let _ = channel.window_change(u32::from(cols), u32::from(rows), 0, 0).await;
                    }
                }
                Some(Input::Sftp(id, request)) => match &sftp {
                    Some(Browser::Sftp(s)) => {
                        let transfer = sftp::Transfer { preserve_dates: params.preserve_dates };
                        tokio::spawn(sftp::execute(s.clone(), id, request, transfer, events.clone()));
                    }
                    Some(Browser::Scp { chunk }) => match session.channel_open_session().await {
                        Ok(channel) => {
                            let transfer = scp::Transfer { chunk: *chunk };
                            tokio::spawn(scp::execute(channel, id, request, transfer, events.clone()));
                        }
                        Err(e) => events.send(SshEvent::Sftp(SftpEvent::Failed {
                            id,
                            error: format!("cannot open a channel: {e}"),
                        })),
                    },
                    None => events.send(SshEvent::Sftp(SftpEvent::Failed {
                        id,
                        error: "the file browser is not available on this connection".into(),
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
                        sftp = Some(Browser::Sftp(s));
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

enum KeyLoad {
    Encrypted,
    Failed(String),
}

/// A private key file: PuTTY `.ppk` through [`keygen`], the OpenSSH and
/// PEM formats through russh.
fn load_key_file(path: &std::path::Path, pass: Option<&str>) -> Result<keys::PrivateKey, KeyLoad> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    if text.trim_start().starts_with("PuTTY-User-Key-File-") {
        return match keygen::load(&text, pass) {
            Ok(k) => Ok(k),
            Err(keygen::LoadError::NeedsPassphrase) => Err(KeyLoad::Encrypted),
            Err(keygen::LoadError::Invalid(e)) if pass.is_some() && e == "wrong passphrase" => {
                Err(KeyLoad::Encrypted)
            }
            Err(keygen::LoadError::Invalid(e)) => Err(KeyLoad::Failed(e)),
        };
    }
    match keys::load_secret_key(path, pass) {
        Ok(k) => Ok(k),
        Err(keys::Error::KeyIsEncrypted) => Err(KeyLoad::Encrypted),
        Err(e) => Err(KeyLoad::Failed(e.to_string())),
    }
}

/// Key file, then the SSH agents, then password / keyboard-interactive.
async fn authenticate(
    session: &mut client::Handle<Handler>,
    endpoint: &Endpoint,
    agent_config: &AgentConfig,
    refuse_trivial: bool,
    replies: &Replies,
    events: &Events,
) -> Result<(), String> {
    let user = endpoint.user.clone();
    let err = |e: russh::Error| format!("authentication error: {e}");

    if let Some(path) = &endpoint.private_key {
        let key = match load_key_file(path, None) {
            Ok(k) => Some(k),
            Err(KeyLoad::Encrypted) => {
                let prompt = format!("Passphrase for key {}:", path.display());
                match ask_secret(events, replies, endpoint, prompt, SecretKind::KeyPassphrase).await
                {
                    Some(pass) => match load_key_file(path, Some(&pass)) {
                        Ok(k) => Some(k),
                        Err(KeyLoad::Encrypted) => return Err("wrong passphrase".into()),
                        Err(KeyLoad::Failed(e)) => {
                            return Err(format!("cannot load key {}: {e}", path.display()));
                        }
                    },
                    None => return Err("cancelled".into()),
                }
            }
            Err(KeyLoad::Failed(e)) => {
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

    // "Use internal SSH agent": its key files, before the external agents.
    for path in agent_config.keys.iter().take(5) {
        if endpoint.private_key.as_deref() == Some(path.as_path()) {
            continue;
        }
        let key = match load_key_file(path, None) {
            Ok(k) => k,
            Err(KeyLoad::Encrypted) => {
                let key_ep = Endpoint {
                    private_key: Some(path.clone()),
                    ..endpoint.clone()
                };
                let prompt = format!("Passphrase for key {}:", path.display());
                let Some(pass) =
                    ask_secret(events, replies, &key_ep, prompt, SecretKind::KeyPassphrase).await
                else {
                    continue;
                };
                match load_key_file(path, Some(&pass)) {
                    Ok(k) => k,
                    Err(_) => continue,
                }
            }
            Err(KeyLoad::Failed(e)) => {
                events.send(SshEvent::Info(format!(
                    "cannot load key {}: {e}",
                    path.display()
                )));
                continue;
            }
        };
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
            events.send(SshEvent::Info(format!(
                "Authenticated with {} (internal agent)",
                path.display()
            )));
            return Ok(());
        }
    }

    if agent::authenticate(session, &user, agent_config, events).await? {
        return Ok(());
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
                    && let Some(answered) =
                        keyboard_interactive(session, endpoint, Some(password), replies, events)
                            .await?
                {
                    if answered == 0 && refuse_trivial {
                        return Err(TRIVIAL_AUTH.into());
                    }
                    return Ok(());
                }
                events.send(SshEvent::Output(b"Access denied\r\n".to_vec()));
            }
        }
    }
    Err("authentication failed".into())
}

/// Why a session stops when the server let us in trivially.
const TRIVIAL_AUTH: &str = "the server accepted the login without asking for any password, key \
                            or answer; disconnected (\"Disconnect if authentication succeeds \
                            trivially\")";

/// Keyboard-interactive (PAM-style) login; the first hidden prompt reuses
/// the password the user already typed. On success, how many prompts were
/// answered (0 = the server asked for nothing).
async fn keyboard_interactive(
    session: &mut client::Handle<Handler>,
    endpoint: &Endpoint,
    mut password: Option<String>,
    replies: &Replies,
    events: &Events,
) -> Result<Option<usize>, String> {
    let mut answered = 0;
    let err = |e: russh::Error| format!("authentication error: {e}");
    let mut response = session
        .authenticate_keyboard_interactive_start(endpoint.user.as_str(), None)
        .await
        .map_err(err)?;
    loop {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(Some(answered)),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(None),
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
                answered += answers.len();
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
