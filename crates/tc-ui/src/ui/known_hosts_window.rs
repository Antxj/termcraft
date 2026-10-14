//! "Known hosts": the SSH host keys TermCraft trusts, with a filter and
//! Delete (through the `known_host_remove` command).

use super::{Action, icon};
use std::path::Path;
use tc_commands::Command;
use tc_core::known_hosts::{self, KnownHost};

#[derive(Default)]
pub struct KnownHostsWindow {
    pub open: bool,
    list: Option<Result<Vec<KnownHost>, String>>,
    filter: String,
}

impl KnownHostsWindow {
    pub fn open(&mut self) {
        self.open = true;
        self.list = None;
    }

    /// Read the file again (after a change).
    pub fn refresh(&mut self) {
        self.list = None;
    }

    pub fn show(&mut self, ctx: &egui::Context, file: &Path, out: &mut Vec<Action>) {
        if !self.open {
            return;
        }
        let list = self
            .list
            .get_or_insert_with(|| known_hosts::load(file).map_err(|e| e.to_string()));
        let mut open = true;
        let mut removed = false;
        egui::Window::new("Known hosts")
            .id(egui::Id::new("known-hosts"))
            .open(&mut open)
            .collapsible(false)
            .default_width(720.0)
            .max_width(760.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(icon::MAGNIFYING_GLASS);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text("Filter by host or fingerprint")
                            .desired_width(280.0),
                    );
                    ui.add(
                        egui::Label::new(egui::RichText::new(file.display().to_string()).weak())
                            .truncate(),
                    );
                });
                ui.separator();
                match list {
                    Err(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                    }
                    Ok(hosts) if hosts.is_empty() => {
                        ui.weak("No host key is trusted yet.");
                    }
                    Ok(hosts) => {
                        let needle = self.filter.to_lowercase();
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            egui::Grid::new("known-hosts-grid")
                                .num_columns(4)
                                .striped(true)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    ui.strong("Host");
                                    ui.strong("Key type");
                                    ui.strong("Fingerprint");
                                    ui.label("");
                                    ui.end_row();
                                    for h in hosts.iter().filter(|h| {
                                        needle.is_empty()
                                            || h.hosts.to_lowercase().contains(&needle)
                                            || h.fingerprint.to_lowercase().contains(&needle)
                                    }) {
                                        let host = match &h.marker {
                                            Some(m) => format!("{m} {}", h.hosts),
                                            None => h.hosts.clone(),
                                        };
                                        ui.label(host);
                                        ui.label(&h.key_type);
                                        ui.monospace(&h.fingerprint);
                                        if ui
                                            .small_button(icon::TRASH)
                                            .on_hover_text("Forget this host key")
                                            .clicked()
                                        {
                                            out.push(Action::Run(Command::KnownHostRemove {
                                                line: h.line,
                                                fingerprint: h.fingerprint.clone(),
                                            }));
                                            removed = true;
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                    }
                }
                ui.separator();
                ui.weak(
                    "A forgotten host is validated again (fingerprint dialog) at the next connection.",
                );
            });
        if removed {
            self.refresh();
        }
        self.open = open;
    }
}
