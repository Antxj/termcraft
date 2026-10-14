//! Running network services of the "Servers" window, with their
//! "Server output" (the last lines are kept).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use tc_commands::{CommandError, ServerInfo};
use tc_core::settings::ServersSettings;
use tc_servers::ServerHandle;

/// Lines of "Server output" kept per server.
const LOG_LINES: usize = 500;

#[derive(Default)]
pub struct Servers {
    running: BTreeMap<&'static str, ServerHandle>,
    logs: BTreeMap<&'static str, VecDeque<String>>,
}

fn name_of(server: &str) -> Result<&'static str, CommandError> {
    tc_commands::SERVERS
        .iter()
        .copied()
        .find(|n| *n == server)
        .ok_or_else(|| CommandError::InvalidArgs(format!("unknown server {server:?}")))
}

/// The root folder of a server: as set, else `default` (a dedicated,
/// initially empty folder — never the home folder, which holds keys and
/// settings that must not be shared or overwritten).
fn root(dir: &str, default: &std::path::Path) -> Result<PathBuf, CommandError> {
    let dir = dir.trim();
    if !dir.is_empty() {
        return Ok(PathBuf::from(dir));
    }
    std::fs::create_dir_all(default)
        .map_err(|e| CommandError::Failed(format!("{}: {e}", default.display())))?;
    Ok(default.to_path_buf())
}

impl Servers {
    /// Start `server`; an empty root folder means `default_root`.
    pub fn start(
        &mut self,
        server: &str,
        s: &ServersSettings,
        default_root: &std::path::Path,
    ) -> Result<String, CommandError> {
        let name = name_of(server)?;
        if self.is_running(name) {
            return Err(CommandError::InvalidArgs(format!(
                "{name} is already running"
            )));
        }
        let started = match name {
            "http" => tc_servers::http::start(tc_servers::http::HttpConfig {
                root: root(&s.http.root, default_root)?,
                port: s.http.port,
                bind: "0.0.0.0".into(),
                list_directories: s.http.list_directories,
                stop_after: s.http.stop_after.duration(),
            }),
            _ => tc_servers::tftp::start(tc_servers::tftp::TftpConfig {
                root: root(&s.tftp.root, default_root)?,
                port: s.tftp.port,
                bind: "0.0.0.0".into(),
                buffer_size: s.tftp.buffer_size as usize,
                stop_after: s.tftp.stop_after.duration(),
            }),
        };
        let handle = started.map_err(|e| {
            let hint = if e.kind() == std::io::ErrorKind::PermissionDenied {
                " (ports below 1024 may need administrator rights; choose another port)"
            } else {
                ""
            };
            let msg = format!("{name}: {e}{hint}");
            self.push(name, msg.clone());
            CommandError::Failed(msg)
        })?;
        let address = handle.addr.to_string();
        self.running.insert(name, handle);
        Ok(address)
    }

    pub fn stop(&mut self, server: &str) -> Result<(), CommandError> {
        let name = name_of(server)?;
        match self.running.remove(name) {
            Some(handle) => {
                handle.stop();
                self.push(name, format!("{} server stopped", name.to_uppercase()));
                Ok(())
            }
            None => Err(CommandError::NotFound(format!("{name} is not running"))),
        }
    }

    pub fn is_running(&self, name: &str) -> bool {
        self.running.get(name).is_some_and(ServerHandle::is_running)
    }

    pub fn list(&self) -> Vec<ServerInfo> {
        tc_commands::SERVERS
            .iter()
            .map(|name| ServerInfo {
                server: (*name).to_string(),
                running: self.is_running(name),
                address: self
                    .running
                    .get(name)
                    .filter(|h| h.is_running())
                    .map(|h| h.addr.to_string()),
            })
            .collect()
    }

    pub fn running_count(&self) -> usize {
        self.running.values().filter(|h| h.is_running()).count()
    }

    pub fn log(&self, name: &str) -> impl Iterator<Item = &String> {
        self.logs.get(name).into_iter().flatten()
    }

    fn push(&mut self, name: &'static str, line: String) {
        let log = self.logs.entry(name).or_default();
        if log.len() == LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }

    /// Collect new output; returns the lines worth a notification
    /// ("Show message after successful download / upload").
    pub fn pump(&mut self, s: &ServersSettings) -> Vec<String> {
        let mut notify = Vec::new();
        let mut lines = Vec::new();
        for (name, handle) in &self.running {
            for line in handle.log() {
                let tftp = *name == "tftp";
                if tftp
                    && line.contains(" done (")
                    && ((s.tftp.notify_download && line.contains(" download of "))
                        || (s.tftp.notify_upload && line.contains(" upload of ")))
                {
                    notify.push(format!("TFTP: {line}"));
                }
                lines.push((*name, line));
            }
        }
        for (name, line) in lines {
            self.push(name, line);
        }
        notify
    }
}
