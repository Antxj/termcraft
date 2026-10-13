#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod tab;
mod term_view;
mod ui;
mod updates;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TermCraft")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "TermCraft",
        options,
        Box::new(|cc| Ok(Box::new(app::TermCraftApp::new(cc)))),
    )
}
