//! "Session settings" dialog (reference `TFORMSESSIONEDIT`, 864×545): the
//! row of session-type buttons, the "Basic … settings" block, the tabs
//! "Advanced … / Terminal / Network / Bookmark settings", and OK / Cancel.
//! The buttons of the tabs open the sub-dialogs "Expert SSH settings",
//! "Font settings" and "Color settings".
//!
//! Every control is drawn where the reference has it
//! (`tc_core::layout`), bound to its parameter of the generated schema;
//! values are kept as strings in the schema encoding and written back on OK.

use super::jump_hosts_dialog::{self, JumpHostsDialog};
use super::session_icons::{self, Choice, IconChooser};
use super::{Action, NOT_YET, icon};
use egui::{Pos2, Rect, vec2};
use std::collections::BTreeMap;
use tc_commands::Command;
use tc_core::jump_hosts::{self, GatewayIds, JumpHost};
use tc_core::layout::{self, Ctl, Kind, Page};
use tc_core::schema::{self, Param, SESSION_TYPES, Widget};
use tc_core::{Session, SessionIcon, SessionKind};

/// Shell session "Terminal shell" (3_2), in `LocalShell::from_option` order.
const SHELL_CHOICES: [&str; 6] = [
    "<default>",
    "Bash",
    "Zsh",
    "Cmd",
    "PowerShell",
    "Windows Subsystem for Linux (WSL)",
];

/// Icon per reference type index.
const TYPE_ICONS: [&str; 15] = [
    icon::TERMINAL_WINDOW,    // SSH
    icon::PLUGS,              // Telnet
    icon::TERMINAL,           // Rsh
    icon::MONITOR,            // Xdmcp
    icon::DESKTOP,            // RDP
    icon::SCREENCAST,         // VNC
    icon::FOLDER_OPEN,        // FTP
    icon::FOLDER_SIMPLE_USER, // SFTP
    icon::USB,                // Serial
    icon::FILE,               // File
    icon::CODE,               // Shell
    icon::BROWSER,            // Browser
    icon::LIGHTNING,          // Mosh
    icon::CLOUD,              // Aws S3
    icon::LINUX_LOGO,         // WSL
];

/// Types with a terminal (they get the "Terminal settings" tab).
const TERMINAL_TYPES: &[usize] = &[0, 1, 2, 8, 10, 12, 14];

/// Reference dialog size, and where its parts are.
const WIDTH: f32 = 864.0;
const HEIGHT: f32 = 545.0;
const BASIC_Y: f32 = 121.0;
const BASIC_H: f32 = 80.0;
const TABS_Y: f32 = 229.0;
const TABS_H: f32 = 270.0;
const BUTTONS_Y: f32 = 502.0;
/// Height of a page control's tab header.
const HEADER_H: f32 = 24.0;

/// A check box that enables the field next to it. When the field is a
/// parameter of its own (a path), the box is checked while it is set.
const ENABLES: &[(&str, &str)] = &[
    ("sCheckBox23", "sFilenameEdit2"),  // SSH "Use private key"
    ("sCheckBox34", "sFilenameEdit5"),  // SFTP "Use private key"
    ("sCheckBox1", "sEdit6"),           // WSL "Specify username"
    ("sCheckBox18", "sDirectoryEdit1"), // "Log terminal output to:"
    ("sRadioButton2", "Edit1"),         // Xdmcp "Specify server to connect to:"
    ("sCheckBox9", "sSpinEdit15"),      // Font "Force fixed rows / columns"
    ("sCheckBox9", "sSpinEdit16"),
    ("sCheckBox2", "sEdit24"), // Xdmcp "Enable font server"
];

/// Internet Explorer options of a Browser session (the Edge ones are on
/// panel `sPanel3`).
const IE_CONTROLS: &[&str] = &[
    "sCheckBox43",
    "sCheckBox44",
    "sCheckBox45",
    "sCheckBox46",
    "sCheckBox47",
    "sCheckBox48",
    "sCheckBox49",
    "sCheckBox50",
    "sComboBox33",
];

/// The multi-line "Execute the following commands at startup" fields.
const STARTUP_MEMOS: &[&str] = &["Memo1", "WslScript"];

/// Small buttons next to "Username": named credentials.
const CREDENTIAL_BUTTONS: &[&str] = &["sBitBtn3", "sBitBtn4", "sBitBtn5", "sBitBtn6", "sBitBtn9"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Advanced,
    Terminal,
    Network,
    Bookmark,
}

impl Tab {
    fn sheet(self, type_index: usize) -> &'static str {
        match self {
            Tab::Advanced => layout::TYPE_SHEETS[type_index].1,
            Tab::Terminal => "TabTerminal",
            Tab::Network => "TabNetwork",
            Tab::Bookmark => "TabSession",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Advanced => icon::WRENCH,
            Tab::Terminal => icon::TERMINAL_WINDOW,
            Tab::Network => icon::GLOBE_SIMPLE,
            Tab::Bookmark => icon::STAR,
        }
    }
}

/// What a button of a page asks for.
enum Press {
    SubDialog(&'static str),
    CloseSubDialog,
    RestoreDefaults(&'static str),
    JumpHosts,
    Icon,
    Shortcut,
    CustomizeSyntax,
    Browse { target: &'static str, folder: bool },
}

pub struct SessionEditor {
    original: Session,
    is_new: bool,
    type_index: usize,
    name: String,
    folder: String,
    values: BTreeMap<&'static str, String>,
    /// Values of fields whose enabling box was unchecked (restored when it
    /// is checked again).
    stash: BTreeMap<&'static str, String>,
    /// Shell / WSL "Execute the following commands at startup".
    startup: String,
    tab: Tab,
    /// Open sub-dialog (`FormExpertSSH`, `FormFont`, `FormColor`).
    sub_dialog: Option<&'static str>,
    error: Option<String>,
    /// "Bookmark icon" and its chooser.
    icon: Option<SessionIcon>,
    chooser: Option<IconChooser>,
    /// "SSH gateway (jump host)" dialog.
    jump_hosts: Option<JumpHostsDialog>,
    /// Named credentials offered next to "Username" (set by the app).
    pub credentials: Vec<String>,
    /// "Syntax highlighting" choices, built-in and user (set by the app).
    pub syntaxes: Vec<(usize, String)>,
    /// Saved macro names, for "Execute macro at session start".
    pub macros: Vec<String>,
    manage_credentials: bool,
}

impl SessionEditor {
    pub fn new_session() -> Self {
        let kind = SessionKind::default_for(0).expect("SSH is supported");
        Self::from_session(Session::new(String::new(), kind), true)
    }

    /// "New session" with another type preselected (snapshots, menus).
    pub fn new_of_type(type_index: usize) -> Self {
        let mut editor = Self::new_session();
        if SessionKind::default_for(type_index).is_some() {
            editor.set_type(type_index);
        }
        editor
    }

    /// Snapshots: an SSH session with two jump hosts, their dialog open.
    pub fn jump_hosts_preview() -> Self {
        let mut editor = Self::new_session();
        editor.tab = Tab::Network;
        let hop = |host: &str, user: &str, port: &str| JumpHost {
            host: host.into(),
            user: user.into(),
            port: port.into(),
            key: String::new(),
        };
        let hops = vec![
            hop("bastion.example", "ops", ""),
            hop("10.0.0.5", "", "2222"),
        ];
        editor.set_jump_hosts(&hops);
        editor.jump_hosts = Some(JumpHostsDialog::new(hops));
        editor
    }

    /// Snapshots: a tab or sub-dialog (`terminal`, `network`, `bookmark`,
    /// `expert_ssh`, `font`, `colors`) shown.
    pub fn preview(&mut self, what: &str) {
        match what {
            "terminal" => self.tab = Tab::Terminal,
            "network" => self.tab = Tab::Network,
            "bookmark" => self.tab = Tab::Bookmark,
            "expert_ssh" => self.sub_dialog = Some("FormExpertSSH"),
            "font" => {
                self.tab = Tab::Terminal;
                self.sub_dialog = Some("FormFont");
            }
            "colors" => {
                self.tab = Tab::Terminal;
                self.sub_dialog = Some("FormColor");
            }
            _ => {}
        }
    }

    pub fn edit(session: &Session) -> Self {
        Self::from_session(session.clone(), false)
    }

    fn from_session(session: Session, is_new: bool) -> Self {
        let mut editor = Self {
            type_index: session.kind.type_index(),
            name: session.name.clone(),
            folder: session.folder.clone(),
            icon: session.icon.clone(),
            original: session,
            is_new,
            values: BTreeMap::new(),
            stash: BTreeMap::new(),
            startup: String::new(),
            tab: Tab::Advanced,
            sub_dialog: None,
            error: None,
            chooser: None,
            jump_hosts: None,
            credentials: Vec::new(),
            syntaxes: Vec::new(),
            macros: Vec::new(),
            manage_credentials: false,
        };
        editor.load_values();
        editor.startup = editor
            .original
            .options
            .get(tc_core::session::STARTUP_COMMANDS)
            .cloned()
            .unwrap_or_default();
        editor
    }

    fn params(&self) -> impl Iterator<Item = &'static Param> + use<> {
        let t = self.type_index;
        let terminal: &'static [Param] = if TERMINAL_TYPES.contains(&t) {
            schema::terminal_params()
        } else {
            &[]
        };
        schema::type_params(t)
            .iter()
            .chain(schema::network_params(t))
            .chain(terminal)
            .chain(schema::bookmark_params())
    }

    fn param(&self, id: &str) -> Option<&'static Param> {
        self.params().find(|p| p.id == id)
    }

    /// Fill buffers from the session (typed field, stored option, or default).
    fn load_values(&mut self) {
        let same_type = self.original.kind.type_index() == self.type_index;
        let kind = if same_type {
            self.original.kind.clone()
        } else {
            SessionKind::default_for(self.type_index).expect("only supported types are selectable")
        };
        let mut values = BTreeMap::new();
        for p in self.params() {
            let from_kind = kind.typed_get(p.id);
            // Type-specific options (part 3) only carry over within the same type.
            let from_options = (same_type || !p.id.starts_with("3_"))
                .then(|| self.original.options.get(p.id).cloned())
                .flatten();
            let value = from_kind
                .or(from_options)
                .unwrap_or_else(|| p.default.to_string());
            values.insert(p.id, value);
        }
        self.values = values;
        self.stash.clear();
    }

    /// Switch session type, keeping values of free-text fields that mean the
    /// same thing in both types (same id and label, empty default: host,
    /// username). Fields with type-specific defaults (ports) are reset.
    fn set_type(&mut self, index: usize) {
        if index == self.type_index {
            return;
        }
        let before: Vec<(&'static str, &'static str, String)> = self
            .params()
            .filter_map(|p| Some((p.id, p.label, self.values.get(p.id)?.clone())))
            .collect();
        self.type_index = index;
        self.sub_dialog = None;
        self.load_values();
        for p in self.params().collect::<Vec<_>>() {
            if !p.default.is_empty() || p.widget != Widget::Text {
                continue;
            }
            if let Some((_, _, v)) = before
                .iter()
                .find(|(id, label, _)| *id == p.id && *label == p.label)
            {
                self.values.insert(p.id, v.clone());
            }
        }
    }

    /// Build the session from the buffers, validating typed fields.
    fn build(&self) -> Result<Session, String> {
        let mut kind =
            SessionKind::default_for(self.type_index).ok_or("unsupported session type")?;
        let typed = kind.typed_ids();
        let mut options: BTreeMap<String, String> = self
            .original
            .options
            .iter()
            .filter(|(k, _)| !k.starts_with("3_"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for p in self.params() {
            let value = self.values.get(p.id).map_or(p.default, String::as_str);
            if typed.contains(&p.id) {
                kind.typed_set(p.id, value)
                    .map_err(|e| format!("{}: {e}", p.label))?;
            } else if p.widget == Widget::Number
                && !value.is_empty()
                && value.parse::<u64>().is_err()
            {
                return Err(format!("{}: not a number", p.label));
            } else if value == p.default {
                options.remove(p.id);
            } else {
                options.insert(p.id.to_string(), value.to_string());
            }
        }
        if matches!(self.type_index, 10 | 14) && !self.startup.trim().is_empty() {
            options.insert(
                tc_core::session::STARTUP_COMMANDS.to_string(),
                self.startup.trim_end().to_string(),
            );
        } else {
            options.remove(tc_core::session::STARTUP_COMMANDS);
        }
        if kind.typed_ids().contains(&"3_2") && kind.target().is_none() {
            return Err("Remote host is required".into());
        }
        if matches!(kind, SessionKind::Serial { .. }) && kind.target().is_none() {
            return Err("Serial port is required".into());
        }
        let name = match self.name.trim() {
            "" => kind
                .target()
                .map_or_else(|| SESSION_TYPES[self.type_index].to_string(), String::from),
            n => n.to_string(),
        };
        Ok(Session {
            id: self.original.id,
            name,
            folder: self.folder.trim().trim_matches('/').to_string(),
            last_used: self.original.last_used,
            kind,
            options,
            icon: self.icon.clone(),
        })
    }

    /// Tabs of this session type.
    fn tabs(&self) -> Vec<Tab> {
        let t = self.type_index;
        let mut tabs = vec![Tab::Advanced];
        if TERMINAL_TYPES.contains(&t) {
            tabs.push(Tab::Terminal);
        }
        if !schema::network_params(t).is_empty() {
            tabs.push(Tab::Network);
        }
        tabs.push(Tab::Bookmark);
        tabs
    }

    /// Draw the dialog. Returns false when it should close.
    pub fn show(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) -> bool {
        if std::mem::take(&mut self.manage_credentials) {
            out.push(Action::ManageCredentials);
        }
        if let Some(chooser) = &mut self.chooser {
            match chooser.show(ctx) {
                Choice::Pending => {}
                Choice::Cancel => self.chooser = None,
                Choice::Chosen(icon) => {
                    self.icon = icon;
                    self.chooser = None;
                }
            }
        }
        if let Some(dialog) = &mut self.jump_hosts {
            match dialog.show(ctx) {
                jump_hosts_dialog::Outcome::Pending => {}
                jump_hosts_dialog::Outcome::Cancel => self.jump_hosts = None,
                jump_hosts_dialog::Outcome::Done(hops) => {
                    self.set_jump_hosts(&hops);
                    self.jump_hosts = None;
                }
            }
        }
        let mut presses = Vec::new();
        let mut open = true;
        let mut close = false;
        egui::Window::new("Session settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size([WIDTH, HEIGHT])
            .show(ctx, |ui| {
                compact(ui);
                let (area, _) = ui.allocate_exact_size(vec2(WIDTH, HEIGHT), egui::Sense::hover());
                let o = area.min;
                self.type_row(ui, Rect::from_min_size(o, vec2(WIDTH, 64.0)));
                let t = self.type_index;
                let (basic, _) = layout::TYPE_SHEETS[t];

                // "Basic … settings".
                if let Some(page) = layout::page(basic) {
                    let top = o + vec2(0.0, BASIC_Y);
                    header(ui, top, &[(page.caption, true)]);
                    let body = Rect::from_min_size(
                        top + vec2(0.0, HEADER_H),
                        vec2(WIDTH, BASIC_H - HEADER_H),
                    );
                    frame(ui, body);
                    self.draw_page(ui, page, body.min + vec2(4.0, 4.0), "", &mut presses);
                }

                // The tabs.
                let tabs = self.tabs();
                if !tabs.contains(&self.tab) {
                    self.tab = Tab::Advanced;
                }
                let top = o + vec2(0.0, TABS_Y);
                let labels: Vec<(String, Tab)> = tabs
                    .iter()
                    .filter_map(|tab| {
                        let page = layout::page(tab.sheet(t))?;
                        Some((format!("{} {}", tab.icon(), page.caption), *tab))
                    })
                    .collect();
                let mut x = top.x + 4.0;
                for (label, tab) in &labels {
                    let width = text_width(ui, label) + 18.0;
                    let rect =
                        Rect::from_min_size(Pos2::new(x, top.y), vec2(width, HEADER_H - 2.0));
                    if ui
                        .put(
                            rect,
                            egui::Button::selectable(self.tab == *tab, label.as_str()),
                        )
                        .clicked()
                    {
                        self.tab = *tab;
                    }
                    x += width + 2.0;
                }
                let body =
                    Rect::from_min_size(top + vec2(0.0, HEADER_H), vec2(WIDTH, TABS_H - HEADER_H));
                frame(ui, body);
                // The reference shows the session type's artwork on the right.
                ui.painter().text(
                    Pos2::new(body.min.x + 789.0, body.center().y),
                    egui::Align2::CENTER_CENTER,
                    TYPE_ICONS[t],
                    egui::FontId::proportional(72.0),
                    ui.visuals().weak_text_color().gamma_multiply(0.35),
                );
                if let Some(page) = layout::page(self.tab.sheet(t)) {
                    self.draw_page(ui, page, body.min + vec2(4.0, 4.0), "", &mut presses);
                }
                if self.tab == Tab::Bookmark {
                    self.bookmark_extras(ui, body.min + vec2(4.0, 4.0));
                }

                // OK / Cancel.
                let at = |x: f32| Rect::from_min_size(o + vec2(x, BUTTONS_Y), vec2(82.0, 32.0));
                if let Some(e) = &self.error {
                    ui.put(
                        Rect::from_min_size(o + vec2(8.0, BUTTONS_Y + 6.0), vec2(290.0, 20.0)),
                        egui::Label::new(egui::RichText::new(e).color(ui.visuals().error_fg_color)),
                    );
                }
                if ui.put(at(304.0), egui::Button::new("OK")).clicked() {
                    match self.build() {
                        Ok(session) => {
                            let id = session.id.to_string();
                            if self.is_new {
                                out.push(Action::Run(Command::SessionAdd {
                                    session: Box::new(session),
                                }));
                                out.push(Action::Run(Command::TabOpen { session: Some(id) }));
                            } else {
                                out.push(Action::Run(Command::SessionUpdate {
                                    session: Box::new(session),
                                }));
                            }
                            close = true;
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                if ui.put(at(454.0), egui::Button::new("Cancel")).clicked() {
                    close = true;
                }
            });
        self.sub_dialog_window(ctx, &mut presses);
        for press in presses {
            self.press(press, out);
        }
        open && !close
    }

    /// The open sub-dialog, drawn from its reference form.
    fn sub_dialog_window(&mut self, ctx: &egui::Context, presses: &mut Vec<Press>) {
        let Some(name) = self.sub_dialog else {
            return;
        };
        let Some(page) = layout::page(name) else {
            self.sub_dialog = None;
            return;
        };
        let mut open = true;
        let size = vec2(page.width as f32, page.height as f32);
        egui::Window::new(page.caption)
            .id(egui::Id::new(("session-sub-dialog", name)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size(size)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                compact(ui);
                let (area, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                // The reference's banner strip on the left.
                let strip = Rect::from_min_size(area.min, vec2(89.0, size.y));
                ui.painter()
                    .rect_filled(strip, 0.0, ui.visuals().faint_bg_color);
                ui.painter().text(
                    strip.center_top() + vec2(0.0, 40.0),
                    egui::Align2::CENTER_CENTER,
                    TYPE_ICONS[self.type_index],
                    egui::FontId::proportional(36.0),
                    ui.visuals().weak_text_color(),
                );
                self.draw_page(ui, page, area.min, name, presses);
            });
        if !open {
            self.sub_dialog = None;
        }
    }

    fn press(&mut self, press: Press, out: &mut Vec<Action>) {
        match press {
            Press::SubDialog(name) => self.sub_dialog = Some(name),
            Press::CloseSubDialog => self.sub_dialog = None,
            Press::RestoreDefaults(sub) => {
                let ids: Vec<&'static str> = layout::BINDINGS
                    .iter()
                    .filter(|b| b.sub_dialog == sub)
                    .filter_map(|b| layout::param_for(self.type_index, sub, b.control))
                    .collect();
                for id in ids {
                    if let Some(p) = self.param(id) {
                        self.values.insert(p.id, p.default.to_string());
                    }
                }
            }
            Press::JumpHosts => {
                if let Some(ids) = jump_hosts::ids_for(self.type_index) {
                    self.jump_hosts = Some(JumpHostsDialog::new(self.jump_hosts(ids)));
                }
            }
            Press::Icon => self.chooser = Some(IconChooser::new(self.icon.as_ref())),
            Press::Shortcut => {
                if let Ok(session) = self.build() {
                    out.push(Action::DesktopShortcut(session.name));
                }
            }
            Press::CustomizeSyntax => out.push(Action::CustomizeSyntax),
            Press::Browse { target, folder } => {
                let picked = if folder {
                    rfd::FileDialog::new().pick_folder()
                } else {
                    rfd::FileDialog::new().pick_file()
                };
                if let (Some(path), Some(id)) =
                    (picked, layout::param_for(self.type_index, "", target))
                {
                    self.values.insert(id, path.display().to_string());
                }
            }
        }
    }

    fn type_row(&mut self, ui: &mut egui::Ui, row: Rect) {
        for (index, name) in SESSION_TYPES.iter().enumerate() {
            let supported = SessionKind::default_for(index).is_some();
            let selected = index == self.type_index;
            let rect = Rect::from_min_size(
                row.min + vec2(4.0 + 56.0 * index as f32, 0.0),
                vec2(56.0, 64.0),
            );
            let sense = if supported {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            };
            let response = ui.allocate_rect(rect, sense);
            let v = ui.style().interact_selectable(&response, selected);
            if selected || (supported && response.hovered()) {
                ui.painter().rect_filled(rect, 4.0, v.weak_bg_fill);
            }
            let color = if supported {
                v.text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            ui.painter().text(
                rect.center_top() + vec2(0.0, 22.0),
                egui::Align2::CENTER_CENTER,
                TYPE_ICONS[index],
                egui::FontId::proportional(26.0),
                color,
            );
            ui.painter().text(
                rect.center_bottom() - vec2(0.0, 8.0),
                egui::Align2::CENTER_BOTTOM,
                *name,
                egui::FontId::proportional(12.0),
                color,
            );
            let response = if supported {
                response.on_hover_text(format!("{name} session"))
            } else {
                response.on_hover_text(format!("{name}: {NOT_YET}"))
            };
            if response.clicked() {
                self.set_type(index);
            }
        }
    }

    /// Bookmark tab: the TermCraft folder (the reference keeps sessions in
    /// folders through the sessions tree) and the session start macro.
    fn bookmark_extras(&mut self, ui: &mut egui::Ui, origin: Pos2) {
        let row = |x: f32, y: f32, w: f32| Rect::from_min_size(origin + vec2(x, y), vec2(w, 24.0));
        bound_label(ui, row(178.0, 210.0, 175.0), "Folder:");
        ui.put(
            row(178.0, 210.0, 175.0),
            egui::TextEdit::singleline(&mut self.folder).hint_text("e.g. Clients/ACME"),
        );
        if let Some(p) = self.param("4_38") {
            let rect = row(552.0, 210.0, 150.0);
            bound_label(ui, rect, "Execute macro at session start:");
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            self.combo_macro(&mut child, p, rect.width());
        }
    }

    /// Every control of `page`, at its reference position from `origin`.
    fn draw_page(
        &mut self,
        ui: &mut egui::Ui,
        page: &'static Page,
        origin: Pos2,
        sub: &'static str,
        presses: &mut Vec<Press>,
    ) {
        for c in page.controls {
            if sub.is_empty() && !self.visible(c) {
                continue;
            }
            let rect = Rect::from_min_size(
                origin + vec2(c.x as f32, c.y as f32),
                vec2(c.w as f32, c.h as f32),
            );
            match c.kind {
                Kind::Group => {
                    group(ui, rect, c.caption);
                    continue;
                }
                Kind::Label => {
                    if !c.caption.is_empty() && c.w < 600 {
                        ui.put(
                            rect,
                            egui::Label::new(egui::RichText::new(c.caption).small()),
                        );
                    }
                    continue;
                }
                Kind::Button => {
                    self.button(ui, c, rect, sub, presses);
                    continue;
                }
                Kind::List => {
                    list(ui, rect, c);
                    continue;
                }
                _ => {}
            }
            self.control(ui, c, rect, sub);
        }
    }

    /// Controls the reference shows or hides depending on another choice.
    fn visible(&self, c: &Ctl) -> bool {
        let choice = |id: &str| self.values.get(id).and_then(|v| v.parse::<usize>().ok());
        match self.type_index {
            // Telnet: the PuTTY options, or "Extra options" of the others.
            1 => {
                let putty = choice("3_6").unwrap_or(2) == 2;
                if c.panel == "sPanel5" {
                    putty
                } else if c.name == "sEdit4" {
                    !putty
                } else {
                    true
                }
            }
            // Browser: Internet Explorer or Edge (WebView2) options.
            11 => {
                let engine = choice("3_12").unwrap_or(0);
                if c.panel == "sPanel3" {
                    engine == 3
                } else if IE_CONTROLS.contains(&c.name) {
                    engine == 0
                } else {
                    true
                }
            }
            _ => true,
        }
    }

    /// Whether the control is enabled by its box (see [`ENABLES`]).
    fn enabled_by_box(&self, name: &str, sub: &str) -> bool {
        let Some((toggle, _)) = ENABLES.iter().find(|(_, field)| *field == name) else {
            return true;
        };
        match layout::param_for(self.type_index, sub, toggle) {
            Some(id) => self.values.get(id).is_some_and(|v| schema::is_true(v)),
            // A box of its own: checked while the field is set.
            None => layout::param_for(self.type_index, sub, name)
                .and_then(|id| self.values.get(id))
                .is_some_and(|v| !v.trim().is_empty()),
        }
    }

    fn button(
        &mut self,
        ui: &mut egui::Ui,
        c: &'static Ctl,
        rect: Rect,
        sub: &'static str,
        presses: &mut Vec<Press>,
    ) {
        let press = match (sub, c.name) {
            ("", "sBitBtn12") => Some(Press::SubDialog("FormExpertSSH")),
            ("", "sSpeedButton23") => Some(Press::SubDialog("FormFont")),
            ("", "sSpeedButton21") => Some(Press::SubDialog("FormColor")),
            ("", "sSpeedButton22") => Some(Press::CustomizeSyntax),
            ("", "BtnJumpHost") => Some(Press::JumpHosts),
            ("", "IconBookmark") => Some(Press::Icon),
            ("", "sBitBtn1") if !self.is_new => Some(Press::Shortcut),
            ("", "sSpeedButton15") => Some(Press::Browse {
                target: "sEdit15",
                folder: true,
            }),
            ("", "sSpeedButton16") => Some(Press::Browse {
                target: "sEdit15",
                folder: false,
            }),
            ("FormExpertSSH", "sBitBtn2") => Some(Press::RestoreDefaults("FormExpertSSH")),
            (s, "sBitBtn1") if !s.is_empty() => Some(Press::CloseSubDialog),
            _ => None,
        };
        if sub.is_empty() && CREDENTIAL_BUTTONS.contains(&c.name) {
            return self.credentials_button(ui, rect);
        }
        let caption = match c.name {
            "IconBookmark" => {
                let preview = Session {
                    icon: self.icon.clone(),
                    ..self.original.clone()
                };
                format!("{}  {}", session_icons::glyph(&preview), c.caption)
            }
            "sSpeedButton15" => icon::FOLDER_OPEN.to_string(),
            "sSpeedButton16" => icon::FILE.to_string(),
            "BtnJumpHost" => format!("{}  {}", icon::ARROWS_SPLIT, c.caption),
            _ => c.caption.to_string(),
        };
        if caption.is_empty() {
            return;
        }
        let enabled = press.is_some();
        let response = ui
            .add_enabled_ui(enabled, |ui| ui.put(rect, egui::Button::new(caption)))
            .inner;
        if c.name == "BtnJumpHost"
            && let Some(ids) = jump_hosts::ids_for(self.type_index)
        {
            let hops = self.jump_hosts(ids);
            let text = if hops.is_empty() {
                "no gateway".to_string()
            } else {
                jump_hosts::summary(&hops)
            };
            ui.put(
                Rect::from_min_size(
                    rect.left_bottom() + vec2(0.0, 4.0),
                    vec2(rect.width(), 20.0),
                ),
                egui::Label::new(egui::RichText::new(text).weak()),
            );
        }
        if !enabled {
            response.on_disabled_hover_text(NOT_YET);
            return;
        }
        if response.clicked()
            && let Some(press) = press
        {
            presses.push(press);
        }
    }

    /// The key button next to "Username": pick a named credential.
    fn credentials_button(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let user = ["3_4", "3_3", "3_7"]
            .into_iter()
            .find(|id| self.param(id).is_some_and(|p| p.label == "Username"));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        child
            .menu_button(icon::KEY, |ui| {
                for name in &self.credentials {
                    if ui.button(format!("[{name}]")).clicked() {
                        if let Some(id) = user {
                            self.values.insert(id, format!("[{name}]"));
                        }
                        ui.close();
                    }
                }
                if !self.credentials.is_empty() {
                    ui.separator();
                }
                if ui.button("Manage credentials…").clicked() {
                    self.manage_credentials = true;
                    ui.close();
                }
            })
            .response
            .on_hover_text("Create or manage credentials");
    }

    /// A parameter control (or the session name / tab color).
    fn control(&mut self, ui: &mut egui::Ui, c: &'static Ctl, rect: Rect, sub: &'static str) {
        let t = self.type_index;
        if sub.is_empty() && STARTUP_MEMOS.contains(&c.name) {
            ui.put(
                rect,
                egui::TextEdit::multiline(&mut self.startup)
                    .code_editor()
                    .hint_text("one command per line"),
            );
            return;
        }
        // Controls that are not plain parameters.
        match (sub, c.name) {
            ("", "sEdit1") => {
                bound_label(ui, rect, c.caption);
                ui.put(
                    rect,
                    egui::TextEdit::singleline(&mut self.name).hint_text("the remote host"),
                );
                return;
            }
            ("", "ChkCustomTabColor") => {
                let value = self.values.entry("7_1").or_insert_with(|| "-1".into());
                let mut on = value.trim() != "-1";
                if ui
                    .put(rect, egui::Checkbox::new(&mut on, c.caption))
                    .changed()
                {
                    *value = if on { "16744448".into() } else { "-1".into() };
                }
                return;
            }
            ("", "CustomTabColor") => {
                let value = self.values.entry("7_1").or_insert_with(|| "-1".into());
                if let Some(mut rgb) = deccol(value) {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    if egui::color_picker::color_edit_button_srgb(&mut child, &mut rgb).changed() {
                        *value = to_deccol(rgb);
                    }
                }
                return;
            }
            _ => {}
        }
        // A box that only enables its field (no parameter of its own).
        if layout::param_for(t, sub, c.name).is_none()
            && let Some((_, field)) = ENABLES.iter().find(|(toggle, _)| *toggle == c.name)
        {
            if let Some(id) = layout::param_for(t, sub, field) {
                let current = self.values.get(id).cloned().unwrap_or_default();
                let mut on = !current.trim().is_empty();
                let response = ui.put(rect, egui::Checkbox::new(&mut on, c.caption.trim()));
                if response.changed() {
                    if on {
                        let back = self.stash.remove(id).unwrap_or_default();
                        self.values.insert(id, back);
                    } else {
                        self.stash.insert(id, current);
                        self.values.insert(id, String::new());
                    }
                }
            }
            return;
        }
        let Some(p) = layout::param_for(t, sub, c.name).and_then(|id| self.param(id)) else {
            // A reference control TermCraft has no setting for: shown, greyed.
            placeholder(ui, c, rect);
            return;
        };
        let id = p.id;
        let enabled = honoured(t, id) && self.enabled_by_box(c.name, sub);
        // A field enabled by a box with the same caption shows it once.
        let same_as_box = ENABLES.iter().any(|(toggle, field)| {
            *field == c.name
                && layout::page_control(page_of(sub, t), toggle)
                    .is_some_and(|b| b.caption.trim() == c.caption.trim())
        });
        let labelled = !matches!(c.kind, Kind::Check | Kind::Color)
            && !c.caption.trim().is_empty()
            && !same_as_box;
        if labelled {
            bound_label(ui, rect, c.caption.trim());
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if !enabled {
            child.disable();
        }
        let response = self.widget(&mut child, c, p, rect, sub);
        if !honoured(t, id)
            && let Some(r) = response
        {
            r.on_disabled_hover_text(NOT_YET);
        }
    }

    /// The widget of parameter `p` in control `c`.
    fn widget(
        &mut self,
        ui: &mut egui::Ui,
        c: &'static Ctl,
        p: &'static Param,
        rect: Rect,
        sub: &'static str,
    ) -> Option<egui::Response> {
        let w = rect.width();
        // Xdmcp: two radio buttons for two yes / no parameters.
        if c.name == "sRadioButton1" || c.name == "sRadioButton2" {
            let other = if c.name == "sRadioButton1" {
                "sRadioButton2"
            } else {
                "sRadioButton1"
            };
            let on = self.values.get(p.id).is_some_and(|v| schema::is_true(v));
            let r = ui.radio(on, c.caption);
            if r.clicked() {
                self.values
                    .insert(p.id, schema::bool_value(true).to_string());
                if let Some(o) = layout::param_for(self.type_index, sub, other) {
                    self.values.insert(o, schema::bool_value(false).to_string());
                }
            }
            return Some(r);
        }
        if p.id == "4_1" {
            let value = self.values.entry(p.id).or_default();
            crate::term_fonts::combo(ui, "session-font", value);
            return None;
        }
        if p.id == "4_38" {
            self.combo_macro(ui, p, w);
            return None;
        }
        if p.id == "4_36" && !self.syntaxes.is_empty() {
            let value = self.values.entry(p.id).or_default();
            let current = value.parse::<usize>().unwrap_or(1);
            let selected = self
                .syntaxes
                .iter()
                .find(|(i, _)| *i == current)
                .map_or("", |(_, label)| label.as_str());
            egui::ComboBox::from_id_salt(("choice", p.id))
                .selected_text(selected)
                .width(w)
                .show_ui(ui, |ui| {
                    for (i, label) in &self.syntaxes {
                        if ui.selectable_label(current == *i, label).clicked() {
                            *value = i.to_string();
                        }
                    }
                });
            return None;
        }
        let value = self
            .values
            .entry(p.id)
            .or_insert_with(|| p.default.to_string());
        if p.label == "Distribution" {
            egui::ComboBox::from_id_salt(("choice", p.id))
                .selected_text(value.as_str())
                .width(w)
                .show_ui(ui, |ui| {
                    let installed = tc_import::wsl::distributions();
                    for name in
                        std::iter::once("Default").chain(installed.iter().map(String::as_str))
                    {
                        if ui.selectable_label(value == name, name).clicked() {
                            *value = name.to_string();
                        }
                    }
                });
            return None;
        }
        let widget = if p.label == "Terminal shell" {
            Widget::Choice(&SHELL_CHOICES)
        } else {
            p.widget
        };
        Some(match (c.kind, widget) {
            (_, Widget::Check) => {
                let mut on = schema::is_true(value);
                let caption = if c.caption.trim().is_empty() {
                    p.label
                } else {
                    c.caption.trim()
                };
                let r = ui.checkbox(&mut on, caption);
                if r.changed() {
                    *value = schema::bool_value(on).to_string();
                }
                r
            }
            (_, Widget::Choice(items)) => {
                let mut index = value
                    .parse::<usize>()
                    .unwrap_or(0)
                    .min(items.len().saturating_sub(1));
                let r = egui::ComboBox::from_id_salt(("choice", p.id))
                    .selected_text(items.get(index).copied().unwrap_or(""))
                    .width(w - 8.0)
                    .show_ui(ui, |ui| {
                        for (i, label) in items.iter().enumerate() {
                            ui.selectable_value(&mut index, i, *label);
                        }
                    })
                    .response;
                *value = index.to_string();
                r
            }
            (Kind::Color, _) => {
                let mut rgb = rgb_of(value);
                let r = egui::color_picker::color_edit_button_srgb(ui, &mut rgb);
                if r.changed() {
                    *value = format!("{},{},{}", rgb[0], rgb[1], rgb[2]);
                }
                if !c.caption.trim().is_empty() {
                    ui.label(c.caption.trim());
                }
                r
            }
            (Kind::File | Kind::Folder, _) | (_, Widget::File | Widget::Folder) => {
                let folder = c.kind == Kind::Folder || p.widget == Widget::Folder;
                // The reference writes an empty folder as "<>".
                let empty = if p.default == "<>" { "<>" } else { "" };
                let mut shown = if value == empty {
                    String::new()
                } else {
                    value.clone()
                };
                let r = ui.add(egui::TextEdit::singleline(&mut shown).desired_width(w - 30.0));
                if ui.small_button(icon::FOLDER_OPEN).clicked() {
                    let picked = if folder {
                        rfd::FileDialog::new().pick_folder()
                    } else {
                        rfd::FileDialog::new().pick_file()
                    };
                    if let Some(path) = picked {
                        shown = path.display().to_string();
                    }
                }
                *value = if shown.trim().is_empty() {
                    empty.to_string()
                } else {
                    shown
                };
                r
            }
            // The reference uses `-1` as "unset" for some text values.
            (_, Widget::Text) if p.default == "-1" => {
                let mut shown = if value == "-1" {
                    String::new()
                } else {
                    value.clone()
                };
                let r = ui.add(egui::TextEdit::singleline(&mut shown).desired_width(w));
                if r.changed() {
                    *value = if shown.is_empty() { "-1".into() } else { shown };
                }
                r
            }
            _ => ui.add(egui::TextEdit::singleline(value).desired_width(w)),
        })
    }

    fn combo_macro(&mut self, ui: &mut egui::Ui, p: &'static Param, width: f32) {
        let value = self.values.entry(p.id).or_default();
        let none = crate::app::NO_MACRO;
        let shown = if value.trim().is_empty() {
            none.to_string()
        } else {
            value.clone()
        };
        egui::ComboBox::from_id_salt(("choice", p.id))
            .selected_text(shown.as_str())
            .width(width)
            .show_ui(ui, |ui| {
                for name in std::iter::once(none).chain(self.macros.iter().map(String::as_str)) {
                    if ui.selectable_label(shown == name, name).clicked() {
                        *value = name.to_string();
                    }
                }
            });
    }

    /// The current jump hosts, from the edited values.
    fn jump_hosts(&self, ids: GatewayIds) -> Vec<JumpHost> {
        let options = [ids.host, ids.port, ids.user, ids.key]
            .iter()
            .filter_map(|id| Some((id.to_string(), self.values.get(id)?.clone())))
            .collect();
        jump_hosts::parse(&options, ids)
    }

    fn set_jump_hosts(&mut self, hops: &[JumpHost]) {
        let Some(ids) = jump_hosts::ids_for(self.type_index) else {
            return;
        };
        let mut options = BTreeMap::new();
        jump_hosts::store(&mut options, ids, hops);
        for id in [ids.host, ids.port, ids.user, ids.key] {
            if let Some(v) = options.remove(id) {
                self.values.insert(id, v);
            }
        }
    }
}

/// The reference dialogs' small font, so captions fit their controls.
fn compact(ui: &mut egui::Ui) {
    use egui::{FontId, TextStyle};
    let style = ui.style_mut();
    for (text_style, size) in [
        (TextStyle::Body, 12.0),
        (TextStyle::Button, 12.0),
        (TextStyle::Small, 10.0),
    ] {
        style
            .text_styles
            .insert(text_style, FontId::proportional(size));
    }
    style.spacing.interact_size.y = 20.0;
}

fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.fonts_mut(|f| {
        f.layout_no_wrap(
            text.to_string(),
            egui::TextStyle::Button.resolve(ui.style()),
            egui::Color32::WHITE,
        )
        .size()
        .x
    })
}

/// A page control's tab header row.
fn header(ui: &mut egui::Ui, top: Pos2, tabs: &[(&str, bool)]) {
    let mut x = top.x + 4.0;
    for (label, selected) in tabs {
        let width = text_width(ui, label) + 18.0;
        let rect = Rect::from_min_size(Pos2::new(x, top.y), vec2(width, HEADER_H - 2.0));
        ui.put(rect, egui::Button::selectable(*selected, *label));
        x += width + 2.0;
    }
}

fn frame(ui: &egui::Ui, rect: Rect) {
    let v = ui.visuals();
    ui.painter().rect(
        rect,
        2.0,
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
}

/// A reference control without a TermCraft setting, greyed out.
fn placeholder(ui: &mut egui::Ui, c: &Ctl, rect: Rect) {
    let caption = c.caption.trim();
    let response = ui
        .add_enabled_ui(false, |ui| match c.kind {
            Kind::Check => ui.put(rect, egui::Checkbox::new(&mut false, caption)),
            Kind::Combo | Kind::Edit | Kind::Spin | Kind::File | Kind::Folder | Kind::Memo => {
                if !caption.is_empty() {
                    bound_label(ui, rect, caption);
                }
                ui.put(rect, egui::TextEdit::singleline(&mut String::new()))
            }
            _ => ui.allocate_rect(rect, egui::Sense::hover()),
        })
        .inner;
    response.on_disabled_hover_text(NOT_YET);
}

/// A list box (the SSH algorithm lists): caption above, its items in
/// order. TermCraft uses its own secure algorithm choice, so it is shown
/// only.
fn list(ui: &mut egui::Ui, rect: Rect, c: &Ctl) {
    let v = ui.visuals().clone();
    ui.painter().text(
        rect.left_top() - vec2(0.0, 4.0),
        egui::Align2::LEFT_BOTTOM,
        c.caption.trim(),
        egui::TextStyle::Body.resolve(ui.style()),
        v.weak_text_color(),
    );
    ui.painter().rect(
        rect,
        2.0,
        v.extreme_bg_color,
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(3.0)));
    child.disable();
    egui::ScrollArea::vertical()
        .id_salt(("list", c.name))
        .max_height(rect.height() - 6.0)
        .show(&mut child, |ui| {
            for item in c.items {
                if item.starts_with('=') {
                    ui.separator();
                } else {
                    ui.label(egui::RichText::new(*item).small());
                }
            }
        });
    let _ = ui
        .interact(
            rect,
            egui::Id::new(("list-hover", c.name)),
            egui::Sense::hover(),
        )
        .on_hover_text(NOT_YET);
}

/// A group box: a frame with its caption on the top border.
fn group(ui: &egui::Ui, rect: Rect, caption: &str) {
    let v = ui.visuals();
    let stroke = egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color);
    let frame = Rect::from_min_max(rect.min + vec2(0.0, 8.0), rect.max);
    ui.painter()
        .rect_stroke(frame, 3.0, stroke, egui::StrokeKind::Inside);
    if !caption.is_empty() {
        let galley = ui.painter().layout_no_wrap(
            caption.to_string(),
            egui::TextStyle::Small.resolve(ui.style()),
            v.text_color(),
        );
        let at = rect.min + vec2(10.0, 1.0);
        let back = Rect::from_min_size(at - vec2(3.0, 0.0), galley.size() + vec2(6.0, 0.0));
        ui.painter().rect_filled(back, 0.0, v.panel_fill);
        ui.painter().galley(at, galley, v.text_color());
    }
}

/// The reference's bound label: left of the control, right-aligned.
fn bound_label(ui: &egui::Ui, rect: Rect, caption: &str) {
    ui.painter().text(
        Pos2::new(rect.left() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        caption.trim(),
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
    );
}

/// The page a control of sub-dialog `sub` (or of the main dialog) is on.
fn page_of(sub: &str, type_index: usize) -> &'static str {
    if sub.is_empty() {
        layout::TYPE_SHEETS[type_index].1
    } else {
        layout::page(sub).map_or("", |p| p.name)
    }
}

/// `r,g,b` (color parameters), black when unset.
fn rgb_of(value: &str) -> [u8; 3] {
    let mut parts = value
        .split(',')
        .map(|p| p.trim().parse::<u8>().unwrap_or(0));
    [
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    ]
}

/// Delphi color (`B*65536 + G*256 + R`); `None` for `-1` (unset).
fn deccol(value: &str) -> Option<[u8; 3]> {
    let v: u32 = value.trim().parse().ok()?;
    (v <= 0x00ff_ffff).then_some([
        (v & 0xff) as u8,
        (v >> 8 & 0xff) as u8,
        (v >> 16 & 0xff) as u8,
    ])
}

fn to_deccol([r, g, b]: [u8; 3]) -> String {
    (u32::from(b) << 16 | u32::from(g) << 8 | u32::from(r)).to_string()
}

/// Session parameters shown but not applied yet (session type index, id).
const NOT_APPLIED: &[(Option<usize>, &str)] = &[
    // SSH: protocol version, algorithm lists, certificates, locales.
    (Some(0), "3_24"),
    (Some(0), "3_28"),
    (Some(0), "3_29"),
    (Some(0), "3_30"),
    (Some(0), "3_31"),
    (Some(0), "3_33"),
    (Some(0), "3_36"),
    (Some(0), "3_37"),
    (Some(0), "3_38"),
    (Some(0), "3_39"),
    // RDP (external client): zoom, smart cards, authentication level,
    // expert settings, WebAuthn.
    (Some(4), "3_28"),
    (Some(4), "3_30"),
    (Some(4), "3_31"),
    (Some(4), "3_32"),
    (Some(4), "3_33"),
    // Every type: custom macro, font charset / antialiasing / ligatures,
    // advanced terminal settings.
    (None, "4_39"),
    (None, "4_41"),
    (None, "4_42"),
    (None, "4_43"),
    (None, "4_44"),
];

fn honoured(type_index: usize, id: &str) -> bool {
    !NOT_APPLIED
        .iter()
        .any(|(t, i)| *i == id && t.is_none_or(|t| t == type_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unapplied_parameters_are_marked() {
        assert!(!honoured(0, "3_31"), "SSH ciphers");
        assert!(honoured(1, "3_31"), "same id, other type");
        assert!(!honoured(7, "4_44"), "every type");
        assert!(honoured(0, "3_32"), "trivial authentication is applied");
        // Every listed id exists in the schema for its type.
        for (t, id) in NOT_APPLIED {
            let types: Vec<usize> = t.map_or_else(|| (0..15).collect(), |t| vec![t]);
            assert!(
                types.iter().any(|&t| {
                    schema::type_params(t)
                        .iter()
                        .chain(schema::network_params(t))
                        .chain(schema::terminal_params())
                        .chain(schema::bookmark_params())
                        .any(|p| p.id == *id)
                }),
                "{id}"
            );
        }
    }

    #[test]
    fn shell_startup_commands_are_saved() {
        let mut editor = SessionEditor::new_of_type(10);
        editor.startup = "cd /tmp
ls -l
".into();
        let session = editor.build().unwrap();
        assert_eq!(
            session.options.get(tc_core::session::STARTUP_COMMANDS).map(String::as_str),
            Some("cd /tmp
ls -l")
        );
        // Other session types have no such field.
        let mut ssh = SessionEditor::new_of_type(0);
        ssh.values.insert("3_2", "h".into());
        ssh.startup = "x".into();
        let session = ssh.build().unwrap();
        assert!(!session.options.contains_key(tc_core::session::STARTUP_COMMANDS));
    }

    #[test]
    fn delphi_colors() {
        assert_eq!(deccol("-1"), None);
        assert_eq!(deccol("255"), Some([255, 0, 0]));
        assert_eq!(to_deccol([0, 0, 255]), "16711680");
        assert_eq!(deccol(&to_deccol([1, 2, 3])), Some([1, 2, 3]));
        assert_eq!(rgb_of("1, 2,3"), [1, 2, 3]);
    }

    /// Every parameter of every session type has a control in the
    /// reference layout, or is drawn by TermCraft itself.
    #[test]
    fn every_parameter_has_a_place() {
        // Drawn by TermCraft (icon chooser, jump hosts dialog, macro list),
        // or with no control in the reference either.
        const OWN: &[&str] = &["2_1", "4_38", "4_39", "4_44"];
        let mut missing = Vec::new();
        for t in 0..15 {
            let editor = SessionEditor::new_of_type(t);
            if editor.type_index != t {
                continue;
            }
            let gateway = jump_hosts::ids_for(t);
            for p in editor.params() {
                // RDP "Expert settings": its own reference dialog, not applied.
                if OWN.contains(&p.id)
                    || (t == 4 && p.id == "3_32")
                    || gateway.is_some_and(|g| [g.host, g.port, g.user, g.key].contains(&p.id))
                {
                    continue;
                }
                let placed = layout::BINDINGS.iter().any(|b| {
                    b.param == p.id && layout::param_for(t, b.sub_dialog, b.control) == Some(p.id)
                });
                if !placed {
                    missing.push(format!("type {t}: {} {}", p.id, p.label));
                }
            }
        }
        assert!(missing.is_empty(), "{missing:#?}");
    }
}
