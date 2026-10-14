//! Image viewer for files opened from the SSH-browser (PNG, JPEG, GIF,
//! BMP). Decoding is bounded (file size, dimensions, memory), since the
//! files come from other machines.

use std::path::Path;

/// Largest image file opened.
const MAX_BYTES: u64 = 50 * 1024 * 1024;
/// Largest width / height decoded.
const MAX_SIDE: u32 = 16_384;

/// Images the viewer opens, by extension.
pub fn is_image(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".bmp"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// Decode an image to RGBA with limits.
pub fn decode(path: &Path) -> Result<egui::ColorImage, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_BYTES {
        return Err(format!("{} is larger than 50 MB", path.display()));
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let rgba = reader.decode().map_err(|e| e.to_string())?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_raw(),
    ))
}

struct Shown {
    title: String,
    texture: egui::TextureHandle,
    size: egui::Vec2,
}

#[derive(Default)]
pub struct ImageViewer {
    shown: Option<Shown>,
    zoom: f32,
    error: Option<String>,
}

impl ImageViewer {
    pub fn open(&mut self, ctx: &egui::Context, path: &Path, title: &str) {
        match decode(path) {
            Ok(image) => {
                let size = egui::vec2(image.size[0] as f32, image.size[1] as f32);
                let texture = ctx.load_texture(title, image, egui::TextureOptions::LINEAR);
                self.shown = Some(Shown {
                    title: title.to_string(),
                    texture,
                    size,
                });
                self.zoom = 0.0; // fit
                self.error = None;
            }
            Err(e) => {
                self.shown = None;
                self.error = Some(e);
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if let Some(e) = self.error.clone() {
            let mut open = true;
            egui::Window::new("Image viewer")
                .id(egui::Id::new("image-viewer-error"))
                .open(&mut open)
                .show(ctx, |ui| ui.colored_label(ui.visuals().error_fg_color, e));
            if !open {
                self.error = None;
            }
        }
        let Some(shown) = &self.shown else { return };
        let mut open = true;
        let title = format!("{} ({}×{})", shown.title, shown.size.x, shown.size.y);
        egui::Window::new(title)
            .id(egui::Id::new("image-viewer"))
            .open(&mut open)
            .default_size([720.0, 520.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Fit").clicked() {
                        self.zoom = 0.0;
                    }
                    if ui.button("100 %").clicked() {
                        self.zoom = 1.0;
                    }
                    if ui.button("−").clicked() {
                        self.zoom = (self.zoom.max(0.05) / 1.25).max(0.05);
                    }
                    if ui.button("+").clicked() {
                        self.zoom = (self.zoom.max(0.05) * 1.25).min(16.0);
                    }
                });
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let avail = ui.available_size();
                        let scale = if self.zoom <= 0.0 {
                            (avail.x / shown.size.x)
                                .min(avail.y / shown.size.y)
                                .min(1.0)
                        } else {
                            self.zoom
                        };
                        ui.add(
                            egui::Image::new(&shown.texture).fit_to_exact_size(shown.size * scale),
                        );
                    });
            });
        if !open {
            self.shown = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_with_limits() {
        assert!(is_image("a/B.JPG") && is_image("x.png") && !is_image("x.svg"));
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("p.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
            .save(&png)
            .unwrap();
        let img = decode(&png).unwrap();
        assert_eq!(img.size, [3, 2]);
        let junk = dir.path().join("j.png");
        std::fs::write(&junk, b"not an image").unwrap();
        assert!(decode(&junk).is_err());
    }
}
