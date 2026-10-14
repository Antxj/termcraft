//! Keyboard shortcuts ("Settings › Keyboard shortcuts", reference
//! `TFORMHOTKEYS`): application actions, their defaults, and the
//! reference notation (`Ctrl + Shift + T`, `Alt + F4`, `<none>`).

use std::collections::BTreeMap;

/// A key combination. `key` is the reference key name (`T`, `F11`,
/// `LEFT`, `PAGE_UP`, `TAB`…).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: String,
}

pub const NONE: &str = "<none>";

/// Keys that can be combined with modifiers, in the reference order.
const NAMED_KEYS: &[&str] = &[
    "LEFT",
    "RIGHT",
    "UP",
    "DOWN",
    "SPACE",
    "HOME",
    "END",
    "PAGE_UP",
    "PAGE_DOWN",
    "BACKSPACE",
    "DEL",
    "ESC",
    "TAB",
    "RETURN",
    "PRINT",
];

impl Hotkey {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text == NONE {
            return None;
        }
        let mut hk = Hotkey {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            key: String::new(),
        };
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key, mods) = parts.split_last()?;
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => hk.ctrl = true,
                "alt" => hk.alt = true,
                "shift" => hk.shift = true,
                "win" => hk.win = true,
                _ => return None,
            }
        }
        let key = key.to_ascii_uppercase();
        if key.is_empty() {
            return None;
        }
        hk.key = key;
        Some(hk)
    }

    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("WIN");
        }
        parts.push(&self.key);
        parts.join(" + ")
    }
}

/// Every combination offered in the editor (like the reference list):
/// F-keys alone, then Ctrl / Alt / Ctrl+Alt / Ctrl+Shift combinations with
/// F-keys, letters, digits and named keys, then a few others.
pub fn available() -> Vec<String> {
    let f_keys: Vec<String> = (1..=12).map(|n| format!("F{n}")).collect();
    let letters: Vec<String> = ('A'..='Z').map(String::from).collect();
    let digits: Vec<String> = ('1'..='9').chain(['0']).map(String::from).collect();
    let mut out = vec![NONE.to_string()];
    out.extend(f_keys.iter().cloned());
    for prefix in ["Ctrl", "Alt", "Ctrl + Alt", "Ctrl + Shift"] {
        for k in f_keys.iter().chain(&letters).chain(&digits) {
            out.push(format!("{prefix} + {k}"));
        }
        for k in NAMED_KEYS {
            out.push(format!("{prefix} + {k}"));
        }
    }
    out.push("Ctrl + Shift + INSERT".into());
    for k in digits.iter().chain(f_keys.iter().skip(1)) {
        out.push(format!("WIN + {k}"));
    }
    for k in NAMED_KEYS {
        out.push(format!("Shift + {k}"));
    }
    out.push("Shift + INSERT".into());
    out.push("Ctrl + INSERT".into());
    // Numeric keypad operators.
    for prefix in ["Ctrl", "Alt", "Ctrl + Alt", "Ctrl + Shift", "Shift"] {
        for k in ["ADD", "SUBTRACT", "MULTIPLY", "DIVIDE"] {
            out.push(format!("{prefix} + {k}"));
        }
    }
    out
}

/// Application actions that can have a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HotkeyAction {
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    DuplicateTab,
    RenameTab,
    NewSession,
    FindInTerminal,
    FindNext,
    SaveTerminalText,
    Fullscreen,
    CompactMode,
    ToggleSidebar,
    MultiExec,
    SingleTerminal,
    SplitVertical,
    SplitHorizontal,
    FourTerminals,
    ZoomIn,
    ZoomOut,
    Quit,
    /// System-wide: show / hide the popup terminal.
    PopupTerminal,
}

impl HotkeyAction {
    pub const ALL: [HotkeyAction; 22] = [
        Self::NewTab,
        Self::CloseTab,
        Self::NextTab,
        Self::PreviousTab,
        Self::DuplicateTab,
        Self::RenameTab,
        Self::NewSession,
        Self::FindInTerminal,
        Self::FindNext,
        Self::SaveTerminalText,
        Self::Fullscreen,
        Self::CompactMode,
        Self::ToggleSidebar,
        Self::MultiExec,
        Self::SingleTerminal,
        Self::SplitVertical,
        Self::SplitHorizontal,
        Self::FourTerminals,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::Quit,
        Self::PopupTerminal,
    ];

    /// Stable id for the settings file.
    pub fn id(self) -> &'static str {
        match self {
            Self::NewTab => "new_tab",
            Self::CloseTab => "close_tab",
            Self::NextTab => "next_tab",
            Self::PreviousTab => "previous_tab",
            Self::DuplicateTab => "duplicate_tab",
            Self::RenameTab => "rename_tab",
            Self::NewSession => "new_session",
            Self::FindInTerminal => "find_in_terminal",
            Self::FindNext => "find_next",
            Self::SaveTerminalText => "save_terminal_text",
            Self::Fullscreen => "fullscreen",
            Self::CompactMode => "compact_mode",
            Self::ToggleSidebar => "toggle_sidebar",
            Self::MultiExec => "multiexec",
            Self::SingleTerminal => "single_terminal",
            Self::SplitVertical => "split_vertical",
            Self::SplitHorizontal => "split_horizontal",
            Self::FourTerminals => "four_terminals",
            Self::ZoomIn => "zoom_in",
            Self::ZoomOut => "zoom_out",
            Self::Quit => "quit",
            Self::PopupTerminal => "popup_terminal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NewTab => "Open new tab",
            Self::CloseTab => "Close current tab",
            Self::NextTab => "Go to next tab",
            Self::PreviousTab => "Go to previous tab",
            Self::DuplicateTab => "Duplicate current tab",
            Self::RenameTab => "Set tab title",
            Self::NewSession => "New session",
            Self::FindInTerminal => "Find in terminal",
            Self::FindNext => "Find next",
            Self::SaveTerminalText => "Save terminal text",
            Self::Fullscreen => "Fullscreen mode",
            Self::CompactMode => "Compact mode",
            Self::ToggleSidebar => "Show / hide the sidebar",
            Self::MultiExec => "Write commands on all terminals",
            Self::SingleTerminal => "Single terminal mode",
            Self::SplitVertical => "2 terminals mode (vertical split)",
            Self::SplitHorizontal => "2 terminals mode (horizontal split)",
            Self::FourTerminals => "4 terminals mode",
            Self::ZoomIn => "Increase font size",
            Self::ZoomOut => "Decrease font size",
            Self::Quit => "Quit",
            Self::PopupTerminal => "Show / hide the popup terminal (system-wide)",
        }
    }

    /// Default shortcut. Combinations a shell needs (Ctrl+letter, Alt+letter)
    /// are left free.
    pub fn default_hotkey(self) -> &'static str {
        match self {
            Self::NewTab => "Ctrl + Shift + T",
            Self::CloseTab => "Ctrl + Shift + W",
            Self::NextTab => "Ctrl + TAB",
            Self::PreviousTab => "Ctrl + Shift + TAB",
            Self::DuplicateTab => "Ctrl + Shift + D",
            Self::RenameTab => NONE,
            Self::NewSession => "Ctrl + Shift + N",
            Self::FindInTerminal => "Ctrl + Shift + F",
            Self::FindNext => "F3",
            Self::SaveTerminalText => NONE,
            Self::Fullscreen => "F11",
            Self::CompactMode => "Ctrl + Shift + F11",
            Self::ToggleSidebar => NONE,
            Self::MultiExec => NONE,
            Self::SingleTerminal => NONE,
            Self::SplitVertical => NONE,
            Self::SplitHorizontal => NONE,
            Self::FourTerminals => NONE,
            Self::ZoomIn => "Ctrl + Shift + ADD",
            Self::ZoomOut => "Ctrl + Shift + SUBTRACT",
            Self::Quit => NONE,
            Self::PopupTerminal => "Ctrl + Alt + F12",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// The effective shortcut of every action: `overrides` (from the settings,
/// action id → hotkey text, `<none>` to remove) over the defaults.
pub fn effective(overrides: &BTreeMap<String, String>) -> Vec<(HotkeyAction, Option<Hotkey>)> {
    HotkeyAction::ALL
        .into_iter()
        .map(|a| {
            let text = overrides
                .get(a.id())
                .map_or(a.default_hotkey(), String::as_str);
            (a, Hotkey::parse(text))
        })
        .collect()
}

/// Actions sharing a shortcut (shown as a warning in the editor).
pub fn conflicts(overrides: &BTreeMap<String, String>) -> Vec<(Hotkey, Vec<HotkeyAction>)> {
    let mut by_key: BTreeMap<String, (Hotkey, Vec<HotkeyAction>)> = BTreeMap::new();
    for (action, hk) in effective(overrides) {
        if let Some(hk) = hk {
            by_key
                .entry(hk.label())
                .or_insert_with(|| (hk.clone(), Vec::new()))
                .1
                .push(action);
        }
    }
    by_key.into_values().filter(|(_, a)| a.len() > 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_label_round_trip() {
        let hk = Hotkey::parse("ctrl + shift + t").unwrap();
        assert!(hk.ctrl && hk.shift && !hk.alt);
        assert_eq!(hk.label(), "Ctrl + Shift + T");
        assert_eq!(Hotkey::parse("F3").unwrap().label(), "F3");
        assert_eq!(Hotkey::parse(NONE), None);
        assert_eq!(Hotkey::parse("Hyper + X"), None);
        assert_eq!(Hotkey::parse(""), None);
    }

    #[test]
    fn available_list_matches_the_reference_shape() {
        let list = available();
        assert_eq!(list[0], NONE);
        assert!(list.contains(&"Ctrl + Alt + PAGE_DOWN".into()));
        assert!(list.contains(&"Shift + INSERT".into()));
        for a in HotkeyAction::ALL {
            let d = a.default_hotkey();
            assert!(
                list.contains(&d.to_string()),
                "{a:?} default {d} is not offered"
            );
            assert_eq!(HotkeyAction::from_id(a.id()), Some(a));
        }
    }

    #[test]
    fn defaults_have_no_conflicts_and_overrides_apply() {
        assert!(conflicts(&BTreeMap::new()).is_empty());
        let mut o = BTreeMap::new();
        o.insert("quit".to_string(), "Ctrl + Shift + T".to_string());
        assert_eq!(conflicts(&o).len(), 1);
        o.insert("new_tab".to_string(), NONE.to_string());
        let eff = effective(&o);
        assert_eq!(eff[0], (HotkeyAction::NewTab, None));
        assert!(conflicts(&o).is_empty());
    }
}
