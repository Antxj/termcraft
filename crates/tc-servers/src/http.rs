//! "HTTP server": serves the files of a folder (GET / HEAD only), with an
//! optional listing of folders ("Allow users to list directory content").
//! Read-only; requests stay inside the root folder; one thread per
//! connection, at most [`MAX_CLIENTS`] at a time.

use crate::{Ctx, ServerHandle, resolve};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Connections served at the same time (others get 503).
pub const MAX_CLIENTS: usize = 32;
/// Longest request head accepted.
const MAX_HEAD: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub root: PathBuf,
    pub port: u16,
    /// Listen address (all interfaces by default, like the reference).
    pub bind: String,
    /// "Allow users to list directory content".
    pub list_directories: bool,
    /// "Stop server after".
    pub stop_after: Option<Duration>,
}

pub fn start(config: HttpConfig) -> std::io::Result<ServerHandle> {
    if !config.root.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not a folder", config.root.display()),
        ));
    }
    let listener = TcpListener::bind((config.bind.as_str(), config.port))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let (ctx, log, stop) = Ctx::new(config.stop_after);
    ctx.say(format!(
        "HTTP server started on {addr}, serving {}",
        config.root.display()
    ));
    let worker = {
        let ctx = ctx.clone();
        let config = Arc::new(config);
        let busy = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new()
            .name("tc-http".into())
            .spawn(move || accept_loop(&listener, &ctx, &config, &busy))?
    };
    Ok(crate::handle(ctx, log, stop, addr, vec![worker]))
}

fn accept_loop(
    listener: &TcpListener,
    ctx: &Ctx,
    config: &Arc<HttpConfig>,
    busy: &Arc<AtomicUsize>,
) {
    while !ctx.stopping() {
        match listener.accept() {
            Ok((stream, peer)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
                if busy.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
                    busy.fetch_sub(1, Ordering::AcqRel);
                    let mut s = stream;
                    let _ = respond(
                        &mut s,
                        503,
                        "Service Unavailable",
                        "text/plain",
                        b"busy",
                        false,
                    );
                    continue;
                }
                let (ctx, config, slot) = (ctx.clone(), config.clone(), busy.clone());
                let spawned = std::thread::Builder::new()
                    .name("tc-http-client".into())
                    .spawn(move || {
                        let line = serve(stream, &config).unwrap_or_else(|e| format!("error: {e}"));
                        ctx.say(format!("{peer} {line}"));
                        slot.fetch_sub(1, Ordering::AcqRel);
                    });
                if spawned.is_err() {
                    busy.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                ctx.say(format!("accept failed: {e}"));
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    ctx.say("HTTP server stopped".into());
}

fn respond(
    s: &mut TcpStream,
    code: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )?;
    if !head_only {
        s.write_all(body)?;
    }
    s.flush()
}

/// `%41` → `A`; `None` for broken escapes or non-UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().filter(|s| !s.contains('\0'))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("txt" | "log" | "md" | "ini" | "conf" | "cfg") => "text/plain; charset=utf-8",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Serve one request; returns the log line.
fn serve(stream: TcpStream, config: &HttpConfig) -> std::io::Result<String> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = String::new();
    let mut first = String::new();
    reader
        .by_ref()
        .take(MAX_HEAD as u64)
        .read_line(&mut first)?;
    // Skip the headers (bounded).
    loop {
        let mut line = String::new();
        let n = reader.by_ref().take(MAX_HEAD as u64).read_line(&mut line)?;
        head.push_str(&line);
        if n == 0 || line == "\r\n" || line == "\n" || head.len() > MAX_HEAD {
            break;
        }
    }
    let mut s = stream;
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let head_only = method == "HEAD";
    if method != "GET" && !head_only {
        respond(
            &mut s,
            405,
            "Method Not Allowed",
            "text/plain",
            b"read-only server",
            false,
        )?;
        return Ok(format!("{method} refused (405)"));
    }
    let path_part = target.split(['?', '#']).next().unwrap_or("/");
    let Some(decoded) = percent_decode(path_part) else {
        respond(
            &mut s,
            400,
            "Bad Request",
            "text/plain",
            b"bad path",
            head_only,
        )?;
        return Ok(format!("{method} {target} 400"));
    };
    let Some(path) = resolve(&config.root, &decoded, true) else {
        respond(
            &mut s,
            404,
            "Not Found",
            "text/plain",
            b"not found",
            head_only,
        )?;
        return Ok(format!("{method} {decoded} 404"));
    };
    if path.is_dir() {
        let index = path.join("index.html");
        if index.is_file() {
            return send_file(&mut s, &index, method, &decoded, head_only);
        }
        if !config.list_directories {
            respond(
                &mut s,
                403,
                "Forbidden",
                "text/plain",
                b"listing disabled",
                head_only,
            )?;
            return Ok(format!("{method} {decoded} 403"));
        }
        let body = listing(&path, &decoded);
        respond(
            &mut s,
            200,
            "OK",
            "text/html; charset=utf-8",
            body.as_bytes(),
            head_only,
        )?;
        return Ok(format!("{method} {decoded} 200 (listing)"));
    }
    send_file(&mut s, &path, method, &decoded, head_only)
}

fn send_file(
    s: &mut TcpStream,
    path: &Path,
    method: &str,
    shown: &str,
    head_only: bool,
) -> std::io::Result<String> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            respond(s, 403, "Forbidden", "text/plain", b"cannot read", head_only)?;
            return Ok(format!("{method} {shown} 403"));
        }
    };
    let len = file.metadata()?.len();
    write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {len}\r\nConnection: close\r\n\
         X-Content-Type-Options: nosniff\r\n\r\n",
        content_type(path)
    )?;
    if !head_only {
        std::io::copy(&mut file, s)?;
    }
    s.flush()?;
    Ok(format!("{method} {shown} 200 ({len} bytes)"))
}

fn listing(dir: &Path, shown: &str) -> String {
    let mut names: Vec<(bool, String)> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .take(10_000)
                .map(|e| {
                    let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                    (is_dir, e.file_name().to_string_lossy().into_owned())
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let base = if shown.ends_with('/') {
        shown.to_string()
    } else {
        format!("{shown}/")
    };
    let mut html = format!(
        "<!doctype html><meta charset=utf-8><title>Index of {0}</title><h1>Index of {0}</h1><ul>",
        html_escape(&base)
    );
    if base != "/" {
        html.push_str("<li><a href=\"../\">../</a></li>");
    }
    for (is_dir, name) in names {
        let slash = if is_dir { "/" } else { "" };
        html.push_str(&format!(
            "<li><a href=\"{}{slash}\">{}{slash}</a></li>",
            percent_encode(&format!("{base}{name}")),
            html_escape(&name)
        ));
    }
    html.push_str("</ul>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(addr: std::net::SocketAddr, request: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_files_and_refuses_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a b.txt"), b"hello").unwrap();
        let server = start(HttpConfig {
            root: dir.path().to_path_buf(),
            port: 0,
            bind: "127.0.0.1".into(),
            list_directories: false,
            stop_after: None,
        })
        .unwrap();
        let addr = server.addr;
        let ok = get(addr, "GET /a%20b.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            ok.starts_with("HTTP/1.1 200") && ok.ends_with("hello"),
            "{ok}"
        );
        let head = get(addr, "HEAD /a%20b.txt HTTP/1.1\r\n\r\n");
        assert!(head.contains("Content-Length: 5") && !head.ends_with("hello"));
        assert!(get(addr, "GET /../Cargo.toml HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        assert!(get(addr, "GET /%2e%2e/x HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        assert!(
            get(addr, "GET / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403"),
            "no listing"
        );
        assert!(get(addr, "PUT /x HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405"));
        assert!(get(addr, "GET /%zz HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 400"));
        let lines: Vec<String> = server.log().collect();
        assert!(lines[0].starts_with("HTTP server started"), "{lines:?}");
        server.stop();
    }

    #[test]
    fn listing_escapes_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a&b'sub")).unwrap();
        std::fs::write(dir.path().join("f.txt"), b"x").unwrap();
        let html = listing(dir.path(), "/");
        assert!(html.contains("a&amp;b&#39;sub/") && !html.contains("a&b'sub"));
        assert!(html.contains("href=\"/a%26b%27sub/\""));
        assert!(
            html.find("sub").unwrap() < html.find("f.txt").unwrap(),
            "folders first"
        );
        assert_eq!(percent_decode("a%2Fb").as_deref(), Some("a/b"));
        assert_eq!(percent_decode("%00"), None);
    }

    #[test]
    fn stops_after_the_time_limit() {
        let dir = tempfile::tempdir().unwrap();
        let server = start(HttpConfig {
            root: dir.path().to_path_buf(),
            port: 0,
            bind: "127.0.0.1".into(),
            list_directories: true,
            stop_after: Some(Duration::from_millis(100)),
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert!(!server.is_running());
    }
}
