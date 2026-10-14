//! Terminal emulation core, UI-agnostic. Renderers read [`CellView`]s; input
//! layers turn [`Key`]s into bytes with [`encode_key`].
//!
//! The VT parser is currently `vt100`; it is wrapped so it can be swapped for
//! `alacritty_terminal` later without touching renderers.

pub mod charset;
mod input;
mod mouse;
mod osc7;
mod palette;
mod selection;
mod strip;

pub use charset::TermCharset;
pub use input::{Key, Modifiers, encode_key};
pub use mouse::{MouseAction, MouseButton, MouseEvent};
pub use palette::{CursorShape, DEFAULT_BG, DEFAULT_FG, Rgb, Theme};
pub use selection::{DEFAULT_WORD_DELIMITERS, Point, Selection, SelectionMode};
pub use strip::AnsiStripper;

pub struct Terminal {
    parser: vt100::Parser,
    osc7: osc7::Osc7,
    /// Latest directory reported via OSC 7, not yet taken.
    cwd_report: Option<String>,
    pub theme: Theme,
    selection: Option<Selection>,
    /// Characters that end a word on double click (empty = the default set).
    pub word_delimiters: String,
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
            osc7: osc7::Osc7::default(),
            cwd_report: None,
            theme: Theme::default(),
            selection: None,
            word_delimiters: String::new(),
        }
    }

    /// Feed bytes from the PTY/connection. Returns bytes that must be written
    /// back (answers to terminal queries such as cursor-position reports).
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        if let Some(dir) = self.osc7.scan(bytes) {
            self.cwd_report = Some(dir);
        }
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

    /// Directory the shell reported (OSC 7) since the last call.
    pub fn take_cwd_report(&mut self) -> Option<String> {
        self.cwd_report.take()
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

    /// Full-screen programs (less, vim…) use the alternate screen.
    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn cell(&self, row: u16, col: u16) -> Option<CellView<'_>> {
        let cell = self.parser.screen().cell(row, col)?;
        let t = &self.theme;
        let mut fg = t.resolve(cell.fgcolor(), t.foreground, cell.bold());
        let mut bg = t.resolve(cell.bgcolor(), t.background, false);
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

    /// Absolute line of a view row (0 = top of the live screen).
    fn line_of(&self, row: u16) -> i64 {
        i64::from(row) - self.scrollback() as i64
    }

    /// Cell texts of an absolute line, read through the scrollback; `None`
    /// when the line is not on screen or in history.
    fn line_cells(&mut self, line: i64) -> Option<(Vec<String>, bool)> {
        let (rows, cols) = self.size();
        let saved = self.scrollback();
        let (offset, row) = if line >= 0 {
            (0, u16::try_from(line).ok()?)
        } else {
            (usize::try_from(-line).ok()?, 0)
        };
        self.parser.screen_mut().set_scrollback(offset);
        // Asked for more history than there is: the view stops early.
        let reachable = self.scrollback() == offset && row < rows;
        let result = reachable.then(|| {
            let screen = self.parser.screen();
            let cells = (0..cols)
                .map(|c| {
                    screen
                        .cell(row, c)
                        .map(|cell| cell.contents().to_string())
                        .unwrap_or_default()
                })
                .collect();
            (cells, screen.row_wrapped(row))
        });
        self.parser.screen_mut().set_scrollback(saved);
        result
    }

    /// Bounds of the selection unit under a view cell.
    fn unit_at(&mut self, row: u16, col: u16, mode: SelectionMode) -> (Point, Point) {
        let line = self.line_of(row);
        let (_, cols) = self.size();
        let point = |col| Point { line, col };
        match mode {
            SelectionMode::Char | SelectionMode::Block => (point(col), point(col)),
            SelectionMode::Line => (point(0), point(cols.saturating_sub(1))),
            SelectionMode::Word => {
                let cells = self.line_cells(line).map(|(c, _)| c).unwrap_or_default();
                let refs: Vec<&str> = cells.iter().map(String::as_str).collect();
                let (a, b) = selection::word_bounds(&refs, usize::from(col), &self.word_delimiters);
                let to = |v: usize| u16::try_from(v).unwrap_or(u16::MAX);
                (point(to(a)), point(to(b)))
            }
        }
    }

    /// Start a selection at a view cell (mouse press / double / triple click).
    pub fn select_start(&mut self, row: u16, col: u16, mode: SelectionMode) {
        let unit = self.unit_at(row, col, mode);
        self.selection = Some(Selection::new(mode, unit));
    }

    /// Drag the selection to a view cell.
    pub fn select_extend(&mut self, row: u16, col: u16) {
        let Some(mode) = self.selection.map(|s| s.mode) else {
            return;
        };
        let unit = self.unit_at(row, col, mode);
        if let Some(s) = &mut self.selection {
            s.extend(unit);
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// Whether a view cell is selected (for the renderer).
    pub fn is_selected(&self, row: u16, col: u16) -> bool {
        self.selection
            .is_some_and(|s| s.contains(self.line_of(row), col))
    }

    /// The selected text: trailing blanks trimmed, wrapped lines joined,
    /// other lines separated by `\n`.
    pub fn selection_text(&mut self) -> Option<String> {
        let s = self.selection?;
        let (_, cols) = self.size();
        let mut out = String::new();
        for line in s.start.line..=s.end.line {
            let Some((cells, wrapped)) = self.line_cells(line) else {
                continue;
            };
            let (from, to) = match s.mode {
                SelectionMode::Block => (s.start.col, s.end.col),
                _ => (
                    if line == s.start.line { s.start.col } else { 0 },
                    if line == s.end.line {
                        s.end.col
                    } else {
                        cols - 1
                    },
                ),
            };
            let text: String = cells
                .iter()
                .skip(usize::from(from))
                .take(usize::from(to.saturating_sub(from)) + 1)
                .map(|c| if c.is_empty() { " " } else { c.as_str() })
                .collect();
            let joined = wrapped && s.mode != SelectionMode::Block && line != s.end.line;
            if joined {
                out.push_str(&text);
            } else {
                out.push_str(text.trim_end());
                if line != s.end.line {
                    out.push('\n');
                }
            }
        }
        Some(out)
    }

    /// The program asked for mouse events (then the UI reports clicks
    /// instead of selecting, unless Shift is held).
    pub fn mouse_reporting(&self) -> bool {
        self.mouse_mode() != mouse::Mode::Off
    }

    fn mouse_mode(&self) -> mouse::Mode {
        use vt100::MouseProtocolMode as M;
        match self.parser.screen().mouse_protocol_mode() {
            M::None => mouse::Mode::Off,
            M::Press => mouse::Mode::Press,
            M::PressRelease => mouse::Mode::PressRelease,
            M::ButtonMotion => mouse::Mode::ButtonMotion,
            M::AnyMotion => mouse::Mode::AnyMotion,
        }
    }

    /// Bytes to send for a mouse event, if the program wants this one.
    pub fn encode_mouse(&self, ev: MouseEvent) -> Option<Vec<u8>> {
        use vt100::MouseProtocolEncoding as E;
        let encoding = match self.parser.screen().mouse_protocol_encoding() {
            E::Default => mouse::Encoding::Default,
            E::Utf8 => mouse::Encoding::Utf8,
            E::Sgr => mouse::Encoding::Sgr,
        };
        mouse::encode(ev, self.mouse_mode(), encoding)
    }

    /// How many history lines exist above the live screen.
    fn history_len(&mut self) -> usize {
        let saved = self.scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let len = self.scrollback();
        self.parser.screen_mut().set_scrollback(saved);
        len
    }

    /// A line's text with the starting column of each character.
    fn line_text(&mut self, line: i64) -> Option<(String, Vec<u16>)> {
        let (cells, _) = self.line_cells(line)?;
        let mut text = String::new();
        let mut cols = Vec::new();
        for (col, cell) in cells.iter().enumerate() {
            let col = u16::try_from(col).unwrap_or(u16::MAX);
            if cell.is_empty() {
                text.push(' ');
                cols.push(col);
            } else {
                for c in cell.chars() {
                    text.push(c);
                    cols.push(col);
                }
            }
        }
        Some((text, cols))
    }

    /// Everything: history and screen ("Save terminal text"), trailing
    /// blanks trimmed, wrapped lines joined.
    pub fn history_text(&mut self) -> String {
        let (rows, _) = self.size();
        let first = -(self.history_len() as i64);
        let mut out = String::new();
        for line in first..i64::from(rows) {
            let Some((cells, wrapped)) = self.line_cells(line) else {
                continue;
            };
            let text: String = cells
                .iter()
                .map(|c| if c.is_empty() { " " } else { c.as_str() })
                .collect();
            if wrapped {
                out.push_str(&text);
            } else {
                out.push_str(text.trim_end());
                out.push('\n');
            }
        }
        let trimmed = out.trim_end_matches('\n').len();
        out.truncate(trimmed);
        out.push('\n');
        out
    }

    /// "Find in terminal" / "Find next": search upwards from just before
    /// `before` (or from the bottom), select the match and scroll to it.
    pub fn find(
        &mut self,
        needle: &str,
        before: Option<Point>,
        match_case: bool,
    ) -> Option<(Point, Point)> {
        if needle.is_empty() {
            return None;
        }
        let fold = |s: &str| {
            if match_case {
                s.to_string()
            } else {
                s.to_lowercase()
            }
        };
        let needle = fold(needle);
        let (rows, _) = self.size();
        let first = -(self.history_len() as i64);
        let start = before.map_or(i64::from(rows) - 1, |p| p.line);
        for line in (first..=start).rev() {
            let Some((text, cols)) = self.line_text(line) else {
                continue;
            };
            let hay = fold(&text);
            // Positions in characters; lowercasing may change byte lengths.
            let hay_chars: Vec<char> = hay.chars().collect();
            let n: Vec<char> = needle.chars().collect();
            if hay_chars.len() != cols.len() || n.len() > hay_chars.len() {
                continue;
            }
            let limit = match before {
                Some(p) if p.line == line => {
                    cols.iter().position(|&c| c >= p.col).unwrap_or(cols.len())
                }
                _ => hay_chars.len(),
            };
            let found = (0..=hay_chars.len() - n.len())
                .rev()
                .filter(|&i| i < limit)
                .find(|&i| hay_chars[i..i + n.len()] == n[..]);
            if let Some(i) = found {
                let start = Point { line, col: cols[i] };
                let end = Point {
                    line,
                    col: cols[i + n.len() - 1],
                };
                self.select_range(start, end);
                // Bring the line into view.
                let view_top = self.line_of(0);
                if line < view_top || line >= view_top + i64::from(rows) {
                    let offset = (-line).max(0) as usize;
                    self.set_scrollback(offset);
                }
                return Some((start, end));
            }
        }
        None
    }

    /// Select from `start` to `end` (absolute points).
    pub fn select_range(&mut self, start: Point, end: Point) {
        let mut s = Selection::new(SelectionMode::Char, (start, start));
        s.extend((end, end));
        self.selection = Some(s);
    }

    /// The current selection, if any.
    pub fn selection(&self) -> Option<Selection> {
        self.selection
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
        assert_ne!(r.fg, t.theme.foreground);
    }

    #[test]
    fn selection_reads_words_lines_and_history() {
        let mut t = Terminal::new(3, 20, 100);
        t.feed(b"first line\r\nsecond word here\r\nthird\r\nfourth\r\nfifth");
        // Live screen: third / fourth / fifth; "first", "second" in history.
        t.select_start(1, 1, SelectionMode::Word);
        assert_eq!(t.selection_text().as_deref(), Some("fourth"));
        assert!(t.is_selected(1, 0) && !t.is_selected(0, 0));

        t.select_start(0, 0, SelectionMode::Line);
        t.select_extend(1, 3);
        assert_eq!(t.selection_text().as_deref(), Some("third\nfourth"));

        // Scroll up two lines and select from history.
        t.set_scrollback(2);
        t.select_start(0, 0, SelectionMode::Char);
        t.select_extend(1, 5);
        assert_eq!(t.selection_text().as_deref(), Some("first line\nsecond"));
        t.set_scrollback(0);
        assert!(!t.is_selected(0, 0), "the selection stays in history");
        assert_eq!(t.selection_text().as_deref(), Some("first line\nsecond"));
        t.clear_selection();
        assert_eq!(t.selection_text(), None);
    }

    #[test]
    fn wrapped_lines_are_joined_and_blocks_are_columns() {
        let mut t = Terminal::new(4, 5, 0);
        t.feed(b"abcdefgh\r\nxy");
        t.select_start(0, 0, SelectionMode::Char);
        t.select_extend(1, 2);
        assert_eq!(t.selection_text().as_deref(), Some("abcdefgh"));
        t.select_start(0, 1, SelectionMode::Block);
        t.select_extend(2, 2);
        assert_eq!(t.selection_text().as_deref(), Some("bc\ngh\ny"));
    }

    #[test]
    fn mouse_reporting_follows_the_program() {
        let mut t = Terminal::new(4, 20, 0);
        let click = MouseEvent {
            button: MouseButton::Left,
            action: MouseAction::Press,
            row: 0,
            col: 0,
            mods: Modifiers::default(),
        };
        assert!(!t.mouse_reporting());
        assert_eq!(t.encode_mouse(click), None);
        t.feed(b"\x1b[?1000h\x1b[?1006h");
        assert!(t.mouse_reporting());
        assert_eq!(t.encode_mouse(click).unwrap(), b"\x1b[<0;1;1M");
    }

    #[test]
    fn find_searches_upwards_through_history() {
        let mut t = Terminal::new(3, 20, 100);
        t.feed(b"error one\r\nok\r\nError two\r\nok\r\nlast");
        let (s, e) = t.find("error", None, false).unwrap();
        assert_eq!((s.line, s.col, e.col), (0, 0, 4), "nearest match first");
        assert_eq!(t.selection_text().as_deref(), Some("Error"));
        let (s2, _) = t.find("error", Some(s), false).unwrap();
        assert_eq!(s2.line, -2, "then the older one");
        assert_eq!(t.scrollback(), 2, "scrolled to it");
        assert!(t.find("error", Some(s2), false).is_none());
        assert!(t.find("ERROR", None, true).is_none(), "match case");
        assert_eq!(t.history_text(), "error one\nok\nError two\nok\nlast\n");
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
