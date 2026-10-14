//! Tab strip above the terminal (reference `Cts1` chrome tabs) with the
//! per-tab context menu (`PopupMenu1`) and "Show tab list" on the right.

use super::{Action, icon, item};
use crate::skin;
use tc_commands::Command;
use tc_core::settings::ViewSettings;

pub struct TabView {
    pub id: u64,
    pub title: String,
    pub alive: bool,
    /// Session icon glyph.
    pub icon: &'static str,
}

const TAB_HEIGHT: f32 = 28.0;
const TAB_MIN_WIDTH: f32 = 96.0;
const TAB_MAX_WIDTH: f32 = 220.0;

pub fn show(
    ui: &mut egui::Ui,
    tabs: &[TabView],
    active: Option<u64>,
    view: &ViewSettings,
    out: &mut Vec<Action>,
) {
    let palette = skin::current(ui.ctx());
    egui::Frame::new()
        .fill(palette.chrome)
        .inner_margin(egui::Margin {
            left: 4,
            right: 4,
            top: 4,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                let right_width = 32.0;
                let width = (ui.available_width() - right_width).max(0.0);
                ui.allocate_ui(egui::vec2(width, TAB_HEIGHT), |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("tab-strip")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (i, tab) in tabs.iter().enumerate() {
                                    let selected = active == Some(tab.id);
                                    chrome_tab(ui, i, tab, tabs, selected, view, &palette, out);
                                }
                                let new = ui
                                    .add(egui::Button::new(icon::PLUS).frame(false))
                                    .on_hover_text("New terminal");
                                if new.clicked() {
                                    out.push(Action::Run(Command::TabOpen { session: None }));
                                }
                            });
                        });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    tab_list(ui, tabs, active, out);
                });
            });
        });
    // The active tab merges into this line.
    let rect = ui.cursor();
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        egui::Stroke::new(1.0, palette.border),
    );
}

fn tab_list(ui: &mut egui::Ui, tabs: &[TabView], active: Option<u64>, out: &mut Vec<Action>) {
    let list = ui
        .add(egui::Button::new(icon::LIST).frame(false))
        .on_hover_text("Show tab list");
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
}

/// One tab: rounded top corners, accent line on the active tab, close
/// button on the right. Middle click closes it, like in the reference.
#[allow(clippy::too_many_arguments)]
fn chrome_tab(
    ui: &mut egui::Ui,
    index: usize,
    tab: &TabView,
    all: &[TabView],
    selected: bool,
    view: &ViewSettings,
    palette: &skin::Palette,
    out: &mut Vec<Action>,
) {
    let mut title = String::new();
    if view.show_tab_numbers {
        title.push_str(&format!("{}. ", index + 1));
    }
    title.push_str(&tab.title);
    if !tab.alive {
        title.push_str(" (closed)");
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_color = if selected {
        palette.text
    } else {
        palette.weak_text
    };
    let close_w = if view.show_close_button { 20.0 } else { 0.0 };
    let icon_w = 20.0;
    let pad = 10.0;
    let full = ui
        .painter()
        .layout_no_wrap(title.clone(), font.clone(), text_color);
    let width = (pad * 2.0 + icon_w + full.size().x + close_w).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, TAB_HEIGHT), egui::Sense::click());
    let response = response.on_hover_text(&tab.title);

    let fill = if selected {
        palette.tab_active
    } else if response.hovered() {
        palette.tab_hover
    } else {
        palette.tab_inactive
    };
    let radius = if view.rounded_tabs { 7 } else { 0 };
    let corners = egui::CornerRadius {
        nw: radius,
        ne: radius,
        sw: 0,
        se: 0,
    };
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, corners, fill);
    if selected {
        let top = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.0));
        painter.rect_filled(
            top.shrink2(egui::vec2(f32::from(radius) / 2.0, 0.0)),
            1,
            palette.accent,
        );
    }

    // Icon, then the title cut to the room left.
    let mut x = rect.left() + pad;
    let dot = if tab.alive { palette.ok } else { palette.warn };
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        tab.icon,
        font.clone(),
        if selected { palette.accent } else { dot },
    );
    x += icon_w;
    let room = (rect.right() - pad - close_w - x).max(0.0);
    let mut job = egui::text::LayoutJob::single_section(
        title,
        egui::TextFormat::simple(font.clone(), text_color),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: room,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    painter.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        text_color,
    );

    let mut close_clicked = false;
    if view.show_close_button {
        let center = egui::pos2(rect.right() - pad - 4.0, rect.center().y);
        let close_rect = egui::Rect::from_center_size(center, egui::vec2(16.0, 16.0));
        let close = ui
            .interact(close_rect, response.id.with("close"), egui::Sense::click())
            .on_hover_text("Close tab");
        if close.hovered() {
            painter.circle_filled(center, 8.0, palette.border);
        }
        if selected || response.hovered() || close.hovered() {
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                icon::X,
                egui::FontId::proportional(12.0),
                palette.text,
            );
        }
        close_clicked = close.clicked();
    }

    if close_clicked || response.middle_clicked() {
        out.push(Action::Run(Command::TabClose { tab: Some(tab.id) }));
    } else if response.clicked() {
        out.push(Action::SelectTab(tab.id));
    }
    response.context_menu(|ui| tab_menu(ui, index, tab, all, out));
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
