//! Big-button toolbar (reference panel `sPanel4`, buttons ~50×54 with icon
//! over caption). Left group: Session … Help; right group: X server, Exit.

use super::{Action, NOT_YET, icon, menus};
use egui::{Align2, FontId, Sense, Vec2, vec2};
use tc_core::Session;
use tc_core::settings::{ToolbarStyle, ViewSettings};

enum Kind {
    Action(Option<Action>),
    Popup(fn(&mut egui::Ui, &PopupCtx<'_>, &mut Vec<Action>)),
}

struct PopupCtx<'a> {
    view: &'a ViewSettings,
    sessions: &'a [Session],
}

fn metrics(style: ToolbarStyle) -> (Vec2, f32, bool) {
    match style {
        ToolbarStyle::Small => (vec2(30.0, 28.0), 16.0, false),
        ToolbarStyle::Standard => (vec2(42.0, 40.0), 24.0, false),
        ToolbarStyle::StandardWithCaptions => (vec2(54.0, 54.0), 24.0, true),
    }
}

pub fn height(style: ToolbarStyle) -> f32 {
    metrics(style).0.y + 4.0
}

fn button(
    ui: &mut egui::Ui,
    glyph: &str,
    caption: &str,
    hint: &str,
    enabled: bool,
    style: ToolbarStyle,
) -> egui::Response {
    let (size, icon_size, with_caption) = metrics(style);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let visuals = ui.style().interact(&response);
    if enabled && (response.hovered() || response.is_pointer_button_down_on()) {
        ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
    }
    let color = if enabled {
        visuals.text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let icon_center = if with_caption {
        rect.center_top() + vec2(0.0, 4.0 + icon_size / 2.0)
    } else {
        rect.center()
    };
    ui.painter().text(
        icon_center,
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(icon_size),
        color,
    );
    if with_caption {
        ui.painter().text(
            rect.center_bottom() - vec2(0.0, 6.0),
            Align2::CENTER_BOTTOM,
            caption,
            FontId::proportional(11.5),
            color,
        );
    }
    let tip = if enabled {
        hint.to_string()
    } else {
        format!("{hint}\n{NOT_YET}")
    };
    response.on_hover_text(if with_caption {
        tip
    } else {
        format!("{caption}: {tip}")
    })
}

pub fn show(ui: &mut egui::Ui, view: &ViewSettings, sessions: &[Session], out: &mut Vec<Action>) {
    let style = view.toolbar_style;
    let cx = PopupCtx { view, sessions };
    let left: [(&str, &str, &str, Kind); 12] = [
        (
            icon::MONITOR_PLAY,
            "Session",
            "Start a new remote session",
            Kind::Action(Some(Action::NewSession)),
        ),
        (
            icon::HARD_DRIVES,
            "Servers",
            "Network services",
            Kind::Action(None),
        ),
        (
            icon::WRENCH,
            "Tools",
            "Start an embedded program",
            Kind::Popup(|ui, _, out| menus::tools_menu(ui, out)),
        ),
        (
            icon::GAME_CONTROLLER,
            "Games",
            "Start a terminal game",
            Kind::Popup(|ui, _, out| menus::games_menu(ui, out)),
        ),
        (
            icon::STAR,
            "Sessions",
            "Start a previously saved session",
            Kind::Popup(|ui, cx, out| menus::sessions_menu(ui, cx.sessions, out)),
        ),
        (
            icon::SQUARES_FOUR,
            "View",
            "Toolbars, menus and buttons options",
            Kind::Popup(|ui, cx, out| menus::view_menu(ui, cx.view, out)),
        ),
        (
            icon::SQUARE_SPLIT_HORIZONTAL,
            "Split",
            "Terminal splitting options",
            Kind::Popup(|ui, _, out| menus::split_items(ui, out)),
        ),
        (
            icon::BROADCAST,
            "MultiExec",
            "Send commands to all terminals at once",
            Kind::Action(None),
        ),
        (
            icon::ARROWS_LEFT_RIGHT,
            "Tunneling",
            "Graphical SSH port forwarding builder",
            Kind::Action(None),
        ),
        (
            icon::PACKAGE,
            "Packages",
            "Package manager",
            Kind::Action(None),
        ),
        (
            icon::GEAR,
            "Settings",
            "Edit TermCraft configuration",
            Kind::Action(None),
        ),
        (
            icon::QUESTION,
            "Help",
            "Show TermCraft help",
            Kind::Popup(|ui, _, out| menus::help_menu(ui, out)),
        ),
    ];
    let right: [(&str, &str, &str, Kind); 2] = [
        (
            icon::DESKTOP,
            "X server",
            "X server is stopped",
            Kind::Action(None),
        ),
        (
            icon::POWER,
            "Exit",
            "Exit TermCraft",
            Kind::Action(Some(Action::Quit)),
        ),
    ];

    let mut draw =
        |ui: &mut egui::Ui, (glyph, caption, hint, kind): (&str, &str, &str, Kind)| match kind {
            Kind::Action(action) => {
                let enabled = action.is_some();
                if button(ui, glyph, caption, hint, enabled, style).clicked()
                    && let Some(a) = action
                {
                    out.push(a);
                }
            }
            Kind::Popup(content) => {
                let response = button(ui, glyph, caption, hint, true, style);
                egui::Popup::menu(&response).show(|ui| content(ui, &cx, out));
            }
        };

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        for b in left {
            draw(ui, b);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for b in right.into_iter().rev() {
                draw(ui, b);
            }
        });
    });
}
