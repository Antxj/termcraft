use crate::tab::{Tab, TabEnv};
use crate::term_view;
use crate::tunnels::RunningTunnel;
use crate::ui::session_editor::SessionEditor;
use crate::ui::{self, Action, home, menus, sidebar, tabbar, toolbar};
use crate::updates::{UpdateState, Updates};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use tc_commands::{Command, CommandError, Host, TabInfo, execute, recent_sessions};
use tc_core::settings::{Theme, UpdateChannel};
use tc_core::{Paths, Session, SessionId, SessionStore, Settings, Tunnel, TunnelId, TunnelStore};

/// Everything the command registry can act on. Kept separate from widget
/// state so `execute(&mut core, cmd)` borrows cleanly.
struct Core {
    paths: Paths,
    store: SessionStore,
    settings: Settings,
    tabs: Vec<Tab>,
    active: Option<u64>,
    next_tab_id: u64,
    repaint: Arc<dyn Fn() + Send + Sync>,
    vault: Arc<tc_vault::Vault>,
    macros: tc_core::MacroStore,
    tunnels: TunnelStore,
    running: BTreeMap<TunnelId, RunningTunnel>,
}

impl Core {
    /// What a new tab gets from the settings.
    fn tab_env(&self) -> TabEnv {
        TabEnv {
            scrollback: self.settings.terminal.scrollback_lines,
            known_hosts: self.paths.known_hosts_file(),
            repaint: self.repaint.clone(),
            vault: self.vault.clone(),
            save_passwords: self.settings.passwords.save,
            ssh: self.settings.ssh.clone(),
            backspace_ctrl_h: self.settings.terminal.backspace_ctrl_h,
            paste_right_click: self.settings.terminal.paste_right_click,
            local_shell: self.settings.terminal.local_shell,
            save_key_passphrases: self.settings.passwords.save_key_passphrases,
            log_dir: Some(self.settings.terminal.log_dir.trim())
                .filter(|d| !d.is_empty())
                .map(std::path::PathBuf::from),
            use_windows_path: self.settings.terminal.use_windows_path,
        }
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let active = self.active?;
        self.tabs.iter_mut().find(|t| t.id == active)
    }

    fn tab_index(&self, tab: Option<u64>) -> Result<usize, CommandError> {
        let id = tab
            .or(self.active)
            .ok_or_else(|| CommandError::NotFound("no active tab".into()))?;
        self.tabs
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| CommandError::NotFound(format!("tab {id}")))
    }

    fn save_settings(&self) -> Result<(), String> {
        self.settings
            .save(&self.paths.settings_file())
            .map_err(|e| e.to_string())
    }
}

impl Host for Core {
    fn store(&mut self) -> &mut SessionStore {
        &mut self.store
    }

    fn persist(&mut self) -> Result<(), CommandError> {
        self.store
            .save(&self.paths.sessions_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn tab_list(&self) -> Result<Vec<TabInfo>, CommandError> {
        Ok(self
            .tabs
            .iter()
            .map(|t| TabInfo {
                id: t.id,
                title: t.title.clone(),
                active: Some(t.id) == self.active,
            })
            .collect())
    }

    fn tab_open(&mut self, session: Option<Session>) -> Result<u64, CommandError> {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let env = self.tab_env();
        self.tabs.push(Tab::open(id, session, &env));
        self.active = Some(id);
        Ok(id)
    }

    fn tab_send(&mut self, tab: Option<u64>, text: &str) -> Result<(), CommandError> {
        let i = self.tab_index(tab)?;
        self.tabs[i].send(text.as_bytes());
        Ok(())
    }

    fn tab_close(&mut self, tab: Option<u64>) -> Result<(), CommandError> {
        let i = self.tab_index(tab)?;
        let id = self.tabs.remove(i).id;
        if self.active == Some(id) {
            // Focus the neighbour on the left, else the one on the right.
            self.active = self
                .tabs
                .get(i.saturating_sub(1))
                .or_else(|| self.tabs.get(i))
                .map(|t| t.id);
        }
        Ok(())
    }

    fn tab_duplicate(&mut self, tab: Option<u64>) -> Result<u64, CommandError> {
        let i = self.tab_index(tab)?;
        let session = self.tabs[i].session.clone();
        self.tab_open(session)
    }

    fn vault(&self) -> Result<&tc_vault::Vault, CommandError> {
        Ok(&self.vault)
    }

    fn known_hosts_file(&self) -> Result<std::path::PathBuf, CommandError> {
        Ok(self.paths.known_hosts_file())
    }

    fn macros(&mut self) -> Result<&mut tc_core::MacroStore, CommandError> {
        Ok(&mut self.macros)
    }

    fn persist_macros(&mut self) -> Result<(), CommandError> {
        self.macros
            .save(&self.paths.macros_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn tunnels(&mut self) -> Result<&mut TunnelStore, CommandError> {
        Ok(&mut self.tunnels)
    }

    fn persist_tunnels(&mut self) -> Result<(), CommandError> {
        self.tunnels
            .save(&self.paths.tunnels_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn tunnel_status(&self, id: TunnelId) -> Option<String> {
        self.running.get(&id).map(|t| t.status.clone())
    }

    fn tunnel_start(&mut self, tunnel: Tunnel) -> Result<(), CommandError> {
        let running = RunningTunnel::start(
            &tunnel,
            self.paths.known_hosts_file(),
            self.vault.clone(),
            &self.settings.passwords,
            crate::tab::agent_config(&self.settings.ssh),
            self.repaint.clone(),
        );
        self.running.insert(tunnel.id, running);
        Ok(())
    }

    fn tunnel_stop(&mut self, id: TunnelId) -> Result<(), CommandError> {
        self.running
            .remove(&id)
            .map(drop)
            .ok_or_else(|| CommandError::NotFound(format!("tunnel {id} is not running")))
    }

    fn tab_rename(&mut self, tab: Option<u64>, title: &str) -> Result<(), CommandError> {
        let i = self.tab_index(tab)?;
        let title = title.trim();
        if title.is_empty() {
            return Err(CommandError::InvalidArgs("title must not be empty".into()));
        }
        self.tabs[i].title = title.to_string();
        Ok(())
    }
}

enum Rename {
    Tab(u64),
    Session(SessionId),
    Macro(tc_core::MacroId),
    /// Name for a macro that was just recorded (its keys).
    NewMacro(String),
    /// A new folder inside this one.
    NewFolder(String),
    Folder(String),
}

pub struct TermCraftApp {
    core: Core,
    updates: Updates,
    quick_connect: String,
    home_search: String,
    import_path: String,
    show_import: bool,
    show_about: bool,
    passwords: crate::ui::passwords_window::PasswordsUi,
    /// "Hotkeys edition" window.
    show_hotkeys: bool,
    keygen: crate::ui::keygen_window::KeygenWindow,
    known_hosts: crate::ui::known_hosts_window::KnownHostsWindow,
    /// Loopback control channel (other processes send commands).
    control: Option<crate::control_server::ControlServer>,
    /// Commands from the command line, run on the first frame.
    startup: Vec<Command>,
    show_config: bool,
    config_tab: crate::ui::config_window::ConfigTab,
    /// "Close prompt" waiting for an answer.
    confirm_exit: bool,
    exit_confirmed: bool,
    show_tunnels: bool,
    /// MultiExec: keystrokes go to every selected terminal.
    multiexec: MultiExec,
    /// Macro being recorded: tab id and what was typed so far.
    recording: Option<(u64, String)>,
    tunnel_editor: Option<crate::ui::tunnels_window::TunnelEditor>,
    /// "Do you want to save this password?" questions from tabs and tunnels.
    offers: VecDeque<(tc_vault::Target, String)>,
    dont_ask_again: bool,
    rename: Option<(Rename, String)>,
    confirm_delete: Option<SessionId>,
    /// "Delete folder" waiting for confirmation.
    confirm_delete_folder: Option<String>,
    editor: Option<SessionEditor>,
    snapshot: Option<crate::snapshot::Snapshot>,
    status: String,
}

impl TermCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Vec<Command>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);

        let paths = Paths::resolve();
        let mut status = String::new();
        let store = SessionStore::load(&paths.sessions_file()).unwrap_or_else(|e| {
            status = format!("could not load sessions: {e}");
            SessionStore::default()
        });
        let settings = Settings::load(&paths.settings_file()).unwrap_or_default();
        let macros = tc_core::MacroStore::load(&paths.macros_file()).unwrap_or_default();
        let tunnels = TunnelStore::load(&paths.tunnels_file()).unwrap_or_else(|e| {
            status = format!("could not load tunnels: {e}");
            TunnelStore::default()
        });
        let vault = Arc::new(tc_vault::Vault::open(
            paths.passwords_index_file(),
            Box::new(tc_vault::OsKeychain),
        ));
        crate::skin::apply(&cc.egui_ctx, settings.view.theme);
        let ctx = cc.egui_ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());

        let channel = match settings.update.channel {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
        };
        let mut updates = Updates::new(channel);
        if settings.update.check_on_startup {
            updates.check(&cc.egui_ctx);
        }

        let mut app = Self {
            core: Core {
                paths,
                store,
                settings,
                tabs: Vec::new(),
                active: None,
                next_tab_id: 1,
                repaint,
                vault,
                macros,
                tunnels,
                running: BTreeMap::new(),
            },
            updates,
            quick_connect: String::new(),
            home_search: String::new(),
            import_path: String::new(),
            show_import: false,
            show_about: false,
            passwords: Default::default(),
            show_hotkeys: false,
            keygen: Default::default(),
            known_hosts: Default::default(),
            control: None,
            startup,
            show_config: false,
            config_tab: Default::default(),
            confirm_exit: false,
            exit_confirmed: false,
            show_tunnels: false,
            multiexec: MultiExec::default(),
            recording: None,
            tunnel_editor: None,
            offers: VecDeque::new(),
            dont_ask_again: false,
            rename: None,
            confirm_delete: None,
            confirm_delete_folder: None,
            editor: None,
            snapshot: crate::snapshot::Snapshot::from_env(),
            status,
        };
        app.control = crate::control_server::ControlServer::start(
            app.core.paths.control_file(),
            app.core.repaint.clone(),
        );
        if app.core.settings.general.auto_backup && app.snapshot.is_none() {
            let p = &app.core.paths;
            let files = [
                p.sessions_file(),
                p.settings_file(),
                p.macros_file(),
                p.tunnels_file(),
            ];
            if let Err(e) = tc_core::backup::run(&p.config_dir, &files) {
                app.status = format!("configuration backup failed: {e}");
            }
        }
        app.unlock_vault_at_startup();
        app.autostart_tunnels();
        app
    }

    /// Configuration file storage: unlock from the key cached on this
    /// computer, or ask for the Master Password.
    fn unlock_vault_at_startup(&mut self) {
        use tc_core::settings::MasterPrompt;
        let vault = &self.core.vault;
        if !vault.is_locked() || self.snapshot.is_some() {
            return;
        }
        let cached = self.core.settings.passwords.master_prompt == MasterPrompt::NewComputer;
        if !(cached && vault.try_cached_unlock()) {
            self.passwords.ask_unlock();
        }
    }

    /// Start tunnels marked "Start this tunnel when TermCraft starts".
    fn autostart_tunnels(&mut self) {
        let ids: Vec<TunnelId> = self
            .core
            .tunnels
            .tunnels
            .iter()
            .filter(|t| t.autostart)
            .map(|t| t.id)
            .collect();
        for id in ids {
            self.run(Command::TunnelStart { id });
        }
    }

    fn run(&mut self, cmd: Command) {
        let name = cmd.name();
        match execute(&mut self.core, cmd) {
            Ok(value) if name == "session_import_mobaxterm" => {
                self.status = format!(
                    "Imported {} session(s), {} already existed, {} unsupported",
                    value["imported"],
                    value["skipped_existing"],
                    value["unsupported"].as_array().map_or(0, Vec::len)
                );
            }
            Ok(_) => {}
            Err(e) => self.status = format!("{name}: {e}"),
        }
    }

    /// Apply one UI action. View changes are persisted to settings.toml.
    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        let view = &mut self.core.settings.view;
        match action {
            Action::Run(cmd) => return self.run(cmd),
            Action::Quit => return self.request_exit(ctx),
            Action::Configuration => return self.show_config = true,
            Action::Minimize => {
                return ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            Action::ToggleFullscreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                return ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            Action::About => return self.show_about = true,
            Action::PasswordsSettings => return self.passwords.open = true,
            Action::ManageCredentials => return self.passwords.open_credentials(),
            Action::Tunnels => return self.show_tunnels = true,
            Action::CheckUpdates => return self.updates.check(ctx),
            Action::ImportDialog => return self.show_import = true,
            Action::ExportSessions(one) => return self.export_sessions(one),
            Action::SaveTerminalText => return self.save_terminal_text(),
            Action::PrintTerminalText => return self.print_terminal_text(),
            Action::FindInTerminal => {
                if let Some(tab) = self.core.active_tab_mut() {
                    let bar = tab.find.get_or_insert_with(Default::default);
                    bar.focus = true;
                }
                return;
            }
            Action::FindNext => {
                if let Some(tab) = self.core.active_tab_mut() {
                    find_next(tab);
                }
                return;
            }
            Action::NewSession => return self.editor = Some(SessionEditor::new_session()),
            Action::EditSession(id) => {
                self.editor = self.core.store.get(id).map(SessionEditor::edit);
                return;
            }
            Action::RenameTab(id) => {
                let current = self.core.tabs.iter().find(|t| t.id == id);
                self.rename = current.map(|t| (Rename::Tab(id), t.title.clone()));
                return;
            }
            Action::RenameSession(id) => {
                let current = self.core.store.get(id);
                self.rename = current.map(|s| (Rename::Session(id), s.name.clone()));
                return;
            }
            Action::DeleteSession(id) => return self.confirm_delete = Some(id),
            Action::NewFolder(parent) => {
                return self.rename = Some((Rename::NewFolder(parent), String::new()));
            }
            Action::RenameFolder(path) => {
                let name = path.clone();
                return self.rename = Some((Rename::Folder(path), name));
            }
            Action::DeleteFolder(path) => return self.confirm_delete_folder = Some(path),
            Action::MoveSession(id, folder) => {
                if let Some(mut session) = self.core.store.get(id).cloned()
                    && session.folder != folder
                {
                    session.folder = folder;
                    self.run(Command::SessionUpdate {
                        session: Box::new(session),
                    });
                }
                return;
            }
            Action::CloseTabs(ids) => {
                for id in ids {
                    self.run(Command::TabClose { tab: Some(id) });
                }
                return;
            }
            Action::SelectTab(id) => return self.core.active = Some(id),
            Action::CycleTab(step) => {
                let tabs = &self.core.tabs;
                if let Some(i) = tabs.iter().position(|t| Some(t.id) == self.core.active) {
                    let n = tabs.len() as i64;
                    let next = (i as i64 + i64::from(step)).rem_euclid(n) as usize;
                    self.core.active = Some(tabs[next].id);
                }
                return;
            }
            Action::KeyboardShortcuts => return self.show_hotkeys = true,
            Action::KeyGenerator => return self.keygen.open = true,
            Action::KnownHosts => return self.known_hosts.open(),
            Action::DetachTab(id, on) => {
                if let Some(tab) = self.core.tabs.iter_mut().find(|t| t.id == id) {
                    tab.detached = on;
                }
                return;
            }
            Action::SetTabColor(id, color) => {
                if let Some(tab) = self.core.tabs.iter_mut().find(|t| t.id == id) {
                    tab.color = color;
                }
                return;
            }
            Action::TogglePin(id) => {
                if let Some(tab) = self.core.tabs.iter_mut().find(|t| t.id == id) {
                    tab.pinned = !tab.pinned;
                }
                return;
            }
            Action::QuickConnect(target) => return self.run(Command::TabQuickConnect { target }),
            Action::ZoomIn | Action::ZoomOut => {
                let delta = if matches!(action, Action::ZoomIn) {
                    1.0
                } else {
                    -1.0
                };
                let active = self.core.active;
                // Zoom the active tab when it has its own (session) font size.
                if let Some(tab) = self.core.tabs.iter_mut().find(|t| Some(t.id) == active)
                    && let Some(size) = &mut tab.font_size
                {
                    *size = (*size + delta).clamp(8.0, 40.0);
                    return;
                }
                let t = &mut self.core.settings.terminal;
                t.font_size = (t.font_size + delta).clamp(8.0, 32.0);
            }
            Action::ToggleSidebar => view.sidebar_visible = !view.sidebar_visible,
            Action::ToggleCompact => view.compact = !view.compact,
            Action::SidebarRight(right) => view.sidebar_right = right,
            Action::SidebarTab(tab) => view.sidebar_tab = tab,
            Action::ToggleTheme => {
                // Light -> Dim -> Dark -> Light.
                view.theme = match view.theme {
                    Theme::Light => Theme::Dim,
                    Theme::Dim => Theme::Dark,
                    Theme::Dark | Theme::System => Theme::Light,
                };
            }
            Action::Bars { menu, toolbar } => {
                view.show_menu_bar = menu;
                view.show_toolbar = toolbar;
            }
            Action::ToolbarStyle(style) => view.toolbar_style = style,
            Action::ToggleTabNumbers => view.show_tab_numbers = !view.show_tab_numbers,
            Action::Split(mode) => view.split = mode,
            Action::ToggleMultiExec => return self.multiexec.on = !self.multiexec.on,
            Action::StartRecording => {
                if let Some(id) = self.core.active {
                    self.recording = Some((id, String::new()));
                }
                return;
            }
            Action::StopRecording => {
                if let Some((_, keys)) = self.recording.take()
                    && !keys.is_empty()
                {
                    let n = self.core.macros.macros.len() + 1;
                    self.rename = Some((Rename::NewMacro(keys), format!("Macro {n}")));
                }
                return;
            }
            Action::RenameMacro(id) => {
                let name = self.core.macros.get(id).map(|m| m.name.clone());
                self.rename = name.map(|n| (Rename::Macro(id), n));
                return;
            }
            Action::ToggleCloseButton => view.show_close_button = !view.show_close_button,
        }
        if let Err(e) = self.core.save_settings() {
            self.status = format!("could not save settings: {e}");
        }
    }

    fn live_sessions(&self) -> usize {
        self.core.tabs.iter().filter(|t| t.is_alive()).count() + self.core.running.len()
    }

    /// Ask where to save, then export through the command (one session, or
    /// all of them).
    fn export_sessions(&mut self, one: Option<SessionId>) {
        let name = one
            .and_then(|id| self.core.store.get(id))
            .map_or_else(|| "MobaXterm sessions".to_string(), |s| s.name.clone());
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{name}.mxtsessions"))
            .add_filter("MobaXterm sessions", &["mxtsessions"])
            .save_file()
        else {
            return;
        };
        let command = Command::SessionExportMobaxterm {
            path: path.display().to_string(),
            sessions: one.map(|id| vec![id.to_string()]),
            // The save dialog already asked before replacing a file.
            overwrite: true,
        };
        self.status = match tc_commands::execute(&mut self.core, command) {
            Ok(v) => format!(
                "Exported {} session(s) to {}",
                v["exported"],
                path.display()
            ),
            Err(e) => format!("export failed: {e}"),
        };
    }

    /// Run commands from other processes (control channel) and answer.
    fn serve_control(&mut self, ctx: &egui::Context) {
        let Some(control) = &self.control else {
            return;
        };
        let incoming = control.take();
        for msg in incoming {
            let opens_tab = matches!(msg.command, Command::TabOpen { .. });
            let response = match tc_commands::execute(&mut self.core, msg.command) {
                Ok(result) => tc_commands::control::Response {
                    ok: true,
                    result,
                    error: None,
                },
                Err(e) => tc_commands::control::Response {
                    ok: false,
                    result: serde_json::Value::Null,
                    error: Some(e.to_string()),
                },
            };
            if opens_tab && response.ok {
                // Like opening a bookmark from the shell: bring us forward.
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            let _ = msg.reply.send(response);
        }
    }

    /// "Detach tab": each detached tab in its own native window; closing
    /// the window puts the tab back.
    fn detached_windows(&mut self, ctx: &egui::Context) {
        let font_size = self.core.settings.terminal.font_size;
        let opts = self.core.settings.terminal.clone();
        for tab in self.core.tabs.iter_mut().filter(|t| t.detached) {
            let viewport = egui::ViewportId::from_hash_of(("detached-tab", tab.id));
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{} - TermCraft", tab.title))
                .with_inner_size([900.0, 600.0]);
            let close = ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
                let output = term_view::show(ui, tab, font_size, true, &opts);
                if let Some(text) = output.paste {
                    tab.paste(&text);
                }
                for bytes in output.input {
                    tab.send(&bytes);
                }
                ui.ctx().input(|i| i.viewport().close_requested())
            });
            if close {
                tab.detached = false;
            }
        }
    }

    /// Keys pressed at a "Session stopped" prompt: close, restart, save.
    fn ended_actions(&mut self) {
        let chosen: Vec<(u64, crate::tab::EndedAction)> = self
            .core
            .tabs
            .iter_mut()
            .filter_map(|t| t.ended_action.take().map(|a| (t.id, a)))
            .collect();
        for (id, action) in chosen {
            use crate::tab::EndedAction as E;
            match action {
                E::Close => self.run(Command::TabClose { tab: Some(id) }),
                E::Restart => {
                    let env = self.core.tab_env();
                    if let Some(tab) = self.core.tabs.iter_mut().find(|t| t.id == id) {
                        let session = tab.session.clone();
                        let title = tab.title.clone();
                        *tab = Tab::open(id, session, &env);
                        tab.title = title;
                    }
                }
                E::Save => {
                    self.core.active = Some(id);
                    self.save_terminal_text();
                }
            }
        }
    }

    /// "Save terminal text": history and screen of the active tab.
    fn save_terminal_text(&mut self) {
        let Some(tab) = self.core.active_tab_mut() else {
            return;
        };
        let name = tab
            .title
            .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{name}.txt"))
            .add_filter("Text", &["txt", "log"])
            .save_file()
        else {
            return;
        };
        let text = tab.term.history_text();
        self.status = match std::fs::write(&path, text) {
            Ok(()) => format!("Terminal text saved to {}", path.display()),
            Err(e) => format!("could not save {}: {e}", path.display()),
        };
    }

    /// "Print terminal text": the system's plain-text printing (Notepad on
    /// Windows, `lp` elsewhere).
    fn print_terminal_text(&mut self) {
        let Some(tab) = self.core.active_tab_mut() else {
            return;
        };
        let text = tab.term.history_text();
        let dir = std::env::temp_dir().join("termcraft-print");
        let file = dir.join(format!("terminal-{}.txt", tab.id));
        let result = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&file, text))
            .map_err(|e| e.to_string())
            .and_then(|()| {
                let (program, args): (&str, Vec<std::ffi::OsString>) = if cfg!(windows) {
                    ("notepad.exe", vec!["/p".into(), file.clone().into()])
                } else {
                    ("lp", vec![file.clone().into()])
                };
                std::process::Command::new(program)
                    .args(args)
                    .spawn()
                    .map(drop)
                    .map_err(|e| format!("{program}: {e}"))
            });
        self.status = match result {
            Ok(()) => "Terminal text sent to the printer".into(),
            Err(e) => format!("printing failed: {e}"),
        };
    }

    /// "Remote-monitoring" on or off in every SSH tab.
    fn apply_remote_monitoring(&mut self) {
        let on = self.core.settings.ssh.remote_monitoring;
        for tab in &mut self.core.tabs {
            tab.set_monitoring(on);
        }
    }

    /// Exit now, or ask first ("Close prompt") when sessions are open.
    fn request_exit(&mut self, ctx: &egui::Context) {
        if self.core.settings.terminal.confirm_exit
            && self.live_sessions() > 0
            && !self.exit_confirmed
        {
            self.confirm_exit = true;
        } else {
            self.exit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) {
        if self.show_config {
            let mut open = true;
            let changed = crate::ui::config_window::show(
                ctx,
                &mut open,
                &mut self.config_tab,
                &mut self.core.settings,
                &self.core.paths,
                out,
            );
            self.show_config = open;
            if changed {
                self.apply_remote_monitoring();
                if let Err(e) = self.core.save_settings() {
                    self.status = format!("could not save settings: {e}");
                }
            }
        }
        if self.confirm_exit {
            let n = self.live_sessions();
            let mut answer = None;
            egui::Window::new("Exit TermCraft")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{n} session(s) or tunnel(s) are still open. Do you want to exit TermCraft?"
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Exit").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            if let Some(exit) = answer {
                self.confirm_exit = false;
                if exit {
                    self.exit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if self.show_tunnels {
            let mut open = true;
            crate::ui::tunnels_window::manager(
                ctx,
                &mut open,
                &self.core.tunnels.tunnels,
                &self.core.running,
                &mut self.tunnel_editor,
                out,
            );
            self.show_tunnels = open;
        }
        if let Some(editor) = &mut self.tunnel_editor
            && !editor.show(ctx, out)
        {
            self.tunnel_editor = None;
        }
        crate::ui::tunnels_window::prompts(ctx, &mut self.core.running);
        self.save_password_dialog(ctx);
        self.paste_confirm_dialog(ctx);
        self.upload_edited_dialog(ctx);
        self.keygen.show(ctx);
        let known_hosts_file = self.core.paths.known_hosts_file();
        self.known_hosts.show(ctx, &known_hosts_file, out);
        if self.show_hotkeys
            && crate::ui::hotkeys_window::show(
                ctx,
                &mut self.show_hotkeys,
                &mut self.core.settings.hotkeys,
            )
            && let Err(e) = self.core.save_settings()
        {
            self.status = format!("could not save settings: {e}");
        }
        self.passwords_window(ctx, out);
        self.host_key_dialog(ctx);
        self.certificate_dialog(ctx);
        if let Some(editor) = &mut self.editor {
            editor.credentials = self
                .core
                .vault
                .credentials()
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            if !editor.show(ctx, out) {
                self.editor = None;
            }
        }

        if self.show_import {
            let mut open = true;
            egui::Window::new("Import MobaXterm sessions")
                .open(&mut open)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Path to an exported .mxtsessions file or MobaXterm.ini:");
                    ui.text_edit_singleline(&mut self.import_path);
                    if ui.button("Import").clicked() && !self.import_path.trim().is_empty() {
                        out.push(Action::Run(Command::SessionImportMobaxterm {
                            path: self.import_path.trim().trim_matches('"').to_string(),
                        }));
                        self.show_import = false;
                    }
                });
            self.show_import &= open;
        }

        if self.show_about {
            let (portable, dir) = (self.core.paths.portable, self.core.paths.config_dir.clone());
            egui::Window::new("About TermCraft")
                .open(&mut self.show_about)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading(format!("TermCraft {}", env!("CARGO_PKG_VERSION")));
                    ui.label("All-in-one terminal and remote-access toolbox, in pure Rust.");
                    ui.label("Licensed under MIT OR Apache-2.0. Icons: Phosphor (MIT).");
                    if portable {
                        ui.label("Running in portable mode.");
                    }
                    ui.weak(format!("Data: {}", dir.display()));
                });
        }

        let mut finish_rename = None;
        if let Some((target, text)) = &mut self.rename {
            let title = match target {
                Rename::Tab(_) => "Rename tab",
                Rename::Session(_) => "Rename session",
                Rename::Macro(_) => "Rename macro",
                Rename::NewMacro(_) => "Save recorded macro",
                Rename::NewFolder(_) => "New folder",
                Rename::Folder(_) => "Rename folder",
            };
            let mut open = true;
            egui::Window::new(title)
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    let edit = ui.text_edit_singleline(text);
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if !edit.has_focus() && !enter {
                        edit.request_focus();
                    }
                    if ui.button("OK").clicked() || enter {
                        finish_rename = Some(true);
                    }
                });
            if !open {
                finish_rename = Some(false);
            }
        }
        if let Some(ok) = finish_rename
            && let Some((target, text)) = self.rename.take()
            && ok
        {
            out.push(Action::Run(match target {
                Rename::Tab(id) => Command::TabRename {
                    tab: Some(id),
                    title: text,
                },
                Rename::Session(id) => Command::SessionRename { id, name: text },
                Rename::Macro(id) => Command::MacroRename { id, name: text },
                Rename::NewMacro(keys) => Command::MacroAdd {
                    r#macro: tc_core::Macro::new(text, keys),
                },
                Rename::NewFolder(parent) => Command::SessionFolderAdd {
                    path: if parent.is_empty() {
                        text
                    } else {
                        format!("{parent}/{text}")
                    },
                },
                Rename::Folder(from) => Command::SessionFolderRename { from, to: text },
            }));
        }

        if let Some(path) = self.confirm_delete_folder.clone() {
            let inside = self
                .core
                .store
                .sessions
                .iter()
                .filter(|s| s.folder == path || s.folder.starts_with(&format!("{path}/")))
                .count();
            let mut decided = None;
            egui::Window::new("Delete folder")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Delete folder \"{path}\"?"));
                    if inside > 0 {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("The {inside} session(s) inside it are deleted too."),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            decided = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decided = Some(false);
                        }
                    });
                });
            if let Some(yes) = decided {
                self.confirm_delete_folder = None;
                if yes {
                    out.push(Action::Run(Command::SessionFolderRemove { path }));
                }
            }
        }

        if let Some(id) = self.confirm_delete {
            let name = self
                .core
                .store
                .get(id)
                .map_or_else(String::new, |s| s.name.clone());
            let mut decided = None;
            egui::Window::new("Delete session")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Delete session \"{name}\"?"));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            decided = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decided = Some(false);
                        }
                    });
                });
            if let Some(yes) = decided {
                self.confirm_delete = None;
                if yes {
                    out.push(Action::Run(Command::SessionRemove { id }));
                }
            }
        }
    }

    /// Reference `TFORMSTOREPASSWORDS`: "Do you want to save this password?"
    fn save_password_dialog(&mut self, ctx: &egui::Context) {
        let Some((target, _)) = self.offers.front() else {
            return;
        };
        let mut answer = None;
        egui::Window::new("Save password")
            .id(egui::Id::new("save-password"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(ui::icon::KEY).size(36.0));
                    ui.vertical(|ui| {
                        ui.strong("Do you want to save this password?");
                        ui.label(format!("{}@{}", target.username, target.server));
                    });
                });
                ui.weak(match self.core.vault.storage() {
                    tc_vault::Storage::Keychain => {
                        "TermCraft stores passwords in your system keychain \
                         (Windows Credential Manager, macOS Keychain or Secret Service)."
                    }
                    tc_vault::Storage::File => {
                        "TermCraft will use your \"Master Password\" to secure \
                         all your stored passwords with strong encryption."
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("No").clicked() {
                        answer = Some(false);
                    }
                });
                ui.checkbox(&mut self.dont_ask_again, "Do not show this message again");
            });
        let Some(yes) = answer else { return };
        let (target, password) = self.offers.pop_front().expect("checked above");
        if yes && let Err(e) = self.core.vault.save(&target, &password) {
            self.status = format!("could not save password: {e}");
        }
        if self.dont_ask_again {
            self.core.settings.passwords.save = if yes {
                tc_core::settings::SavePasswords::Always
            } else {
                tc_core::settings::SavePasswords::Never
            };
            if let Err(e) = self.core.save_settings() {
                self.status = format!("could not save settings: {e}");
            }
        }
    }

    /// An edited remote file changed locally: upload it?
    fn upload_edited_dialog(&mut self, ctx: &egui::Context) {
        let Some(tab) = self
            .core
            .tabs
            .iter_mut()
            .find(|t| !t.upload_prompts.is_empty())
        else {
            return;
        };
        let changed = tab.upload_prompts.front().cloned().expect("checked");
        let mut answer = None;
        egui::Window::new("TermCraft")
            .id(egui::Id::new(("upload-edited", tab.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(ui::icon::UPLOAD_SIMPLE).size(36.0));
                    ui.vertical(|ui| {
                        ui.strong("The file has been modified.");
                        ui.monospace(&changed.remote);
                        ui.label("Do you want to upload it to the server?");
                    });
                });
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("No").clicked() {
                        answer = Some(false);
                    }
                });
            });
        let Some(yes) = answer else { return };
        tab.upload_prompts.pop_front();
        if yes {
            tab.upload_edited(&changed);
        }
    }

    /// "You are about to paste several lines": confirm before sending.
    fn paste_confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(tab) = self
            .core
            .tabs
            .iter_mut()
            .find(|t| t.pending_paste.is_some())
        else {
            return;
        };
        let text = tab.pending_paste.clone().unwrap_or_default();
        let lines = text.trim_end().lines().count();
        let mut answer = None;
        let mut dont_ask = !self.core.settings.terminal.warn_multiline_paste;
        egui::Window::new("TermCraft")
            .id(egui::Id::new(("paste-confirm", tab.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(ui::icon::CLIPBOARD_TEXT).size(36.0));
                    ui.vertical(|ui| {
                        ui.strong(format!("You are about to paste {lines} lines."));
                        ui.label("Each line may run as a command. Do you want to continue?");
                    });
                });
                let preview: String = text.lines().take(6).collect::<Vec<_>>().join("\n");
                ui.add(
                    egui::Label::new(egui::RichText::new(preview).monospace().small()).truncate(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Paste").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
                ui.checkbox(&mut dont_ask, "Do not show this message again");
            });
        let Some(paste) = answer else { return };
        if let Some(text) = tab.pending_paste.take()
            && paste
        {
            tab.paste(&text);
        }
        if dont_ask == self.core.settings.terminal.warn_multiline_paste {
            self.core.settings.terminal.warn_multiline_paste = !dont_ask;
            if let Err(e) = self.core.save_settings() {
                self.status = format!("could not save settings: {e}");
            }
        }
    }

    /// Reference `TFORMMANAGEPASSWORDS` and the Master Password dialogs.
    fn passwords_window(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) {
        let outcome = self.passwords.show(
            ctx,
            &self.core.vault,
            &mut self.core.settings.passwords,
            out,
        );
        if let Some(status) = outcome.status {
            self.status = status;
        }
        if outcome.settings_changed
            && let Err(e) = self.core.save_settings()
        {
            self.status = format!("could not save settings: {e}");
        }
    }

    /// "Host key validation" for the active tab's pending SSH connection.
    /// FTPS: the server certificate did not verify. Like the host key
    /// dialog: Accept (remember it) / Connect once / Cancel.
    fn certificate_dialog(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.core.tabs.iter_mut().find(|t| t.cert_prompt.is_some()) else {
            return;
        };
        let prompt = tab.cert_prompt.as_ref().expect("checked");
        let mut answer = None;
        egui::Window::new("TermCraft Security Alert")
            .id(egui::Id::new(("ftps-cert", tab.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(560.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(ui::icon::SHIELD_WARNING)
                            .size(44.0)
                            .color(ui.visuals().warn_fg_color),
                    );
                    ui.vertical(|ui| {
                        ui.label("The certificate of this FTPS server could not be verified:");
                        ui.monospace(format!("    {}", prompt.reason));
                        ui.label("Its fingerprint is:");
                        ui.monospace(format!("    {}", prompt.fingerprint));
                        ui.label(
                            "Compare it with the one the server administrator gave you. Press \
                             \"Accept\" to trust this certificate from now on, \"Connect Once\" \
                             to trust it for this connection only, or \"Cancel\".",
                        );
                    });
                });
                ui.horizontal(|ui| {
                    if ui.button("Accept").clicked() {
                        answer = Some((true, true));
                    }
                    if ui.button("Connect Once").clicked() {
                        answer = Some((true, false));
                    }
                    let cancel = ui.button("Cancel");
                    cancel.request_focus();
                    if cancel.clicked() {
                        answer = Some((false, false));
                    }
                });
            });
        if let Some((accept, remember)) = answer {
            tab.answer_certificate(accept, remember);
        }
    }

    /// Host key validation for the active tab's pending SSH connection.
    fn host_key_dialog(&mut self, ctx: &egui::Context) {
        let active = self.core.active;
        let Some(tab) = self.core.tabs.iter_mut().find(|t| Some(t.id) == active) else {
            return;
        };
        let Some(info) = &tab.host_key else {
            return;
        };
        let id = egui::Id::new(("host-key", tab.id));
        if let Some(a) = ui::host_key_dialog::show(ctx, id, info) {
            tab.answer_host_key(a.accept, a.remember);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut install = None;
                match &self.updates.state {
                    UpdateState::Idle | UpdateState::UpToDate => {}
                    UpdateState::Checking => {
                        ui.label("Checking for updates…");
                    }
                    UpdateState::Available(update) => {
                        ui.label(format!("Update {} available", update.version));
                        if ui.button("Install").clicked() {
                            install = Some(update.clone());
                        }
                    }
                    UpdateState::Installing => {
                        ui.label("Installing update…");
                    }
                    UpdateState::Installed(v) => {
                        ui.label(format!("Updated to {v} — restart to apply"));
                    }
                    UpdateState::Error(e) => {
                        ui.weak(format!("Updates: {e}"));
                    }
                }
                if let Some(update) = install {
                    self.updates.install(&ctx, update);
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                    if ui.small_button(ui::icon::X).clicked() {
                        self.status.clear();
                    }
                }
            });
        });
    }
}

/// Reference macro-recording bar; returns true when STOP is clicked.
fn recording_banner(ui: &mut egui::Ui) -> bool {
    let mut stop = false;
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(120, 24, 24))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  Recording macro: everything typed in this terminal is recorded",
                        ui::icon::RECORD
                    ))
                    .color(egui::Color32::WHITE),
                );
                stop = ui.button("STOP").clicked();
            });
        });
    stop
}

/// Reference MultiExec bar.
/// "Write commands on all terminals" (MultiExec).
#[derive(Default)]
struct MultiExec {
    on: bool,
    /// "Synchronize mouse wheel scrolling".
    sync_wheel: bool,
}

/// The MultiExec bar: which terminals take part (reference
/// `PopupMultiExec`: activate all / none / invert, synchronized wheel).
fn multiexec_banner(ui: &mut egui::Ui, multiexec: &mut MultiExec, tabs: &mut [Tab]) {
    egui::Frame::new()
        .fill(ui.visuals().selection.bg_fill)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let text = ui.visuals().selection.stroke.color;
                ui.label(
                    egui::RichText::new("Multi-execution mode: commands are typed to").color(text),
                );
                for tab in tabs.iter_mut().filter(|t| t.is_alive()) {
                    let label = egui::RichText::new(&tab.title).color(text);
                    ui.checkbox(&mut tab.multiexec_on, label);
                }
                ui.menu_button(ui::icon::DOTS_THREE, |ui| {
                    if ui.button("Activate all").clicked() {
                        tabs.iter_mut().for_each(|t| t.multiexec_on = true);
                    }
                    if ui.button("Activate none").clicked() {
                        tabs.iter_mut().for_each(|t| t.multiexec_on = false);
                    }
                    if ui.button("Invert selection").clicked() {
                        tabs.iter_mut()
                            .for_each(|t| t.multiexec_on = !t.multiexec_on);
                    }
                    ui.separator();
                    ui.checkbox(
                        &mut multiexec.sync_wheel,
                        "Synchronize mouse wheel scrolling",
                    );
                });
                if ui.button("Exit multi-execution mode").clicked() {
                    multiexec.on = false;
                }
            });
        });
}

/// Tabs that receive input typed in `source`: the source itself, and in
/// MultiExec mode every live terminal that takes part.
fn receives(tab: &Tab, source: u64, multiexec: bool) -> bool {
    tab.id == source || multiexec && tab.multiexec_on && tab.is_alive()
}

/// "Find next": the next match above the last one (or from the bottom).
fn find_next(tab: &mut Tab) {
    let Some(bar) = &mut tab.find else {
        return;
    };
    match tab.term.find(&bar.text, bar.last, bar.match_case) {
        Some((start, _)) => {
            bar.last = Some(start);
            bar.not_found = false;
        }
        None => {
            // Wrap around once from the bottom.
            bar.not_found = bar.last.is_none();
            bar.last = None;
            if let Some((start, _)) = tab.term.find(&bar.text, None, bar.match_case) {
                bar.last = Some(start);
            }
        }
    }
}

/// The "Find in terminal" strip above a terminal. Enter / F3 find the next
/// match upwards; Esc closes it.
fn find_bar(ui: &mut egui::Ui, tab: &mut Tab) {
    let Some(bar) = &mut tab.find else {
        return;
    };
    let mut close = false;
    let mut search = false;
    ui.horizontal(|ui| {
        ui.label(ui::icon::MAGNIFYING_GLASS);
        let edit = ui.add(
            egui::TextEdit::singleline(&mut bar.text)
                .hint_text("Find in terminal")
                .desired_width(220.0),
        );
        if std::mem::take(&mut bar.focus) {
            edit.request_focus();
        }
        if edit.changed() {
            bar.last = None;
            bar.not_found = false;
        }
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            search = true;
            edit.request_focus();
        }
        if ui.button("Find next").on_hover_text("F3").clicked() {
            search = true;
        }
        if ui.checkbox(&mut bar.match_case, "Match case").changed() {
            bar.last = None;
        }
        if bar.not_found {
            ui.colored_label(ui.visuals().warn_fg_color, "Not found");
        }
        if ui
            .small_button(ui::icon::X)
            .on_hover_text("Close")
            .clicked()
            || ui.input(|i| i.key_pressed(egui::Key::Escape)) && edit.has_focus()
        {
            close = true;
        }
    });
    if search {
        find_next(tab);
    }
    if close {
        tab.find = None;
        tab.term.clear_selection();
    }
}

/// Draw the terminals for the current split mode: the active tab alone, or
/// the first 2 / 4 tabs in panes. Returns each pane's output.
fn terminals(
    ui: &mut egui::Ui,
    core: &mut Core,
    split: tc_core::settings::SplitMode,
    font_size: f32,
) -> Vec<(u64, term_view::TermOutput)> {
    use tc_core::settings::SplitMode as S;
    let opts = &core.settings.terminal.clone();
    let area = ui.available_rect_before_wrap();
    let gap = 3.0;
    let rects: Vec<egui::Rect> = match split {
        S::Single => vec![area],
        S::Vertical => {
            let (l, r) = area.split_left_right_at_fraction(0.5);
            vec![
                l.shrink2(egui::vec2(gap, 0.0)),
                r.shrink2(egui::vec2(gap, 0.0)),
            ]
        }
        S::Horizontal => {
            let (t, b) = area.split_top_bottom_at_fraction(0.5);
            vec![
                t.shrink2(egui::vec2(0.0, gap)),
                b.shrink2(egui::vec2(0.0, gap)),
            ]
        }
        S::Quad => {
            let (t, b) = area.split_top_bottom_at_fraction(0.5);
            let (tl, tr) = t.split_left_right_at_fraction(0.5);
            let (bl, br) = b.split_left_right_at_fraction(0.5);
            [tl, tr, bl, br].map(|r| r.shrink(gap)).to_vec()
        }
    };
    let active = core.active;
    let ids: Vec<u64> = if split == S::Single {
        active.into_iter().collect()
    } else {
        core.tabs.iter().take(rects.len()).map(|t| t.id).collect()
    };
    let mut outputs = Vec::new();
    for (i, rect) in rects.iter().enumerate() {
        let mut pane = ui.new_child(egui::UiBuilder::new().max_rect(*rect));
        match ids
            .get(i)
            .and_then(|id| core.tabs.iter_mut().find(|t| t.id == *id))
        {
            Some(tab) if tab.detached => {
                pane.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 3.0);
                        ui.label(format!("\"{}\" is shown in its own window.", tab.title));
                        if ui.button("Re-attach tab").clicked() {
                            tab.detached = false;
                        }
                    });
                });
            }
            Some(tab) => {
                let is_active = Some(tab.id) == active;
                let id = tab.id;
                if tab.find.is_some() {
                    find_bar(&mut pane, tab);
                    let rest = pane.available_rect_before_wrap();
                    pane = ui.new_child(egui::UiBuilder::new().max_rect(rest));
                }
                if tab.monitor.is_some() {
                    // "Remote monitoring" bar under the terminal.
                    let (term_rect, bar) =
                        rect.split_top_bottom_at_y(rect.bottom() - crate::ui::monitor_bar::HEIGHT);
                    crate::ui::monitor_bar::show(&mut pane, bar, tab.monitor.as_deref());
                    pane = ui.new_child(egui::UiBuilder::new().max_rect(term_rect));
                }
                outputs.push((
                    id,
                    term_view::show(&mut pane, tab, font_size, is_active, opts),
                ));
                if split != S::Single && is_active {
                    pane.painter().rect_stroke(
                        *rect,
                        0.0,
                        egui::Stroke::new(1.5, pane.visuals().selection.bg_fill),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            None => {
                pane.centered_and_justified(|ui| {
                    ui.weak("Open another tab to fill this terminal pane.");
                });
            }
        }
    }
    ui.allocate_rect(area, egui::Sense::hover());
    outputs
}

/// Skin and "Graphical interface elements size"; both are no-ops when
/// nothing changed.
fn apply_view(ctx: &egui::Context, view: &tc_core::settings::ViewSettings) {
    crate::skin::apply(ctx, view.theme);
    let zoom = view.ui_size.zoom();
    if (ctx.zoom_factor() - zoom).abs() > f32::EPSILON {
        ctx.set_zoom_factor(zoom);
    }
}

impl eframe::App for TermCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        apply_view(&ctx, &self.core.settings.view);
        for cmd in std::mem::take(&mut self.startup) {
            self.run(cmd);
        }
        self.serve_control(&ctx);
        // Shortcuts first, so the terminal never sees their keys.
        let shortcuts = crate::ui::hotkeys_window::take_shortcuts(
            &ctx,
            &self.core.settings.hotkeys,
            self.core.active,
        );
        // The window close button goes through the same "Close prompt".
        if ctx.input(|i| i.viewport().close_requested()) && !self.exit_confirmed {
            // Snapshot runs close themselves without asking.
            if self.core.settings.terminal.confirm_exit
                && self.live_sessions() > 0
                && self.snapshot.is_none()
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_exit = true;
            } else {
                self.exit_confirmed = true;
            }
        }
        self.updates.poll();
        if self.core.settings.update.auto_install
            && let UpdateState::Available(update) = &self.updates.state
        {
            let update = update.clone();
            self.updates.install(&ctx, update);
        }
        for t in self.core.running.values_mut() {
            t.pump();
            self.offers.extend(t.offers.drain(..));
        }
        let mut monitoring = None;
        let active = self.core.active;
        for tab in &mut self.core.tabs {
            if tab.pump() && Some(tab.id) != active {
                tab.activity = true;
            }
            if Some(tab.id) == active {
                tab.activity = false;
            }
            monitoring = monitoring.or(tab.monitoring_request.take());
            if let Some(after) = tab.wakeup() {
                ctx.request_repaint_after(after);
            }
            self.offers.extend(tab.offers.drain(..));
            // Like the reference: the SSH-browser shows up on SSH login.
            if std::mem::take(&mut tab.sftp_just_ready)
                && Some(tab.id) == self.core.active
                && self.core.settings.ssh.switch_to_browser
            {
                self.core.settings.view.sidebar_tab = tc_core::settings::SidebarTab::Sftp;
                self.core.settings.view.sidebar_visible = true;
            }
        }
        self.ended_actions();
        if let Some(on) = monitoring {
            self.core.settings.ssh.remote_monitoring = on;
            self.apply_remote_monitoring();
            if let Err(e) = self.core.save_settings() {
                self.status = format!("could not save settings: {e}");
            }
        }

        if let Some(snap) = &mut self.snapshot {
            let active = self.core.active;
            let tab = self.core.tabs.iter_mut().find(|t| Some(t.id) == active);
            let first_frame = !self.show_tunnels && snap.tunnels;
            match snap.window.take().as_deref() {
                Some("config") => {
                    self.show_config = true;
                    self.config_tab = crate::ui::config_window::ConfigTab::Terminal;
                }
                Some("passwords") => self.passwords.open = true,
                Some("master") => self.passwords.ask_unlock(),
                Some("credentials") => self.passwords.open_credentials(),
                Some("hotkeys") => self.show_hotkeys = true,
                Some("keygen") => self.keygen.open = true,
                Some("known_hosts") => self.known_hosts.open(),
                Some(w) if w.starts_with("new_session") => {
                    let index = w
                        .split(':')
                        .nth(1)
                        .and_then(|i| i.parse().ok())
                        .unwrap_or(0);
                    self.editor = Some(SessionEditor::new_of_type(index));
                }
                Some("master_settings") => self
                    .passwords
                    .open_master_settings(self.core.settings.passwords.master_prompt),
                _ => {}
            }
            for cmd in snap.step(&ctx, tab, &mut self.core.running) {
                self.run(cmd);
            }
            // Keep "Do you want to save this password?" out of snapshots.
            self.offers.clear();
            if first_frame {
                self.show_tunnels = true;
                let ids: Vec<TunnelId> = self.core.tunnels.tunnels.iter().map(|t| t.id).collect();
                for id in ids {
                    self.run(Command::TunnelStart { id });
                }
            }
        }

        let mut out = shortcuts;
        let view = self.core.settings.view.clone();

        if view.show_menu_bar && !view.compact {
            let cx = menus::MenuCtx {
                view: &view,
                sessions: &self.core.store.sessions,
                macros: &self.core.macros.macros,
                recording: self.recording.is_some(),
                has_tab: !self.core.tabs.is_empty(),
                active_tab: self.core.active,
            };
            egui::Panel::top("menu").show(ui, |ui| menus::menu_bar(ui, &cx, &mut out));
        }
        if view.show_toolbar && !view.compact {
            let sessions = &self.core.store.sessions;
            egui::Panel::top("toolbar")
                .exact_size(toolbar::height(view.toolbar_style))
                .show(ui, |ui| toolbar::show(ui, &view, sessions, &mut out));
        }
        if !view.compact {
            self.status_bar(ui);
        }

        let side = |id: &'static str| {
            if view.sidebar_right {
                egui::Panel::right(id)
            } else {
                egui::Panel::left(id)
            }
        };
        if !view.compact {
            side("sidebar-strip")
                .exact_size(sidebar::STRIP_WIDTH)
                .resizable(false)
                .show(ui, |ui| sidebar::strip(ui, &view, &mut out));
        }
        if view.sidebar_visible && !view.compact {
            // Open sessions: connected if any of their tabs is.
            let mut open_sessions = std::collections::HashMap::new();
            for t in &self.core.tabs {
                if let Some(s) = &t.session {
                    *open_sessions.entry(s.id).or_insert(false) |= t.is_alive();
                }
            }
            let folders = self.core.store.all_folders();
            let sessions = &self.core.store.sessions;
            let quick = &mut self.quick_connect;
            let macros = &self.core.macros.macros;
            let recording = self.recording.is_some();
            let active = self.core.active;
            let active_tab = self.core.tabs.iter_mut().find(|t| Some(t.id) == active);
            side("sidebar")
                .default_size(sidebar::PANEL_WIDTH)
                .min_size(160.0)
                .show(ui, |ui| {
                    let macros = sidebar::Macros {
                        list: macros,
                        recording,
                    };
                    let sessions = sidebar::Sessions {
                        folders: &folders,
                        list: sessions,
                        open: &open_sessions,
                        dots: view.session_dots,
                    };
                    sidebar::content(ui, quick, &view, sessions, macros, active_tab, &mut out)
                });
        }

        let tab_views: Vec<tabbar::TabView> = self
            .core
            .tabs
            .iter()
            .map(|t| tabbar::TabView {
                id: t.id,
                title: t.title.clone(),
                alive: t.is_alive(),
                icon: t
                    .session
                    .as_ref()
                    .map_or(crate::ui::icon::TERMINAL_WINDOW, crate::ui::session_icon),
                color: t.color,
                pinned: t.pinned,
                activity: t.activity,
                detached: t.detached,
            })
            .collect();
        let font_size = self.core.settings.terminal.font_size;
        let core = &mut self.core;
        let home_search = &mut self.home_search;
        let multiexec = &mut self.multiexec;
        let recording = &mut self.recording;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(ui.visuals().panel_fill))
            .show(ui, |ui| {
                tabbar::show(ui, &tab_views, core.active, &view, &mut out);
                if multiexec.on {
                    multiexec_banner(ui, multiexec, &mut core.tabs);
                }
                if recording.is_some() && recording_banner(ui) {
                    out.push(Action::StopRecording);
                }
                if core.tabs.is_empty() {
                    let recent = recent_sessions(&core.store, 9);
                    home::show(ui, home_search, &core.store.sessions, &recent, &mut out);
                    return;
                }
                let outputs = terminals(ui, core, view.split, font_size);
                for (id, output) in outputs {
                    if output.clicked {
                        out.push(Action::SelectTab(id));
                    }
                    let on = multiexec.on;
                    if let Some(text) = output.paste {
                        for tab in core.tabs.iter_mut().filter(|t| receives(t, id, on)) {
                            tab.paste(&text);
                        }
                    }
                    if let Some(lines) = output.scrolled
                        && on
                        && multiexec.sync_wheel
                    {
                        for tab in core
                            .tabs
                            .iter_mut()
                            .filter(|t| t.id != id && receives(t, id, on))
                        {
                            term_view::scroll_by(tab, lines);
                        }
                    }
                    for bytes in output.input {
                        if let Some((rid, keys)) = recording.as_mut()
                            && *rid == id
                        {
                            keys.push_str(&String::from_utf8_lossy(&bytes));
                        }
                        for tab in core.tabs.iter_mut().filter(|t| receives(t, id, on)) {
                            tab.send(&bytes);
                        }
                    }
                }
            });

        self.detached_windows(&ctx);
        self.dialogs(&ctx, &mut out);
        for action in out {
            self.apply(&ctx, action);
        }
    }
}
