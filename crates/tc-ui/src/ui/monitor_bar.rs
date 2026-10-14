//! "Remote monitoring" bar below an SSH terminal: CPU, RAM, network, disk,
//! uptime and logged-in users of the server.

use super::icon;
use tc_core::monitor::{Sample, human_bytes, human_rate, human_uptime};

pub const HEIGHT: f32 = 22.0;

fn kib(v: u64) -> String {
    human_bytes(v as f64 * 1024.0)
}

/// The bar's items, in order (text only, for tests and tooltips).
pub fn items(s: &Sample) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(cpu) = s.cpu_percent {
        out.push((icon::CPU, format!("CPU {cpu:.0}%")));
    }
    if let (Some(used), Some(total)) = (s.mem_used, s.mem_total) {
        out.push((icon::MEMORY, format!("RAM {} / {}", kib(used), kib(total))));
    }
    if let (Some(rx), Some(tx)) = (s.rx_rate, s.tx_rate) {
        out.push((
            icon::ARROWS_DOWN_UP,
            format!(
                "{} {}  {} {}",
                icon::ARROW_DOWN,
                human_rate(rx),
                icon::ARROW_UP,
                human_rate(tx)
            ),
        ));
    }
    if let (Some(used), Some(total)) = (s.disk_used, s.disk_total) {
        let pct = used as f64 * 100.0 / total as f64;
        out.push((icon::HARD_DRIVE, format!("/ {pct:.0}% of {}", kib(total))));
    }
    if let Some(up) = s.uptime_secs {
        out.push((icon::CLOCK, format!("Up {}", human_uptime(up))));
    }
    if let Some(users) = s.users {
        out.push((icon::USERS, format!("{users} user(s)")));
    }
    out
}

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, sample: Option<&Sample>) {
    let palette = crate::skin::current(ui.ctx());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, palette.chrome);
    painter.hline(
        rect.x_range(),
        rect.top(),
        egui::Stroke::new(1.0, palette.border),
    );
    let font = egui::FontId::proportional(12.0);
    let mut x = rect.left() + 8.0;
    let y = rect.center().y;
    let list = sample.map(items).unwrap_or_default();
    if list.is_empty() {
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            "Remote monitoring: waiting for data (Linux servers only)…",
            font,
            palette.weak_text,
        );
        return;
    }
    for (glyph, text) in list {
        let g = painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            glyph,
            font.clone(),
            palette.accent,
        );
        x = g.right() + 4.0;
        let t = painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            text,
            font.clone(),
            palette.text,
        );
        x = t.right() + 16.0;
        if x > rect.right() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_skip_missing_values() {
        assert!(items(&Sample::default()).is_empty());
        let s = Sample {
            cpu_percent: Some(12.4),
            mem_used: Some(1024 * 1024),
            mem_total: Some(4 * 1024 * 1024),
            disk_used: Some(50),
            disk_total: Some(100),
            ..Default::default()
        };
        let texts: Vec<String> = items(&s).into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            texts,
            ["CPU 12%", "RAM 1.0 GB / 4.0 GB", "/ 50% of 100.0 KB"]
        );
    }
}
