//! Minimal SSH server for development and tests. Binds to loopback only.
//! Supports password and public-key auth, PTY + interactive shell (the
//! local shell via `tc-pty`), exec requests and window resizes.

use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{Auth, Msg, Server, Session};
use russh::{Channel, ChannelId, Pty};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tc_pty::{PtyEvent, PtySession};
use tokio::net::TcpListener;

/// Development credentials (also documented in docs/DEV.md). Loopback only.
pub const DEV_USER: &str = "termcraft";
pub const DEV_PASSWORD: &str = "termcraft-dev";
/// Fixed seed → the same host key on every run, so known_hosts stays valid.
const HOST_KEY_SEED: [u8; 32] = *b"termcraft-devserver-hostkey-0001";

#[derive(Clone)]
pub struct DevConfig {
    pub user: String,
    pub password: String,
    pub authorized_keys: Vec<PublicKey>,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            user: DEV_USER.into(),
            password: DEV_PASSWORD.into(),
            authorized_keys: Vec::new(),
        }
    }
}

pub fn host_key() -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&HOST_KEY_SEED))
}

/// Bind `127.0.0.1:port` (0 = any free port) and return the bound address
/// plus the server future to drive.
pub async fn bind(
    port: u16,
    config: DevConfig,
) -> std::io::Result<(SocketAddr, impl Future<Output = std::io::Result<()>>)> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    let server_config = Arc::new(russh::server::Config {
        auth_rejection_time: Duration::from_millis(200),
        auth_rejection_time_initial: Some(Duration::ZERO),
        inactivity_timeout: Some(Duration::from_secs(3600)),
        keys: vec![host_key()],
        ..Default::default()
    });
    let mut server = DevServer {
        config: Arc::new(config),
    };
    let fut = async move { server.run_on_socket(server_config, &listener).await };
    Ok((addr, fut))
}

#[derive(Clone)]
struct DevServer {
    config: Arc<DevConfig>,
}

impl Server for DevServer {
    type Handler = Conn;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Conn {
        Conn {
            config: self.config.clone(),
            sizes: HashMap::new(),
            ptys: HashMap::new(),
        }
    }
}

struct Conn {
    config: Arc<DevConfig>,
    sizes: HashMap<ChannelId, (u16, u16)>,
    ptys: HashMap<ChannelId, Arc<Mutex<PtySession>>>,
}

fn reject() -> Auth {
    Auth::Reject {
        proceed_with_methods: None,
        partial_success: false,
    }
}

impl russh::server::Handler for Conn {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(
            if user == self.config.user && password == self.config.password {
                Auth::Accept
            } else {
                reject()
            },
        )
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let known = self
            .config
            .authorized_keys
            .iter()
            .any(|k| k.key_data() == key.key_data());
        Ok(if user == self.config.user && known {
            Auth::Accept
        } else {
            reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.sizes.insert(channel, (to_u16(rows), to_u16(cols)));
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let (rows, cols) = self.sizes.get(&channel).copied().unwrap_or((24, 80));
        let pty = PtySession::spawn(None, &[], rows, cols, Arc::new(|| {}))?;
        let events = pty.events().clone();
        self.ptys.insert(channel, Arc::new(Mutex::new(pty)));

        let handle = session.handle();
        let runtime = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            for event in events.iter() {
                match event {
                    PtyEvent::Output(bytes) => {
                        if runtime.block_on(handle.data(channel, bytes)).is_err() {
                            return;
                        }
                    }
                    PtyEvent::Exited => break,
                }
            }
            runtime.block_on(async {
                let _ = handle.exit_status_request(channel, 0).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
        });
        session.channel_success(channel)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        let handle = session.handle();
        session.channel_success(channel)?;
        tokio::spawn(async move {
            let mut cmd = if cfg!(windows) {
                let mut c = tokio::process::Command::new("cmd");
                c.arg("/C").arg(&command);
                c
            } else {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c").arg(&command);
                c
            };
            let (out, code) = match cmd.output().await {
                Ok(o) => {
                    let mut bytes = o.stdout;
                    bytes.extend_from_slice(&o.stderr);
                    (bytes, o.status.code().unwrap_or(1) as u32)
                }
                Err(e) => (format!("exec failed: {e}\r\n").into_bytes(), 127),
            };
            let _ = handle.data(channel, out).await;
            let _ = handle.exit_status_request(channel, code).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(pty) = self.ptys.get(&channel) {
            let _ = pty.lock().map(|mut p| p.write(data));
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(pty) = self.ptys.get(&channel) {
            let _ = pty.lock().map(|p| p.resize(to_u16(rows), to_u16(cols)));
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ptys.remove(&channel);
        Ok(())
    }
}

fn to_u16(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX).max(1)
}
