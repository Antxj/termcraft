//! Session "Terminal settings" (schema section `[2]`) → terminal look and
//! behavior: colors, cursor, font size, Backspace and terminal type.

use tc_core::{Session, schema};
use tc_term::{CursorShape, Rgb, Theme};

pub struct TermSettings {
    pub theme: Theme,
    /// Font size in pixels; `None` = global setting.
    pub font_px: Option<f32>,
    pub backspace_sends_ctrl_h: bool,
    /// `TERM` for remote shells when set explicitly in the session.
    pub term_type: Option<String>,
}

/// Normal / bright ANSI color parameter ids, in ANSI order.
const ANSI_IDS: [(&str, &str); 8] = [
    ("4_17", "4_18"), // black
    ("4_19", "4_20"), // red
    ("4_21", "4_22"), // green
    ("4_23", "4_24"), // yellow
    ("4_25", "4_26"), // blue
    ("4_27", "4_28"), // magenta
    ("4_29", "4_30"), // cyan
    ("4_31", "4_32"), // white
];

pub fn for_session(session: Option<&Session>) -> TermSettings {
    let Some(session) = session else {
        return TermSettings {
            theme: Theme::default(),
            font_px: None,
            backspace_sends_ctrl_h: false,
            term_type: None,
        };
    };
    let value = |id: &str| -> String {
        schema::terminal_params()
            .iter()
            .find(|p| p.id == id)
            .map_or_else(String::new, |p| session.option(p).to_string())
    };
    let color = |id: &str, fallback: Rgb| Rgb::parse(&value(id)).unwrap_or(fallback);
    let defaults = Theme::default();

    let mut ansi = defaults.ansi;
    for (i, (normal, bright)) in ANSI_IDS.iter().enumerate() {
        ansi[i] = color(normal, ansi[i]);
        ansi[i + 8] = color(bright, ansi[i + 8]);
    }
    let cursor_type = value("4_10").parse::<u8>().unwrap_or(0);
    let theme = Theme {
        foreground: color("4_7", defaults.foreground),
        background: color("4_8", defaults.background),
        cursor: color("4_9", defaults.cursor),
        cursor_shape: match cursor_type % 3 {
            1 => CursorShape::Underline,
            2 => CursorShape::Line,
            _ => CursorShape::Block,
        },
        cursor_blink: cursor_type >= 3,
        ansi,
        bold_is_bright: schema::is_true(&value("4_37")),
    };
    // The reference stores the font size in points; 1 pt = 4/3 px at 96 dpi.
    let font_px = value("4_2")
        .parse::<f32>()
        .ok()
        .filter(|pt| (4.0..=72.0).contains(pt))
        .map(|pt| pt * 4.0 / 3.0);
    TermSettings {
        theme,
        font_px,
        backspace_sends_ctrl_h: schema::is_true(&value("4_11")),
        // Only an explicit choice overrides TermCraft's xterm-256color.
        term_type: session
            .options
            .get("4_14")
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tc_core::SessionKind;

    #[test]
    fn session_colors_font_and_keys() {
        let mut s = Session::new("x", SessionKind::default_for(0).unwrap());
        // Schema defaults (reference look): background 30,30,30, 10 pt font.
        let d = for_session(Some(&s));
        assert_eq!(d.theme.background, Rgb(30, 30, 30));
        assert_eq!(d.theme.ansi[1], Rgb(255, 96, 96));
        assert_eq!(d.font_px, Some(10.0 * 4.0 / 3.0));
        assert!(!d.backspace_sends_ctrl_h);
        assert_eq!(d.term_type, None);

        s.options.insert("4_8".into(), "0,0,64".into());
        s.options.insert("4_2".into(), "12".into());
        s.options.insert("4_10".into(), "4".into()); // blinking underline
        s.options.insert("4_11".into(), "-1".into());
        s.options.insert("4_14".into(), "vt100".into());
        let c = for_session(Some(&s));
        assert_eq!(c.theme.background, Rgb(0, 0, 64));
        assert_eq!(c.font_px, Some(16.0));
        assert_eq!(c.theme.cursor_shape, CursorShape::Underline);
        assert!(c.theme.cursor_blink);
        assert!(c.backspace_sends_ctrl_h);
        assert_eq!(c.term_type.as_deref(), Some("vt100"));

        assert!(for_session(None).font_px.is_none());
    }
}
