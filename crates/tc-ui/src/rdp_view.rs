//! An RDP session inside a tab: the remote desktop drawn from the
//! session's picture, scaled down to fit (never up), and the keyboard /
//! mouse sent back. Printable text goes as Unicode characters (whatever
//! the local keyboard layout); other keys, and keys pressed with Ctrl or
//! Alt, go as PC scancodes. Paste (Ctrl+V) types the local clipboard.

use crate::term_view::TermOutput;
use egui::{Event, Key, Modifiers, Pos2, Rect, TextureHandle, TextureOptions, vec2};
use tc_rdp::{Button, RdpInput, RdpSession};

/// The live session of a tab and what was last sent to it.
pub struct RdpView {
    pub session: RdpSession,
    /// Logged in: the desktop replaces the terminal.
    pub connected: bool,
    texture: Option<TextureHandle>,
    generation: u64,
    /// Modifier keys held on the remote side.
    sent: Modifiers,
    pointer: Option<(u16, u16)>,
    had_focus: bool,
}

impl RdpView {
    pub fn new(session: RdpSession) -> Self {
        Self {
            session,
            connected: false,
            texture: None,
            generation: u64::MAX,
            sent: Modifiers::default(),
            pointer: None,
            had_focus: false,
        }
    }
}

const SHIFT: u16 = 0x2A;
const CTRL: u16 = 0x1D;
const ALT: u16 = 0x38;

/// PC/AT (set 1) scancode of a key; extended keys have 0xE0 in the high byte.
pub fn scancode(key: Key) -> Option<u16> {
    use Key::*;
    Some(match key {
        Escape => 0x01,
        Num1 => 0x02,
        Num2 => 0x03,
        Num3 => 0x04,
        Num4 => 0x05,
        Num5 => 0x06,
        Num6 => 0x07,
        Num7 => 0x08,
        Num8 => 0x09,
        Num9 => 0x0A,
        Num0 => 0x0B,
        Minus => 0x0C,
        Equals => 0x0D,
        Backspace => 0x0E,
        Tab => 0x0F,
        Q => 0x10,
        W => 0x11,
        E => 0x12,
        R => 0x13,
        T => 0x14,
        Y => 0x15,
        U => 0x16,
        I => 0x17,
        O => 0x18,
        P => 0x19,
        OpenBracket => 0x1A,
        CloseBracket => 0x1B,
        Enter => 0x1C,
        A => 0x1E,
        S => 0x1F,
        D => 0x20,
        F => 0x21,
        G => 0x22,
        H => 0x23,
        J => 0x24,
        K => 0x25,
        L => 0x26,
        Semicolon => 0x27,
        Quote => 0x28,
        Backtick => 0x29,
        Backslash => 0x2B,
        Z => 0x2C,
        X => 0x2D,
        C => 0x2E,
        V => 0x2F,
        B => 0x30,
        N => 0x31,
        M => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash => 0x35,
        Space => 0x39,
        F1 => 0x3B,
        F2 => 0x3C,
        F3 => 0x3D,
        F4 => 0x3E,
        F5 => 0x3F,
        F6 => 0x40,
        F7 => 0x41,
        F8 => 0x42,
        F9 => 0x43,
        F10 => 0x44,
        F11 => 0x57,
        F12 => 0x58,
        Home => 0xE047,
        ArrowUp => 0xE048,
        PageUp => 0xE049,
        ArrowLeft => 0xE04B,
        ArrowRight => 0xE04D,
        End => 0xE04F,
        ArrowDown => 0xE050,
        PageDown => 0xE051,
        Insert => 0xE052,
        Delete => 0xE053,
        _ => return None,
    })
}

/// Keys that type a character: sent as text unless Ctrl / Alt is held.
fn types_text(key: Key) -> bool {
    scancode(key).is_some_and(
        |code| matches!(code, 0x02..=0x0D | 0x10..=0x1B | 0x1E..=0x29 | 0x2B..=0x35 | 0x39),
    )
}

/// The image rectangle: centered, scaled down to fit `area`, never up.
pub fn fit(area: Rect, width: u16, height: u16) -> Rect {
    let (w, h) = (f32::from(width.max(1)), f32::from(height.max(1)));
    let scale = (area.width() / w).min(area.height() / h).min(1.0);
    Rect::from_center_size(area.center(), vec2(w * scale, h * scale))
}

/// A window position as remote desktop coordinates.
pub fn remote_pos(image: Rect, width: u16, height: u16, pos: Pos2) -> (u16, u16) {
    let x = (pos.x - image.left()) / image.width() * f32::from(width);
    let y = (pos.y - image.top()) / image.height() * f32::from(height);
    let clamp = |v: f32, max: u16| v.clamp(0.0, f32::from(max.saturating_sub(1))) as u16;
    (clamp(x, width), clamp(y, height))
}

pub fn show(ui: &mut egui::Ui, view: &mut RdpView, active: bool) -> TermOutput {
    let mut out = TermOutput::default();
    let area = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(area, egui::Sense::click_and_drag());
    ui.painter().rect_filled(area, 0.0, egui::Color32::BLACK);

    let (width, height, generation) = {
        let f = view.session.frame();
        (f.width, f.height, f.generation)
    };
    if width == 0 || height == 0 {
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "Waiting for the remote desktop …",
            egui::TextStyle::Body.resolve(ui.style()),
            egui::Color32::GRAY,
        );
        return out;
    }
    if generation != view.generation {
        let f = view.session.frame();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [usize::from(f.width), usize::from(f.height)],
            &f.rgba,
        );
        drop(f);
        match &mut view.texture {
            Some(t) => t.set(image, TextureOptions::LINEAR),
            None => {
                view.texture = Some(ui.ctx().load_texture("rdp", image, TextureOptions::LINEAR));
            }
        }
        view.generation = generation;
    }
    let image = fit(area, width, height);
    if let Some(t) = &view.texture {
        ui.painter().image(
            t.id(),
            image,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    // Focus: like a terminal, the active tab takes the keyboard.
    let nothing_focused = ui.memory(|m| m.focused().is_none());
    if response.clicked() || response.drag_started() {
        out.clicked = true;
        response.request_focus();
    } else if active && nothing_focused {
        response.request_focus();
    }
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        );
    });
    let focused = response.has_focus();
    if view.had_focus && !focused {
        view.session.send(RdpInput::ReleaseAll);
        view.sent = Modifiers::default();
    }
    view.had_focus = focused;

    let mut inputs = Vec::new();
    // The mouse, over the picture.
    if let Some(pos) = response.hover_pos().filter(|p| image.contains(*p)) {
        let at = remote_pos(image, width, height, pos);
        if view.pointer != Some(at) {
            view.pointer = Some(at);
            inputs.push(RdpInput::MouseMove { x: at.0, y: at.1 });
        }
    }
    let events = ui.input(|i| i.events.clone());
    let hovered = response.hovered();
    for event in &events {
        match event {
            Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } if image.contains(*pos) && (hovered || !pressed) => {
                let button = match button {
                    egui::PointerButton::Primary => Button::Left,
                    egui::PointerButton::Secondary => Button::Right,
                    egui::PointerButton::Middle => Button::Middle,
                    _ => continue,
                };
                inputs.push(RdpInput::Button {
                    button,
                    down: *pressed,
                });
            }
            Event::MouseWheel { unit, delta, .. } if hovered => {
                let per = match unit {
                    egui::MouseWheelUnit::Point => 1.0 / 40.0,
                    egui::MouseWheelUnit::Line => 1.0,
                    egui::MouseWheelUnit::Page => 3.0,
                };
                for (value, horizontal) in [(delta.y, false), (delta.x, true)] {
                    let units = (value * per * 120.0).round().clamp(-3600.0, 3600.0) as i16;
                    if units != 0 {
                        inputs.push(RdpInput::Wheel {
                            delta: units,
                            horizontal,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    if focused {
        let mods = ui.input(|i| i.modifiers);
        keyboard(view, &events, mods, &mut inputs);
    }
    for input in inputs {
        view.session.send(input);
    }
    out
}

fn keyboard(view: &mut RdpView, events: &[Event], mods: Modifiers, inputs: &mut Vec<RdpInput>) {
    let key = |inputs: &mut Vec<RdpInput>, scancode: u16, down: bool| {
        inputs.push(RdpInput::Key { scancode, down });
    };
    // Modifier keys follow egui's state (they have no events of their own).
    let ctrl = mods.ctrl || mods.command;
    for (now, before, code) in [
        (mods.shift, view.sent.shift, SHIFT),
        (ctrl, view.sent.ctrl, CTRL),
        (mods.alt, view.sent.alt, ALT),
    ] {
        if now != before {
            key(inputs, code, now);
        }
    }
    view.sent = Modifiers {
        shift: mods.shift,
        ctrl,
        alt: mods.alt,
        ..Modifiers::default()
    };
    let chord = ctrl || mods.alt;
    for event in events {
        match event {
            Event::Text(text) if !chord => {
                for ch in text.chars() {
                    inputs.push(RdpInput::Char { ch, down: true });
                    inputs.push(RdpInput::Char { ch, down: false });
                }
            }
            Event::Key {
                key: k, pressed, ..
            } => {
                if types_text(*k) && !chord {
                    continue; // comes as Event::Text
                }
                if let Some(code) = scancode(*k) {
                    key(inputs, code, *pressed);
                }
            }
            // egui turns Ctrl+C / Ctrl+X into these.
            Event::Copy | Event::Cut => {
                let code = if matches!(event, Event::Copy) {
                    0x2E
                } else {
                    0x2D
                };
                key(inputs, code, true);
                key(inputs, code, false);
            }
            // Ctrl+V: type the local clipboard (Ctrl released meanwhile).
            Event::Paste(text) => {
                if view.sent.ctrl {
                    key(inputs, CTRL, false);
                }
                for ch in text.chars().filter(|c| *c != '\r') {
                    if ch == '\n' {
                        key(inputs, 0x1C, true);
                        key(inputs, 0x1C, false);
                    } else {
                        inputs.push(RdpInput::Char { ch, down: true });
                        inputs.push(RdpInput::Char { ch, down: false });
                    }
                }
                if view.sent.ctrl {
                    key(inputs, CTRL, true);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scancodes() {
        assert_eq!(scancode(Key::A), Some(0x1E));
        assert_eq!(scancode(Key::ArrowLeft), Some(0xE04B));
        assert!(types_text(Key::A) && types_text(Key::Space));
        assert!(!types_text(Key::Enter) && !types_text(Key::F5));
    }

    #[test]
    fn picture_fits_and_maps_back() {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let big = fit(area, 1600, 1200);
        assert_eq!(big.size(), vec2(800.0, 600.0));
        assert_eq!(
            remote_pos(big, 1600, 1200, Pos2::new(400.0, 300.0)),
            (800, 600)
        );
        let small = fit(area, 400, 300);
        assert_eq!(small.size(), vec2(400.0, 300.0), "never scaled up");
        assert_eq!(remote_pos(small, 400, 300, Pos2::new(0.0, 0.0)), (0, 0));
    }

    #[test]
    fn typing_and_chords() {
        let session = RdpSession::connect(
            tc_rdp::RdpParams {
                host: "127.0.0.1".into(),
                port: 1,
                user: String::new(),
                domain: None,
                password: String::new().into(),
                width: 200,
                height: 200,
                pinned: None,
                keyboard_layout: 0,
                via: None,
            },
            std::sync::Arc::new(|| {}),
        );
        let mut view = RdpView::new(session);
        let mut inputs = Vec::new();
        let events = [
            Event::Text("é".into()),
            Event::Key {
                key: Key::E,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            },
        ];
        keyboard(&mut view, &events, Modifiers::default(), &mut inputs);
        assert_eq!(
            inputs,
            [
                RdpInput::Char {
                    ch: 'é',
                    down: true
                },
                RdpInput::Char {
                    ch: 'é',
                    down: false
                }
            ],
            "text as characters, the letter key not twice"
        );
        let mut inputs = Vec::new();
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let events = [Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: ctrl,
        }];
        keyboard(&mut view, &events, ctrl, &mut inputs);
        assert_eq!(
            inputs,
            [
                RdpInput::Key {
                    scancode: CTRL,
                    down: true
                },
                RdpInput::Key {
                    scancode: 0x1E,
                    down: true
                }
            ]
        );
    }
}
