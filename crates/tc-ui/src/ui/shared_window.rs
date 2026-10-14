//! "Shared sessions management" (reference `TFORMSHAREDSESSIONS`): the
//! list of shared sessions locations (folder name, URL) with Add / Edit /
//! Delete / Move, and the edit page (tree folder name, backend, location).

use super::{NOT_YET, icon};
use tc_core::settings::{SharedBackend, SharedSessionsEntry};

pub struct SharedWindow {
    entries: Vec<SharedSessionsEntry>,
    selected: Option<usize>,
    /// The entry being edited (index, `None` = a new one).
    editing: Option<(Option<usize>, SharedSessionsEntry)>,
}

pub enum Outcome {
    Pending,
    Cancel,
    Apply(Vec<SharedSessionsEntry>),
}

impl SharedWindow {
    pub fn new(entries: &[SharedSessionsEntry]) -> Self {
        Self {
            entries: entries.to_vec(),
            selected: None,
            editing: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Pending;
        let mut open = true;
        egui::Window::new("Shared sessions management")
            .id(egui::Id::new("shared-sessions"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.heading("Shared sessions locations");
                ui.add_space(6.0);
                if self.editing.is_some() {
                    self.edit_page(ui);
                } else {
                    self.list_page(ui);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        outcome = Outcome::Apply(self.entries.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    fn list_page(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_size(egui::vec2(385.0, 220.0));
            egui::Grid::new("shared-list")
                .num_columns(2)
                .striped(true)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    ui.strong("Folder name");
                    ui.strong("URL");
                    ui.end_row();
                    for (i, e) in self.entries.iter().enumerate() {
                        let r = ui.selectable_label(self.selected == Some(i), &e.name);
                        if r.clicked() {
                            self.selected = Some(i);
                        }
                        if r.double_clicked() {
                            self.editing = Some((Some(i), e.clone()));
                        }
                        ui.label(&e.location);
                        ui.end_row();
                    }
                });
        });
        ui.horizontal(|ui| {
            if ui
                .button("Add")
                .on_hover_text("Add a new shared sessions file entry")
                .clicked()
            {
                self.editing = Some((None, SharedSessionsEntry::default()));
            }
            let sel = self.selected.filter(|i| *i < self.entries.len());
            if ui
                .add_enabled(sel.is_some(), egui::Button::new("Delete"))
                .on_hover_text("Delete selected shared sessions file entry")
                .clicked()
                && let Some(i) = sel
            {
                self.entries.remove(i);
                self.selected = None;
            }
            if ui
                .add_enabled(sel.is_some(), egui::Button::new("Edit"))
                .clicked()
                && let Some(i) = sel
            {
                self.editing = Some((Some(i), self.entries[i].clone()));
            }
            if ui
                .add_enabled(
                    sel.is_some_and(|i| i > 0),
                    egui::Button::new(icon::ARROW_UP),
                )
                .on_hover_text("Move up selected shared sessions file entry")
                .clicked()
                && let Some(i) = sel
            {
                self.entries.swap(i, i - 1);
                self.selected = Some(i - 1);
            }
            if ui
                .add_enabled(
                    sel.is_some_and(|i| i + 1 < self.entries.len()),
                    egui::Button::new(icon::ARROW_DOWN),
                )
                .on_hover_text("Move down selected shared sessions file entry")
                .clicked()
                && let Some(i) = sel
            {
                self.entries.swap(i, i + 1);
                self.selected = Some(i + 1);
            }
        });
    }

    fn edit_page(&mut self, ui: &mut egui::Ui) {
        let Some((_, entry)) = &mut self.editing else {
            return;
        };
        let mut done = None;
        ui.group(|ui| {
            ui.strong("Shared sessions root folder");
            ui.horizontal(|ui| {
                ui.label("Tree folder name:");
                ui.add(egui::TextEdit::singleline(&mut entry.name).desired_width(154.0));
            });
        });
        ui.group(|ui| {
            ui.strong("Shared sessions file location");
            ui.horizontal(|ui| {
                ui.label("Backend protocol:");
                egui::ComboBox::from_id_salt("shared-backend")
                    .selected_text(entry.backend.label())
                    .show_ui(ui, |ui| {
                        for b in SharedBackend::ALL {
                            ui.selectable_value(&mut entry.backend, b, b.label());
                        }
                        for label in ["ftp / ftps", "ssh / sftp"] {
                            ui.add_enabled(false, egui::Button::new(label))
                                .on_disabled_hover_text(NOT_YET);
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("File path or URL:");
                ui.add(egui::TextEdit::singleline(&mut entry.location).desired_width(260.0));
                if entry.backend == SharedBackend::Folder
                    && ui.button(icon::FOLDER_OPEN).clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .add_filter("MobaXterm sessions", &["mxtsessions", "ini"])
                        .pick_file()
                {
                    entry.location = p.display().to_string();
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                done = Some(false);
            }
            let valid = !entry.location.trim().is_empty();
            if ui.add_enabled(valid, egui::Button::new("Save")).clicked() {
                done = Some(true);
            }
        });
        match done {
            Some(true) => {
                let (index, entry) = self.editing.take().expect("editing");
                match index {
                    Some(i) if i < self.entries.len() => self.entries[i] = entry,
                    _ => self.entries.push(entry),
                }
            }
            Some(false) => self.editing = None,
            None => {}
        }
    }
}
