//! Drawing reference dialogs from their layout (`tc_core::layout`): tab
//! headers, frames, group boxes, bound labels, list boxes and greyed-out
//! controls, each in its reference rectangle.

use super::NOT_YET;
use egui::{Pos2, Rect, vec2};
use tc_core::layout::{Ctl, Kind};

/// Height of a page control's tab header.
pub(crate) const HEADER_H: f32 = 24.0;

/// The reference dialogs' small font, so captions fit their controls.
pub(crate) fn compact(ui: &mut egui::Ui) {
    use egui::{FontId, TextStyle};
    let style = ui.style_mut();
    for (text_style, size) in [
        (TextStyle::Body, 12.0),
        (TextStyle::Button, 12.0),
        (TextStyle::Small, 10.0),
    ] {
        style
            .text_styles
            .insert(text_style, FontId::proportional(size));
    }
    style.spacing.interact_size.y = 20.0;
}

pub(crate) fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.fonts_mut(|f| {
        f.layout_no_wrap(
            text.to_string(),
            egui::TextStyle::Button.resolve(ui.style()),
            egui::Color32::WHITE,
        )
        .size()
        .x
    })
}

/// A page control's tab header row.
pub(crate) fn header(ui: &mut egui::Ui, top: Pos2, tabs: &[(&str, bool)]) {
    let mut x = top.x + 4.0;
    for (label, selected) in tabs {
        let width = text_width(ui, label) + 18.0;
        let rect = Rect::from_min_size(Pos2::new(x, top.y), vec2(width, HEADER_H - 2.0));
        ui.put(rect, egui::Button::selectable(*selected, *label));
        x += width + 2.0;
    }
}

pub(crate) fn frame(ui: &egui::Ui, rect: Rect) {
    let v = ui.visuals();
    ui.painter().rect(
        rect,
        2.0,
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
}

/// A reference control without a TermCraft setting, greyed out.
pub(crate) fn placeholder(ui: &mut egui::Ui, c: &Ctl, rect: Rect) {
    let caption = c.caption.trim();
    let response = ui
        .add_enabled_ui(false, |ui| match c.kind {
            Kind::Check => ui.put(rect, egui::Checkbox::new(&mut false, caption)),
            Kind::Combo | Kind::Edit | Kind::Spin | Kind::File | Kind::Folder | Kind::Memo => {
                if !caption.is_empty() {
                    bound_label(ui, rect, caption);
                }
                ui.put(rect, egui::TextEdit::singleline(&mut String::new()))
            }
            _ => ui.allocate_rect(rect, egui::Sense::hover()),
        })
        .inner;
    response.on_disabled_hover_text(NOT_YET);
}

/// A list box (the SSH algorithm lists): caption above, its items in
/// order. TermCraft uses its own secure algorithm choice, so it is shown
/// only.
pub(crate) fn list(ui: &mut egui::Ui, rect: Rect, c: &Ctl) {
    let v = ui.visuals().clone();
    ui.painter().text(
        rect.left_top() - vec2(0.0, 4.0),
        egui::Align2::LEFT_BOTTOM,
        c.caption.trim(),
        egui::TextStyle::Body.resolve(ui.style()),
        v.weak_text_color(),
    );
    ui.painter().rect(
        rect,
        2.0,
        v.extreme_bg_color,
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(3.0)));
    child.disable();
    egui::ScrollArea::vertical()
        .id_salt(("list", c.name))
        .max_height(rect.height() - 6.0)
        .show(&mut child, |ui| {
            for item in c.items {
                if item.starts_with('=') {
                    ui.separator();
                } else {
                    ui.label(egui::RichText::new(*item).small());
                }
            }
        });
    let _ = ui
        .interact(
            rect,
            egui::Id::new(("list-hover", c.name)),
            egui::Sense::hover(),
        )
        .on_hover_text(NOT_YET);
}

/// A group box: a frame with its caption on the top border.
pub(crate) fn group(ui: &egui::Ui, rect: Rect, caption: &str) {
    let v = ui.visuals();
    let stroke = egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color);
    let frame = Rect::from_min_max(rect.min + vec2(0.0, 8.0), rect.max);
    ui.painter()
        .rect_stroke(frame, 3.0, stroke, egui::StrokeKind::Inside);
    if !caption.is_empty() {
        let galley = ui.painter().layout_no_wrap(
            caption.to_string(),
            egui::TextStyle::Small.resolve(ui.style()),
            v.text_color(),
        );
        let at = rect.min + vec2(10.0, 1.0);
        let back = Rect::from_min_size(at - vec2(3.0, 0.0), galley.size() + vec2(6.0, 0.0));
        ui.painter().rect_filled(back, 0.0, v.panel_fill);
        ui.painter().galley(at, galley, v.text_color());
    }
}

/// The reference's bound label: left of the control, right-aligned.
pub(crate) fn bound_label(ui: &egui::Ui, rect: Rect, caption: &str) {
    ui.painter().text(
        Pos2::new(rect.left() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        caption.trim(),
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
    );
}
