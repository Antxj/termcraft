//! Running SSH tunnels: one `SshSession::tunnel` per started tunnel, with
//! its own host-key and password prompts (tunnels have no terminal).

use crate::credentials::{Credentials, PromptAction};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use tc_core::settings::SavePasswords;
use tc_core::{Tunnel, TunnelKind};
use tc_ssh::{Endpoint, Reply, SecretKind, SshEvent, SshSession, TunnelParams, TunnelSpec};
use tc_vault::{Target, Vault};

pub struct HostKeyQuestion {
    pub host: String,
    pub algorithm: String,
    pub fingerprint: String,
}

pub struct SecretQuestion {
    pub prompt: String,
    pub kind: SecretKind,
    pub target: Target,
    pub input: String,
}

pub struct RunningTunnel {
    pub name: String,
    session: SshSession,
    pub status: String,
    pub listening: bool,
    pub host_key: Option<HostKeyQuestion>,
    pub secret: Option<SecretQuestion>,
    pub closed: bool,
    credentials: Credentials,
    /// Passwords to offer for saving after login.
    pub offers: VecDeque<(Target, String)>,
}

pub fn params(t: &Tunnel, known_hosts: PathBuf) -> TunnelParams {
    let spec = match t.kind {
        TunnelKind::Local => TunnelSpec::Local {
            bind: t.bind.clone(),
            port: t.forward_port,
            dest_host: t.dest_host.clone(),
            dest_port: t.dest_port,
        },
        TunnelKind::Remote => TunnelSpec::Remote {
            bind: t.bind.clone(),
            port: t.forward_port,
            dest_host: t.dest_host.clone(),
            dest_port: t.dest_port,
        },
        TunnelKind::Dynamic => TunnelSpec::Dynamic {
            bind: t.bind.clone(),
            port: t.forward_port,
        },
    };
    TunnelParams {
        ssh: Endpoint {
            host: t.ssh_host.clone(),
            port: t.ssh_port,
            user: t.ssh_user.clone(),
            private_key: t.private_key.as_ref().map(PathBuf::from),
        },
        gateway: None,
        known_hosts,
        spec,
    }
}

impl RunningTunnel {
    pub fn start(
        tunnel: &Tunnel,
        known_hosts: PathBuf,
        vault: Arc<Vault>,
        mode: SavePasswords,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            name: tunnel.name.clone(),
            session: SshSession::tunnel(params(tunnel, known_hosts), repaint),
            status: "Connecting…".into(),
            listening: false,
            host_key: None,
            secret: None,
            closed: false,
            credentials: Credentials::new(vault, mode),
            offers: VecDeque::new(),
        }
    }

    pub fn pump(&mut self) {
        let events: Vec<SshEvent> = self.session.events().try_iter().collect();
        for event in events {
            match event {
                SshEvent::Info(text) => self.status = text,
                SshEvent::UnknownHostKey {
                    host,
                    port,
                    algorithm,
                    fingerprint,
                } => {
                    self.host_key = Some(HostKeyQuestion {
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
                } => {
                    let target = Target::ssh(&user, &host, port);
                    match self.credentials.on_prompt(kind, &target) {
                        PromptAction::AutoFill(pw) => self.session.reply(Reply::Secret(pw)),
                        PromptAction::Ask { saved_rejected } => {
                            if saved_rejected {
                                self.status = "The saved password was rejected.".into();
                            }
                            self.secret = Some(SecretQuestion {
                                prompt,
                                kind,
                                target,
                                input: String::new(),
                            });
                        }
                    }
                }
                SshEvent::Connected => {
                    self.status = "Connected".into();
                    let (offers, errors) = self.credentials.after_login();
                    self.offers.extend(offers);
                    if let Some(e) = errors.into_iter().next() {
                        self.status = e;
                    }
                }
                SshEvent::Listening(addr) => {
                    self.listening = true;
                    self.status = format!("Listening on {addr}");
                }
                SshEvent::Error(e) => self.status = e,
                SshEvent::Closed { .. } => {
                    self.closed = true;
                    self.listening = false;
                    self.host_key = None;
                    self.secret = None;
                    if !self.status.starts_with("Stopped") && self.status.starts_with("Listening") {
                        self.status = "Stopped".into();
                    }
                }
                SshEvent::Output(_) | SshEvent::Sftp(_) => {}
            }
        }
    }

    pub fn answer_host_key(&mut self, accept: bool, remember: bool) {
        self.host_key = None;
        self.session.reply(Reply::HostKey { accept, remember });
    }

    /// `Some(text)` = OK with that secret, `None` = cancel.
    pub fn answer_secret(&mut self, answer: Option<String>) {
        let Some(q) = self.secret.take() else { return };
        match answer {
            Some(secret) => {
                self.credentials.on_typed(q.kind, q.target, secret.clone());
                self.session.reply(Reply::Secret(secret));
            }
            None => self.session.reply(Reply::Cancel),
        }
    }
}

impl Drop for RunningTunnel {
    fn drop(&mut self) {
        self.session.close();
    }
}
