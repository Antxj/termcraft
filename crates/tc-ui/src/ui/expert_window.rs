//! Configuration › Terminal › "Expert settings" (reference
//! `TFORMADVANCEDTERMSETTINGS`, 784×521): terminal features, answerback,
//! bell, scrollback, copy / paste and reconnection keys. Features TermCraft
//! does not have yet are listed disabled, in the reference order.

use super::NOT_YET;
use tc_core::settings::{
    ANY_OTHER_KEY, BellType, ExpertTerminal, RECONNECTION_KEYS, TerminalSettings,
};

/// "Terminal features", in reference order; `None` = not available yet.
fn feature(e: &mut ExpertTerminal, index: usize) -> Option<FeatureRef<'_>> {
    Some(match index {
        1 => FeatureRef::Flag(&mut e.implicit_cr_in_lf),
        2 => FeatureRef::Flag(&mut e.implicit_lf_in_cr),
        3 => FeatureRef::Flag(&mut e.flush_logs),
        5 => FeatureRef::Flag(&mut e.mouse_reporting),
        16 => FeatureRef::Force(&mut e.app_cursor, true),
        17 => FeatureRef::Force(&mut e.app_cursor, false),
        22 => FeatureRef::Flag(&mut e.bracketed_paste),
        _ => return None,
    })
}

enum FeatureRef<'a> {
    Flag(&'a mut bool),
    /// One of the "Force … on / off" pair.
    Force(&'a mut Option<bool>, bool),
}

const FEATURES: [&str; 23] = [
    "Display scrollbar",
    "Implicit CR in every LF",
    "Implicit LF in every CR",
    "Flush log files frequently",
    "Ctrl+Alt is different from AltGr",
    "Enable xterm-style mouse reporting",
    "Enable switching to the alternate screen",
    "Enable scrollback clearing",
    "Enable destructive backspace",
    "Enable character set configuration",
    "Enable arabic text shaping",
    "Enable bidirectional text display",
    "Force local echo on",
    "Force local echo off",
    "Force local line editing on",
    "Force local line editing off",
    "Force application cursor keys mode on",
    "Force application cursor keys mode off",
    "Set application cursor keys on by default",
    "Force application keypad mode on",
    "Force application keypad mode off",
    "Set application keypad on by default",
    "Enable bracketed-paste",
];

fn key_combo(ui: &mut egui::Ui, id: &str, label: &str, value: &mut String, any: bool) {
    ui.label(label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_str())
        .width(112.0)
        .height(300.0)
        .show_ui(ui, |ui| {
            let names = any
                .then_some(ANY_OTHER_KEY)
                .into_iter()
                .chain(RECONNECTION_KEYS.iter().copied());
            for name in names {
                if ui.selectable_label(value == name, name).clicked() {
                    *value = name.to_string();
                }
            }
        });
}

/// Draw the window; returns true when a setting changed.
pub fn show(ctx: &egui::Context, open: &mut bool, t: &mut TerminalSettings) -> bool {
    let before = t.clone();
    let mut close = false;
    egui::Window::new("Advanced terminal settings")
        .id(egui::Id::new("expert-terminal"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(700.0)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(361.0);
                    ui.group(|ui| features(ui, t));
                });
                ui.vertical(|ui| {
                    ui.set_width(217.0);
                    ui.group(|ui| copy_paste(ui, t));
                });
            });
            ui.add_space(8.0);
            ui.group(|ui| {
                ui.set_width(625.0);
                ui.strong("Reconnection keys").on_hover_text(
                    "When a remote session ends, the terminal offers to reconnect, save \
                     the output or close the tab; choose the keys for each.",
                );
                ui.horizontal(|ui| {
                    let e = &mut t.expert;
                    key_combo(
                        ui,
                        "key-reconnect",
                        "Reconnect:",
                        &mut e.reconnect_key,
                        false,
                    );
                    ui.add_space(16.0);
                    key_combo(ui, "key-save", "Save:", &mut e.save_key, false);
                    ui.add_space(16.0);
                    key_combo(ui, "key-exit", "Exit:", &mut e.exit_key, true);
                });
            });
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if ui
                    .button("Restore default settings")
                    .on_hover_text("Restore the default expert terminal settings")
                    .clicked()
                {
                    t.expert = ExpertTerminal::default();
                    t.copy_on_select = true;
                }
                ui.add_space(8.0);
                if ui
                    .add(egui::Button::new("OK").min_size(egui::vec2(82.0, 30.0)))
                    .clicked()
                {
                    close = true;
                }
            });
        });
    if close {
        *open = false;
    }
    *t != before
}

fn features(ui: &mut egui::Ui, t: &mut TerminalSettings) {
    ui.strong("Terminal features");
    egui::ScrollArea::vertical()
        .id_salt("expert-features")
        .max_height(110.0)
        .show(ui, |ui| {
            for (i, label) in FEATURES.iter().enumerate() {
                match feature(&mut t.expert, i) {
                    Some(FeatureRef::Flag(flag)) => {
                        ui.checkbox(flag, *label);
                    }
                    Some(FeatureRef::Force(mode, on)) => {
                        let mut checked = *mode == Some(on);
                        if ui.checkbox(&mut checked, *label).changed() {
                            *mode = checked.then_some(on);
                        }
                    }
                    None => {
                        ui.add_enabled(false, egui::Checkbox::new(&mut false, *label))
                            .on_disabled_hover_text(NOT_YET);
                    }
                }
            }
        });
    ui.add_space(4.0);
    egui::Grid::new("expert-grid")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Answerback to ^E:");
            ui.add(
                egui::TextEdit::singleline(&mut t.expert.answerback)
                    .desired_width(132.0)
                    .char_limit(64),
            );
            ui.end_row();
            ui.label("Bell Type:");
            egui::ComboBox::from_id_salt("expert-bell")
                .selected_text(t.expert.bell.label())
                .width(132.0)
                .show_ui(ui, |ui| {
                    for bell in BellType::ALL {
                        // Sounds are not played yet: those choices flash too.
                        let label = match bell {
                            BellType::Sound | BellType::Speaker => {
                                format!("{} (flashes for now)", bell.label())
                            }
                            _ => bell.label().to_string(),
                        };
                        ui.selectable_value(&mut t.expert.bell, bell, label);
                    }
                });
            ui.end_row();
            ui.label("Scrollback lines:");
            ui.add(
                egui::DragValue::new(&mut t.scrollback_lines)
                    .range(100..=360_000)
                    .speed(100),
            );
            ui.end_row();
        });
}

fn copy_paste(ui: &mut egui::Ui, t: &mut TerminalSettings) {
    ui.strong("Copy/paste settings").on_hover_text(
        "Double-clicking a word selects up to the nearest delimiter on each side. \
         Add characters to the default delimiters (for instance /-~_&) or remove some \
         (for instance '=:()[]), separately for the left and the right side.",
    );
    ui.checkbox(&mut t.copy_on_select, "Auto-copy selected text");
    ui.add_space(4.0);
    egui::Grid::new("expert-delims")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            let e = &mut t.expert;
            for (label, value) in [
                ("Add left delimiters:", &mut e.add_left_delimiters),
                ("Add right delimiters:", &mut e.add_right_delimiters),
                ("Remove left delimiters:", &mut e.remove_left_delimiters),
                ("Remove right delimiters:", &mut e.remove_right_delimiters),
            ] {
                ui.label(label);
                ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(36.0)
                        .char_limit(32),
                );
                ui.end_row();
            }
        });
}
