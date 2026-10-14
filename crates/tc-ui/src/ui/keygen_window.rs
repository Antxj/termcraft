//! "SSH key generator" (Tools › Network), laid out like PuTTYgen /
//! MobaKeyGen: the key, actions (generate, load, save) and parameters.

use crossbeam_channel::Receiver;
use tc_ssh::PrivateKey;
use tc_ssh::keygen::{self, KeyKind, LoadError};
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Rsa,
    Ecdsa,
    EdDsa,
}

pub struct KeygenWindow {
    pub open: bool,
    family: Family,
    rsa_bits: usize,
    ecdsa_bits: usize,
    key: Option<PrivateKey>,
    comment: String,
    passphrase: Zeroizing<String>,
    confirm: Zeroizing<String>,
    /// Save the private key as PuTTY `.ppk` (else OpenSSH).
    ppk: bool,
    busy: Option<Receiver<Result<PrivateKey, String>>>,
    /// A loaded key that needs its passphrase.
    locked: Option<(String, Zeroizing<String>)>,
    /// Saving without a passphrase: asked once.
    confirm_unprotected: bool,
    message: Option<String>,
}

impl Default for KeygenWindow {
    fn default() -> Self {
        Self {
            open: false,
            family: Family::EdDsa,
            rsa_bits: 3072,
            ecdsa_bits: 256,
            key: None,
            comment: String::new(),
            passphrase: Zeroizing::default(),
            confirm: Zeroizing::default(),
            ppk: true,
            busy: None,
            locked: None,
            confirm_unprotected: false,
            message: None,
        }
    }
}

fn default_comment(kind: &str) -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into());
    format!(
        "{kind}-key-{}-{user}",
        chrono::Local::now().format("%Y%m%d")
    )
}

/// Write a private key so only this user can read it.
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(path)?.write_all(text.as_bytes())
}

impl KeygenWindow {
    fn kind(&self) -> KeyKind {
        match self.family {
            Family::EdDsa => KeyKind::Ed25519,
            Family::Rsa => KeyKind::Rsa(self.rsa_bits),
            Family::Ecdsa => match self.ecdsa_bits {
                384 => KeyKind::EcdsaP384,
                521 => KeyKind::EcdsaP521,
                _ => KeyKind::EcdsaP256,
            },
        }
    }

    fn generate(&mut self, ctx: &egui::Context) {
        let kind = self.kind();
        let comment = default_comment(match self.family {
            Family::Rsa => "rsa",
            Family::Ecdsa => "ecdsa",
            Family::EdDsa => "eddsa",
        });
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(keygen::generate(kind, &comment));
            ctx.request_repaint();
        });
        self.busy = Some(rx);
        self.message = Some(format!("Generating a {} key…", kind.label()));
    }

    fn set_key(&mut self, key: PrivateKey) {
        self.comment = key.comment().to_string();
        self.key = Some(key);
        self.passphrase = Zeroizing::default();
        self.confirm = Zeroizing::default();
        self.message = None;
    }

    fn load_text(&mut self, text: String, passphrase: Option<&str>) {
        match keygen::load(&text, passphrase) {
            Ok(key) => {
                self.locked = None;
                self.set_key(key);
            }
            Err(LoadError::NeedsPassphrase) => {
                self.locked = Some((text, Zeroizing::default()));
                self.message = Some("This key is protected: enter its passphrase.".into());
            }
            Err(LoadError::Invalid(e)) => self.message = Some(format!("Cannot load the key: {e}")),
        }
    }

    fn save_private(&mut self) {
        let Some(key) = &self.key else { return };
        let mut key = key.clone();
        key.set_comment(self.comment.trim());
        let pass = (!self.passphrase.is_empty()).then(|| self.passphrase.as_str());
        let (ext, name) = if self.ppk {
            ("ppk", "PuTTY private key")
        } else {
            ("", "OpenSSH private key")
        };
        let mut dialog =
            rfd::FileDialog::new().set_file_name(if self.ppk { "key.ppk" } else { "id_key" });
        if self.ppk {
            dialog = dialog.add_filter(name, &[ext]);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        let text = if self.ppk {
            keygen::to_ppk(&key, pass)
        } else {
            keygen::to_openssh(&key, pass)
        };
        self.message = Some(match text.map(|t| write_private(&path, &t)) {
            Ok(Ok(())) => format!("Private key saved to {}", path.display()),
            Ok(Err(e)) => format!("Cannot write {}: {e}", path.display()),
            Err(e) => format!("Cannot save the key: {e}"),
        });
    }

    fn save_public(&mut self) {
        let Some(key) = &self.key else { return };
        let mut key = key.clone();
        key.set_comment(self.comment.trim());
        let Some(path) = rfd::FileDialog::new().set_file_name("key.pub").save_file() else {
            return;
        };
        let line = format!("{}\n", keygen::public_line(&key));
        self.message = Some(match std::fs::write(&path, line) {
            Ok(()) => format!("Public key saved to {}", path.display()),
            Err(e) => format!("Cannot write {}: {e}", path.display()),
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.busy
            && let Ok(result) = rx.try_recv()
        {
            self.busy = None;
            match result {
                Ok(key) => self.set_key(key),
                Err(e) => self.message = Some(format!("Generation failed: {e}")),
            }
        }
        let mut open = self.open;
        egui::Window::new("SSH key generator")
            .id(egui::Id::new("keygen"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(620.0)
            .show(ctx, |ui| self.contents(ui));
        self.open = open;
        if !self.open {
            // Do not keep a private key around once the window is closed.
            self.key = None;
            self.locked = None;
            self.passphrase = Zeroizing::default();
            self.confirm = Zeroizing::default();
        }
    }

    fn contents(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(600.0);
            ui.strong("Key");
            match &self.key {
                None => {
                    ui.label("No key.");
                }
                Some(key) => {
                    let mut with_comment = key.clone();
                    with_comment.set_comment(self.comment.trim());
                    ui.label("Public key for pasting into OpenSSH authorized_keys file:");
                    let mut public = keygen::public_line(&with_comment);
                    ui.add(
                        egui::TextEdit::multiline(&mut public)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    egui::Grid::new("keygen-key")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Key fingerprint:");
                            ui.monospace(keygen::fingerprint(key));
                            ui.end_row();
                            ui.label("Key comment:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.comment).desired_width(380.0),
                            );
                            ui.end_row();
                            ui.label("Key passphrase:");
                            ui.add(
                                egui::TextEdit::singleline(&mut *self.passphrase)
                                    .password(true)
                                    .desired_width(380.0),
                            );
                            ui.end_row();
                            ui.label("Confirm passphrase:");
                            ui.add(
                                egui::TextEdit::singleline(&mut *self.confirm)
                                    .password(true)
                                    .desired_width(380.0),
                            );
                            ui.end_row();
                        });
                }
            }
            if let Some((text, pass)) = &mut self.locked {
                let mut unlock = None;
                ui.horizontal(|ui| {
                    ui.label("Passphrase:");
                    let edit = ui.add(egui::TextEdit::singleline(&mut **pass).password(true));
                    if ui.button("Unlock").clicked()
                        || edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        unlock = Some((text.clone(), pass.to_string()));
                    }
                });
                if let Some((text, pass)) = unlock {
                    self.load_text(text, Some(&pass));
                }
            }
        });

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(600.0);
            ui.strong("Actions");
            let busy = self.busy.is_some();
            egui::Grid::new("keygen-actions")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Generate a public/private key pair");
                    if ui
                        .add_enabled(!busy, egui::Button::new("Generate"))
                        .clicked()
                    {
                        self.generate(&ctx);
                    }
                    ui.end_row();
                    ui.label("Load an existing private key file");
                    if ui.add_enabled(!busy, egui::Button::new("Load")).clicked()
                        && let Some(path) = rfd::FileDialog::new().pick_file()
                    {
                        match std::fs::read_to_string(&path) {
                            Ok(text) => self.load_text(text, None),
                            Err(e) => {
                                self.message = Some(format!("Cannot read {}: {e}", path.display()))
                            }
                        }
                    }
                    ui.end_row();
                    ui.label("Save the generated key");
                    ui.horizontal(|ui| {
                        let has_key = self.key.is_some();
                        if ui
                            .add_enabled(has_key, egui::Button::new("Save public key"))
                            .clicked()
                        {
                            self.save_public();
                        }
                        if ui
                            .add_enabled(has_key, egui::Button::new("Save private key"))
                            .clicked()
                        {
                            if *self.passphrase != *self.confirm {
                                self.message = Some("The two passphrases do not match.".into());
                            } else if self.passphrase.is_empty() && !self.confirm_unprotected {
                                self.confirm_unprotected = true;
                            } else {
                                self.confirm_unprotected = false;
                                self.save_private();
                            }
                        }
                        ui.radio_value(&mut self.ppk, true, "PuTTY (.ppk)");
                        ui.radio_value(&mut self.ppk, false, "OpenSSH");
                    });
                    ui.end_row();
                });
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Generating…");
                });
            }
            if self.confirm_unprotected {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Are you sure you want to save this key without a passphrase to protect it? \
                     Click \"Save private key\" again to confirm.",
                );
            }
            if let Some(m) = &self.message {
                ui.label(m);
            }
        });

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(600.0);
            ui.strong("Parameters");
            ui.horizontal(|ui| {
                ui.label("Type of key to generate:");
                ui.radio_value(&mut self.family, Family::Rsa, "RSA");
                ui.radio_value(&mut self.family, Family::Ecdsa, "ECDSA");
                ui.radio_value(&mut self.family, Family::EdDsa, "EdDSA (Ed25519)");
            });
            ui.horizontal(|ui| match self.family {
                Family::Rsa => {
                    ui.label("Number of bits in a generated key:");
                    for bits in [2048, 3072, 4096] {
                        ui.radio_value(&mut self.rsa_bits, bits, bits.to_string());
                    }
                }
                Family::Ecdsa => {
                    ui.label("Curve to use for generating this key:");
                    for (bits, name) in [(256, "nistp256"), (384, "nistp384"), (521, "nistp521")] {
                        ui.radio_value(&mut self.ecdsa_bits, bits, name);
                    }
                }
                Family::EdDsa => {
                    ui.weak("Ed25519 keys have a fixed size (255 bits).");
                }
            });
        });
    }
}
