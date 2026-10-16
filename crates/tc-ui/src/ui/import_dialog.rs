//! "Import MobaXterm sessions": an exported `.mxtsessions` file, or a whole
//! `MobaXterm.ini` (sessions, settings and trusted SSH host keys).

use serde_json::Value;

#[derive(Default)]
pub struct ImportDialog {
    pub open: bool,
    path: String,
    /// For a MobaXterm.ini: what to take from it.
    sessions: bool,
    settings: bool,
    host_keys: bool,
    /// The last import's summary, shown until the dialog closes.
    result: Vec<String>,
    /// Opened by TermCraft at first start (a MobaXterm.ini was found).
    found: bool,
}

/// What to import.
pub struct Request {
    pub path: String,
    pub sessions: bool,
    pub settings: bool,
    pub host_keys: bool,
}

fn is_ini(path: &str) -> bool {
    path.trim().to_ascii_lowercase().ends_with(".ini")
}

impl ImportDialog {
    pub fn show_dialog(&mut self) {
        self.open = true;
        self.sessions = true;
        self.settings = true;
        self.host_keys = true;
        self.found = false;
        self.result.clear();
    }

    /// First start: offer the MobaXterm.ini that was found.
    pub fn offer(&mut self, path: &std::path::Path) {
        self.show_dialog();
        self.found = true;
        self.path = path.display().to_string();
    }

    /// Draw it; `Some` when the user asked to import.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        if !self.open {
            return None;
        }
        let mut request = None;
        let mut open = true;
        egui::Window::new("Import MobaXterm sessions")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let enter = crate::ui::EnterWatch::start(ui);
                if self.found {
                    ui.strong("TermCraft found MobaXterm settings on this computer.");
                    ui.label(
                        "Bring your sessions, settings and trusted host keys over? \
                         MobaXterm.ini is only read, never changed.",
                    );
                    ui.add_space(4.0);
                }
                ui.label("An exported .mxtsessions file, or MobaXterm.ini:");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(360.0));
                    if ui.button("Browse…").clicked()
                        && let Some(file) = rfd::FileDialog::new()
                            .add_filter("MobaXterm", &["mxtsessions", "ini"])
                            .pick_file()
                    {
                        self.path = file.display().to_string();
                    }
                });
                let path = self.path.trim().trim_matches('"').to_string();
                if is_ini(&path) {
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.sessions, "Sessions");
                    ui.checkbox(
                        &mut self.settings,
                        "Settings (font, colors, terminal, SSH, display, popup terminal)",
                    );
                    ui.checkbox(&mut self.host_keys, "Trusted SSH host keys");
                    ui.weak(
                        "Saved passwords are not imported: MobaXterm encrypts them with its own \
                         scheme. TermCraft asks for each one again the first time.",
                    );
                }
                ui.add_space(4.0);
                let ini = is_ini(&path);
                let something = !ini || self.sessions || self.settings || self.host_keys;
                let ready = !path.is_empty() && something;
                if ui.add_enabled(ready, egui::Button::new("Import")).clicked()
                    || (ready && enter.pressed(ui))
                {
                    request = Some(Request {
                        path,
                        sessions: !ini || self.sessions,
                        settings: ini && self.settings,
                        host_keys: ini && self.host_keys,
                    });
                }
                for line in &self.result {
                    ui.label(line);
                }
            });
        self.open &= open;
        request
    }

    /// Show what an import did.
    pub fn report(&mut self, lines: Vec<String>) {
        self.result = lines;
    }
}

/// One line for each part of an import.
pub fn summary(
    sessions: Option<&Result<Value, String>>,
    config: Option<&Result<Value, String>>,
) -> Vec<String> {
    let mut out = Vec::new();
    match sessions {
        Some(Ok(v)) => {
            out.push(format!(
                "Sessions: {} imported, {} already there, {} not supported",
                v["imported"],
                v["skipped_existing"],
                v["unsupported"].as_array().map_or(0, Vec::len)
            ));
            // Private keys left in the MobaXterm folder, for instance.
            if let Some(missing) = v["missing_files"].as_array().filter(|m| !m.is_empty()) {
                out.push(format!(
                    "{} file(s) the sessions use are not here (copy them, or set them in each session):",
                    missing.len()
                ));
                for m in missing.iter().take(8) {
                    out.push(format!(
                        "  {} — {}",
                        m["session"].as_str().unwrap_or("?"),
                        m["file"].as_str().unwrap_or("?")
                    ));
                }
            }
        }
        Some(Err(e)) => out.push(format!("Sessions: {e}")),
        None => {}
    }
    match config {
        Some(Ok(v)) => {
            let applied = v["settings_applied"].as_array().map_or(0, Vec::len);
            if applied > 0 {
                out.push(format!("Settings: {applied} applied"));
            }
            if let Some(left) = v["settings_not_applied"].as_array() {
                for item in left {
                    out.push(format!(
                        "  Not applied: {} — {}",
                        item["setting"].as_str().unwrap_or("?"),
                        item["why"].as_str().unwrap_or("")
                    ));
                }
            }
            let found = v["host_keys_found"].as_u64().unwrap_or(0);
            if found > 0 || v["host_keys_skipped"].as_u64().unwrap_or(0) > 0 {
                out.push(format!(
                    "SSH host keys: {} added ({} found, {} checked against their fingerprint, {} skipped)",
                    v["host_keys_added"],
                    found,
                    v["host_keys_verified"],
                    v["host_keys_skipped"]
                ));
            }
        }
        Some(Err(e)) => out.push(format!("Settings: {e}")),
        None => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summary_lines() {
        let sessions = Ok(json!({"imported": 3, "skipped_existing": 1, "unsupported": []}));
        let config = Ok(json!({
            "settings_applied": ["Font.Font2", "Misc.WinPath"],
            "settings_not_applied": [{"setting": "Misc.HomeDir", "why": "kept"}],
            "host_keys_added": 2, "host_keys_found": 2, "host_keys_verified": 1, "host_keys_skipped": 0
        }));
        let lines = summary(Some(&sessions), Some(&config));
        assert_eq!(
            lines[0],
            "Sessions: 3 imported, 1 already there, 0 not supported"
        );
        assert_eq!(lines[1], "Settings: 2 applied");
        assert!(lines[2].contains("Misc.HomeDir"));
        assert!(lines[3].starts_with("SSH host keys: 2 added"));
        assert_eq!(summary(None, Some(&Err("bad".into()))), ["Settings: bad"]);
        assert!(is_ini(" C:/x/MobaXterm.INI ") && !is_ini("a.mxtsessions"));
    }
}
