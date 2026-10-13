#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parse `"r,g,b"` (the session settings color format).
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split(',').map(|p| p.trim().parse::<u8>().ok());
        let rgb = Self(it.next()??, it.next()??, it.next()??);
        it.next().is_none().then_some(rgb)
    }
}

pub const DEFAULT_FG: Rgb = Rgb(0xd8, 0xd8, 0xd8);
pub const DEFAULT_BG: Rgb = Rgb(0x1a, 0x1b, 0x1e);

const ANSI16: [Rgb; 16] = [
    Rgb(0x1a, 0x1b, 0x1e),
    Rgb(0xcd, 0x3b, 0x3b),
    Rgb(0x4e, 0xb3, 0x5a),
    Rgb(0xd7, 0xb2, 0x4a),
    Rgb(0x3b, 0x8e, 0xea),
    Rgb(0xbc, 0x5f, 0xd3),
    Rgb(0x2a, 0xb0, 0xb8),
    Rgb(0xd8, 0xd8, 0xd8),
    Rgb(0x66, 0x66, 0x66),
    Rgb(0xf1, 0x4c, 0x4c),
    Rgb(0x6c, 0xd9, 0x7a),
    Rgb(0xf5, 0xd5, 0x6a),
    Rgb(0x60, 0xa8, 0xff),
    Rgb(0xd6, 0x7a, 0xf0),
    Rgb(0x4c, 0xd6, 0xde),
    Rgb(0xff, 0xff, 0xff),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Line,
}

/// Terminal colors and cursor style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    /// 0–7 normal, 8–15 bright ("bold") ANSI colors.
    pub ansi: [Rgb; 16],
    /// Render bold text with the bright variant of its color.
    pub bold_is_bright: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            foreground: DEFAULT_FG,
            background: DEFAULT_BG,
            cursor: Rgb(0xdc, 0xdc, 0xdc),
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
            ansi: ANSI16,
            bold_is_bright: true,
        }
    }
}

impl Theme {
    pub fn resolve(&self, color: vt100::Color, default: Rgb, bold: bool) -> Rgb {
        match color {
            vt100::Color::Default => default,
            vt100::Color::Rgb(r, g, b) => Rgb(r, g, b),
            vt100::Color::Idx(i) if i < 8 && bold && self.bold_is_bright => {
                self.ansi[i as usize + 8]
            }
            vt100::Color::Idx(i) if i < 16 => self.ansi[i as usize],
            vt100::Color::Idx(i) => indexed(i),
        }
    }
}

/// xterm 256-color palette beyond the 16 ANSI colors: 6×6×6 cube, 24 grays.
fn indexed(i: u8) -> Rgb {
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            Rgb(v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_corners() {
        assert_eq!(indexed(16), Rgb(0, 0, 0));
        assert_eq!(indexed(231), Rgb(255, 255, 255));
        assert_eq!(indexed(232), Rgb(8, 8, 8));
        assert_eq!(indexed(255), Rgb(238, 238, 238));
    }

    #[test]
    fn parse_and_bold_brightening() {
        assert_eq!(Rgb::parse("30, 30,30"), Some(Rgb(30, 30, 30)));
        assert_eq!(Rgb::parse("1,2"), None);
        assert_eq!(Rgb::parse("1,2,3,4"), None);
        assert_eq!(Rgb::parse("300,0,0"), None);
        let t = Theme::default();
        assert_eq!(t.resolve(vt100::Color::Idx(1), DEFAULT_FG, true), t.ansi[9]);
        assert_eq!(
            t.resolve(vt100::Color::Idx(1), DEFAULT_FG, false),
            t.ansi[1]
        );
    }
}
