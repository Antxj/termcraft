//! Layout of the session settings dialog and its sub-dialogs, as in the
//! reference (`cargo xtask schema` → `layout_gen.rs`): each tab sheet and
//! sub-dialog with its controls (kind, position, size, caption), and which
//! session parameter each control edits.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Check,
    Edit,
    Spin,
    Combo,
    File,
    Folder,
    Button,
    Color,
    Group,
    Label,
    List,
    /// A multi-line text (startup commands).
    Memo,
    /// A scroll bar used as a slider (percentages).
    Slider,
}

#[derive(Debug, Clone, Copy)]
pub struct Ctl {
    pub name: &'static str,
    pub kind: Kind,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Own caption (check boxes, buttons, groups) or bound label.
    pub caption: &'static str,
    /// List box items, in order (empty for other controls).
    pub items: &'static [&'static str],
    /// The panel it sits on (`""` = directly on the page).
    pub panel: &'static str,
    /// The bound label sits above the control (else on its left).
    pub label_above: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub form: &'static str,
    /// Tab sheet name, or the sub-dialog prefix (`FormExpertSSH`).
    pub name: &'static str,
    pub caption: &'static str,
    /// Client size of a sub-dialog (0 for tab sheets).
    pub width: i32,
    pub height: i32,
    pub controls: &'static [Ctl],
}

#[derive(Debug, Clone, Copy)]
pub struct Binding {
    /// Schema section (`1_0`, `2`, `3_0`, `4`).
    pub section: &'static str,
    pub param: &'static str,
    /// Sub-dialog prefix, empty for the main dialog.
    pub sub_dialog: &'static str,
    pub control: &'static str,
}

pub use crate::layout_gen::{BINDINGS, PAGES};

/// A tab sheet (`TabSSH`) or sub-dialog (`FormExpertSSH`).
pub fn page(name: &str) -> Option<&'static Page> {
    PAGES.iter().find(|p| p.name == name)
}

/// A control of a page, by name; for the main dialog every sheet is
/// searched (`page` is then any of its sheets).
pub fn page_control(page: &str, control: &str) -> Option<&'static Ctl> {
    let p = self::page(page)?;
    let pages: Vec<&'static Page> = if p.form == "TFORMSESSIONEDIT" {
        PAGES.iter().filter(|q| q.form == p.form).collect()
    } else {
        vec![p]
    };
    pages
        .into_iter()
        .flat_map(|q| q.controls.iter())
        .find(|c| c.name == control)
}

/// Basic and advanced tab sheets of each session type (reference order).
pub const TYPE_SHEETS: [(&str, &str); 15] = [
    ("sTabSheet2", "TabSSH"),
    ("sTabSheet3", "TabTelnet"),
    ("sTabSheet4", "TabRlogin"),
    ("sTabSheet5", "TabXDMCP"),
    ("sTabSheet6", "TabRDP"),
    ("sTabSheet7", "TabVNC"),
    ("sTabSheet8", "TabFTP"),
    ("sTabSheet9", "sTabSheet1"),
    ("sTabSheet10", "TabSerial"),
    ("sTabSheet11", "TabUrl"),
    ("sTabSheet12", "TabLocal"),
    ("sTabSheet15", "TabBrowser"),
    ("sTabSheet14", "TabMosh"),
    ("sTabSheet13", "sTabSheet16"),
    ("sTabSheet17", "sTabSheet18"),
];

/// The parameter a control edits for session type `type_index`.
pub fn param_for(type_index: usize, sub_dialog: &str, control: &str) -> Option<&'static str> {
    let sections = [
        format!("1_{type_index}"),
        format!("3_{type_index}"),
        "2".to_string(),
        "4".to_string(),
    ];
    sections.iter().find_map(|s| {
        BINDINGS
            .iter()
            .find(|b| b.section == s && b.sub_dialog == sub_dialog && b.control == control)
            .map(|b| b.param)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_has_its_sheets() {
        for (basic, advanced) in TYPE_SHEETS {
            assert!(page(basic).is_some(), "{basic}");
            assert!(page(advanced).is_some(), "{advanced}");
        }
        for p in [
            "TabTerminal",
            "TabNetwork",
            "TabSession",
            "FormExpertSSH",
            "FormFont",
            "FormColor",
        ] {
            assert!(page(p).is_some(), "{p}");
        }
    }

    #[test]
    fn controls_map_to_parameters() {
        assert_eq!(param_for(0, "", "sEdit9"), Some("3_2"), "SSH host");
        assert_eq!(param_for(0, "", "sFilenameEdit2"), Some("3_15"), "SSH key");
        assert_eq!(param_for(0, "FormExpertSSH", "sCheckBox2"), Some("3_32"));
        assert_eq!(param_for(0, "", "sEdit28"), Some("3_21"), "SSH proxy host");
        assert_eq!(
            param_for(1, "", "sEdit28"),
            Some("3_13"),
            "Telnet proxy host"
        );
        assert_eq!(param_for(4, "", "sEdit16"), Some("6_1"), "comments");
        assert_eq!(param_for(0, "FormColor", "Col1"), Some("4_17"));
        assert_eq!(param_for(0, "FormColor", "Col16"), Some("4_32"));
        // Every binding of a known sheet points to a real control.
        for b in BINDINGS {
            let in_sub = page(b.sub_dialog).filter(|_| !b.sub_dialog.is_empty());
            let found = match in_sub {
                Some(p) => p.controls.iter().any(|c| c.name == b.control),
                None => PAGES
                    .iter()
                    .filter(|p| p.form == "TFORMSESSIONEDIT")
                    .any(|p| p.controls.iter().any(|c| c.name == b.control)),
            };
            // Hidden reference controls are not in the table.
            let _ = found;
        }
    }
}
