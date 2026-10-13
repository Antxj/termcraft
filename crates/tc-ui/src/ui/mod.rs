//! Main-window chrome laid out after the reference inventory
//! (`docs/inventory/forms/TFORMXTERM.md`, `menus.md`, `toolbar.md`).
//! Widgets never mutate state directly: they emit [`Action`]s that the app
//! applies after the frame, and persistent changes go through the command
//! registry.

pub mod home;
pub mod menus;
pub mod session_editor;
pub mod sftp_browser;
pub mod sidebar;
pub mod tabbar;
pub mod toolbar;
pub mod tunnels_window;

use tc_commands::Command;
use tc_core::SessionId;
use tc_core::settings::{SidebarTab, SplitMode, ToolbarStyle};

pub use egui_phosphor::regular as icon;

pub const NOT_YET: &str = "Not implemented yet (tracked in docs/PARITY.md)";

pub enum Action {
    Run(Command),
    Quit,
    Minimize,
    ToggleFullscreen,
    About,
    PasswordsSettings,
    Tunnels,
    CheckUpdates,
    ImportDialog,
    NewSession,
    EditSession(SessionId),
    RenameTab(u64),
    RenameSession(SessionId),
    DeleteSession(SessionId),
    /// Close several tabs (the "Close all …" family).
    CloseTabs(Vec<u64>),
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

pub fn kind_icon(kind: &tc_core::SessionKind) -> &'static str {
    use tc_core::SessionKind as K;
    match kind {
        K::LocalShell { .. } => icon::TERMINAL_WINDOW,
        K::Ssh { .. } => icon::TERMINAL_WINDOW,
        K::Telnet { .. } => icon::PLUGS,
        K::Serial { .. } => icon::USB,
        K::Rdp { .. } => icon::DESKTOP,
        K::Vnc { .. } => icon::DESKTOP,
        K::Sftp { .. } | K::Ftp { .. } => icon::FOLDER,
    }
}
