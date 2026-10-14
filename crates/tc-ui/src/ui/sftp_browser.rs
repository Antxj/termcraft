//! SSH-browser side panel (reference `sTabSFTP`): toolbar, path box, file
//! list (Name, Size (KB), Last modified, Owner, Group, Access), transfers, and
//! "Follow terminal folder". Files dropped from the OS are uploaded.

use super::{NOT_YET, icon};
use crate::remote_edit::OpenWith;
use crate::tab::{BrowserPrompt, SftpStatus, SftpView, Tab};
use std::path::PathBuf;
use tc_ssh::sftp::{join, parent};
use tc_ssh::{RemoteEntry, SftpRequest};

/// The SSH-browser in the sidebar.
pub fn show(ui: &mut egui::Ui, tab: Option<&mut Tab>) {
    show_in(ui, tab, false);
}

/// The remote side of the two-pane SFTP / FTP session view: no "Follow
/// terminal folder" nor monitoring; local files can be dropped on it.
pub fn show_frame(ui: &mut egui::Ui, tab: &mut Tab) {
    show_in(ui, Some(tab), true);
}

fn show_in(ui: &mut egui::Ui, tab: Option<&mut Tab>, frame: bool) {
    let Some(tab) = tab else {
        ui.weak("The SSH-browser appears here when an SSH session is connected.");
        return;
    };
    match &tab.sftp {
        SftpStatus::Off => {
            ui.weak("The SSH-browser appears here when an SSH session is connected.");
            return;
        }
        SftpStatus::Connecting => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Connecting SSH-browser…");
            });
            return;
        }
        SftpStatus::Unavailable(e) => {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("SSH-browser unavailable: {e}"),
            );
            return;
        }
        SftpStatus::Ready(_) => {}
    }

    let mut actions = Vec::new();
    {
        let SftpStatus::Ready(view) = &mut tab.sftp else {
            return;
        };
        toolbar(ui, view, &mut actions);
        path_box(ui, view, &mut actions);
        prompt(ui, view, &mut actions);
        if let Some(e) = &view.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        transfers(ui, view);
        let list_rect = file_list(ui, view, &mut actions, frame);
        dropped_files(ui, list_rect, view, &mut actions);
        if frame {
            for action in actions {
                apply(tab, action);
            }
            return;
        }
        ui.separator();
        follow_folder(ui, view, &mut actions);
    }
    let monitoring = tab.monitor.is_some();
    let label = format!("{} Remote monitoring", icon::GAUGE);
    if ui
        .add(egui::Button::new(label).selected(monitoring))
        .on_hover_text(
            "Monitor resources usage (CPU / RAM / network / disk…) of the server in a \
             small bar below the SSH terminal (Linux servers).",
        )
        .clicked()
    {
        tab.monitoring_request = Some(!monitoring);
    }
    for action in actions {
        apply(tab, action);
    }
}

enum BrowserAction {
    Cd(String),
    Request(SftpRequest),
    TypeInTerminal(String),
    InstallFolderTracking,
    /// Reopen the browser with (or without) sudo.
    Sudo(bool),
    /// Open a remote file in a local program and watch it.
    Edit(String, OpenWith),
    SudoPassword(String),
}

fn apply(tab: &mut Tab, action: BrowserAction) {
    match action {
        BrowserAction::Cd(path) => tab.sftp_list(&path),
        BrowserAction::Request(r) => tab.sftp_op(r),
        BrowserAction::TypeInTerminal(text) => tab.send(text.as_bytes()),
        BrowserAction::InstallFolderTracking => tab.install_folder_tracking(),
        BrowserAction::Sudo(on) => tab.sftp_sudo(on, None),
        BrowserAction::Edit(path, how) => tab.sftp_edit(&path, how),
        BrowserAction::SudoPassword(pw) => tab.sftp_sudo(true, Some(pw)),
    }
}

/// "Follow terminal folder": follows OSC 7 reports from the shell. If the
/// shell has not reported anything yet, offer to install the prompt hook.
fn follow_folder(ui: &mut egui::Ui, view: &mut SftpView, out: &mut Vec<BrowserAction>) {
    ui.checkbox(&mut view.follow, "Follow terminal folder")
        .on_hover_text("Automatically browse to SSH current folder");
    if view.follow && !view.reports_seen && !view.auto_hook_pending {
        let install = ui
            .small_button("Enable for this shell")
            .on_hover_text(
                "Your shell does not report its folder yet. This types a one-line                  prompt hook (bash / zsh) into the terminal so the browser can follow                  `cd`. Nothing is changed on the server permanently.",
            );
        if install.clicked() {
            out.push(BrowserAction::InstallFolderTracking);
        }
    }
}

fn tool(ui: &mut egui::Ui, glyph: &str, hint: &str, enabled: bool) -> bool {
    let b = ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(glyph).size(15.0)).frame(false),
    );
    if enabled {
        b.on_hover_text(hint).clicked()
    } else {
        b.on_disabled_hover_text(format!("{hint}\n{NOT_YET}"));
        false
    }
}

/// A toolbar toggle (pressed look while on).
fn toggle(ui: &mut egui::Ui, glyph: &str, hint: &str, on: bool) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(glyph).size(15.0))
            .frame(on)
            .selected(on),
    )
    .on_hover_text(hint)
    .clicked()
}

fn selected_entries(view: &SftpView) -> Vec<&RemoteEntry> {
    view.entries
        .iter()
        .filter(|e| view.selected.contains(&e.name))
        .collect()
}

fn toolbar(ui: &mut egui::Ui, view: &mut SftpView, out: &mut Vec<BrowserAction>) {
    let has_selection = !view.selected.is_empty();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if tool(ui, icon::ARROW_UP, "Parent directory", view.cwd != "/") {
            out.push(BrowserAction::Cd(parent(&view.cwd)));
        }
        if tool(
            ui,
            icon::DOWNLOAD_SIMPLE,
            "Download selected files",
            has_selection,
        ) {
            download(view, &selected_entries(view), out);
        }
        if tool(ui, icon::UPLOAD_SIMPLE, "Upload to current folder", true)
            && let Some(files) = rfd::FileDialog::new()
                .set_title("Upload files")
                .pick_files()
        {
            upload(view, files, out);
        }
        if tool(ui, icon::ARROWS_CLOCKWISE, "Refresh folder", true) {
            out.push(BrowserAction::Cd(view.cwd.clone()));
        }
        if tool(ui, icon::FOLDER_PLUS, "Create new directory", true) {
            view.prompt = Some(BrowserPrompt::NewFolder(String::new()));
        }
        if tool(ui, icon::FILE_PLUS, "Create new file", true) {
            view.prompt = Some(BrowserPrompt::NewFile(String::new()));
        }
        if tool(ui, icon::TRASH, "Delete selected files", has_selection) {
            let items = selected_entries(view)
                .iter()
                .map(|e| (join(&view.cwd, &e.name), e.is_dir))
                .collect();
            view.prompt = Some(BrowserPrompt::ConfirmDelete(items));
        }
        if view.sudo_supported
            && toggle(
                ui,
                icon::SHIELD_CHECK,
                "Use SUDO for SSH-browser",
                view.sudo,
            )
        {
            out.push(BrowserAction::Sudo(!view.sudo));
        }
        if toggle(ui, icon::TEXT_AA, "ASCII mode", view.ascii) {
            view.ascii = !view.ascii;
        }
        let hidden_hint = if view.show_hidden {
            "Hide hidden (dot) files"
        } else {
            "Show hidden (dot) files"
        };
        if tool(ui, icon::EYE, hidden_hint, true) {
            view.show_hidden = !view.show_hidden;
        }
        if tool(ui, icon::COPY, "Copy file/folder path", true) {
            let path = first_selected_path(view);
            ui.ctx().copy_text(path);
        }
        if tool(
            ui,
            icon::TERMINAL,
            "Copy file path to terminal (Middle mouse click)",
            true,
        ) {
            out.push(BrowserAction::TypeInTerminal(shell_quote(
                &first_selected_path(view),
            )));
        }
    });
}

fn first_selected_path(view: &SftpView) -> String {
    view.selected
        .iter()
        .next()
        .map_or_else(|| view.cwd.clone(), |n| join(&view.cwd, n))
}

/// Quote a path for a POSIX shell when it contains special characters.
fn shell_quote(path: &str) -> String {
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c))
    {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

fn path_box(ui: &mut egui::Ui, view: &mut SftpView, out: &mut Vec<BrowserAction>) {
    let r = ui.add(egui::TextEdit::singleline(&mut view.path_input).desired_width(f32::INFINITY));
    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let path = view.path_input.trim().to_string();
        if !path.is_empty() {
            out.push(BrowserAction::Cd(path));
        }
    }
}

fn prompt(ui: &mut egui::Ui, view: &mut SftpView, out: &mut Vec<BrowserAction>) {
    let Some(mut p) = view.prompt.take() else {
        return;
    };
    let cwd = view.cwd.clone();
    let mut close = false;
    // Some(true) = OK, Some(false) = Cancel.
    fn buttons(ui: &mut egui::Ui, enter: bool) -> Option<bool> {
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() || enter {
                Some(true)
            } else if ui.button("Cancel").clicked() {
                Some(false)
            } else {
                None
            }
        })
        .inner
    }
    egui::Frame::group(ui.style()).show(ui, |ui| match &mut p {
        BrowserPrompt::NewFolder(name) | BrowserPrompt::NewFile(name) => {
            ui.label("Name:");
            let edit = ui.text_edit_singleline(name);
            edit.request_focus();
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let choice = buttons(ui, enter);
            close |= choice == Some(false);
            if choice == Some(true) && !name.trim().is_empty() {
                let path = join(&cwd, name.trim());
                out.push(BrowserAction::Request(match p {
                    BrowserPrompt::NewFolder(_) => SftpRequest::Mkdir(path),
                    _ => SftpRequest::CreateFile(path),
                }));
                close = true;
            }
        }
        BrowserPrompt::Rename { from, to } => {
            ui.label(format!("Rename {from} to:"));
            let edit = ui.text_edit_singleline(to);
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let choice = buttons(ui, enter);
            close |= choice == Some(false);
            if choice == Some(true) && !to.trim().is_empty() {
                out.push(BrowserAction::Request(SftpRequest::Rename {
                    from: join(&cwd, from),
                    to: join(&cwd, to.trim()),
                }));
                close = true;
            }
        }
        BrowserPrompt::SudoPassword(password) => {
            ui.label("sudo password:");
            let edit = ui.add(egui::TextEdit::singleline(&mut **password).password(true));
            edit.request_focus();
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let choice = buttons(ui, enter);
            close |= choice == Some(false);
            if choice == Some(true) {
                out.push(BrowserAction::SudoPassword(std::mem::take(&mut **password)));
                close = true;
            }
        }
        BrowserPrompt::Permissions { path, mode } => {
            ui.label(format!("Permissions of {path}"));
            egui::Grid::new("perm-grid").num_columns(4).show(ui, |ui| {
                ui.label("");
                for h in ["Read", "Write", "Execute"] {
                    ui.label(h);
                }
                ui.end_row();
                for (who, shift) in [("Owner", 6), ("Group", 3), ("Others", 0)] {
                    ui.label(who);
                    for bit in [4, 2, 1] {
                        let mask = bit << shift;
                        let mut on = *mode & mask != 0;
                        if ui.checkbox(&mut on, "").changed() {
                            *mode ^= mask;
                        }
                    }
                    ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Octal:");
                let mut octal = format!("{:04o}", *mode & 0o7777);
                if ui
                    .add(egui::TextEdit::singleline(&mut octal).desired_width(50.0))
                    .changed()
                    && let Ok(v) = u32::from_str_radix(octal.trim(), 8)
                    && v <= 0o7777
                {
                    *mode = v;
                }
            });
            let choice = buttons(ui, false);
            close |= choice == Some(false);
            if choice == Some(true) {
                out.push(BrowserAction::Request(SftpRequest::SetPermissions {
                    path: path.clone(),
                    mode: *mode,
                }));
                close = true;
            }
        }
        BrowserPrompt::Properties(e) => {
            egui::Grid::new("props-grid").num_columns(2).show(ui, |ui| {
                ui.label("Name:");
                ui.label(&e.name);
                ui.end_row();
                ui.label("Location:");
                ui.label(&cwd);
                ui.end_row();
                ui.label("Type:");
                ui.label(if e.is_dir { "Folder" } else { "File" });
                ui.end_row();
                ui.label("Size:");
                ui.label(format!("{} bytes", e.size));
                ui.end_row();
                ui.label("Modified:");
                ui.label(e.modified.map(format_time).unwrap_or_default());
                ui.end_row();
                ui.label("Owner / group:");
                ui.label(format!(
                    "{} / {}",
                    e.owner.as_deref().unwrap_or("?"),
                    e.group.as_deref().unwrap_or("?")
                ));
                ui.end_row();
                ui.label("Permissions:");
                ui.monospace(e.permissions.map(access_string).unwrap_or_default());
                ui.end_row();
            });
            if ui.button("Close").clicked() {
                close = true;
            }
        }
        BrowserPrompt::ConfirmDelete(items) => {
            ui.label(format!("Delete {} item(s)?", items.len()));
            for (path, _) in items.iter().take(5) {
                ui.weak(path.as_str());
            }
            let choice = buttons(ui, false);
            close |= choice == Some(false);
            if choice == Some(true) {
                for (path, is_dir) in items.drain(..) {
                    out.push(BrowserAction::Request(SftpRequest::Remove { path, is_dir }));
                }
                close = true;
            }
        }
    });
    if !close {
        view.prompt = Some(p);
    }
}

fn transfers(ui: &mut egui::Ui, view: &SftpView) {
    for (label, done, total) in view.transfers.values() {
        ui.label(egui::RichText::new(label).small());
        match total {
            Some(t) if *t > 0 => {
                ui.add(egui::ProgressBar::new(*done as f32 / *t as f32).show_percentage());
            }
            _ => {
                ui.add(egui::ProgressBar::new(0.0).text(format!("{} KB", done / 1024)));
            }
        }
    }
}

fn file_list(
    ui: &mut egui::Ui,
    view: &mut SftpView,
    out: &mut Vec<BrowserAction>,
    frame: bool,
) -> egui::Rect {
    let entries: Vec<RemoteEntry> = view
        .entries
        .iter()
        .filter(|e| view.show_hidden || !e.name.starts_with('.'))
        .cloned()
        .collect();
    // Leave room below for "Follow terminal folder" and its button.
    let reserve = if frame { 0.0 } else { 76.0 };
    let height = (ui.available_height() - reserve).max(80.0);
    let response = egui::ScrollArea::both()
        .id_salt("sftp-list")
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("sftp-grid")
                .num_columns(6)
                .striped(true)
                .spacing([10.0, 2.0])
                .show(ui, |ui| {
                    for h in [
                        "Name",
                        "Size (KB)",
                        "Last modified",
                        "Owner",
                        "Group",
                        "Access",
                    ] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    if view.cwd != "/" {
                        let up = ui.selectable_label(false, format!("{}  ..", icon::FOLDER));
                        if up.double_clicked() {
                            out.push(BrowserAction::Cd(parent(&view.cwd)));
                        }
                        ui.end_row();
                    }
                    for e in &entries {
                        row(ui, view, e, out);
                    }
                });
        });
    response.inner_rect
}

fn row(ui: &mut egui::Ui, view: &mut SftpView, e: &RemoteEntry, out: &mut Vec<BrowserAction>) {
    let glyph = if e.is_dir { icon::FOLDER } else { icon::FILE };
    let selected = view.selected.contains(&e.name);
    let r = ui
        .selectable_label(selected, format!("{glyph}  {}", e.name))
        .interact(egui::Sense::click_and_drag());
    if !e.is_dir && r.drag_started() {
        // Drag the selection (to the local pane) if this file is in it.
        let mut paths: Vec<String> = if selected {
            selected_entries(view)
                .iter()
                .filter(|x| !x.is_dir)
                .map(|x| join(&view.cwd, &x.name))
                .collect()
        } else {
            Vec::new()
        };
        if paths.is_empty() {
            paths.push(join(&view.cwd, &e.name));
        }
        r.dnd_set_drag_payload(super::local_pane::RemoteDrag(paths));
    }
    if r.clicked() {
        if ui.input(|i| i.modifiers.command) {
            if !view.selected.remove(&e.name) {
                view.selected.insert(e.name.clone());
            }
        } else {
            view.selected = std::iter::once(e.name.clone()).collect();
        }
    }
    if r.double_clicked() {
        if e.is_dir {
            out.push(BrowserAction::Cd(join(&view.cwd, &e.name)));
        } else {
            // Like the reference: a file opens in the text editor, an
            // image in the viewer.
            let how = if super::image_viewer::is_image(&e.name) {
                OpenWith::Image
            } else {
                OpenWith::TextEditor
            };
            out.push(BrowserAction::Edit(join(&view.cwd, &e.name), how));
        }
    }
    if r.middle_clicked() {
        out.push(BrowserAction::TypeInTerminal(shell_quote(&join(
            &view.cwd, &e.name,
        ))));
    }
    r.context_menu(|ui| context_menu(ui, view, e, out));
    ui.label(if e.is_dir {
        String::new()
    } else {
        format!("{}", e.size.div_ceil(1024))
    });
    ui.label(e.modified.map(format_time).unwrap_or_default());
    ui.label(e.owner.as_deref().unwrap_or(""));
    ui.label(e.group.as_deref().unwrap_or(""));
    ui.monospace(e.permissions.map(access_string).unwrap_or_default());
    ui.end_row();
}

/// Visible entries of the reference `PopupMenu5`.
fn context_menu(
    ui: &mut egui::Ui,
    view: &mut SftpView,
    e: &RemoteEntry,
    out: &mut Vec<BrowserAction>,
) {
    let path = join(&view.cwd, &e.name);
    if e.is_dir && ui.button("Open").clicked() {
        out.push(BrowserAction::Cd(path.clone()));
        ui.close();
    }
    if !e.is_dir {
        if ui.button("Open with default text editor").clicked() {
            out.push(BrowserAction::Edit(path.clone(), OpenWith::TextEditor));
            ui.close();
        }
        if ui
            .button("Open with default program")
            .on_hover_text("Runs the program your system uses for this kind of file")
            .clicked()
        {
            out.push(BrowserAction::Edit(path.clone(), OpenWith::DefaultProgram));
            ui.close();
        }
    }
    if !e.is_dir
        && ui
            .button("Open with...")
            .on_hover_text("Choose the program that opens this file")
            .clicked()
    {
        ui.close();
        if let Some(program) = rfd::FileDialog::new().set_title("Open with…").pick_file() {
            out.push(BrowserAction::Edit(
                path.clone(),
                OpenWith::Program(program),
            ));
        }
    }
    if !e.is_dir
        && ui
            .button("Compare text files")
            .on_hover_text("Compare this file with another one in the Diff tool")
            .clicked()
    {
        out.push(BrowserAction::Edit(path.clone(), OpenWith::Compare));
        ui.close();
    }
    if !e.is_dir && ui.button("Download").clicked() {
        download(view, &[e], out);
        ui.close();
    }
    ui.separator();
    if ui.button("Delete").clicked() {
        view.prompt = Some(BrowserPrompt::ConfirmDelete(vec![(path.clone(), e.is_dir)]));
        ui.close();
    }
    if ui.button("Rename").clicked() {
        view.prompt = Some(BrowserPrompt::Rename {
            from: e.name.clone(),
            to: e.name.clone(),
        });
        ui.close();
    }
    ui.separator();
    if ui.button("Copy file path").clicked() {
        ui.ctx().copy_text(path.clone());
        ui.close();
    }
    if ui
        .button("Copy file path to terminal (Middle mouse click)")
        .clicked()
    {
        out.push(BrowserAction::TypeInTerminal(shell_quote(&path)));
        ui.close();
    }
    if ui.button("Properties").clicked() {
        view.prompt = Some(BrowserPrompt::Properties(e.clone()));
        ui.close();
    }
    if ui.button("Permissions").clicked() {
        view.prompt = Some(BrowserPrompt::Permissions {
            path: path.clone(),
            mode: e
                .permissions
                .unwrap_or(if e.is_dir { 0o755 } else { 0o644 }),
        });
        ui.close();
    }
}

/// Ask where to save, then queue downloads (directories are skipped). In
/// the two-pane view they go straight to the local folder.
fn download(view: &SftpView, entries: &[&RemoteEntry], out: &mut Vec<BrowserAction>) {
    let files: Vec<&&RemoteEntry> = entries.iter().filter(|e| !e.is_dir).collect();
    if let Some(dir) = &view.local_target {
        for e in files {
            out.push(BrowserAction::Request(SftpRequest::Download {
                remote: join(&view.cwd, &e.name),
                local: dir.join(&e.name),
                ascii: view.ascii,
            }));
        }
        return;
    }
    match files.as_slice() {
        [] => {}
        [one] => {
            if let Some(local) = rfd::FileDialog::new().set_file_name(&one.name).save_file() {
                out.push(BrowserAction::Request(SftpRequest::Download {
                    remote: join(&view.cwd, &one.name),
                    local,
                    ascii: view.ascii,
                }));
            }
        }
        many => {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("Download to folder")
                .pick_folder()
            {
                for e in many {
                    out.push(BrowserAction::Request(SftpRequest::Download {
                        remote: join(&view.cwd, &e.name),
                        local: dir.join(&e.name),
                        ascii: view.ascii,
                    }));
                }
            }
        }
    }
}

fn upload(view: &SftpView, files: Vec<PathBuf>, out: &mut Vec<BrowserAction>) {
    for local in files.into_iter().filter(|p| p.is_file()) {
        let Some(name) = local.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        out.push(BrowserAction::Request(SftpRequest::Upload {
            remote: join(&view.cwd, &name),
            local,
            ascii: view.ascii,
        }));
    }
}

/// Upload files dragged from the OS and dropped over the list.
fn dropped_files(ui: &egui::Ui, rect: egui::Rect, view: &SftpView, out: &mut Vec<BrowserAction>) {
    let (hovering, dropped, pointer) = ui.input(|i| {
        (
            !i.raw.hovered_files.is_empty(),
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect::<Vec<_>>(),
            i.pointer.latest_pos(),
        )
    });
    let over = pointer.is_none_or(|p| rect.contains(p));
    // Files dragged from the local pane of the two-pane view.
    let zone = ui.interact(rect, ui.id().with("remote-drop"), egui::Sense::hover());
    if let Some(drag) = zone.dnd_release_payload::<super::local_pane::LocalDrag>() {
        upload(view, drag.0.clone(), out);
    }
    let local_drag = zone
        .dnd_hover_payload::<super::local_pane::LocalDrag>()
        .is_some();
    if (hovering && over) || local_drag {
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }
    if !dropped.is_empty() && over {
        upload(view, dropped, out);
    }
}

/// `rwxr-xr-x` from Unix permission bits.
fn access_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// `YYYY-MM-DD HH:MM` in the local time zone, from Unix seconds.
pub fn format_time(secs: u64) -> String {
    format_time_in(secs, &chrono::Local)
}

fn format_time_in<Tz: chrono::TimeZone>(secs: u64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    i64::try_from(secs)
        .ok()
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map(|t| t.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(access_string(0o755), "rwxr-xr-x");
        assert_eq!(access_string(0o640), "rw-r-----");
        assert_eq!(format_time_in(0, &chrono::Utc), "1970-01-01 00:00");
        assert_eq!(
            format_time_in(1_791_914_134, &chrono::Utc),
            "2026-10-13 17:55"
        );
        let brt = chrono::FixedOffset::west_opt(3 * 3600).unwrap();
        assert_eq!(format_time_in(1_791_914_134, &brt), "2026-10-13 14:55");
        assert_eq!(shell_quote("/home/u/a.txt"), "/home/u/a.txt");
        assert_eq!(shell_quote("/tmp/it's here"), "'/tmp/it'\\''s here'");
    }
}
