//! Session "Network settings" proxy for the first SSH hop: SOCKS4(a),
//! SOCKS5 (no auth or user / password, RFC 1928 / 1929), HTTP CONNECT
//! (Basic auth on 407), Telnet (a command line sent to the proxy, then the
//! connection), a local proxy command whose stdin / stdout carry the
//! connection (like OpenSSH `ProxyCommand`), and an SSH server as proxy:
//! "SSH forwarding" (a direct-tcpip channel) or "SSH command" (`nc`-like
//! command on it). The two SSH kinds are run by the SSH layer.

use base64::Engine as _;
use std::pin::Pin;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyKind {
    Socks4,
    Socks5,
    Http,
    /// Telnet proxy: this line is sent first (see [`telnet_command`]).
    Telnet(String),
    /// Local command; `%h` / `%p` are replaced by the target host / port.
    Command(String),
    /// The proxy is an SSH server; the target is reached through a
    /// forwarding channel (like a gateway).
    SshForward,
    /// The proxy is an SSH server; this command (`%host` / `%port`, `%h` /
    /// `%p`) is run on it and its input / output carry the connection.
    SshCommand(String),
}

/// What a Telnet proxy is sent when no command is set (PuTTY's default).
pub const DEFAULT_TELNET_COMMAND: &str = "connect %host %port\\n";
/// What an "SSH command" proxy runs when no command is set.
pub const DEFAULT_SSH_COMMAND: &str = "nc %host %port";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    /// Proxy login (SOCKS user id, SOCKS5 / HTTP user name).
    pub user: Option<String>,
}

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

pub type BoxStream = Pin<Box<dyn Stream>>;

fn err(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::other(msg.into())
}

/// The proxy password, asked for only when the proxy wants one.
pub type PasswordFuture = Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>;
pub type AskPassword<'a> = &'a mut (dyn FnMut() -> PasswordFuture + Send);

/// Open `host:port` through the proxy.
pub async fn connect(
    proxy: &Proxy,
    host: &str,
    port: u16,
    ask_password: AskPassword<'_>,
) -> std::io::Result<BoxStream> {
    match &proxy.kind {
        ProxyKind::Command(cmd) => return command(cmd, host, port),
        ProxyKind::SshForward | ProxyKind::SshCommand(_) => {
            return Err(err("an SSH proxy is opened by the SSH connection"));
        }
        _ => {}
    }
    let mut tcp = tokio::net::TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|e| err(format!("proxy {}:{}: {e}", proxy.host, proxy.port)))?;
    tcp.set_nodelay(true)?;
    match &proxy.kind {
        ProxyKind::Socks4 => socks4(&mut tcp, host, port, proxy.user.as_deref()).await?,
        ProxyKind::Socks5 => {
            socks5(&mut tcp, host, port, proxy.user.as_deref(), ask_password).await?
        }
        ProxyKind::Http => {
            let stream = http_connect(tcp, host, port, proxy.user.as_deref(), ask_password).await?;
            return Ok(Box::pin(stream));
        }
        ProxyKind::Telnet(template) => {
            let line = telnet_command(template, host, port, proxy.user.as_deref());
            tcp.write_all(&line).await?;
        }
        ProxyKind::Command(_) | ProxyKind::SshForward | ProxyKind::SshCommand(_) => {
            unreachable!("handled above")
        }
    }
    Ok(Box::pin(tcp))
}

/// SOCKS4a: the host name is sent to the proxy (0.0.0.1 marker).
pub async fn socks4<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    host: &str,
    port: u16,
    user: Option<&str>,
) -> std::io::Result<()> {
    let mut req = vec![4, 1];
    req.extend_from_slice(&port.to_be_bytes());
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            req.extend_from_slice(&ip.octets());
            req.extend_from_slice(user.unwrap_or("").as_bytes());
            req.push(0);
        }
        Err(_) => {
            req.extend_from_slice(&[0, 0, 0, 1]);
            req.extend_from_slice(user.unwrap_or("").as_bytes());
            req.push(0);
            req.extend_from_slice(host.as_bytes());
            req.push(0);
        }
    }
    s.write_all(&req).await?;
    let mut reply = [0u8; 8];
    s.read_exact(&mut reply).await?;
    if reply[1] != 0x5a {
        return Err(err(format!(
            "SOCKS4 proxy refused the connection (code {})",
            reply[1]
        )));
    }
    Ok(())
}

pub async fn socks5<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    host: &str,
    port: u16,
    user: Option<&str>,
    ask_password: AskPassword<'_>,
) -> std::io::Result<()> {
    let user = user.filter(|u| !u.is_empty());
    let methods: &[u8] = if user.is_some() {
        &[0x00, 0x02]
    } else {
        &[0x00]
    };
    let mut hello = vec![5, methods.len() as u8];
    hello.extend_from_slice(methods);
    s.write_all(&hello).await?;
    let mut choice = [0u8; 2];
    s.read_exact(&mut choice).await?;
    match choice {
        [5, 0x00] => {}
        [5, 0x02] => {
            let user = user.unwrap_or("");
            let password = ask_password().await.ok_or_else(|| err("cancelled"))?;
            let password = zeroize::Zeroizing::new(password);
            if user.len() > 255 || password.len() > 255 {
                return Err(err("SOCKS5 user name or password too long"));
            }
            let mut auth = vec![1, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.push(password.len() as u8);
            auth.extend_from_slice(password.as_bytes());
            s.write_all(&auth).await?;
            let mut status = [0u8; 2];
            s.read_exact(&mut status).await?;
            if status[1] != 0 {
                return Err(err("SOCKS5 proxy rejected the user name or password"));
            }
        }
        _ => return Err(err("SOCKS5 proxy offers no supported authentication")),
    }
    if host.len() > 255 {
        return Err(err("host name too long for SOCKS5"));
    }
    let mut req = vec![5, 1, 0];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            req.push(1);
            req.extend_from_slice(&ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            req.push(4);
            req.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            req.push(3);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[1] != 0 {
        return Err(err(format!(
            "SOCKS5 proxy refused the connection (code {})",
            head[1]
        )));
    }
    // Skip the bound address.
    let skip = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            usize::from(len[0])
        }
        _ => return Err(err("SOCKS5 proxy sent a bad reply")),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest).await?;
    Ok(())
}

/// HTTP CONNECT; on 407 the request is sent again with Basic auth on a new
/// connection (many proxies close after a 407). The buffered reader is the
/// stream: it may already hold the first bytes of the SSH banner, and it
/// passes writes through.
pub async fn http_connect(
    tcp: tokio::net::TcpStream,
    host: &str,
    port: u16,
    user: Option<&str>,
    ask_password: AskPassword<'_>,
) -> std::io::Result<BufReader<tokio::net::TcpStream>> {
    let peer = tcp.peer_addr()?;
    let mut reader = BufReader::new(tcp);
    let mut auth: Option<String> = None;
    for attempt in 0..2 {
        let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
        if let Some(a) = &auth {
            req.push_str(&format!("Proxy-Authorization: Basic {a}\r\n"));
        }
        req.push_str("\r\n");
        reader.get_mut().write_all(req.as_bytes()).await?;
        let status = read_status(&mut reader).await?;
        if (200..300).contains(&status) {
            return Ok(reader);
        }
        if status == 407 && attempt == 0 && user.is_some() {
            let password = ask_password().await.ok_or_else(|| err("cancelled"))?;
            let token = zeroize::Zeroizing::new(format!("{}:{}", user.unwrap_or(""), password));
            auth = Some(base64::engine::general_purpose::STANDARD.encode(token.as_bytes()));
            reader = BufReader::new(tokio::net::TcpStream::connect(peer).await?);
            continue;
        }
        return Err(err(format!("HTTP proxy answered {status}")));
    }
    Err(err("HTTP proxy authentication failed"))
}

/// Status code of the response, headers consumed.
async fn read_status<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> std::io::Result<u16> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| err(format!("bad HTTP proxy reply: {}", line.trim())))?;
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if line.len() > 8192 {
            return Err(err("HTTP proxy header too long"));
        }
    }
    Ok(status)
}

/// Run the local proxy command (no shell: split on whitespace).
fn command(cmd: &str, host: &str, port: u16) -> std::io::Result<BoxStream> {
    let port = port.to_string();
    let parts: Vec<String> = cmd
        .split_whitespace()
        .map(|w| w.replace("%h", host).replace("%p", &port))
        .collect();
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| err("empty proxy command"))?;
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| err(format!("proxy command {program}: {e}")))?;
    let stdin = child.stdin.take().ok_or_else(|| err("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| err("no stdout"))?;
    // Keep the child alive as long as the stream.
    Ok(Box::pin(ChildStream {
        io: tokio::io::join(stdout, stdin),
        _child: child,
    }))
}

struct ChildStream {
    io: tokio::io::Join<tokio::process::ChildStdout, tokio::process::ChildStdin>,
    _child: tokio::process::Child,
}

impl AsyncRead for ChildStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for ChildStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

/// `%host`, `%port`, `%user` (and `%h`, `%p`, `%%`) in a proxy command.
pub fn substitute(template: &str, host: &str, port: u16, user: Option<&str>) -> String {
    let mut out = String::with_capacity(template.len() + host.len());
    let mut rest = template;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let (value, len) = if rest.starts_with("host") {
            (host.to_string(), 4)
        } else if rest.starts_with("port") {
            (port.to_string(), 4)
        } else if rest.starts_with("user") {
            (user.unwrap_or_default().to_string(), 4)
        } else if rest.starts_with('h') {
            (host.to_string(), 1)
        } else if rest.starts_with('p') {
            (port.to_string(), 1)
        } else if rest.starts_with('%') {
            ("%".to_string(), 1)
        } else {
            ("%".to_string(), 0)
        };
        out.push_str(&value);
        rest = &rest[len..];
    }
    out.push_str(rest);
    out
}

/// The line sent to a Telnet proxy: [`substitute`]d, with `\n`, `\r`,
/// `\t` and `\\` escapes (as typed in the settings).
pub fn telnet_command(template: &str, host: &str, port: u16, user: Option<&str>) -> Vec<u8> {
    let template = if template.trim().is_empty() {
        DEFAULT_TELNET_COMMAND
    } else {
        template
    };
    let text = substitute(template, host, port, user);
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some(other) => {
                out.push(b'\\');
                let mut buf = [0; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}

/// From session options 3_20 (type), 3_21 host, 3_22 port, 3_23 login,
/// 3_27 command (local proxy command, Telnet command or command run on an
/// SSH proxy). `None` = direct connection.
pub fn from_options(get: &dyn Fn(&str) -> Option<String>) -> Option<Proxy> {
    let command = get("3_27").filter(|c| !c.trim().is_empty());
    let kind = match get("3_20")?.trim() {
        "1" => ProxyKind::Socks4,
        "2" => ProxyKind::Socks5,
        "3" => ProxyKind::Http,
        "4" => ProxyKind::Telnet(command.unwrap_or_else(|| DEFAULT_TELNET_COMMAND.into())),
        "5" => ProxyKind::Command(command?),
        "6" => ProxyKind::SshForward,
        "7" => ProxyKind::SshCommand(command.unwrap_or_else(|| DEFAULT_SSH_COMMAND.into())),
        _ => return None,
    };
    let host = get("3_21").unwrap_or_default().trim().to_string();
    if host.is_empty() && !matches!(kind, ProxyKind::Command(_)) {
        return None;
    }
    Some(Proxy {
        kind,
        host,
        port: get("3_22")
            .and_then(|p| p.trim().parse().ok())
            .filter(|&p| p > 0)
            .unwrap_or(1080),
        user: get("3_23")
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn no_password() -> impl FnMut() -> PasswordFuture + Send {
        || Box::pin(async { None })
    }

    #[tokio::test]
    async fn socks5_with_password() {
        let (mut client, mut server) = duplex(1024);
        let srv = tokio::spawn(async move {
            let mut hello = [0u8; 4];
            server.read_exact(&mut hello).await.unwrap();
            assert_eq!(hello, [5, 2, 0, 2]);
            server.write_all(&[5, 2]).await.unwrap();
            let mut auth = [0u8; 11];
            server.read_exact(&mut auth).await.unwrap();
            assert_eq!(&auth, b"\x01\x03bob\x05s3cr3");
            server.write_all(&[1, 0]).await.unwrap();
            let mut req = [0u8; 4 + 1 + 9 + 2];
            server.read_exact(&mut req).await.unwrap();
            assert_eq!(&req[..5], &[5, 1, 0, 3, 9]);
            assert_eq!(&req[5..14], b"host.test");
            assert_eq!(&req[14..], &22u16.to_be_bytes());
            server
                .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        });
        let mut ask = || -> PasswordFuture { Box::pin(async { Some("s3cr3".to_string()) }) };
        socks5(&mut client, "host.test", 22, Some("bob"), &mut ask)
            .await
            .unwrap();
        srv.await.unwrap();
    }

    #[tokio::test]
    async fn socks4a_and_refusal() {
        let (mut client, mut server) = duplex(1024);
        let srv = tokio::spawn(async move {
            let mut req = vec![0u8; 8 + 1 + 10];
            server.read_exact(&mut req).await.unwrap();
            assert_eq!(&req[..8], &[4, 1, 0, 22, 0, 0, 0, 1]);
            assert_eq!(&req[8..], b"\0host.test\0");
            server
                .write_all(&[0, 0x5b, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        });
        let e = socks4(&mut client, "host.test", 22, None)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("refused"));
        srv.await.unwrap();
        let mut ask = no_password();
        let (mut c2, mut s2) = duplex(64);
        tokio::spawn(async move {
            let mut b = [0u8; 3];
            s2.read_exact(&mut b).await.unwrap();
            s2.write_all(&[5, 0xff]).await.unwrap();
        });
        assert!(socks5(&mut c2, "h", 22, None, &mut ask).await.is_err());
    }

    #[tokio::test]
    async fn http_connect_ok_keeps_extra_bytes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 512];
            let n = s.read(&mut buf).await.unwrap();
            assert!(
                String::from_utf8_lossy(&buf[..n]).starts_with("CONNECT host.test:22 HTTP/1.1\r\n")
            );
            s.write_all(b"HTTP/1.1 200 Connection established\r\nVia: x\r\n\r\nSSH-2.0-test\r\n")
                .await
                .unwrap();
        });
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut ask = no_password();
        let mut reader = http_connect(tcp, "host.test", 22, None, &mut ask)
            .await
            .unwrap();
        let mut banner = String::new();
        reader.read_line(&mut banner).await.unwrap();
        assert_eq!(banner, "SSH-2.0-test\r\n");
    }

    #[test]
    fn options() {
        let opts = |pairs: &'static [(&'static str, &'static str)]| {
            move |id: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == id)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(from_options(&opts(&[])), None);
        assert_eq!(from_options(&opts(&[("3_20", "0"), ("3_21", "p")])), None);
        assert_eq!(
            from_options(&opts(&[
                ("3_20", "2"),
                ("3_21", "proxy"),
                ("3_22", "9050"),
                ("3_23", "me")
            ])),
            Some(Proxy {
                kind: ProxyKind::Socks5,
                host: "proxy".into(),
                port: 9050,
                user: Some("me".into())
            })
        );
        assert!(matches!(
            from_options(&opts(&[("3_20", "5"), ("3_27", "nc %h %p")])).map(|p| p.kind),
            Some(ProxyKind::Command(c)) if c == "nc %h %p"
        ));
        let kind =
            |o: &'static [(&'static str, &'static str)]| from_options(&opts(o)).map(|p| p.kind);
        assert_eq!(
            kind(&[("3_20", "4"), ("3_21", "p")]),
            Some(ProxyKind::Telnet(DEFAULT_TELNET_COMMAND.into()))
        );
        assert_eq!(
            kind(&[("3_20", "6"), ("3_21", "p")]),
            Some(ProxyKind::SshForward)
        );
        assert_eq!(
            kind(&[("3_20", "7"), ("3_21", "p")]),
            Some(ProxyKind::SshCommand(DEFAULT_SSH_COMMAND.into()))
        );
        assert_eq!(
            kind(&[("3_20", "7"), ("3_21", "p"), ("3_27", "ncat %h %p")]),
            Some(ProxyKind::SshCommand("ncat %h %p".into()))
        );
        // The SSH kinds need a proxy host too.
        assert_eq!(kind(&[("3_20", "6")]), None);
    }

    #[test]
    fn proxy_command_substitutions() {
        assert_eq!(
            substitute("nc %host %port %h:%p %user 100%% %x", "srv", 22, Some("me")),
            "nc srv 22 srv:22 me 100% %x"
        );
        assert_eq!(
            telnet_command(DEFAULT_TELNET_COMMAND, "srv", 22, None),
            b"connect srv 22\n"
        );
        assert_eq!(
            telnet_command(r"open %host %port\r\n\\ \q", "h", 1, None),
            b"open h 1\r\n\\ \\q"
        );
        assert_eq!(telnet_command("  ", "h", 1, None), b"connect h 1\n");
    }

    #[tokio::test]
    async fn telnet_proxy_sends_its_line_first() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 64];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(b"pong").await.unwrap();
            buf.truncate(n);
            buf
        });
        let proxy = Proxy {
            kind: ProxyKind::Telnet(DEFAULT_TELNET_COMMAND.into()),
            host: "127.0.0.1".into(),
            port,
            user: None,
        };
        let mut ask = no_password();
        let mut stream = connect(&proxy, "target.example", 2222, &mut ask)
            .await
            .unwrap();
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"pong");
        assert_eq!(server.await.unwrap(), b"connect target.example 2222\n");
    }
}
