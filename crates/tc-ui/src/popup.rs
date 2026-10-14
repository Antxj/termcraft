//! Popup ("quake") terminal: a system-wide hotkey shows or hides a local
//! terminal at a screen edge, above other windows (Configuration › Misc
//! "Popup terminal"; hotkey in "Keyboard shortcuts").

use crate::tab::{Tab, TabEnv};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tc_core::hotkeys::Hotkey;
use tc_core::settings::{PopupSettings, TerminalSettings};

/// Tab id of the popup terminal (never one of the window's tabs).
const POPUP_TAB_ID: u64 = u64::MAX - 7;

/// The system key of a reference key name.
fn code(key: &str) -> Option<Code> {
    use Code::*;
    let k = key.to_ascii_uppercase();
    Some(match k.as_str() {
        "F1" => F1,
        "F2" => F2,
        "F3" => F3,
        "F4" => F4,
        "F5" => F5,
        "F6" => F6,
        "F7" => F7,
        "F8" => F8,
        "F9" => F9,
        "F10" => F10,
        "F11" => F11,
        "F12" => F12,
        "LEFT" => ArrowLeft,
        "RIGHT" => ArrowRight,
        "UP" => ArrowUp,
        "DOWN" => ArrowDown,
        "SPACE" => Space,
        "HOME" => Home,
        "END" => End,
        "PAGE_UP" => PageUp,
        "PAGE_DOWN" => PageDown,
        "BACKSPACE" => Backspace,
        "DEL" => Delete,
        "ESC" => Escape,
        "TAB" => Tab,
        "RETURN" => Enter,
        "PRINT" => PrintScreen,
        "INSERT" => Insert,
        "ADD" => NumpadAdd,
        "SUBTRACT" => NumpadSubtract,
        "MULTIPLY" => NumpadMultiply,
        "DIVIDE" => NumpadDivide,
        "0" => Digit0,
        "1" => Digit1,
        "2" => Digit2,
        "3" => Digit3,
        "4" => Digit4,
        "5" => Digit5,
        "6" => Digit6,
        "7" => Digit7,
        "8" => Digit8,
        "9" => Digit9,
        "A" => KeyA,
        "B" => KeyB,
        "C" => KeyC,
        "D" => KeyD,
        "E" => KeyE,
        "F" => KeyF,
        "G" => KeyG,
        "H" => KeyH,
        "I" => KeyI,
        "J" => KeyJ,
        "K" => KeyK,
        "L" => KeyL,
        "M" => KeyM,
        "N" => KeyN,
        "O" => KeyO,
        "P" => KeyP,
        "Q" => KeyQ,
        "R" => KeyR,
        "S" => KeyS,
        "T" => KeyT,
        "U" => KeyU,
        "V" => KeyV,
        "W" => KeyW,
        "X" => KeyX,
        "Y" => KeyY,
        "Z" => KeyZ,
        _ => return None,
    })
}

/// A shortcut of the settings as a system hotkey.
pub fn system_hotkey(hk: &Hotkey) -> Option<HotKey> {
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::CONTROL, hk.ctrl);
    mods.set(Modifiers::ALT, hk.alt);
    mods.set(Modifiers::SHIFT, hk.shift);
    mods.set(Modifiers::SUPER, hk.win);
    Some(HotKey::new(Some(mods), code(&hk.key)?))
}

pub struct Popup {
    manager: Option<GlobalHotKeyManager>,
    registered: Option<HotKey>,
    presses: crossbeam_channel::Receiver<GlobalHotKeyEvent>,
    pub tab: Option<Tab>,
    pub visible: bool,
    /// Why the hotkey could not be registered (shown in the settings).
    pub error: Option<String>,
}

impl Popup {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, presses) = crossbeam_channel::unbounded();
        let ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            let _ = tx.send(e);
            ctx.request_repaint();
        }));
        let (manager, error) = match GlobalHotKeyManager::new() {
            Ok(m) => (Some(m), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Self {
            manager,
            registered: None,
            presses,
            tab: None,
            visible: false,
            error,
        }
    }

    /// Register the configured hotkey (no-op when unchanged).
    pub fn sync_hotkey(&mut self, wanted: Option<Hotkey>) {
        let wanted = wanted.as_ref().and_then(system_hotkey);
        if wanted == self.registered {
            return;
        }
        let Some(manager) = &self.manager else { return };
        if let Some(old) = self.registered.take() {
            let _ = manager.unregister(old);
        }
        if let Some(hk) = wanted {
            match manager.register(hk) {
                Ok(()) => {
                    self.registered = Some(hk);
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(format!("the popup terminal hotkey is not available: {e}"))
                }
            }
        }
    }

    /// The hotkey was pressed since the last frame.
    fn pressed(&self) -> bool {
        let id = self.registered.map(|h| h.id());
        self.presses
            .try_iter()
            .any(|e| Some(e.id) == id && e.state == HotKeyState::Pressed)
    }

    /// Show / hide on the hotkey; a hidden terminal is kept or closed.
    pub fn handle(&mut self, settings: &PopupSettings, env: impl FnOnce() -> TabEnv) {
        if !self.pressed() {
            return;
        }
        if self.visible {
            self.visible = false;
            if !settings.hide_instead_of_close {
                self.tab = None;
            }
        } else {
            if self.tab.as_ref().is_none_or(|t| !t.is_alive()) {
                self.tab = Some(Tab::open(POPUP_TAB_ID, None, &env()));
            }
            self.visible = true;
        }
    }

    /// Draw the popup window when visible.
    pub fn show(&mut self, ctx: &egui::Context, settings: &PopupSettings, term: &TerminalSettings) {
        if !self.visible {
            return;
        }
        let Some(tab) = &mut self.tab else { return };
        tab.pump();
        let screen = ctx
            .input(|i| i.viewport().monitor_size)
            .unwrap_or(egui::vec2(1280.0, 800.0));
        let (x, y, w, h) = settings.position.rect((screen.x, screen.y), settings.size);
        let transparency = settings
            .transparency
            .min(tc_core::settings::MAX_TRANSPARENCY);
        let floating = settings.position == tc_core::settings::PopupPosition::Floating;
        let builder = egui::ViewportBuilder::default()
            .with_title("TermCraft popup terminal")
            .with_position([x, y])
            .with_inner_size([w, h])
            .with_decorations(floating)
            .with_resizable(settings.sizeable || floating)
            .with_always_on_top()
            .with_taskbar(false)
            .with_transparent(transparency > 0);
        let font = term.font_size;
        let closed = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("popup-terminal"),
            builder,
            |ui, _| {
                if transparency > 0 {
                    ui.set_opacity(1.0 - f32::from(transparency) / 100.0);
                }
                let out = crate::term_view::show(ui, tab, font, true, term);
                if let Some(text) = out.paste {
                    tab.paste(&text);
                }
                for bytes in out.input {
                    tab.send(&bytes);
                }
                ui.ctx().input(|i| i.viewport().close_requested())
            },
        );
        if closed || !tab.is_alive() {
            self.visible = false;
            if closed && !settings.hide_instead_of_close {
                self.tab = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_and_rects() {
        let hk = Hotkey::parse("Ctrl + Alt + F12").unwrap();
        assert_eq!(
            system_hotkey(&hk),
            Some(HotKey::new(
                Some(Modifiers::CONTROL | Modifiers::ALT),
                Code::F12
            ))
        );
        assert!(system_hotkey(&Hotkey::parse("Ctrl + PAGE_UP").unwrap()).is_some());
        use tc_core::settings::PopupPosition as P;
        assert_eq!(
            P::Bottom.rect((1000.0, 800.0), 25),
            (0.0, 600.0, 1000.0, 200.0)
        );
        assert_eq!(
            P::Right.rect((1000.0, 800.0), 50),
            (500.0, 0.0, 500.0, 800.0)
        );
        let (x, y, w, h) = P::Floating.rect((1000.0, 800.0), 50);
        assert_eq!((x + w / 2.0, y + h / 2.0), (500.0, 400.0), "centered");
    }
}
