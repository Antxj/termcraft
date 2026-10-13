//! Native SSH client built on `russh`.
//!
//! [`SshSession::connect`] starts a background thread with its own tokio
//! runtime and returns immediately. The UI reads [`SshEvent`]s from a
//! channel and answers questions (unknown host key, password, key
//! passphrase) with [`SshSession::reply`]. Input and resizes are queued the
//! same way, so the UI thread never blocks on the network.

use crossbeam_channel::{Receiver, Sender};
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, MethodKind};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub private_key: Option<PathBuf>,
    /// OpenSSH-format known_hosts file used for host key verification.
    pub known_hosts: PathBuf,
    pub compression: bool,
    /// Run this command instead of an interactive shell.
    pub command: Option<String>,
    pub term: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshEvent {
    /// Terminal output (stdout and stderr).
    Output(Vec<u8>),
    /// Status line for the terminal ("Connecting…", "Authenticated").
    Info(String),
    /// The server key is not in known_hosts; answer with `Reply::HostKey`.
    UnknownHostKey {
        algorithm: String,
        fingerprint: String,
    },
    /// A secret is needed (password, keyboard-interactive or key
    /// passphrase); answer with `Reply::Secret`.
    SecretNeeded {
        prompt: String,
    },
    Connected,
    Error(String),
    Closed {
        exit_status: Option<u32>,
    },
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
    Close,
}

pub struct SshSession {
    input: mpsc::UnboundedSender<Input>,
    replies: mpsc::UnboundedSender<Reply>,
    events: Receiver<SshEvent>,
}

impl SshSession {
    pub fn connect(params: ConnectParams, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (reply_tx, reply_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = crossbeam_channel::unbounded();
        let events_out = Events {
            tx: event_tx,
            notify,
        };
        std::thread::Builder::new()
            .name(format!("ssh {}", params.host))
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => return events_out.send(SshEvent::Error(e.to_string())),
                };
                let replies = Arc::new(Mutex::new(reply_rx));
                let result = runtime.block_on(run(params, input_rx, replies, events_out.clone()));
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
struct Events {
    tx: Sender<SshEvent>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Events {
    fn send(&self, event: SshEvent) {
        let _ = self.tx.send(event);
        (self.notify)();
    }
}

type Replies = Arc<Mutex<mpsc::UnboundedReceiver<Reply>>>;

async fn next_reply(replies: &Replies) -> Reply {
    replies.lock().await.recv().await.unwrap_or(Reply::Cancel)
}

async fn ask_secret(events: &Events, replies: &Replies, prompt: String) -> Option<String> {
    events.send(SshEvent::SecretNeeded { prompt });
    loop {
        match next_reply(replies).await {
            Reply::Secret(s) => return Some(s),
            Reply::Cancel => return None,
            Reply::HostKey { .. } => {}
        }
    }
}

struct Handler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    events: Events,
    replies: Replies,
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
}

impl Handler {
    async fn ask_host_key(&mut self, key: &PublicKey) -> bool {
        self.events.send(SshEvent::UnknownHostKey {
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

async fn run(
    params: ConnectParams,
    mut input: mpsc::UnboundedReceiver<Input>,
    replies: Replies,
    events: Events,
) -> Result<Option<u32>, String> {
    let mut config = client::Config::default();
    if params.compression {
        config.preferred.compression = std::borrow::Cow::Borrowed(&[
            russh::compression::ZLIB_LEGACY,
            russh::compression::ZLIB,
            russh::compression::NONE,
        ]);
    }
    let handler = Handler {
        host: params.host.clone(),
        port: params.port,
        known_hosts: params.known_hosts.clone(),
        events: events.clone(),
        replies: replies.clone(),
    };
    events.send(SshEvent::Info(format!(
        "Connecting to {}:{} …",
        params.host, params.port
    )));
    let mut session = client::connect(
        Arc::new(config),
        (params.host.as_str(), params.port),
        handler,
    )
    .await
    .map_err(|e| format!("connection failed: {e}"))?;

    authenticate(&mut session, &params, &replies, &events).await?;
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
                Some(Input::Close) | None => break,
            },
        }
    }
    let _ = session
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    Ok(exit_status)
}

async fn authenticate(
    session: &mut client::Handle<Handler>,
    params: &ConnectParams,
    replies: &Replies,
    events: &Events,
) -> Result<(), String> {
    let user = params.user.clone();
    let err = |e: russh::Error| format!("authentication error: {e}");

    if let Some(path) = &params.private_key {
        let key = match keys::load_secret_key(path, None) {
            Ok(k) => Some(k),
            Err(keys::Error::KeyIsEncrypted) => {
                let prompt = format!("Passphrase for key {}:", path.display());
                match ask_secret(events, replies, prompt).await {
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
        let prompt = format!("{}@{}'s password:", user, params.host);
        let Some(password) = ask_secret(events, replies, prompt).await else {
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
                    && keyboard_interactive(session, &user, Some(password), replies, events).await?
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
    user: &str,
    mut password: Option<String>,
    replies: &Replies,
    events: &Events,
) -> Result<bool, String> {
    let err = |e: russh::Error| format!("authentication error: {e}");
    let mut response = session
        .authenticate_keyboard_interactive_start(user, None)
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
                        _ => match ask_secret(events, replies, p.prompt.clone()).await {
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
