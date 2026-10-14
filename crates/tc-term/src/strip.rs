//! Plain text from terminal output, for log files: escape sequences (CSI,
//! OSC, DCS…) removed, `\r\n` → `\n`, lone `\r` and other control bytes
//! dropped. Streaming: sequences may be split across reads.

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Text,
    Esc,
    /// `ESC [` … final byte.
    Csi,
    /// `ESC ]` / `ESC P` / `ESC _` / `ESC ^` … BEL or ST.
    String,
    /// ESC inside a string (maybe ST).
    StringEsc,
    /// `ESC (`-style: one more byte.
    Charset,
    /// A CR, waiting to see whether LF follows.
    Cr,
}

#[derive(Debug, Default, Clone)]
pub struct AnsiStripper {
    state: State,
}

impl AnsiStripper {
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            self.state = match self.state {
                State::Text | State::Cr => {
                    let was_cr = self.state == State::Cr;
                    match b {
                        0x1b => State::Esc,
                        b'\n' => {
                            out.push(b'\n');
                            State::Text
                        }
                        b'\r' => State::Cr,
                        b'\t' => {
                            out.push(b);
                            State::Text
                        }
                        // A carriage return not followed by LF rewrites the
                        // line; keep the log readable by starting a new one.
                        _ if was_cr && b >= 0x20 => {
                            out.push(b'\n');
                            out.push(b);
                            State::Text
                        }
                        0x00..=0x1f | 0x7f => State::Text,
                        _ => {
                            out.push(b);
                            State::Text
                        }
                    }
                }
                State::Esc => match b {
                    b'[' => State::Csi,
                    b']' | b'P' | b'_' | b'^' | b'X' => State::String,
                    b'(' | b')' | b'*' | b'+' | b'#' | b'%' => State::Charset,
                    _ => State::Text,
                },
                State::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        State::Text
                    } else {
                        State::Csi
                    }
                }
                State::String => match b {
                    0x07 => State::Text,
                    0x1b => State::StringEsc,
                    _ => State::String,
                },
                State::StringEsc => {
                    if b == b'\\' {
                        State::Text
                    } else {
                        State::String
                    }
                }
                State::Charset => State::Text,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(chunks: &[&[u8]]) -> String {
        let mut s = AnsiStripper::default();
        let mut out = Vec::new();
        for c in chunks {
            s.feed(c, &mut out);
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn removes_sequences_and_keeps_text() {
        assert_eq!(
            strip(&[b"\x1b[1;31mred\x1b[0m ok\r\n", b"\x1b]0;title\x07next\r\n"]),
            "red ok\nnext\n"
        );
        assert_eq!(
            strip(&[b"a\x1b[3", b"8;5;1mb\x1b]7;file://h/x\x1b", b"\\c"]),
            "abc"
        );
        assert_eq!(strip(&[b"\x1b(Bplain\x08\x07"]), "plain");
        assert_eq!(strip(&[b"50%\r", b"100%\r\n"]), "50%\n100%\n");
        assert_eq!("é".len(), 2);
        assert_eq!(strip(&["café\n".as_bytes()]), "café\n", "UTF-8 is kept");
    }
}
