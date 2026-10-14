//! Keyboard shortcuts: matching key presses to actions (before the
//! terminal sees them) and the "Hotkeys edition" dialog.

use super::Action;
use std::collections::BTreeMap;
use tc_commands::Command;
use tc_core::hotkeys::{self, Hotkey, HotkeyAction};
use tc_core::settings::SplitMode;

/// egui key for a reference key name.
fn egui_key(name: &str) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match name {
        "LEFT" => K::ArrowLeft,
        "RIGHT" => K::ArrowRight,
        "UP" => K::ArrowUp,
        "DOWN" => K::ArrowDown,
        "SPACE" => K::Space,
        "HOME" => K::Home,
        "END" => K::End,
        "PAGE_UP" => K::PageUp,
        "PAGE_DOWN" => K::PageDown,
        "BACKSPACE" => K::Backspace,
        "DEL" => K::Delete,
        "ESC" => K::Escape,
        "TAB" => K::Tab,
        "RETURN" => K::Enter,
        "INSERT" => K::Insert,
        "ADD" => K::Plus,
        "SUBTRACT" => K::Minus,
        "DIVIDE" => K::Slash,
        "0" => K::Num0,
        "1" => K::Num1,
        "2" => K::Num2,
        "3" => K::Num3,
        "4" => K::Num4,
        "5" => K::Num5,
        "6" => K::Num6,
        "7" => K::Num7,
        "8" => K::Num8,
        "9" => K::Num9,
        other => return K::from_name(other),
    })
}

/// The shortcut matches this key press exactly (no extra modifiers).
fn matches(hk: &Hotkey, key: egui::Key, m: egui::Modifiers) -> bool {
    !hk.win
        && egui_key(&hk.key) == Some(key)
        && hk.ctrl == m.ctrl
        && hk.alt == m.alt
        && hk.shift == m.shift
}

fn to_action(a: HotkeyAction, active_tab: Option<u64>) -> Option<Action> {
    use HotkeyAction as H;
    Some(match a {
        H::NewTab => Action::Run(Command::TabOpen { session: None }),
        H::CloseTab => Action::Run(Command::TabClose { tab: None }),
        H::NextTab => Action::CycleTab(1),
        H::PreviousTab => Action::CycleTab(-1),
        H::DuplicateTab => Action::Run(Command::TabDuplicate { tab: None }),
        H::RenameTab => Action::RenameTab(active_tab?),
        H::NewSession => Action::NewSession,
        H::FindInTerminal => Action::FindInTerminal,
        H::FindNext => Action::FindNext,
        H::SaveTerminalText => Action::SaveTerminalText,
        H::Fullscreen => Action::ToggleFullscreen,
        H::ToggleSidebar => Action::ToggleSidebar,
        H::MultiExec => Action::ToggleMultiExec,
        H::SingleTerminal => Action::Split(SplitMode::Single),
        H::SplitVertical => Action::Split(SplitMode::Vertical),
        H::SplitHorizontal => Action::Split(SplitMode::Horizontal),
        H::FourTerminals => Action::Split(SplitMode::Quad),
        H::ZoomIn => Action::ZoomIn,
        H::ZoomOut => Action::ZoomOut,
        H::Quit => Action::Quit,
    })
}

/// Take this frame's shortcut key presses out of the input (so the
/// terminal never receives them) and return their actions.
pub fn take_shortcuts(
    ctx: &egui::Context,
    overrides: &BTreeMap<String, String>,
    active_tab: Option<u64>,
) -> Vec<Action> {
    let bound: Vec<(HotkeyAction, Hotkey)> = hotkeys::effective(overrides)
        .into_iter()
        .filter_map(|(a, hk)| hk.map(|hk| (a, hk)))
        .collect();
    if bound.is_empty() {
        return Vec::new();
    }
    let mut actions = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|ev| {
            let egui::Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } = ev
            else {
                return true;
            };
            let Some((action, _)) = bound.iter().find(|(_, hk)| matches(hk, *key, *modifiers))
            else {
                return true;
            };
            if *pressed && let Some(a) = to_action(*action, active_tab) {
                actions.push(a);
            }
            false
        });
    });
    actions
}

/// "Hotkeys edition". Returns true when a shortcut changed.
pub fn show(
    ctx: &egui::Context,
    open: &mut bool,
    overrides: &mut BTreeMap<String, String>,
) -> bool {
    let mut changed = false;
    let available = hotkeys::available();
    let mut keep_open = *open;
    egui::Window::new("Hotkeys edition")
        .id(egui::Id::new("hotkeys"))
        .open(&mut keep_open)
        .collapsible(false)
        .resizable(false)
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.label("Edit keyboard shortcut");
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    egui::Grid::new("hotkeys-grid")
                        .num_columns(2)
                        .striped(true)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            for action in HotkeyAction::ALL {
                                ui.label(action.label());
                                let current = overrides
                                    .get(action.id())
                                    .cloned()
                                    .unwrap_or_else(|| action.default_hotkey().to_string());
                                let mut selected = current.clone();
                                egui::ComboBox::from_id_salt(("hotkey", action.id()))
                                    .selected_text(&selected)
                                    .width(190.0)
                                    .height(300.0)
                                    .show_ui(ui, |ui| {
                                        for name in &available {
                                            ui.selectable_value(&mut selected, name.clone(), name);
                                        }
                                    });
                                if selected != current {
                                    if selected == action.default_hotkey() {
                                        overrides.remove(action.id());
                                    } else {
                                        overrides.insert(action.id().to_string(), selected);
                                    }
                                    changed = true;
                                }
                                ui.end_row();
                            }
                        });
                });
            for (hk, actions) in hotkeys::conflicts(overrides) {
                let names: Vec<&str> = actions.iter().map(|a| a.label()).collect();
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{} is used by: {}", hk.label(), names.join(", ")),
                );
            }
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    *open = false;
                }
                if ui.button("Restore defaults").clicked() && !overrides.is_empty() {
                    overrides.clear();
                    changed = true;
                }
            });
        });
    if !keep_open {
        *open = false;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_key_name_maps_to_an_egui_key() {
        let unmapped: Vec<String> = hotkeys::available()
            .into_iter()
            .skip(1)
            .filter_map(|n| Hotkey::parse(&n))
            .filter(|hk| egui_key(&hk.key).is_none())
            .map(|hk| hk.key)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        // PRINT and MULTIPLY have no egui key; they are offered like in
        // the reference but never fire.
        assert_eq!(unmapped, ["MULTIPLY", "PRINT"]);
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let hk = Hotkey::parse("Ctrl + TAB").unwrap();
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let ctrl_shift = egui::Modifiers {
            shift: true,
            ..ctrl
        };
        assert!(matches(&hk, egui::Key::Tab, ctrl));
        assert!(!matches(&hk, egui::Key::Tab, ctrl_shift));
        assert!(!matches(&hk, egui::Key::Tab, egui::Modifiers::default()));
    }
}
