#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod credentials;
mod external;
mod snapshot;
mod tab;
mod term_settings;
mod term_view;
mod tunnels;
mod ui;
mod updates;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TermCraft")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([480.0, 320.0])
            // Snapshot runs must not steal focus from whatever the user is doing.
            .with_active(std::env::var_os("TERMCRAFT_SNAPSHOT").is_none()),
        ..Default::default()
    };
    eframe::run_native(
        "TermCraft",
        options,
        Box::new(|cc| Ok(Box::new(app::TermCraftApp::new(cc)))),
    )
}
