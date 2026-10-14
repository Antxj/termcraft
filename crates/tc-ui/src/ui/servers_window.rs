//! "Servers management" (reference `TFORMSERVICES`, 722×429): one row per
//! network service with Start / Stop / Settings, and a settings page with
//! the "Server output" for each. TFTP and HTTP run; the other services of
//! the reference are listed disabled.

use super::{Action, NOT_YET, icon};
use crate::servers::Servers;
use tc_commands::Command;
use tc_core::settings::{ServersSettings, StopAfter};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Main,
    Tftp,
    Http,
}

/// The services of the reference, in its order: (label, page, available).
const ROWS: [(&str, Option<(Page, &str)>); 9] = [
    ("TFTP server", Some((Page::Tftp, "tftp"))),
    ("FTP server", None),
    ("HTTP server", Some((Page::Http, "http"))),
    ("SSH/SFTP server", None),
    ("Telnet server", None),
    ("NFS server", None),
    ("VNC server", None),
    ("Cron server", None),
    ("Iperf server", None),
];

/// Draw the window; returns true when a setting changed.
pub fn show(
    ctx: &egui::Context,
    open: &mut bool,
    page: &mut Page,
    settings: &mut ServersSettings,
    servers: &Servers,
    out: &mut Vec<Action>,
) -> bool {
    let before = settings.clone();
    egui::Window::new("Servers management")
        .id(egui::Id::new("servers-window"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(722.0)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(272.0);
                    rows(ui, page, servers, out);
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(412.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(page, Page::Main, "Main");
                        ui.selectable_value(page, Page::Tftp, "TFTP");
                        ui.selectable_value(page, Page::Http, "HTTP");
                    });
                    ui.separator();
                    match page {
                        Page::Main => main_page(ui),
                        Page::Tftp => tftp_page(ui, settings, servers),
                        Page::Http => http_page(ui, settings, servers),
                    }
                });
            });
        });
    *settings != before
}

fn rows(ui: &mut egui::Ui, page: &mut Page, servers: &Servers, out: &mut Vec<Action>) {
    egui::Grid::new("servers-rows")
        .num_columns(5)
        .spacing([6.0, 12.0])
        .show(ui, |ui| {
            for (label, target) in ROWS {
                match target {
                    Some((p, name)) => {
                        let running = servers.is_running(name);
                        let dot = if running {
                            egui::RichText::new(icon::CIRCLE)
                                .color(egui::Color32::from_rgb(46, 160, 67))
                        } else {
                            egui::RichText::new(icon::CIRCLE).weak()
                        };
                        ui.label(dot);
                        ui.label(label);
                        if ui
                            .add_enabled(!running, egui::Button::new(icon::PLAY))
                            .on_hover_text(format!("Start {label}"))
                            .clicked()
                        {
                            out.push(Action::Run(Command::ServerStart {
                                server: name.into(),
                            }));
                        }
                        if ui
                            .add_enabled(running, egui::Button::new(icon::STOP))
                            .on_hover_text(format!("Stop {label}"))
                            .clicked()
                        {
                            out.push(Action::Run(Command::ServerStop {
                                server: name.into(),
                            }));
                        }
                        if ui
                            .button(icon::GEAR)
                            .on_hover_text(format!("{label} settings"))
                            .clicked()
                        {
                            *page = p;
                        }
                    }
                    None => {
                        ui.label(egui::RichText::new(icon::CIRCLE).weak());
                        ui.add_enabled(false, egui::Label::new(label))
                            .on_disabled_hover_text(NOT_YET);
                        for glyph in [icon::PLAY, icon::STOP, icon::GEAR] {
                            ui.add_enabled(false, egui::Button::new(glyph))
                                .on_disabled_hover_text(NOT_YET);
                        }
                    }
                }
                ui.end_row();
            }
        });
}

fn main_page(ui: &mut egui::Ui) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(icon::HARD_DRIVES).size(48.0));
        ui.heading("Welcome to TermCraft Network Services");
        ui.label(
            "Share files with the devices of your network: start a server on the left, \
             its settings and output are on its page. Servers listen on every network \
             interface of this computer and only serve their root folder.",
        );
    });
}

fn root_dir(ui: &mut egui::Ui, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label("Root directory");
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text("termcraft-data/shared")
                .desired_width(260.0),
        );
        if ui.button(icon::FOLDER_OPEN).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            *value = dir.display().to_string();
        }
    });
}

fn port(ui: &mut egui::Ui, value: &mut u16) {
    ui.horizontal(|ui| {
        ui.label("Listening port");
        ui.add(egui::DragValue::new(value).range(1..=65535));
    });
}

fn stop_after(ui: &mut egui::Ui, value: &mut StopAfter) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut value.enabled, "Stop server after");
        ui.add_enabled(
            value.enabled,
            egui::DragValue::new(&mut value.seconds).range(1..=86_400),
        );
        ui.label("seconds");
    });
}

fn output(ui: &mut egui::Ui, servers: &Servers, name: &str) {
    ui.label("Server output");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt(("server-output", name))
            .max_height(100.0)
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in servers.log(name) {
                    ui.monospace(line);
                }
            });
    });
}

fn tftp_page(ui: &mut egui::Ui, s: &mut ServersSettings, servers: &Servers) {
    let t = &mut s.tftp;
    ui.strong("TFTP server settings");
    root_dir(ui, &mut t.root);
    ui.checkbox(
        &mut t.notify_download,
        "Show message after successful download",
    );
    ui.checkbox(&mut t.notify_upload, "Show message after successful upload");
    ui.horizontal(|ui| {
        port(ui, &mut t.port);
        ui.add_space(20.0);
        ui.label("TFTP buffer size");
        ui.add(egui::DragValue::new(&mut t.buffer_size).range(512..=65_464));
        ui.label("Bytes");
    });
    stop_after(ui, &mut t.stop_after);
    output(ui, servers, "tftp");
}

fn http_page(ui: &mut egui::Ui, s: &mut ServersSettings, servers: &Servers) {
    let h = &mut s.http;
    ui.strong("HTTP server settings");
    root_dir(ui, &mut h.root);
    ui.checkbox(
        &mut h.list_directories,
        "Allow users to list directory content",
    );
    port(ui, &mut h.port);
    stop_after(ui, &mut h.stop_after);
    output(ui, servers, "http");
}
