//! `cargo xtask icons`: SVG copies of the icons TermCraft shows (Phosphor,
//! MIT licensed), for the README. Each SVG is the glyph's outline.

use anyhow::{Context, Result};
use egui_phosphor::regular as icon;
use std::fmt::Write as _;
use std::path::Path;

/// File name → glyph, as the app uses them.
pub const ICONS: &[(&str, &str)] = &[
    ("ssh", icon::TERMINAL_WINDOW),
    ("telnet", icon::PLUGS),
    ("rsh", icon::TERMINAL),
    ("xdmcp", icon::MONITOR),
    ("rdp", icon::DESKTOP),
    ("vnc", icon::SCREENCAST),
    ("ftp", icon::FOLDER_OPEN),
    ("sftp", icon::FOLDER_SIMPLE_USER),
    ("serial", icon::USB),
    ("file", icon::FILE),
    ("shell", icon::CODE),
    ("browser", icon::BROWSER),
    ("mosh", icon::LIGHTNING),
    ("s3", icon::CLOUD),
    ("wsl", icon::LINUX_LOGO),
    ("session", icon::MONITOR_PLAY),
    ("servers", icon::HARD_DRIVES),
    ("tools", icon::WRENCH),
    ("sessions", icon::STAR),
    ("split", icon::SQUARE_SPLIT_HORIZONTAL),
    ("multiexec", icon::BROADCAST),
    ("tunneling", icon::ARROWS_LEFT_RIGHT),
    ("settings", icon::GEAR),
    ("key", icon::KEY),
    ("shield", icon::SHIELD_CHECK),
    ("macros", icon::RECORD),
    ("editor", icon::NOTE_PENCIL),
    ("diff", icon::GIT_DIFF),
    ("image", icon::IMAGE),
    ("popup", icon::ARROW_SQUARE_DOWN),
    ("update", icon::ARROWS_CLOCKWISE),
    ("shared", icon::USERS),
    ("highlight", icon::PALETTE),
    ("x11", icon::APP_WINDOW),
    ("jump", icon::ARROWS_SPLIT),
    ("lock", icon::LOCK_KEY),
    ("cli", icon::TERMINAL),
    ("portable", icon::USB),
];

struct Path2(String);

impl ttf_parser::OutlineBuilder for Path2 {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "M{x} {y}");
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "L{x} {y}");
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.0, "Q{x1} {y1} {x} {y}");
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(self.0, "C{x1} {y1} {x2} {y2} {x} {y}");
    }
    fn close(&mut self) {
        self.0.push('Z');
    }
}

/// The SVG of one glyph (`color` fill).
pub fn svg(face: &ttf_parser::Face<'_>, glyph: &str, color: &str) -> Option<String> {
    let c = glyph.chars().next()?;
    let id = face.glyph_index(c)?;
    let mut path = Path2(String::new());
    face.outline_glyph(id, &mut path)?;
    let em = f32::from(face.units_per_em());
    let ascender = f32::from(face.ascender());
    // Font units grow upwards: flip around the baseline.
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {em} {em}\" width=\"32\" height=\"32\">\
         <path transform=\"translate(0 {ascender}) scale(1 -1)\" fill=\"{color}\" d=\"{}\"/></svg>\n",
        path.0
    ))
}

pub fn write_all(out: &Path) -> Result<usize> {
    let bytes = egui_phosphor::Variant::Regular.font_bytes();
    let face = ttf_parser::Face::parse(bytes, 0).context("Phosphor font")?;
    std::fs::create_dir_all(out)?;
    let mut n = 0;
    for (name, glyph) in ICONS {
        let s = svg(&face, glyph, "#3d93f0").with_context(|| format!("glyph for {name}"))?;
        std::fs::write(out.join(format!("{name}.svg")), s)?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_an_outline() {
        let face =
            ttf_parser::Face::parse(egui_phosphor::Variant::Regular.font_bytes(), 0).unwrap();
        for (name, glyph) in ICONS {
            let s = svg(&face, glyph, "#000").unwrap_or_else(|| panic!("{name}"));
            assert!(s.starts_with("<svg") && s.contains(" d=\"M"), "{name}");
        }
    }
}
