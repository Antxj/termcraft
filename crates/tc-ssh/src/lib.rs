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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshEvent {
    /// Terminal output (stdout and stderr).
    Output(Vec<u8>),
    /// Status line for the terminal ("Connecting…", "Authenticated").
    Info(String),
    /// The server key is not in known_hosts; answer with `Reply::HostKey`.
    UnknownHostKey {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
    },
    /// A secret is needed (password, keyboard-interactive or key
    /// passphrase) for `user@host:port`; answer with `Reply::Secret`.
    SecretNeeded {
        prompt: String,
        kind: SecretKind,
        user: String,
        host: String,
        port: u16,
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
}

/// Command run on a separate exec channel to find the login shell.
pub const LOGIN_SHELL_PROBE: &str = "echo \"$SHELL\"";

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
            Err(keys::Error::KeyChanged { line }) => {
                self.events.send(SshEvent::Error(format!(
                    "WARNING: the host key of {}:{} has CHANGED (known_hosts line {line}). \
                     This may be a man-in-the-middle attack; connection refused.",
                    self.host, self.port
                )));
                Ok(false)
            }
            // Not recorded yet (or no known_hosts file): ask the user.
            Ok(false) | Err(_) => Ok(self.ask_host_key(key).await),
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
    async fn ask_host_key(&mut self, key: &PublicKey) -> bool {
        self.events.send(SshEvent::UnknownHostKey {
            host: self.host.clone(),
            port: self.port,
            algorithm: key.algorithm().to_string(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        });
        loop {
            match next_reply(&self.replies).await {
                Reply::HostKey { accept, remember } => {
                    if accept && remember {
                        if let Some(dir) = self.known_hosts.parent() {
                            let _ = std::fs::create_dir_all(dir);
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
    let (sftp, ()) = tokio::join!(open_sftp, probe);

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
                Some(Input::Close) | None => break,
            },
        }
    }
    let _ = session
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    Ok(exit_status)
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
