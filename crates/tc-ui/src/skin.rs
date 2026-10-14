//! Skins ("Skin selection" in Configuration › Display): Light (the
//! default, like the reference), Dim (between light and dark) and Dark.
//! A [`Palette`] holds the few colours the custom-painted parts (tab strip,
//! sidebar strip, toolbar) need; it is stored in the egui context.

use egui::{Color32, Visuals};
use tc_core::settings::Theme;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Toolbar and tab strip background.
    pub chrome: Color32,
    pub tab_active: Color32,
    pub tab_inactive: Color32,
    pub tab_hover: Color32,
    /// Selected tab marker, selection, links.
    pub accent: Color32,
    pub text: Color32,
    pub weak_text: Color32,
    pub border: Color32,
    pub panel: Color32,
    pub field: Color32,
    /// Green / orange dots next to open sessions.
    pub ok: Color32,
    pub warn: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const LIGHT: Palette = Palette {
    dark: false,
    chrome: rgb(0xE9, 0xED, 0xF2),
    tab_active: rgb(0xFF, 0xFF, 0xFF),
    tab_inactive: rgb(0xD5, 0xDC, 0xE5),
    tab_hover: rgb(0xE6, 0xEC, 0xF3),
    accent: rgb(0x1F, 0x6F, 0xC5),
    text: rgb(0x1E, 0x23, 0x2A),
    weak_text: rgb(0x5F, 0x68, 0x73),
    border: rgb(0xB8, 0xC2, 0xCE),
    panel: rgb(0xF4, 0xF6, 0xF9),
    field: rgb(0xFF, 0xFF, 0xFF),
    ok: rgb(0x2E, 0xA0, 0x43),
    warn: rgb(0xE0, 0x8A, 0x1E),
};

pub const DIM: Palette = Palette {
    dark: true,
    chrome: rgb(0x3B, 0x41, 0x4A),
    tab_active: rgb(0x4C, 0x53, 0x5E),
    tab_inactive: rgb(0x33, 0x38, 0x40),
    tab_hover: rgb(0x43, 0x49, 0x53),
    accent: rgb(0x5C, 0xA4, 0xEC),
    text: rgb(0xE6, 0xE9, 0xED),
    weak_text: rgb(0xA9, 0xB1, 0xBC),
    border: rgb(0x58, 0x60, 0x6C),
    panel: rgb(0x42, 0x48, 0x51),
    field: rgb(0x32, 0x37, 0x3E),
    ok: rgb(0x52, 0xC4, 0x6A),
    warn: rgb(0xF0, 0xA3, 0x45),
};

pub const DARK: Palette = Palette {
    dark: true,
    chrome: rgb(0x23, 0x25, 0x29),
    tab_active: rgb(0x33, 0x36, 0x3C),
    tab_inactive: rgb(0x1B, 0x1D, 0x20),
    tab_hover: rgb(0x2A, 0x2D, 0x32),
    accent: rgb(0x3D, 0x93, 0xF0),
    text: rgb(0xDA, 0xDD, 0xE1),
    weak_text: rgb(0x95, 0x9C, 0xA6),
    border: rgb(0x3E, 0x42, 0x49),
    panel: rgb(0x26, 0x28, 0x2C),
    field: rgb(0x16, 0x17, 0x1A),
    ok: rgb(0x4C, 0xBB, 0x63),
    warn: rgb(0xE8, 0x9B, 0x3C),
};

/// The palette for a setting; `System` follows the OS light / dark mode.
pub fn palette(theme: Theme, system_dark: bool) -> Palette {
    match theme {
        Theme::Light => LIGHT,
        Theme::Dim => DIM,
        Theme::Dark => DARK,
        Theme::System if system_dark => DARK,
        Theme::System => LIGHT,
    }
}

fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.tab_hover;
    v.code_bg_color = p.field;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.75 } else { 0.9 });
    v.selection.stroke.color = if p.dark { p.text } else { Color32::WHITE };
    v.window_stroke.color = p.border;
    v.widgets.noninteractive.bg_fill = p.panel;
    v.widgets.noninteractive.weak_bg_fill = p.panel;
    v.widgets.noninteractive.bg_stroke.color = p.border;
    v.widgets.noninteractive.fg_stroke.color = p.text;
    v.widgets.inactive.weak_bg_fill = p.tab_inactive;
    v.widgets.inactive.bg_fill = p.tab_inactive;
    v.widgets.inactive.fg_stroke.color = p.text;
    v.widgets.hovered.weak_bg_fill = p.tab_hover;
    v.widgets.hovered.bg_fill = p.tab_hover;
    v.widgets.hovered.bg_stroke.color = p.accent;
    v.widgets.active.bg_stroke.color = p.accent;
    v.widgets.open.weak_bg_fill = p.tab_hover;
    v
}

fn palette_id() -> egui::Id {
    egui::Id::new("termcraft-skin")
}

/// Apply a skin (cheap to call: does nothing when it is already applied).
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let system_dark = ctx.system_theme() == Some(egui::Theme::Dark);
    let p = palette(theme, system_dark);
    if ctx.data(|d| d.get_temp::<Palette>(palette_id())) == Some(p) {
        return;
    }
    let egui_theme = if p.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_visuals_of(egui_theme, visuals(&p));
    ctx.set_theme(egui_theme);
    ctx.data_mut(|d| d.insert_temp(palette_id(), p));
}

/// The palette applied last (Light before the first `apply`).
pub fn current(ctx: &egui::Context) -> Palette {
    ctx.data(|d| d.get_temp::<Palette>(palette_id()))
        .unwrap_or(LIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio between two colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let ch = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_is_readable_in_every_skin() {
        for p in [LIGHT, DIM, DARK] {
            for bg in [p.panel, p.chrome, p.tab_active, p.tab_inactive, p.field] {
                assert!(contrast(p.text, bg) >= 4.5, "{p:?} on {bg:?}");
            }
            assert!(contrast(p.weak_text, p.panel) >= 3.0, "{p:?}");
        }
    }

    #[test]
    fn system_follows_os() {
        assert_eq!(palette(Theme::System, true), DARK);
        assert_eq!(palette(Theme::System, false), LIGHT);
        assert_eq!(palette(Theme::Dim, false), DIM);
    }
}
