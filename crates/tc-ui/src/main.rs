#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod control_server;
mod credentials;
mod external;
mod folder_hook;
mod mosh;
mod popup;
mod remote_edit;
mod servers;
mod shared;
mod skin;
mod snapshot;
mod system_fonts;
mod tab;
mod term_fonts;
mod term_log;
mod term_settings;
mod term_view;
mod tray;
mod tunnels;
mod ui;
mod updates;

/// Create the window transparent only when a transparency is set: an
/// opaque window is cheaper and safe on every desktop.
pub(crate) fn wants_transparency(view: &tc_core::settings::ViewSettings) -> bool {
    view.transparency > 0 || view.detached_transparency > 0
}

fn main() -> eframe::Result<()> {
    // Command-line options: hand them to the running window if there is
    // one, else run them in this new one.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = tc_core::Paths::resolve();
    let macros = tc_core::MacroStore::load(&paths.macros_file()).unwrap_or_default();
    let startup = match control_server::commands_from_args(&args, &|name| {
        macros.macros.iter().find(|m| m.name == name).map(|m| m.id)
    }) {
        Ok(startup) => startup,
        Err(e) => {
            eprintln!("termcraft: {e}");
            std::process::exit(2);
        }
    };
    // A running window gets the commands; the startup flags only apply to
    // a window this process opens (so --exitwhendone never closes the
    // user's other sessions).
    if !startup.commands.is_empty() && forward(&paths, &startup.commands) {
        return Ok(());
    }
    let settings = tc_core::Settings::load(&paths.settings_file()).unwrap_or_default();
    // A second start shows the running window ("Allow multiple instances"
    // off): two windows would overwrite each other's sessions file.
    if startup.commands.is_empty()
        && !settings.general.multiple_instances
        && std::env::var_os("TERMCRAFT_SNAPSHOT").is_none()
        && forward(&paths, &[tc_commands::Command::WindowShow {}])
    {
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("TermCraft")
        .with_icon(std::sync::Arc::new(app_icon()))
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([480.0, 320.0]);
    if wants_transparency(&settings.view) {
        viewport = viewport.with_transparent(true);
    }
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
        Box::new(|cc| Ok(Box::new(app::TermCraftApp::new(cc, startup)))),
    )
}

/// The program icon (`cargo xtask app-icon`), for the window and taskbar.
pub(crate) fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png"))
        .expect("the bundled icon is a valid PNG")
}

/// Send the commands to an already running window. False when none runs.
fn forward(paths: &tc_core::Paths, commands: &[tc_commands::Command]) -> bool {
    let file = paths.control_file();
    for cmd in commands {
        match tc_commands::control::request(&file, cmd.clone()) {
            Ok(_) => {}
            // Nothing listening: start a window. Only the first command can
            // get here, as it already tells whether a window runs.
            Err(tc_commands::control::SendError::NotRunning) => return false,
            // A running window refused it (an unknown session...): say so,
            // and do not start a second window.
            Err(e) => eprintln!("termcraft: {e}"),
        }
    }
    true
}
