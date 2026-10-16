//! Jump hosts configuration (reference `TFORMJUMPHOSTS`, 727×433): one
//! row per SSH gateway, reached in order before the end server; add,
//! remove and reorder rows, then OK.

use super::icon;
use tc_core::jump_hosts::JumpHost;

pub enum Outcome {
    Pending,
    Cancel,
    Done(Vec<JumpHost>),
}

pub struct JumpHostsDialog {
    rows: Vec<JumpHost>,
    selected: usize,
}

impl JumpHostsDialog {
    pub fn new(hops: Vec<JumpHost>) -> Self {
        let rows = if hops.is_empty() {
            vec![JumpHost::default()]
        } else {
            hops
        };
        Self { rows, selected: 0 }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Pending;
        let mut open = true;
        egui::Window::new("Jump hosts configuration")
            .id(egui::Id::new("jump-hosts"))
            .open(&mut open)
            // Above the "Session settings" window it was opened from.
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(727.0)
            .show(ctx, |ui| {
                let enter = crate::ui::EnterWatch::start(ui);
                ui.vertical_centered(|ui| {
                    ui.heading("Define one or several SSH jump hosts");
                    ui.label(
                        "(Jump through one or several SSH servers in order to reach your \
                         end-server)",
                    );
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(657.0);
                        ui.vertical(|ui| {
                            egui::ScrollArea::vertical()
                                .max_height(180.0)
                                .min_scrolled_height(180.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| self.rows_ui(ui));
                        });
                    });
                    ui.vertical(|ui| self.side_buttons(ui));
                });
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    if ui
                        .button(format!("{}  Add another jump host", icon::PLUS))
                        .clicked()
                    {
                        self.rows.push(JumpHost::default());
                        self.selected = self.rows.len() - 1;
                    }
                    ui.add_space(10.0);
                    if ui
                        .add(egui::Button::new("OK").min_size(egui::vec2(89.0, 30.0)))
                        .clicked()
                        || enter.pressed(ui)
                    {
                        outcome = Outcome::Done(std::mem::take(&mut self.rows));
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    fn rows_ui(&mut self, ui: &mut egui::Ui) {
        let Self { rows, selected } = self;
        for (i, row) in rows.iter_mut().enumerate() {
            // A row is selected (for move / remove) with its radio button
            // or by editing one of its fields.
            let mut focused = false;
            ui.horizontal(|ui| {
                ui.radio_value(
                    selected,
                    i,
                    egui::RichText::new(format!("{}", i + 1)).strong(),
                );
                ui.label(icon::ARROW_RIGHT);
                ui.label("SSH gateway host");
                focused |= ui
                    .add(
                        egui::TextEdit::singleline(&mut row.host)
                            .hint_text("host")
                            .desired_width(120.0),
                    )
                    .has_focus();
                ui.label("Username");
                focused |= ui
                    .add(egui::TextEdit::singleline(&mut row.user).desired_width(80.0))
                    .has_focus();
                ui.label("Port");
                focused |= ui
                    .add(
                        egui::TextEdit::singleline(&mut row.port)
                            .hint_text("22")
                            .desired_width(40.0),
                    )
                    .has_focus();
            });
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                ui.label(icon::KEY);
                ui.label("Use SSH key");
                focused |= ui
                    .add(
                        egui::TextEdit::singleline(&mut row.key)
                            .hint_text("path to private key (optional)")
                            .desired_width(380.0),
                    )
                    .has_focus();
            });
            if focused {
                *selected = i;
            }
            ui.separator();
        }
    }

    fn side_buttons(&mut self, ui: &mut egui::Ui) {
        let n = self.rows.len();
        let i = self.selected.min(n.saturating_sub(1));
        ui.add_space(16.0);
        if ui
            .add_enabled(i > 0, egui::Button::new(icon::ARROW_UP))
            .on_hover_text("Move up")
            .clicked()
        {
            self.rows.swap(i, i - 1);
            self.selected = i - 1;
        }
        if ui
            .add_enabled(n > 1, egui::Button::new(icon::TRASH))
            .on_hover_text("Remove selected SSH jump host")
            .clicked()
        {
            self.rows.remove(i);
            self.selected = i.min(self.rows.len() - 1);
        }
        if ui
            .add_enabled(i + 1 < n, egui::Button::new(icon::ARROW_DOWN))
            .on_hover_text("Move down")
            .clicked()
        {
            self.rows.swap(i, i + 1);
            self.selected = i + 1;
        }
    }
}
