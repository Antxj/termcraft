//! The main window: tabs, menus, dialogs and the command host behind them.

mod dialogs;
mod host;
mod panels;

use crate::tab::{Tab, TabEnv};
use crate::term_view;
use crate::tunnels::RunningTunnel;
use crate::ui::session_editor::SessionEditor;
use crate::ui::{self, Action, home, menus, sidebar, tabbar, toolbar};
use crate::updates::{UpdateState, Updates};
use panels::*;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use tc_commands::{Command, CommandError, Host, TabInfo, execute, recent_sessions};
use tc_core::settings::{Theme, UpdateChannel};
use tc_core::{Paths, Session, SessionId, SessionStore, Settings, Tunnel, TunnelId, TunnelStore};

/// Everything the command registry can act on. Kept separate from widget
/// state so `execute(&mut core, cmd)` borrows cleanly.
/// 4_38 value for "no macro".
pub const NO_MACRO: &str = "<none>";

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
    /// Network services of the "Servers" window.
    servers: crate::servers::Servers,
}

impl Core {
    /// A new tab for `session`, with its "Execute macro at session start".
    fn new_tab(&self, id: u64, session: Option<Session>) -> Tab {
        let start_macro = session
            .as_ref()
            .and_then(|s| s.options.get("4_38"))
            .map(|name| name.trim())
            .filter(|name| !name.is_empty() && *name != NO_MACRO)
            .and_then(|name| self.macros.macros.iter().find(|m| m.name == name))
            .map(|m| m.keys.clone());
        let mut tab = Tab::open(id, session, &self.tab_env());
        tab.start_macro = start_macro;
        tab
    }

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
            home: self.settings.general.terminal_home(),
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
    /// "Customize syntax highlighting".
    syntax_window: Option<crate::ui::syntax_window::SyntaxWindow>,
    /// Configuration › Terminal › "Expert settings".
    show_expert: bool,
    /// Built-in text editor.
    text_editor: crate::ui::text_editor::TextEditor,
    /// Popup ("quake") terminal and its system-wide hotkey.
    popup: crate::popup::Popup,
    /// The icon in the system tray (`None` without a tray, and in
    /// snapshot runs).
    tray: Option<crate::tray::Tray>,
    /// Shared sessions (read-only) and their management window.
    shared: crate::shared::SharedSessions,
    shared_window: Option<crate::ui::shared_window::SharedWindow>,
    /// "Connect as...": the session and the user being typed.
    connect_as: Option<(Box<tc_core::Session>, String)>,
    diff_tool: crate::ui::diff_window::DiffWindow,
    image_viewer: crate::ui::image_viewer::ImageViewer,
    ascii_table: crate::ui::diff_window::AsciiTable,
    /// "Servers management" window and its page.
    show_servers: bool,
    servers_page: crate::ui::servers_window::Page,
    /// "Shortcut options" for this session (hide terminal, close on exit).
    shortcut_dialog: Option<(String, tc_core::shortcut::ShortcutOptions)>,
    known_hosts: crate::ui::known_hosts_window::KnownHostsWindow,
    /// Loopback control channel (other processes send commands).
    control: Option<crate::control_server::ControlServer>,
    /// Commands from the command line, run on the first frame.
    startup: Vec<Command>,
    /// `--exitwhendone`: quit once the startup sessions ended (and whether
    /// one of them has been seen running).
    exit_when_done: bool,
    startup_seen_alive: bool,
    /// `--hideterm`: minimize on the first frame.
    start_minimized: bool,
    /// "Restart now" after an update: start the new version on exit.
    restart_after_exit: bool,
    /// The window was created transparent (a transparency was set at
    /// start-up); changes from 0 apply after a restart.
    transparent_window: bool,
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
    pub fn new(cc: &eframe::CreationContext<'_>, startup: crate::control_server::Startup) -> Self {
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
        let transparent_window = crate::wants_transparency(&settings.view);
        let macros = tc_core::MacroStore::load(&paths.macros_file()).unwrap_or_default();
        let tunnels = TunnelStore::load(&paths.tunnels_file()).unwrap_or_else(|e| {
            status = format!("could not load tunnels: {e}");
            TunnelStore::default()
        });
        let vault = Arc::new(tc_vault::Vault::open(
            paths.passwords_index_file(),
            Box::new(tc_vault::OsKeychain),
        ));
        crate::skin::apply(&cc.egui_ctx, settings.view.theme, settings.view.saturation);
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
                servers: Default::default(),
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
            syntax_window: None,
            show_expert: false,
            text_editor: Default::default(),
            popup: crate::popup::Popup::new(&cc.egui_ctx),
            tray: None,
            shared: Default::default(),
            shared_window: None,
            connect_as: None,
            diff_tool: Default::default(),
            image_viewer: Default::default(),
            ascii_table: Default::default(),
            show_servers: false,
            servers_page: Default::default(),
            shortcut_dialog: None,
            known_hosts: Default::default(),
            control: None,
            startup: startup.commands,
            exit_when_done: startup.exit_when_done,
            startup_seen_alive: false,
            start_minimized: startup.minimized,
            restart_after_exit: false,
            transparent_window,
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
        if app.snapshot.as_ref().is_none_or(|s| s.tray.is_some()) {
            app.tray = crate::tray::Tray::new(&cc.egui_ctx, &app.core.store.sessions);
        }
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
        // Shared sessions are read in the background at start-up.
        app.shared
            .refresh(&app.core.settings.shared_sessions, app.core.repaint.clone());
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
            Ok(value) if name == "session_desktop_shortcut" => {
                self.status = format!(
                    "Desktop shortcut created: {}",
                    value["path"].as_str().unwrap_or("?")
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
            // "Iconify TermCraft": into the tray when there is one.
            Action::Minimize if self.tray.is_some() => {
                return ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
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
            Action::ExportHtml => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("sessions.html")
                    .add_filter("Web page", &["html"])
                    .save_file()
                {
                    let cmd = Command::SessionExportHtml {
                        path: path.display().to_string(),
                        overwrite: true,
                    };
                    self.status = match tc_commands::execute(&mut self.core, cmd) {
                        Ok(_) => format!("Web page written to {}", path.display()),
                        Err(e) => e.to_string(),
                    };
                }
                return;
            }
            Action::OpenFolder(folder) => {
                let inside = |f: &str| f == folder || f.starts_with(&format!("{folder}/"));
                let ids: Vec<String> = self
                    .core
                    .store
                    .sessions
                    .iter()
                    .filter(|s| inside(&s.folder))
                    .map(|s| s.id.to_string())
                    .collect();
                for id in ids {
                    self.run(Command::TabOpen { session: Some(id) });
                }
                return;
            }
            Action::CollapseFolders => {
                let id = egui::Id::new(crate::ui::sidebar::COLLAPSE_FOLDERS);
                ctx.data_mut(|d| d.insert_temp(id, true));
                return;
            }
            Action::ConnectAs(session) => {
                self.connect_as = Some((session, String::new()));
                return;
            }
            Action::OpenSharedSession(session) => {
                if let Err(e) = self.core.tab_open(Some(*session)) {
                    self.status = e.to_string();
                }
                return;
            }
            Action::RefreshShared => {
                self.shared.refresh(
                    &self.core.settings.shared_sessions,
                    self.core.repaint.clone(),
                );
                return;
            }
            Action::ManageShared => {
                self.shared_window = Some(crate::ui::shared_window::SharedWindow::new(
                    &self.core.settings.shared_sessions,
                ));
                return;
            }
            Action::DiffTool => {
                self.diff_tool.open = true;
                return;
            }
            Action::AsciiTable => {
                self.ascii_table.open = true;
                return;
            }
            Action::TextEditor => {
                self.text_editor.open = true;
                return;
            }
            Action::Servers => {
                self.show_servers = true;
                return;
            }
            Action::ExpertTerminal => {
                self.show_expert = true;
                return;
            }
            Action::CustomizeSyntax => {
                self.syntax_window = Some(crate::ui::syntax_window::SyntaxWindow::new(
                    &self.core.settings.terminal.custom_syntaxes,
                ));
                return;
            }
            Action::Tunnels => return self.show_tunnels = true,
            Action::CheckUpdates => return self.updates.check(ctx),
            Action::ImportDialog => return self.show_import = true,
            Action::ExportSessions(one) => return self.export_sessions(one),
            Action::DesktopShortcut(name) => {
                self.shortcut_dialog = Some((name, Default::default()));
                return;
            }
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
            let response = tc_commands::control::Response::from_result(tc_commands::execute(
                &mut self.core,
                msg.command,
            ));
            if opens_tab && response.ok {
                // Like opening a bookmark from the shell: bring us forward.
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            let _ = msg.reply.send(response);
        }
    }

    /// Requests from the tray icon and its menu.
    fn serve_tray(&mut self, ctx: &egui::Context) {
        let requests = match &mut self.tray {
            Some(tray) => {
                tray.sync_sessions(&self.core.store.sessions);
                tray.take()
            }
            None => Vec::new(),
        };
        for request in requests {
            self.tray_request(ctx, request);
        }
        if let Some(step) = self.snapshot.as_mut().and_then(|s| s.tray_step()) {
            use crate::snapshot::TrayStep;
            match step {
                TrayStep::Iconify => {
                    let into = if self.tray.is_some() {
                        "the tray"
                    } else {
                        "the taskbar (no tray)"
                    };
                    eprintln!("snapshot: iconify into {into}");
                    self.apply(ctx, Action::Minimize);
                }
                TrayStep::Show => self.tray_request(ctx, crate::tray::Request::Show),
            }
        }
    }

    fn tray_request(&mut self, ctx: &egui::Context, request: crate::tray::Request) {
        let focus = self.snapshot.is_none();
        match request {
            crate::tray::Request::PopupTerminal => {
                let core = &self.core;
                self.popup.open(|| core.tab_env());
            }
            crate::tray::Request::Show => show_window(ctx, focus),
            crate::tray::Request::Then(action) => {
                // Its dialogs (or the exit prompt) need the window.
                show_window(ctx, focus);
                self.apply(ctx, action);
            }
        }
    }

    /// "Detach tab": each detached tab in its own native window; closing
    /// the window puts the tab back.
    fn detached_windows(&mut self, ctx: &egui::Context) {
        let font_size = self.core.settings.terminal.font_size;
        let opts = self.core.settings.terminal.clone();
        let transparency = self
            .core
            .settings
            .view
            .detached_transparency
            .min(tc_core::settings::MAX_TRANSPARENCY);
        for tab in self.core.tabs.iter_mut().filter(|t| t.detached) {
            let viewport = egui::ViewportId::from_hash_of(("detached-tab", tab.id));
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{} - TermCraft", tab.title))
                .with_inner_size([900.0, 600.0])
                .with_maximized(tab.detached_maximized)
                .with_transparent(transparency > 0);
            let close = ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
                if transparency > 0 {
                    ui.set_opacity(1.0 - f32::from(transparency) / 100.0);
                }
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

    /// `--exitwhendone` ("Close … on exit" shortcuts): once the sessions
    /// opened at startup ran and none is alive any more, quit.
    fn exit_when_done(&mut self, ctx: &egui::Context) {
        if !self.exit_when_done || !self.startup.is_empty() {
            return;
        }
        let alive = self.core.tabs.iter().any(|t| t.is_alive());
        if alive {
            self.startup_seen_alive = true;
        } else if self.startup_seen_alive {
            self.exit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
                    if let Some(i) = self.core.tabs.iter().position(|t| t.id == id) {
                        let old = &self.core.tabs[i];
                        let (session, title) = (old.session.clone(), old.title.clone());
                        let mut tab = self.core.new_tab(id, session);
                        tab.title = title;
                        self.core.tabs[i] = tab;
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
        let id = tab.id;
        self.status = match print_text(&text, id) {
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
}

/// Reference MultiExec bar.
/// "Write commands on all terminals" (MultiExec).
#[derive(Default)]
struct MultiExec {
    on: bool,
    /// "Synchronize mouse wheel scrolling".
    sync_wheel: bool,
}

/// Print plain text: `lp` reads it from its input on Unix (no file);
/// Windows prints a copy in the user's own temporary folder with Notepad.
fn print_text(text: &str, tab_id: u64) -> Result<(), String> {
    if cfg!(windows) {
        let dir = std::env::temp_dir().join("termcraft-print");
        let file = dir.join(format!("terminal-{tab_id}.txt"));
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&file, text))
            .map_err(|e| e.to_string())?;
        return std::process::Command::new("notepad.exe")
            .arg("/p")
            .arg(&file)
            .spawn()
            .map(drop)
            .map_err(|e| format!("notepad.exe: {e}"));
    }
    use std::io::Write as _;
    let mut child = std::process::Command::new("lp")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("lp: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("lp: no input")?
        .write_all(text.as_bytes())
        .map_err(|e| format!("lp: {e}"))?;
    // Reap it once done (the input was closed above).
    std::thread::spawn(move || drop(child.wait()));
    Ok(())
}

/// Skin and "Graphical interface elements size"; both are no-ops when
/// nothing changed.
/// Bring the main window back from the tray or the taskbar, in front
/// (`focus`; snapshot runs stay behind the user's windows).
fn show_window(ctx: &egui::Context, focus: bool) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    if focus {
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
}

fn apply_view(ctx: &egui::Context, view: &tc_core::settings::ViewSettings) {
    crate::skin::apply(ctx, view.theme, view.saturation);
    let zoom = view.ui_size.zoom();
    if (ctx.zoom_factor() - zoom).abs() > f32::EPSILON {
        ctx.set_zoom_factor(zoom);
    }
}

impl eframe::App for TermCraftApp {
    fn on_exit(&mut self) {
        if self.restart_after_exit
            && let Ok(exe) = std::env::current_exe()
        {
            let _ = std::process::Command::new(exe).spawn();
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.transparent_window {
            [0.0; 4]
        } else {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        apply_view(&ctx, &self.core.settings.view);
        // "Transparency" / "Detached/fullscreen transparency" (the window
        // itself is transparent only when it was created so).
        if self.transparent_window {
            let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            let v = &self.core.settings.view;
            let t = if full {
                v.detached_transparency
            } else {
                v.transparency
            };
            ui.set_opacity(1.0 - f32::from(t.min(tc_core::settings::MAX_TRANSPARENCY)) / 100.0);
        }
        for cmd in std::mem::take(&mut self.startup) {
            self.run(cmd);
        }
        self.serve_control(&ctx);
        self.serve_tray(&ctx);
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
        // Bookmark 5_1 "Fullscreen": the tab asks once it is open.
        if let Some(tab) = self.core.tabs.iter_mut().find(|t| t.wants_fullscreen) {
            tab.wants_fullscreen = false;
            self.core.active = Some(tab.id);
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        self.exit_when_done(&ctx);
        if std::mem::take(&mut self.start_minimized) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
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
                Some("jump_hosts") => self.editor = Some(SessionEditor::jump_hosts_preview()),
                Some("expert") => self.show_expert = true,
                Some("editor") => self.text_editor.preview(),
                Some("diff") => self.diff_tool.preview(),
                Some("servers") => {
                    self.show_servers = true;
                    self.servers_page = crate::ui::servers_window::Page::Http;
                }
                Some("syntax") => {
                    self.syntax_window = Some(crate::ui::syntax_window::SyntaxWindow::preview());
                }
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
            let (shared, shared_loading) = (&self.shared.groups, self.shared.is_loading());
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
                        shared,
                        shared_loading,
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
        // With the sidebar hidden, "Quick connect" moves next to the tabs.
        let quick =
            (!view.sidebar_visible && !view.compact && !view.no_quick_connect_without_sidebar)
                .then_some(&mut self.quick_connect);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(ui.visuals().panel_fill))
            .show(ui, |ui| {
                tabbar::show(ui, &tab_views, core.active, &view, quick, &mut out);
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
