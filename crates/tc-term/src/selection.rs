//! Mouse text selection: by character, word (double click), line (triple
//! click) or block (Ctrl+Alt drag). Points are kept in *absolute* lines
//! (`line = view row - scrollback offset`), so scrolling the view does not
//! move a selection; text is read back through the scrollback.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    Char,
    Word,
    Line,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    /// 0 = top row of the live screen, negative = scrollback.
    pub line: i64,
    pub col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub mode: SelectionMode,
    /// Where the drag started (for words / lines: that unit's bounds).
    anchor: (Point, Point),
    /// Current extent, ordered.
    pub start: Point,
    pub end: Point,
}

/// Characters that end a word on double click when the setting is empty.
pub const DEFAULT_WORD_DELIMITERS: &str = " \t,;:\"'`()[]{}<>|";

impl Selection {
    pub(crate) fn new(mode: SelectionMode, unit: (Point, Point)) -> Self {
        Self {
            mode,
            anchor: unit,
            start: unit.0,
            end: unit.1,
        }
    }

    /// Extend to the unit under the pointer (`unit` = its bounds).
    pub(crate) fn extend(&mut self, unit: (Point, Point)) {
        if self.mode == SelectionMode::Block {
            let (a, b) = (self.anchor.0, unit.0);
            self.start = Point {
                line: a.line.min(b.line),
                col: a.col.min(b.col),
            };
            self.end = Point {
                line: a.line.max(b.line),
                col: a.col.max(b.col),
            };
            return;
        }
        if unit.0 < self.anchor.0 {
            self.start = unit.0;
            self.end = self.anchor.1;
        } else {
            self.start = self.anchor.0;
            self.end = unit.1.max(self.anchor.1);
        }
    }

    pub fn contains(&self, line: i64, col: u16) -> bool {
        if line < self.start.line || line > self.end.line {
            return false;
        }
        if self.mode == SelectionMode::Block {
            return col >= self.start.col && col <= self.end.col;
        }
        let p = Point { line, col };
        p >= self.start && p <= self.end
    }
}

/// Characters that end a word on double click, on each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delimiters {
    pub left: String,
    pub right: String,
}

impl Default for Delimiters {
    fn default() -> Self {
        Self::new("", "", "", "", "")
    }
}

impl Delimiters {
    /// `base` (empty = the default set), plus / minus the expert settings'
    /// "Add / Remove left / right delimiters".
    pub fn new(base: &str, add_left: &str, add_right: &str, rm_left: &str, rm_right: &str) -> Self {
        let base = if base.is_empty() {
            DEFAULT_WORD_DELIMITERS
        } else {
            base
        };
        let side = |add: &str, rm: &str| -> String {
            let mut set: String = base.chars().filter(|c| !rm.contains(*c)).collect();
            for c in add.chars() {
                if !set.contains(c) && !c.is_control() {
                    set.push(c);
                }
            }
            set
        };
        Self {
            left: side(add_left, rm_left),
            right: side(add_right, rm_right),
        }
    }
}

/// Word bounds around `col` in a row (`chars[i]` = cell text).
pub(crate) fn word_bounds(chars: &[&str], col: usize, delimiters: &Delimiters) -> (usize, usize) {
    let is_delim = |s: &str, set: &str| s.is_empty() || s.chars().all(|c| set.contains(c));
    if col >= chars.len()
        || (is_delim(chars[col], &delimiters.left) && is_delim(chars[col], &delimiters.right))
    {
        return (col, col);
    }
    let mut start = col;
    while start > 0 && !is_delim(chars[start - 1], &delimiters.left) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && !is_delim(chars[end + 1], &delimiters.right) {
        end += 1;
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: i64, col: u16) -> Point {
        Point { line, col }
    }

    #[test]
    fn char_selection_extends_both_ways() {
        let mut s = Selection::new(SelectionMode::Char, (p(2, 5), p(2, 5)));
        s.extend((p(3, 1), p(3, 1)));
        assert_eq!((s.start, s.end), (p(2, 5), p(3, 1)));
        assert!(s.contains(2, 79) && s.contains(3, 0) && !s.contains(3, 2));
        s.extend((p(1, 7), p(1, 7)));
        assert_eq!((s.start, s.end), (p(1, 7), p(2, 5)));
    }

    #[test]
    fn block_selection_is_a_rectangle() {
        let mut s = Selection::new(SelectionMode::Block, (p(0, 10), p(0, 10)));
        s.extend((p(2, 4), p(2, 4)));
        assert!(s.contains(1, 4) && s.contains(1, 10));
        assert!(!s.contains(1, 11) && !s.contains(1, 3));
    }

    #[test]
    fn words() {
        let text: Vec<String> = "ls -la /etc/hosts; echo"
            .chars()
            .map(String::from)
            .collect();
        let row: Vec<&str> = text.iter().map(String::as_str).collect();
        let d = Delimiters::default();
        assert_eq!(word_bounds(&row, 8, &d), (7, 16));
        let row = ["a", "b", "(", "c", ")"];
        assert_eq!(word_bounds(&row, 3, &d), (3, 3));
        assert_eq!(word_bounds(&row, 0, &d), (0, 1));
        assert_eq!(word_bounds(&row, 2, &d), (2, 2), "a delimiter alone");
        let custom = Delimiters::new("x", "", "", "", "");
        assert_eq!(word_bounds(&row, 3, &custom), (0, 4), "custom delimiters");
        // "Remove left delimiters: (" keeps the parenthesis on the left only.
        let asym = Delimiters::new("", "", "", "(", "");
        assert_eq!(word_bounds(&row, 3, &asym), (0, 3));
        // "Add right delimiters: /" stops at a slash on the right only.
        let text: Vec<String> = "/etc/hosts".chars().map(String::from).collect();
        let row: Vec<&str> = text.iter().map(String::as_str).collect();
        let slash = Delimiters::new("", "", "/", "", "");
        assert_eq!(word_bounds(&row, 1, &slash), (0, 3));
    }
}
