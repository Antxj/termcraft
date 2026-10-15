//! Settings › Configuration (reference `TFORMPARAMS`, 804×522): the tabs
//! General, Terminal, X11, SSH, Display, Toolbar, Misc and Security drawn
//! from the reference layout, each control at its place and bound to its
//! TermCraft setting; reference options TermCraft does not have are greyed
//! out. "Default font settings" / "Default color settings" open the font
//! and color dialogs for the defaults. A last tab, "TermCraft", holds what
//! the reference has elsewhere: data files, links, known hosts, updates.

use super::form::{HEADER_H, bound_label, compact, frame, group, list, placeholder, text_width};
use super::{Action, NOT_YET, icon};
use egui::{Pos2, Rect, vec2};
use tc_core::layout::{self, Ctl, Kind};
use tc_core::settings::{
    HomeLocation, LocalShell, PopupPosition, TerminalColors, Theme, ToolbarStyle, UiSize,
    UpdateChannel,
};
use tc_core::{Paths, Settings};

const WIDTH: f32 = 804.0;
const HEIGHT: f32 = 522.0;
/// The reference page control.
const PAGES: Rect = Rect {
    min: Pos2::new(17.0, 17.0),
    max: Pos2::new(17.0 + 656.0, 17.0 + 432.0),
};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigTab {
    #[default]
    General,
    Terminal,
    X11,
    Ssh,
    Display,
    Toolbar,
    Misc,
    Security,
    TermCraft,
}

impl ConfigTab {
    const ALL: [ConfigTab; 9] = [
        ConfigTab::General,
        ConfigTab::Terminal,
        ConfigTab::X11,
        ConfigTab::Ssh,
        ConfigTab::Display,
        ConfigTab::Toolbar,
        ConfigTab::Misc,
        ConfigTab::Security,
        ConfigTab::TermCraft,
    ];

    /// By its name in lower case (`ssh`, `display` …), for snapshots.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.caption().eq_ignore_ascii_case(name))
    }

    /// The reference tab sheet (`None` for TermCraft's own tab).
    fn sheet(self) -> Option<&'static str> {
        Some(match self {
            ConfigTab::General => "Params.OngletMisc",
            ConfigTab::Terminal => "Params.OngletTerminal",
            ConfigTab::X11 => "Params.OngletX11",
            ConfigTab::Ssh => "Params.OngletSSH",
            ConfigTab::Display => "Params.OngletDisplay",
            ConfigTab::Toolbar => "Params.sTabSheet1",
            ConfigTab::Misc => "Params.sTabSheet2",
            ConfigTab::Security => "Params.OngletCustom",
            ConfigTab::TermCraft => return None,
        })
    }

    fn caption(self) -> &'static str {
        match self.sheet().and_then(layout::page) {
            Some(p) => p.caption,
            None => "TermCraft",
        }
    }
}

/// The window's state between frames.
#[derive(Default)]
pub struct ConfigState {
    pub tab: ConfigTab,
    /// "Default font settings" (`FormFont`) / "Default color settings"
    /// (`FormColor`).
    sub_dialog: Option<&'static str>,
    /// "Paste warning settings".
    paste_settings: bool,
    /// Selected key of "Load following keys at MobAgent startup".
    agent_key: Option<usize>,
    /// Selected button in "Available buttons" / "Toolbar buttons".
    available: Option<&'static str>,
    on_toolbar: Option<&'static str>,
}

/// Returns true when a setting changed (the caller persists it).
pub fn show(
    ctx: &egui::Context,
    open: &mut bool,
    state: &mut ConfigState,
    settings: &mut Settings,
    paths: &Paths,
    out: &mut Vec<Action>,
) -> bool {
    let before = settings.clone();
    let mut close = false;
    egui::Window::new("TermCraft Configuration")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .fixed_size([WIDTH, HEIGHT])
        .show(ctx, |ui| {
            compact(ui);
            let (area, _) = ui.allocate_exact_size(vec2(WIDTH, HEIGHT), egui::Sense::hover());
            let o = area.min.to_vec2();
            // The reference's artwork on the right.
            let strip = Rect::from_min_size(area.min + vec2(672.0, 0.0), vec2(132.0, HEIGHT));
            ui.painter()
                .rect_filled(strip, 0.0, ui.visuals().faint_bg_color);
            ui.painter().text(
                strip.center_top() + vec2(0.0, 70.0),
                egui::Align2::CENTER_CENTER,
                icon::GEAR,
                egui::FontId::proportional(56.0),
                ui.visuals().weak_text_color(),
            );
            // Tab headers.
            let mut x = PAGES.min.x + o.x + 4.0;
            for tab in ConfigTab::ALL {
                let caption = tab.caption();
                let width = text_width(ui, caption) + 14.0;
                let rect = Rect::from_min_size(
                    Pos2::new(x, PAGES.min.y + o.y),
                    vec2(width, HEADER_H - 2.0),
                );
                if ui
                    .put(rect, egui::Button::selectable(state.tab == tab, caption))
                    .clicked()
                {
                    state.tab = tab;
                }
                x += width + 2.0;
            }
            let body = Rect::from_min_max(PAGES.min + o + vec2(0.0, HEADER_H), PAGES.max + o);
            frame(ui, body);
            let origin = body.min + vec2(4.0, 4.0);
            match state.tab.sheet().and_then(layout::page) {
                Some(page) => {
                    for c in page.controls {
                        let rect = Rect::from_min_size(
                            origin + vec2(c.x as f32, c.y as f32),
                            vec2(c.w as f32, c.h as f32),
                        );
                        control(ui, page.name, c, rect, settings, state, out);
                    }
                }
                None => termcraft_tab(
                    ui,
                    Rect::from_min_size(origin, body.size()),
                    settings,
                    paths,
                    out,
                ),
            }
            let ok = Rect::from_min_size(area.min + vec2(285.0, 468.0), vec2(121.0, 34.0));
            if ui.put(ok, egui::Button::new("OK")).clicked() {
                close = true;
            }
        });
    sub_dialog(ctx, state, settings);
    paste_settings(ctx, state, settings);
    if close {
        *open = false;
    }
    *settings != before
}

/// One reference control, bound to its setting.
fn control(
    ui: &mut egui::Ui,
    page: &str,
    c: &'static Ctl,
    rect: Rect,
    s: &mut Settings,
    state: &mut ConfigState,
    out: &mut Vec<Action>,
) {
    // A label above the control is drawn here; the control gets none.
    let caption = if c.label_above && c.kind != Kind::List {
        super::form::label_over(ui, rect, c.caption);
        ""
    } else {
        c.caption.trim()
    };
    let check = |ui: &mut egui::Ui, value: &mut bool| {
        ui.put(rect, egui::Checkbox::new(value, caption));
    };
    let button = |ui: &mut egui::Ui, label: &str| ui.put(rect, egui::Button::new(label)).clicked();
    let page = page.trim_start_matches("Params.");
    match (page, c.name) {
        // ---------------------------------------------------------- General
        ("OngletMisc", "sEdit1") => {
            bound_label(ui, rect, caption);
            let g = &mut s.general;
            let mut text = match g.home_location {
                HomeLocation::Custom => g.home_dir.clone(),
                other => other.label().to_string(),
            };
            if ui
                .put(rect, egui::TextEdit::singleline(&mut text))
                .on_hover_text("HOME of the local Bash / Zsh terminals")
                .changed()
            {
                match HomeLocation::ALL.iter().find(|l| l.label() == text.trim()) {
                    Some(l) => g.home_location = *l,
                    None => {
                        g.home_location = HomeLocation::Custom;
                        g.home_dir = text;
                    }
                }
            }
        }
        ("OngletMisc", "sBitBtn11") => {
            if ui
                .put(rect, egui::Button::new(icon::HOURGLASS))
                .on_hover_text("Do not use a persistent home folder (use a temporary one)")
                .clicked()
            {
                s.general.home_location = HomeLocation::Temp;
            }
        }
        ("OngletMisc", "sSpeedButton9") => {
            if ui
                .put(rect, egui::Button::new(icon::FOLDER_OPEN))
                .on_hover_text("Browse home folder")
                .clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                s.general.home_location = HomeLocation::Custom;
                s.general.home_dir = dir.display().to_string();
            }
        }
        ("OngletMisc", "sCheckBox52") => check(ui, &mut s.general.auto_backup),
        ("OngletMisc", "sBitBtn12") => {
            if button(ui, caption) {
                out.push(Action::KeyboardShortcuts);
            }
        }
        ("OngletMisc", "sBitBtn14") => {
            if button(ui, caption) {
                out.push(Action::PasswordsSettings);
            }
        }
        ("OngletMisc", "sSpeedButton14") => {
            if button(ui, caption) {
                out.push(Action::ManageShared);
            }
        }
        // --------------------------------------------------------- Terminal
        ("OngletTerminal", "sSpeedButton23") => {
            if button(ui, caption) {
                state.sub_dialog = Some("FormFont");
            }
        }
        ("OngletTerminal", "sSpeedButton21") => {
            if button(ui, caption) {
                state.sub_dialog = Some("FormColor");
            }
        }
        ("OngletTerminal", "sBitBtn2") => {
            if button(ui, caption) {
                out.push(Action::ExpertTerminal);
            }
        }
        ("OngletTerminal", "sSpeedButton13") => {
            if button(ui, caption) {
                out.push(Action::CustomizeSyntax);
            }
        }
        ("OngletTerminal", "BtnPasteOpts") => {
            if button(ui, caption) {
                state.paste_settings = true;
            }
        }
        ("OngletTerminal", "sComboBox32") => {
            bound_label(ui, rect, caption);
            let t = &mut s.terminal;
            let choices = super::syntax_window::choices(&t.custom_syntaxes);
            let selected = choices
                .iter()
                .find(|(i, _)| *i == t.syntax_highlighting)
                .map_or("", |(_, label)| label.as_str())
                .to_string();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            egui::ComboBox::from_id_salt("cfg-syntax")
                .selected_text(selected)
                .width(rect.width() - 8.0)
                .show_ui(&mut child, |ui| {
                    for (i, label) in &choices {
                        ui.selectable_value(&mut t.syntax_highlighting, *i, label);
                    }
                });
        }
        ("OngletTerminal", "sCheckBox5") => check(ui, &mut s.terminal.paste_right_click),
        ("OngletTerminal", "sCheckBox8") => check(ui, &mut s.terminal.confirm_exit),
        ("OngletTerminal", "sCheckBox11") => {
            let t = &mut s.terminal;
            let mut on = !t.log_dir.is_empty();
            if ui
                .put(rect, egui::Checkbox::new(&mut on, caption))
                .changed()
            {
                t.log_dir = if on {
                    rfd::FileDialog::new()
                        .pick_folder()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
            }
        }
        ("OngletTerminal", "sEdit3") => {
            let t = &mut s.terminal;
            ui.add_enabled_ui(!t.log_dir.is_empty(), |ui| {
                ui.put(rect, egui::TextEdit::singleline(&mut t.log_dir));
            });
        }
        ("OngletTerminal", "ComboShells") => {
            bound_label(ui, rect, caption);
            let t = &mut s.terminal;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            egui::ComboBox::from_id_salt("cfg-local-shell")
                .selected_text(t.local_shell.label())
                .width(rect.width() - 8.0)
                .show_ui(&mut child, |ui| {
                    for shell in LocalShell::ALL {
                        ui.selectable_value(&mut t.local_shell, shell, shell.label());
                    }
                });
        }
        ("OngletTerminal", "CheckPath") => check(ui, &mut s.terminal.use_windows_path),
        ("OngletTerminal", "sComboBox10") => {
            use tc_core::settings::LogType;
            let mut index = LogType::ALL
                .iter()
                .position(|t| *t == s.terminal.log_type)
                .unwrap_or(0);
            let labels: Vec<&str> = LogType::ALL.iter().map(|t| t.label()).collect();
            if choice(ui, rect, caption, "cfg-log-type", &labels, &mut index) {
                s.terminal.log_type = LogType::ALL[index];
            }
        }
        ("OngletTerminal", "BtnLogFilename") => {
            bound_label(ui, rect, caption);
            ui.put(
                rect,
                egui::TextEdit::singleline(&mut s.terminal.log_name).hint_text("<default>"),
            )
            .on_hover_text(
                "{date}, {time} and {session} are replaced; default {date}_{time}_{session}",
            );
        }
        ("OngletMisc", "sSpeedButton2") if cfg!(windows) => {
            if button(ui, caption) {
                let answer = rfd::MessageDialog::new()
                    .set_title("Windows right-click menu entries")
                    .set_description(
                        "Add \"Open TermCraft here\" to the Explorer menus of folders (for \
                         your account only)?",
                    )
                    .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                        "Add".into(),
                        "Remove".into(),
                        "Cancel".into(),
                    ))
                    .show();
                let remove = match answer {
                    rfd::MessageDialogResult::Custom(c) if c == "Add" => Some(false),
                    rfd::MessageDialogResult::Custom(c) if c == "Remove" => Some(true),
                    rfd::MessageDialogResult::Yes => Some(false),
                    rfd::MessageDialogResult::No => Some(true),
                    _ => None,
                };
                if let Some(remove) = remove {
                    out.push(Action::Run(tc_commands::Command::ExplorerMenu { remove }));
                }
            }
        }
        ("OngletMisc", "sSpeedButton15") => {
            if button(ui, caption) {
                out.push(Action::EditPresets);
            }
        }
        ("OngletMisc", "sFilenameEdit1") => {
            bound_label(ui, rect, caption);
            let g = &mut s.general;
            let browse = Rect::from_min_size(
                egui::pos2(rect.right() - rect.height(), rect.top()),
                vec2(rect.height(), rect.height()),
            );
            let field =
                Rect::from_min_max(rect.min, egui::pos2(browse.left() - 2.0, rect.bottom()));
            ui.put(
                field,
                egui::TextEdit::singleline(&mut g.text_editor).hint_text("built-in text editor"),
            )
            .on_hover_text("The program that opens remote files for editing");
            if ui
                .put(browse, egui::Button::new(icon::FOLDER_OPEN))
                .clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                g.text_editor = path.display().to_string();
            }
        }
        ("OngletTerminal", "sCheckBox57") => check(ui, &mut s.terminal.backspace_ctrl_h),
        // -------------------------------------------------------------- SSH
        ("OngletSSH", "sCheckBox27") => check(ui, &mut s.ssh.browser),
        ("OngletSSH", "sCheckBox4") => check(ui, &mut s.ssh.switch_to_browser),
        ("OngletSSH", "ChkRemoteMonitoring") => check(ui, &mut s.ssh.remote_monitoring),
        ("OngletSSH", "sCheckBox26") => {
            let mut on = s.ssh.keepalive_secs > 0;
            if ui
                .put(rect, egui::Checkbox::new(&mut on, caption))
                .changed()
            {
                s.ssh.keepalive_secs = if on { 60 } else { 0 };
            }
        }
        // Always on in TermCraft.
        ("OngletSSH", "sCheckBox64") => {
            ui.add_enabled_ui(false, |ui| {
                ui.put(rect, egui::Checkbox::new(&mut true, caption))
            })
            .inner
            .on_disabled_hover_text(
                "TermCraft always checks a server's identity at the first connection",
            );
        }
        ("OngletSSH", "sCheckBox45") => check(ui, &mut s.ssh.internal_agent),
        ("OngletSSH", "sBitBtn19") => {
            let enabled = s.ssh.internal_agent && !s.ssh.agent_keys.is_empty();
            if ui
                .add_enabled_ui(enabled, |ui| ui.put(rect, egui::Button::new(caption)))
                .inner
                .clicked()
            {
                let lines: Vec<String> = s
                    .ssh
                    .agent_keys
                    .iter()
                    .map(|k| {
                        let about = std::fs::read_to_string(k.trim())
                            .ok()
                            .and_then(|t| tc_ssh::keygen::describe_public(&t))
                            .unwrap_or_else(|| "cannot be read".into());
                        format!("{}\n    {about}", k.trim())
                    })
                    .collect();
                rfd::MessageDialog::new()
                    .set_title("Keys of the TermCraft agent")
                    .set_description(lines.join("\n\n"))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
            }
        }
        ("OngletSSH", "sListBox1") => agent_keys(ui, rect, caption, s, state),
        ("OngletSSH", "sBitBtn17") => {
            if button(ui, "+")
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                s.ssh.agent_keys.push(path.display().to_string());
            }
        }
        ("OngletSSH", "sBitBtn18") => {
            let selected = state.agent_key.filter(|i| *i < s.ssh.agent_keys.len());
            let clicked = ui
                .add_enabled_ui(selected.is_some(), |ui| {
                    ui.put(rect, egui::Button::new("-"))
                })
                .inner
                .clicked();
            if clicked && let Some(i) = selected {
                s.ssh.agent_keys.remove(i);
                state.agent_key = None;
            }
        }
        ("OngletSSH", "sCheckBox62") if cfg!(windows) => check(ui, &mut s.ssh.use_pageant),
        ("OngletSSH", "sCheckBox2") => {
            let label = if cfg!(windows) {
                caption
            } else {
                "Use SSH agent"
            };
            ui.put(
                rect,
                egui::Checkbox::new(&mut s.ssh.use_system_agent, label),
            );
        }
        // ---------------------------------------------------------- Display
        ("OngletDisplay", "ComboSkins") => {
            bound_label(ui, rect, caption);
            let v = &mut s.view;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            egui::ComboBox::from_id_salt("cfg-theme")
                .selected_text(v.theme.label())
                .width(rect.width() - 8.0)
                .show_ui(&mut child, |ui| {
                    for theme in Theme::ALL {
                        ui.selectable_value(&mut v.theme, theme, theme.label());
                    }
                });
        }
        ("OngletDisplay", "sScrollBar1") => slider(ui, rect, &mut s.view.saturation, 90),
        ("OngletDisplay", "sScrollBar2") => {
            slider(
                ui,
                rect,
                &mut s.view.transparency,
                tc_core::settings::MAX_TRANSPARENCY,
            );
        }
        ("OngletDisplay", "sScrollBar3") => slider(
            ui,
            rect,
            &mut s.view.detached_transparency,
            tc_core::settings::MAX_TRANSPARENCY,
        ),
        ("OngletDisplay", "sLabel2") => percent(ui, rect, s.view.saturation),
        ("OngletDisplay", "sLabel7") => percent(ui, rect, s.view.transparency),
        ("OngletDisplay", "sLabel6") => percent(ui, rect, s.view.detached_transparency),
        ("OngletDisplay", "sRadioGroup1") => {
            let v = &mut s.view;
            let mut index = match (v.show_menu_bar, v.show_toolbar) {
                (true, false) => 0,
                (false, true) => 1,
                _ => 2,
            };
            if choice(
                ui,
                rect,
                caption,
                "cfg-bars",
                &["Show menu bar", "Show buttons bar", "Show both"],
                &mut index,
            ) {
                (v.show_menu_bar, v.show_toolbar) = (index != 1, index != 0);
            }
        }
        ("OngletDisplay", "sRadioGroup2") => {
            let styles = [
                ToolbarStyle::Small,
                ToolbarStyle::Standard,
                ToolbarStyle::StandardWithCaptions,
            ];
            let mut index = styles
                .iter()
                .position(|t| *t == s.view.toolbar_style)
                .unwrap_or(2);
            if choice(
                ui,
                rect,
                caption,
                "cfg-buttons",
                &["Small buttons", "Standard buttons", "Buttons with captions"],
                &mut index,
            ) {
                s.view.toolbar_style = styles[index];
            }
        }
        ("OngletDisplay", "sRadioGroup3") => {
            let mut index = usize::from(s.view.sidebar_right);
            if choice(
                ui,
                rect,
                caption,
                "cfg-sidebar",
                &["left", "right"],
                &mut index,
            ) {
                s.view.sidebar_right = index == 1;
            }
        }
        ("OngletDisplay", "ComboCustomDpi") => {
            let mut index = UiSize::ALL
                .iter()
                .position(|u| *u == s.view.ui_size)
                .unwrap_or(0);
            let labels: Vec<&str> = UiSize::ALL.iter().map(|u| u.label()).collect();
            if choice(ui, rect, caption, "cfg-ui-size", &labels, &mut index) {
                s.view.ui_size = UiSize::ALL[index];
            }
        }
        ("OngletDisplay", "sCheckBox51") => check(ui, &mut s.view.no_quick_connect_without_sidebar),
        ("OngletDisplay", "sCheckBox31") => check(ui, &mut s.view.show_tab_numbers),
        ("OngletDisplay", "sCheckBox33") => check(ui, &mut s.view.show_close_button),
        ("OngletDisplay", "ChkRoundedTabs") => check(ui, &mut s.view.rounded_tabs),
        ("OngletDisplay", "ChkSessionsDots") => check(ui, &mut s.view.session_dots),
        // ---------------------------------------------------------- Toolbar
        ("sTabSheet1", "sListView1") => {
            let shown = super::toolbar::chosen(&s.view);
            let available: Vec<&'static str> = super::toolbar::BUTTONS
                .into_iter()
                .filter(|b| !shown.contains(b))
                .collect();
            button_list(
                ui,
                rect,
                caption,
                "cfg-available",
                &available,
                &mut state.available,
            );
        }
        ("sTabSheet1", "sListView2") => {
            let shown = super::toolbar::chosen(&s.view);
            button_list(
                ui,
                rect,
                caption,
                "cfg-toolbar",
                &shown,
                &mut state.on_toolbar,
            );
        }
        // → add the selected available button / ← remove the selected one.
        ("sTabSheet1", "sBitBtn1") => {
            let enabled = state.available.is_some();
            if ui
                .add_enabled_ui(enabled, |ui| {
                    ui.put(rect, egui::Button::new(icon::ARROW_RIGHT))
                })
                .inner
                .clicked()
                && let Some(name) = state.available.take()
            {
                let mut list = super::toolbar::chosen(&s.view);
                list.push(name);
                s.view.toolbar_buttons = list.into_iter().map(String::from).collect();
            }
        }
        ("sTabSheet1", "sBitBtn10") => {
            let enabled = state.on_toolbar.is_some();
            if ui
                .add_enabled_ui(enabled, |ui| {
                    ui.put(rect, egui::Button::new(icon::ARROW_LEFT))
                })
                .inner
                .clicked()
                && let Some(name) = state.on_toolbar.take()
            {
                let list: Vec<String> = super::toolbar::chosen(&s.view)
                    .into_iter()
                    .filter(|b| *b != name)
                    .map(String::from)
                    .collect();
                // An empty list means "all": keep one button at least.
                if !list.is_empty() {
                    s.view.toolbar_buttons = list;
                }
            }
        }
        ("sTabSheet1", "sBitBtn13" | "sBitBtn15") => {
            let up = c.name == "sBitBtn13";
            let enabled = state.on_toolbar.is_some();
            if ui
                .add_enabled_ui(enabled, |ui| ui.put(rect, egui::Button::new(caption)))
                .inner
                .clicked()
                && let Some(name) = state.on_toolbar
            {
                let mut list = super::toolbar::chosen(&s.view);
                if let Some(i) = list.iter().position(|b| *b == name) {
                    let j = if up {
                        i.checked_sub(1)
                    } else {
                        Some(i + 1).filter(|j| *j < list.len())
                    };
                    if let Some(j) = j {
                        list.swap(i, j);
                    }
                }
                s.view.toolbar_buttons = list.into_iter().map(String::from).collect();
            }
        }
        ("sTabSheet1", "sBitBtn16") => {
            if button(ui, caption) {
                s.view.toolbar_buttons.clear();
                state.available = None;
                state.on_toolbar = None;
            }
        }
        // ------------------------------------------------- Misc (popup term.)
        ("sTabSheet2", "ScrollPopupConsoleTransparency") => {
            slider(
                ui,
                rect,
                &mut s.popup.transparency,
                tc_core::settings::MAX_TRANSPARENCY,
            );
        }
        ("sTabSheet2", "ScrollPopupConsoleSize") => slider(ui, rect, &mut s.popup.size, 100),
        ("sTabSheet2", "sLabel11") => percent(ui, rect, s.popup.transparency),
        ("sTabSheet2", "sLabel13") => percent(ui, rect, s.popup.size),
        ("sTabSheet2", "ComboPopupConsolePosition") => {
            let mut index = PopupPosition::ALL
                .iter()
                .position(|p| *p == s.popup.position)
                .unwrap_or(3);
            let labels: Vec<&str> = PopupPosition::ALL.iter().map(|p| p.label()).collect();
            if choice(ui, rect, caption, "cfg-popup-position", &labels, &mut index) {
                s.popup.position = PopupPosition::ALL[index];
            }
        }
        ("sTabSheet2", "ChkPopupConsoleIsSizeable") => check(ui, &mut s.popup.sizeable),
        ("sTabSheet2", "sComboEdit1") => {
            use tc_core::settings::StartupTab;
            let mut index = StartupTab::ALL
                .iter()
                .position(|t| *t == s.general.startup_tab)
                .unwrap_or(1);
            let labels: Vec<&str> = StartupTab::ALL.iter().map(|t| t.label()).collect();
            if choice(ui, rect, caption, "cfg-startup-tab", &labels, &mut index) {
                s.general.startup_tab = StartupTab::ALL[index];
            }
        }
        ("sTabSheet2", "sComboEdit2") => {
            let mut index = usize::from(!s.general.plus_opens_home);
            if choice(
                ui,
                rect,
                caption,
                "cfg-plus-tab",
                &["<Home>", "<Terminal>"],
                &mut index,
            ) {
                s.general.plus_opens_home = index == 0;
            }
        }
        ("sTabSheet2", "CmbNewTabPosition") => {
            use tc_core::settings::NewTabPosition;
            let mut index = NewTabPosition::ALL
                .iter()
                .position(|t| *t == s.general.new_tab_position)
                .unwrap_or(1);
            let labels: Vec<&str> = NewTabPosition::ALL.iter().map(|t| t.label()).collect();
            if choice(ui, rect, caption, "cfg-new-tab", &labels, &mut index) {
                s.general.new_tab_position = NewTabPosition::ALL[index];
            }
        }
        ("sTabSheet2", "sCheckBox60") => {
            let mut stay = !s.general.exit_on_last_tab;
            if ui
                .put(rect, egui::Checkbox::new(&mut stay, caption))
                .changed()
            {
                s.general.exit_on_last_tab = !stay;
            }
        }
        ("sTabSheet2", "sCheckBox63") => check(ui, &mut s.general.multiple_instances),
        ("OngletSSH", "sCheckBox61") => check(ui, &mut s.ssh.show_banner),
        // -------------------------------------------------------------- X11
        ("OngletX11", "sCheckBox3") => check(ui, &mut s.x_server.autostart),
        ("OngletX11", "sComboBox6") => {
            use tc_core::settings::XDisplayMode;
            let mut index = XDisplayMode::ALL
                .iter()
                .position(|m| *m == s.x_server.display_mode)
                .unwrap_or(0);
            let labels: Vec<&str> = XDisplayMode::ALL.iter().map(|m| m.label()).collect();
            if choice(ui, rect, caption, "cfg-x-mode", &labels, &mut index) {
                s.x_server.display_mode = XDisplayMode::ALL[index];
            }
        }
        ("OngletX11", "sComboBox12") => {
            let mut index = usize::from(!s.x_server.opengl_hardware);
            if choice(
                ui,
                rect,
                caption,
                "cfg-x-gl",
                &["hardware", "software"],
                &mut index,
            ) {
                s.x_server.opengl_hardware = index == 0;
            }
        }
        ("OngletX11", "sComboBox7") => {
            let mut index = usize::from(s.x_server.clipboard.min(2));
            let items = ["disabled", "enabled", "disable primary"];
            if choice(ui, rect, caption, "cfg-x-clip", &items, &mut index) {
                s.x_server.clipboard = u8::try_from(index).unwrap_or(1);
            }
        }
        ("OngletX11", "sComboBox5") => {
            // The XKB layouts of the XDMCP "Keyboard type" list.
            let layouts: Vec<&str> = tc_core::schema::type_params(3)
                .iter()
                .find(|p| p.id == "3_11")
                .and_then(|p| match p.widget {
                    tc_core::schema::Widget::Choice(items) => Some(items.to_vec()),
                    _ => None,
                })
                .unwrap_or_else(|| vec!["Automatic"]);
            let mut index = layouts
                .iter()
                .position(|l| *l == s.x_server.keyboard)
                .unwrap_or(0);
            if choice(ui, rect, caption, "cfg-x-kbd", &layouts, &mut index) {
                s.x_server.keyboard = if index == 0 {
                    String::new()
                } else {
                    layouts[index].to_string()
                };
            }
        }
        ("OngletX11", "sSpinEdit1") => {
            bound_label(ui, rect, caption);
            ui.put(
                rect,
                egui::DragValue::new(&mut s.x_server.display).range(0..=99),
            )
            .on_hover_text("The X display number: :0, :1 ...");
        }
        ("OngletX11", "sComboBox4") => {
            let values = [0u16, 96, 120, 144, 192];
            let labels = ["auto", "96", "120", "144", "192"];
            let mut index = values
                .iter()
                .position(|v| *v == s.x_server.dpi)
                .unwrap_or(0);
            if choice(ui, rect, caption, "cfg-x-dpi", &labels, &mut index) {
                s.x_server.dpi = values[index];
            }
        }
        ("OngletX11", "ComboAccessControl") => {
            let mut index = usize::from(s.x_server.remote_access);
            let items = ["disabled", "full (no access control)"];
            if choice(ui, rect, caption, "cfg-x-access", &items, &mut index) {
                s.x_server.remote_access = index == 1;
            }
        }
        // ---------------------------------------------------------- Security
        ("OngletCustom", "sCheckBox25") => check(ui, &mut s.passwords.disable_show),
        ("OngletCustom", "ChkDisablePassphraseCaching") => {
            let mut off = !s.passwords.save_key_passphrases;
            if ui
                .put(rect, egui::Checkbox::new(&mut off, caption))
                .changed()
            {
                s.passwords.save_key_passphrases = !off;
            }
        }
        ("OngletCustom", "sCheckBox15") => {
            use tc_core::settings::SavePasswords;
            let mut off = s.passwords.save == SavePasswords::Never;
            if ui
                .put(rect, egui::Checkbox::new(&mut off, caption))
                .changed()
            {
                s.passwords.save = if off {
                    SavePasswords::Never
                } else {
                    SavePasswords::Ask
                };
            }
        }
        ("OngletCustom", "sCheckBox6") => {
            use tc_core::settings::MasterPrompt;
            let mut off = s.passwords.master_prompt == MasterPrompt::EveryStartup;
            if ui
                .put(rect, egui::Checkbox::new(&mut off, caption))
                .changed()
            {
                s.passwords.master_prompt = if off {
                    MasterPrompt::EveryStartup
                } else {
                    MasterPrompt::NewComputer
                };
            }
        }
        ("OngletCustom", "sCheckBox13") => check(ui, &mut s.terminal.disable_log),
        ("OngletCustom", "ChkDisableRdpReconnect") => {
            check(ui, &mut s.security.disable_rdp_reconnect);
        }
        ("OngletCustom", "ChkDisableXAuto") => check(ui, &mut s.security.disable_x11_autostart),
        ("OngletCustom", "CmbCyg") => {
            // "Disable all local terminals" = the Shell and WSL session types.
            let local = ["Shell", "WSL"];
            let off = local
                .iter()
                .all(|t| s.security.disabled_sessions.iter().any(|d| d == t));
            let mut index = usize::from(off);
            let labels = ["Enable local terminals", "Disable all local terminals"];
            if choice(ui, rect, caption, "cfg-sec-local", &labels, &mut index) {
                s.security
                    .disabled_sessions
                    .retain(|d| !local.contains(&d.as_str()));
                if index == 1 {
                    s.security
                        .disabled_sessions
                        .extend(local.iter().map(|t| (*t).to_string()));
                }
            }
        }
        ("OngletCustom", "CmbForceMasterPassword") => {
            let values = [0u8, 8, 12];
            let labels = [
                "none",
                "standard (8 characters min)",
                "strong (12 characters min)",
            ];
            let mut index = values
                .iter()
                .position(|v| *v == s.passwords.master_min_len)
                .unwrap_or(0);
            if choice(ui, rect, caption, "cfg-master-min", &labels, &mut index) {
                s.passwords.master_min_len = values[index];
            }
        }
        ("OngletCustom", "sComboBox2") => {
            let mut index = usize::from(s.security.disable_macros);
            if choice(
                ui,
                rect,
                caption,
                "cfg-sec-macros",
                &["Enable all macros", "Disable all macros"],
                &mut index,
            ) {
                s.security.disable_macros = index == 1;
            }
        }
        ("OngletCustom", "ComboX11RemoteAccess") => {
            let labels = [
                "let user choose X11 security settings",
                "disable remote access",
                "disable X11 server",
            ];
            let mut index = usize::from(s.security.x11_policy.min(2));
            if choice(ui, rect, caption, "cfg-sec-x11", &labels, &mut index) {
                s.security.x11_policy = u8::try_from(index).unwrap_or(0);
            }
        }
        ("OngletCustom", "ChkListClients") => {
            let items: Vec<(&str, &str)> = tc_core::schema::SESSION_TYPES
                .iter()
                .map(|t| (*t, *t))
                .collect();
            check_list(
                ui,
                rect,
                "cfg-sec-sessions",
                &items,
                &mut s.security.disabled_sessions,
            );
        }
        ("OngletCustom", "ChkListServers") => {
            let items = [("HTTP", "http"), ("TFTP", "tftp")];
            check_list(
                ui,
                rect,
                "cfg-sec-servers",
                &items,
                &mut s.security.disabled_servers,
            );
        }
        ("OngletCustom", "ChkListTools") => {
            let items: Vec<(&str, &str)> = super::menus::TOOLS
                .iter()
                .flat_map(|(_, tools)| tools.iter())
                .filter(|t| super::menus::implemented_tool(t).is_some())
                .map(|t| (*t, *t))
                .collect();
            check_list(
                ui,
                rect,
                "cfg-sec-tools",
                &items,
                &mut s.security.disabled_tools,
            );
        }
        ("OngletCustom", "BtnStandardSec" | "BtnHighSec" | "BtnInsaneSec") => {
            if button(ui, caption) {
                security_preset(s, c.name);
            }
        }
        ("OngletSSH", "sEdit5") => {
            bound_label(ui, rect, caption);
            ui.put(rect, egui::TextEdit::singleline(&mut s.ssh.default_login))
                .on_hover_text("The user of SSH sessions that name none");
        }
        ("OngletSSH", "ChkPreserveFilesDates") => check(ui, &mut s.ssh.preserve_file_dates),
        ("sTabSheet2", "ChkBackgroundPopupTermInsteadOfClosingIt") => {
            check(ui, &mut s.popup.hide_instead_of_close);
        }
        // ------------------------------------------- everything else: drawn
        _ => match c.kind {
            Kind::Group => group(ui, rect, c.caption),
            Kind::Label => {
                if !caption.is_empty() && c.w < 600 {
                    ui.put(rect, egui::Label::new(caption));
                }
            }
            Kind::Button => {
                if !caption.is_empty() {
                    ui.add_enabled_ui(false, |ui| ui.put(rect, egui::Button::new(caption)))
                        .inner
                        .on_disabled_hover_text(NOT_YET);
                }
            }
            Kind::List if !c.items.is_empty() => list(ui, rect, c),
            // Its label above is drawn already.
            _ if c.label_above => placeholder(ui, &Ctl { caption: "", ..*c }, rect),
            _ => placeholder(ui, c, rect),
        },
    }
}

/// A list of check boxes: checked = the item's id is in `disabled`.
fn check_list(
    ui: &mut egui::Ui,
    rect: Rect,
    id: &str,
    items: &[(&str, &str)],
    disabled: &mut Vec<String>,
) {
    let v = ui.visuals().clone();
    ui.painter().rect(
        rect,
        2.0,
        v.extreme_bg_color,
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(3.0)));
    child.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(rect.height() - 6.0)
        .show(&mut child, |ui| {
            ui.set_width(rect.width() - 8.0);
            for (label, key) in items {
                let mut off = disabled.iter().any(|d| d == key);
                if ui.checkbox(&mut off, *label).changed() {
                    disabled.retain(|d| d != key);
                    if off {
                        disabled.push((*key).to_string());
                    }
                }
            }
        });
}

/// "Apply presets": Defaults, High security (passwords are never kept or
/// shown) and Restricted features (also no logs, servers or tools).
fn security_preset(s: &mut Settings, button: &str) {
    use tc_core::settings::{MasterPrompt, SavePasswords};
    let high = button != "BtnStandardSec";
    let restricted = button == "BtnInsaneSec";
    let p = &mut s.passwords;
    p.disable_show = high;
    p.save_key_passphrases = false;
    p.save = if high {
        SavePasswords::Never
    } else {
        SavePasswords::Ask
    };
    p.master_prompt = if high {
        MasterPrompt::EveryStartup
    } else {
        MasterPrompt::NewComputer
    };
    s.terminal.disable_log = restricted;
    p.master_min_len = if high { 12 } else { 0 };
    s.security.disable_macros = restricted;
    s.security.disable_rdp_reconnect = restricted;
    s.security.disable_x11_autostart = restricted;
    s.security.x11_policy = u8::from(high);
    s.security.disabled_sessions.clear();
    if restricted {
        s.security.disabled_servers = vec!["http".into(), "tftp".into()];
        s.security.disabled_tools = super::menus::TOOLS
            .iter()
            .flat_map(|(_, tools)| tools.iter())
            .filter(|t| super::menus::implemented_tool(t).is_some())
            .map(|t| (*t).to_string())
            .collect();
    } else {
        s.security.disabled_servers.clear();
        s.security.disabled_tools.clear();
    }
}

/// A list of toolbar buttons with its caption above; click selects one.
fn button_list(
    ui: &mut egui::Ui,
    rect: Rect,
    caption: &str,
    id: &str,
    buttons: &[&'static str],
    selected: &mut Option<&'static str>,
) {
    let v = ui.visuals().clone();
    ui.painter().text(
        rect.left_top() - vec2(0.0, 3.0),
        egui::Align2::LEFT_BOTTOM,
        caption,
        egui::TextStyle::Body.resolve(ui.style()),
        v.text_color(),
    );
    ui.painter().rect(
        rect,
        2.0,
        v.extreme_bg_color,
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(3.0)));
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(rect.height() - 6.0)
        .show(&mut child, |ui| {
            ui.set_width(rect.width() - 8.0);
            for b in buttons {
                if ui.selectable_label(*selected == Some(*b), *b).clicked() {
                    *selected = Some(*b);
                }
            }
        });
}

/// A percentage slider in a reference scroll bar's place.
fn slider(ui: &mut egui::Ui, rect: Rect, value: &mut u8, max: u8) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.spacing_mut().slider_width = rect.width();
    child.add(egui::Slider::new(value, 0..=max).show_value(false));
}

/// The value label beside a slider.
fn percent(ui: &mut egui::Ui, rect: Rect, value: u8) {
    ui.put(rect, egui::Label::new(format!("{value}%")));
}

/// A combo box with its bound label; true when the choice changed.
fn choice(
    ui: &mut egui::Ui,
    rect: Rect,
    caption: &str,
    id: &str,
    items: &[&str],
    index: &mut usize,
) -> bool {
    if !caption.is_empty() {
        bound_label(ui, rect, caption);
    }
    let before = *index;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    egui::ComboBox::from_id_salt(id)
        .selected_text(items.get(*index).copied().unwrap_or(""))
        .width(rect.width() - 8.0)
        .show_ui(&mut child, |ui| {
            for (i, label) in items.iter().enumerate() {
                ui.selectable_value(index, i, *label);
            }
        });
    *index != before
}

/// "Load following keys at MobAgent startup": the internal agent's keys.
fn agent_keys(
    ui: &mut egui::Ui,
    rect: Rect,
    caption: &str,
    s: &mut Settings,
    state: &mut ConfigState,
) {
    let v = ui.visuals().clone();
    ui.painter().text(
        rect.left_top() - vec2(0.0, 3.0),
        egui::Align2::LEFT_BOTTOM,
        caption,
        egui::TextStyle::Body.resolve(ui.style()),
        v.text_color(),
    );
    ui.painter().rect(
        rect,
        2.0,
        v.extreme_bg_color,
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(3.0)));
    if !s.ssh.internal_agent {
        child.disable();
    }
    egui::ScrollArea::vertical()
        .id_salt("cfg-agent-keys")
        .max_height(rect.height() - 6.0)
        .show(&mut child, |ui| {
            for (i, key) in s.ssh.agent_keys.iter().enumerate() {
                if ui
                    .selectable_label(state.agent_key == Some(i), egui::RichText::new(key).small())
                    .clicked()
                {
                    state.agent_key = Some(i);
                }
            }
        });
}

/// "Default font settings" / "Default color settings", drawn from the
/// reference dialogs and bound to the default terminal settings.
fn sub_dialog(ctx: &egui::Context, state: &mut ConfigState, s: &mut Settings) {
    let Some(name) = state.sub_dialog else {
        return;
    };
    let Some(page) = layout::page(name) else {
        state.sub_dialog = None;
        return;
    };
    let size = vec2(page.width as f32, page.height as f32);
    let mut open = true;
    let mut close = false;
    egui::Window::new(page.caption)
        .id(egui::Id::new(("config-sub-dialog", name)))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .fixed_size(size)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            compact(ui);
            let (area, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let strip = Rect::from_min_size(area.min, vec2(89.0, size.y));
            ui.painter()
                .rect_filled(strip, 0.0, ui.visuals().faint_bg_color);
            for c in page.controls {
                let rect = Rect::from_min_size(
                    area.min + vec2(c.x as f32, c.y as f32),
                    vec2(c.w as f32, c.h as f32),
                );
                if c.kind == Kind::Button && c.caption.trim() == "OK" {
                    if ui.put(rect, egui::Button::new("OK")).clicked() {
                        close = true;
                    }
                    continue;
                }
                default_control(ui, name, c, rect, s);
            }
        });
    if !open || close {
        state.sub_dialog = None;
    }
}

/// A control of the font / color dialog, bound to the defaults.
fn default_control(ui: &mut egui::Ui, dialog: &str, c: &'static Ctl, rect: Rect, s: &mut Settings) {
    let caption = c.caption.trim();
    let t = &mut s.terminal;
    match (dialog, c.name) {
        ("FormFont", "ComboFontName") => {
            bound_label(ui, rect, caption);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            crate::term_fonts::combo(&mut child, "cfg-font", &mut t.font_name);
        }
        // Points, as in the reference (1 pt = 4/3 px).
        ("FormFont", "SpinFontSize") => {
            bound_label(ui, rect, caption);
            let mut pt = (t.font_size * 3.0 / 4.0).round() as u32;
            if ui
                .put(rect, egui::DragValue::new(&mut pt).range(5..=48))
                .changed()
            {
                t.font_size = pt as f32 * 4.0 / 3.0;
            }
        }
        ("FormFont", "CheckBold") => {
            ui.put(rect, egui::Checkbox::new(&mut t.font_bold, caption));
        }
        ("FormFont", "sCheckBox9") => {
            ui.put(rect, egui::Checkbox::new(&mut t.fixed_grid, caption));
        }
        ("FormFont", "sSpinEdit15") => {
            ui.add_enabled_ui(t.fixed_grid, |ui| {
                ui.put(
                    rect,
                    egui::DragValue::new(&mut t.fixed_cols).range(20..=500),
                );
            });
        }
        ("FormFont", "sSpinEdit16") => {
            if !caption.is_empty() {
                bound_label(ui, rect, caption);
            }
            ui.add_enabled_ui(t.fixed_grid, |ui| {
                ui.put(rect, egui::DragValue::new(&mut t.fixed_rows).range(5..=200));
            });
        }
        ("FormColor", name)
            if name == "ColBack"
                || name == "ColFore"
                || name == "ColCur"
                || name.starts_with("Col") && name[3..].parse::<usize>().is_ok() =>
        {
            let colors = t.colors.get_or_insert_with(default_colors);
            let slot = match name {
                "ColBack" => &mut colors.background,
                "ColFore" => &mut colors.foreground,
                "ColCur" => &mut colors.cursor,
                n => {
                    let i: usize = n[3..].parse().unwrap_or(1);
                    // Col1 Black, Col2 BoldBlack, Col3 Red …
                    let ansi = (i - 1) / 2 + if i.is_multiple_of(2) { 8 } else { 0 };
                    &mut colors.ansi[ansi.min(15)]
                }
            };
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            egui::color_picker::color_edit_button_srgb(&mut child, slot);
            if !caption.is_empty() {
                child.label(caption);
            }
        }
        ("FormColor", "ComboCursor") => {
            let colors = t.colors.get_or_insert_with(default_colors);
            let mut index =
                usize::from(colors.cursor_type.min(2)) + if colors.cursor_blink { 3 } else { 0 };
            if choice(
                ui,
                rect,
                caption,
                "cfg-cursor",
                &[
                    "Block",
                    "Underline",
                    "Line",
                    "Blinking block",
                    "Blinking underline",
                    "Blinking line",
                ],
                &mut index,
            ) {
                colors.cursor_type = (index % 3) as u8;
                colors.cursor_blink = index >= 3;
            }
        }
        _ => match c.kind {
            Kind::Group => group(ui, rect, c.caption),
            Kind::Label => {
                if !caption.is_empty() {
                    ui.put(rect, egui::Label::new(caption));
                }
            }
            Kind::Button => {}
            _ => placeholder(ui, c, rect),
        },
    }
}

/// TermCraft's own palette, as colors to edit.
fn default_colors() -> TerminalColors {
    let theme = tc_term::Theme::default();
    let rgb = |c: tc_term::Rgb| [c.0, c.1, c.2];
    TerminalColors {
        foreground: rgb(theme.foreground),
        background: rgb(theme.background),
        cursor: rgb(theme.cursor),
        ansi: theme.ansi.map(rgb),
        cursor_type: 0,
        cursor_blink: false,
    }
}

/// "Paste warning settings".
fn paste_settings(ctx: &egui::Context, state: &mut ConfigState, s: &mut Settings) {
    if !state.paste_settings {
        return;
    }
    let mut open = true;
    egui::Window::new("TermCraft paste settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.strong("Select which warnings to show before pasting");
            ui.add_space(6.0);
            ui.checkbox(
                &mut s.terminal.warn_multiline_paste,
                "Warn me when pasted content contains multiple lines",
            )
            .on_hover_text("Several lines may run several commands when pasted into a terminal");
            ui.add_space(6.0);
            if ui.button("OK").clicked() {
                state.paste_settings = false;
            }
        });
    if !open {
        state.paste_settings = false;
    }
}

/// TermCraft's own tab: where its data is, links, known hosts, updates.
fn termcraft_tab(
    ui: &mut egui::Ui,
    rect: Rect,
    s: &mut Settings,
    paths: &Paths,
    out: &mut Vec<Action>,
) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)));
    let ui = &mut child;
    ui.set_clip_rect(rect.shrink(12.0));
    // A long path is cut at the page edge; the whole one is in the hint.
    let path = |ui: &mut egui::Ui, p: std::path::PathBuf| {
        let text = p.display().to_string();
        ui.add(egui::Label::new(egui::RichText::new(&text).monospace()).truncate())
            .on_hover_text(text);
    };
    egui::Grid::new("cfg-termcraft")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .max_col_width(rect.width() - 150.0)
        .show(ui, |ui| {
            ui.label("Configuration file:");
            path(ui, paths.settings_file());
            ui.end_row();
            ui.label("Sessions file:");
            path(ui, paths.sessions_file());
            ui.end_row();
            ui.label("Mode:");
            ui.label(if paths.portable {
                "Portable (data next to the program)"
            } else {
                "Installed (per-user data folders)"
            });
            ui.end_row();
        });
    ui.weak(
        "Tip: an empty \"termcraft.portable\" file next to the program switches to portable mode.",
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .button("Open termcraft:// links with TermCraft")
            .on_hover_text("termcraft://session/<name> opens a saved session (current user only)")
            .clicked()
        {
            out.push(Action::Run(tc_commands::Command::UrlHandlerRegister {}));
        }
        if ui.button("Known hosts…").clicked() {
            out.push(Action::KnownHosts);
        }
    });
    ui.add_space(8.0);
    ui.group(|ui| {
        ui.strong("Updates");
        let u = &mut s.update;
        ui.checkbox(&mut u.check_on_startup, "Check for updates at startup");
        ui.checkbox(&mut u.auto_install, "Install updates automatically");
        ui.horizontal(|ui| {
            ui.label("Update channel:");
            ui.radio_value(&mut u.channel, UpdateChannel::Stable, "Stable");
            ui.radio_value(&mut u.channel, UpdateChannel::Beta, "Beta");
        });
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Scrollback lines:");
        ui.add(
            egui::DragValue::new(&mut s.terminal.scrollback_lines)
                .range(100..=200_000)
                .speed(100),
        );
        ui.checkbox(&mut s.terminal.copy_on_select, "Copy on select")
            .on_hover_text("Selected text is copied to the clipboard (also in Expert settings)");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_presets() {
        let mut s = Settings::default();
        security_preset(&mut s, "BtnInsaneSec");
        assert!(s.passwords.disable_show && s.terminal.disable_log);
        assert!(!s.security.server_allowed("http"));
        assert!(!s.security.tool_allowed("Text editor"));
        security_preset(&mut s, "BtnHighSec");
        assert!(s.passwords.disable_show && !s.terminal.disable_log);
        assert!(s.security.tool_allowed("Text editor"));
        security_preset(&mut s, "BtnStandardSec");
        assert_eq!(s.passwords, Settings::default().passwords);
        assert_eq!(s.security, Settings::default().security);
    }

    #[test]
    fn every_reference_tab_exists() {
        for tab in ConfigTab::ALL {
            if let Some(sheet) = tab.sheet() {
                assert!(layout::page(sheet).is_some(), "{sheet}");
            }
        }
    }

    #[test]
    fn color_slots_follow_the_reference_order() {
        // Col1 Black (0), Col2 BoldBlack (8), Col3 Red (1) … Col16 BoldWhite (15).
        let slot = |i: usize| (i - 1) / 2 + if i.is_multiple_of(2) { 8 } else { 0 };
        assert_eq!(slot(1), 0);
        assert_eq!(slot(2), 8);
        assert_eq!(slot(3), 1);
        assert_eq!(slot(16), 15);
    }
}
