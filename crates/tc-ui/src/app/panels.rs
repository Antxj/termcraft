//! The terminal area, status bar and the banners above the terminals.

use super::*;

impl TermCraftApp {
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut install = None;
                let mut restart = false;
                match &self.updates.state {
                    UpdateState::Idle | UpdateState::UpToDate => {}
                    UpdateState::Checking => {
                        ui.label("Checking for updates…");
                    }
                    UpdateState::Available(update) => {
                        ui.label(format!("Update {} available", update.version));
                        if ui.button("Install").clicked() {
                            install = Some(update.clone());
                        }
                    }
                    UpdateState::Installing => {
                        ui.label("Installing update…");
                    }
                    UpdateState::Installed(v) => {
                        ui.label(format!("Updated to {v}"));
                        if ui.button("Restart now").clicked() {
                            restart = true;
                        }
                    }
                    UpdateState::Error(e) => {
                        ui.weak(format!("Updates: {e}"));
                    }
                }
                if let Some(update) = install {
                    self.updates.install(&ctx, update);
                }
                if restart {
                    // The new version starts once this one has exited (the
                    // "Close prompt" still asks about open sessions).
                    self.restart_after_exit = true;
                    self.request_exit(&ctx);
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                    if ui.small_button(ui::icon::X).clicked() {
                        self.status.clear();
                    }
                }
            });
        });
    }
}

/// Reference macro-recording bar; returns true when STOP is clicked.
pub(super) fn recording_banner(ui: &mut egui::Ui) -> bool {
    let mut stop = false;
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(120, 24, 24))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  Recording macro: everything typed in this terminal is recorded",
                        ui::icon::RECORD
                    ))
                    .color(egui::Color32::WHITE),
                );
                stop = ui.button("STOP").clicked();
            });
        });
    stop
}

/// The MultiExec bar: which terminals take part (reference
/// `PopupMultiExec`: activate all / none / invert, synchronized wheel).
pub(super) fn multiexec_banner(ui: &mut egui::Ui, multiexec: &mut MultiExec, tabs: &mut [Tab]) {
    egui::Frame::new()
        .fill(ui.visuals().selection.bg_fill)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let text = ui.visuals().selection.stroke.color;
                ui.label(
                    egui::RichText::new("Multi-execution mode: commands are typed to").color(text),
                );
                for tab in tabs.iter_mut().filter(|t| t.is_alive()) {
                    let label = egui::RichText::new(&tab.title).color(text);
                    ui.checkbox(&mut tab.multiexec_on, label);
                }
                ui.menu_button(ui::icon::DOTS_THREE, |ui| {
                    if ui.button("Activate all").clicked() {
                        tabs.iter_mut().for_each(|t| t.multiexec_on = true);
                    }
                    if ui.button("Activate none").clicked() {
                        tabs.iter_mut().for_each(|t| t.multiexec_on = false);
                    }
                    if ui.button("Invert selection").clicked() {
                        tabs.iter_mut()
                            .for_each(|t| t.multiexec_on = !t.multiexec_on);
                    }
                    ui.separator();
                    ui.checkbox(
                        &mut multiexec.sync_wheel,
                        "Synchronize mouse wheel scrolling",
                    );
                });
                if ui.button("Exit multi-execution mode").clicked() {
                    multiexec.on = false;
                }
            });
        });
}

/// Tabs that receive input typed in `source`: the source itself, and in
/// MultiExec mode every live terminal that takes part.
pub(super) fn receives(tab: &Tab, source: u64, multiexec: bool) -> bool {
    tab.id == source || multiexec && tab.multiexec_on && tab.is_alive()
}

/// "Find next": the next match above the last one (or from the bottom).
pub(super) fn find_next(tab: &mut Tab) {
    let Some(bar) = &mut tab.find else {
        return;
    };
    match tab.term.find(&bar.text, bar.last, bar.match_case) {
        Some((start, _)) => {
            bar.last = Some(start);
            bar.not_found = false;
        }
        None => {
            // Wrap around once from the bottom.
            bar.not_found = bar.last.is_none();
            bar.last = None;
            if let Some((start, _)) = tab.term.find(&bar.text, None, bar.match_case) {
                bar.last = Some(start);
            }
        }
    }
}

/// The "Find in terminal" strip above a terminal. Enter / F3 find the next
/// match upwards; Esc closes it.
pub(super) fn find_bar(ui: &mut egui::Ui, tab: &mut Tab) {
    let Some(bar) = &mut tab.find else {
        return;
    };
    let mut close = false;
    let mut search = false;
    ui.horizontal(|ui| {
        ui.label(ui::icon::MAGNIFYING_GLASS);
        let edit = ui.add(
            egui::TextEdit::singleline(&mut bar.text)
                .hint_text("Find in terminal")
                .desired_width(220.0),
        );
        if std::mem::take(&mut bar.focus) {
            edit.request_focus();
        }
        if edit.changed() {
            bar.last = None;
            bar.not_found = false;
        }
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            search = true;
            edit.request_focus();
        }
        if ui.button("Find next").on_hover_text("F3").clicked() {
            search = true;
        }
        if ui.checkbox(&mut bar.match_case, "Match case").changed() {
            bar.last = None;
        }
        if bar.not_found {
            ui.colored_label(ui.visuals().warn_fg_color, "Not found");
        }
        if ui
            .small_button(ui::icon::X)
            .on_hover_text("Close")
            .clicked()
            || ui.input(|i| i.key_pressed(egui::Key::Escape)) && edit.has_focus()
        {
            close = true;
        }
    });
    if search {
        find_next(tab);
    }
    if close {
        tab.find = None;
        tab.term.clear_selection();
    }
}

/// Draw the terminals for the current split mode: the active tab alone, or
/// the first 2 / 4 tabs in panes. Returns each pane's output.
pub(super) fn terminals(
    ui: &mut egui::Ui,
    core: &mut Core,
    split: tc_core::settings::SplitMode,
    font_size: f32,
) -> Vec<(u64, term_view::TermOutput)> {
    use tc_core::settings::SplitMode as S;
    let opts = &core.settings.terminal.clone();
    let area = ui.available_rect_before_wrap();
    let gap = 3.0;
    let rects: Vec<egui::Rect> = match split {
        S::Single => vec![area],
        S::Vertical => {
            let (l, r) = area.split_left_right_at_fraction(0.5);
            vec![
                l.shrink2(egui::vec2(gap, 0.0)),
                r.shrink2(egui::vec2(gap, 0.0)),
            ]
        }
        S::Horizontal => {
            let (t, b) = area.split_top_bottom_at_fraction(0.5);
            vec![
                t.shrink2(egui::vec2(0.0, gap)),
                b.shrink2(egui::vec2(0.0, gap)),
            ]
        }
        S::Quad => {
            let (t, b) = area.split_top_bottom_at_fraction(0.5);
            let (tl, tr) = t.split_left_right_at_fraction(0.5);
            let (bl, br) = b.split_left_right_at_fraction(0.5);
            [tl, tr, bl, br].map(|r| r.shrink(gap)).to_vec()
        }
    };
    let active = core.active;
    let ids: Vec<u64> = if split == S::Single {
        active.into_iter().collect()
    } else {
        core.tabs.iter().take(rects.len()).map(|t| t.id).collect()
    };
    let mut outputs = Vec::new();
    for (i, rect) in rects.iter().enumerate() {
        let mut pane = ui.new_child(egui::UiBuilder::new().max_rect(*rect));
        match ids
            .get(i)
            .and_then(|id| core.tabs.iter_mut().find(|t| t.id == *id))
        {
            Some(tab) if tab.detached => {
                pane.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 3.0);
                        ui.label(format!("\"{}\" is shown in its own window.", tab.title));
                        if ui.button("Re-attach tab").clicked() {
                            tab.detached = false;
                        }
                    });
                });
            }
            Some(tab) => {
                let is_active = Some(tab.id) == active;
                let id = tab.id;
                if tab.find.is_some() {
                    find_bar(&mut pane, tab);
                    let rest = pane.available_rect_before_wrap();
                    pane = ui.new_child(egui::UiBuilder::new().max_rect(rest));
                }
                if tab.is_file_session() {
                    // Reference TFRAMEFTP: local | remote files, the log below.
                    let (files, log) = rect.split_top_bottom_at_fraction(0.72);
                    let mut frame = ui.new_child(egui::UiBuilder::new().max_rect(files));
                    file_frame(&mut frame, tab);
                    pane = ui.new_child(
                        egui::UiBuilder::new().max_rect(log.shrink2(egui::vec2(0.0, 2.0))),
                    );
                }
                if tab.monitor.is_some() {
                    // "Remote monitoring" bar under the terminal.
                    let (term_rect, bar) =
                        rect.split_top_bottom_at_y(rect.bottom() - crate::ui::monitor_bar::HEIGHT);
                    crate::ui::monitor_bar::show(&mut pane, bar, tab.monitor.as_deref());
                    pane = ui.new_child(egui::UiBuilder::new().max_rect(term_rect));
                }
                outputs.push((
                    id,
                    term_view::show(&mut pane, tab, font_size, is_active, opts),
                ));
                if split != S::Single && is_active {
                    pane.painter().rect_stroke(
                        *rect,
                        0.0,
                        egui::Stroke::new(1.5, pane.visuals().selection.bg_fill),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            None => {
                pane.centered_and_justified(|ui| {
                    ui.weak("Open another tab to fill this terminal pane.");
                });
            }
        }
    }
    ui.allocate_rect(area, egui::Sense::hover());
    outputs
}

/// The two-pane SFTP / FTP view: local files on the left, the remote
/// browser on the right; drag files between them to transfer.
pub(super) fn file_frame(ui: &mut egui::Ui, tab: &mut crate::tab::Tab) {
    use crate::ui::local_pane::LocalAction;
    let rect = ui.max_rect();
    let (left, right) = rect.split_left_right_at_fraction(0.42);
    let mut actions = Vec::new();
    let mut l = ui.new_child(egui::UiBuilder::new().max_rect(left.shrink(4.0)));
    egui::Frame::group(l.style()).show(&mut l, |ui| {
        ui.set_min_size(ui.available_size());
        ui.push_id("local-pane", |ui| {
            tab.local_pane_mut().show(ui, &mut actions)
        });
    });
    let local_dir = tab.local_pane_mut().cwd.clone();
    if let crate::tab::SftpStatus::Ready(view) = &mut tab.sftp {
        view.local_target = Some(local_dir.clone());
    }
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink(4.0)));
    egui::Frame::group(r.style()).show(&mut r, |ui| {
        ui.set_min_size(ui.available_size());
        ui.push_id("remote-pane", |ui| {
            crate::ui::sftp_browser::show_frame(ui, tab)
        });
    });
    for action in actions {
        match action {
            LocalAction::Upload(files) => {
                let crate::tab::SftpStatus::Ready(view) = &tab.sftp else {
                    continue;
                };
                let (cwd, ascii) = (view.cwd.clone(), view.ascii);
                for local in files {
                    let Some(name) = local.file_name().map(|n| n.to_string_lossy().into_owned())
                    else {
                        continue;
                    };
                    tab.sftp_op(tc_ssh::SftpRequest::Upload {
                        remote: tc_ssh::sftp::join(&cwd, &name),
                        local,
                        ascii,
                    });
                }
            }
            LocalAction::Download(remote, dir) => {
                let ascii = matches!(&tab.sftp, crate::tab::SftpStatus::Ready(v) if v.ascii);
                for path in remote {
                    let name = path.rsplit('/').next().unwrap_or_default().to_string();
                    if name.is_empty() {
                        continue;
                    }
                    tab.sftp_op(tc_ssh::SftpRequest::Download {
                        remote: path,
                        local: dir.join(name),
                        ascii,
                    });
                }
            }
        }
    }
}
