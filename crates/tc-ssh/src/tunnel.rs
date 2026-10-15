//! SSH port forwarding ("MobaSSHTunnel"): local, remote and dynamic
//! (SOCKS5) tunnels, each in its own SSH session.

use crate::{ConnectOptions, Endpoint, Events, Input, Replies, SshEvent, establish};
use russh::Disconnect;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelSpec {
    /// Listen on `bind:port` here; forward to `dest_host:dest_port` as seen
    /// from the SSH server.
    Local {
        bind: String,
        port: u16,
        dest_host: String,
        dest_port: u16,
    },
    /// Listen on `bind:port` on the SSH server; forward to
    /// `dest_host:dest_port` as seen from this computer.
    Remote {
        bind: String,
        port: u16,
        dest_host: String,
        dest_port: u16,
    },
    /// SOCKS5 proxy on `bind:port` here; every connection goes out through
    /// the SSH server.
    Dynamic { bind: String, port: u16 },
}

#[derive(Debug, Clone)]
pub struct TunnelParams {
    pub ssh: Endpoint,
    pub gateways: Vec<Endpoint>,
    pub known_hosts: std::path::PathBuf,
    pub spec: TunnelSpec,
    pub agent: crate::AgentConfig,
}

pub(crate) async fn run(
    params: TunnelParams,
    mut input: mpsc::UnboundedReceiver<Input>,
    replies: Replies,
    events: Events,
) -> Result<(), String> {
    let conn = establish(
        ConnectOptions {
            target: &params.ssh,
            gateways: &params.gateways,
            known_hosts: &params.known_hosts,
            compression: false,
            keepalive: Some(std::time::Duration::from_secs(30)),
            agent: params.agent.clone(),
            proxy: None,
            agent_forward: false,
            x11: None,
            refuse_trivial_auth: false,
            show_banner: false,
            algorithms: Default::default(),
            certificate: None,
        },
        &replies,
        &events,
    )
    .await?;
    events.send(SshEvent::Connected);
    let handle = Arc::new(conn.handle);

    let wait_close = async {
        loop {
            match input.recv().await {
                Some(Input::Close) | None => return,
                Some(_) => {}
            }
        }
    };

    match params.spec {
        TunnelSpec::Remote {
            bind,
            port,
            dest_host,
            dest_port,
        } => {
            let bound = handle
                .tcpip_forward(bind.clone(), u32::from(port))
                .await
                .map_err(|e| format!("server refused remote forward {bind}:{port}: {e}"))?;
            let bound = if bound == 0 { u32::from(port) } else { bound };
            conn.forwards
                .lock()
                .expect("lock")
                .insert(bound, (dest_host.clone(), dest_port));
            events.send(SshEvent::Listening(format!(
                "{bind}:{bound} on the server -> {dest_host}:{dest_port}"
            )));
            wait_close.await;
            let _ = handle.cancel_tcpip_forward(bind, bound).await;
        }
        TunnelSpec::Local {
            bind,
            port,
            dest_host,
            dest_port,
        } => {
            let listener = bind_listener(&bind, port).await?;
            events.send(SshEvent::Listening(format!(
                "{} -> {dest_host}:{dest_port}",
                local_addr(&listener)
            )));
            let accept = async {
                loop {
                    let Ok((tcp, peer)) = listener.accept().await else {
                        continue;
                    };
                    let (handle, events) = (handle.clone(), events.clone());
                    let (host, port) = (dest_host.clone(), dest_port);
                    tokio::spawn(async move {
                        if let Err(e) = forward(&handle, tcp, &host, port, &peer).await {
                            events.send(SshEvent::Info(format!("{peer}: {e}")));
                        }
                    });
                }
            };
            tokio::select! { () = wait_close => {}, () = accept => {} }
        }
        TunnelSpec::Dynamic { bind, port } => {
            let listener = bind_listener(&bind, port).await?;
            events.send(SshEvent::Listening(format!(
                "SOCKS5 proxy on {}",
                local_addr(&listener)
            )));
            let accept = async {
                loop {
                    let Ok((tcp, peer)) = listener.accept().await else {
                        continue;
                    };
                    let (handle, events) = (handle.clone(), events.clone());
                    tokio::spawn(async move {
                        if let Err(e) = socks5(&handle, tcp, &peer).await {
                            events.send(SshEvent::Info(format!("SOCKS {peer}: {e}")));
                        }
                    });
                }
            };
            tokio::select! { () = wait_close => {}, () = accept => {} }
        }
    }
    let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
    Ok(())
}

/// The local port of a `Local` tunnel, from its `SshEvent::Listening`
/// message (`127.0.0.1:50123 -> host:port`).
pub fn listening_port(message: &str) -> Option<u16> {
    let (addr, _) = message.split_once(" -> ")?;
    addr.parse::<std::net::SocketAddr>().ok().map(|a| a.port())
}

async fn bind_listener(bind: &str, port: u16) -> Result<TcpListener, String> {
    TcpListener::bind((bind, port))
        .await
        .map_err(|e| format!("cannot listen on {bind}:{port}: {e}"))
}

fn local_addr(listener: &TcpListener) -> String {
    listener
        .local_addr()
        .map_or_else(|_| "?".into(), |a| a.to_string())
}

type Handle = russh::client::Handle<crate::Handler>;

async fn forward(
    handle: &Handle,
    mut tcp: TcpStream,
    host: &str,
    port: u16,
    peer: &std::net::SocketAddr,
) -> Result<(), String> {
    let channel = handle
        .channel_open_direct_tcpip(
            host,
            u32::from(port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        )
        .await
        .map_err(|e| format!("cannot open {host}:{port} through SSH: {e}"))?;
    let mut stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
    Ok(())
}

/// Minimal SOCKS5 server (RFC 1928): no authentication, CONNECT only.
async fn socks5(
    handle: &Handle,
    mut tcp: TcpStream,
    peer: &std::net::SocketAddr,
) -> Result<(), String> {
    let (host, port) = socks5_handshake(&mut tcp).await?;
    match handle
        .channel_open_direct_tcpip(
            host.clone(),
            u32::from(port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        )
        .await
    {
        Ok(channel) => {
            // Success, bound address 0.0.0.0:0.
            tcp.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
                .await
                .map_err(|e| e.to_string())?;
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
            Ok(())
        }
        Err(e) => {
            // 0x05: connection refused.
            let _ = tcp.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            Err(format!("cannot open {host}:{port}: {e}"))
        }
    }
}

/// Negotiate "no auth" and read a CONNECT request; returns the target.
pub(crate) async fn socks5_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
) -> Result<(String, u16), String> {
    let io = |e: std::io::Error| e.to_string();
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await.map_err(io)?;
    if head[0] != 5 {
        return Err(format!("not SOCKS5 (version {})", head[0]));
    }
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await.map_err(io)?;
    if !methods.contains(&0) {
        s.write_all(&[5, 0xff]).await.map_err(io)?;
        return Err("client requires authentication".into());
    }
    s.write_all(&[5, 0]).await.map_err(io)?;

    let mut req = [0u8; 4];
    s.read_exact(&mut req).await.map_err(io)?;
    if req[1] != 1 {
        // 0x07: command not supported.
        s.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])
            .await
            .map_err(io)?;
        return Err(format!("unsupported SOCKS command {}", req[1]));
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await.map_err(io)?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await.map_err(io)?;
            let mut name = vec![0u8; len[0] as usize];
            s.read_exact(&mut name).await.map_err(io)?;
            String::from_utf8(name).map_err(|_| "invalid domain name".to_string())?
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await.map_err(io)?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        t => return Err(format!("unknown SOCKS address type {t}")),
    };
    let mut port = [0u8; 2];
    s.read_exact(&mut port).await.map_err(io)?;
    Ok((host, u16::from_be_bytes(port)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_port_from_the_listening_message() {
        assert_eq!(
            listening_port("127.0.0.1:50123 -> rdp.lan:3389"),
            Some(50123)
        );
        assert_eq!(listening_port("[::1]:7 -> h:1"), Some(7));
        assert_eq!(listening_port("SOCKS5 proxy on 127.0.0.1:1080"), None);
    }

    #[tokio::test]
    async fn socks5_parses_domain_and_ipv4() {
        let (mut client, mut server) = tokio::io::duplex(256);
        let task = tokio::spawn(async move { socks5_handshake(&mut server).await });
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [5, 0]);
        let mut req = vec![5, 1, 0, 3, 11];
        req.extend_from_slice(b"example.org");
        req.extend_from_slice(&443u16.to_be_bytes());
        client.write_all(&req).await.unwrap();
        assert_eq!(task.await.unwrap().unwrap(), ("example.org".into(), 443));

        let (mut client, mut server) = tokio::io::duplex(256);
        let task = tokio::spawn(async move { socks5_handshake(&mut server).await });
        client
            .write_all(&[5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 7, 0, 22])
            .await
            .unwrap();
        assert_eq!(task.await.unwrap().unwrap(), ("10.0.0.7".into(), 22));
    }

    #[tokio::test]
    async fn socks5_rejects_auth_only_clients() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let task = tokio::spawn(async move { socks5_handshake(&mut server).await });
        client.write_all(&[5, 1, 2]).await.unwrap(); // only user/pass
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [5, 0xff]);
        assert!(task.await.unwrap().is_err());
    }
}
