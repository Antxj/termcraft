//! Output rewriting for the "Expert settings" switches, before the parser:
//! implicit CR / LF, and dropping what the user switched off (alternate
//! screen, scrollback clearing, character set changes) or making BS
//! destructive. Sequences split across reads are kept until complete;
//! strings (OSC, DCS, APC…) pass through untouched.

/// What to rewrite. All off = bytes pass unchanged (fast path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub implicit_cr_in_lf: bool,
    pub implicit_lf_in_cr: bool,
    /// Drop `CSI ? 47 / 1047 / 1049 h|l`.
    pub no_alt_screen: bool,
    /// Drop `CSI 3 J`.
    pub no_scrollback_clear: bool,
    /// Drop `ESC ( x`, `ESC ) x`, `ESC * x`, `ESC + x`, SO and SI.
    pub no_charsets: bool,
    /// BS also erases the character it moves onto.
    pub destructive_backspace: bool,
}

impl Rules {
    fn any(&self) -> bool {
        self.implicit_cr_in_lf
            || self.implicit_lf_in_cr
            || self.no_alt_screen
            || self.no_scrollback_clear
            || self.no_charsets
            || self.destructive_backspace
    }
}

/// Longest escape sequence kept while waiting for its end; longer ones are
/// passed on as they are.
const MAX_PENDING: usize = 256;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// In an escape sequence (bytes kept in `pending`).
    Escape,
    /// In a string (OSC / DCS / SOS / PM / APC) up to BEL or ST.
    Str,
    /// ESC seen inside a string (ST is ESC \).
    StrEscape,
}

#[derive(Debug, Default)]
pub struct Filter {
    state: State,
    pending: Vec<u8>,
}

enum Seq {
    /// Not finished yet.
    More,
    /// Starts a string: pass on, then stream the string.
    String,
    /// Complete.
    Done,
}

/// Where the escape sequence in `p` (starting with ESC) stands.
fn classify(p: &[u8]) -> Seq {
    let Some(&second) = p.get(1) else {
        return Seq::More;
    };
    match second {
        b'[' => match p[2..].iter().position(|b| (0x40..=0x7e).contains(b)) {
            Some(_) => Seq::Done,
            None => Seq::More,
        },
        b']' | b'P' | b'X' | b'^' | b'_' => Seq::String,
        // Intermediate bytes, then a final byte.
        0x20..=0x2f => match p[2..].iter().position(|b| (0x30..=0x7e).contains(b)) {
            Some(_) => Seq::Done,
            None => Seq::More,
        },
        _ => Seq::Done,
    }
}

impl Filter {
    /// Rewrite `bytes`; `None` when they pass unchanged.
    pub fn apply(&mut self, rules: &Rules, bytes: &[u8]) -> Option<Vec<u8>> {
        if !rules.any() && self.state == State::Ground {
            return None;
        }
        let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 8);
        for &b in bytes {
            match self.state {
                State::Ground => match b {
                    0x1b => {
                        self.pending.clear();
                        self.pending.push(b);
                        self.state = State::Escape;
                    }
                    b'\n' if rules.implicit_cr_in_lf => out.extend_from_slice(b"\r\n"),
                    b'\r' if rules.implicit_lf_in_cr => out.extend_from_slice(b"\r\n"),
                    0x08 if rules.destructive_backspace => out.extend_from_slice(b"\x08\x1b[X"),
                    0x0e | 0x0f if rules.no_charsets => {}
                    _ => out.push(b),
                },
                // A new ESC restarts the sequence, as in the parser.
                State::Escape if b == 0x1b => {
                    out.append(&mut self.pending);
                    self.pending.push(b);
                }
                State::Escape => {
                    self.pending.push(b);
                    match classify(&self.pending) {
                        Seq::More if self.pending.len() < MAX_PENDING => {}
                        Seq::More => {
                            out.append(&mut self.pending);
                            self.state = State::Ground;
                        }
                        Seq::String => {
                            out.append(&mut self.pending);
                            self.state = State::Str;
                        }
                        Seq::Done => {
                            let seq = std::mem::take(&mut self.pending);
                            rewrite(rules, &seq, &mut out);
                            self.state = State::Ground;
                        }
                    }
                }
                State::Str => {
                    out.push(b);
                    match b {
                        0x07 => self.state = State::Ground,
                        0x1b => self.state = State::StrEscape,
                        _ => {}
                    }
                }
                State::StrEscape => {
                    out.push(b);
                    self.state = if b == b'\\' {
                        State::Ground
                    } else {
                        State::Str
                    };
                }
            }
        }
        if !rules.any() && self.state == State::Ground && out == bytes {
            return None;
        }
        Some(out)
    }
}

/// A complete escape sequence: keep, drop or edit it.
fn rewrite(rules: &Rules, seq: &[u8], out: &mut Vec<u8>) {
    if rules.no_charsets && seq.len() == 3 && matches!(seq[1], b'(' | b')' | b'*' | b'+') {
        return;
    }
    if seq.len() >= 3 && seq[1] == b'[' {
        let body = &seq[2..seq.len() - 1];
        let last = seq[seq.len() - 1];
        if rules.no_scrollback_clear && last == b'J' && body == b"3" {
            return;
        }
        if rules.no_alt_screen
            && matches!(last, b'h' | b'l')
            && let Some(params) = body.strip_prefix(b"?")
        {
            let kept: Vec<&[u8]> = params
                .split(|&b| b == b';')
                .filter(|p| !matches!(*p, b"47" | b"1047" | b"1049"))
                .collect();
            if kept.iter().all(|p| p.is_empty()) {
                return;
            }
            out.extend_from_slice(b"\x1b[?");
            out.extend_from_slice(&kept.join(&b';'));
            out.push(last);
            return;
        }
    }
    out.extend_from_slice(seq);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rules: Rules, chunks: &[&[u8]]) -> Vec<u8> {
        let mut f = Filter::default();
        let mut out = Vec::new();
        for c in chunks {
            match f.apply(&rules, c) {
                Some(v) => out.extend(v),
                None => out.extend_from_slice(c),
            }
        }
        out
    }

    const OFF: Rules = Rules {
        implicit_cr_in_lf: false,
        implicit_lf_in_cr: false,
        no_alt_screen: false,
        no_scrollback_clear: false,
        no_charsets: false,
        destructive_backspace: false,
    };

    #[test]
    fn nothing_switched_off_passes_unchanged() {
        let mut f = Filter::default();
        assert!(f.apply(&OFF, b"a\x1b[?1049h\x1b[3J\x1b(0b").is_none());
    }

    #[test]
    fn alternate_screen_and_scrollback_clearing() {
        let rules = Rules {
            no_alt_screen: true,
            no_scrollback_clear: true,
            ..OFF
        };
        // Split in the middle of the sequences.
        let out = run(rules, &[b"a\x1b[?10", b"49hb\x1b[3", b"Jc\x1b[2J"]);
        assert_eq!(out, b"abc\x1b[2J");
        // An ESC inside a sequence starts a new one.
        assert_eq!(run(rules, &[b"\x1b\x1b[?1049h"]), b"\x1b");
        // Other private modes in the same sequence stay.
        assert_eq!(run(rules, &[b"\x1b[?1049;25h"]), b"\x1b[?25h");
        assert_eq!(run(rules, &[b"\x1b[?25l"]), b"\x1b[?25l");
    }

    #[test]
    fn charsets_and_backspace() {
        let rules = Rules {
            no_charsets: true,
            destructive_backspace: true,
            ..OFF
        };
        assert_eq!(run(rules, &[b"\x1b(0q\x0e\x1b(Bx\x08"]), b"qx\x08\x1b[X");
    }

    #[test]
    fn strings_pass_through_untouched() {
        let rules = Rules {
            destructive_backspace: true,
            implicit_cr_in_lf: true,
            ..OFF
        };
        // A BS and an LF inside an OSC title are not rewritten.
        let out = run(rules, &[b"\x1b]0;t\x08\n", b"x\x07\n"]);
        assert_eq!(out, b"\x1b]0;t\x08\nx\x07\r\n");
        let out = run(rules, &[b"\x1bPq\x08\x1b\\\x08"]);
        assert_eq!(out, b"\x1bPq\x08\x1b\\\x08\x1b[X");
    }

    #[test]
    fn runaway_sequences_are_not_kept_forever() {
        let rules = Rules {
            no_alt_screen: true,
            ..OFF
        };
        let mut long = b"\x1b[".to_vec();
        long.extend(std::iter::repeat_n(b'1', 1000));
        let out = run(rules, &[&long]);
        assert_eq!(out, long);
    }
}
