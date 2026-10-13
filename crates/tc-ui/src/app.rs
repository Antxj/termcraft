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
    tunnels: TunnelStore,
    running: BTreeMap<TunnelId, RunningTunnel>,
}

impl Core {
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
        let env = TabEnv {
            scrollback: self.settings.terminal.scrollback_lines,
            known_hosts: self.paths.known_hosts_file(),
            repaint: self.repaint.clone(),
            vault: self.vault.clone(),
            save_passwords: self.settings.passwords.save,
        };
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
            self.settings.passwords.save,
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
}

pub struct TermCraftApp {
    core: Core,
    updates: Updates,
    quick_connect: String,
    home_search: String,
    import_path: String,
    show_import: bool,
    show_about: bool,
    show_passwords: bool,
    show_tunnels: bool,
    tunnel_editor: Option<crate::ui::tunnels_window::TunnelEditor>,
    /// "Do you want to save this password?" questions from tabs and tunnels.
    offers: VecDeque<(tc_vault::Target, String)>,
    /// Selected rows in the Passwords settings list.
    password_selection: std::collections::BTreeSet<tc_vault::Target>,
    dont_ask_again: bool,
    rename: Option<(Rename, String)>,
    confirm_delete: Option<SessionId>,
    editor: Option<SessionEditor>,
    snapshot: Option<crate::snapshot::Snapshot>,
    status: String,
}

impl TermCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
        let tunnels = TunnelStore::load(&paths.tunnels_file()).unwrap_or_else(|e| {
            status = format!("could not load tunnels: {e}");
            TunnelStore::default()
        });
        let vault = Arc::new(tc_vault::Vault::open(
            paths.passwords_index_file(),
            Box::new(tc_vault::OsKeychain),
        ));
        apply_theme(&cc.egui_ctx, settings.view.theme);
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
                tunnels,
                running: BTreeMap::new(),
            },
            updates,
            quick_connect: String::new(),
            home_search: String::new(),
            import_path: String::new(),
            show_import: false,
            show_about: false,
            show_passwords: false,
            show_tunnels: false,
            tunnel_editor: None,
            offers: VecDeque::new(),
            password_selection: Default::default(),
            dont_ask_again: false,
            rename: None,
            confirm_delete: None,
            editor: None,
            snapshot: crate::snapshot::Snapshot::from_env(),
            status,
        };
        app.autostart_tunnels();
        app
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
            Action::Quit => return ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Minimize => {
                return ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            Action::ToggleFullscreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                return ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            Action::About => return self.show_about = true,
            Action::PasswordsSettings => return self.show_passwords = true,
            Action::Tunnels => return self.show_tunnels = true,
            Action::CheckUpdates => return self.updates.check(ctx),
            Action::ImportDialog => return self.show_import = true,
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
            Action::CloseTabs(ids) => {
                for id in ids {
                    self.run(Command::TabClose { tab: Some(id) });
                }
                return;
            }
            Action::SelectTab(id) => return self.core.active = Some(id),
            Action::QuickConnect(target) => return self.run(Command::TabQuickConnect { target }),
            Action::ZoomIn | Action::ZoomOut => {
                let t = &mut self.core.settings.terminal;
                let delta = if matches!(action, Action::ZoomIn) {
                    1.0
                } else {
                    -1.0
                };
                t.font_size = (t.font_size + delta).clamp(8.0, 32.0);
            }
            Action::ToggleSidebar => view.sidebar_visible = !view.sidebar_visible,
            Action::SidebarRight(right) => view.sidebar_right = right,
            Action::SidebarTab(tab) => view.sidebar_tab = tab,
            Action::ToggleTheme => {
                view.theme = if ctx.theme() == egui::Theme::Dark {
                    Theme::Light
                } else {
                    Theme::Dark
                };
                apply_theme(ctx, view.theme);
            }
            Action::Bars { menu, toolbar } => {
                view.show_menu_bar = menu;
                view.show_toolbar = toolbar;
            }
            Action::ToolbarStyle(style) => view.toolbar_style = style,
            Action::ToggleTabNumbers => view.show_tab_numbers = !view.show_tab_numbers,
            Action::ToggleCloseButton => view.show_close_button = !view.show_close_button,
        }
        if let Err(e) = self.core.save_settings() {
            self.status = format!("could not save settings: {e}");
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) {
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
        self.passwords_window(ctx, out);
        self.host_key_dialog(ctx);
        if let Some(editor) = &mut self.editor
            && !editor.show(ctx, out)
        {
            self.editor = None;
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
            }));
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
                ui.weak(
                    "TermCraft stores passwords in your system keychain \
                     (Windows Credential Manager, macOS Keychain or Secret Service).",
                );
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

    /// Reference `TFORMMANAGEPASSWORDS` (without "Show passwords": secrets
    /// stay in the keychain).
    fn passwords_window(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) {
        if !self.show_passwords {
            return;
        }
        use tc_core::settings::SavePasswords as S;
        let mut open = true;
        let mut mode = self.core.settings.passwords.save;
        let saved = self.core.vault.list();
        egui::Window::new("TermCraft passwords settings")
            .open(&mut open)
            .collapsible(false)
            .default_width(640.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Save sessions passwords");
                    ui.radio_value(&mut mode, S::Always, "Always");
                    ui.radio_value(&mut mode, S::Never, "Never");
                    ui.radio_value(&mut mode, S::Ask, "Ask");
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        egui::Grid::new("saved-passwords")
                            .num_columns(3)
                            .striped(true)
                            .show(ui, |ui| {
                                ui.strong("Protocol");
                                ui.strong("Username");
                                ui.strong("Servername");
                                ui.end_row();
                                for t in &saved {
                                    let selected = self.password_selection.contains(t);
                                    if ui.selectable_label(selected, &t.protocol).clicked() {
                                        if selected {
                                            self.password_selection.remove(t);
                                        } else {
                                            self.password_selection.insert(t.clone());
                                        }
                                    }
                                    ui.label(&t.username);
                                    ui.label(&t.server);
                                    ui.end_row();
                                }
                            });
                        if saved.is_empty() {
                            ui.weak("No saved passwords.");
                        }
                    });
                ui.horizontal(|ui| {
                    let any = !self.password_selection.is_empty();
                    if ui
                        .add_enabled(any, egui::Button::new("Delete selected"))
                        .clicked()
                    {
                        for target in std::mem::take(&mut self.password_selection) {
                            out.push(Action::Run(Command::PasswordForget { target }));
                        }
                    }
                    if ui
                        .add_enabled(!saved.is_empty(), egui::Button::new("Delete all"))
                        .clicked()
                    {
                        self.password_selection.clear();
                        out.push(Action::Run(Command::PasswordForgetAll {}));
                    }
                });
                ui.weak(
                    "Passwords are kept in the system keychain; this list only holds their names.",
                );
            });
        self.show_passwords = open;
        if mode != self.core.settings.passwords.save {
            self.core.settings.passwords.save = mode;
            if let Err(e) = self.core.save_settings() {
                self.status = format!("could not save settings: {e}");
            }
        }
    }

    /// "Host key validation" for the active tab's pending SSH connection.
    fn host_key_dialog(&mut self, ctx: &egui::Context) {
        let active = self.core.active;
        let Some(tab) = self.core.tabs.iter_mut().find(|t| Some(t.id) == active) else {
            return;
        };
        let Some(prompt) = &tab.host_key else {
            return;
        };
        let mut answer = None;
        egui::Window::new("Host key validation")
            .id(egui::Id::new(("host-key", tab.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(ui::icon::SHIELD_WARNING).size(40.0));
                    ui.vertical(|ui| {
                        ui.label(format!("The host key of {} is not known yet.", prompt.host));
                        ui.label("Check the fingerprint before trusting this server.");
                    });
                });
                ui.add_space(6.0);
                egui::Grid::new("host-key-info")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Key type:");
                        ui.monospace(&prompt.algorithm);
                        ui.end_row();
                        ui.label("SHA256 fingerprint:");
                        ui.monospace(&prompt.fingerprint);
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Accept and save").clicked() {
                        answer = Some((true, true));
                    }
                    if ui.button("Accept once").clicked() {
                        answer = Some((true, false));
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some((false, false));
                    }
                });
            });
        if let Some((accept, remember)) = answer {
            tab.answer_host_key(accept, remember);
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

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::System => egui::ThemePreference::System,
        Theme::Light => egui::ThemePreference::Light,
        Theme::Dark => egui::ThemePreference::Dark,
    });
}

impl eframe::App for TermCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
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
        for tab in &mut self.core.tabs {
            tab.pump();
            self.offers.extend(tab.offers.drain(..));
            // Like the reference: the SSH-browser shows up on SSH login.
            if std::mem::take(&mut tab.sftp_just_ready) && Some(tab.id) == self.core.active {
                self.core.settings.view.sidebar_tab = tc_core::settings::SidebarTab::Sftp;
                self.core.settings.view.sidebar_visible = true;
            }
        }

        if let Some(snap) = &mut self.snapshot {
            let active = self.core.active;
            let tab = self.core.tabs.iter_mut().find(|t| Some(t.id) == active);
            let first_frame = !self.show_tunnels && snap.tunnels;
            if let Some(cmd) = snap.step(&ctx, tab, &mut self.core.running) {
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

        let mut out = Vec::new();
        let view = self.core.settings.view.clone();

        if view.show_menu_bar {
            let cx = menus::MenuCtx {
                view: &view,
                sessions: &self.core.store.sessions,
                has_tab: !self.core.tabs.is_empty(),
                active_tab: self.core.active,
            };
            egui::Panel::top("menu").show(ui, |ui| menus::menu_bar(ui, &cx, &mut out));
        }
        if view.show_toolbar {
            let sessions = &self.core.store.sessions;
            egui::Panel::top("toolbar")
                .exact_size(toolbar::height(view.toolbar_style))
                .show(ui, |ui| toolbar::show(ui, &view, sessions, &mut out));
        }
        self.status_bar(ui);

        let side = |id: &'static str| {
            if view.sidebar_right {
                egui::Panel::right(id)
            } else {
                egui::Panel::left(id)
            }
        };
        side("sidebar-strip")
            .exact_size(sidebar::STRIP_WIDTH)
            .resizable(false)
            .show(ui, |ui| sidebar::strip(ui, &view, &mut out));
        if view.sidebar_visible {
            let sessions = &self.core.store.sessions;
            let quick = &mut self.quick_connect;
            let active = self.core.active;
            let active_tab = self.core.tabs.iter_mut().find(|t| Some(t.id) == active);
            side("sidebar")
                .default_size(sidebar::PANEL_WIDTH)
                .min_size(160.0)
                .show(ui, |ui| {
                    sidebar::content(ui, quick, &view, sessions, active_tab, &mut out)
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
            })
            .collect();
        let font_size = self.core.settings.terminal.font_size;
        let core = &mut self.core;
        let home_search = &mut self.home_search;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(ui.visuals().panel_fill))
            .show(ui, |ui| {
                tabbar::show(ui, &tab_views, core.active, &view, &mut out);
                let active = core.active;
                if let Some(tab) = core.tabs.iter_mut().find(|t| Some(t.id) == active) {
                    term_view::show(ui, tab, font_size);
                } else {
                    let recent = recent_sessions(&core.store, 9);
                    home::show(ui, home_search, &core.store.sessions, &recent, &mut out);
                }
            });

        self.dialogs(&ctx, &mut out);
        for action in out {
            self.apply(&ctx, action);
        }
    }
}
