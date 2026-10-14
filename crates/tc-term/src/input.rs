/// UI-independent key identifiers (the GUI maps its own key events to these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Encode a key press as the byte sequence an xterm would send.
/// `app_cursor` = DECCKM application cursor mode (arrows send `ESC O x`).
pub fn encode_key(key: Key, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let modifier_param =
        1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
    let csi_letter = |letter: char| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[1;{modifier_param}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[{n};{modifier_param}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };

    let bytes = match key {
        Key::Char(c) => {
            let mut out = Vec::new();
            if mods.alt {
                out.push(0x1b);
            }
            if mods.ctrl {
                out.push(ctrl_byte(c)?);
            } else {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            out
        }
        Key::Enter => b"\r".to_vec(),
        Key::Backspace => vec![if mods.ctrl { 0x08 } else { 0x7f }],
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Escape => b"\x1b".to_vec(),
        Key::Up => csi_letter('A'),
        Key::Down => csi_letter('B'),
        Key::Right => csi_letter('C'),
        Key::Left => csi_letter('D'),
        Key::Home => csi_letter('H'),
        Key::End => csi_letter('F'),
        Key::Insert => csi_tilde(2),
        Key::Delete => csi_tilde(3),
        Key::PageUp => csi_tilde(5),
        Key::PageDown => csi_tilde(6),
        Key::F(n @ 1..=4) => {
            let letter = (b'P' + n - 1) as char;
            if modifier_param > 1 {
                format!("\x1b[1;{modifier_param}{letter}").into_bytes()
            } else {
                format!("\x1bO{letter}").into_bytes()
            }
        }
        Key::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return None,
            };
            csi_tilde(code)
        }
    };
    Some(bytes)
}

/// Does `bytes` (what one key press sent) come from the key named `name`
/// (`RETURN`, `F5`, `PAGE_UP`, `A`, `7` …, as in the reconnection key
/// lists)? Letters match either case; cursor-mode variants of Home / End
/// match too.
pub fn key_name_matches(name: &str, bytes: &[u8]) -> bool {
    let key = match name {
        "RETURN" => return bytes == b"\r" || bytes == b"\n",
        "SPACE" => Key::Char(' '),
        "TAB" => Key::Tab,
        "ESC" => Key::Escape,
        "DEL" => Key::Delete,
        "HOME" => Key::Home,
        "END" => Key::End,
        "PAGE_UP" => Key::PageUp,
        "PAGE_DOWN" => Key::PageDown,
        "ADD" => Key::Char('+'),
        "SUBTRACT" => Key::Char('-'),
        "MULTIPLY" => Key::Char('*'),
        "DIVIDE" => Key::Char('/'),
        n if n.len() > 1 && n.starts_with('F') => match n[1..].parse() {
            Ok(f) => Key::F(f),
            Err(_) => return false,
        },
        n => {
            let mut chars = n.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => {
                    bytes.len() == 1 && bytes[0].eq_ignore_ascii_case(&(c as u8))
                }
                _ => false,
            };
        }
    };
    [false, true]
        .iter()
        .any(|&app| encode_key(key, Modifiers::default(), app).as_deref() == Some(bytes))
}

/// One key press, not a paste: a single byte or one escape sequence.
pub fn is_single_key(bytes: &[u8]) -> bool {
    match bytes {
        [_] => true,
        [0x1b, rest @ ..] => !rest.is_empty() && !rest.contains(&0x1b) && rest.len() <= 7,
        _ => std::str::from_utf8(bytes).is_ok_and(|s| s.chars().count() == 1),
    }
}

fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '-' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn key_names() {
        assert!(key_name_matches("RETURN", b"\r"));
        assert!(key_name_matches("R", b"r") && key_name_matches("R", b"R"));
        assert!(!key_name_matches("R", b"rr"));
        assert!(key_name_matches("F5", b"\x1b[15~"));
        assert!(key_name_matches("F1", b"\x1bOP"));
        assert!(key_name_matches("HOME", b"\x1b[H") && key_name_matches("HOME", b"\x1bOH"));
        assert!(key_name_matches("PAGE_DOWN", b"\x1b[6~"));
        assert!(key_name_matches("7", b"7") && key_name_matches("ADD", b"+"));
        assert!(!key_name_matches("F99", b"x") && !key_name_matches("Any other key", b"x"));
        assert!(is_single_key(b"x") && is_single_key(b"\x1b[15~") && is_single_key("é".as_bytes()));
        assert!(!is_single_key(b"ls\r") && !is_single_key(b"\x1b[A\x1b[A"));
    }

    #[test]
    fn ctrl_c() {
        let m = Modifiers {
            ctrl: true,
            ..none()
        };
        assert_eq!(encode_key(Key::Char('c'), m, false), Some(vec![3]));
    }

    #[test]
    fn arrows_respect_app_cursor() {
        assert_eq!(encode_key(Key::Up, none(), false).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(Key::Up, none(), true).unwrap(), b"\x1bOA");
        let m = Modifiers {
            ctrl: true,
            ..none()
        };
        assert_eq!(encode_key(Key::Right, m, true).unwrap(), b"\x1b[1;5C");
    }

    #[test]
    fn function_keys() {
        assert_eq!(encode_key(Key::F(1), none(), false).unwrap(), b"\x1bOP");
        assert_eq!(encode_key(Key::F(5), none(), false).unwrap(), b"\x1b[15~");
    }

    #[test]
    fn unicode_passthrough() {
        assert_eq!(
            encode_key(Key::Char('ç'), none(), false).unwrap(),
            "ç".as_bytes()
        );
    }
}
