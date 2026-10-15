//! Session "Terminal settings" (schema section `[2]`) → terminal look and
//! behavior: colors, cursor, font size, Backspace and terminal type.

use tc_core::{Session, schema};
use tc_term::{CursorShape, Rgb, Theme};

pub struct TermSettings {
    pub theme: Theme,
    /// Font size in pixels; `None` = global setting.
    pub font_px: Option<f32>,
    /// 4_1 font name / 4_3 bold, when the session sets them.
    pub font_name: Option<String>,
    pub font_bold: Option<bool>,
    /// 4_35 "Force fixed rows/columns on resize" with 4_33 columns and
    /// 4_34 rows, when the session sets it (`Some(None)` = off).
    pub fixed_grid: Option<Option<(u16, u16)>>,
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

/// The default terminal colors set in the configuration (or imported from
/// MobaXterm); `None` when there are none.
pub fn default_theme(colors: Option<&tc_core::settings::TerminalColors>) -> Option<Theme> {
    let c = colors?;
    let rgb = |[r, g, b]: [u8; 3]| Rgb(r, g, b);
    Some(Theme {
        foreground: rgb(c.foreground),
        background: rgb(c.background),
        cursor: rgb(c.cursor),
        cursor_shape: match c.cursor_type {
            1 => CursorShape::Underline,
            2 => CursorShape::Line,
            _ => CursorShape::Block,
        },
        cursor_blink: c.cursor_blink,
        ansi: c.ansi.map(rgb),
        ..Theme::default()
    })
}

/// The terminal settings of `session`. What it does not set comes from
/// `global` (the configured colors) when there is one, else from the
/// session defaults (the reference look); a local terminal without a
/// session uses `global` or TermCraft's palette.
pub fn for_session(session: Option<&Session>, global: Option<&Theme>) -> TermSettings {
    let Some(session) = session else {
        return TermSettings {
            theme: global.copied().unwrap_or_default(),
            font_px: None,
            font_name: None,
            font_bold: None,
            fixed_grid: None,
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
    let defaults = Theme::default();
    let color = |id: &str, fallback: Rgb| {
        let set = session.options.get(id).and_then(|v| Rgb::parse(v));
        match global {
            Some(_) => set.unwrap_or(fallback),
            None => set.or_else(|| Rgb::parse(&value(id))).unwrap_or(fallback),
        }
    };
    let base = global.copied().unwrap_or(defaults);

    let mut ansi = base.ansi;
    for (i, (normal, bright)) in ANSI_IDS.iter().enumerate() {
        ansi[i] = color(normal, ansi[i]);
        ansi[i + 8] = color(bright, ansi[i + 8]);
    }
    let cursor_type = session
        .options
        .get("4_10")
        .cloned()
        .or_else(|| global.is_none().then(|| value("4_10")))
        .and_then(|v| v.parse::<u8>().ok());
    let theme = Theme {
        foreground: color("4_7", base.foreground),
        background: color("4_8", base.background),
        cursor: color("4_9", base.cursor),
        cursor_shape: match cursor_type.map(|t| t % 3) {
            None => base.cursor_shape,
            Some(1) => CursorShape::Underline,
            Some(2) => CursorShape::Line,
            Some(_) => CursorShape::Block,
        },
        cursor_blink: cursor_type.map_or(base.cursor_blink, |t| t >= 3),
        ansi,
        bold_is_bright: schema::is_true(&value("4_37")),
    };
    // The reference stores the font size in points; 1 pt = 4/3 px at 96 dpi.
    let font_px = value("4_2")
        .parse::<f32>()
        .ok()
        .filter(|pt| (4.0..=72.0).contains(pt))
        .map(|pt| pt * 4.0 / 3.0);
    let explicit = |id: &str| session.options.get(id).map(|v| v.trim().to_string());
    let number = |id: &str, default: u16| {
        value(id)
            .parse::<u16>()
            .ok()
            .filter(|n| (2..=1000).contains(n))
            .unwrap_or(default)
    };
    TermSettings {
        theme,
        font_px,
        font_name: explicit("4_1").filter(|n| !n.is_empty()),
        font_bold: explicit("4_3").map(|v| schema::is_true(&v)),
        fixed_grid: explicit("4_35")
            .map(|v| schema::is_true(&v).then(|| (number("4_33", 80), number("4_34", 24)))),
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
        let d = for_session(Some(&s), None);
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
        let c = for_session(Some(&s), None);
        assert_eq!(c.theme.background, Rgb(0, 0, 64));
        assert_eq!(c.font_px, Some(16.0));
        assert_eq!(c.theme.cursor_shape, CursorShape::Underline);
        assert!(c.theme.cursor_blink);
        assert!(c.backspace_sends_ctrl_h);
        assert_eq!(c.term_type.as_deref(), Some("vt100"));

        assert!(for_session(None, None).font_px.is_none());
    }

    #[test]
    fn configured_colors_apply_where_the_session_sets_none() {
        let global = Theme {
            background: Rgb(1, 2, 3),
            cursor_shape: CursorShape::Line,
            ..Theme::default()
        };
        let mut s = Session::new("x", SessionKind::default_for(0).unwrap());
        let t = for_session(Some(&s), Some(&global)).theme;
        assert_eq!(t.background, Rgb(1, 2, 3));
        assert_eq!(t.cursor_shape, CursorShape::Line);
        s.options.insert("4_8".into(), "9,9,9".into());
        assert_eq!(
            for_session(Some(&s), Some(&global)).theme.background,
            Rgb(9, 9, 9),
            "the session's own color wins"
        );
        assert_eq!(
            for_session(None, Some(&global)).theme.background,
            Rgb(1, 2, 3)
        );
        assert_eq!(for_session(None, None).theme, Theme::default());
    }
}
