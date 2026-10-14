//! "Customize syntax highlighting" (reference `TFORMSYNTAX`, 1009×568):
//! 8 user syntax definitions, each a list of patterns per color attribute,
//! with samples, file import / export and a regex mode. Also maps a 4_36
//! index to the highlighter a terminal uses.

use std::sync::Arc;
use tc_core::syntax::{ATTRIBUTES, CustomSyntax, FIRST_CUSTOM_INDEX, SLOTS};
use tc_term::{CustomRules, Highlighter, Syntax};

type Slots = [CustomSyntax; SLOTS];

/// The highlighter for a 4_36 index: built-in (0…5) or a user syntax
/// (6…13). User syntaxes are compiled once and cached until they change.
pub fn highlighter(ctx: &egui::Context, index: usize, custom: &Slots) -> Highlighter {
    let Some(slot) = index.checked_sub(FIRST_CUSTOM_INDEX) else {
        return Highlighter::Builtin(Syntax::from_index(index));
    };
    let Some(def) = custom.get(slot).filter(|d| d.is_used()) else {
        return Highlighter::Builtin(Syntax::None);
    };
    let id = egui::Id::new(("custom-syntax", slot));
    type Cached = (CustomSyntax, Option<Arc<CustomRules>>);
    let cached: Option<Cached> = ctx.data(|d| d.get_temp(id));
    let rules = match cached {
        Some((source, rules)) if source == *def => rules,
        _ => {
            let rules = CustomRules::compile(&def.patterns, def.case_sensitive, def.regex)
                .ok()
                .map(Arc::new);
            ctx.data_mut(|d| d.insert_temp(id, (def.clone(), rules.clone())));
            rules
        }
    };
    rules.map_or(Highlighter::Builtin(Syntax::None), Highlighter::Custom)
}

/// The 4_36 choices: built-in names, then the used user syntaxes.
pub fn choices(custom: &Slots) -> Vec<(usize, String)> {
    let builtin = tc_core::schema::terminal_params()
        .iter()
        .find(|p| p.id == "4_36")
        .and_then(|p| match p.widget {
            tc_core::schema::Widget::Choice(items) => Some(items),
            _ => None,
        })
        .unwrap_or(&[]);
    builtin
        .iter()
        .enumerate()
        .map(|(i, s)| (i, (*s).to_string()))
        .chain(
            custom
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is_used())
                .map(|(i, c)| (FIRST_CUSTOM_INDEX + i, c.name.clone())),
        )
        .collect()
}

pub enum Outcome {
    Pending,
    Cancel,
    Done(Box<Slots>),
}

pub struct SyntaxWindow {
    slots: Box<Slots>,
    tab: usize,
    message: Option<(bool, String)>,
}

/// Text colors of the attribute labels (Underline and Blinking unchanged).
const LABEL_COLORS: [Option<[u8; 3]>; 8] = [
    None,
    Some([205, 49, 49]),
    Some([13, 188, 121]),
    Some([229, 229, 16]),
    Some([36, 114, 200]),
    Some([188, 63, 188]),
    Some([17, 168, 205]),
    None,
];

impl SyntaxWindow {
    pub fn new(slots: &Slots) -> Self {
        Self {
            slots: Box::new(slots.clone()),
            tab: 0,
            message: None,
        }
    }

    /// Snapshots: the first slot filled with a sample.
    pub fn preview() -> Self {
        let mut slots: Slots = Default::default();
        slots[0] = tc_core::syntax::samples().remove(6).1;
        Self::new(&slots)
    }

    fn check(def: &CustomSyntax) -> Result<(), String> {
        CustomRules::compile(&def.patterns, def.case_sensitive, def.regex).map(|_| ())
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Pending;
        let mut open = true;
        egui::Window::new("Customize syntax highlighting")
            .id(egui::Id::new("syntax-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(1009.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading(
                        "User highlighter settings: modify one of the 8 custom syntax definitions",
                    );
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    for i in 0..SLOTS {
                        let name = &self.slots[i].name;
                        let label = if name.trim().is_empty() {
                            format!("Custom syntax #{i}")
                        } else {
                            name.clone()
                        };
                        if ui.selectable_label(self.tab == i, label).clicked() {
                            self.tab = i;
                            self.message = None;
                        }
                    }
                });
                ui.separator();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(509.0);
                        self.definition(ui);
                    });
                    ui.add_space(12.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(429.0);
                            self.help(ui);
                        });
                    });
                });
                if let Some((ok, text)) = &self.message {
                    let color = if *ok {
                        ui.visuals().text_color()
                    } else {
                        ui.visuals().error_fg_color
                    };
                    ui.colored_label(color, text);
                }
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    if ui
                        .add(egui::Button::new("OK").min_size(egui::vec2(82.0, 30.0)))
                        .clicked()
                    {
                        match self
                            .slots
                            .iter()
                            .enumerate()
                            .find_map(|(i, d)| Self::check(d).err().map(|e| (i, e)))
                        {
                            Some((i, e)) => {
                                self.tab = i;
                                self.message = Some((false, e));
                            }
                            None => outcome = Outcome::Done(self.slots.clone()),
                        }
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    fn definition(&mut self, ui: &mut egui::Ui) {
        let tab = self.tab;
        let def = &mut self.slots[tab];
        ui.horizontal(|ui| {
            ui.label(format!("Name for custom syntax #{tab}:"));
            ui.add(
                egui::TextEdit::singleline(&mut def.name)
                    .desired_width(171.0)
                    .char_limit(64),
            );
        });
        ui.add_space(4.0);
        ui.label("Expressions corresponding to each color attribute:");
        egui::Grid::new(("syntax-patterns", tab))
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for (i, name) in ATTRIBUTES.iter().enumerate() {
                    let mut text = egui::RichText::new(*name);
                    if let Some([r, g, b]) = LABEL_COLORS[i] {
                        text = text.color(egui::Color32::from_rgb(r, g, b));
                    }
                    if i == 0 {
                        text = text.underline();
                    }
                    ui.label(text);
                    ui.add(
                        egui::TextEdit::singleline(&mut def.patterns[i])
                            .desired_width(410.0)
                            .char_limit(tc_core::syntax::MAX_PATTERNS_LEN),
                    );
                    ui.end_row();
                }
            });
        ui.checkbox(
            &mut def.case_sensitive,
            "Case-sensitive expressions matching",
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Remove this custom syntax").clicked() {
                *def = CustomSyntax::default();
                self.message = Some((true, format!("Custom syntax #{tab} removed.")));
            }
            if ui.button("Save modifications").clicked() {
                self.message = Some(match Self::check(def) {
                    Ok(()) => (true, "This syntax definition is valid.".into()),
                    Err(e) => (false, e),
                });
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Import syntax from file").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("TermCraft syntax", &["toml"])
                    .pick_file()
            {
                self.message = Some(
                    match std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|t| CustomSyntax::from_file(&t))
                    {
                        Ok(imported) => {
                            *def = imported;
                            (true, format!("Imported {}.", path.display()))
                        }
                        Err(e) => (false, format!("Cannot import {}: {e}", path.display())),
                    },
                );
            }
            if ui.button("Export syntax to file").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("TermCraft syntax", &["toml"])
                    .set_file_name(format!("{}.toml", def.name.trim()))
                    .save_file()
            {
                self.message = Some(match std::fs::write(&path, def.to_file()) {
                    Ok(()) => (true, format!("Exported to {}.", path.display())),
                    Err(e) => (false, format!("Cannot write {}: {e}", path.display())),
                });
            }
        });
    }

    fn help(&mut self, ui: &mut egui::Ui) {
        let def = &mut self.slots[self.tab];
        ui.strong("Custom syntax help and samples");
        ui.label(
            "Give each color a list of patterns separated by commas. A pattern that starts \
             or ends with a letter or digit matches whole words only. Highlighting applies to \
             text the program did not color itself.",
        );
        ui.label(
            "Regular expressions allow complex definitions; a wrong expression is reported \
             when you save.",
        );
        ui.checkbox(&mut def.regex, "Use regular expressions (experts only)");
        ui.add_space(6.0);
        ui.label("Sample syntax definitions:");
        for (label, sample) in tc_core::syntax::samples() {
            if ui
                .add(egui::Button::new(label).min_size(egui::vec2(411.0, 26.0)))
                .clicked()
            {
                *def = sample;
                self.message = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_map_to_builtin_and_user_syntaxes() {
        let ctx = egui::Context::default();
        let mut slots: Slots = Default::default();
        assert!(matches!(
            highlighter(&ctx, 1, &slots),
            Highlighter::Builtin(Syntax::Standard)
        ));
        assert!(matches!(
            highlighter(&ctx, 7, &slots),
            Highlighter::Builtin(Syntax::None)
        ));
        slots[1] = tc_core::syntax::samples().remove(1).1;
        assert!(matches!(
            highlighter(&ctx, 7, &slots),
            Highlighter::Custom(_)
        ));
        let names = choices(&slots);
        assert_eq!(names.len(), 7);
        assert_eq!(names[6], (7, "OK / error".to_string()));
        // A broken regex highlights nothing rather than failing.
        slots[1].regex = true;
        slots[1].patterns[1] = "(".into();
        assert!(matches!(
            highlighter(&ctx, 7, &slots),
            Highlighter::Builtin(Syntax::None)
        ));
    }
}
