//! Main-window chrome laid out after the reference inventory
//! (`docs/inventory/forms/TFORMXTERM.md`, `menus.md`, `toolbar.md`).
//! Widgets never mutate state directly: they emit [`Action`]s that the app
//! applies after the frame, and persistent changes go through the command
//! registry.

pub mod config_window;
pub mod diff_window;
pub mod expert_window;
pub mod form;
pub mod home;
pub mod host_key_dialog;
pub mod hotkeys_window;
pub mod image_viewer;
pub mod import_dialog;
pub mod jump_hosts_dialog;
pub mod keygen_window;
pub mod known_hosts_window;
pub mod local_pane;
pub mod menus;
pub mod monitor_bar;
pub mod passwords_window;
pub mod servers_window;
pub mod session_editor;
pub mod session_icons;
pub mod sftp_browser;
pub mod shared_window;
pub mod sidebar;
pub mod syntax_window;
pub mod tabbar;
pub mod text_editor;
pub mod toolbar;
pub mod tunnels_window;
pub mod wol_dialog;

use tc_commands::Command;
use tc_core::settings::{SidebarTab, SplitMode, ToolbarStyle};
use tc_core::{MacroId, SessionId};

pub use egui_phosphor::regular as icon;

pub const NOT_YET: &str = "Not implemented yet (tracked in docs/PARITY.md)";

pub enum Action {
    Run(Command),
    Quit,
    Minimize,
    ToggleFullscreen,
    /// View › Compact mode.
    ToggleCompact,
    About,
    /// Help › "Documentation": the project's documentation in the browser.
    Documentation,
    /// Help › "View TermCraft log file" (in the text editor).
    ViewLog,
    /// View › "Take a screenshot" of the window (asks where to save it).
    Screenshot,
    /// Tools › "List running processes" / "List hardware devices": a
    /// terminal tab showing them.
    ListProcesses,
    ListHardware,
    /// Tools › "Command Prompt (admin)" / "Windows Powershell (admin)"
    /// (`true` = PowerShell): asks Windows to run it elevated.
    AdminShell(bool),
    /// Tools › "Wake On Lan".
    WakeOnLan,
    /// "Import sessions from a CSV file" (asks for the file).
    ImportCsv,
    /// The Home tab (no terminal tab active).
    ShowHome,
    /// Ctrl+Alt+Del to the remote desktop of this tab.
    RdpCtrlAltDel(u64),
    /// Settings › "Edit session presets" (the editor in preset mode).
    EditPresets,
    /// Keep this preset for its session type / drop the preset of a type.
    SavePreset(Box<tc_core::Session>),
    RemovePreset(usize),
    /// Session menu › "Ping host": a terminal tab pinging it.
    PingHost(String),
    PasswordsSettings,
    /// Passwords settings on the Credentials tab.
    ManageCredentials,
    /// "Customize syntax highlighting".
    CustomizeSyntax,
    /// Configuration › Terminal › "Expert settings".
    ExpertTerminal,
    /// "Servers": the network services window.
    Servers,
    /// Tools › "Text editor".
    TextEditor,
    /// "Generate HTML web page" (asks where to save).
    ExportHtml,
    /// Settings › "Export configuration" / "Import configuration" (ask for
    /// the file) and "Reset configuration" (asks first).
    ExportConfig,
    ImportConfig,
    ResetConfig,
    /// "Execute all sessions from this folder".
    OpenFolder(String),
    /// "Close all folders" in the sessions tree.
    CollapseFolders,
    /// "Connect as...": ask for a user, then open a copy as that user.
    ConnectAs(Box<tc_core::Session>),
    /// Open a shared session (a read-only copy) in a tab.
    OpenSharedSession(Box<tc_core::Session>),
    /// "Refresh shared sessions" / "Manage existing shared sessions".
    RefreshShared,
    ManageShared,
    /// Tools › "Diff tool" / "Ascii table".
    DiffTool,
    AsciiTable,
    Configuration,
    Tunnels,
    CheckUpdates,
    ImportDialog,
    SaveTerminalText,
    PrintTerminalText,
    FindInTerminal,
    /// `F3`.
    FindNext,
    /// "Export MobaXterm sessions" (all) / "Save session to file" (one).
    ExportSessions(Option<SessionId>),
    /// "Create a desktop shortcut" to the named session ("Shortcut
    /// options" first).
    DesktopShortcut(String),
    NewSession,
    /// The session editor for a new session of this type (index).
    NewSessionOfType(usize),
    EditSession(SessionId),
    RenameTab(u64),
    RenameSession(SessionId),
    DeleteSession(SessionId),
    /// Drag and drop in the sessions tree: new folder ("" = top level).
    MoveSession(SessionId, String),
    /// "New folder" inside this folder ("" = top level).
    NewFolder(String),
    RenameFolder(String),
    DeleteFolder(String),
    /// Close several tabs (the "Close all …" family).
    CloseTabs(Vec<u64>),
    SetTabColor(u64, Option<[u8; 3]>),
    /// "Detach tab" / "Re-attach tab".
    DetachTab(u64, bool),
    TogglePin(u64),
    SelectTab(u64),
    /// Next (+1) / previous (-1) tab.
    CycleTab(i32),
    /// Settings › Keyboard shortcuts.
    KeyboardShortcuts,
    /// Tools › SSH key generator.
    KeyGenerator,
    /// Configuration › SSH › Known hosts.
    KnownHosts,
    QuickConnect(String),
    ZoomIn,
    ZoomOut,
    ToggleSidebar,
    SidebarRight(bool),
    SidebarTab(SidebarTab),
    ToggleTheme,
    Bars {
        menu: bool,
        toolbar: bool,
    },
    ToolbarStyle(ToolbarStyle),
    ToggleTabNumbers,
    Split(SplitMode),
    ToggleMultiExec,
    StartRecording,
    StopRecording,
    RenameMacro(MacroId),
    ToggleCloseButton,
    ToggleSidebarTitles,
}

/// A menu entry: enabled when it has an action, greyed out with a hint otherwise.
pub fn item(ui: &mut egui::Ui, label: &str, action: Option<Action>, out: &mut Vec<Action>) {
    match action {
        Some(action) => {
            if ui.button(label).clicked() {
                out.push(action);
                ui.close();
            }
        }
        None => {
            ui.add_enabled(false, egui::Button::new(label))
                .on_disabled_hover_text(NOT_YET);
        }
    }
}

/// A checkable menu entry.
pub fn check(ui: &mut egui::Ui, label: &str, checked: bool, action: Action, out: &mut Vec<Action>) {
    if ui.add(egui::Button::new(label).selected(checked)).clicked() {
        out.push(action);
        ui.close();
    }
}

/// A non-clickable group title inside a menu (e.g. "System" in Tools).
pub fn header(ui: &mut egui::Ui, label: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(label).strong().small());
}

/// Glyph for a session: its chosen icon, else its type's icon.
pub fn session_icon(session: &tc_core::Session) -> &'static str {
    session_icons::glyph(session)
}

/// Enter in a single-line field of a window acts like the window's
/// default button: `let enter = EnterWatch::start(ui);` first in the
/// window's contents, then `enter.pressed(ui)` next to that button. A
/// single-line field gives the focus up on Enter; terminals and multi-line
/// fields keep it, so they never trigger it.
pub struct EnterWatch(bool);

impl EnterWatch {
    pub fn start(ui: &egui::Ui) -> Self {
        Self(ui.memory(|m| m.focused().is_some()))
    }

    pub fn pressed(&self, ui: &egui::Ui) -> bool {
        self.0
            && ui.memory(|m| m.focused().is_none())
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
    }
}

#[cfg(test)]
mod enter_tests {
    use super::EnterWatch;

    fn enter_key() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// One frame with a single-line (or multi-line) field; true if the
    /// window's default button would act.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, multiline: bool, focus: bool) -> bool {
        let mut pressed = false;
        let mut text = String::from("abc");
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let enter = EnterWatch::start(ui);
            let edit = if multiline {
                egui::TextEdit::multiline(&mut text)
            } else {
                egui::TextEdit::singleline(&mut text)
            };
            let r = ui.add(edit.id_salt("field"));
            if focus {
                r.request_focus();
            }
            pressed = enter.pressed(ui);
        });
        output.textures_delta.clear();
        pressed
    }

    #[test]
    fn enter_in_a_single_line_field_submits() {
        let ctx = egui::Context::default();
        assert!(!frame(&ctx, Vec::new(), false, true), "no key");
        assert!(
            frame(&ctx, vec![enter_key()], false, false),
            "Enter in the field"
        );
        assert!(
            !frame(&ctx, vec![enter_key()], false, false),
            "the field had no focus"
        );
    }

    #[test]
    fn enter_in_a_multi_line_field_does_not() {
        let ctx = egui::Context::default();
        assert!(!frame(&ctx, Vec::new(), true, true));
        assert!(
            !frame(&ctx, vec![enter_key()], true, false),
            "a new line, not OK"
        );
    }
}
