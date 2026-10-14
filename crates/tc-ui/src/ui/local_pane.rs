//! Local side of the SFTP / FTP session view (reference `TFRAMEFTP`, left
//! panel): the local folder with Name / Size (KB) / Last modified, parent,
//! refresh, home and new folder. Files dragged to the remote list are
//! uploaded; remote files dropped here are downloaded into this folder.

use super::icon;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Local files being dragged to the remote list.
#[derive(Debug, Clone)]
pub struct LocalDrag(pub Vec<PathBuf>);

/// Remote files (full paths) being dragged to the local list.
#[derive(Debug, Clone)]
pub struct RemoteDrag(pub Vec<String>);

#[derive(Debug, Clone)]
struct LocalEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
}

pub struct LocalPane {
    pub cwd: PathBuf,
    path_input: String,
    entries: Vec<LocalEntry>,
    /// Folder the entries were read from (re-read when it changes).
    listed: Option<PathBuf>,
    selected: BTreeSet<String>,
    error: Option<String>,
    new_folder: Option<String>,
    /// Folder tree: sub-folder names of the folders opened in it.
    tree: HashMap<PathBuf, Vec<String>>,
}

/// Sub-folders shown per tree node at most.
const MAX_TREE_CHILDREN: usize = 500;

/// Tree roots: the drives on Windows, `/` and the home folder elsewhere.
fn tree_roots() -> Vec<PathBuf> {
    if cfg!(windows) {
        (b'A'..=b'Z')
            .map(|d| PathBuf::from(format!("{}:\\", d as char)))
            .filter(|p| p.is_dir())
            .collect()
    } else {
        let mut roots = vec![PathBuf::from("/")];
        if let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()) {
            roots.push(home);
        }
        roots
    }
}

/// Sub-folder names of `dir`, sorted, bounded.
fn subfolders(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .take(MAX_TREE_CHILDREN)
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// What the local pane asks the session to do.
pub enum LocalAction {
    /// Upload these files to the remote folder.
    Upload(Vec<PathBuf>),
    /// Download these remote paths into this local folder.
    Download(Vec<String>, PathBuf),
}

/// Longest folder listing kept (keeps huge folders responsive).
const MAX_ENTRIES: usize = 20_000;

impl LocalPane {
    pub fn new(start: Option<&str>) -> Self {
        let cwd = start
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("/"));
        Self {
            path_input: cwd.display().to_string(),
            cwd,
            entries: Vec::new(),
            listed: None,
            selected: BTreeSet::new(),
            error: None,
            new_folder: None,
            tree: HashMap::new(),
        }
    }

    /// The folder tree above the list (reference `sShellTreeView1`).
    fn tree_ui(&mut self, ui: &mut egui::Ui) {
        let mut go = None;
        egui::ScrollArea::vertical()
            .id_salt("local-tree")
            .max_height((ui.available_height() * 0.35).max(60.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for root in tree_roots() {
                    let label = root.display().to_string();
                    self.node(ui, &root, &label, &mut go);
                }
            });
        if let Some(dir) = go {
            self.cd(dir);
        }
    }

    fn node(&mut self, ui: &mut egui::Ui, dir: &Path, label: &str, go: &mut Option<PathBuf>) {
        let id = ui.make_persistent_id(("local-tree", dir));
        // Folders on the way to the current one start open.
        let on_path = self.cwd.starts_with(dir);
        let state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, on_path);
        let open = state.is_open();
        state
            .show_header(ui, |ui| {
                let current = self.cwd == dir;
                if ui
                    .selectable_label(current, format!("{}  {label}", icon::FOLDER))
                    .clicked()
                {
                    *go = Some(dir.to_path_buf());
                }
            })
            .body(|ui| {
                if open && !self.tree.contains_key(dir) {
                    self.tree.insert(dir.to_path_buf(), subfolders(dir));
                }
                let children = self.tree.get(dir).cloned().unwrap_or_default();
                for name in children {
                    self.node(ui, &dir.join(&name), &name, go);
                }
            });
    }

    fn cd(&mut self, dir: PathBuf) {
        self.path_input = dir.display().to_string();
        self.cwd = dir;
        self.listed = None;
        self.selected.clear();
    }

    fn refresh(&mut self) {
        self.error = None;
        self.entries = match std::fs::read_dir(&self.cwd) {
            Ok(dir) => dir
                .filter_map(Result::ok)
                .take(MAX_ENTRIES)
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    Some(LocalEntry {
                        name: e.file_name().to_string_lossy().into_owned(),
                        is_dir: meta.is_dir(),
                        size: meta.len(),
                        modified: meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs()),
                    })
                })
                .collect(),
            Err(e) => {
                self.error = Some(format!("{}: {e}", self.cwd.display()));
                Vec::new()
            }
        };
        self.entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.listed = Some(self.cwd.clone());
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|e| !e.is_dir && self.selected.contains(&e.name))
            .map(|e| self.cwd.join(&e.name))
            .collect()
    }

    pub fn show(&mut self, ui: &mut egui::Ui, out: &mut Vec<LocalAction>) {
        if self.listed.as_ref() != Some(&self.cwd) {
            self.refresh();
        }
        self.toolbar(ui, out);
        self.tree_ui(ui);
        ui.separator();
        let r =
            ui.add(egui::TextEdit::singleline(&mut self.path_input).desired_width(f32::INFINITY));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let dir = PathBuf::from(self.path_input.trim());
            if dir.is_dir() {
                self.cd(dir);
            } else {
                self.error = Some(format!("{} is not a folder", dir.display()));
            }
        }
        if let Some(name) = &mut self.new_folder {
            let mut done = None;
            ui.horizontal(|ui| {
                ui.label("New folder:");
                let r = ui.text_edit_singleline(name);
                r.request_focus();
                if ui.button("Create").clicked()
                    || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
            match done {
                Some(true) => {
                    let name = name.trim().to_string();
                    self.new_folder = None;
                    if valid_name(&name) {
                        if let Err(e) = std::fs::create_dir(self.cwd.join(&name)) {
                            self.error = Some(e.to_string());
                        }
                        self.listed = None;
                    } else {
                        self.error = Some(format!("invalid folder name {name:?}"));
                    }
                }
                Some(false) => self.new_folder = None,
                None => {}
            }
        }
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        let list = self.list(ui);
        // Remote files dropped here: download into this folder.
        let zone = ui.interact(list, ui.id().with("local-drop"), egui::Sense::hover());
        if let Some(drag) = zone.dnd_release_payload::<RemoteDrag>() {
            out.push(LocalAction::Download(drag.0.clone(), self.cwd.clone()));
        }
        if zone.dnd_hover_payload::<RemoteDrag>().is_some() {
            ui.painter().rect_stroke(
                list,
                4.0,
                egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, out: &mut Vec<LocalAction>) {
        let tool = |ui: &mut egui::Ui, glyph: &str, hint: &str, enabled: bool| {
            ui.add_enabled(
                enabled,
                egui::Button::new(egui::RichText::new(glyph).size(15.0)).frame(false),
            )
            .on_hover_text(hint)
            .clicked()
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new(icon::DESKTOP).strong())
                .on_hover_text("Local files");
            let parent = self.cwd.parent().map(Path::to_path_buf);
            if tool(ui, icon::ARROW_UP, "Parent directory", parent.is_some())
                && let Some(p) = parent
            {
                self.cd(p);
            }
            if tool(ui, icon::ARROWS_CLOCKWISE, "Refresh folder", true) {
                self.listed = None;
                self.tree.clear();
            }
            if tool(ui, icon::HOUSE, "Home folder", true)
                && let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())
            {
                self.cd(home);
            }
            if tool(ui, icon::FOLDER_PLUS, "Create new directory", true) {
                self.new_folder = Some(String::new());
            }
            let files = self.selected_paths();
            if tool(
                ui,
                icon::UPLOAD_SIMPLE,
                "Upload selected files to the remote folder",
                !files.is_empty(),
            ) {
                out.push(LocalAction::Upload(files));
            }
        });
    }

    /// The file list; returns its rectangle (the drop zone).
    fn list(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let mut cd = None;
        let response = egui::ScrollArea::both()
            .id_salt("local-list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("local-grid")
                    .num_columns(3)
                    .striped(true)
                    .spacing([10.0, 2.0])
                    .show(ui, |ui| {
                        for h in ["Name", "Size (KB)", "Last modified"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        if let Some(parent) = self.cwd.parent() {
                            let up = ui.selectable_label(false, format!("{}  ..", icon::FOLDER));
                            if up.double_clicked() {
                                cd = Some(parent.to_path_buf());
                            }
                            ui.end_row();
                        }
                        for e in &self.entries {
                            let glyph = if e.is_dir { icon::FOLDER } else { icon::FILE };
                            let selected = self.selected.contains(&e.name);
                            let r = ui
                                .selectable_label(selected, format!("{glyph}  {}", e.name))
                                .interact(egui::Sense::click_and_drag());
                            if r.clicked() {
                                if ui.input(|i| i.modifiers.command) {
                                    if !self.selected.remove(&e.name) {
                                        self.selected.insert(e.name.clone());
                                    }
                                } else {
                                    self.selected = std::iter::once(e.name.clone()).collect();
                                }
                            }
                            if r.double_clicked() {
                                if e.is_dir {
                                    cd = Some(self.cwd.join(&e.name));
                                } else {
                                    let _ = crate::external::open_path(
                                        &self.cwd.join(&e.name).display().to_string(),
                                    );
                                }
                            }
                            if !e.is_dir && r.drag_started() {
                                // Drag the selection if this file is part of it.
                                let mut files: Vec<PathBuf> = if selected {
                                    self.entries
                                        .iter()
                                        .filter(|x| !x.is_dir && self.selected.contains(&x.name))
                                        .map(|x| self.cwd.join(&x.name))
                                        .collect()
                                } else {
                                    Vec::new()
                                };
                                if files.is_empty() {
                                    files.push(self.cwd.join(&e.name));
                                }
                                r.dnd_set_drag_payload(LocalDrag(files));
                            }
                            ui.label(if e.is_dir {
                                String::new()
                            } else {
                                e.size.div_ceil(1024).to_string()
                            });
                            ui.label(
                                e.modified
                                    .map(super::sftp_browser::format_time)
                                    .unwrap_or_default(),
                            );
                            ui.end_row();
                        }
                    });
            });
        if let Some(dir) = cd {
            self.cd(dir);
        }
        response.inner_rect
    }
}

/// A plain file / folder name: no separators, not `.` / `..`.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_folders_first_and_navigates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), b"12345").unwrap();
        std::fs::create_dir(dir.path().join("Zed")).unwrap();
        let mut pane = LocalPane::new(Some(&dir.path().display().to_string()));
        pane.refresh();
        let names: Vec<&str> = pane.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Zed", "b.txt"]);
        pane.selected.insert("b.txt".into());
        pane.selected.insert("Zed".into());
        assert_eq!(
            pane.selected_paths(),
            [dir.path().join("b.txt")],
            "files only"
        );
        pane.cd(dir.path().join("Zed"));
        assert!(pane.selected.is_empty() && pane.listed.is_none());
        assert!(!valid_name("a/b") && !valid_name("..") && valid_name("new folder"));
        assert!(LocalPane::new(Some("/definitely/not/here")).cwd.is_dir());
        assert_eq!(subfolders(dir.path()), ["Zed"], "folders only");
        assert!(!tree_roots().is_empty());
    }
}
