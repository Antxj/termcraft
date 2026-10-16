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
    /// "Set tab color".
    pub color: Option<[u8; 3]>,
    pub pinned: bool,
    /// New output while the tab was in the background.
    pub activity: bool,
    /// Shown in its own window.
    pub detached: bool,
    /// A remote desktop (built-in RDP) is shown.
    pub rdp: bool,
}

/// Colours offered by "Set tab color".
const TAB_COLORS: &[(&str, [u8; 3])] = &[
    ("Red", [0xd7, 0x3a, 0x3a]),
    ("Orange", [0xe0, 0x86, 0x1e]),
    ("Yellow", [0xd6, 0xb1, 0x1a]),
    ("Green", [0x2e, 0xa0, 0x43]),
    ("Teal", [0x1f, 0x9e, 0x9e]),
    ("Blue", [0x2f, 0x7f, 0xd8]),
    ("Purple", [0x89, 0x57, 0xe5]),
    ("Grey", [0x7d, 0x85, 0x90]),
];

const TAB_HEIGHT: f32 = 28.0;
const TAB_MIN_WIDTH: f32 = 96.0;
const TAB_MAX_WIDTH: f32 = 220.0;

/// Width of the "Quick connect" box shown here while the sidebar is hidden.
const QUICK_CONNECT_WIDTH: f32 = 180.0;

/// The Home tab: pinned before the terminal tabs, and what "+" opens.
#[derive(Debug, Clone, Copy, Default)]
pub struct Home {
    pub pinned: bool,
    pub on_plus: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    tabs: &[TabView],
    active: Option<u64>,
    view: &ViewSettings,
    home: Home,
    quick_connect: Option<&mut String>,
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
                let right_width = 32.0
                    + if quick_connect.is_some() {
                        QUICK_CONNECT_WIDTH + 8.0
                    } else {
                        0.0
                    };
                let width = (ui.available_width() - right_width).max(0.0);
                ui.allocate_ui(egui::vec2(width, TAB_HEIGHT), |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("tab-strip")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if home.pinned && !tabs.is_empty() {
                                    let shown = active.is_none();
                                    let button = ui
                                        .add(egui::Button::selectable(shown, icon::HOUSE))
                                        .on_hover_text("Home");
                                    if button.clicked() {
                                        out.push(Action::ShowHome);
                                    }
                                }
                                for (i, tab) in tabs.iter().enumerate() {
                                    let selected = active == Some(tab.id);
                                    chrome_tab(ui, i, tab, tabs, selected, view, &palette, out);
                                }
                                let hint = if home.on_plus { "Home" } else { "New terminal" };
                                let new = ui
                                    .add(egui::Button::new(icon::PLUS).frame(false))
                                    .on_hover_text(hint);
                                if new.clicked() {
                                    out.push(if home.on_plus {
                                        Action::ShowHome
                                    } else {
                                        Action::Run(Command::TabOpen { session: None })
                                    });
                                }
                            });
                        });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    tab_list(ui, tabs, active, view.compact, out);
                    if let Some(text) = quick_connect {
                        super::sidebar::quick_connect_box(ui, text, QUICK_CONNECT_WIDTH, out);
                    }
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

fn tab_list(
    ui: &mut egui::Ui,
    tabs: &[TabView],
    active: Option<u64>,
    compact: bool,
    out: &mut Vec<Action>,
) {
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
        if compact {
            ui.separator();
            if ui.button("Exit compact mode").clicked() {
                out.push(Action::ToggleCompact);
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
    if tab.detached {
        title.push_str(" (detached)");
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_color = if selected {
        palette.text
    } else {
        palette.weak_text
    };
    let show_close = view.show_close_button && !tab.pinned;
    let close_w = if show_close || tab.pinned { 20.0 } else { 0.0 };
    let icon_w = 20.0;
    let pad = 10.0;
    let full = ui
        .painter()
        .layout_no_wrap(title.clone(), font.clone(), text_color);
    let width = (pad * 2.0 + icon_w + full.size().x + close_w).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, TAB_HEIGHT), egui::Sense::click());
    let response = response.on_hover_text(&tab.title);

    let base = if selected {
        palette.tab_active
    } else if response.hovered() {
        palette.tab_hover
    } else {
        palette.tab_inactive
    };
    // A tab colour tints the whole tab, more on the active one.
    let fill = match tab.color {
        Some([r, g, b]) => base.lerp_to_gamma(
            egui::Color32::from_rgb(r, g, b),
            if selected { 0.45 } else { 0.3 },
        ),
        None => base,
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

    // Background output: a small accent dot in the corner.
    if tab.activity && !selected {
        painter.circle_filled(
            egui::pos2(rect.left() + 6.0, rect.top() + 7.0),
            3.0,
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
    if tab.pinned {
        painter.text(
            egui::pos2(rect.right() - pad - 4.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon::PUSH_PIN,
            egui::FontId::proportional(12.0),
            palette.weak_text,
        );
    }
    if show_close {
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

    if close_clicked || (response.middle_clicked() && !tab.pinned) {
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
    // Pinned tabs stay when closing several.
    let ids = |f: &dyn Fn(usize, &TabView) -> bool| ids(&|i, t| !t.pinned && f(i, t));
    let some = |v: Vec<u64>| (!v.is_empty()).then_some(Action::CloseTabs(v));

    item(ui, "Rename tab", Some(Action::RenameTab(tab.id)), out);
    ui.menu_button("Set tab color", |ui| {
        for (name, rgb) in TAB_COLORS {
            let [r, g, b] = *rgb;
            let label = egui::RichText::new(format!("{}  {name}", icon::CIRCLE))
                .color(egui::Color32::from_rgb(r, g, b));
            if ui.button(label).clicked() {
                out.push(Action::SetTabColor(tab.id, Some(*rgb)));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Default color").clicked() {
            out.push(Action::SetTabColor(tab.id, None));
            ui.close();
        }
    });
    item(
        ui,
        "Duplicate tab",
        Some(Action::Run(Command::TabDuplicate { tab: Some(tab.id) })),
        out,
    );
    if tab.rdp {
        // The local system keeps Ctrl+Alt+Del for itself.
        item(
            ui,
            "Send Ctrl+Alt+Del",
            Some(Action::RdpCtrlAltDel(tab.id)),
            out,
        );
    }
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
    if tab.detached {
        item(
            ui,
            "Re-attach tab",
            Some(Action::DetachTab(tab.id, false)),
            out,
        );
    } else {
        item(ui, "Detach tab", Some(Action::DetachTab(tab.id, true)), out);
    }
    item(ui, "Fullscreen", Some(Action::ToggleFullscreen), out);
    item(
        ui,
        "Pin/unpin this tab",
        Some(Action::TogglePin(tab.id)),
        out,
    );
    ui.separator();
    item(
        ui,
        "Save terminal output",
        Some(Action::SaveTerminalText),
        out,
    );
    item(
        ui,
        "Print terminal output",
        Some(Action::PrintTerminalText),
        out,
    );
    item(ui, "Increase font size", Some(Action::ZoomIn), out);
    item(ui, "Decrease font size", Some(Action::ZoomOut), out);
}
