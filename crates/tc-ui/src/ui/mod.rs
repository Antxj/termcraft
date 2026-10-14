//! Main-window chrome laid out after the reference inventory
//! (`docs/inventory/forms/TFORMXTERM.md`, `menus.md`, `toolbar.md`).
//! Widgets never mutate state directly: they emit [`Action`]s that the app
//! applies after the frame, and persistent changes go through the command
//! registry.

pub mod config_window;
pub mod home;
pub mod host_key_dialog;
pub mod menus;
pub mod monitor_bar;
pub mod passwords_window;
pub mod session_editor;
pub mod session_icons;
pub mod sftp_browser;
pub mod sidebar;
pub mod tabbar;
pub mod toolbar;
pub mod tunnels_window;

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
    About,
    PasswordsSettings,
    /// Passwords settings on the Credentials tab.
    ManageCredentials,
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
    NewSession,
    EditSession(SessionId),
    RenameTab(u64),
    RenameSession(SessionId),
    DeleteSession(SessionId),
    /// Close several tabs (the "Close all …" family).
    CloseTabs(Vec<u64>),
    SetTabColor(u64, Option<[u8; 3]>),
    TogglePin(u64),
    SelectTab(u64),
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
