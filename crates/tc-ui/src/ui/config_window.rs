//! Settings › Configuration (reference `TFORMPARAMS`): General, Terminal,
//! X11, SSH, Display, Toolbar and Misc tabs. Options TermCraft supports are
//! live; the rest are shown disabled so the layout matches.

use super::{Action, NOT_YET};
use tc_core::settings::{LocalShell, Theme, ToolbarStyle, UiSize, UpdateChannel};
use tc_core::{Paths, Settings};

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
}

fn disabled_check(ui: &mut egui::Ui, label: &str) {
    ui.add_enabled(false, egui::Checkbox::new(&mut false, label))
        .on_disabled_hover_text(NOT_YET);
}

/// Returns true when a setting changed (the caller persists it).
pub fn show(
    ctx: &egui::Context,
    open: &mut bool,
    tab: &mut ConfigTab,
    settings: &mut Settings,
    paths: &Paths,
    out: &mut Vec<Action>,
) -> bool {
    let before = settings.clone();
    egui::Window::new("TermCraft Configuration")
        .open(open)
        .collapsible(false)
        .default_width(640.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (t, label) in [
                    (ConfigTab::General, "General"),
                    (ConfigTab::Terminal, "Terminal"),
                    (ConfigTab::X11, "X11"),
                    (ConfigTab::Ssh, "SSH"),
                    (ConfigTab::Display, "Display"),
                    (ConfigTab::Toolbar, "Toolbar"),
                    (ConfigTab::Misc, "Misc"),
                ] {
                    ui.selectable_value(tab, t, label);
                }
            });
            ui.separator();
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(egui::vec2(600.0, 260.0));
                match tab {
                    ConfigTab::General => general(ui, paths),
                    ConfigTab::Terminal => terminal(ui, settings),
                    ConfigTab::X11 => x11(ui),
                    ConfigTab::Ssh => ssh(ui, settings),
                    ConfigTab::Display => display(ui, settings),
                    ConfigTab::Toolbar => {
                        ui.weak("Toolbar customization (add / remove / reorder buttons) is not implemented yet.");
                    }
                    ConfigTab::Misc => misc(ui, settings, out),
                }
            });
        });
    *settings != before
}

fn general(ui: &mut egui::Ui, paths: &Paths) {
    egui::Grid::new("cfg-general")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Configuration file location:");
            ui.monospace(paths.settings_file().display().to_string());
            ui.end_row();
            ui.label("Sessions file:");
            ui.monospace(paths.sessions_file().display().to_string());
            ui.end_row();
            ui.label("Mode:");
            ui.label(if paths.portable {
                "Portable (data next to the executable)"
            } else {
                "Installed (per-user data folders)"
            });
            ui.end_row();
        });
    ui.add_space(8.0);
    disabled_check(ui, "Automatically backup TermCraft configuration file");
    ui.weak("Tip: create an empty \"termcraft.portable\" file next to the executable for portable mode.");
}

fn terminal(ui: &mut egui::Ui, s: &mut Settings) {
    let t = &mut s.terminal;
    ui.horizontal(|ui| {
        ui.label("Local terminal environment:");
        egui::ComboBox::from_id_salt("cfg-local-shell")
            .selected_text(t.local_shell.label())
            .show_ui(ui, |ui| {
                for shell in LocalShell::ALL {
                    ui.selectable_value(&mut t.local_shell, shell, shell.label());
                }
            })
            .response
            .on_hover_text("Shell used by new local terminals (Start local terminal)");
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.checkbox(&mut t.backspace_ctrl_h, "Backspace sends ^H");
        ui.checkbox(&mut t.paste_right_click, "Paste using right-click");
        ui.checkbox(&mut t.confirm_exit, "Close prompt")
            .on_hover_text("Ask before exiting TermCraft while sessions are open");
    });
    ui.add_space(6.0);
    egui::Grid::new("cfg-term")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Default font size (px):");
            ui.add(egui::Slider::new(&mut t.font_size, 8.0..=32.0).step_by(1.0));
            ui.end_row();
            ui.label("Scrollback lines:");
            ui.add(
                egui::DragValue::new(&mut t.scrollback_lines)
                    .range(100..=200_000)
                    .speed(100),
            );
            ui.end_row();
        });
    ui.add_space(6.0);
    disabled_check(ui, "Log terminal output to the following directory");
    disabled_check(ui, "Use Windows PATH");
    ui.weak(
        "Per-session font, colors and terminal type are in each session's \"Terminal settings\".",
    );
}

fn x11(ui: &mut egui::Ui) {
    disabled_check(ui, "Automatically start X server at TermCraft start up");
    for ext in [
        "Randr",
        "Composite",
        "Damage",
        "Xfixes",
        "Xtest",
        "Xinerama",
    ] {
        disabled_check(ui, ext);
    }
    ui.weak("The X11 server and X11 forwarding are not implemented yet.");
}

fn ssh(ui: &mut egui::Ui, s: &mut Settings) {
    let ssh = &mut s.ssh;
    ui.checkbox(&mut ssh.browser, "Enable graphical SSH-browser");
    ui.checkbox(
        &mut ssh.switch_to_browser,
        "Automatically switch to SSH-browser tab after login",
    );
    ui.horizontal(|ui| {
        let mut on = ssh.keepalive_secs > 0;
        if ui.checkbox(&mut on, "SSH keepalive").changed() {
            ssh.keepalive_secs = if on { 60 } else { 0 };
        }
        if on {
            ui.add(
                egui::DragValue::new(&mut ssh.keepalive_secs)
                    .range(5..=3600)
                    .suffix(" s"),
            );
        }
    });
    ui.add_space(6.0);
    ui.checkbox(&mut ssh.remote_monitoring, "Remote-monitoring")
        .on_hover_text(
            "Display the \"remote monitoring\" bar below each SSH session: resources usage \
         (CPU / RAM / network / disk…) of the server. Linux servers only.",
        );
    disabled_check(ui, "GSSAPI Kerberos");
    disabled_check(ui, "Use internal SSH agent");
    disabled_check(ui, "Use PuTTY agent");
    disabled_check(ui, "Use Windows SSH agent");
    ui.weak("Host keys are always validated against known_hosts at first connection.");
}

fn display(ui: &mut egui::Ui, s: &mut Settings) {
    let v = &mut s.view;
    egui::Grid::new("cfg-display")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Skin selection:");
            egui::ComboBox::from_id_salt("cfg-theme")
                .selected_text(v.theme.label())
                .show_ui(ui, |ui| {
                    for theme in Theme::ALL {
                        ui.selectable_value(&mut v.theme, theme, theme.label());
                    }
                });
            ui.end_row();
            ui.label("Graphical interface elements size:");
            egui::ComboBox::from_id_salt("cfg-ui-size")
                .selected_text(v.ui_size.label())
                .show_ui(ui, |ui| {
                    for size in UiSize::ALL {
                        ui.selectable_value(&mut v.ui_size, size, size.label());
                    }
                });
            ui.end_row();
            ui.label("Menus and buttons:");
            let both = v.show_menu_bar && v.show_toolbar;
            let label = if both {
                "Show both"
            } else if v.show_menu_bar {
                "Show menu bar"
            } else {
                "Show buttons bar"
            };
            egui::ComboBox::from_id_salt("cfg-bars")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(both, "Show both").clicked() {
                        (v.show_menu_bar, v.show_toolbar) = (true, true);
                    }
                    if ui
                        .selectable_label(v.show_menu_bar && !v.show_toolbar, "Show menu bar")
                        .clicked()
                    {
                        (v.show_menu_bar, v.show_toolbar) = (true, false);
                    }
                    if ui
                        .selectable_label(!v.show_menu_bar && v.show_toolbar, "Show buttons bar")
                        .clicked()
                    {
                        (v.show_menu_bar, v.show_toolbar) = (false, true);
                    }
                });
            ui.end_row();
            ui.label("Buttons bar:");
            egui::ComboBox::from_id_salt("cfg-buttons")
                .selected_text(match v.toolbar_style {
                    ToolbarStyle::Small => "Small buttons",
                    ToolbarStyle::Standard => "Standard buttons",
                    ToolbarStyle::StandardWithCaptions => "Buttons with captions",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut v.toolbar_style, ToolbarStyle::Small, "Small buttons");
                    ui.selectable_value(
                        &mut v.toolbar_style,
                        ToolbarStyle::Standard,
                        "Standard buttons",
                    );
                    ui.selectable_value(
                        &mut v.toolbar_style,
                        ToolbarStyle::StandardWithCaptions,
                        "Buttons with captions",
                    );
                });
            ui.end_row();
            ui.label("Sidebar position:");
            egui::ComboBox::from_id_salt("cfg-sidebar")
                .selected_text(if v.sidebar_right { "right" } else { "left" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut v.sidebar_right, false, "left");
                    ui.selectable_value(&mut v.sidebar_right, true, "right");
                });
            ui.end_row();
        });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Tabs settings:");
        ui.checkbox(&mut v.rounded_tabs, "Rounded tabs");
        ui.checkbox(&mut v.show_tab_numbers, "Show tab numbers");
        ui.checkbox(&mut v.show_close_button, "Show close buttons");
    });
    ui.checkbox(
        &mut v.session_dots,
        "Show green/orange buttons next to each open session in the sessions tree",
    );
    disabled_check(ui, "Disable \"Quick connect\" when sidebar is hidden");
}

fn misc(ui: &mut egui::Ui, s: &mut Settings, out: &mut Vec<Action>) {
    let u = &mut s.update;
    ui.checkbox(&mut u.check_on_startup, "Check for updates at startup");
    ui.checkbox(&mut u.auto_install, "Install updates automatically");
    ui.horizontal(|ui| {
        ui.label("Update channel:");
        ui.radio_value(&mut u.channel, UpdateChannel::Stable, "Stable");
        ui.radio_value(&mut u.channel, UpdateChannel::Beta, "Beta");
    });
    ui.separator();
    if ui.button("TermCraft passwords settings").clicked() {
        out.push(Action::PasswordsSettings);
    }
}
