//! The window's end of the loopback control channel
//! (`tc_commands::control`): connections are read on a background thread,
//! commands run on the UI thread like menu actions, and the answer goes
//! back on the same connection.

use crossbeam_channel::{Receiver, Sender};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tc_commands::Command;
use tc_commands::control::{self, Endpoint, Response};

/// A command from another process and where to send its answer.
pub struct Incoming {
    pub command: Command,
    pub reply: Sender<Response>,
}

pub struct ControlServer {
    incoming: Receiver<Incoming>,
    endpoint: Endpoint,
    file: PathBuf,
}

impl ControlServer {
    /// Listen and publish `control.json`. `None` if the port cannot be
    /// opened (the app still works, just without remote control).
    pub fn start(file: PathBuf, repaint: Arc<dyn Fn() + Send + Sync>) -> Option<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        let endpoint = Endpoint {
            port: listener.local_addr().ok()?.port(),
            token: control::new_token(),
            pid: std::process::id(),
        };
        control::publish(&file, &endpoint).ok()?;
        let (tx, incoming) = crossbeam_channel::unbounded();
        let token = endpoint.token.clone();
        std::thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (tx, token, repaint) = (tx.clone(), token.clone(), repaint.clone());
                    let _ = std::thread::Builder::new()
                        .name("control-conn".into())
                        .spawn(move || serve(stream, &token, &tx, &*repaint));
                }
            })
            .ok()?;
        Some(Self {
            incoming,
            endpoint,
            file,
        })
    }

    /// Commands received since the last frame.
    pub fn take(&self) -> Vec<Incoming> {
        self.incoming.try_iter().collect()
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        control::unpublish(&self.file, &self.endpoint);
    }
}

fn refuse(stream: &TcpStream, error: &str) {
    let _ = control::write_response(
        stream,
        &Response {
            ok: false,
            result: serde_json::Value::Null,
            error: Some(error.into()),
        },
    );
}

fn serve(
    stream: TcpStream,
    token: &str,
    tx: &Sender<Incoming>,
    repaint: &(dyn Fn() + Send + Sync),
) {
    let request = match control::read_request(&stream) {
        Ok(r) => r,
        Err(_) => return refuse(&stream, "invalid request"),
    };
    if !control::token_matches(token, &request.token) {
        // Slow down guessing.
        std::thread::sleep(Duration::from_millis(250));
        return refuse(&stream, "bad token");
    }
    let (reply, answer) = crossbeam_channel::bounded(1);
    if tx
        .send(Incoming {
            command: request.command,
            reply,
        })
        .is_err()
    {
        return refuse(&stream, "TermCraft is closing");
    }
    repaint();
    match answer.recv_timeout(Duration::from_secs(30)) {
        Ok(response) => {
            let _ = control::write_response(&stream, &response);
        }
        Err(_) => refuse(&stream, "timeout"),
    }
}

/// Commands for the command-line options (reference style `-bookmark`
/// also accepted): `--bookmark NAME`, `--newtab CMD` / `--exec CMD` (a
/// local terminal running CMD), `--runmacro NAME`.
pub fn commands_from_args(
    args: &[String],
    macro_id: &dyn Fn(&str) -> Option<tc_core::MacroId>,
) -> Result<Vec<Command>, String> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let flag = arg.trim_start_matches('-').to_ascii_lowercase();
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match flag.as_str() {
            "bookmark" => out.push(Command::TabOpen {
                session: Some(value()?),
            }),
            "newtab" | "exec" => {
                let cmd = value()?;
                out.push(Command::TabOpen { session: None });
                out.push(Command::TabSend {
                    tab: None,
                    text: format!("{cmd}\r"),
                });
            }
            "runmacro" => {
                let name = value()?;
                let id =
                    macro_id(&name).ok_or_else(|| format!("there is no macro named {name:?}"))?;
                out.push(Command::MacroRun { id, tab: None });
            }
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_become_commands() {
        let args: Vec<String> = [
            "-bookmark",
            "web01",
            "--newtab",
            "top",
            "--runmacro",
            "deploy",
        ]
        .map(String::from)
        .to_vec();
        let id = tc_core::MacroId::default();
        let cmds = commands_from_args(&args, &|n| (n == "deploy").then_some(id)).unwrap();
        assert_eq!(cmds.len(), 4);
        assert!(matches!(&cmds[0], Command::TabOpen { session: Some(s) } if s == "web01"));
        assert!(matches!(&cmds[2], Command::TabSend { text, .. } if text == "top\r"));
        assert!(matches!(cmds[3], Command::MacroRun { id: m, .. } if m == id));
        assert!(commands_from_args(&["--bookmark".into()], &|_| None).is_err());
        assert!(commands_from_args(&["--runmacro".into(), "x".into()], &|_| None).is_err());
        assert!(commands_from_args(&["--evil".into()], &|_| None).is_err());
    }
}
