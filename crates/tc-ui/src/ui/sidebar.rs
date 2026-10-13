//! Left sidebar (reference `PanGauche` / `PCGauche`): a vertical tab strip
//! (pin, Sessions, Tools, Macros, Sftp) and a 240px content panel with the
//! "Quick connect..." box on top.

use super::{Action, icon, item, kind_icon, menus};
use egui::{Align2, FontId, Sense, vec2};
use std::collections::BTreeMap;
use tc_commands::Command;
use tc_core::Session;
use tc_core::settings::{SidebarTab, ViewSettings};

pub const STRIP_WIDTH: f32 = 34.0;
pub const PANEL_WIDTH: f32 = 240.0;

const TABS: [(SidebarTab, &str, &str); 4] = [
    (SidebarTab::Sessions, icon::STAR, "Sessions"),
    (SidebarTab::Tools, icon::TOOLBOX, "Tools"),
    (SidebarTab::Macros, icon::RECORD, "Macros"),
    (SidebarTab::Sftp, icon::FOLDER_SIMPLE, "Sftp"),
];

fn strip_button(ui: &mut egui::Ui, glyph: &str, selected: bool, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(STRIP_WIDTH, 32.0), Sense::click());
    let v = ui.style().interact_selectable(&response, selected);
    if selected || response.hovered() {
        ui.painter().rect_filled(rect, 3.0, v.weak_bg_fill);
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(18.0),
        v.text_color(),
    );
    response.on_hover_text(hint)
}

pub fn strip(ui: &mut egui::Ui, view: &ViewSettings, out: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add_space(4.0);
    if strip_button(ui, icon::PUSH_PIN, false, "Pin sidebar").clicked() {
        out.push(Action::ToggleSidebar);
    }
    for (tab, glyph, hint) in TABS {
        let selected = view.sidebar_visible && view.sidebar_tab == tab;
        if strip_button(ui, glyph, selected, hint).clicked() {
            if !view.sidebar_visible {
                out.push(Action::ToggleSidebar);
            }
            out.push(Action::SidebarTab(tab));
        }
    }
}

pub fn content(
    ui: &mut egui::Ui,
    quick_connect: &mut String,
    view: &ViewSettings,
    sessions: &[Session],
    out: &mut Vec<Action>,
) {
    ui.add_space(4.0);
    let response = ui.add(
        egui::TextEdit::singleline(quick_connect)
            .hint_text("Quick connect...")
            .desired_width(f32::INFINITY),
    );
    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let target = std::mem::take(quick_connect);
        if !target.trim().is_empty() {
            out.push(Action::QuickConnect(target));
        }
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| match view.sidebar_tab {
            SidebarTab::Sessions => session_tree(ui, sessions, &quick_connect.to_lowercase(), out),
            SidebarTab::Tools => tools(ui, out),
            SidebarTab::Macros => {
                item(ui, "Record new macro", None, out);
                ui.weak("No saved macros");
            }
            SidebarTab::Sftp => {
                ui.weak("The SSH-browser appears here when an SSH session is connected.");
            }
        });
}

#[derive(Default)]
struct Folder<'a> {
    folders: BTreeMap<&'a str, Folder<'a>>,
    sessions: Vec<&'a Session>,
}

fn session_tree(ui: &mut egui::Ui, sessions: &[Session], filter: &str, out: &mut Vec<Action>) {
    let mut root = Folder::default();
    for s in sessions {
        if !filter.is_empty() && !s.name.to_lowercase().contains(filter) {
            continue;
        }
        let mut node = &mut root;
        for part in s.folder.split('/').filter(|p| !p.is_empty()) {
            node = node.folders.entry(part).or_default();
        }
        node.sessions.push(s);
    }
    let header = egui::CollapsingHeader::new(format!("{}  User sessions", icon::STAR))
        .id_salt("user-sessions")
        .default_open(true)
        .show(ui, |ui| draw_folder(ui, &root, "", out));
    header.header_response.context_menu(|ui| root_menu(ui, out));
}

fn draw_folder(ui: &mut egui::Ui, folder: &Folder<'_>, path: &str, out: &mut Vec<Action>) {
    for (name, sub) in &folder.folders {
        let sub_path = format!("{path}/{name}");
        egui::CollapsingHeader::new(format!("{}  {name}", icon::FOLDER))
            .id_salt(&sub_path)
            .default_open(true)
            .show(ui, |ui| draw_folder(ui, sub, &sub_path, out));
    }
    for s in &folder.sessions {
        let row = ui.selectable_label(false, format!("{}  {}", kind_icon(&s.kind), s.name));
        if row.double_clicked() {
            out.push(open(s));
        }
        row.context_menu(|ui| session_menu(ui, s, out));
    }
}

fn open(s: &Session) -> Action {
    Action::Run(Command::TabOpen {
        session: Some(s.id.to_string()),
    })
}

/// Reference `PopupSessions`, implemented entries enabled.
fn session_menu(ui: &mut egui::Ui, s: &Session, out: &mut Vec<Action>) {
    item(ui, "Execute", Some(open(s)), out);
    item(ui, "Connect as...", None, out);
    item(ui, "SFTP session", None, out);
    item(ui, "Ping host", None, out);
    ui.separator();
    item(ui, "New session", Some(Action::NewSession), out);
    item(ui, "New folder", None, out);
    item(ui, "Rename", Some(Action::RenameSession(s.id)), out);
    item(ui, "Edit", Some(Action::EditSession(s.id)), out);
    item(ui, "Delete", Some(Action::DeleteSession(s.id)), out);
    item(
        ui,
        "Duplicate",
        Some(Action::Run(Command::SessionDuplicate { id: s.id })),
        out,
    );
    item(ui, "Save session to file", None, out);
    item(ui, "Create a desktop shortcut", None, out);
    ui.separator();
    root_menu(ui, out);
}

fn root_menu(ui: &mut egui::Ui, out: &mut Vec<Action>) {
    item(
        ui,
        "Import MobaXterm sessions",
        Some(Action::ImportDialog),
        out,
    );
    item(ui, "Export MobaXterm sessions", None, out);
    ui.menu_button("Import sessions from third-party programs", |ui| {
        for p in [
            "WSL",
            "PuTTY",
            "SuperPuTTY",
            "MRemote",
            "SecureCRT",
            "a CSV file",
        ] {
            item(ui, &format!("Import {p} sessions"), None, out);
        }
    });
}

fn tools(ui: &mut egui::Ui, out: &mut Vec<Action>) {
    for (group, tools) in menus::TOOLS {
        egui::CollapsingHeader::new(*group)
            .default_open(true)
            .show(ui, |ui| {
                for t in *tools {
                    item(ui, t, None, out);
                }
            });
    }
}
