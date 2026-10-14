//! Terminal fonts: the built-in one (Hack) or an installed monospace font,
//! regular or bold. Installed fonts are indexed on a background thread the
//! first time they are needed; until then (and when a font is missing, as
//! MobaXterm's own "MobaFont" in an imported session) the built-in font is
//! used.

use crate::system_fonts::{self, Index};
use egui::{FontData, FontDefinitions, FontFamily};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// What the font choice shows for the built-in font.
pub const BUILT_IN: &str = "Hack (built-in)";

#[derive(Default)]
struct State {
    /// The app's fonts, extended with the terminal fonts loaded so far.
    defs: Option<FontDefinitions>,
    index: Option<Arc<Index>>,
    scanning: bool,
    /// Family key → loaded (true) or not available (false).
    loaded: HashMap<String, bool>,
    /// Keys whose fonts are in `defs` but maybe not yet in the context.
    pending: HashSet<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}

/// The app's font set (call once, after `set_fonts`).
pub fn init(defs: FontDefinitions) {
    if let Ok(mut s) = state().lock() {
        s.defs = Some(defs);
    }
}

fn is_built_in(name: &str) -> bool {
    let n = name.trim();
    n.is_empty() || n == BUILT_IN || n.eq_ignore_ascii_case("hack")
}

/// Start indexing the installed fonts; true once they are known.
fn ensure_index(ctx: &egui::Context, s: &mut State) -> bool {
    if s.index.is_some() {
        return true;
    }
    if !s.scanning {
        s.scanning = true;
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("font-scan".into())
            .spawn(move || {
                let index = Arc::new(system_fonts::scan(&system_fonts::font_dirs()));
                if let Ok(mut s) = state().lock() {
                    s.index = Some(index);
                    s.scanning = false;
                }
                ctx.request_repaint();
            });
    }
    false
}

/// Installed monospace families (`None` while they are being indexed).
pub fn monospace_families(ctx: &egui::Context) -> Option<Vec<String>> {
    let mut s = state().lock().ok()?;
    if !ensure_index(ctx, &mut s) {
        return None;
    }
    s.index.as_ref().map(|i| i.monospace())
}

/// The font family to draw a terminal with.
pub fn family(ctx: &egui::Context, name: &str, bold: bool) -> FontFamily {
    if is_built_in(name) {
        return FontFamily::Monospace;
    }
    let key = format!(
        "term:{}:{}",
        name.trim(),
        if bold { "bold" } else { "regular" }
    );
    let Ok(mut s) = state().lock() else {
        return FontFamily::Monospace;
    };
    let wanted = FontFamily::Name(key.as_str().into());
    match s.loaded.get(&key) {
        Some(false) => return FontFamily::Monospace,
        Some(true) => {
            // Usable once the context has the new font set (next frame).
            if s.pending.contains(&key) {
                if ctx.fonts(|f| f.families().contains(&wanted)) {
                    s.pending.remove(&key);
                } else {
                    return FontFamily::Monospace;
                }
            }
            return wanted;
        }
        None => {}
    }
    if !ensure_index(ctx, &mut s) {
        return FontFamily::Monospace;
    }
    let face = s
        .index
        .as_ref()
        .and_then(|i| i.face(name.trim(), bold))
        .cloned();
    let bytes = face
        .as_ref()
        .and_then(|f| std::fs::read(&f.path).ok().map(|b| (b, f.index)));
    let Some((bytes, index)) = bytes else {
        s.loaded.insert(key, false);
        return FontFamily::Monospace;
    };
    let Some(defs) = s.defs.as_mut() else {
        return FontFamily::Monospace;
    };
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    defs.font_data.insert(key.clone(), Arc::new(data));
    // Missing glyphs come from the built-in fonts.
    let mut chain = vec![key.clone()];
    chain.extend(
        defs.families
            .get(&FontFamily::Monospace)
            .cloned()
            .unwrap_or_default(),
    );
    defs.families.insert(wanted, chain);
    ctx.set_fonts(defs.clone());
    s.loaded.insert(key.clone(), true);
    s.pending.insert(key);
    ctx.request_repaint();
    FontFamily::Monospace
}

/// The font choice: the built-in font, then the installed monospace
/// families. An empty `value` is the built-in font.
pub fn combo(ui: &mut egui::Ui, id: &str, value: &mut String) {
    let built_in = is_built_in(value);
    let shown = if built_in {
        BUILT_IN.to_string()
    } else {
        value.trim().to_string()
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(210.0)
        .height(320.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(built_in, BUILT_IN).clicked() {
                value.clear();
            }
            let Some(list) = monospace_families(ui.ctx()) else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Looking for installed fonts…");
                });
                return;
            };
            if !built_in && !list.iter().any(|n| n == value.trim()) {
                ui.add_enabled(false, egui::Button::selectable(true, value.trim()))
                    .on_disabled_hover_text("Not installed here: the built-in font is used");
            }
            for name in list {
                if ui.selectable_label(value.trim() == name, &name).clicked() {
                    *value = name;
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_names() {
        assert!(is_built_in(""));
        assert!(is_built_in(BUILT_IN));
        assert!(is_built_in(" Hack "));
        assert!(!is_built_in("Consolas"));
    }

    #[test]
    fn unknown_fonts_fall_back_to_the_built_in_one() {
        let ctx = egui::Context::default();
        init(FontDefinitions::default());
        if let Ok(mut s) = state().lock() {
            s.index = Some(Arc::new(Index::default()));
        }
        assert_eq!(family(&ctx, "MobaFont", false), FontFamily::Monospace);
        assert_eq!(
            state().lock().unwrap().loaded.get("term:MobaFont:regular"),
            Some(&false),
            "remembered, not searched every frame"
        );
    }
}
