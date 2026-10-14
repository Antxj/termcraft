#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod credentials;
mod external;
mod folder_hook;
mod mosh;
mod skin;
mod snapshot;
mod tab;
mod term_settings;
mod term_view;
mod tunnels;
mod ui;
mod updates;

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("TermCraft")
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([480.0, 320.0]);
    if std::env::var_os("TERMCRAFT_SNAPSHOT").is_some() {
        // Snapshot runs must not get in the way of whatever the user is
        // doing: no focus, below other windows, clicks pass through.
        viewport = viewport
            .with_active(false)
            .with_window_level(egui::WindowLevel::AlwaysOnBottom)
            .with_mouse_passthrough(true);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "TermCraft",
        options,
        Box::new(|cc| Ok(Box::new(app::TermCraftApp::new(cc)))),
    )
}
