//! Home screen shown when no tab is open (reference `TFRAMEHOME`): logo and
//! title, "Start local terminal", session search, and a 3×3 grid of recent
//! sessions.

use super::{Action, icon, session_icon};
use tc_commands::Command;
use tc_core::Session;

const GRID: usize = 9;

pub fn show(
    ui: &mut egui::Ui,
    search: &mut String,
    sessions: &[Session],
    recent: &[&Session],
    out: &mut Vec<Action>,
) {
    let width = 577.0_f32.min(ui.available_width() - 32.0);
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.18).max(16.0));
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - 260.0) / 2.0).max(0.0));
            ui.label(egui::RichText::new(icon::TERMINAL_WINDOW).size(56.0));
            ui.label(egui::RichText::new("TermCraft").size(34.0).strong());
        });
        ui.add_space(16.0);

        let start = egui::Button::new(
            egui::RichText::new(format!("{}  Start local terminal", icon::TERMINAL_WINDOW))
                .size(15.0),
        )
        .min_size(egui::vec2(201.0, 33.0));
        if ui.add(start).clicked() {
            out.push(Action::Run(Command::TabOpen { session: None }));
        }
        ui.add_space(12.0);

        let response = ui.add(
            egui::TextEdit::singleline(search)
                .hint_text(format!(
                    "{}  Find existing session or server name...",
                    icon::MAGNIFYING_GLASS
                ))
                .desired_width(403.0_f32.min(width)),
        );
        let query = search.trim().to_lowercase();
        if !query.is_empty() {
            let hits: Vec<&Session> = sessions
                .iter()
                .filter(|s| s.name.to_lowercase().contains(&query))
                .take(8)
                .collect();
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let action = match hits.first() {
                    Some(s) => open(s),
                    None => Action::QuickConnect(search.trim().to_string()),
                };
                out.push(action);
                search.clear();
            }
            for s in &hits {
                if ui.button(label(s)).clicked() {
                    out.push(open(s));
                }
            }
            if hits.is_empty() {
                ui.weak("No session found. Press Enter to connect to this server over SSH.");
            }
        }
        ui.add_space(24.0);

        ui.label(egui::RichText::new("Recent sessions").strong());
        ui.add_space(4.0);
        let cell = egui::vec2(186.0, 24.0);
        let columns = recent.len().clamp(1, 3) as f32;
        let grid_width = columns * cell.x + (columns - 1.0) * 2.0;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - grid_width) / 2.0).max(0.0));
            recent_grid(ui, recent, cell, out);
        });
    });
}

fn recent_grid(ui: &mut egui::Ui, recent: &[&Session], cell: egui::Vec2, out: &mut Vec<Action>) {
    egui::Grid::new("recent-sessions")
        .num_columns(3)
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            if recent.is_empty() {
                let local = egui::Button::new(format!("{}  Local terminal", icon::TERMINAL_WINDOW))
                    .min_size(cell);
                if ui.add(local).clicked() {
                    out.push(Action::Run(Command::TabOpen { session: None }));
                }
            }
            for (i, s) in recent.iter().take(GRID).enumerate() {
                if ui.add(egui::Button::new(label(s)).min_size(cell)).clicked() {
                    out.push(open(s));
                }
                if i % 3 == 2 {
                    ui.end_row();
                }
            }
        });
}

fn label(s: &Session) -> String {
    format!("{}  {}", session_icon(s), s.name)
}

fn open(s: &Session) -> Action {
    Action::Run(Command::TabOpen {
        session: Some(s.id.to_string()),
    })
}
