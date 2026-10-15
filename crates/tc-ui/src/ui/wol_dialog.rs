//! Tools › "Wake On Lan": MAC address, broadcast address and port, then
//! the `wake_on_lan` command.

use tc_commands::Command;

pub struct WolDialog {
    mac: String,
    address: String,
    port: u16,
    /// The result of the last send.
    pub message: Option<String>,
}

impl Default for WolDialog {
    fn default() -> Self {
        Self {
            mac: String::new(),
            address: "255.255.255.255".into(),
            port: tc_core::wol::DEFAULT_PORT,
            message: None,
        }
    }
}

pub enum Outcome {
    Open,
    Closed,
    Send(Command),
}

impl WolDialog {
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut open = true;
        let mut outcome = Outcome::Open;
        egui::Window::new("Wake On Lan")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("wol").num_columns(2).show(ui, |ui| {
                    ui.label("MAC address:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.mac)
                            .hint_text("aa:bb:cc:dd:ee:ff")
                            .desired_width(180.0),
                    );
                    ui.end_row();
                    ui.label("Broadcast address:");
                    ui.add(egui::TextEdit::singleline(&mut self.address).desired_width(180.0));
                    ui.end_row();
                    ui.label("Port:");
                    ui.add(egui::DragValue::new(&mut self.port).range(1..=65535));
                    ui.end_row();
                });
                let valid = tc_core::wol::parse_mac(&self.mac).is_some();
                ui.horizontal(|ui| {
                    let send = ui
                        .add_enabled(valid, egui::Button::new("Wake up"))
                        .on_disabled_hover_text("Type the MAC address of the computer");
                    if send.clicked() {
                        outcome = Outcome::Send(Command::WakeOnLan {
                            mac: self.mac.trim().to_string(),
                            address: Some(self.address.trim().to_string()),
                            port: Some(self.port),
                        });
                    }
                });
                if let Some(m) = &self.message {
                    ui.label(m);
                }
            });
        if open { outcome } else { Outcome::Closed }
    }
}
