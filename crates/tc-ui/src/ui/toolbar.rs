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

/// The buttons of the left group, in the reference order (Configuration ›
/// Toolbar lists and orders them by caption).
pub const BUTTONS: [&str; 12] = [
    "Session",
    "Servers",
    "Tools",
    "Games",
    "Sessions",
    "View",
    "Split",
    "MultiExec",
    "Tunneling",
    "Packages",
    "Settings",
    "Help",
];

/// The toolbar buttons to show, in order.
pub fn chosen(view: &ViewSettings) -> Vec<&'static str> {
    if view.toolbar_buttons.is_empty() {
        return BUTTONS.to_vec();
    }
    view.toolbar_buttons
        .iter()
        .filter_map(|name| BUTTONS.iter().find(|b| **b == name).copied())
        .collect()
}

pub fn show(
    ui: &mut egui::Ui,
    view: &ViewSettings,
    sessions: &[Session],
    x_display: Option<u16>,
    out: &mut Vec<Action>,
) {
    let style = view.toolbar_style;
    let cx = PopupCtx { view, sessions };
    let left: [(&str, &str, &str, Kind); BUTTONS.len()] = [
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
            Kind::Action(Some(Action::Servers)),
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
            Kind::Popup(|ui, cx, out| menus::split_items(ui, cx.view.split, out)),
        ),
        (
            icon::BROADCAST,
            "MultiExec",
            "Send commands to all terminals at once",
            Kind::Action(Some(Action::ToggleMultiExec)),
        ),
        (
            icon::ARROWS_LEFT_RIGHT,
            "Tunneling",
            "Graphical SSH port forwarding builder",
            Kind::Action(Some(Action::Tunnels)),
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
            Kind::Action(Some(Action::Configuration)),
        ),
        (
            icon::QUESTION,
            "Help",
            "Show TermCraft help",
            Kind::Popup(|ui, _, out| menus::help_menu(ui, out)),
        ),
    ];
    let x_hint = match x_display {
        Some(d) => format!("X server is running on :{d} (click to stop it)"),
        None => "X server is stopped (click to start it)".to_string(),
    };
    let right: [(&str, &str, &str, Kind); 2] = [
        (
            icon::DESKTOP,
            "X server",
            x_hint.as_str(),
            Kind::Action(Some(Action::Run(match x_display {
                None => tc_commands::Command::XServerStart {},
                Some(_) => tc_commands::Command::XServerStop {},
            }))),
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

    let mut left: Vec<Option<(&str, &str, &str, Kind)>> = left.into_iter().map(Some).collect();
    let order = chosen(view);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        for name in order {
            if let Some(b) = left
                .iter_mut()
                .find(|b| b.as_ref().is_some_and(|b| b.1 == name))
                .and_then(Option::take)
            {
                draw(ui, b);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for b in right.into_iter().rev() {
                draw(ui, b);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chosen_buttons_keep_their_order() {
        let mut view = ViewSettings::default();
        assert_eq!(chosen(&view), BUTTONS.to_vec(), "empty = all");
        view.toolbar_buttons = vec!["Help".into(), "Session".into(), "Gone".into()];
        assert_eq!(
            chosen(&view),
            ["Help", "Session"],
            "unknown names are skipped"
        );
    }
}
