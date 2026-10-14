//! Modal dialogs and prompts of the main window.

use super::*;

impl TermCraftApp {
    /// "Connect as...": open a copy of the session as another user.
    pub(super) fn connect_as_dialog(&mut self, ctx: &egui::Context) {
        let Some((session, user)) = &mut self.connect_as else {
            return;
        };
        let mut answer = None;
        egui::Window::new("Connect as...")
            .id(egui::Id::new("connect-as"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Connect to \"{}\" as:", session.name));
                let r = ui.add(egui::TextEdit::singleline(user).hint_text("user name"));
                r.request_focus();
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    answer = Some(true);
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
        match answer {
            Some(true) => {
                let (session, user) = self.connect_as.take().expect("open");
                if let Some(kind) = session.kind.with_user(&user) {
                    let copy = tc_core::Session { kind, ..*session };
                    if let Err(e) = self.core.tab_open(Some(copy)) {
                        self.status = e.to_string();
                    }
                }
            }
            Some(false) => self.connect_as = None,
            None => {}
        }
    }

    /// Reference `TFORMDESKTOPSHORTCUT`: options, then the shortcut is
    /// created on the desktop.
    pub(super) fn shortcut_options_dialog(&mut self, ctx: &egui::Context) {
        let Some((name, opts)) = &mut self.shortcut_dialog else {
            return;
        };
        let mut answer = None;
        egui::Window::new("Shortcut options")
            .id(egui::Id::new("shortcut-options"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Desktop shortcut to \"{name}\""));
                ui.add_space(6.0);
                ui.checkbox(&mut opts.hide_terminal, "Hide terminal on startup")
                    .on_hover_text("TermCraft starts minimized");
                ui.checkbox(&mut opts.close_on_exit, "Close TermCraft on exit")
                    .on_hover_text("TermCraft quits when this session ends");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
        match answer {
            Some(true) => {
                let (name, opts) = self.shortcut_dialog.take().expect("open");
                self.run(Command::SessionDesktopShortcut {
                    session: name,
                    hide_terminal: opts.hide_terminal,
                    close_on_exit: opts.close_on_exit,
                    folder: None,
                });
            }
            Some(false) => self.shortcut_dialog = None,
            None => {}
        }
    }

    pub(super) fn dialogs(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) {
        if self.show_config {
            let mut open = true;
            let changed = crate::ui::config_window::show(
                ctx,
                &mut open,
                &mut self.config_tab,
                &mut self.core.settings,
                &self.core.paths,
                out,
            );
            self.show_config = open;
            if changed {
                self.apply_remote_monitoring();
                if let Err(e) = self.core.save_settings() {
                    self.status = format!("could not save settings: {e}");
                }
            }
        }
        if self.confirm_exit {
            let n = self.live_sessions();
            let mut answer = None;
            egui::Window::new("Exit TermCraft")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{n} session(s) or tunnel(s) are still open. Do you want to exit TermCraft?"
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Exit").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            if let Some(exit) = answer {
                self.confirm_exit = false;
                if exit {
                    self.exit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if self.show_tunnels {
            let mut open = true;
            crate::ui::tunnels_window::manager(
                ctx,
                &mut open,
                &self.core.tunnels.tunnels,
                &self.core.running,
                &mut self.tunnel_editor,
                out,
            );
            self.show_tunnels = open;
        }
        if let Some(editor) = &mut self.tunnel_editor
            && !editor.show(ctx, out)
        {
            self.tunnel_editor = None;
        }
        crate::ui::tunnels_window::prompts(ctx, &mut self.core.running);
        self.save_password_dialog(ctx);
        self.paste_confirm_dialog(ctx);
        self.upload_edited_dialog(ctx);
        self.keygen.show(ctx);
        self.shortcut_options_dialog(ctx);
        // Remote files opened with the text editor; too big or binary ones
        // go to the system editor instead.
        let opens: Vec<(std::path::PathBuf, String)> = self
            .core
            .tabs
            .iter_mut()
            .flat_map(|t| std::mem::take(&mut t.open_in_editor))
            .collect();
        for (local, remote) in opens {
            if let Err(e) = self.text_editor.open_file(&local, Some(&remote))
                && let Err(e2) = crate::external::open_in_text_editor(&local)
            {
                self.status = format!("{e}; {e2}");
            }
        }
        let compares: Vec<std::path::PathBuf> = self
            .core
            .tabs
            .iter_mut()
            .flat_map(|t| std::mem::take(&mut t.open_in_diff))
            .collect();
        if let Some(file) = compares.last() {
            self.diff_tool.compare_with(file);
        }
        let images: Vec<(std::path::PathBuf, String)> = self
            .core
            .tabs
            .iter_mut()
            .flat_map(|t| std::mem::take(&mut t.open_in_viewer))
            .collect();
        if let Some((local, remote)) = images.last() {
            let title = remote.rsplit('/').next().unwrap_or(remote);
            self.image_viewer.open(ctx, local, title);
        }
        self.image_viewer.show(ctx);
        self.connect_as_dialog(ctx);
        self.shared.poll();
        if let Some(window) = &mut self.shared_window {
            use crate::ui::shared_window::Outcome;
            match window.show(ctx) {
                Outcome::Pending => {}
                Outcome::Cancel => self.shared_window = None,
                Outcome::Apply(entries) => {
                    self.core.settings.shared_sessions = entries;
                    self.shared_window = None;
                    if let Err(e) = self.core.save_settings() {
                        self.status = format!("could not save settings: {e}");
                    }
                    self.shared.refresh(
                        &self.core.settings.shared_sessions,
                        self.core.repaint.clone(),
                    );
                }
            }
        }
        // Popup terminal: the hotkey works from any program (not in
        // snapshot runs, which must not grab keys).
        if self.snapshot.is_none() {
            let hotkey = tc_core::hotkeys::effective(&self.core.settings.hotkeys)
                .into_iter()
                .find(|(a, _)| *a == tc_core::hotkeys::HotkeyAction::PopupTerminal)
                .and_then(|(_, hk)| hk);
            self.popup.sync_hotkey(hotkey);
            let core = &self.core;
            self.popup.handle(&core.settings.popup, || core.tab_env());
        }
        self.popup
            .show(ctx, &self.core.settings.popup, &self.core.settings.terminal);
        self.text_editor.show(ctx);
        self.diff_tool.show(ctx);
        self.ascii_table.show(ctx);
        for note in self.core.servers.pump(&self.core.settings.servers) {
            self.status = note;
        }
        if self.show_servers
            && crate::ui::servers_window::show(
                ctx,
                &mut self.show_servers,
                &mut self.servers_page,
                &mut self.core.settings.servers,
                &self.core.servers,
                out,
            )
            && let Err(e) = self.core.save_settings()
        {
            self.status = format!("could not save settings: {e}");
        }
        if self.core.servers.running_count() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if self.show_expert
            && crate::ui::expert_window::show(
                ctx,
                &mut self.show_expert,
                &mut self.core.settings.terminal,
            )
            && let Err(e) = self.core.save_settings()
        {
            self.status = format!("could not save settings: {e}");
        }
        if let Some(window) = &mut self.syntax_window {
            use crate::ui::syntax_window::Outcome;
            match window.show(ctx) {
                Outcome::Pending => {}
                Outcome::Cancel => self.syntax_window = None,
                Outcome::Done(slots) => {
                    self.core.settings.terminal.custom_syntaxes = *slots;
                    self.syntax_window = None;
                    if let Err(e) = self.core.save_settings() {
                        self.status = format!("could not save settings: {e}");
                    }
                }
            }
        }
        let known_hosts_file = self.core.paths.known_hosts_file();
        self.known_hosts.show(ctx, &known_hosts_file, out);
        if self.show_hotkeys
            && crate::ui::hotkeys_window::show(
                ctx,
                &mut self.show_hotkeys,
                &mut self.core.settings.hotkeys,
            )
            && let Err(e) = self.core.save_settings()
        {
            self.status = format!("could not save settings: {e}");
        }
        self.passwords_window(ctx, out);
        self.host_key_dialog(ctx);
        self.certificate_dialog(ctx);
        if let Some(editor) = &mut self.editor {
            editor.credentials = self
                .core
                .vault
                .credentials()
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            editor.syntaxes =
                crate::ui::syntax_window::choices(&self.core.settings.terminal.custom_syntaxes);
            if !editor.show(ctx, out) {
                self.editor = None;
            }
        }

        if self.show_import {
            let mut open = true;
            egui::Window::new("Import MobaXterm sessions")
                .open(&mut open)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Path to an exported .mxtsessions file or MobaXterm.ini:");
                    ui.text_edit_singleline(&mut self.import_path);
                    if ui.button("Import").clicked() && !self.import_path.trim().is_empty() {
                        out.push(Action::Run(Command::SessionImportMobaxterm {
                            path: self.import_path.trim().trim_matches('"').to_string(),
                        }));
                        self.show_import = false;
                    }
                });
            self.show_import &= open;
        }

        if self.show_about {
            let (portable, dir) = (self.core.paths.portable, self.core.paths.config_dir.clone());
            egui::Window::new("About TermCraft")
                .open(&mut self.show_about)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading(format!("TermCraft {}", env!("CARGO_PKG_VERSION")));
                    ui.label("All-in-one terminal and remote-access toolbox, in pure Rust.");
                    ui.label("Licensed under MIT OR Apache-2.0. Icons: Phosphor (MIT).");
                    if portable {
                        ui.label("Running in portable mode.");
                    }
                    ui.weak(format!("Data: {}", dir.display()));
                });
        }

        let mut finish_rename = None;
        if let Some((target, text)) = &mut self.rename {
            let title = match target {
                Rename::Tab(_) => "Rename tab",
                Rename::Session(_) => "Rename session",
                Rename::Macro(_) => "Rename macro",
                Rename::NewMacro(_) => "Save recorded macro",
                Rename::NewFolder(_) => "New folder",
                Rename::Folder(_) => "Rename folder",
            };
            let mut open = true;
            egui::Window::new(title)
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    let edit = ui.text_edit_singleline(text);
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if !edit.has_focus() && !enter {
                        edit.request_focus();
                    }
                    if ui.button("OK").clicked() || enter {
                        finish_rename = Some(true);
                    }
                });
            if !open {
                finish_rename = Some(false);
            }
        }
        if let Some(ok) = finish_rename
            && let Some((target, text)) = self.rename.take()
            && ok
        {
            out.push(Action::Run(match target {
                Rename::Tab(id) => Command::TabRename {
                    tab: Some(id),
                    title: text,
                },
                Rename::Session(id) => Command::SessionRename { id, name: text },
                Rename::Macro(id) => Command::MacroRename { id, name: text },
                Rename::NewMacro(keys) => Command::MacroAdd {
                    r#macro: tc_core::Macro::new(text, keys),
                },
                Rename::NewFolder(parent) => Command::SessionFolderAdd {
                    path: if parent.is_empty() {
                        text
                    } else {
                        format!("{parent}/{text}")
                    },
                },
                Rename::Folder(from) => Command::SessionFolderRename { from, to: text },
            }));
        }

        if let Some(path) = self.confirm_delete_folder.clone() {
            let inside = self
                .core
                .store
                .sessions
                .iter()
                .filter(|s| s.folder == path || s.folder.starts_with(&format!("{path}/")))
                .count();
            let mut decided = None;
            egui::Window::new("Delete folder")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Delete folder \"{path}\"?"));
                    if inside > 0 {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("The {inside} session(s) inside it are deleted too."),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            decided = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decided = Some(false);
                        }
                    });
                });
            if let Some(yes) = decided {
                self.confirm_delete_folder = None;
                if yes {
                    out.push(Action::Run(Command::SessionFolderRemove { path }));
                }
            }
        }

        if let Some(id) = self.confirm_delete {
            let name = self
                .core
                .store
                .get(id)
                .map_or_else(String::new, |s| s.name.clone());
            let mut decided = None;
            egui::Window::new("Delete session")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Delete session \"{name}\"?"));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            decided = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decided = Some(false);
                        }
                    });
                });
            if let Some(yes) = decided {
                self.confirm_delete = None;
                if yes {
                    out.push(Action::Run(Command::SessionRemove { id }));
                }
            }
        }
    }

    /// Reference `TFORMSTOREPASSWORDS`: "Do you want to save this password?"
    pub(super) fn save_password_dialog(&mut self, ctx: &egui::Context) {
        let Some((target, _)) = self.offers.front() else {
            return;
        };
        let mut answer = None;
        egui::Window::new("Save password")
            .id(egui::Id::new("save-password"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(ui::icon::KEY).size(36.0));
                    ui.vertical(|ui| {
                        ui.strong("Do you want to save this password?");
                        ui.label(format!("{}@{}", target.username, target.server));
                    });
                });
                ui.weak(match self.core.vault.storage() {
                    tc_vault::Storage::Keychain => {
                        "TermCraft stores passwords in your system keychain \
                         (Windows Credential Manager, macOS Keychain or Secret Service)."
                    }
                    tc_vault::Storage::File => {
                        "TermCraft will use your \"Master Password\" to secure \
                         all your stored passwords with strong encryption."
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("No").clicked() {
                        answer = Some(false);
                    }
                });
                ui.checkbox(&mut self.dont_ask_again, "Do not show this message again");
            });
        let Some(yes) = answer else { return };
        let (target, password) = self.offers.pop_front().expect("checked above");
        if yes && let Err(e) = self.core.vault.save(&target, &password) {
            self.status = format!("could not save password: {e}");
        }
        if self.dont_ask_again {
            self.core.settings.passwords.save = if yes {
                tc_core::settings::SavePasswords::Always
            } else {
                tc_core::settings::SavePasswords::Never
            };
            if let Err(e) = self.core.save_settings() {
                self.status = format!("could not save settings: {e}");
            }
        }
    }

    /// An edited remote file changed locally: upload it?
    pub(super) fn upload_edited_dialog(&mut self, ctx: &egui::Context) {
        let Some(tab) = self
            .core
            .tabs
            .iter_mut()
            .find(|t| !t.upload_prompts.is_empty())
        else {
            return;
        };
        let changed = tab.upload_prompts.front().cloned().expect("checked");
        let mut answer = None;
        egui::Window::new("TermCraft")
            .id(egui::Id::new(("upload-edited", tab.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(ui::icon::UPLOAD_SIMPLE).size(36.0));
                    ui.vertical(|ui| {
                        ui.strong("The file has been modified.");
                        ui.monospace(&changed.remote);
                        ui.label("Do you want to upload it to the server?");
                    });
                });
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("No").clicked() {
                        answer = Some(false);
                    }
                });
            });
        let Some(yes) = answer else { return };
        tab.upload_prompts.pop_front();
        if yes {
            tab.upload_edited(&changed);
        }
    }

    /// "You are about to paste several lines": confirm before sending.
    pub(super) fn paste_confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(tab) = self
            .core
            .tabs
            .iter_mut()
            .find(|t| t.pending_paste.is_some())
        else {
            return;
        };
        let text = tab.pending_paste.clone().unwrap_or_default();
        let lines = text.trim_end().lines().count();
        let mut answer = None;
        let mut dont_ask = !self.core.settings.terminal.warn_multiline_paste;
        egui::Window::new("TermCraft")
            .id(egui::Id::new(("paste-confirm", tab.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(ui::icon::CLIPBOARD_TEXT).size(36.0));
                    ui.vertical(|ui| {
                        ui.strong(format!("You are about to paste {lines} lines."));
                        ui.label("Each line may run as a command. Do you want to continue?");
                    });
                });
                let preview: String = text.lines().take(6).collect::<Vec<_>>().join("\n");
                ui.add(
                    egui::Label::new(egui::RichText::new(preview).monospace().small()).truncate(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Paste").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
                ui.checkbox(&mut dont_ask, "Do not show this message again");
            });
        let Some(paste) = answer else { return };
        if let Some(text) = tab.pending_paste.take()
            && paste
        {
            tab.paste(&text);
        }
        if dont_ask == self.core.settings.terminal.warn_multiline_paste {
            self.core.settings.terminal.warn_multiline_paste = !dont_ask;
            if let Err(e) = self.core.save_settings() {
                self.status = format!("could not save settings: {e}");
            }
        }
    }

    /// Reference `TFORMMANAGEPASSWORDS` and the Master Password dialogs.
    pub(super) fn passwords_window(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) {
        let outcome = self.passwords.show(
            ctx,
            &self.core.vault,
            &mut self.core.settings.passwords,
            out,
        );
        if let Some(status) = outcome.status {
            self.status = status;
        }
        if outcome.settings_changed
            && let Err(e) = self.core.save_settings()
        {
            self.status = format!("could not save settings: {e}");
        }
    }

    /// "Host key validation" for the active tab's pending SSH connection.
    /// FTPS: the server certificate did not verify. Like the host key
    /// dialog: Accept (remember it) / Connect once / Cancel.
    pub(super) fn certificate_dialog(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.core.tabs.iter_mut().find(|t| t.cert_prompt.is_some()) else {
            return;
        };
        let prompt = tab.cert_prompt.as_ref().expect("checked");
        let mut answer = None;
        egui::Window::new("TermCraft Security Alert")
            .id(egui::Id::new(("ftps-cert", tab.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(560.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(ui::icon::SHIELD_WARNING)
                            .size(44.0)
                            .color(ui.visuals().warn_fg_color),
                    );
                    ui.vertical(|ui| {
                        ui.label("The certificate of this FTPS server could not be verified:");
                        ui.monospace(format!("    {}", prompt.reason));
                        ui.label("Its fingerprint is:");
                        ui.monospace(format!("    {}", prompt.fingerprint));
                        ui.label(
                            "Compare it with the one the server administrator gave you. Press \
                             \"Accept\" to trust this certificate from now on, \"Connect Once\" \
                             to trust it for this connection only, or \"Cancel\".",
                        );
                    });
                });
                ui.horizontal(|ui| {
                    if ui.button("Accept").clicked() {
                        answer = Some((true, true));
                    }
                    if ui.button("Connect Once").clicked() {
                        answer = Some((true, false));
                    }
                    let cancel = ui.button("Cancel");
                    cancel.request_focus();
                    if cancel.clicked() {
                        answer = Some((false, false));
                    }
                });
            });
        if let Some((accept, remember)) = answer {
            tab.answer_certificate(accept, remember);
        }
    }

    /// Host key validation for the active tab's pending SSH connection.
    pub(super) fn host_key_dialog(&mut self, ctx: &egui::Context) {
        let active = self.core.active;
        let Some(tab) = self.core.tabs.iter_mut().find(|t| Some(t.id) == active) else {
            return;
        };
        let Some(info) = &tab.host_key else {
            return;
        };
        let id = egui::Id::new(("host-key", tab.id));
        if let Some(a) = ui::host_key_dialog::show(ctx, id, info) {
            tab.answer_host_key(a.accept, a.remember);
        }
    }
}
