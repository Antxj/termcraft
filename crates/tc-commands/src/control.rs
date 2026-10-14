//! Loopback control channel to a running TermCraft window: a second
//! `termcraft --bookmark …`, `termcraft-cli gui …` or an agent sends
//! commands to it. The window listens on `127.0.0.1` only, on a random
//! port, and writes the port and a random token to a file only this user
//! can read; every request must carry the token. One JSON line in, one
//! JSON line out:
//!
//! ```text
//! → {"token":"…","command":{"command":"tab_open","args":{"session":"web01"}}}
//! ← {"ok":true,"result":{"tab":3}}
//! ```

use crate::{Command, CommandError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// Largest request accepted.
pub const MAX_REQUEST: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

/// `control.json`: where the running window listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub token: String,
    pub command: Command,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The window cannot run this command (it is for files only, such as
    /// `update_check`): the caller may run it itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsupported: bool,
}

impl Response {
    /// The answer to a command the window ran.
    pub fn from_result(result: Result<Value, CommandError>) -> Self {
        match result {
            Ok(result) => Self {
                ok: true,
                result,
                error: None,
                unsupported: false,
            },
            Err(e) => Self {
                ok: false,
                result: Value::Null,
                unsupported: matches!(e, CommandError::Unsupported(_)),
                error: Some(e.to_string()),
            },
        }
    }

    /// A request the window did not run.
    pub fn refused(error: &str) -> Self {
        Self {
            ok: false,
            result: Value::Null,
            error: Some(error.into()),
            unsupported: false,
        }
    }
}

/// Why [`request`] got no result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    #[error("TermCraft is not running")]
    NotRunning,
    /// The window does not run this command.
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Failed(String),
}

/// A new random token (two v4 UUIDs: 244 random bits).
pub fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Compare tokens without an early exit on the first difference.
pub fn token_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Write `control.json` so only this user can read it.
pub fn publish(path: &Path, endpoint: &Endpoint) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string(endpoint).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(text.as_bytes())?;
    }
    std::fs::rename(&tmp, path)
}

/// Remove `control.json` if it is still ours.
pub fn unpublish(path: &Path, endpoint: &Endpoint) {
    if read_endpoint(path).is_some_and(|e| e == *endpoint) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn read_endpoint(path: &Path) -> Option<Endpoint> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Read one request line (bounded) from a connection.
pub fn read_request(stream: &TcpStream) -> std::io::Result<Request> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(stream.take(MAX_REQUEST)).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(std::io::Error::other)
}

pub fn write_response(mut stream: &TcpStream, response: &Response) -> std::io::Result<()> {
    let mut text = serde_json::to_string(response).map_err(std::io::Error::other)?;
    text.push('\n');
    stream.write_all(text.as_bytes())
}

/// Send one command to the running window. `Err` when none is running
/// (no file, nothing listening) or it refused the command.
pub fn send(control_file: &Path, command: Command) -> Result<Value, String> {
    request(control_file, command).map_err(|e| e.to_string())
}

/// Run `command` in the running window.
pub fn request(control_file: &Path, command: Command) -> Result<Value, SendError> {
    let endpoint = read_endpoint(control_file).ok_or(SendError::NotRunning)?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|_| SendError::NotRunning)?;
    let failed = |e: std::io::Error| SendError::Failed(e.to_string());
    stream.set_read_timeout(Some(TIMEOUT)).map_err(failed)?;
    let request = Request {
        token: endpoint.token,
        command,
    };
    let mut text = serde_json::to_string(&request).map_err(|e| SendError::Failed(e.to_string()))?;
    text.push('\n');
    (&stream).write_all(text.as_bytes()).map_err(failed)?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST))
        .read_line(&mut line)
        .map_err(failed)?;
    let response: Response =
        serde_json::from_str(&line).map_err(|e| SendError::Failed(e.to_string()))?;
    match response {
        Response {
            ok: true, result, ..
        } => Ok(result),
        Response {
            unsupported: true,
            error,
            ..
        } => Err(SendError::Unsupported(error.unwrap_or_default())),
        Response { error, .. } => Err(SendError::Failed(error.unwrap_or_else(|| "refused".into()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn tokens() {
        let t = new_token();
        assert_eq!(t.len(), 64);
        assert_ne!(t, new_token());
        assert!(token_matches(&t, &t.clone()));
        assert!(!token_matches(&t, &t[..63]));
        assert!(!token_matches(&t, &format!("{}x", &t[..63])));
        assert!(!token_matches("", ""));
    }

    /// A fake window: checks the token and answers.
    #[test]
    fn round_trip_and_wrong_token() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("control.json");
        assert_eq!(
            request(&file, Command::AppVersion {}),
            Err(SendError::NotRunning)
        );

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = Endpoint {
            port: listener.local_addr().unwrap().port(),
            token: new_token(),
            pid: std::process::id(),
        };
        publish(&file, &endpoint).unwrap();
        let token = endpoint.token.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (stream, _) = listener.accept().unwrap();
                let req = read_request(&stream).unwrap();
                let response = if !token_matches(&token, &req.token) {
                    Response::refused("bad token")
                } else if matches!(req.command, Command::UpdateCheck {}) {
                    Response::from_result(Err(CommandError::Unsupported("update_check")))
                } else {
                    Response::from_result(Ok(serde_json::json!({ "command": req.command.name() })))
                };
                write_response(&stream, &response).unwrap();
            }
        });
        let v = send(&file, Command::AppVersion {}).unwrap();
        assert_eq!(v["command"], "app_version");
        assert!(matches!(
            request(&file, Command::UpdateCheck {}),
            Err(SendError::Unsupported(_))
        ));

        let mut forged = endpoint.clone();
        forged.token = new_token();
        publish(&file, &forged).unwrap();
        assert_eq!(
            request(&file, Command::AppVersion {}),
            Err(SendError::Failed("bad token".into()))
        );
        server.join().unwrap();

        unpublish(&file, &endpoint);
        assert!(file.exists(), "not ours any more: kept");
        unpublish(&file, &forged);
        assert!(!file.exists());
    }
}
