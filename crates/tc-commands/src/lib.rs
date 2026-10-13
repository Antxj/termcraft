//! The single command registry. Every menu item, CLI subcommand and MCP tool
//! is a [`Command`]; front-ends only translate input into commands and render
//! the JSON result. Adding a feature = add a variant + a [`CommandSpec`].

mod catalog;

pub use catalog::{CATALOG, CommandSpec, spec};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tc_core::{Session, SessionId, SessionStore};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", content = "args", rename_all = "snake_case")]
pub enum Command {
    AppVersion {},
    SessionList {},
    SessionAdd {
        session: Session,
    },
    SessionRemove {
        id: SessionId,
    },
    SessionImportMobaxterm {
        path: String,
    },
    SessionRename {
        id: SessionId,
        name: String,
    },
    SessionDuplicate {
        id: SessionId,
    },
    TabList {},
    TabOpen {
        /// Session name or id; omitted = local shell.
        #[serde(default)]
        session: Option<String>,
    },
    TabSend {
        /// Target tab; omitted = active tab.
        #[serde(default)]
        tab: Option<u64>,
        text: String,
    },
    /// Open a saved session by name/host, or an ad-hoc SSH tab for
    /// `[user@]host[:port]` (the sidebar "Quick connect" box).
    TabQuickConnect {
        target: String,
    },
    TabClose {
        #[serde(default)]
        tab: Option<u64>,
    },
    TabDuplicate {
        #[serde(default)]
        tab: Option<u64>,
    },
    TabRename {
        #[serde(default)]
        tab: Option<u64>,
        title: String,
    },
    UpdateCheck {},
}

impl Command {
    /// Build a command from a tool/command name plus a JSON object of arguments
    /// (the shape used by MCP `tools/call` and the CLI `run` subcommand).
    pub fn from_call(name: &str, args: Value) -> Result<Self, CommandError> {
        let args = if args.is_null() { json!({}) } else { args };
        serde_json::from_value(json!({ "command": name, "args": args }))
            .map_err(|e| CommandError::InvalidArgs(format!("{name}: {e}")))
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::AppVersion {} => "app_version",
            Self::SessionList {} => "session_list",
            Self::SessionAdd { .. } => "session_add",
            Self::SessionRemove { .. } => "session_remove",
            Self::SessionImportMobaxterm { .. } => "session_import_mobaxterm",
            Self::SessionRename { .. } => "session_rename",
            Self::SessionDuplicate { .. } => "session_duplicate",
            Self::TabList {} => "tab_list",
            Self::TabOpen { .. } => "tab_open",
            Self::TabSend { .. } => "tab_send",
            Self::TabQuickConnect { .. } => "tab_quick_connect",
            Self::TabClose { .. } => "tab_close",
            Self::TabDuplicate { .. } => "tab_duplicate",
            Self::TabRename { .. } => "tab_rename",
            Self::UpdateCheck {} => "update_check",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("not available in this front-end: {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: u64,
    pub title: String,
    pub active: bool,
}

/// What a front-end must provide. Session storage is mandatory; UI-only
/// capabilities default to `Unsupported` so headless hosts (CLI, MCP) work.
pub trait Host {
    fn store(&mut self) -> &mut SessionStore;
    fn persist(&mut self) -> Result<(), CommandError>;

    fn tab_list(&self) -> Result<Vec<TabInfo>, CommandError> {
        Err(CommandError::Unsupported("tabs require the GUI"))
    }
    fn tab_open(&mut self, _session: Option<Session>) -> Result<u64, CommandError> {
        Err(CommandError::Unsupported("tabs require the GUI"))
    }
    fn tab_send(&mut self, _tab: Option<u64>, _text: &str) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("tabs require the GUI"))
    }
    fn tab_close(&mut self, _tab: Option<u64>) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("tabs require the GUI"))
    }
    /// Re-open the session behind a tab in a new tab.
    fn tab_duplicate(&mut self, _tab: Option<u64>) -> Result<u64, CommandError> {
        Err(CommandError::Unsupported("tabs require the GUI"))
    }
    fn tab_rename(&mut self, _tab: Option<u64>, _title: &str) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("tabs require the GUI"))
    }
    fn update_check(&mut self) -> Result<Value, CommandError> {
        Err(CommandError::Unsupported("updater not wired"))
    }
}

pub fn execute(host: &mut dyn Host, cmd: Command) -> Result<Value, CommandError> {
    match cmd {
        Command::AppVersion {} => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
        Command::SessionList {} => to_json(&host.store().sessions),
        Command::SessionAdd { session } => {
            if host.store().find_by_name(&session.name).is_some() {
                return Err(CommandError::InvalidArgs(format!(
                    "a session named {:?} already exists",
                    session.name
                )));
            }
            let id = host.store().add(session);
            host.persist()?;
            Ok(json!({ "id": id }))
        }
        Command::SessionRemove { id } => {
            let removed = host
                .store()
                .remove(id)
                .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
            host.persist()?;
            to_json(&removed)
        }
        Command::SessionImportMobaxterm { path } => {
            let text = tc_import::read_text_lossy(std::path::Path::new(&path))
                .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?;
            let report = tc_import::mobaxterm::parse(&text);
            let mut imported = 0usize;
            let mut skipped_existing = 0usize;
            for session in report.sessions {
                if host.store().find_by_name(&session.name).is_some() {
                    skipped_existing += 1;
                } else {
                    host.store().add(session);
                    imported += 1;
                }
            }
            host.persist()?;
            Ok(json!({
                "imported": imported,
                "skipped_existing": skipped_existing,
                "unsupported": report.unsupported,
            }))
        }
        Command::SessionRename { id, name } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(CommandError::InvalidArgs("name must not be empty".into()));
            }
            if host
                .store()
                .sessions
                .iter()
                .any(|s| s.name == name && s.id != id)
            {
                return Err(CommandError::InvalidArgs(format!(
                    "a session named {name:?} already exists"
                )));
            }
            session_mut(host.store(), id)?.name = name;
            host.persist()?;
            Ok(json!({ "ok": true }))
        }
        Command::SessionDuplicate { id } => {
            let store = host.store();
            let original = session_mut(store, id)?.clone();
            let name = unique_name(store, &format!("{} (copy)", original.name));
            let copy = Session {
                id: SessionId::new(),
                name,
                last_used: None,
                ..original
            };
            let new_id = store.add(copy);
            host.persist()?;
            Ok(json!({ "id": new_id }))
        }
        Command::TabList {} => to_json(&host.tab_list()?),
        Command::TabOpen { session } => {
            let session = match session {
                None => None,
                Some(key) => {
                    let id = resolve_session(host.store(), &key)?.id;
                    let opened = session_mut(host.store(), id)?;
                    opened.last_used = Some(now_secs());
                    let opened = opened.clone();
                    host.persist()?;
                    Some(opened)
                }
            };
            Ok(json!({ "tab": host.tab_open(session)? }))
        }
        Command::TabQuickConnect { target } => {
            let target = target.trim();
            if target.is_empty() {
                return Err(CommandError::InvalidArgs("empty target".into()));
            }
            let saved = host.store().sessions.iter().find(|s| {
                s.name == target || session_host(s).is_some_and(|h| h.eq_ignore_ascii_case(target))
            });
            if let Some(saved) = saved {
                let id = saved.id.to_string();
                return execute(host, Command::TabOpen { session: Some(id) });
            }
            let session = parse_quick_target(target)
                .ok_or_else(|| CommandError::InvalidArgs(format!("cannot parse {target:?}")))?;
            Ok(json!({ "tab": host.tab_open(Some(session))? }))
        }
        Command::TabClose { tab } => {
            host.tab_close(tab)?;
            Ok(json!({ "ok": true }))
        }
        Command::TabDuplicate { tab } => Ok(json!({ "tab": host.tab_duplicate(tab)? })),
        Command::TabRename { tab, title } => {
            host.tab_rename(tab, &title)?;
            Ok(json!({ "ok": true }))
        }
        Command::TabSend { tab, text } => {
            host.tab_send(tab, &text)?;
            Ok(json!({ "ok": true }))
        }
        Command::UpdateCheck {} => host.update_check(),
    }
}

fn resolve_session(store: &SessionStore, key: &str) -> Result<Session, CommandError> {
    store
        .sessions
        .iter()
        .find(|s| s.name == key || s.id.to_string() == key)
        .cloned()
        .ok_or_else(|| CommandError::NotFound(format!("session {key:?}")))
}

fn session_mut(store: &mut SessionStore, id: SessionId) -> Result<&mut Session, CommandError> {
    store
        .sessions
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))
}

fn session_host(s: &Session) -> Option<&str> {
    use tc_core::SessionKind as K;
    match &s.kind {
        K::Ssh { host, .. }
        | K::Telnet { host, .. }
        | K::Rdp { host, .. }
        | K::Vnc { host, .. }
        | K::Sftp { host, .. }
        | K::Ftp { host, .. } => Some(host),
        K::LocalShell { .. } | K::Serial { .. } => None,
    }
}

/// `[user@]host[:port]` → unsaved SSH session. A host with several `:` is
/// taken as a bare IPv6 address.
fn parse_quick_target(target: &str) -> Option<Session> {
    let (user, rest) = match target.rsplit_once('@') {
        Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
        Some(_) => return None,
        None => (None, target),
    };
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) if !p.contains(':') => (h, p.parse().ok()?),
        _ => (rest, 22),
    };
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    Some(Session::new(
        target,
        tc_core::SessionKind::Ssh {
            host: host.into(),
            port,
            user,
            private_key: None,
            x11_forwarding: true,
        },
    ))
}

fn unique_name(store: &SessionStore, base: &str) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while store.find_by_name(&name).is_some() {
        name = format!("{base} {n}");
        n += 1;
    }
    name
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Sessions most recently opened first (home screen "Recent sessions").
pub fn recent_sessions(store: &SessionStore, limit: usize) -> Vec<&Session> {
    let mut used: Vec<&Session> = store
        .sessions
        .iter()
        .filter(|s| s.last_used.is_some())
        .collect();
    used.sort_by_key(|s| std::cmp::Reverse(s.last_used));
    used.truncate(limit);
    used
}

fn to_json<T: Serialize>(value: &T) -> Result<Value, CommandError> {
    serde_json::to_value(value).map_err(|e| CommandError::Failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemHost {
        store: SessionStore,
    }

    impl Host for MemHost {
        fn store(&mut self) -> &mut SessionStore {
            &mut self.store
        }
        fn persist(&mut self) -> Result<(), CommandError> {
            Ok(())
        }
    }

    #[test]
    fn every_catalog_example_parses_and_names_match() {
        for spec in CATALOG {
            let example: Value = serde_json::from_str(spec.example_args).unwrap();
            let cmd = Command::from_call(spec.name, example)
                .unwrap_or_else(|e| panic!("example for {} does not parse: {e}", spec.name));
            assert_eq!(cmd.name(), spec.name);
        }
    }

    #[test]
    fn add_list_remove() {
        let mut host = MemHost::default();
        let args = json!({ "session": { "name": "db", "type": "ssh", "host": "db.local" } });
        let out = execute(&mut host, Command::from_call("session_add", args).unwrap()).unwrap();
        let id: SessionId = serde_json::from_value(out["id"].clone()).unwrap();

        let list = execute(&mut host, Command::SessionList {}).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);

        execute(&mut host, Command::SessionRemove { id }).unwrap();
        assert!(host.store.sessions.is_empty());
    }

    #[test]
    fn duplicate_names_rejected() {
        let mut host = MemHost::default();
        let args = json!({ "session": { "name": "a", "type": "local_shell" } });
        execute(
            &mut host,
            Command::from_call("session_add", args.clone()).unwrap(),
        )
        .unwrap();
        assert!(execute(&mut host, Command::from_call("session_add", args).unwrap()).is_err());
    }

    #[test]
    fn rename_duplicate_and_recent() {
        let mut host = MemHost::default();
        let args = json!({ "session": { "name": "web", "type": "ssh", "host": "w" } });
        let out = execute(&mut host, Command::from_call("session_add", args).unwrap()).unwrap();
        let id: SessionId = serde_json::from_value(out["id"].clone()).unwrap();

        execute(&mut host, Command::SessionDuplicate { id }).unwrap();
        execute(&mut host, Command::SessionDuplicate { id }).unwrap();
        let names: Vec<_> = host
            .store
            .sessions
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["web", "web (copy)", "web (copy) 2"]);

        let taken = Command::SessionRename {
            id,
            name: "web (copy)".into(),
        };
        assert!(execute(&mut host, taken).is_err());
        let ok = Command::SessionRename {
            id,
            name: "web-1".into(),
        };
        execute(&mut host, ok).unwrap();
        assert_eq!(host.store.sessions[0].name, "web-1");

        assert!(recent_sessions(&host.store, 9).is_empty());
        // Headless tab_open fails, but the session is marked as used first.
        let _ = execute(
            &mut host,
            Command::TabOpen {
                session: Some("web-1".into()),
            },
        );
        let recent = recent_sessions(&host.store, 9);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].name, "web-1");
    }

    #[test]
    fn quick_connect_parsing() {
        use tc_core::SessionKind;
        let ssh = |t: &str| match parse_quick_target(t).map(|s| s.kind) {
            Some(SessionKind::Ssh {
                host, port, user, ..
            }) => Some((host, port, user)),
            _ => None,
        };
        assert_eq!(ssh("box"), Some(("box".into(), 22, None)));
        assert_eq!(
            ssh("root@10.0.0.1:2222"),
            Some(("10.0.0.1".into(), 2222, Some("root".into())))
        );
        assert_eq!(ssh("fe80::1"), Some(("fe80::1".into(), 22, None)));
        assert_eq!(ssh("@host"), None);
        assert_eq!(ssh("host:notaport"), None);
        assert_eq!(ssh("two words"), None);
    }

    #[test]
    fn headless_tabs_are_unsupported() {
        let mut host = MemHost::default();
        let err = execute(&mut host, Command::TabList {}).unwrap_err();
        assert!(matches!(err, CommandError::Unsupported(_)));
    }
}
