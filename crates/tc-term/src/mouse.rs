//! Mouse reporting to programs that ask for it (vim, htop, tmux…): xterm
//! X10 / normal / button-motion / any-motion modes, with the default,
//! UTF-8 and SGR encodings.

use crate::Modifiers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub button: MouseButton,
    pub action: MouseAction,
    /// 0-based cell.
    pub row: u16,
    pub col: u16,
    pub mods: Modifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Off,
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Default,
    Utf8,
    Sgr,
}

pub(crate) fn encode(ev: MouseEvent, mode: Mode, encoding: Encoding) -> Option<Vec<u8>> {
    let wheel = matches!(ev.button, MouseButton::WheelUp | MouseButton::WheelDown);
    let wanted = match (mode, ev.action) {
        (Mode::Off, _) => false,
        (Mode::Press, MouseAction::Press) => true,
        (Mode::Press, _) => false,
        (Mode::PressRelease, MouseAction::Motion) => false,
        (Mode::ButtonMotion, MouseAction::Motion) => ev.button != MouseButton::None,
        _ => true,
    };
    if !wanted || (wheel && ev.action == MouseAction::Release) {
        return None;
    }
    let mut code: u32 = match ev.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if mode != Mode::Press {
        if ev.mods.shift {
            code += 4;
        }
        if ev.mods.alt {
            code += 8;
        }
        if ev.mods.ctrl {
            code += 16;
        }
    }
    if ev.action == MouseAction::Motion {
        code += 32;
    }
    let (x, y) = (u32::from(ev.col) + 1, u32::from(ev.row) + 1);
    match encoding {
        Encoding::Sgr => {
            let end = if ev.action == MouseAction::Release {
                'm'
            } else {
                'M'
            };
            Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes())
        }
        Encoding::Default | Encoding::Utf8 => {
            // Classic encoding has no button for releases.
            if ev.action == MouseAction::Release {
                code = (code & !3) | 3;
            }
            let mut out = b"\x1b[M".to_vec();
            out.push(u8::try_from(32 + code).ok()?);
            for v in [x, y] {
                let n = 32 + v;
                if encoding == Encoding::Default {
                    out.push(u8::try_from(n).ok()?); // > 223 cannot be sent
                } else {
                    let c = char::from_u32(n).filter(|_| n < 2048)?;
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(button: MouseButton, action: MouseAction, row: u16, col: u16) -> MouseEvent {
        MouseEvent {
            button,
            action,
            row,
            col,
            mods: Modifiers::default(),
        }
    }

    #[test]
    fn sgr_press_release_and_wheel() {
        let press = ev(MouseButton::Left, MouseAction::Press, 4, 9);
        assert_eq!(
            encode(press, Mode::PressRelease, Encoding::Sgr).unwrap(),
            b"\x1b[<0;10;5M"
        );
        let release = ev(MouseButton::Left, MouseAction::Release, 4, 9);
        assert_eq!(
            encode(release, Mode::PressRelease, Encoding::Sgr).unwrap(),
            b"\x1b[<0;10;5m"
        );
        let wheel = ev(MouseButton::WheelDown, MouseAction::Press, 0, 0);
        assert_eq!(
            encode(wheel, Mode::PressRelease, Encoding::Sgr).unwrap(),
            b"\x1b[<65;1;1M"
        );
    }

    #[test]
    fn modes_filter_events() {
        let motion = ev(MouseButton::None, MouseAction::Motion, 1, 1);
        assert!(encode(motion, Mode::ButtonMotion, Encoding::Sgr).is_none());
        assert!(encode(motion, Mode::AnyMotion, Encoding::Sgr).is_some());
        let drag = ev(MouseButton::Left, MouseAction::Motion, 1, 1);
        assert_eq!(
            encode(drag, Mode::ButtonMotion, Encoding::Sgr).unwrap(),
            b"\x1b[<32;2;2M"
        );
        let release = ev(MouseButton::Left, MouseAction::Release, 1, 1);
        assert!(encode(release, Mode::Press, Encoding::Sgr).is_none());
        assert!(encode(release, Mode::Off, Encoding::Sgr).is_none());
    }

    #[test]
    fn classic_encodings() {
        let press = ev(MouseButton::Right, MouseAction::Press, 0, 0);
        assert_eq!(
            encode(press, Mode::PressRelease, Encoding::Default).unwrap(),
            b"\x1b[M\x22\x21\x21"
        );
        let release = ev(MouseButton::Right, MouseAction::Release, 0, 0);
        assert_eq!(
            encode(release, Mode::PressRelease, Encoding::Default).unwrap(),
            b"\x1b[M\x23\x21\x21"
        );
        let far = ev(MouseButton::Left, MouseAction::Press, 0, 300);
        assert!(encode(far, Mode::PressRelease, Encoding::Default).is_none());
        assert!(encode(far, Mode::PressRelease, Encoding::Utf8).is_some());
    }
}
