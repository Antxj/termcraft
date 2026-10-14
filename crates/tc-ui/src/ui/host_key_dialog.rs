//! Host key validation in the PuTTY style the reference uses: the security
//! alert with Accept / Connect Once / More info… / Cancel, and "Host key
//! validation - more information" (reference `TFORMHKFINGERPRINTDIALOG`)
//! with both fingerprints, the full key and a paste-to-compare field.
//! Shared by terminal tabs and tunnels.

use super::icon;
use tc_ssh::HostKeyInfo;

/// The user's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub accept: bool,
    /// Save the key in known_hosts ("Accept"), not just this once.
    pub remember: bool,
}

#[derive(Clone, Default)]
struct State {
    more: bool,
    pasted: String,
}

/// Show the alert for `info`; `id` keeps per-prompt state (one per tab or
/// tunnel). Returns the decision once a button is pressed.
pub fn show(ctx: &egui::Context, id: egui::Id, info: &HostKeyInfo) -> Option<Answer> {
    let mut state: State = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut answer = None;
    let changed = info.changed_line.is_some();
    egui::Window::new(if changed {
        "TermCraft Security Alert"
    } else {
        "TermCraft - Host key validation"
    })
    .id(id)
    .collapsible(false)
    .resizable(false)
    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
    .min_width(640.0)
    .show(ctx, |ui| {
        ui.horizontal(|ui| {
            let (glyph, color) = if changed {
                (icon::WARNING, ui.visuals().error_fg_color)
            } else {
                (icon::SHIELD_WARNING, ui.visuals().warn_fg_color)
            };
            ui.label(egui::RichText::new(glyph).size(44.0).color(color));
            ui.vertical(|ui| alert_text(ui, info));
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(60.0);
            if ui.button("Accept").clicked() {
                answer = Some(Answer {
                    accept: true,
                    remember: true,
                });
            }
            if ui.button("Connect Once").clicked() {
                answer = Some(Answer {
                    accept: true,
                    remember: false,
                });
            }
            if ui.button("More info...").clicked() {
                state.more = true;
            }
            let cancel = ui.button("Cancel");
            if changed {
                // The only guaranteed safe choice is the default.
                cancel.request_focus();
            }
            if cancel.clicked() {
                answer = Some(Answer {
                    accept: false,
                    remember: false,
                });
            }
        });
    });
    if ui_escape(ctx) && answer.is_none() && !state.more {
        answer = Some(Answer {
            accept: false,
            remember: false,
        });
    }
    if state.more {
        more_info(ctx, id, info, &mut state);
    }
    if answer.is_some() {
        ctx.data_mut(|d| d.remove::<State>(id));
    } else {
        ctx.data_mut(|d| d.insert_temp(id, state));
    }
    answer
}

fn ui_escape(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

fn alert_text(ui: &mut egui::Ui, info: &HostKeyInfo) {
    let server = format!("    {} (port {})", info.host, info.port);
    let fingerprint = format!("    {} {}", info.algorithm, info.sha256);
    if info.changed_line.is_some() {
        ui.label(
            egui::RichText::new("WARNING - POTENTIAL SECURITY BREACH!")
                .strong()
                .color(ui.visuals().error_fg_color),
        );
        ui.label("The host key does not match the one TermCraft has cached for this server:");
        ui.monospace(server);
        ui.label(
            "This means that either the server administrator has changed the host key, \
             or you have actually connected to another computer pretending to be the server.",
        );
        ui.label(format!("The new {} key fingerprint is:", info.algorithm));
        ui.monospace(fingerprint);
        ui.label(
            "If you were expecting this change and trust the new key, press \"Accept\" to \
             update TermCraft's cache and continue connecting.",
        );
        ui.label(
            "If you want to carry on connecting but without updating the cache, press \
             \"Connect Once\".",
        );
        ui.label(
            "If you want to abandon the connection completely, press \"Cancel\". Pressing \
             \"Cancel\" is the ONLY guaranteed safe choice.",
        );
    } else {
        ui.label("The host key is not cached for this server:");
        ui.monospace(server);
        ui.label("You have no guarantee that the server is the computer you think it is.");
        ui.label(format!(
            "The server's {} key fingerprint is:",
            info.algorithm
        ));
        ui.monospace(fingerprint);
        ui.label(
            "If you trust this host, press \"Accept\" to add the key to TermCraft's cache \
             and carry on connecting.",
        );
        ui.label(
            "If you want to carry on connecting just once, without adding the key to the \
             cache, press \"Connect Once\".",
        );
        ui.label("If you do not trust this host, press \"Cancel\" to abandon the connection.");
    }
}

/// "Host key validation - more information".
fn more_info(ctx: &egui::Context, id: egui::Id, info: &HostKeyInfo, state: &mut State) {
    let mut open = true;
    egui::Window::new("Host key validation - more information")
        .id(id.with("more"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(640.0)
        .show(ctx, |ui| {
            egui::Grid::new(id.with("more-grid"))
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("SHA256 fingerprint:");
                    read_only(ui, &info.sha256, 1);
                    ui.end_row();
                    ui.label("MD5 fingerprint:");
                    read_only(ui, &format!("MD5:{}", info.md5), 1);
                    ui.end_row();
                    ui.label("Full public key:");
                    read_only(ui, &info.public_key, 4);
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.label("You can verify the expected host key by pasting it below:");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.pasted)
                        .hint_text("SHA256:…, MD5 fingerprint or the public key")
                        .desired_width(520.0),
                );
                if !state.pasted.trim().is_empty() {
                    if info.matches(&state.pasted) {
                        ui.label(
                            egui::RichText::new(icon::CHECK_CIRCLE)
                                .color(egui::Color32::from_rgb(0x2e, 0xa0, 0x43)),
                        )
                        .on_hover_text("This is the expected key");
                    } else {
                        ui.label(
                            egui::RichText::new(icon::X_CIRCLE).color(ui.visuals().error_fg_color),
                        )
                        .on_hover_text("This is NOT the expected key");
                    }
                }
            });
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                if ui.button("Close").clicked() {
                    state.more = false;
                }
            });
        });
    if !open {
        state.more = false;
    }
}

fn read_only(ui: &mut egui::Ui, text: &str, rows: usize) {
    let mut shown = text.to_string();
    ui.add(
        egui::TextEdit::multiline(&mut shown)
            .font(egui::TextStyle::Monospace)
            .desired_rows(rows)
            .desired_width(470.0)
            .interactive(true),
    );
}
