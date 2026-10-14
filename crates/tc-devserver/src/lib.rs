//! Minimal SSH server for development and tests. Binds to loopback only.
//! Supports password and public-key auth, PTY + interactive shell (the
//! local shell via `tc-pty`), exec requests, window resizes and an SFTP
//! subsystem rooted at `DevConfig::sftp_root`.

pub mod sftp;

use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{Auth, Msg, Server, Session};
use russh::{Channel, ChannelId, Pty};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
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
    /// Local directory served as SFTP `/` (created on demand).
    pub sftp_root: PathBuf,
    /// Interactive shell; `None` = the platform default shell.
    pub shell: Option<tc_pty::ShellCommand>,
    /// `$SHELL` reported to exec requests that print it (`echo "$SHELL"`),
    /// so clients can detect the login shell on any platform.
    pub login_shell: Option<String>,
    /// Program that runs exec requests as `PROGRAM -c COMMAND` (`None` =
    /// `cmd /C` on Windows, `sh -c` elsewhere).
    pub exec_shell: Option<String>,
    /// Offer the SFTP subsystem (off = test clients' SCP fallback).
    pub sftp_subsystem: bool,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            user: DEV_USER.into(),
            password: DEV_PASSWORD.into(),
            authorized_keys: Vec::new(),
            sftp_root: std::env::temp_dir().join("termcraft-devserver"),
            shell: None,
            login_shell: None,
            exec_shell: None,
            sftp_subsystem: true,
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
            channels: HashMap::new(),
        }
    }
}

struct Conn {
    config: Arc<DevConfig>,
    sizes: HashMap<ChannelId, (u16, u16)>,
    ptys: HashMap<ChannelId, Arc<Mutex<PtySession>>>,
    /// Opened channels not yet claimed by a subsystem.
    channels: HashMap<ChannelId, Channel<Msg>>,
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
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
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
        let pty = match &self.config.shell {
            Some(shell) => PtySession::spawn_command(shell, rows, cols, Arc::new(|| {}))?,
            None => PtySession::spawn(None, &[], rows, cols, Arc::new(|| {}))?,
        };
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
        let login_shell = self.config.login_shell.clone();
        // With an exec shell configured, stream stdin / stdout / stderr
        // (needed by interactive protocols such as scp -t / -f).
        if let (Some(program), Some(ch)) = (
            self.config.exec_shell.clone(),
            self.channels.remove(&channel),
        ) && login_shell.is_none()
        {
            tokio::spawn(stream_exec(program, command, ch, handle, channel));
            return Ok(());
        }
        tokio::spawn(async move {
            if let Some(shell) = login_shell.filter(|_| command.contains("$SHELL")) {
                let _ = handle
                    .data(channel, format!("{shell}\n").into_bytes())
                    .await;
                let _ = handle.exit_status_request(channel, 0).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
                return;
            }
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
        self.channels.remove(&channel);
        Ok(())
    }

    /// `direct-tcpip`: local forwards, SOCKS and jump hosts.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let target = (host_to_connect.to_string(), to_u16(port_to_connect));
        match tokio::net::TcpStream::connect((target.0.as_str(), target.1)).await {
            Ok(mut tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                });
            }
            Err(_) => {
                reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            }
        }
        Ok(())
    }

    /// `tcpip-forward`: listen on the (loopback) address and open a
    /// `forwarded-tcpip` channel back to the client per connection.
    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        // Never expose the dev server beyond loopback, whatever was asked.
        let listener = TcpListener::bind(("127.0.0.1", u16::try_from(*port).unwrap_or(0))).await?;
        *port = u32::from(listener.local_addr()?.port());
        let (address, bound) = (address.to_string(), *port);
        let handle = session.handle();
        tokio::spawn(async move {
            while let Ok((mut tcp, peer)) = listener.accept().await {
                let handle = handle.clone();
                let address = address.clone();
                tokio::spawn(async move {
                    let opened = handle
                        .channel_open_forwarded_tcpip(
                            address,
                            bound,
                            peer.ip().to_string(),
                            u32::from(peer.port()),
                        )
                        .await;
                    if let Ok(channel) = opened {
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                    }
                });
            }
        });
        Ok(true)
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match (name, self.channels.remove(&channel_id)) {
            ("sftp", Some(channel)) if self.config.sftp_subsystem => {
                std::fs::create_dir_all(&self.config.sftp_root)?;
                session.channel_success(channel_id)?;
                let fs = sftp::SftpFs::new(self.config.sftp_root.clone());
                russh_sftp::server::run(channel.into_stream(), fs).await;
            }
            _ => session.channel_failure(channel_id)?,
        }
        Ok(())
    }
}

/// Run `program -c command` with the channel as its stdin / stdout and
/// stderr as extended data, then report the exit status.
async fn stream_exec(
    program: String,
    command: String,
    channel: Channel<Msg>,
    handle: russh::server::Handle,
    id: ChannelId,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let child = tokio::process::Command::new(&program)
        .arg("-c")
        .arg(&command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = handle
                .extended_data(id, 1, format!("exec failed: {e}\n").into_bytes())
                .await;
            let _ = handle.exit_status_request(id, 127).await;
            let _ = handle.close(id).await;
            return;
        }
    };
    let (mut from_client, mut to_client) = tokio::io::split(channel.into_stream());
    let mut stdin = child.stdin.take().expect("piped");
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let input = async move {
        let _ = tokio::io::copy(&mut from_client, &mut stdin).await;
        let _ = stdin.shutdown().await;
    };
    let output = async move {
        let _ = tokio::io::copy(&mut stdout, &mut to_client).await;
        let _ = to_client.flush().await;
    };
    let errors = {
        let handle = handle.clone();
        async move {
            let mut buf = vec![0u8; 8192];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let _ = handle.extended_data(id, 1, buf[..n].to_vec()).await;
            }
        }
    };
    let input = tokio::spawn(input);
    tokio::join!(output, errors);
    let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(1);
    input.abort();
    let _ = handle
        .exit_status_request(id, u32::try_from(code).unwrap_or(1))
        .await;
    let _ = handle.eof(id).await;
    let _ = handle.close(id).await;
}

fn to_u16(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX).max(1)
}
