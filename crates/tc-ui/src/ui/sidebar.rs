//! Left sidebar (reference `PanGauche` / `PCGauche`): a vertical tab strip
//! (pin, Sessions, Tools, Macros, Sftp) and a 240px content panel with the
//! "Quick connect..." box on top.

use super::{Action, icon, item, menus, session_icons};
use egui::{Align2, FontId, Sense, vec2};
use std::collections::{BTreeMap, HashMap};
use tc_commands::Command;
use tc_core::settings::{SidebarTab, ViewSettings};
use tc_core::{Session, SessionId};

pub const STRIP_WIDTH: f32 = 34.0;
pub const PANEL_WIDTH: f32 = 300.0;

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

/// A sidebar tab: icon on top, title written top to bottom, accent bar on
/// the selected one (reference vertical tabs).
fn strip_tab(ui: &mut egui::Ui, glyph: &str, title: &str, selected: bool) -> egui::Response {
    let palette = crate::skin::current(ui.ctx());
    let font = FontId::proportional(12.5);
    let color = if selected {
        palette.text
    } else {
        palette.weak_text
    };
    let galley = ui.painter().layout_no_wrap(title.to_string(), font, color);
    let height = 30.0 + galley.size().x + 10.0;
    let (rect, response) = ui.allocate_exact_size(vec2(STRIP_WIDTH, height), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 3.0, palette.tab_active);
        let bar = egui::Rect::from_min_size(rect.min, vec2(3.0, rect.height()));
        painter.rect_filled(bar, 1.0, palette.accent);
    } else if response.hovered() {
        painter.rect_filled(rect, 3.0, palette.tab_hover);
    }
    painter.text(
        egui::pos2(rect.center().x, rect.top() + 15.0),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(17.0),
        if selected { palette.accent } else { color },
    );
    // Rotated a quarter turn clockwise: the text runs downwards.
    let pos = egui::pos2(rect.center().x + galley.size().y / 2.0, rect.top() + 30.0);
    painter.add(
        egui::epaint::TextShape::new(pos, galley, color).with_angle(std::f32::consts::FRAC_PI_2),
    );
    response
}

pub fn strip(ui: &mut egui::Ui, view: &ViewSettings, out: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add_space(4.0);
    if strip_button(ui, icon::PUSH_PIN, false, "Pin sidebar").clicked() {
        out.push(Action::ToggleSidebar);
    }
    for (tab, glyph, title) in TABS {
        let selected = view.sidebar_visible && view.sidebar_tab == tab;
        if strip_tab(ui, glyph, title, selected).clicked() {
            if !view.sidebar_visible {
                out.push(Action::ToggleSidebar);
            }
            out.push(Action::SidebarTab(tab));
        }
    }
}

/// What the Sessions tab shows.
pub struct Sessions<'a> {
    pub list: &'a [Session],
    /// Sessions open in a tab: `true` = connected (green), `false` = closed (orange).
    pub open: &'a HashMap<SessionId, bool>,
    /// "Show green/orange buttons next to each open session".
    pub dots: bool,
}

/// What the Macros tab shows.
pub struct Macros<'a> {
    pub list: &'a [tc_core::Macro],
    pub recording: bool,
}

pub fn content(
    ui: &mut egui::Ui,
    quick_connect: &mut String,
    view: &ViewSettings,
    sessions: Sessions<'_>,
    macros: Macros<'_>,
    active_tab: Option<&mut crate::tab::Tab>,
    out: &mut Vec<Action>,
) {
    let has_tab = active_tab.is_some();
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
    if view.sidebar_tab == SidebarTab::Sftp {
        // The browser manages its own scrolling.
        super::sftp_browser::show(ui, active_tab);
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| match view.sidebar_tab {
            SidebarTab::Sessions => session_tree(ui, &sessions, &quick_connect.to_lowercase(), out),
            SidebarTab::Tools => tools(ui, out),
            SidebarTab::Macros => macro_list(ui, macros.list, macros.recording, has_tab, out),
            SidebarTab::Sftp => {}
        });
}

#[derive(Default)]
struct Folder<'a> {
    folders: BTreeMap<&'a str, Folder<'a>>,
    sessions: Vec<&'a Session>,
}

fn session_tree(ui: &mut egui::Ui, sessions: &Sessions<'_>, filter: &str, out: &mut Vec<Action>) {
    let mut root = Folder::default();
    for s in sessions.list {
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
        .show(ui, |ui| draw_folder(ui, &root, "", sessions, out));
    header.header_response.context_menu(|ui| root_menu(ui, out));
}

fn draw_folder(
    ui: &mut egui::Ui,
    folder: &Folder<'_>,
    path: &str,
    sessions: &Sessions<'_>,
    out: &mut Vec<Action>,
) {
    for (name, sub) in &folder.folders {
        let sub_path = format!("{path}/{name}");
        egui::CollapsingHeader::new(format!("{}  {name}", icon::FOLDER))
            .id_salt(&sub_path)
            .default_open(true)
            .show(ui, |ui| draw_folder(ui, sub, &sub_path, sessions, out));
    }
    for s in &folder.sessions {
        let row = ui
            .horizontal(|ui| {
                let row = match session_icons::session_texture(ui.ctx(), s) {
                    Some(tex) => ui.add(
                        egui::Button::image_and_text(
                            egui::Image::new(&tex).fit_to_exact_size(vec2(16.0, 16.0)),
                            format!(" {}", s.name),
                        )
                        .frame(false),
                    ),
                    None => ui.selectable_label(false, session_icons::label(s, ui)),
                };
                if sessions.dots
                    && let Some(alive) = sessions.open.get(&s.id)
                {
                    let p = crate::skin::current(ui.ctx());
                    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().circle_filled(
                        rect.center(),
                        4.0,
                        if *alive { p.ok } else { p.warn },
                    );
                }
                row
            })
            .inner;
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
    item(
        ui,
        "Save session to file",
        Some(Action::ExportSessions(Some(s.id))),
        out,
    );
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
    item(
        ui,
        "Export MobaXterm sessions",
        Some(Action::ExportSessions(None)),
        out,
    );
    ui.menu_button("Import sessions from third-party programs", |ui| {
        let wsl = cfg!(windows).then_some(Action::Run(Command::SessionImportWsl {}));
        item(ui, "Import WSL sessions", wsl, out);
        item(ui, "Import External Bash sessions", None, out);
        item(
            ui,
            "Import PuTTY sessions",
            Some(Action::Run(Command::SessionImportPutty {})),
            out,
        );
        for p in ["PuTTYCM", "SuperPuTTY", "MRemote", "Exceed", "SCRT", "RDM"] {
            item(ui, &format!("Import {p} sessions"), None, out);
        }
        item(ui, "Import sessions from a CSV file", None, out);
        ui.separator();
        item(
            ui,
            "Import OpenSSH sessions (~/.ssh/config)",
            Some(Action::Run(Command::SessionImportSshConfig { path: None })),
            out,
        );
    });
}

fn macro_list(
    ui: &mut egui::Ui,
    macros: &[tc_core::Macro],
    recording: bool,
    has_tab: bool,
    out: &mut Vec<Action>,
) {
    if recording {
        if ui
            .button(format!("{}  Stop recording", icon::STOP))
            .clicked()
        {
            out.push(Action::StopRecording);
        }
    } else {
        let record = ui.add_enabled(
            has_tab,
            egui::Button::new(format!("{}  Record new macro", icon::RECORD)),
        );
        if record
            .on_disabled_hover_text("Open a terminal first")
            .clicked()
        {
            out.push(Action::StartRecording);
        }
    }
    ui.separator();
    if macros.is_empty() {
        ui.weak("No saved macros");
    }
    for m in macros {
        let row = ui
            .selectable_label(false, format!("{}  {}", icon::RECORD, m.name))
            .on_hover_text(m.preview(120));
        let run = Action::Run(Command::MacroRun {
            id: m.id,
            tab: None,
        });
        if row.double_clicked() && has_tab {
            out.push(run);
        }
        row.context_menu(|ui| {
            let run = Action::Run(Command::MacroRun {
                id: m.id,
                tab: None,
            });
            item(ui, "Execute", has_tab.then_some(run), out);
            item(ui, "Rename", Some(Action::RenameMacro(m.id)), out);
            item(
                ui,
                "Delete",
                Some(Action::Run(Command::MacroRemove { id: m.id })),
                out,
            );
        });
    }
}

fn tools(ui: &mut egui::Ui, out: &mut Vec<Action>) {
    for (group, tools) in menus::TOOLS {
        egui::CollapsingHeader::new(*group)
            .default_open(true)
            .show(ui, |ui| {
                for t in *tools {
                    item(ui, t, menus::tool_action(t), out);
                }
            });
    }
}
