//! "TermCraft passwords settings": saved passwords and named credentials,
//! where they are stored (OS keychain or encrypted configuration file) and
//! the Master Password. Secrets are shown only after the Master Password
//! is entered. Deleting and editing go through commands; the Master
//! Password and the storage choice never leave the UI.

use super::{Action, icon};
use std::collections::{BTreeMap, BTreeSet};
use tc_commands::Command;
use tc_core::settings::{MasterPrompt, PasswordSettings, SavePasswords};
use tc_vault::{Storage, Target, Vault, VaultError};
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    Passwords,
    Credentials,
}

/// Why the Master Password is being asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Startup with the configuration file storage.
    Unlock,
    ShowPasswords,
    Export,
    Storage(Storage),
}

struct MasterPromptDialog {
    purpose: Purpose,
    input: Zeroizing<String>,
    show: bool,
    error: Option<String>,
}

struct MasterDialog {
    current: Zeroizing<String>,
    new: Zeroizing<String>,
    retype: Zeroizing<String>,
    prompt: MasterPrompt,
    /// Continue with this once the Master Password is set.
    then: Option<Purpose>,
    error: Option<String>,
}

struct CredentialEditor {
    /// Name being edited; `None` for "New".
    original: Option<String>,
    name: String,
    username: String,
    password: Zeroizing<String>,
    error: Option<String>,
}

#[derive(Default)]
pub struct PasswordsUi {
    pub open: bool,
    page: Page,
    selection: BTreeSet<Target>,
    selected_credential: Option<String>,
    /// Secrets after "Show passwords" (cleared when the window closes).
    revealed: Option<BTreeMap<String, Zeroizing<String>>>,
    prompt: Option<MasterPromptDialog>,
    master: Option<MasterDialog>,
    editor: Option<CredentialEditor>,
    message: Option<String>,
}

/// What the window wants from the app.
#[derive(Default)]
pub struct Outcome {
    pub settings_changed: bool,
    pub status: Option<String>,
}

fn secret_key_of_target(t: &Target) -> String {
    t.key()
}

fn credential_secret_key(name: &str) -> String {
    format!("cred:{name}")
}

/// 0..=4 from length and character classes.
fn strength(pw: &str) -> usize {
    let classes = [
        pw.chars().any(|c| c.is_ascii_lowercase()),
        pw.chars().any(|c| c.is_ascii_uppercase()),
        pw.chars().any(|c| c.is_ascii_digit()),
        pw.chars().any(|c| !c.is_ascii_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let len = pw.chars().count();
    match (len, classes) {
        (0, _) => 0,
        (1..=7, _) => 1,
        (8..=11, 0..=2) => 2,
        (8..=11, _) | (12..=15, 0..=2) => 3,
        _ => 4,
    }
}

impl PasswordsUi {
    pub fn open_credentials(&mut self) {
        self.open = true;
        self.page = Page::Credentials;
    }

    /// "Master Password settings".
    pub fn open_master_settings(&mut self, prompt: MasterPrompt) {
        self.master = Some(MasterDialog {
            current: Zeroizing::default(),
            new: Zeroizing::default(),
            retype: Zeroizing::default(),
            prompt,
            then: None,
            error: None,
        });
    }

    /// Ask for the Master Password at startup (configuration file storage).
    pub fn ask_unlock(&mut self) {
        self.prompt = Some(MasterPromptDialog {
            purpose: Purpose::Unlock,
            input: Zeroizing::default(),
            show: false,
            error: None,
        });
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        vault: &Vault,
        settings: &mut PasswordSettings,
        out: &mut Vec<Action>,
    ) -> Outcome {
        let mut outcome = Outcome::default();
        if self.open {
            self.main_window(ctx, vault, settings, out, &mut outcome);
        }
        self.prompt_dialog(ctx, vault, settings, &mut outcome);
        self.master_dialog(ctx, vault, settings, &mut outcome);
        self.credential_editor(ctx, vault, out);
        if !self.open {
            self.revealed = None;
        }
        outcome
    }

    fn ask(&mut self, vault: &Vault, purpose: Purpose) {
        if !vault.has_master_password() {
            // Set one first, then carry on.
            self.master = Some(MasterDialog {
                current: Zeroizing::default(),
                new: Zeroizing::default(),
                retype: Zeroizing::default(),
                prompt: MasterPrompt::default(),
                then: Some(purpose),
                error: None,
            });
            return;
        }
        self.prompt = Some(MasterPromptDialog {
            purpose,
            input: Zeroizing::default(),
            show: false,
            error: None,
        });
    }

    fn main_window(
        &mut self,
        ctx: &egui::Context,
        vault: &Vault,
        settings: &mut PasswordSettings,
        out: &mut Vec<Action>,
        outcome: &mut Outcome,
    ) {
        let mut open = true;
        let before = settings.clone();
        egui::Window::new("TermCraft passwords settings")
            .id(egui::Id::new("passwords-settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(660.0)
            .show(ctx, |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Save sessions passwords");
                        ui.radio_value(&mut settings.save, SavePasswords::Always, "Always");
                        ui.radio_value(&mut settings.save, SavePasswords::Never, "Never");
                        ui.radio_value(&mut settings.save, SavePasswords::Ask, "Ask");
                        ui.add_space(24.0);
                        ui.checkbox(
                            &mut settings.save_key_passphrases,
                            "Save SSH keys passphrases as well",
                        )
                        .on_hover_text("Also save the passphrases of SSH private key files");
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.page, Page::Passwords, "Passwords");
                    ui.selectable_value(&mut self.page, Page::Credentials, "Credentials");
                });
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(640.0);
                    ui.set_min_height(180.0);
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| match self.page {
                            Page::Passwords => self.passwords_list(ui, vault),
                            Page::Credentials => self.credentials_list(ui, vault),
                        });
                });
                ui.horizontal(|ui| self.buttons(ui, vault, out));
                ui.add_space(8.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Choose where to save passwords");
                        let current = vault.storage();
                        let mut choice = current;
                        ui.radio_value(&mut choice, Storage::Keychain, "System keychain")
                            .on_hover_text(
                                "Windows Credential Manager, macOS Keychain or Secret Service",
                            );
                        ui.radio_value(&mut choice, Storage::File, "Configuration file")
                            .on_hover_text(
                                "Encrypted with your Master Password; travels with portable mode",
                            );
                        if choice != current {
                            self.ask(vault, Purpose::Storage(choice));
                        }
                    });
                });
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    let label = format!("{} Master Password settings", icon::LOCK_KEY);
                    if ui
                        .button(label)
                        .on_hover_text(
                            "A Master Password protects your stored passwords: it is needed \
                             to show or export them, and it encrypts them when they are \
                             saved in the configuration file.",
                        )
                        .clicked()
                    {
                        self.open_master_settings(settings.master_prompt);
                    }
                    if let Some(m) = &self.message {
                        ui.colored_label(ui.visuals().warn_fg_color, m);
                    }
                    if vault.is_locked() {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "The configuration file is locked: saved passwords are not used.",
                        );
                        if ui.button("Unlock").clicked() {
                            self.ask_unlock();
                        }
                    }
                    if ui.button("OK").clicked() {
                        self.open = false;
                    }
                });
            });
        if !open {
            self.open = false;
        }
        if *settings != before {
            outcome.settings_changed = true;
        }
    }

    fn shown(&self, key: &str) -> String {
        match &self.revealed {
            Some(map) => map.get(key).map(|s| s.to_string()).unwrap_or_default(),
            None => "••••••••".into(),
        }
    }

    fn passwords_list(&mut self, ui: &mut egui::Ui, vault: &Vault) {
        let saved = vault.list();
        egui::Grid::new("saved-passwords")
            .num_columns(4)
            .striped(true)
            .min_col_width(60.0)
            .show(ui, |ui| {
                ui.strong("Protocol");
                ui.strong("Username");
                ui.strong("Servername");
                ui.strong("Password");
                ui.end_row();
                for t in &saved {
                    let selected = self.selection.contains(t);
                    let protocol = if t.protocol == "sshkey" {
                        "SSH key"
                    } else {
                        t.protocol.as_str()
                    };
                    let row = ui.selectable_label(selected, protocol);
                    if row.clicked() {
                        if selected {
                            self.selection.remove(t);
                        } else {
                            self.selection.insert(t.clone());
                        }
                    }
                    ui.label(&t.username);
                    ui.label(&t.server);
                    let secret = self.shown(&secret_key_of_target(t));
                    ui.label(&secret);
                    ui.end_row();
                    let revealed = self.revealed.is_some();
                    row.context_menu(|ui| {
                        if ui.button("Copy username").clicked() {
                            ui.ctx().copy_text(t.username.clone());
                            ui.close();
                        }
                        if ui.button("Copy servername").clicked() {
                            ui.ctx().copy_text(t.server.clone());
                            ui.close();
                        }
                        let copy = ui
                            .add_enabled(revealed, egui::Button::new("Copy password"))
                            .on_disabled_hover_text("Click \"Show passwords\" first");
                        if copy.clicked() {
                            ui.ctx().copy_text(secret.clone());
                            ui.close();
                        }
                    });
                }
            });
        if saved.is_empty() {
            ui.weak("No saved passwords.");
        }
    }

    fn credentials_list(&mut self, ui: &mut egui::Ui, vault: &Vault) {
        let creds = vault.credentials();
        egui::Grid::new("credentials")
            .num_columns(3)
            .striped(true)
            .min_col_width(80.0)
            .show(ui, |ui| {
                ui.strong("Name");
                ui.strong("Username");
                ui.strong("Password");
                ui.end_row();
                for (name, user) in &creds {
                    let selected = self.selected_credential.as_deref() == Some(name.as_str());
                    let row = ui.selectable_label(selected, name);
                    if row.clicked() {
                        self.selected_credential = (!selected).then(|| name.clone());
                    }
                    if row.double_clicked() {
                        self.edit_credential(vault, name);
                    }
                    ui.label(user);
                    let secret = self.shown(&credential_secret_key(name));
                    ui.label(&secret);
                    ui.end_row();
                    let revealed = self.revealed.is_some();
                    row.context_menu(|ui| {
                        if ui.button("Copy username").clicked() {
                            ui.ctx().copy_text(user.clone());
                            ui.close();
                        }
                        let copy = ui
                            .add_enabled(revealed, egui::Button::new("Copy password"))
                            .on_disabled_hover_text("Click \"Show passwords\" first");
                        if copy.clicked() {
                            ui.ctx().copy_text(secret.clone());
                            ui.close();
                        }
                    });
                }
            });
        if creds.is_empty() {
            ui.weak("No credentials. Sessions can use one as [name] in their username field.");
        }
    }

    fn edit_credential(&mut self, vault: &Vault, name: &str) {
        let (username, password) = match vault.credential(name) {
            Ok(Some((u, p))) => (u, p),
            Ok(None) => return,
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        self.editor = Some(CredentialEditor {
            original: Some(name.to_string()),
            name: name.to_string(),
            username,
            password: Zeroizing::new(password),
            error: None,
        });
    }

    fn buttons(&mut self, ui: &mut egui::Ui, vault: &Vault, out: &mut Vec<Action>) {
        let shown = self.revealed.is_some();
        let label = if shown {
            "Hide passwords"
        } else {
            "Show passwords"
        };
        if ui.button(format!("{} {label}", icon::EYE)).clicked() {
            if shown {
                self.revealed = None;
            } else {
                self.ask(vault, Purpose::ShowPasswords);
            }
        }
        match self.page {
            Page::Passwords => {
                let any = !self.selection.is_empty();
                if ui
                    .add_enabled(any, egui::Button::new("Delete selected"))
                    .clicked()
                {
                    for target in std::mem::take(&mut self.selection) {
                        out.push(Action::Run(Command::PasswordForget { target }));
                    }
                }
                if ui
                    .add_enabled(!vault.list().is_empty(), egui::Button::new("Delete all"))
                    .clicked()
                {
                    self.selection.clear();
                    out.push(Action::Run(Command::PasswordForgetAll {}));
                }
            }
            Page::Credentials => {
                let selected = self.selected_credential.clone();
                if ui
                    .add_enabled(selected.is_some(), egui::Button::new("Delete selected"))
                    .clicked()
                    && let Some(name) = selected.clone()
                {
                    self.selected_credential = None;
                    out.push(Action::Run(Command::CredentialRemove { name }));
                }
                if ui
                    .add_enabled(
                        !vault.credentials().is_empty(),
                        egui::Button::new("Delete all"),
                    )
                    .clicked()
                {
                    self.selected_credential = None;
                    for (name, _) in vault.credentials() {
                        out.push(Action::Run(Command::CredentialRemove { name }));
                    }
                }
            }
        }
        if ui.button("Export to file").clicked() {
            self.ask(vault, Purpose::Export);
        }
        if self.page == Page::Credentials {
            if ui.button("New").clicked() {
                self.editor = Some(CredentialEditor {
                    original: None,
                    name: String::new(),
                    username: String::new(),
                    password: Zeroizing::default(),
                    error: None,
                });
            }
            if ui
                .add_enabled(
                    self.selected_credential.is_some(),
                    egui::Button::new("Edit"),
                )
                .clicked()
                && let Some(name) = self.selected_credential.clone()
            {
                self.edit_credential(vault, &name);
            }
        }
    }

    /// Run `purpose` once the Master Password is known to be right.
    fn proceed(
        &mut self,
        vault: &Vault,
        settings: &PasswordSettings,
        purpose: Purpose,
        outcome: &mut Outcome,
    ) {
        match purpose {
            Purpose::Unlock => {
                outcome.status = Some("Password vault unlocked".into());
            }
            Purpose::ShowPasswords => match read_all(vault) {
                Ok(map) => self.revealed = Some(map),
                Err(e) => self.message = Some(e.to_string()),
            },
            Purpose::Export => self.export(vault),
            Purpose::Storage(to) => match vault.set_storage(to) {
                Ok(()) => {
                    if to == Storage::File {
                        let _ =
                            vault.remember_key(settings.master_prompt == MasterPrompt::NewComputer);
                    }
                    self.message = None;
                    outcome.status = Some(match to {
                        Storage::File => "Passwords moved to the configuration file".into(),
                        Storage::Keychain => "Passwords moved to the system keychain".into(),
                    });
                }
                Err(e) => self.message = Some(e.to_string()),
            },
        }
    }

    fn export(&mut self, vault: &Vault) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("termcraft-passwords.csv")
            .add_filter("CSV", &["csv"])
            .save_file()
        else {
            return;
        };
        let result = read_all(vault).and_then(|secrets| {
            let mut csv = Zeroizing::new(String::from("Protocol,Username,Servername,Password\n"));
            let get = |k: &str| secrets.get(k).map(|s| s.to_string()).unwrap_or_default();
            for t in vault.list() {
                csv.push_str(&csv_line(&[
                    &t.protocol,
                    &t.username,
                    &t.server,
                    &get(&secret_key_of_target(&t)),
                ]));
            }
            for (name, user) in vault.credentials() {
                csv.push_str(&csv_line(&[
                    "credential",
                    &user,
                    &name,
                    &get(&credential_secret_key(&name)),
                ]));
            }
            std::fs::write(&path, csv.as_bytes()).map_err(|e| VaultError::Index {
                path: path.display().to_string(),
                message: e.to_string(),
            })
        });
        self.message = Some(match result {
            Ok(()) => format!("Exported to {} (keep this file safe)", path.display()),
            Err(e) => format!("export failed: {e}"),
        });
    }

    fn prompt_dialog(
        &mut self,
        ctx: &egui::Context,
        vault: &Vault,
        settings: &PasswordSettings,
        outcome: &mut Outcome,
    ) {
        let Some(p) = &mut self.prompt else { return };
        let mut answer: Option<bool> = None;
        egui::Window::new("TermCraft")
            .id(egui::Id::new("master-prompt"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icon::LOCK_KEY).size(48.0));
                    ui.vertical(|ui| {
                        ui.label(match p.purpose {
                            Purpose::Unlock => {
                                "Please enter your Master password\nto unlock your stored passwords"
                            }
                            _ => "Please enter your Master password\nto confirm your identity",
                        });
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut *p.input)
                                .password(!p.show)
                                .desired_width(260.0),
                        );
                        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            answer = Some(true);
                        }
                        edit.request_focus();
                        ui.checkbox(&mut p.show, "Show password");
                        if let Some(e) = &p.error {
                            ui.colored_label(ui.visuals().error_fg_color, e);
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.add_space(60.0);
                    if ui.button("OK").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                    ui.add_space(20.0);
                    ui.label(icon::QUESTION).on_hover_text(
                        "TermCraft needs to confirm your identity before proceeding. \
                         Enter your TermCraft Master Password to continue.",
                    );
                });
            });
        let Some(ok) = answer else { return };
        if !ok {
            self.prompt = None;
            return;
        }
        let purpose = p.purpose;
        let remember = settings.master_prompt == MasterPrompt::NewComputer;
        let checked = if vault.is_locked() {
            vault.unlock(&p.input, remember).map(|()| true)
        } else {
            vault.verify_master(&p.input)
        };
        match checked {
            Ok(true) => {
                self.prompt = None;
                self.proceed(vault, settings, purpose, outcome);
            }
            Ok(false) | Err(VaultError::WrongPassword) => {
                p.error = Some("Wrong Master Password".into());
                p.input = Zeroizing::default();
            }
            Err(e) => p.error = Some(e.to_string()),
        }
    }

    fn master_dialog(
        &mut self,
        ctx: &egui::Context,
        vault: &Vault,
        settings: &mut PasswordSettings,
        outcome: &mut Outcome,
    ) {
        let Some(m) = &mut self.master else { return };
        let exists = vault.has_master_password();
        let mut answer: Option<bool> = None;
        egui::Window::new("TermCraft Master Password")
            .id(egui::Id::new("master-password"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icon::SHIELD_CHECK).size(64.0));
                    ui.vertical(|ui| {
                        ui.heading("Secure my stored passwords");
                        ui.label(
                            "Choose a \"Master Password\" to protect your password vault. \
                             It is needed to show or export your passwords, and it encrypts \
                             them when they are saved in the configuration file.",
                        );
                        ui.label(
                            egui::RichText::new(
                                "Do not forget it: a lost Master Password cannot be recovered, \
                                 and passwords encrypted with it are lost too.",
                            )
                            .strong(),
                        );
                        ui.add_space(8.0);
                        egui::Grid::new("master-fields")
                            .num_columns(2)
                            .spacing([8.0, 6.0])
                            .show(ui, |ui| {
                                if exists {
                                    ui.label("Current master password:");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut *m.current)
                                            .password(true)
                                            .desired_width(200.0),
                                    );
                                    ui.end_row();
                                }
                                ui.label("My master password:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut *m.new)
                                        .password(true)
                                        .desired_width(200.0),
                                );
                                ui.end_row();
                                ui.label("Re-type my master password:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut *m.retype)
                                        .password(true)
                                        .desired_width(200.0),
                                );
                                ui.end_row();
                                ui.label("Master password strength:");
                                strength_bar(ui, strength(&m.new));
                                ui.end_row();
                            });
                        ui.add_space(8.0);
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.label("Prompt me for my master password");
                            ui.radio_value(
                                &mut m.prompt,
                                MasterPrompt::NewComputer,
                                "only on new user account or new computer",
                            );
                            ui.radio_value(
                                &mut m.prompt,
                                MasterPrompt::EveryStartup,
                                "at every TermCraft startup",
                            );
                            ui.add_enabled(
                                false,
                                egui::RadioButton::new(
                                    false,
                                    "at every TermCraft startup and after unlocking my user session",
                                ),
                            )
                            .on_disabled_hover_text(super::NOT_YET);
                        });
                        if let Some(e) = &m.error {
                            ui.colored_label(ui.visuals().error_fg_color, e);
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.add_space(160.0);
                    if ui.button("OK").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
        let Some(ok) = answer else { return };
        if !ok {
            self.master = None;
            return;
        }
        let password_changed = !m.new.is_empty() || !exists;
        if password_changed {
            if m.new.is_empty() {
                m.error = Some("Please enter a master password".into());
                return;
            }
            if *m.new != *m.retype {
                m.error = Some("The two passwords do not match".into());
                return;
            }
            let current = (exists && !m.current.is_empty()).then(|| m.current.as_str());
            if exists && current.is_none() {
                m.error = Some("Enter your current master password".into());
                return;
            }
            if let Err(e) = vault.set_master_password(current, &m.new) {
                m.error = Some(e.to_string());
                return;
            }
        } else if !vault.verify_master(&m.current).unwrap_or(false) {
            m.error = Some("Wrong Master Password".into());
            return;
        }
        if settings.master_prompt != m.prompt {
            settings.master_prompt = m.prompt;
            outcome.settings_changed = true;
        }
        let _ = vault.remember_key(
            settings.master_prompt == MasterPrompt::NewComputer && vault.storage() == Storage::File,
        );
        let then = m.then;
        self.master = None;
        outcome.status = Some("Master Password saved".into());
        if let Some(purpose) = then {
            self.proceed(vault, settings, purpose, outcome);
        }
    }

    fn credential_editor(&mut self, ctx: &egui::Context, vault: &Vault, out: &mut Vec<Action>) {
        let Some(e) = &mut self.editor else { return };
        let mut answer: Option<bool> = None;
        let title = if e.original.is_some() {
            "Edit credential"
        } else {
            "New credential"
        };
        egui::Window::new(title)
            .id(egui::Id::new("credential-editor"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("credential-fields")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut e.name);
                        ui.end_row();
                        ui.label("Username:");
                        ui.text_edit_singleline(&mut e.username);
                        ui.end_row();
                        ui.label("Password:");
                        ui.add(egui::TextEdit::singleline(&mut *e.password).password(true));
                        ui.end_row();
                    });
                if let Some(err) = &e.error {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
        let Some(ok) = answer else { return };
        if !ok {
            self.editor = None;
            return;
        }
        let name = e.name.trim().to_string();
        if name.is_empty() || name.contains(['[', ']']) {
            e.error = Some("Enter a name (without [ or ])".into());
            return;
        }
        if e.original.as_deref() != Some(name.as_str())
            && vault.credentials().iter().any(|(n, _)| *n == name)
        {
            e.error = Some(format!("A credential named \"{name}\" already exists"));
            return;
        }
        out.push(Action::Run(Command::CredentialSet {
            name: name.clone(),
            username: e.username.trim().to_string(),
            password: e.password.to_string(),
        }));
        if let Some(old) = e.original.take()
            && old != name
        {
            out.push(Action::Run(Command::CredentialRemove { name: old }));
        }
        self.selected_credential = Some(name);
        self.editor = None;
        self.revealed = None;
    }
}

fn read_all(vault: &Vault) -> Result<BTreeMap<String, Zeroizing<String>>, VaultError> {
    let mut map = BTreeMap::new();
    for t in vault.list() {
        if let Some(s) = vault.get(&t)? {
            map.insert(secret_key_of_target(&t), Zeroizing::new(s));
        }
    }
    for (name, _) in vault.credentials() {
        if let Some((_, p)) = vault.credential(&name)? {
            map.insert(credential_secret_key(&name), Zeroizing::new(p));
        }
    }
    Ok(map)
}

fn csv_line(fields: &[&str]) -> String {
    let quoted: Vec<String> = fields
        .iter()
        .map(|f| {
            if f.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", f.replace('"', "\"\""))
            } else {
                f.to_string()
            }
        })
        .collect();
    format!("{}\n", quoted.join(","))
}

fn strength_bar(ui: &mut egui::Ui, level: usize) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(200.0, 8.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let colors = [
        egui::Color32::TRANSPARENT,
        egui::Color32::from_rgb(0xd9, 0x3f, 0x3f),
        egui::Color32::from_rgb(0xe8, 0x8f, 0x2a),
        egui::Color32::from_rgb(0xc9, 0xc0, 0x2c),
        egui::Color32::from_rgb(0x3f, 0xa3, 0x4d),
    ];
    let mut filled = rect;
    filled.set_width(rect.width() * level as f32 / 4.0);
    painter.rect_filled(filled, 2.0, colors[level.min(4)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_levels() {
        assert_eq!(strength(""), 0);
        assert_eq!(strength("abc"), 1);
        assert_eq!(strength("abcdefgh"), 2);
        assert_eq!(strength("abcdEF12"), 3);
        assert_eq!(strength("correct-Horse-battery-9"), 4);
    }

    #[test]
    fn csv_quoting() {
        assert_eq!(csv_line(&["a", "b,c", "d\"e"]), "a,\"b,c\",\"d\"\"e\"\n");
    }
}
