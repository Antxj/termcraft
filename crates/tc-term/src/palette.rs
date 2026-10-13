#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

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

pub fn color_to_rgb(color: vt100::Color, default: Rgb) -> Rgb {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Rgb(r, g, b),
        vt100::Color::Idx(i) => indexed(i),
    }
}

/// xterm 256-color palette: 16 ANSI, 6×6×6 cube, 24 grays.
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
}
