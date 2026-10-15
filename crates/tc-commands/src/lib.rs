//! The single command registry. Every menu item, CLI subcommand and MCP tool
//! is a [`Command`]; front-ends only translate input into commands and render
//! the JSON result. Adding a feature = add a variant + a [`CommandSpec`].

mod catalog;
pub mod control;

pub use catalog::{CATALOG, CommandSpec, spec};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tc_core::{
    Macro, MacroId, MacroStore, Session, SessionId, SessionStore, Tunnel, TunnelId, TunnelStore,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", content = "args", rename_all = "snake_case")]
pub enum Command {
    AppVersion {},
    SessionList {},
    SessionAdd {
        session: Box<Session>,
    },
    SessionRemove {
        id: SessionId,
    },
    SessionImportMobaxterm {
        path: String,
    },
    /// "Import MobaXterm settings": the global settings and trusted SSH host
    /// keys of a MobaXterm.ini (never its saved passwords).
    ConfigImportMobaxterm {
        path: String,
        /// Apply its global settings.
        #[serde(default = "yes")]
        settings: bool,
        /// Add its SSH host keys to the known hosts.
        #[serde(default = "yes")]
        host_keys: bool,
    },
    /// Settings › "Export configuration": the settings as a TOML file (no
    /// passwords: they stay in the password store).
    ConfigExport {
        path: String,
        /// Replace an existing file.
        #[serde(default)]
        overwrite: bool,
    },
    /// Settings › "Import configuration": settings from a file written by
    /// `config_export`.
    ConfigImport {
        path: String,
    },
    /// Settings › "Reset configuration": every setting back to its default.
    ConfigReset {},
    /// "Import sessions from a CSV file" (columns name, type, host, port,
    /// username, folder; see `tc_import::csv`).
    SessionImportCsv {
        path: String,
    },
    /// "Import PuTTY sessions" (registry on Windows, ~/.putty on Unix).
    SessionImportPutty {},
    /// "Import WSL sessions": one session per installed distribution.
    SessionImportWsl {},
    /// OpenSSH client config; `path` defaults to ~/.ssh/config.
    SessionImportSshConfig {
        #[serde(default)]
        path: Option<String>,
    },
    /// "Export MobaXterm sessions" / "Save session to file".
    SessionExportMobaxterm {
        path: String,
        /// Session names or ids; omitted = all sessions.
        #[serde(default)]
        sessions: Option<Vec<String>>,
        /// Replace an existing file.
        #[serde(default)]
        overwrite: bool,
    },
    /// "Create a desktop shortcut": a launcher that opens the session.
    SessionDesktopShortcut {
        /// Session name or id.
        session: String,
        /// "Hide terminal on startup" (start minimized).
        #[serde(default)]
        hide_terminal: bool,
        /// "Close … on exit": quit when the session ends.
        #[serde(default)]
        close_on_exit: bool,
        /// Where to create it (default: the desktop).
        #[serde(default)]
        folder: Option<String>,
    },
    /// Make `termcraft://` links open TermCraft (current user).
    UrlHandlerRegister {},
    /// "Generate HTML web page": a static page listing the sessions.
    SessionExportHtml {
        path: String,
        #[serde(default)]
        overwrite: bool,
    },
    SessionRename {
        id: SessionId,
        name: String,
    },
    SessionDuplicate {
        id: SessionId,
    },
    /// "New folder" in the sessions tree (`a/b` = nested).
    SessionFolderAdd {
        path: String,
    },
    /// Rename / move a folder; its sessions and sub-folders follow.
    SessionFolderRename {
        from: String,
        to: String,
    },
    /// Delete a folder with its sessions and sub-folders.
    SessionFolderRemove {
        path: String,
    },
    /// Replace a saved session (matched by `session.id`) with new settings.
    SessionUpdate {
        session: Box<Session>,
    },
    TabList {},
    /// Bring the window to the front (a second start shows the running
    /// window instead of opening another one).
    WindowShow {},
    /// A local terminal in this folder ("Open TermCraft here").
    TabOpenFolder {
        path: String,
    },
    /// General › "Windows right-click menu entries": add (or, with
    /// `remove`, take away) "Open TermCraft here" in Explorer.
    ExplorerMenu {
        #[serde(default)]
        remove: bool,
    },
    /// Tools › "Wake On Lan": send the magic packet that wakes the computer
    /// with this MAC address.
    WakeOnLan {
        mac: String,
        /// Broadcast (or unicast) IPv4 address; default 255.255.255.255.
        #[serde(default)]
        address: Option<String>,
        /// UDP port; default 9.
        #[serde(default)]
        port: Option<u16>,
    },
    /// "Start X server" (Configuration › X11 settings).
    XServerStart {},
    /// Stop the X server TermCraft started.
    XServerStop {},
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
    MacroList {},
    MacroAdd {
        #[serde(rename = "macro")]
        r#macro: Macro,
    },
    MacroRename {
        id: MacroId,
        name: String,
    },
    MacroRemove {
        id: MacroId,
    },
    /// Type a saved macro into a tab (the active tab if omitted).
    MacroRun {
        id: MacroId,
        #[serde(default)]
        tab: Option<u64>,
    },
    TunnelList {},
    TunnelAdd {
        tunnel: Tunnel,
    },
    TunnelUpdate {
        tunnel: Tunnel,
    },
    TunnelRemove {
        id: TunnelId,
    },
    /// Network services of the "Servers" window (`http`, `tftp`).
    ServerList {},
    /// Start a server with its saved settings (GUI only).
    ServerStart {
        server: String,
    },
    ServerStop {
        server: String,
    },
    TunnelStart {
        id: TunnelId,
    },
    TunnelStop {
        id: TunnelId,
    },
    PasswordList {},
    PasswordForget {
        target: tc_vault::Target,
    },
    PasswordForgetAll {},
    /// Trusted SSH host keys (known_hosts).
    KnownHostList {},
    /// Forget a host key: `line` and `fingerprint` from `known_host_list`.
    KnownHostRemove {
        line: usize,
        fingerprint: String,
    },
    CredentialList {},
    /// Create or replace a named credential ("Credentials" tab).
    CredentialSet {
        name: String,
        username: String,
        #[serde(default)]
        password: String,
    },
    CredentialRemove {
        name: String,
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
            Self::ConfigImportMobaxterm { .. } => "config_import_mobaxterm",
            Self::ConfigExport { .. } => "config_export",
            Self::ConfigImport { .. } => "config_import",
            Self::ConfigReset {} => "config_reset",
            Self::SessionImportCsv { .. } => "session_import_csv",
            Self::SessionImportPutty {} => "session_import_putty",
            Self::SessionImportWsl {} => "session_import_wsl",
            Self::SessionImportSshConfig { .. } => "session_import_ssh_config",
            Self::SessionExportMobaxterm { .. } => "session_export_mobaxterm",
            Self::SessionDesktopShortcut { .. } => "session_desktop_shortcut",
            Self::SessionExportHtml { .. } => "session_export_html",
            Self::UrlHandlerRegister {} => "url_handler_register",
            Self::SessionRename { .. } => "session_rename",
            Self::SessionDuplicate { .. } => "session_duplicate",
            Self::SessionFolderAdd { .. } => "session_folder_add",
            Self::SessionFolderRename { .. } => "session_folder_rename",
            Self::SessionFolderRemove { .. } => "session_folder_remove",
            Self::SessionUpdate { .. } => "session_update",
            Self::TabList {} => "tab_list",
            Self::WindowShow {} => "window_show",
            Self::WakeOnLan { .. } => "wake_on_lan",
            Self::TabOpenFolder { .. } => "tab_open_folder",
            Self::ExplorerMenu { .. } => "explorer_menu",
            Self::XServerStart {} => "x_server_start",
            Self::XServerStop {} => "x_server_stop",
            Self::TabOpen { .. } => "tab_open",
            Self::TabSend { .. } => "tab_send",
            Self::TabQuickConnect { .. } => "tab_quick_connect",
            Self::TabClose { .. } => "tab_close",
            Self::TabDuplicate { .. } => "tab_duplicate",
            Self::TabRename { .. } => "tab_rename",
            Self::MacroList {} => "macro_list",
            Self::MacroAdd { .. } => "macro_add",
            Self::MacroRename { .. } => "macro_rename",
            Self::MacroRemove { .. } => "macro_remove",
            Self::MacroRun { .. } => "macro_run",
            Self::TunnelList {} => "tunnel_list",
            Self::TunnelAdd { .. } => "tunnel_add",
            Self::TunnelUpdate { .. } => "tunnel_update",
            Self::TunnelRemove { .. } => "tunnel_remove",
            Self::ServerList {} => "server_list",
            Self::ServerStart { .. } => "server_start",
            Self::ServerStop { .. } => "server_stop",
            Self::TunnelStart { .. } => "tunnel_start",
            Self::TunnelStop { .. } => "tunnel_stop",
            Self::PasswordList {} => "password_list",
            Self::PasswordForget { .. } => "password_forget",
            Self::PasswordForgetAll {} => "password_forget_all",
            Self::KnownHostList {} => "known_host_list",
            Self::KnownHostRemove { .. } => "known_host_remove",
            Self::CredentialList {} => "credential_list",
            Self::CredentialSet { .. } => "credential_set",
            Self::CredentialRemove { .. } => "credential_remove",
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

/// A network service of the "Servers" window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub server: String,
    pub running: bool,
    /// Listening address while running.
    pub address: Option<String>,
}

/// Servers TermCraft can run.
pub const SERVERS: [&str; 2] = ["tftp", "http"];

fn check_server(name: &str) -> Result<(), CommandError> {
    if SERVERS.contains(&name) {
        Ok(())
    } else {
        Err(CommandError::InvalidArgs(format!(
            "unknown server {name:?} (known: {})",
            SERVERS.join(", ")
        )))
    }
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

    fn window_show(&mut self) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("the window is a GUI feature"))
    }
    /// Start the X server; returns its display (`:0`) and program.
    fn x_server_start(&mut self) -> Result<Value, CommandError> {
        Err(CommandError::Unsupported("the X server is a GUI feature"))
    }
    fn x_server_stop(&mut self) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("the X server is a GUI feature"))
    }
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
    fn macros(&mut self) -> Result<&mut MacroStore, CommandError> {
        Err(CommandError::Unsupported("no macro store"))
    }
    fn persist_macros(&mut self) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("no macro store"))
    }
    fn tunnels(&mut self) -> Result<&mut TunnelStore, CommandError> {
        Err(CommandError::Unsupported("no tunnel store"))
    }
    fn persist_tunnels(&mut self) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("no tunnel store"))
    }
    /// Human-readable state of a running tunnel ("Listening on …").
    fn tunnel_status(&self, _id: TunnelId) -> Option<String> {
        None
    }
    /// Running state of the network services.
    fn server_list(&self) -> Result<Vec<ServerInfo>, CommandError> {
        Err(CommandError::Unsupported("servers run in the GUI"))
    }
    fn server_start(&mut self, _server: &str) -> Result<String, CommandError> {
        Err(CommandError::Unsupported("servers run in the GUI"))
    }
    fn server_stop(&mut self, _server: &str) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("servers run in the GUI"))
    }
    fn tunnel_start(&mut self, _tunnel: Tunnel) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("tunnels run in the GUI"))
    }
    fn tunnel_stop(&mut self, _id: TunnelId) -> Result<(), CommandError> {
        Err(CommandError::Unsupported("tunnels run in the GUI"))
    }
    /// The known_hosts file the SSH client trusts.
    /// The app settings (to change them; then [`Host::persist_settings`]).
    fn settings(&mut self) -> Result<&mut tc_core::Settings, CommandError> {
        Err(CommandError::Unsupported("settings are not available here"))
    }
    fn persist_settings(&mut self) -> Result<(), CommandError> {
        Ok(())
    }
    fn known_hosts_file(&self) -> Result<std::path::PathBuf, CommandError> {
        Err(CommandError::Unsupported("no known_hosts file"))
    }
    /// Saved-password store; secrets are never returned through commands.
    fn vault(&self) -> Result<&tc_vault::Vault, CommandError> {
        Err(CommandError::Unsupported("no credential store"))
    }
    fn update_check(&mut self) -> Result<Value, CommandError> {
        Err(CommandError::Unsupported("updater not wired"))
    }
}

fn yes() -> bool {
    true
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
            let id = host.store().add(*session);
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
            let file = std::path::Path::new(&path);
            let text = tc_import::read_text_lossy(file)
                .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?;
            let mut report = tc_import::mobaxterm::parse(&text);
            // `_MobaFolder_\key.ppk`: the folder of the imported file.
            let folder = std::path::absolute(file)
                .ok()
                .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
                .unwrap_or_default();
            tc_import::mobaxterm::resolve_paths(&mut report, &folder);
            let missing = missing_files(&report.sessions);
            let mut out = merge_import(host, report)?;
            out["missing_files"] = json!(missing);
            Ok(out)
        }
        Command::ConfigImportMobaxterm {
            path,
            settings,
            host_keys,
        } => {
            use tc_import::mobaxterm_ini;
            let file = std::path::Path::new(&path);
            let size = std::fs::metadata(file)
                .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?
                .len();
            if size > 16 * 1024 * 1024 {
                return Err(CommandError::InvalidArgs(format!("{path} is too large")));
            }
            let text = tc_import::read_text_lossy(file)
                .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?;
            let ini = mobaxterm_ini::parse(&text);
            if !ini.contains_key("Misc") {
                return Err(CommandError::InvalidArgs(format!(
                    "{path} is not a MobaXterm.ini (no [Misc] section)"
                )));
            }
            let report = if settings {
                let report = mobaxterm_ini::apply_settings(&ini, host.settings()?);
                host.persist_settings()?;
                report
            } else {
                Default::default()
            };
            let keys = if host_keys {
                mobaxterm_ini::host_keys(&ini)
            } else {
                Default::default()
            };
            let added = if keys.lines.is_empty() {
                0
            } else {
                tc_core::known_hosts::add(&host.known_hosts_file()?, &keys.lines)
                    .map_err(|e| CommandError::Failed(format!("known hosts: {e}")))?
            };
            let not_applied: Vec<Value> = report
                .not_applied
                .iter()
                .map(|(key, why)| json!({ "setting": key, "why": why }))
                .collect();
            Ok(json!({
                "settings_applied": report.applied,
                "settings_not_applied": not_applied,
                "host_keys_added": added,
                "host_keys_found": keys.lines.len(),
                "host_keys_verified": keys.verified,
                "host_keys_skipped": keys.skipped,
            }))
        }
        Command::ConfigExport { path, overwrite } => {
            let file = std::path::Path::new(&path);
            if file.exists() && !overwrite {
                return Err(CommandError::InvalidArgs(format!(
                    "{path} exists (pass overwrite to replace it)"
                )));
            }
            host.settings()?
                .save(file)
                .map_err(|e| CommandError::Failed(e.to_string()))?;
            Ok(json!({ "path": path }))
        }
        Command::ConfigImport { path } => {
            let file = std::path::Path::new(&path);
            let size = std::fs::metadata(file)
                .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?
                .len();
            if size > 4 * 1024 * 1024 {
                return Err(CommandError::InvalidArgs(format!("{path} is too large")));
            }
            let imported = tc_core::Settings::load(file)
                .map_err(|e| CommandError::InvalidArgs(e.to_string()))?;
            *host.settings()? = imported;
            host.persist_settings()?;
            Ok(json!({ "ok": true }))
        }
        Command::ConfigReset {} => {
            let settings = host.settings()?;
            // The first-start offer to import a MobaXterm.ini is not made again.
            let offered = settings.general.mobaxterm_import_offered;
            *settings = tc_core::Settings::default();
            settings.general.mobaxterm_import_offered = offered;
            host.persist_settings()?;
            Ok(json!({ "ok": true }))
        }
        Command::SessionFolderAdd { path } => {
            let path = host
                .store()
                .add_folder(&path)
                .ok_or_else(|| CommandError::InvalidArgs("empty folder name".into()))?;
            host.persist()?;
            Ok(json!({ "folder": path }))
        }
        Command::SessionFolderRename { from, to } => {
            if !host
                .store()
                .all_folders()
                .contains(&tc_core::store::normalize_folder(&from))
            {
                return Err(CommandError::NotFound(format!("folder {from:?}")));
            }
            if !host.store().rename_folder(&from, &to) {
                return Err(CommandError::InvalidArgs(format!(
                    "cannot rename {from:?} to {to:?}"
                )));
            }
            host.persist()?;
            Ok(json!({ "ok": true }))
        }
        Command::SessionFolderRemove { path } => {
            let removed = host.store().remove_folder(&path);
            host.persist()?;
            Ok(json!({ "removed_sessions": removed }))
        }
        Command::SessionImportPutty {} => {
            let report = tc_import::putty::load().map_err(CommandError::Failed)?;
            merge_import(host, report)
        }
        Command::SessionImportWsl {} => {
            let distros = tc_import::wsl::distributions();
            if distros.is_empty() {
                return Err(CommandError::Failed(
                    "no WSL distribution is installed".into(),
                ));
            }
            merge_import(host, tc_import::wsl::sessions(distros))
        }
        Command::SessionImportCsv { path } => {
            let file = std::path::Path::new(&path);
            let size = std::fs::metadata(file)
                .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?
                .len();
            if size > 16 * 1024 * 1024 {
                return Err(CommandError::InvalidArgs(format!("{path} is too large")));
            }
            let text = tc_import::read_text_lossy(file)
                .map_err(|e| CommandError::Failed(format!("{path}: {e}")))?;
            let report = tc_import::csv::parse(&text).map_err(CommandError::InvalidArgs)?;
            merge_import(host, report)
        }
        Command::SessionImportSshConfig { path } => {
            let path = match path {
                Some(p) => std::path::PathBuf::from(p),
                None => tc_import::ssh_config::default_path()
                    .ok_or_else(|| CommandError::Failed("no home folder".into()))?,
            };
            let text = tc_import::read_text_lossy(&path)
                .map_err(|e| CommandError::Failed(format!("{}: {e}", path.display())))?;
            merge_import(host, tc_import::ssh_config::parse(&text))
        }
        Command::SessionExportMobaxterm {
            path,
            sessions,
            overwrite,
        } => {
            let store = host.store();
            let chosen: Vec<Session> = match &sessions {
                None => store.sessions.clone(),
                Some(keys) => keys
                    .iter()
                    .map(|k| resolve_session(store, k))
                    .collect::<Result<_, _>>()?,
            };
            let path = std::path::Path::new(&path);
            if path.exists() && !overwrite {
                return Err(CommandError::InvalidArgs(format!(
                    "{} exists (pass overwrite to replace it)",
                    path.display()
                )));
            }
            std::fs::write(path, tc_import::mobaxterm::export(&chosen))
                .map_err(|e| CommandError::Failed(format!("{}: {e}", path.display())))?;
            Ok(json!({ "exported": chosen.len() }))
        }
        Command::SessionDesktopShortcut {
            session,
            hide_terminal,
            close_on_exit,
            folder,
        } => {
            let session = resolve_session(host.store(), &session)?;
            let dir = match folder {
                Some(f) => std::path::PathBuf::from(f),
                None => tc_core::shortcut::desktop_dir()
                    .ok_or_else(|| CommandError::Failed("no desktop folder".into()))?,
            };
            if !dir.is_dir() {
                return Err(CommandError::InvalidArgs(format!(
                    "{} is not a folder",
                    dir.display()
                )));
            }
            let exe = tc_core::shortcut::gui_exe().ok_or_else(|| {
                CommandError::Failed("the TermCraft program was not found".into())
            })?;
            let opts = tc_core::shortcut::ShortcutOptions {
                hide_terminal,
                close_on_exit,
            };
            let path = tc_core::shortcut::create(&dir, &exe, &session.name, opts)
                .map_err(|e| CommandError::Failed(format!("{}: {e}", dir.display())))?;
            Ok(json!({ "path": path.display().to_string() }))
        }
        Command::UrlHandlerRegister {} => {
            let exe = tc_core::shortcut::gui_exe().ok_or_else(|| {
                CommandError::Failed("the TermCraft program was not found".into())
            })?;
            let done = tc_core::url::register(&exe).map_err(CommandError::Failed)?;
            Ok(json!({ "done": done }))
        }
        Command::SessionExportHtml { path, overwrite } => {
            let path = std::path::Path::new(&path);
            if path.exists() && !overwrite {
                return Err(CommandError::InvalidArgs(format!(
                    "{} exists (pass overwrite to replace it)",
                    path.display()
                )));
            }
            let sessions = host.store().sessions.clone();
            std::fs::write(path, tc_import::mobaxterm::html_page(&sessions))
                .map_err(|e| CommandError::Failed(format!("{}: {e}", path.display())))?;
            Ok(json!({ "exported": sessions.len() }))
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
        Command::SessionUpdate { session } => {
            let name = session.name.trim();
            if name.is_empty() {
                return Err(CommandError::InvalidArgs("name must not be empty".into()));
            }
            if host
                .store()
                .sessions
                .iter()
                .any(|s| s.name == name && s.id != session.id)
            {
                return Err(CommandError::InvalidArgs(format!(
                    "a session named {name:?} already exists"
                )));
            }
            let slot = session_mut(host.store(), session.id)?;
            *slot = Session {
                name: name.to_string(),
                last_used: slot.last_used,
                ..*session
            };
            host.persist()?;
            Ok(json!({ "ok": true }))
        }
        Command::WindowShow {} => {
            host.window_show()?;
            Ok(json!({ "ok": true }))
        }
        Command::TabOpenFolder { path } => {
            // Explorer quotes a drive root as "C:\" and the backslash
            // escapes the quote: C:" arrives.
            let mut folder = path.trim().trim_end_matches('"').to_string();
            if folder.ends_with(':') {
                folder.push('\\');
            }
            let dir = std::path::Path::new(&folder);
            if !dir.is_dir() {
                return Err(CommandError::InvalidArgs(format!("{path} is not a folder")));
            }
            let name = dir
                .file_name()
                .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
            let mut session = Session::new(
                name,
                tc_core::session::SessionKind::LocalShell { program: None },
            );
            // Shell 3_3 "Startup directory".
            session
                .options
                .insert("3_3".into(), dir.display().to_string());
            Ok(json!({ "tab": host.tab_open(Some(session))? }))
        }
        Command::ExplorerMenu { remove } => {
            let done = if remove {
                tc_core::explorer::unregister()
            } else {
                let exe = tc_core::shortcut::gui_exe().ok_or_else(|| {
                    CommandError::Failed("the TermCraft program was not found".into())
                })?;
                tc_core::explorer::register(&exe)
            }
            .map_err(CommandError::Failed)?;
            Ok(json!({ "done": done }))
        }
        Command::WakeOnLan { mac, address, port } => {
            let bytes = tc_core::wol::parse_mac(&mac)
                .ok_or_else(|| CommandError::InvalidArgs(format!("invalid MAC address {mac:?}")))?;
            let address = match address.as_deref().map(str::trim) {
                None | Some("") => std::net::Ipv4Addr::BROADCAST,
                Some(a) => a.parse().map_err(|_| {
                    CommandError::InvalidArgs(format!("invalid IPv4 address {a:?}"))
                })?,
            };
            let port = port.unwrap_or(tc_core::wol::DEFAULT_PORT);
            tc_core::wol::wake(bytes, address, port)
                .map_err(|e| CommandError::Failed(format!("wake on LAN: {e}")))?;
            Ok(json!({ "sent_to": format!("{address}:{port}") }))
        }
        Command::XServerStart {} => host.x_server_start(),
        Command::XServerStop {} => {
            host.x_server_stop()?;
            Ok(json!({ "ok": true }))
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
        Command::MacroList {} => to_json(&host.macros()?.macros),
        Command::MacroAdd { r#macro } => {
            if r#macro.name.trim().is_empty() {
                return Err(CommandError::InvalidArgs("name must not be empty".into()));
            }
            let id = r#macro.id;
            host.macros()?.macros.push(r#macro);
            host.persist_macros()?;
            Ok(json!({ "id": id }))
        }
        Command::MacroRename { id, name } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(CommandError::InvalidArgs("name must not be empty".into()));
            }
            let m = host
                .macros()?
                .macros
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
            m.name = name;
            host.persist_macros()?;
            Ok(json!({ "ok": true }))
        }
        Command::MacroRemove { id } => {
            let store = host.macros()?;
            let before = store.macros.len();
            store.macros.retain(|m| m.id != id);
            if store.macros.len() == before {
                return Err(CommandError::NotFound(id.to_string()));
            }
            host.persist_macros()?;
            Ok(json!({ "ok": true }))
        }
        Command::MacroRun { id, tab } => {
            if host.settings().is_ok_and(|s| s.security.disable_macros) {
                return Err(CommandError::Failed(
                    "macros are disabled (Settings › Configuration › Security)".into(),
                ));
            }
            let keys = host
                .macros()?
                .get(id)
                .map(|m| m.keys.clone())
                .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
            host.tab_send(tab, &keys)?;
            Ok(json!({ "ok": true }))
        }
        Command::TunnelList {} => {
            let tunnels = host.tunnels()?.tunnels.clone();
            let rows: Vec<Value> = tunnels
                .iter()
                .map(|t| {
                    let mut v = serde_json::to_value(t).unwrap_or_default();
                    v["status"] = json!(host.tunnel_status(t.id));
                    v
                })
                .collect();
            Ok(Value::Array(rows))
        }
        Command::TunnelAdd { tunnel } => {
            tunnel.validate().map_err(CommandError::InvalidArgs)?;
            let id = tunnel.id;
            host.tunnels()?.tunnels.push(tunnel);
            host.persist_tunnels()?;
            Ok(json!({ "id": id }))
        }
        Command::TunnelUpdate { tunnel } => {
            tunnel.validate().map_err(CommandError::InvalidArgs)?;
            let store = host.tunnels()?;
            let slot = store
                .tunnels
                .iter_mut()
                .find(|t| t.id == tunnel.id)
                .ok_or_else(|| CommandError::NotFound(tunnel.id.to_string()))?;
            *slot = tunnel;
            host.persist_tunnels()?;
            Ok(json!({ "ok": true }))
        }
        Command::TunnelRemove { id } => {
            let _ = host.tunnel_stop(id);
            let store = host.tunnels()?;
            let before = store.tunnels.len();
            store.tunnels.retain(|t| t.id != id);
            if store.tunnels.len() == before {
                return Err(CommandError::NotFound(id.to_string()));
            }
            host.persist_tunnels()?;
            Ok(json!({ "ok": true }))
        }
        Command::ServerList {} => to_json(&host.server_list()?),
        Command::ServerStart { server } => {
            check_server(&server)?;
            let address = host.server_start(&server)?;
            Ok(json!({ "server": server, "address": address }))
        }
        Command::ServerStop { server } => {
            check_server(&server)?;
            host.server_stop(&server)?;
            Ok(json!({ "stopped": server }))
        }
        Command::TunnelStart { id } => {
            let tunnel = host
                .tunnels()?
                .get(id)
                .cloned()
                .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
            tunnel.validate().map_err(CommandError::InvalidArgs)?;
            host.tunnel_start(tunnel)?;
            Ok(json!({ "ok": true }))
        }
        Command::TunnelStop { id } => {
            host.tunnel_stop(id)?;
            Ok(json!({ "ok": true }))
        }
        Command::PasswordList {} => to_json(&host.vault()?.list()),
        Command::PasswordForget { target } => {
            host.vault()?
                .forget(&target)
                .map_err(|e| CommandError::Failed(e.to_string()))?;
            Ok(json!({ "ok": true }))
        }
        Command::PasswordForgetAll {} => {
            host.vault()?
                .forget_all()
                .map_err(|e| CommandError::Failed(e.to_string()))?;
            Ok(json!({ "ok": true }))
        }
        Command::KnownHostList {} => {
            let path = host.known_hosts_file()?;
            let hosts = tc_core::known_hosts::load(&path)
                .map_err(|e| CommandError::Failed(format!("{}: {e}", path.display())))?;
            to_json(&hosts)
        }
        Command::KnownHostRemove { line, fingerprint } => {
            let path = host.known_hosts_file()?;
            let removed = tc_core::known_hosts::remove(&path, line, &fingerprint)
                .map_err(|e| CommandError::Failed(format!("{}: {e}", path.display())))?;
            if !removed {
                return Err(CommandError::NotFound(format!(
                    "line {line} with {fingerprint}"
                )));
            }
            Ok(json!({ "ok": true }))
        }
        Command::CredentialList {} => {
            let list: Vec<_> = host
                .vault()?
                .credentials()
                .into_iter()
                .map(|(name, username)| json!({ "name": name, "username": username }))
                .collect();
            Ok(json!(list))
        }
        Command::CredentialSet {
            name,
            username,
            password,
        } => {
            host.vault()?
                .save_credential(&name, &username, &password)
                .map_err(|e| CommandError::Failed(e.to_string()))?;
            Ok(json!({ "ok": true }))
        }
        Command::CredentialRemove { name } => {
            host.vault()?
                .forget_credential(&name)
                .map_err(|e| CommandError::Failed(e.to_string()))?;
            Ok(json!({ "ok": true }))
        }
        Command::UpdateCheck {} => host.update_check(),
    }
}

/// Add imported sessions, skipping names that already exist.
/// Sessions whose file settings (private keys …) name a file that does
/// not exist here: `{session, file}`.
fn missing_files(sessions: &[Session]) -> Vec<Value> {
    let mut out = Vec::new();
    for s in sessions {
        let t = s.kind.type_index();
        let files = tc_core::schema::type_params(t)
            .iter()
            .chain(tc_core::schema::network_params(t))
            .filter(|p| p.widget == tc_core::schema::Widget::File);
        for p in files {
            let value = s
                .kind
                .typed_get(p.id)
                .or_else(|| s.options.get(p.id).cloned())
                .unwrap_or_default();
            // Gateway keys hold one path per gateway.
            for path in value.split(tc_core::jump_hosts::SEPARATOR) {
                let path = path.trim();
                if !path.is_empty() && !std::path::Path::new(path).is_file() {
                    out.push(json!({ "session": s.name, "file": path }));
                }
            }
        }
    }
    out
}

fn merge_import(
    host: &mut dyn Host,
    report: tc_import::mobaxterm::ImportReport,
) -> Result<Value, CommandError> {
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
        | K::Ftp { host, .. }
        | K::Rsh { host, .. }
        | K::Mosh { host, .. }
        | K::Xdmcp { host } => Some(host),
        K::LocalShell { .. }
        | K::Serial { .. }
        | K::File { .. }
        | K::Browser { .. }
        | K::Wsl { .. }
        | K::AwsS3 { .. } => None,
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
    fn update_replaces_and_keeps_last_used() {
        let mut host = MemHost::default();
        let args = json!({ "session": { "name": "a", "type": "ssh", "host": "h1" } });
        let out = execute(&mut host, Command::from_call("session_add", args).unwrap()).unwrap();
        let id: SessionId = serde_json::from_value(out["id"].clone()).unwrap();
        host.store.sessions[0].last_used = Some(42);

        let mut edited = host.store.sessions[0].clone();
        edited.name = " b ".into();
        edited.last_used = None;
        edited.options.insert("3_7".into(), "-1".into());
        execute(
            &mut host,
            Command::SessionUpdate {
                session: Box::new(edited),
            },
        )
        .unwrap();
        let s = host.store.get(id).unwrap();
        assert_eq!(s.name, "b");
        assert_eq!(s.last_used, Some(42));
        assert_eq!(s.options["3_7"], "-1");

        let mut ghost = s.clone();
        ghost.id = SessionId::new();
        assert!(
            execute(
                &mut host,
                Command::SessionUpdate {
                    session: Box::new(ghost)
                }
            )
            .is_err()
        );
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
    fn password_commands_never_expose_secrets() {
        struct VaultHost(SessionStore, tc_vault::Vault);
        impl Host for VaultHost {
            fn store(&mut self) -> &mut SessionStore {
                &mut self.0
            }
            fn persist(&mut self) -> Result<(), CommandError> {
                Ok(())
            }
            fn vault(&self) -> Result<&tc_vault::Vault, CommandError> {
                Ok(&self.1)
            }
        }
        let dir = std::env::temp_dir().join(format!("tc-vault-test-{}", std::process::id()));
        let vault = tc_vault::Vault::open(
            dir.join("i.toml"),
            Box::new(tc_vault::MemoryStore::default()),
        );
        let t = tc_vault::Target::ssh("root", "h", 22);
        vault.save(&t, "hunter2").unwrap();
        let mut host = VaultHost(SessionStore::default(), vault);

        let list = execute(&mut host, Command::PasswordList {}).unwrap();
        assert!(!list.to_string().contains("hunter2"));
        assert_eq!(list[0]["server"], "h:22");
        execute(&mut host, Command::PasswordForget { target: t }).unwrap();
        assert!(host.1.list().is_empty());

        execute(
            &mut host,
            Command::CredentialSet {
                name: "lab".into(),
                username: "admin".into(),
                password: "cred-secret".into(),
            },
        )
        .unwrap();
        let list = execute(&mut host, Command::CredentialList {}).unwrap();
        assert!(!list.to_string().contains("cred-secret"));
        assert_eq!(list[0]["username"], "admin");
        execute(&mut host, Command::CredentialRemove { name: "lab".into() }).unwrap();
        assert!(host.1.credentials().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn macros_add_rename_run_remove() {
        #[derive(Default)]
        struct MacroHost {
            store: SessionStore,
            macros: MacroStore,
            typed: Vec<(Option<u64>, String)>,
        }
        impl Host for MacroHost {
            fn store(&mut self) -> &mut SessionStore {
                &mut self.store
            }
            fn persist(&mut self) -> Result<(), CommandError> {
                Ok(())
            }
            fn macros(&mut self) -> Result<&mut MacroStore, CommandError> {
                Ok(&mut self.macros)
            }
            fn persist_macros(&mut self) -> Result<(), CommandError> {
                Ok(())
            }
            fn tab_send(&mut self, tab: Option<u64>, text: &str) -> Result<(), CommandError> {
                self.typed.push((tab, text.to_string()));
                Ok(())
            }
        }
        let mut host = MacroHost::default();
        let args = json!({ "macro": { "name": "up", "keys": "uptime\r" } });
        let out = execute(&mut host, Command::from_call("macro_add", args).unwrap()).unwrap();
        let id: MacroId = serde_json::from_value(out["id"].clone()).unwrap();
        execute(
            &mut host,
            Command::MacroRename {
                id,
                name: "load".into(),
            },
        )
        .unwrap();
        execute(&mut host, Command::MacroRun { id, tab: Some(3) }).unwrap();
        assert_eq!(host.typed, vec![(Some(3), "uptime\r".to_string())]);
        assert_eq!(host.macros.macros[0].name, "load");
        execute(&mut host, Command::MacroRemove { id }).unwrap();
        assert!(host.macros.macros.is_empty());
    }

    #[test]
    fn headless_tabs_are_unsupported() {
        let mut host = MemHost::default();
        let err = execute(&mut host, Command::TabList {}).unwrap_err();
        assert!(matches!(err, CommandError::Unsupported(_)));
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    struct SettingsHost {
        store: SessionStore,
        settings: tc_core::Settings,
    }

    impl Host for SettingsHost {
        fn store(&mut self) -> &mut SessionStore {
            &mut self.store
        }
        fn persist(&mut self) -> Result<(), CommandError> {
            Ok(())
        }
        fn settings(&mut self) -> Result<&mut tc_core::Settings, CommandError> {
            Ok(&mut self.settings)
        }
    }

    #[test]
    fn export_reset_import() {
        let dir = std::env::temp_dir().join(format!("tc-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.toml");
        let path = file.display().to_string();
        let mut host = SettingsHost {
            store: SessionStore::default(),
            settings: tc_core::Settings::default(),
        };
        host.settings.ssh.default_login = "me".into();
        host.settings.general.mobaxterm_import_offered = true;
        let export = |overwrite| Command::ConfigExport {
            path: path.clone(),
            overwrite,
        };
        execute(&mut host, export(false)).unwrap();
        assert!(
            execute(&mut host, export(false)).is_err(),
            "no silent overwrite"
        );

        execute(&mut host, Command::ConfigReset {}).unwrap();
        assert_eq!(host.settings.ssh.default_login, "");
        assert!(
            host.settings.general.mobaxterm_import_offered,
            "not offered again"
        );

        execute(&mut host, Command::ConfigImport { path: path.clone() }).unwrap();
        assert_eq!(host.settings.ssh.default_login, "me");

        std::fs::write(&file, "not = [toml").unwrap();
        assert!(execute(&mut host, Command::ConfigImport { path: path.clone() }).is_err());
        let missing = dir.join("missing.toml").display().to_string();
        assert!(execute(&mut host, Command::ConfigImport { path: missing }).is_err());
        assert_eq!(
            host.settings.ssh.default_login, "me",
            "kept after a failure"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
