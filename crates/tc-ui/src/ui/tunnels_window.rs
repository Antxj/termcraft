//! SSH tunnels window (reference `TFORMPORTFORWARDING` / MobaSSHTunnel) and
//! the tunnel editor (`TFORMPORTFORWARDINGEDIT`).

use super::{Action, icon};
use crate::tunnels::RunningTunnel;
use std::collections::BTreeMap;
use tc_commands::Command;
use tc_core::{Tunnel, TunnelId, TunnelKind};

pub fn manager(
    ctx: &egui::Context,
    open: &mut bool,
    tunnels: &[Tunnel],
    running: &BTreeMap<TunnelId, RunningTunnel>,
    editor: &mut Option<TunnelEditor>,
    out: &mut Vec<Action>,
) {
    egui::Window::new("TermCraft SSH tunnels")
        .open(open)
        .default_width(980.0)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.heading("Graphical port forwarding tool");
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    egui::Grid::new("tunnels")
                        .num_columns(8)
                        .striped(true)
                        .spacing([14.0, 6.0])
                        .show(ui, |ui| {
                            for h in [
                                "Start/stop",
                                "Name",
                                "Settings",
                                "Type",
                                "Forward port",
                                "Destination server",
                                "SSH server",
                                "",
                            ] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            for t in tunnels {
                                row(ui, t, running.get(&t.id), editor, out);
                            }
                        });
                    if tunnels.is_empty() {
                        ui.weak("No SSH tunnels yet. Click \"New SSH tunnel\" to create one.");
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{}  New SSH tunnel", icon::PLUS))
                    .clicked()
                {
                    *editor = Some(TunnelEditor::new());
                }
                if ui
                    .button(format!("{}  Start all tunnels", icon::PLAY))
                    .clicked()
                {
                    for t in tunnels.iter().filter(|t| !running.contains_key(&t.id)) {
                        out.push(Action::Run(Command::TunnelStart { id: t.id }));
                    }
                }
                if ui
                    .button(format!("{}  Stop all tunnels", icon::STOP))
                    .clicked()
                {
                    for id in running.keys() {
                        out.push(Action::Run(Command::TunnelStop { id: *id }));
                    }
                }
            });
        });
}

fn row(
    ui: &mut egui::Ui,
    t: &Tunnel,
    state: Option<&RunningTunnel>,
    editor: &mut Option<TunnelEditor>,
    out: &mut Vec<Action>,
) {
    let running = state.is_some_and(|s| !s.closed);
    let (glyph, hint) = if running {
        (icon::STOP, "Stop tunnel")
    } else {
        (icon::PLAY, "Start tunnel")
    };
    if ui.button(glyph).on_hover_text(hint).clicked() {
        out.push(Action::Run(if running {
            Command::TunnelStop { id: t.id }
        } else {
            Command::TunnelStart { id: t.id }
        }));
    }
    ui.vertical(|ui| {
        ui.set_min_width(220.0);
        ui.label(&t.name);
        if let Some(s) = state {
            let color = if s.listening {
                ui.visuals().widgets.active.fg_stroke.color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.add(
                egui::Label::new(egui::RichText::new(&s.status).small().color(color)).truncate(),
            )
            .on_hover_text(&s.status);
        }
    });
    if ui
        .button(icon::PENCIL_SIMPLE)
        .on_hover_text("Edit tunnel")
        .clicked()
    {
        *editor = Some(TunnelEditor::edit(t));
    }
    ui.label(t.kind.label());
    ui.label(format!("{}:{}", t.bind, t.forward_port));
    ui.label(t.destination());
    ui.label(t.ssh_server());
    if ui
        .button(icon::TRASH)
        .on_hover_text("Delete tunnel")
        .clicked()
    {
        out.push(Action::Run(Command::TunnelRemove { id: t.id }));
    }
    ui.end_row();
}

pub struct TunnelEditor {
    tunnel: Tunnel,
    is_new: bool,
    forward_port: String,
    dest_port: String,
    ssh_port: String,
    key: String,
    error: Option<String>,
}

impl TunnelEditor {
    pub fn new() -> Self {
        Self::from(Tunnel::new(""), true)
    }

    pub fn edit(t: &Tunnel) -> Self {
        Self::from(t.clone(), false)
    }

    fn from(tunnel: Tunnel, is_new: bool) -> Self {
        let port = |p: u16| if p == 0 { String::new() } else { p.to_string() };
        Self {
            forward_port: port(tunnel.forward_port),
            dest_port: port(tunnel.dest_port),
            ssh_port: tunnel.ssh_port.to_string(),
            key: tunnel.private_key.clone().unwrap_or_default(),
            tunnel,
            is_new,
            error: None,
        }
    }

    fn build(&self) -> Result<Tunnel, String> {
        let port = |s: &str, what: &str| -> Result<u16, String> {
            match s.trim() {
                "" => Ok(0),
                v => v.parse().map_err(|_| format!("{what}: not a port number")),
            }
        };
        let mut t = self.tunnel.clone();
        t.forward_port = port(&self.forward_port, "Forwarded port")?;
        t.dest_port = port(&self.dest_port, "Remote port")?;
        t.ssh_port = port(&self.ssh_port, "SSH port")?;
        if t.ssh_port == 0 {
            t.ssh_port = 22;
        }
        t.private_key = Some(self.key.trim().to_string()).filter(|k| !k.is_empty());
        if t.name.trim().is_empty() {
            t.name = format!("{} {}", t.kind.label(), t.forward_port);
        }
        t.validate()?;
        Ok(t)
    }

    /// Returns false when the editor should close.
    pub fn show(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) -> bool {
        let mut keep = true;
        let mut open = true;
        egui::Window::new(if self.is_new { "New SSH tunnel" } else { "Edit SSH tunnel" })
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let enter = crate::ui::EnterWatch::start(ui);
                ui.horizontal(|ui| {
                    let k = &mut self.tunnel.kind;
                    ui.radio_value(k, TunnelKind::Local, "Local port forwarding");
                    ui.radio_value(k, TunnelKind::Remote, "Remote port forwarding");
                    ui.radio_value(k, TunnelKind::Dynamic, "Dynamic port forwarding (SOCKS proxy)");
                });
                ui.weak(match self.tunnel.kind {
                    TunnelKind::Local => "Local port forwarding: connection from local clients to remote server",
                    TunnelKind::Remote => "Remote port forwarding: connection from remote clients to a local server",
                    TunnelKind::Dynamic => "Dynamic port forwarding: a SOCKS proxy here, all traffic through the SSH server",
                });
                ui.separator();
                egui::Grid::new("tunnel-edit").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut self.tunnel.name);
                    ui.end_row();
                    let here = self.tunnel.kind != TunnelKind::Remote;
                    ui.label(if here { "Forwarded port (on my computer)" } else { "Forwarded port (on the SSH server)" });
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.tunnel.bind).desired_width(110.0));
                        ui.label(":");
                        ui.add(egui::TextEdit::singleline(&mut self.forward_port).hint_text("<Forwarded port>").desired_width(70.0));
                    });
                    ui.end_row();
                    if self.tunnel.kind != TunnelKind::Dynamic {
                        ui.label("Remote server");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.tunnel.dest_host).hint_text("<Remote server>").desired_width(160.0));
                            ui.label(":");
                            ui.add(egui::TextEdit::singleline(&mut self.dest_port).hint_text("<Remote port>").desired_width(70.0));
                        });
                        ui.end_row();
                    }
                    ui.label("SSH server");
                    ui.add(egui::TextEdit::singleline(&mut self.tunnel.ssh_host).hint_text("<SSH server>"));
                    ui.end_row();
                    ui.label("SSH login");
                    ui.add(egui::TextEdit::singleline(&mut self.tunnel.ssh_user).hint_text("<SSH login>"));
                    ui.end_row();
                    ui.label("SSH port");
                    ui.add(egui::TextEdit::singleline(&mut self.ssh_port).hint_text("<SSH port>").desired_width(70.0));
                    ui.end_row();
                    ui.label("Private key");
                    ui.add(egui::TextEdit::singleline(&mut self.key).hint_text("optional path"));
                    ui.end_row();
                });
                ui.checkbox(&mut self.tunnel.autostart, "Start this tunnel when TermCraft starts");
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || enter.pressed(ui) {
                        match self.build() {
                            Ok(tunnel) => {
                                out.push(Action::Run(if self.is_new {
                                    Command::TunnelAdd { tunnel }
                                } else {
                                    Command::TunnelUpdate { tunnel }
                                }));
                                keep = false;
                            }
                            Err(e) => self.error = Some(e),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
        keep && open
    }
}

/// Host key and password questions of running tunnels.
pub fn prompts(ctx: &egui::Context, running: &mut BTreeMap<TunnelId, RunningTunnel>) {
    for (id, t) in running.iter_mut() {
        if let Some(info) = &t.host_key
            && let Some(a) =
                super::host_key_dialog::show(ctx, egui::Id::new(("tunnel-hk", *id)), info)
        {
            t.answer_host_key(a.accept, a.remember);
        }
        if let Some(q) = &mut t.secret {
            let mut answer = None;
            egui::Window::new(format!("Tunnel {}", t.name))
                .id(egui::Id::new(("tunnel-pw", *id)))
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(&q.prompt);
                    let edit = ui.add(egui::TextEdit::singleline(&mut q.input).password(true));
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || enter {
                            answer = Some(Some(std::mem::take(&mut q.input)));
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(None);
                        }
                    });
                });
            if let Some(a) = answer {
                t.answer_secret(a);
            }
        }
    }
}
