//! Main menu bar — same menus, order and labels as the reference `MainMenu3`.
//! Items without an implementation are shown disabled.

use super::{Action, check, header, item};
use tc_commands::Command;
use tc_core::Session;
use tc_core::settings::{ToolbarStyle, ViewSettings};

pub struct MenuCtx<'a> {
    pub view: &'a ViewSettings,
    pub sessions: &'a [Session],
    pub has_tab: bool,
    pub active_tab: Option<u64>,
}

pub fn menu_bar(ui: &mut egui::Ui, cx: &MenuCtx<'_>, out: &mut Vec<Action>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("Terminal", |ui| terminal_menu(ui, cx, out));
        ui.menu_button("Sessions", |ui| sessions_menu(ui, cx.sessions, out));
        ui.menu_button("View", |ui| view_menu(ui, cx.view, out));
        ui.menu_button("X server", |ui| {
            item(ui, "Start X server", None, out);
            item(ui, "XDMCP session", None, out);
        });
        ui.menu_button("Tools", |ui| tools_menu(ui, out));
        ui.menu_button("Games", |ui| games_menu(ui, out));
        ui.menu_button("Settings", |ui| {
            item(ui, "Configuration", None, out);
            item(ui, "Keyboard shortcuts", None, out);
            ui.separator();
            item(ui, "Import configuration", None, out);
            item(ui, "Export configuration", None, out);
            item(ui, "Reset configuration", None, out);
            ui.separator();
            item(
                ui,
                "Passwords settings",
                Some(Action::PasswordsSettings),
                out,
            );
            item(ui, "Manage shared sessions", None, out);
            ui.separator();
            item(ui, "Edit session presets", None, out);
        });
        ui.menu_button("Macros", |ui| {
            item(ui, "Record new macro", None, out);
            ui.separator();
            header(ui, "Saved macros");
        });
        ui.menu_button("Help", |ui| help_menu(ui, out));
    });
}

fn terminal_menu(ui: &mut egui::Ui, cx: &MenuCtx<'_>, out: &mut Vec<Action>) {
    let on_tab = |a: Action| cx.has_tab.then_some(a);
    item(
        ui,
        "Open new tab",
        Some(Action::Run(Command::TabOpen { session: None })),
        out,
    );
    item(
        ui,
        "Close current tab",
        on_tab(Action::Run(Command::TabClose { tab: None })),
        out,
    );
    ui.separator();
    item(
        ui,
        "Set tab title",
        cx.active_tab.map(Action::RenameTab),
        out,
    );
    item(ui, "Detach tab", None, out);
    item(ui, "Save terminal text", None, out);
    item(ui, "Print terminal text", None, out);
    item(ui, "Find in terminal", None, out);
    item(ui, "Find next", None, out);
    ui.separator();
    item(
        ui,
        "Duplicate current tab",
        on_tab(Action::Run(Command::TabDuplicate { tab: None })),
        out,
    );
    item(ui, "Write commands on all terminals", None, out);
    ui.separator();
    item(ui, "Quit", Some(Action::Quit), out);
}

pub fn sessions_menu(ui: &mut egui::Ui, sessions: &[Session], out: &mut Vec<Action>) {
    item(ui, "New session", Some(Action::NewSession), out);
    ui.separator();
    if sessions.is_empty() {
        ui.weak("No saved sessions");
    }
    for s in sessions {
        let label = format!("{}  {}", super::kind_icon(&s.kind), s.name);
        let open = Command::TabOpen {
            session: Some(s.id.to_string()),
        };
        item(ui, &label, Some(Action::Run(open)), out);
    }
}

pub fn view_menu(ui: &mut egui::Ui, v: &ViewSettings, out: &mut Vec<Action>) {
    split_items(ui, out);
    ui.separator();
    item(ui, "Compact mode", None, out);
    item(ui, "Fullscreen mode", Some(Action::ToggleFullscreen), out);
    item(ui, "Iconify TermCraft", Some(Action::Minimize), out);
    ui.separator();
    let bars = |menu, toolbar| Action::Bars { menu, toolbar };
    check(
        ui,
        "Show menu bar",
        v.show_menu_bar && !v.show_toolbar,
        bars(true, false),
        out,
    );
    check(
        ui,
        "Show buttons bar",
        !v.show_menu_bar && v.show_toolbar,
        bars(false, true),
        out,
    );
    check(
        ui,
        "Show both",
        v.show_menu_bar && v.show_toolbar,
        bars(true, true),
        out,
    );
    ui.separator();
    for (label, style) in [
        ("Small buttons", ToolbarStyle::Small),
        ("Standard buttons", ToolbarStyle::Standard),
        (
            "Standard buttons with captions",
            ToolbarStyle::StandardWithCaptions,
        ),
    ] {
        check(
            ui,
            label,
            v.toolbar_style == style,
            Action::ToolbarStyle(style),
            out,
        );
    }
    ui.separator();
    item(ui, "Terminal zoom", Some(Action::ZoomIn), out);
    item(ui, "Terminal unzoom", Some(Action::ZoomOut), out);
    ui.separator();
    check(
        ui,
        "Show/hide sidebar",
        v.sidebar_visible,
        Action::ToggleSidebar,
        out,
    );
    check(
        ui,
        "Put sidebar on the left",
        !v.sidebar_right,
        Action::SidebarRight(false),
        out,
    );
    check(
        ui,
        "Put sidebar on the right",
        v.sidebar_right,
        Action::SidebarRight(true),
        out,
    );
    ui.separator();
    check(
        ui,
        "Show tab numbers",
        v.show_tab_numbers,
        Action::ToggleTabNumbers,
        out,
    );
    check(
        ui,
        "Show close button",
        v.show_close_button,
        Action::ToggleCloseButton,
        out,
    );
    ui.separator();
    item(
        ui,
        "Toggle light/dark theme",
        Some(Action::ToggleTheme),
        out,
    );
    item(ui, "Show/hide sidebar tab titles", None, out);
    ui.separator();
    item(ui, "Take a screenshot", None, out);
}

pub fn split_items(ui: &mut egui::Ui, out: &mut Vec<Action>) {
    item(ui, "Single terminal mode", None, out);
    item(ui, "2 terminals mode (vertical split)", None, out);
    item(ui, "2 terminals mode (horizontal split)", None, out);
    item(ui, "4 terminals mode", None, out);
}

pub fn tools_menu(ui: &mut egui::Ui, out: &mut Vec<Action>) {
    for (group, tools) in TOOLS {
        header(ui, group);
        for t in *tools {
            item(ui, t, tool_action(t), out);
        }
        ui.separator();
    }
}

/// Tools that are implemented.
pub fn tool_action(name: &str) -> Option<Action> {
    match name {
        "SSH tunnels (port forwarding)" => Some(Action::Tunnels),
        _ => None,
    }
}

/// Visible entries of the reference Tools menu, by group.
pub const TOOLS: &[(&str, &[&str])] = &[
    (
        "System",
        &[
            "Packages manager",
            "X11 tab with Dwm",
            "X11 window with Fvwm",
            "X11 window with Twm",
            "List hardware devices",
            "List running processes",
            "Command Prompt (admin)",
            "Windows Powershell (admin)",
        ],
    ),
    ("Office", &["Text editor", "Diff tool", "Ascii table"]),
    (
        "Network",
        &[
            "Network services",
            "SSH tunnels (port forwarding)",
            "SSH key generator",
            "List open network ports",
            "Wake On Lan",
            "Network scanner",
            "Ports scanner",
            "Network packets capture",
        ],
    ),
];

pub fn games_menu(ui: &mut egui::Ui, out: &mut Vec<Action>) {
    header(ui, "Terminal games");
    for g in [
        "Soliterm",
        "Termwalk",
        "Ctris",
        "Sudoku",
        "Nimesweeper",
        "Ninvaders",
        "Greed",
        "Netwalk",
        "Mines",
        "Solo",
        "Guess",
        "Map",
        "Untangle",
    ] {
        item(ui, g, None, out);
    }
}

pub fn help_menu(ui: &mut egui::Ui, out: &mut Vec<Action>) {
    item(ui, "Documentation", None, out);
    item(ui, "Check for updates", Some(Action::CheckUpdates), out);
    item(ui, "View TermCraft log file", None, out);
    ui.separator();
    item(ui, "About TermCraft", Some(Action::About), out);
}
