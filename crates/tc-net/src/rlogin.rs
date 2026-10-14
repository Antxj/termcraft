//! Rlogin client (RFC 1282), used by "Rsh" sessions like in the reference.
//! The protocol is plain text and trusts the client's port: servers expect
//! a privileged source port (512-1023), so one is tried first.

use crate::{Notify, Stream, StreamEvent};
use crossbeam_channel::{Receiver, Sender};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};

pub const DEFAULT_PORT: u16 = 513;

pub struct Rlogin {
    socket: Arc<Mutex<TcpStream>>,
    events: Receiver<StreamEvent>,
}

/// The connection request: `\0 local-user \0 remote-user \0 term/speed \0`.
pub fn handshake(local_user: &str, remote_user: &str, term: &str) -> Vec<u8> {
    let clean = |s: &str| s.replace('\0', "");
    let mut msg = vec![0u8];
    for part in [
        clean(local_user),
        clean(remote_user),
        format!("{}/38400", clean(term)),
    ] {
        msg.extend_from_slice(part.as_bytes());
        msg.push(0);
    }
    msg
}

/// Window size message: magic `FF FF s s`, then rows, cols, x and y pixels.
pub fn window_size(rows: u16, cols: u16) -> [u8; 12] {
    let mut m = [0xff, 0xff, b's', b's', 0, 0, 0, 0, 0, 0, 0, 0];
    m[4..6].copy_from_slice(&rows.to_be_bytes());
    m[6..8].copy_from_slice(&cols.to_be_bytes());
    m
}

/// Connect from a privileged port when the OS allows it (root on Unix,
/// anyone on Windows), else from any port.
fn connect_tcp(addr: SocketAddr) -> std::io::Result<TcpStream> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    let timeout = std::time::Duration::from_secs(10);
    for local_port in (1000..1024).rev() {
        let Ok(socket) = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))
        else {
            break;
        };
        let local: SocketAddr = match addr {
            SocketAddr::V4(_) => (std::net::Ipv4Addr::UNSPECIFIED, local_port).into(),
            SocketAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, local_port).into(),
        };
        match socket.bind(&SockAddr::from(local)) {
            Ok(()) => {}
            // Not allowed to bind low ports: no point trying the others.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => break,
            Err(_) => continue, // port in use
        }
        return match socket.connect_timeout(&SockAddr::from(addr), timeout) {
            Ok(()) => Ok(socket.into()),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => Err(e),
        };
    }
    TcpStream::connect_timeout(&addr, timeout)
}

impl Rlogin {
    pub fn connect(
        host: &str,
        port: u16,
        remote_user: &str,
        term: &str,
        rows: u16,
        cols: u16,
        notify: Notify,
    ) -> std::io::Result<Self> {
        let addr = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::other(format!("cannot resolve {host}")))?;
        let mut socket = connect_tcp(addr)?;
        socket.set_nodelay(true)?;
        let local_user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        let remote_user = if remote_user.is_empty() {
            local_user.as_str()
        } else {
            remote_user
        };
        socket.write_all(&handshake(&local_user, remote_user, term))?;
        let mut ack = [0u8; 1];
        socket.read_exact(&mut ack)?;
        if ack[0] != 0 {
            // The server refused and sent a message instead.
            let mut msg = vec![ack[0]];
            let _ = socket.read_to_end(&mut msg);
            return Err(std::io::Error::other(
                String::from_utf8_lossy(&msg).trim().to_string(),
            ));
        }
        socket.write_all(&window_size(rows, cols))?;
        let reader = socket.try_clone()?;
        let (tx, events) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name(format!("rlogin {host}"))
            .spawn(move || read_loop(reader, tx, notify))?;
        Ok(Self {
            socket: Arc::new(Mutex::new(socket)),
            events,
        })
    }
}

fn read_loop(mut reader: TcpStream, tx: Sender<StreamEvent>, notify: Notify) {
    let mut buf = [0u8; 8192];
    let reason = loop {
        match reader.read(&mut buf) {
            Ok(0) => break None,
            Ok(n) => {
                if tx.send(StreamEvent::Output(buf[..n].to_vec())).is_err() {
                    return;
                }
                notify();
            }
            Err(e) => break Some(e.to_string()),
        }
    };
    let _ = tx.send(StreamEvent::Closed(reason));
    notify();
}

impl Stream for Rlogin {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.socket.lock().expect("socket").write_all(bytes)
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let _ = self
            .socket
            .lock()
            .expect("socket")
            .write_all(&window_size(rows, cols));
    }

    fn events(&self) -> &Receiver<StreamEvent> {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_and_window_size() {
        assert_eq!(
            handshake("me", "root", "xterm"),
            b"\0me\0root\0xterm/38400\0"
        );
        assert_eq!(handshake("a\0b", "", "t"), b"\0ab\0\0t/38400\0");
        assert_eq!(
            window_size(24, 80),
            [0xff, 0xff, b's', b's', 0, 24, 0, 80, 0, 0, 0, 0]
        );
    }

    /// Against a tiny in-process server: handshake, ack, data both ways.
    #[test]
    fn talks_to_a_server() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut got = vec![0u8; 64];
            let n = s.read(&mut got).unwrap();
            got.truncate(n);
            s.write_all(b"\0welcome\r\n").unwrap();
            let mut more = [0u8; 64];
            let n = s.read(&mut more).unwrap();
            (got, more[..n].to_vec())
        });
        let mut r =
            Rlogin::connect("127.0.0.1", port, "root", "xterm", 24, 80, Arc::new(|| {})).unwrap();
        let mut seen = String::new();
        while !seen.contains("welcome") {
            if let StreamEvent::Output(b) = r.events().recv().unwrap() {
                seen.push_str(&String::from_utf8_lossy(&b));
            }
        }
        r.write(b"ls\r").unwrap();
        let (hello, rest) = server.join().unwrap();
        assert!(hello.starts_with(b"\0"));
        assert!(hello.windows(5).any(|w| w == b"root\0"));
        // The window size, then what we typed.
        assert!(rest.ends_with(b"ls\r") || rest.starts_with(&[0xff, 0xff, b's', b's']));
    }
}
