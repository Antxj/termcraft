//! Tools › "Diff tool": compare two text files side by side, and
//! Tools › "Ascii table".

use super::icon;
use std::path::PathBuf;
use tc_core::diff::{Row, RowKind, side_by_side, summary};

#[derive(Default)]
pub struct DiffWindow {
    pub open: bool,
    left: String,
    right: String,
    rows: Vec<Row>,
    error: Option<String>,
}

fn file_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(420.0));
        if ui.button(icon::FOLDER_OPEN).clicked()
            && let Some(p) = rfd::FileDialog::new().pick_file()
        {
            *value = p.display().to_string();
        }
    });
}

impl DiffWindow {
    /// Snapshots: two sample configurations compared.
    pub fn preview(&mut self) {
        let dir = std::env::temp_dir().join("termcraft-diff-preview");
        let _ = std::fs::create_dir_all(&dir);
        let (a, b) = (dir.join("sshd_config.old"), dir.join("sshd_config"));
        let _ = std::fs::write(
            &a,
            "Port 22
PermitRootLogin yes
PasswordAuthentication yes
X11Forwarding no
",
        );
        let _ = std::fs::write(
            &b,
            "Port 22
PermitRootLogin no
PasswordAuthentication yes
X11Forwarding yes
AllowUsers ops
",
        );
        self.left = a.display().to_string();
        self.right = b.display().to_string();
        self.compare();
        self.open = true;
    }

    fn compare(&mut self) {
        let read = |p: &str| crate::ui::text_editor::read_text(&PathBuf::from(p.trim()));
        match (read(&self.left), read(&self.right)) {
            (Ok(a), Ok(b)) => {
                self.rows = side_by_side(&a, &b);
                self.error = None;
            }
            (Err(e), _) | (_, Err(e)) => {
                self.rows.clear();
                self.error = Some(e);
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = true;
        egui::Window::new("Diff tool")
            .id(egui::Id::new("diff-tool"))
            .open(&mut open)
            .default_size([900.0, 560.0])
            .resizable(true)
            .show(ctx, |ui| {
                file_field(ui, "First file: ", &mut self.left);
                file_field(ui, "Second file:", &mut self.right);
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{}  Compare", icon::ARROWS_LEFT_RIGHT))
                        .clicked()
                    {
                        self.compare();
                    }
                    if !self.rows.is_empty() {
                        let s = summary(&self.rows);
                        if s == Default::default() {
                            ui.label("The files are identical.");
                        } else {
                            ui.label(format!(
                                "{} changed, {} only in the first file, {} only in the second",
                                s.changed, s.removed, s.added
                            ));
                        }
                    }
                });
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.separator();
                rows(ui, &self.rows);
            });
        self.open = open;
    }
}

fn rows(ui: &mut egui::Ui, rows: &[Row]) {
    let dark = ui.visuals().dark_mode;
    let tint = |light: [u8; 3], dark_c: [u8; 3]| {
        let [r, g, b] = if dark { dark_c } else { light };
        egui::Color32::from_rgb(r, g, b)
    };
    let (changed, removed, added) = (
        tint([255, 243, 205], [80, 70, 30]),
        tint([255, 220, 220], [90, 40, 40]),
        tint([220, 245, 220], [35, 80, 45]),
    );
    let height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::both()
        .id_salt("diff-rows")
        .auto_shrink([false, false])
        .show_rows(ui, height, rows.len(), |ui, range| {
            let half = (ui.available_width() / 2.0 - 8.0).max(200.0);
            for row in &rows[range] {
                let fill = match row.kind {
                    RowKind::Same => egui::Color32::TRANSPARENT,
                    RowKind::Changed => changed,
                    RowKind::Removed => removed,
                    RowKind::Added => added,
                };
                ui.horizontal(|ui| {
                    for side in [&row.left, &row.right] {
                        let text = side
                            .as_ref()
                            .map_or_else(String::new, |(n, t)| format!("{n:>5}  {t}"));
                        egui::Frame::new().fill(fill).show(ui, |ui| {
                            ui.set_width(half);
                            ui.add(
                                egui::Label::new(egui::RichText::new(text).monospace()).truncate(),
                            );
                        });
                    }
                });
            }
        });
}

/// Tools › "Ascii table": codes 0–127 with their control names.
#[derive(Default)]
pub struct AsciiTable {
    pub open: bool,
}

const CONTROLS: [&str; 33] = [
    "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "BS", "HT", "LF", "VT", "FF", "CR",
    "SO", "SI", "DLE", "DC1", "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM", "SUB", "ESC",
    "FS", "GS", "RS", "US", "SP",
];

/// What code `c` shows as.
pub fn ascii_name(c: u8) -> String {
    match c {
        0..=32 => CONTROLS[usize::from(c)].to_string(),
        127 => "DEL".into(),
        _ => (c as char).to_string(),
    }
}

impl AsciiTable {
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = true;
        egui::Window::new("Ascii table")
            .id(egui::Id::new("ascii-table"))
            .open(&mut open)
            .default_size([560.0, 480.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("ascii-grid")
                        .striped(true)
                        .num_columns(8)
                        .spacing([14.0, 2.0])
                        .show(ui, |ui| {
                            for _ in 0..2 {
                                for h in ["Dec", "Hex", "Oct", "Char"] {
                                    ui.strong(h);
                                }
                            }
                            ui.end_row();
                            for c in 0u8..64 {
                                for code in [c, c + 64] {
                                    ui.monospace(code.to_string());
                                    ui.monospace(format!("{code:02X}"));
                                    ui.monospace(format!("{code:03o}"));
                                    ui.monospace(ascii_name(code));
                                }
                                ui.end_row();
                            }
                        });
                });
            });
        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_names() {
        assert_eq!(ascii_name(0), "NUL");
        assert_eq!(ascii_name(27), "ESC");
        assert_eq!(ascii_name(32), "SP");
        assert_eq!(ascii_name(65), "A");
        assert_eq!(ascii_name(127), "DEL");
    }
}
