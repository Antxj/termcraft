//! SSH X11 forwarding to a local X server (VcXsrv / Xming on Windows,
//! the native one on Linux, XQuartz on macOS).
//!
//! Like OpenSSH, the server only gets a random fake cookie. Each forwarded
//! connection starts with the X11 setup request: the fake cookie is
//! checked and replaced by the real one of the local display (from
//! `xauth`), or removed when the display has none. Connections with a
//! wrong cookie are refused.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const PROTO: &str = "MIT-MAGIC-COOKIE-1";

/// A local display, `[host]:number[.screen]` or a socket path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    /// `None` = the local Unix socket.
    pub host: Option<String>,
    pub number: u16,
    pub screen: u32,
}

/// `:0`, `:1.0`, `localhost:10.0`, `127.0.0.1:0`, `unix:0`.
pub fn parse_display(s: &str) -> Option<Display> {
    let s = s.trim();
    let (host, rest) = s.rsplit_once(':')?;
    let (number, screen) = match rest.split_once('.') {
        Some((n, sc)) => (n.parse().ok()?, sc.parse().ok()?),
        None => (rest.parse().ok()?, 0),
    };
    let host = match host {
        "" | "unix" => None,
        h => Some(h.to_string()),
    };
    // A Unix socket cannot be used on Windows: use TCP to localhost.
    let host = if cfg!(windows) {
        host.or_else(|| Some("127.0.0.1".into()))
    } else {
        host
    };
    Some(Display {
        host,
        number,
        screen,
    })
}

/// The display to forward to: `$DISPLAY`, else `localhost:0` on Windows
/// (VcXsrv / Xming defaults). `None` on Unix without `$DISPLAY`.
pub fn default_display() -> Option<String> {
    std::env::var("DISPLAY")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .or_else(|| cfg!(windows).then(|| "localhost:0.0".to_string()))
}

pub(crate) struct X11State {
    pub(crate) display: Display,
    display_name: String,
    fake: [u8; 16],
    /// Real cookie of the local display (read once, lazily).
    real: std::sync::OnceLock<Option<Vec<u8>>>,
}

impl X11State {
    pub(crate) fn new(display_name: &str) -> Option<Self> {
        let display = parse_display(display_name)?;
        let mut fake = [0u8; 16];
        use rand_core::Rng as _;
        russh::keys::key::safe_rng().fill_bytes(&mut fake);
        Some(Self {
            display,
            display_name: display_name.to_string(),
            fake,
            real: std::sync::OnceLock::new(),
        })
    }

    pub(crate) fn fake_hex(&self) -> String {
        hex::encode(self.fake)
    }

    pub(crate) fn protocol(&self) -> &'static str {
        PROTO
    }

    fn real_cookie(&self) -> Option<&[u8]> {
        self.real
            .get_or_init(|| xauth_cookie(&self.display_name))
            .as_deref()
    }
}

/// `xauth list <display>` → the MIT-MAGIC-COOKIE-1 data, if any.
fn xauth_cookie(display: &str) -> Option<Vec<u8>> {
    let mut cmd = std::process::Command::new("xauth");
    cmd.args(["list", display]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let mut f = l.split_whitespace();
        let (_, proto, data) = (f.next()?, f.next()?, f.next()?);
        (proto == PROTO).then(|| hex::decode(data).ok()).flatten()
    })
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

/// Check and rewrite the X11 connection setup request.
/// `Ok(None)` = more bytes are needed; `Err` = refused.
pub(crate) fn rewrite_setup(
    buf: &[u8],
    fake: &[u8],
    real: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, &'static str> {
    if buf.len() < 12 {
        return Ok(None);
    }
    let big = match buf[0] {
        b'B' => true,
        b'l' => false,
        _ => return Err("not an X11 connection"),
    };
    let u16_at = |i: usize| {
        let b = [buf[i], buf[i + 1]];
        usize::from(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let (name_len, data_len) = (u16_at(6), u16_at(8));
    let total = 12 + pad4(name_len) + pad4(data_len);
    if buf.len() < total {
        return Ok(None);
    }
    let name = &buf[12..12 + name_len];
    let data_at = 12 + pad4(name_len);
    let data = &buf[data_at..data_at + data_len];
    if name != PROTO.as_bytes() || data != fake {
        return Err("wrong X11 authentication cookie");
    }
    let put_u16 = |v: usize, out: &mut Vec<u8>| {
        let v = v as u16;
        out.extend_from_slice(&if big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        });
    };
    let mut out = buf[..6].to_vec();
    match real {
        Some(cookie) => {
            put_u16(PROTO.len(), &mut out);
            put_u16(cookie.len(), &mut out);
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(PROTO.as_bytes());
            out.resize(12 + pad4(PROTO.len()), 0);
            out.extend_from_slice(cookie);
            out.resize(12 + pad4(PROTO.len()) + pad4(cookie.len()), 0);
        }
        None => {
            put_u16(0, &mut out);
            put_u16(0, &mut out);
            out.extend_from_slice(&[0, 0]);
        }
    }
    out.extend_from_slice(&buf[total..]);
    Ok(Some(out))
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

async fn connect_display(d: &Display) -> std::io::Result<Box<dyn Io>> {
    let port = 6000 + d.number;
    match &d.host {
        Some(h) => {
            let host = if h == "localhost" { "127.0.0.1" } else { h };
            let tcp = tokio::net::TcpStream::connect((host, port)).await?;
            tcp.set_nodelay(true)?;
            Ok(Box::new(tcp))
        }
        None => {
            #[cfg(unix)]
            {
                let path = format!("/tmp/.X11-unix/X{}", d.number);
                Ok(Box::new(tokio::net::UnixStream::connect(path).await?))
            }
            #[cfg(not(unix))]
            {
                let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
                Ok(Box::new(tcp))
            }
        }
    }
}

/// Bridge one forwarded X11 connection to the local display.
pub(crate) async fn bridge<S>(mut remote: S, state: &X11State) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let setup = loop {
        let n = remote.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            return Err("X11 setup too large".into());
        }
        if let Some(out) = rewrite_setup(&buf, &state.fake, state.real_cookie())? {
            break out;
        }
    };
    let mut local = connect_display(&state.display)
        .await
        .map_err(|e| format!("cannot reach the X server: {e}"))?;
    local.write_all(&setup).await.map_err(|e| e.to_string())?;
    let _ = tokio::io::copy_bidirectional(&mut remote, &mut local).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(big: bool, name: &[u8], data: &[u8], extra: &[u8]) -> Vec<u8> {
        let u16b = |v: usize| {
            let v = v as u16;
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let mut b = vec![if big { b'B' } else { b'l' }, 0];
        b.extend_from_slice(&u16b(11));
        b.extend_from_slice(&u16b(0));
        b.extend_from_slice(&u16b(name.len()));
        b.extend_from_slice(&u16b(data.len()));
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(name);
        b.resize(12 + pad4(name.len()), 0);
        b.extend_from_slice(data);
        b.resize(12 + pad4(name.len()) + pad4(data.len()), 0);
        b.extend_from_slice(extra);
        b
    }

    #[test]
    fn displays() {
        let d = parse_display(":1.2").unwrap();
        assert_eq!((d.number, d.screen), (1, 2));
        if cfg!(windows) {
            assert_eq!(d.host.as_deref(), Some("127.0.0.1"));
        } else {
            assert_eq!(d.host, None);
        }
        assert_eq!(
            parse_display("localhost:10.0").unwrap().host.as_deref(),
            Some("localhost")
        );
        assert!(parse_display("nonsense").is_none());
    }

    #[test]
    fn cookie_is_checked_and_replaced() {
        let fake = [7u8; 16];
        let real = [9u8; 16];
        for big in [true, false] {
            let req = setup(big, PROTO.as_bytes(), &fake, b"rest");
            assert_eq!(
                rewrite_setup(&req[..10], &fake, None),
                Ok(None),
                "needs more"
            );
            let out = rewrite_setup(&req, &fake, Some(&real)).unwrap().unwrap();
            assert_eq!(out, setup(big, PROTO.as_bytes(), &real, b"rest"));
            let stripped = rewrite_setup(&req, &fake, None).unwrap().unwrap();
            assert_eq!(stripped, setup(big, b"", b"", b"rest"));
            let wrong = setup(big, PROTO.as_bytes(), &[1u8; 16], b"");
            assert!(rewrite_setup(&wrong, &fake, None).is_err());
        }
        assert!(rewrite_setup(b"GET / HTTP/1.1\r\n", &fake, None).is_err());
    }
}
