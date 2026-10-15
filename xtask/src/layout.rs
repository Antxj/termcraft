//! Generate `crates/tc-core/src/layout_gen.rs`: where each control of the
//! session settings dialog and of its sub-dialogs sits (position, size,
//! kind, caption), and which session parameter it edits. TermCraft draws
//! the dialogs from this table so they look like the reference.
//!
//! Only geometry and short captions are taken; hints (tooltips) stay in the
//! local inventory.

use crate::Reference;
use crate::dfm::{Object, Value};
use anyhow::{Context as _, Result};
use std::fmt::Write as _;

/// Forms drawn by TermCraft, and the prefix their controls have in the
/// `Comp_*` entries of the parameter table.
/// Forms with tab sheets give one page per sheet (named `prefix.sheet`,
/// or just `sheet` without a prefix); the others one page.
const FORMS: &[(&str, &str, bool)] = &[
    ("TFORMSESSIONEDIT", "", true),
    ("TFORMPARAMS", "Params", true),
    ("TFORMSSHPROTOSETTINGS", "FormExpertSSH", false),
    ("TFORMFONTSELECTOR", "FormFont", false),
    ("TFORMCOLORSSELECTOR", "FormColor", false),
];

/// Controls of the parameter table without a `Comp_*` entry.
fn extra_bindings() -> Vec<(String, String, String)> {
    // The 16 ANSI colors: Col1 = Black (4_17) … Col16 = BoldWhite (4_32).
    let mut extra: Vec<(String, String, String)> = (1..=16)
        .map(|n| {
            (
                "2".to_string(),
                format!("4_{}", 16 + n),
                format!("FormColor.Col{n}"),
            )
        })
        .collect();
    // Bookmark "Comments:".
    extra.push(("4".into(), "6_1".into(), "sEdit16".into()));
    extra
}

fn int(o: &Object, key: &str) -> Option<i64> {
    o.prop(key).and_then(Value::as_int)
}

fn kind(class: &str) -> Option<&'static str> {
    Some(match class {
        "TsCheckBox" | "TCheckBox" | "TsRadioButton" => "Check",
        "TsEdit" | "TEdit" => "Edit",
        "TsMemo" | "TSynEdit" | "TMemo" => "Memo",
        "TsSpinEdit" | "TsDecimalSpinEdit" => "Spin",
        "TsComboBox" | "TsComboBoxEx" | "TsFontComboBox" | "TsComboEdit" => "Combo",
        "TsFilenameEdit" => "File",
        "TsDirectoryEdit" => "Folder",
        "TsBitBtn" | "TButton" | "TsButton" | "TsSpeedButton" => "Button",
        "TsColorSelect" => "Color",
        "TsGroupBox" => "Group",
        "TsLabel" | "TLabel" => "Label",
        "TsListBox" | "TsListView" | "TsCheckListBox" => "List",
        "TsScrollBar" => "Slider",
        _ => return None,
    })
}

struct Ctl {
    name: String,
    kind: &'static str,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    caption: String,
    /// List box items (algorithm names); empty for other controls.
    items: Vec<String>,
    /// The panel it is on ("" = directly on the page).
    panel: String,
}

fn hidden(o: &Object) -> bool {
    matches!(o.prop("Visible"), Some(Value::Bool(false)))
}

/// Controls of `o` (a tab sheet or a form) with positions relative to it;
/// panels are flattened, group boxes kept (with their children).
fn controls(o: &Object, dx: i64, dy: i64, panel: &str, out: &mut Vec<Ctl>) {
    for c in &o.children {
        if hidden(c) {
            continue;
        }
        let (Some(x), Some(y)) = (int(c, "Left"), int(c, "Top")) else {
            continue;
        };
        let (x, y) = (x + dx, y + dy);
        if c.class == "TsPanel" || c.class == "TPanel" {
            controls(c, x, y, &c.name, out);
            continue;
        }
        if c.class == "TsPageControl" || c.class == "TsTabSheet" {
            continue;
        }
        let Some(kind) = kind(&c.class) else {
            continue;
        };
        // The tall decorative strip on the left of the sub-dialogs.
        if c.class == "TsSpeedButton" && int(c, "Height").unwrap_or(0) > 100 {
            continue;
        }
        // A bound label is shown only when active.
        let bound = matches!(c.prop("BoundLabel.Active"), Some(Value::Bool(true)))
            .then(|| c.str_prop("BoundLabel.Caption"))
            .flatten();
        let caption = c
            .str_prop("Caption")
            .filter(|s| !s.is_empty())
            .or(bound)
            .unwrap_or("")
            .replace('&', "")
            // TermCraft's captions name TermCraft, not the reference.
            .replace("MobaXterm", "TermCraft")
            .replace("MobAgent", "TermCraft agent");
        out.push(Ctl {
            name: c.name.clone(),
            kind,
            x,
            y,
            w: int(c, "Width").unwrap_or(0),
            h: int(c, "Height").unwrap_or(0),
            caption,
            items: if kind == "List" {
                match c.prop("Items.Strings") {
                    Some(Value::List(v)) => v
                        .iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect(),
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            },
            panel: panel.to_string(),
        });
        if kind == "Group" {
            controls(c, x, y, panel, out);
        }
    }
}

fn sheets<'a>(o: &'a Object, out: &mut Vec<&'a Object>) {
    for c in &o.children {
        if c.class == "TsTabSheet" {
            out.push(c);
        } else {
            sheets(c, out);
        }
    }
}

pub fn generate(r: &Reference) -> Result<String> {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// @generated by `cargo xtask schema` from the reference app {} — do not edit.\n\
         // Geometry of the session settings dialog and its sub-dialogs.\n\n\
         use crate::layout::{{Binding, Ctl, Kind, Page}};\n\n\
         pub const PAGES: &[Page] = &[",
        r.version
    );
    for (form_name, prefix, by_sheet) in FORMS {
        let form = r
            .forms
            .get(*form_name)
            .with_context(|| format!("{form_name} missing"))?;
        let mut pages: Vec<(String, String, i64, i64, Vec<Ctl>)> = Vec::new();
        if *by_sheet {
            let mut list = Vec::new();
            sheets(form, &mut list);
            for sheet in list {
                let mut ctls = Vec::new();
                controls(sheet, 0, 0, "", &mut ctls);
                let caption = sheet.str_prop("Caption").unwrap_or("").to_string();
                let name = if prefix.is_empty() {
                    sheet.name.clone()
                } else {
                    format!("{prefix}.{}", sheet.name)
                };
                pages.push((name, caption, 0, 0, ctls));
            }
        } else {
            let mut ctls = Vec::new();
            controls(form, 0, 0, "", &mut ctls);
            let caption = form.str_prop("Caption").unwrap_or("").to_string();
            pages.push((
                (*prefix).to_string(),
                caption,
                int(form, "ClientWidth").unwrap_or(0),
                int(form, "ClientHeight").unwrap_or(0),
                ctls,
            ));
        }
        for (name, caption, w, h, ctls) in pages {
            let _ = writeln!(
                out,
                "    Page {{\n        form: {form_name:?},\n        name: {name:?},\n        caption: {caption:?},\n        width: {w},\n        height: {h},\n        controls: &["
            );
            for c in ctls {
                let _ = writeln!(
                    out,
                    "            Ctl {{ name: {:?}, kind: Kind::{}, x: {}, y: {}, w: {}, h: {}, caption: {:?}, items: &{:?}, panel: {:?} }},",
                    c.name, c.kind, c.x, c.y, c.w, c.h, c.caption, c.items, c.panel
                );
            }
            out.push_str("        ],\n    },\n");
        }
    }
    out.push_str("];\n\n");

    // Which parameter each control edits, per schema section.
    let raw = r
        .files
        .get("SEPARAMS")
        .context("SEPARAMS resource missing")?;
    let sections = crate::inventory::ini(&String::from_utf8_lossy(raw));
    let mut bindings: Vec<(String, String, String)> = Vec::new();
    for (key, kv) in &sections {
        for (k, comp) in kv {
            let Some(id) = k.strip_prefix("Comp_") else {
                continue;
            };
            if !comp.is_empty() {
                bindings.push((key.clone(), id.to_string(), comp.clone()));
            }
        }
    }
    bindings.extend(extra_bindings());
    out.push_str("pub const BINDINGS: &[Binding] = &[\n");
    for (section, id, comp) in bindings {
        let (page, control) = match comp.split_once('.') {
            Some((p, c)) => (p.to_string(), c.to_string()),
            None => (String::new(), comp),
        };
        let _ = writeln!(
            out,
            "    Binding {{ section: {section:?}, param: {id:?}, sub_dialog: {page:?}, control: {control:?} }},"
        );
    }
    out.push_str("];\n");
    Ok(out)
}
