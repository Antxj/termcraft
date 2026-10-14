//! "Session settings" dialog (reference `TFORMSESSIONEDIT`, 864×545): a row
//! of session-type buttons, a "Basic … settings" block, then tabs
//! "Advanced … / Terminal / Network / Bookmark settings", then OK / Cancel.
//!
//! Every field is drawn from the generated parameter schema; values are kept
//! as strings in the schema encoding and written back on OK.

use super::jump_hosts_dialog::{self, JumpHostsDialog};
use super::session_icons::{self, Choice, IconChooser};
use super::{Action, NOT_YET, icon};
use std::collections::BTreeMap;
use tc_commands::Command;
use tc_core::jump_hosts::{self, GatewayIds, JumpHost};
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Advanced,
    Terminal,
    Network,
    Bookmark,
}

pub struct SessionEditor {
    original: Session,
    is_new: bool,
    type_index: usize,
    name: String,
    folder: String,
    values: BTreeMap<&'static str, String>,
    tab: Tab,
    error: Option<String>,
    /// "Bookmark icon" and its chooser.
    icon: Option<SessionIcon>,
    chooser: Option<IconChooser>,
    /// "SSH gateway (jump host)" dialog.
    jump_hosts: Option<JumpHostsDialog>,
    /// Named credentials offered next to "Username" (set by the app).
    pub credentials: Vec<String>,
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
            tab: Tab::Advanced,
            error: None,
            chooser: None,
            jump_hosts: None,
            credentials: Vec::new(),
            manage_credentials: false,
        };
        editor.load_values();
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
        let mut open = true;
        let mut close = false;
        egui::Window::new("Session settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(864.0)
            .show(ctx, |ui| {
                self.type_row(ui);
                ui.add_space(6.0);
                let ty = SESSION_TYPES[self.type_index];
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.strong(format!("Basic {ty} settings"));
                    let basic: Vec<&Param> = schema::type_params(self.type_index)
                        .iter()
                        .filter(|p| p.basic)
                        .collect();
                    ui.horizontal_wrapped(|ui| {
                        for p in basic {
                            self.field(ui, p, true);
                        }
                    });
                });
                ui.add_space(6.0);
                self.tabs(ui, ty);
                ui.add_space(6.0);
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 180.0) / 2.0);
                    if ui
                        .add(egui::Button::new("OK").min_size(egui::vec2(82.0, 30.0)))
                        .clicked()
                    {
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
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(82.0, 30.0)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        open && !close
    }

    fn type_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (index, name) in SESSION_TYPES.iter().enumerate() {
                let supported = SessionKind::default_for(index).is_some();
                let selected = index == self.type_index;
                let size = egui::vec2(56.0, 64.0);
                let sense = if supported {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                };
                let (rect, response) = ui.allocate_exact_size(size, sense);
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
                    rect.center_top() + egui::vec2(0.0, 22.0),
                    egui::Align2::CENTER_CENTER,
                    TYPE_ICONS[index],
                    egui::FontId::proportional(26.0),
                    color,
                );
                ui.painter().text(
                    rect.center_bottom() - egui::vec2(0.0, 8.0),
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
        });
    }

    fn tabs(&mut self, ui: &mut egui::Ui, ty: &str) {
        let t = self.type_index;
        let mut tabs = vec![(
            Tab::Advanced,
            format!("{} Advanced {ty} settings", icon::WRENCH),
        )];
        if TERMINAL_TYPES.contains(&t) {
            tabs.push((
                Tab::Terminal,
                format!("{} Terminal settings", icon::TERMINAL_WINDOW),
            ));
        }
        if !schema::network_params(t).is_empty() {
            tabs.push((
                Tab::Network,
                format!("{} Network settings", icon::GLOBE_SIMPLE),
            ));
        }
        tabs.push((Tab::Bookmark, format!("{} Bookmark settings", icon::STAR)));
        if !tabs.iter().any(|(tab, _)| *tab == self.tab) {
            self.tab = Tab::Advanced;
        }
        ui.horizontal(|ui| {
            for (tab, label) in &tabs {
                ui.selectable_value(&mut self.tab, *tab, label);
            }
        });
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(240.0);
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let params: Vec<&'static Param> = match self.tab {
                        Tab::Advanced => {
                            schema::type_params(t).iter().filter(|p| !p.basic).collect()
                        }
                        Tab::Terminal => schema::terminal_params().iter().collect(),
                        Tab::Network => {
                            let gateway = jump_hosts::ids_for(t);
                            if let Some(ids) = gateway {
                                self.jump_host_button(ui, ids);
                            }
                            schema::network_params(t)
                                .iter()
                                .filter(|p| gateway.is_none_or(|ids| !is_gateway_id(ids, p.id)))
                                .collect()
                        }
                        Tab::Bookmark => {
                            self.bookmark_header(ui);
                            // 2_1 is the reference icon number; the
                            // "Bookmark icon" chooser above replaces it.
                            schema::bookmark_params()
                                .iter()
                                .filter(|p| p.id != "2_1")
                                .collect()
                        }
                    };
                    if params.is_empty() {
                        ui.weak("No settings for this session type.");
                    }
                    egui::Grid::new(("params", t, self.tab as u8))
                        .num_columns(4)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            for (i, p) in params.iter().enumerate() {
                                self.field(ui, p, false);
                                if i % 2 == 1 {
                                    ui.end_row();
                                }
                            }
                        });
                });
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

    /// Reference `BtnJumpHost`: opens the jump hosts list.
    fn jump_host_button(&mut self, ui: &mut egui::Ui, ids: GatewayIds) {
        let hops = self.jump_hosts(ids);
        ui.horizontal(|ui| {
            let button =
                egui::Button::new(format!("{}  SSH gateway (jump host)", icon::ARROWS_SPLIT))
                    .min_size(egui::vec2(433.0, 36.0));
            if ui
                .add(button)
                .on_hover_text(
                    "Use a gateway SSH server in order to reach your end server: TermCraft \
                     opens a SSH connection to the gateway, then connects to the end server \
                     through it. Several gateways are used in order.",
                )
                .clicked()
            {
                self.jump_hosts = Some(JumpHostsDialog::new(hops.clone()));
            }
            if hops.is_empty() {
                ui.weak("no gateway");
            } else {
                ui.label(jump_hosts::summary(&hops));
            }
        });
        ui.add_space(6.0);
    }

    fn bookmark_header(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("bookmark-name")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Session name");
                ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .hint_text("defaults to the remote host"),
                );
                ui.end_row();
                ui.label("Folder");
                ui.add(egui::TextEdit::singleline(&mut self.folder).hint_text("e.g. Clients/ACME"));
                ui.end_row();
                ui.label("Bookmark icon");
                let preview = Session {
                    icon: self.icon.clone(),
                    ..self.original.clone()
                };
                let glyph = egui::RichText::new(session_icons::glyph(&preview)).size(20.0);
                let glyph = match session_icons::tint(&preview) {
                    Some(c) => glyph.color(c),
                    None => glyph,
                };
                if ui
                    .button(glyph)
                    .on_hover_text("Choose the icon shown for this session")
                    .clicked()
                {
                    self.chooser = Some(IconChooser::new(self.icon.as_ref()));
                }
                ui.end_row();
            });
        ui.separator();
    }

    /// One labelled control. In a grid (`inline == false`) it takes two cells.
    fn field(&mut self, ui: &mut egui::Ui, p: &'static Param, inline: bool) {
        let value = self
            .values
            .entry(p.id)
            .or_insert_with(|| p.default.to_string());
        if p.widget == Widget::Check {
            let mut on = schema::is_true(value);
            if ui.checkbox(&mut on, p.label).changed() {
                *value = schema::bool_value(on).to_string();
            }
            if !inline {
                ui.label("");
            }
            return;
        }
        ui.label(p.label);
        // WSL "Distribution": the installed distributions, by name.
        if p.label == "Distribution" {
            egui::ComboBox::from_id_salt(("choice", p.id))
                .selected_text(value.as_str())
                .width(170.0)
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
            return;
        }
        // The reference fills this combo at runtime, so the schema has no items.
        let widget = if p.label == "Terminal shell" {
            Widget::Choice(&SHELL_CHOICES)
        } else {
            p.widget
        };
        match widget {
            Widget::Choice(items) => {
                let mut index = value
                    .parse::<usize>()
                    .unwrap_or(0)
                    .min(items.len().saturating_sub(1));
                egui::ComboBox::from_id_salt(("choice", p.id))
                    .selected_text(items.get(index).copied().unwrap_or(""))
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for (i, label) in items.iter().enumerate() {
                            ui.selectable_value(&mut index, i, *label);
                        }
                    });
                *value = index.to_string();
            }
            Widget::Number => {
                ui.add(egui::TextEdit::singleline(value).desired_width(70.0));
            }
            Widget::File | Widget::Folder => {
                let hint = if p.widget == Widget::File {
                    "path to file"
                } else {
                    "path to folder"
                };
                ui.add(
                    egui::TextEdit::singleline(value)
                        .hint_text(hint)
                        .desired_width(170.0),
                );
            }
            // The reference uses `-1` as "unset" for some text values
            // (comments, custom tab color): show it as empty.
            Widget::Text if p.default == "-1" => {
                let mut shown = if value == "-1" {
                    String::new()
                } else {
                    value.clone()
                };
                if ui
                    .add(egui::TextEdit::singleline(&mut shown).desired_width(170.0))
                    .changed()
                {
                    *value = if shown.is_empty() { "-1".into() } else { shown };
                }
            }
            // WSL has no remote login, so no credentials there.
            Widget::Text if p.label == "Username" && self.type_index != 14 => {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(value).desired_width(140.0));
                    ui.menu_button(icon::KEY, |ui| {
                        for name in &self.credentials {
                            if ui.button(format!("[{name}]")).clicked() {
                                *value = format!("[{name}]");
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
                });
            }
            Widget::Text | Widget::Check => {
                ui.add(egui::TextEdit::singleline(value).desired_width(170.0));
            }
        }
    }
}

fn is_gateway_id(ids: GatewayIds, id: &str) -> bool {
    [ids.host, ids.port, ids.user, ids.key].contains(&id)
}
