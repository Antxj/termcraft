//! Terminal emulation core, UI-agnostic. Renderers read [`CellView`]s; input
//! layers turn [`Key`]s into bytes with [`encode_key`].
//!
//! The VT parser is currently `vt100`; it is wrapped so it can be swapped for
//! `alacritty_terminal` later without touching renderers.

mod input;
mod palette;

pub use input::{Key, Modifiers, encode_key};
pub use palette::{DEFAULT_BG, DEFAULT_FG, Rgb, color_to_rgb};

pub struct Terminal {
    parser: vt100::Parser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellView<'a> {
    pub text: &'a str,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Second half of a double-width glyph; renderers skip it.
    pub wide_continuation: bool,
}

impl Terminal {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new(rows.max(1), cols.max(1), scrollback),
        }
    }

    /// Feed bytes from the PTY/connection. Returns bytes that must be written
    /// back (answers to terminal queries such as cursor-position reports).
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        self.query_replies(bytes)
    }

    fn query_replies(&self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for window in bytes.windows(4) {
            match window {
                // DSR: report cursor position (1-based).
                b"\x1b[6n" => {
                    let (row, col) = self.parser.screen().cursor_position();
                    out.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
                }
                // DSR: device status OK.
                b"\x1b[5n" => out.extend_from_slice(b"\x1b[0n"),
                _ => {}
            }
        }
        // Primary device attributes: ESC [ c / ESC [ 0 c → VT220-ish.
        if contains(bytes, b"\x1b[c") || contains(bytes, b"\x1b[0c") {
            out.extend_from_slice(b"\x1b[?62;22c");
        }
        out
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
    }

    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    /// Scroll the viewport into history; 0 = live view.
    pub fn set_scrollback(&mut self, rows: usize) {
        self.parser.screen_mut().set_scrollback(rows);
    }

    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn cursor(&self) -> Option<(u16, u16)> {
        let screen = self.parser.screen();
        (!screen.hide_cursor() && screen.scrollback() == 0).then(|| screen.cursor_position())
    }

    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn cell(&self, row: u16, col: u16) -> Option<CellView<'_>> {
        let cell = self.parser.screen().cell(row, col)?;
        let mut fg = color_to_rgb(cell.fgcolor(), DEFAULT_FG);
        let mut bg = color_to_rgb(cell.bgcolor(), DEFAULT_BG);
        if cell.inverse() {
            std::mem::swap(&mut fg, &mut bg);
        }
        Some(CellView {
            text: cell.contents(),
            fg,
            bg,
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
            wide_continuation: cell.is_wide_continuation(),
        })
    }

    /// Plain-text dump of the visible screen (used by tests and `tab_read`).
    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_and_colors() {
        let mut t = Terminal::new(4, 20, 100);
        t.feed(b"hello \x1b[31mred\x1b[0m");
        assert!(t.contents().starts_with("hello red"));
        let r = t.cell(0, 6).unwrap();
        assert_eq!(r.text, "r");
        assert_ne!(r.fg, DEFAULT_FG);
    }

    #[test]
    fn answers_cursor_position_report() {
        let mut t = Terminal::new(4, 20, 0);
        let reply = t.feed(b"ab\x1b[6n");
        assert_eq!(reply, b"\x1b[1;3R");
    }

    #[test]
    fn resize_keeps_working() {
        let mut t = Terminal::new(2, 5, 0);
        t.resize(10, 40);
        assert_eq!(t.size(), (10, 40));
        t.feed(b"0123456789");
        assert_eq!(t.cell(0, 9).unwrap().text, "9");
    }
}
