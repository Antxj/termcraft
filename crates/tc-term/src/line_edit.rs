//! "Local line editing": the line is typed and edited here, and sent with
//! Enter. Backspace / Delete, ^U (whole line) and ^W (last word) edit it;
//! other control keys and escape sequences (arrows, function keys) are
//! sent at once.

#[derive(Debug, Default)]
pub struct LineEditor {
    line: String,
}

/// What one input does.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Edit {
    /// Bytes for the remote side.
    pub send: Vec<u8>,
    /// Bytes to show in the terminal (the local echo of the edit).
    pub shown: Vec<u8>,
}

/// Longest line kept; more input is sent as it comes.
const MAX_LINE: usize = 64 * 1024;

impl LineEditor {
    /// The line typed so far.
    pub fn line(&self) -> &str {
        &self.line
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Edit {
        let mut e = Edit::default();
        if bytes.first() == Some(&0x1b) {
            e.send.extend_from_slice(bytes);
            return e;
        }
        for c in String::from_utf8_lossy(bytes).chars() {
            match c {
                '\r' | '\n' => {
                    e.send
                        .append(&mut std::mem::take(&mut self.line).into_bytes());
                    e.send.push(b'\r');
                    e.shown.extend_from_slice(b"\r\n");
                }
                '\x7f' | '\x08' => {
                    if self.line.pop().is_some() {
                        e.shown.extend_from_slice(b"\x08 \x08");
                    }
                }
                '\x15' => {
                    for _ in self.line.chars() {
                        e.shown.extend_from_slice(b"\x08 \x08");
                    }
                    self.line.clear();
                }
                '\x17' => {
                    let kept = self
                        .line
                        .trim_end()
                        .trim_end_matches(|c: char| !c.is_whitespace());
                    let erased = self.line[kept.len()..].chars().count();
                    self.line.truncate(kept.len());
                    for _ in 0..erased {
                        e.shown.extend_from_slice(b"\x08 \x08");
                    }
                }
                '\t' => self.push(c, &mut e),
                c if c.is_control() => {
                    let mut buf = [0; 4];
                    e.send.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                c => self.push(c, &mut e),
            }
        }
        e
    }

    fn push(&mut self, c: char, e: &mut Edit) {
        let mut buf = [0; 4];
        let s = c.encode_utf8(&mut buf);
        if self.line.len() >= MAX_LINE {
            e.send.extend_from_slice(s.as_bytes());
        } else {
            self.line.push(c);
        }
        e.shown.extend_from_slice(s.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_sent_on_enter() {
        let mut ed = LineEditor::default();
        let e = ed.feed(b"ls -l");
        assert!(e.send.is_empty());
        assert_eq!(e.shown, b"ls -l");
        assert_eq!(ed.line(), "ls -l");
        let e = ed.feed(b"\x7fa\r");
        assert_eq!(e.send, b"ls -a\r");
        assert_eq!(e.shown, b"\x08 \x08a\r\n");
        assert_eq!(ed.line(), "");
    }

    #[test]
    fn kill_line_and_word() {
        let mut ed = LineEditor::default();
        ed.feed("echo héllo wörld  ".as_bytes());
        let e = ed.feed(b"\x17");
        assert_eq!(ed.line(), "echo héllo ");
        assert_eq!(e.shown.len(), 7 * 3, "one erase per character");
        let e = ed.feed(b"\x15");
        assert_eq!(ed.line(), "");
        assert_eq!(e.shown.len(), 11 * 3);
        // Backspace on an empty line shows nothing.
        assert!(ed.feed(b"\x08").shown.is_empty());
    }

    #[test]
    fn control_keys_and_sequences_go_at_once() {
        let mut ed = LineEditor::default();
        ed.feed(b"abc");
        assert_eq!(ed.feed(b"\x03").send, b"\x03");
        assert_eq!(ed.feed(b"\x1b[A").send, b"\x1b[A");
        assert_eq!(ed.line(), "abc", "the line is kept");
    }

    #[test]
    fn pasted_lines_each_go_with_their_enter() {
        let mut ed = LineEditor::default();
        assert_eq!(ed.feed(b"one\rtwo\rthr").send, b"one\rtwo\r");
        assert_eq!(ed.line(), "thr");
    }
}
