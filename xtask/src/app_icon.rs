//! `cargo xtask app-icon`: TermCraft's program icon — the `>_` prompt
//! (Phosphor "terminal", bold, MIT licensed) in white on a blue rounded
//! square — drawn with tiny-skia in the formats each system wants:
//!
//! - `crates/tc-ui/assets/icon-256.png`: window, taskbar and tray icon;
//! - `crates/tc-ui/assets/termcraft.ico`: the Windows `.exe` icon;
//! - `crates/tc-ui/assets/termcraft.icns`: the macOS `.app` icon;
//! - `docs/icons/app.png`: the README logo.

use anyhow::{Context, Result};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider as _};
use std::path::Path;
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode,
    Transform,
};

/// Sizes inside the `.ico`.
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
/// `.icns` entries: (type, size). PNG-encoded types, macOS 10.7+.
const ICNS_TYPES: [(&[u8; 4], u32); 7] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
];

struct Pen(PathBuilder);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, -y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, -y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, -y1, x, -y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, -y1, x2, -y2, x, -y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

/// The glyph's outline, y pointing down (font units).
fn glyph_path() -> Result<tiny_skia::Path> {
    let font = FontRef::new(egui_phosphor::Variant::Bold.font_bytes()).context("Phosphor font")?;
    let c = egui_phosphor::bold::TERMINAL
        .chars()
        .next()
        .context("glyph")?;
    let id = font.charmap().map(c).context("glyph not in font")?;
    let outline = font.outline_glyphs().get(id).context("no outline")?;
    let mut pen = Pen(PathBuilder::new());
    outline
        .draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut pen,
        )
        .map_err(|e| anyhow::anyhow!("outline: {e:?}"))?;
    pen.0.finish().context("empty outline")
}

fn rounded_square(size: f32) -> Option<tiny_skia::Path> {
    let m = size * 0.06;
    let (x0, y0, x1, y1) = (m, m, size - m, size - m);
    let r = size * 0.2;
    // Circle arcs as cubics (k = 4/3 · (√2 − 1)).
    let k = r * 0.552_284_8;
    let mut p = PathBuilder::new();
    p.move_to(x0 + r, y0);
    p.line_to(x1 - r, y0);
    p.cubic_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
    p.line_to(x1, y1 - r);
    p.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    p.line_to(x0 + r, y1);
    p.cubic_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
    p.line_to(x0, y0 + r);
    p.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    p.close();
    p.finish()
}

/// The icon at `size` × `size` pixels.
pub fn render(size: u32) -> Result<Pixmap> {
    let mut pixmap = Pixmap::new(size, size).context("pixmap")?;
    let s = size as f32;
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.shader = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, s),
        vec![
            GradientStop::new(0.0, Color::from_rgba8(0x3d, 0x93, 0xf0, 0xff)),
            GradientStop::new(1.0, Color::from_rgba8(0x1f, 0x5f, 0xc9, 0xff)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .context("gradient")?;
    let square = rounded_square(s).context("square")?;
    pixmap.fill_path(
        &square,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    let glyph = glyph_path()?;
    let b = glyph.bounds();
    // The prompt fills 54 % of the width, centred.
    let scale = s * 0.54 / b.width().max(b.height());
    let (w, h) = (b.width() * scale, b.height() * scale);
    let t = Transform::from_translate((s - w) / 2.0, (s - h) / 2.0)
        .pre_scale(scale, scale)
        .pre_translate(-b.left(), -b.top());
    let mut white = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    white.set_color(Color::WHITE);
    pixmap.fill_path(&glyph, &white, FillRule::Winding, t, None);
    Ok(pixmap)
}

fn png(size: u32) -> Result<Vec<u8>> {
    render(size)?.encode_png().context("png")
}

/// A `.ico` made of PNG images (Windows Vista+).
pub fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in images {
        // 256 is written as 0.
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

/// A `.icns` made of PNG images.
pub fn icns(images: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let body: usize = images.iter().map(|(_, d)| 8 + d.len()).sum();
    let mut out = Vec::with_capacity(8 + body);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&((8 + body) as u32).to_be_bytes());
    for (kind, data) in images {
        out.extend_from_slice(*kind);
        out.extend_from_slice(&((8 + data.len()) as u32).to_be_bytes());
        out.extend_from_slice(data);
    }
    out
}

pub fn write_all(root: &Path) -> Result<()> {
    let assets = root.join("crates/tc-ui/assets");
    std::fs::create_dir_all(&assets)?;
    std::fs::write(assets.join("icon-256.png"), png(256)?)?;
    let ico_images = ICO_SIZES
        .iter()
        .map(|&s| Ok((s, png(s)?)))
        .collect::<Result<Vec<_>>>()?;
    std::fs::write(assets.join("termcraft.ico"), ico(&ico_images))?;
    let icns_images = ICNS_TYPES
        .iter()
        .map(|&(kind, s)| Ok((kind, png(s)?)))
        .collect::<Result<Vec<_>>>()?;
    std::fs::write(assets.join("termcraft.icns"), icns(&icns_images))?;
    std::fs::write(root.join("docs/icons/app.png"), png(128)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_a_white_prompt_on_blue() {
        let p = render(64).unwrap();
        let px = |x: u32, y: u32| p.pixel(x, y).unwrap();
        // Corners are transparent (rounded), the centre row has white.
        assert_eq!(px(0, 0).alpha(), 0);
        let row: Vec<_> = (0..64).map(|x| px(x, 32)).collect();
        assert!(
            row.iter()
                .any(|c| c.red() > 240 && c.green() > 240 && c.blue() > 240)
        );
        assert!(row.iter().any(|c| c.blue() > 150 && c.red() < 120));
    }

    #[test]
    fn ico_and_icns_layout() {
        let images = [(16, vec![1, 2, 3]), (256, vec![4, 5])];
        let ico = ico(&images);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(ico[6 + 16], 0, "256 is written as 0");
        assert_eq!(ico.len(), 6 + 32 + 5);
        let icns = icns(&[(b"ic08", vec![9; 10])]);
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()), 26);
        assert_eq!(&icns[8..12], b"ic08");
    }
}
