//! Tab strip above the terminal (reference `Cts1` chrome tabs) with the
//! per-tab context menu (`PopupMenu1`) and "Show tab list" on the right.

use super::{Action, icon, item};
use tc_commands::Command;
use tc_core::settings::ViewSettings;

pub struct TabView {
    pub id: u64,
    pub title: String,
    pub alive: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    tabs: &[TabView],
    active: Option<u64>,
    view: &ViewSettings,
    out: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let right_width = 60.0;
        let width = (ui.available_width() - right_width).max(0.0);
        ui.allocate_ui(egui::vec2(width, ui.available_height()), |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("tab-strip")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (i, tab) in tabs.iter().enumerate() {
                            tab_button(ui, i, tab, tabs, active == Some(tab.id), view, out);
                        }
                        let new = ui.small_button(icon::PLUS).on_hover_text("New terminal");
                        if new.clicked() {
                            out.push(Action::Run(Command::TabOpen { session: None }));
                        }
                    });
                });
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let list = ui.small_button(icon::LIST).on_hover_text("Show tab list");
            egui::Popup::menu(&list).show(|ui| {
                if tabs.is_empty() {
                    ui.weak("No open tabs");
                }
                for tab in tabs {
                    if ui
                        .add(egui::Button::new(&tab.title).selected(active == Some(tab.id)))
                        .clicked()
                    {
                        out.push(Action::SelectTab(tab.id));
                        ui.close();
                    }
                }
            });
        });
    });
}

fn tab_button(
    ui: &mut egui::Ui,
    index: usize,
    tab: &TabView,
    all: &[TabView],
    selected: bool,
    view: &ViewSettings,
    out: &mut Vec<Action>,
) {
    let mut label = String::new();
    if view.show_tab_numbers {
        label.push_str(&format!("{}. ", index + 1));
    }
    label.push_str(&format!("{}  {}", icon::TERMINAL_WINDOW, tab.title));
    if !tab.alive {
        label.push_str(" (closed)");
    }
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(6, 2))
        .corner_radius(egui::CornerRadius {
            nw: 6,
            ne: 6,
            sw: 0,
            se: 0,
        })
        .fill(if selected {
            ui.visuals().extreme_bg_color
        } else {
            ui.visuals().faint_bg_color
        });
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            let response = ui.selectable_label(selected, label);
            if response.clicked() {
                out.push(Action::SelectTab(tab.id));
            }
            response.context_menu(|ui| tab_menu(ui, index, tab, all, out));
            if view.show_close_button
                && ui
                    .small_button(icon::X)
                    .on_hover_text("Close tab")
                    .clicked()
            {
                out.push(Action::Run(Command::TabClose { tab: Some(tab.id) }));
            }
        });
    });
}

/// Reference `PopupMenu1`, visible entries.
fn tab_menu(
    ui: &mut egui::Ui,
    index: usize,
    tab: &TabView,
    all: &[TabView],
    out: &mut Vec<Action>,
) {
    let ids = |f: &dyn Fn(usize, &TabView) -> bool| -> Vec<u64> {
        all.iter()
            .enumerate()
            .filter(|(i, t)| f(*i, t))
            .map(|(_, t)| t.id)
            .collect()
    };
    let some = |v: Vec<u64>| (!v.is_empty()).then_some(Action::CloseTabs(v));

    item(ui, "Rename tab", Some(Action::RenameTab(tab.id)), out);
    item(ui, "Set tab color", None, out);
    item(
        ui,
        "Duplicate tab",
        Some(Action::Run(Command::TabDuplicate { tab: Some(tab.id) })),
        out,
    );
    item(
        ui,
        "Close tab",
        Some(Action::Run(Command::TabClose { tab: Some(tab.id) })),
        out,
    );
    ui.separator();
    item(
        ui,
        "Close all tabs to the left",
        some(ids(&|i, _| i < index)),
        out,
    );
    item(
        ui,
        "Close all tabs to the right",
        some(ids(&|i, _| i > index)),
        out,
    );
    item(
        ui,
        "Close all inactive tabs",
        some(ids(&|_, t| !t.alive)),
        out,
    );
    item(
        ui,
        "Close all except this tab",
        some(ids(&|i, _| i != index)),
        out,
    );
    item(ui, "Close all tabs", some(ids(&|_, _| true)), out);
    ui.separator();
    item(ui, "Detach tab", None, out);
    item(ui, "Fullscreen", Some(Action::ToggleFullscreen), out);
    item(ui, "Pin/unpin this tab", None, out);
    ui.separator();
    item(ui, "Save terminal output", None, out);
    item(ui, "Print terminal output", None, out);
    item(ui, "Increase font size", Some(Action::ZoomIn), out);
    item(ui, "Decrease font size", Some(Action::ZoomOut), out);
}
