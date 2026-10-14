//! Built-in text editor (Tools › "Text editor", and "Open with default
//! text editor" in the SSH-browser): one window, one tab per file, syntax
//! colors from the terminal highlighter, Save / Save as / New / Open.
//! Remote files are local copies: saving them triggers the "upload it
//! back?" question of the SSH-browser.

use super::icon;
use std::path::{Path, PathBuf};
use tc_term::Syntax;
use tc_term::highlight::{Kind, spans};

/// Largest file opened (bigger files go to the system editor).
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;

struct File {
    path: Option<PathBuf>,
    /// What the tab shows (the remote path for remote copies).
    label: String,
    text: String,
    saved: String,
    syntax: Syntax,
    error: Option<String>,
}

impl File {
    fn dirty(&self) -> bool {
        self.text != self.saved
    }

    fn title(&self) -> String {
        let name = self
            .label
            .rsplit(['/', '\\'])
            .find(|s| !s.is_empty())
            .unwrap_or("untitled");
        if self.dirty() {
            format!("{name} *")
        } else {
            name.to_string()
        }
    }
}

/// The syntax for a file name (extension), like the reference's choice.
pub fn syntax_for(name: &str) -> Syntax {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "sh" | "bash" | "zsh" | "ksh" | "profile" | "bashrc" => Syntax::Shell,
        "pl" | "pm" => Syntax::Perl,
        "sql" => Syntax::Sql,
        "cfg" if lower.contains("cisco") => Syntax::Cisco,
        _ if lower.ends_with(".bashrc") || lower.ends_with(".profile") => Syntax::Shell,
        _ => Syntax::Standard,
    }
}

/// Read a file for editing: text only, bounded.
pub fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_BYTES {
        return Err(format!("{} is larger than 10 MB", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.contains(&0) {
        return Err(format!("{} is not a text file", path.display()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[derive(Default)]
pub struct TextEditor {
    pub open: bool,
    files: Vec<File>,
    active: usize,
    wrap: bool,
    /// Closing a modified file waits for an answer.
    confirm_close: Option<usize>,
    /// "Find" / "Replace" bar (Ctrl+F; F3 = Find next).
    find: Option<Find>,
    /// Selection to apply to the text (a found match), in characters.
    select: Option<(usize, usize)>,
    /// Cursor position in the active file (characters).
    cursor: usize,
}

#[derive(Default)]
struct Find {
    needle: String,
    replace: String,
    case: bool,
    message: Option<String>,
}

fn lowered(s: &str, case: bool) -> Vec<char> {
    if case {
        s.chars().collect()
    } else {
        s.chars().flat_map(char::to_lowercase).collect()
    }
}

/// The next match of `needle` after character `from`, wrapping around;
/// character positions.
pub fn find_next(text: &str, needle: &str, from: usize, case: bool) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    // Lower-casing may change lengths; only search case-insensitively
    // when it does not.
    let hay = lowered(text, case);
    let pat = lowered(needle, case);
    let (hay, pat) = if hay.len() == text.chars().count() && pat.len() == needle.chars().count() {
        (hay, pat)
    } else {
        (text.chars().collect(), needle.chars().collect())
    };
    let at = |i: usize| hay.get(i..i + pat.len()) == Some(&pat[..]);
    let n = hay.len();
    (from..n)
        .chain(0..from.min(n))
        .find(|&i| at(i))
        .map(|i| (i, i + pat.len()))
}

/// Replace every match; returns the new text and the count.
pub fn replace_all(text: &str, needle: &str, with: &str, case: bool) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut count = 0;
    while let Some((a, b)) = find_next(text, needle, i, case).filter(|(a, _)| *a >= i) {
        out.extend(&chars[i..a]);
        out.push_str(with);
        i = b;
        count += 1;
        if i >= chars.len() {
            break;
        }
    }
    out.extend(&chars[i.min(chars.len())..]);
    (out, count)
}

impl TextEditor {
    /// Open a file (`label`: what to show, e.g. the remote path).
    pub fn open_file(&mut self, path: &Path, label: Option<&str>) -> Result<(), String> {
        if let Some(i) = self
            .files
            .iter()
            .position(|f| f.path.as_deref() == Some(path))
        {
            self.active = i;
            self.open = true;
            return Ok(());
        }
        let text = read_text(path)?;
        let label = label.map_or_else(|| path.display().to_string(), String::from);
        self.files.push(File {
            syntax: syntax_for(&label),
            path: Some(path.to_path_buf()),
            label,
            saved: text.clone(),
            text,
            error: None,
        });
        self.active = self.files.len() - 1;
        self.open = true;
        Ok(())
    }

    /// Snapshots: a shell script.
    pub fn preview(&mut self) {
        self.new_file();
        let f = self.files.last_mut().expect("just added");
        f.label = "deploy.sh".into();
        f.syntax = Syntax::Shell;
        f.text = "#!/bin/sh
# deploy the app
if [ -n \"$TARGET\" ]; then
    echo 'copying' to ${TARGET}
fi
exit 0
"
        .into();
        self.find = Some(Find {
            needle: "target".into(),
            ..Find::default()
        });
    }

    pub fn new_file(&mut self) {
        self.files.push(File {
            path: None,
            label: "untitled".into(),
            text: String::new(),
            saved: String::new(),
            syntax: Syntax::Standard,
            error: None,
        });
        self.active = self.files.len() - 1;
        self.open = true;
    }

    fn save(&mut self, i: usize, ask: bool) {
        let Some(f) = self.files.get_mut(i) else {
            return;
        };
        let path = match (&f.path, ask) {
            (Some(p), false) => p.clone(),
            _ => match rfd::FileDialog::new()
                .set_file_name(f.title().trim_end_matches(" *"))
                .save_file()
            {
                Some(p) => p,
                None => return,
            },
        };
        match std::fs::write(&path, f.text.as_bytes()) {
            Ok(()) => {
                f.saved = f.text.clone();
                f.error = None;
                if f.path.as_ref() != Some(&path) {
                    f.label = path.display().to_string();
                    f.syntax = syntax_for(&f.label);
                    f.path = Some(path);
                }
            }
            Err(e) => f.error = Some(format!("{}: {e}", path.display())),
        }
    }

    fn close(&mut self, i: usize) {
        if i < self.files.len() {
            self.files.remove(i);
        }
        self.active = self.active.min(self.files.len().saturating_sub(1));
        if self.files.is_empty() {
            self.open = false;
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        if self.files.is_empty() {
            self.new_file();
        }
        let mut open = true;
        egui::Window::new("Text editor")
            .id(egui::Id::new("text-editor"))
            .open(&mut open)
            .default_size([760.0, 520.0])
            .resizable(true)
            .show(ctx, |ui| self.contents(ui));
        if !open {
            // Closing the window keeps unsaved files for next time.
            self.open = false;
        }
        self.confirm_dialog(ctx);
    }

    fn contents(&mut self, ui: &mut egui::Ui) {
        let (save_key, find_key, next_key) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::F),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F3),
            )
        });
        if save_key {
            self.save(self.active, false);
        }
        if find_key {
            self.find.get_or_insert_with(Find::default);
        }
        if next_key {
            self.find_next();
        }
        ui.horizontal(|ui| {
            if ui.button(format!("{}  New", icon::FILE_PLUS)).clicked() {
                self.new_file();
            }
            if ui.button(format!("{}  Open", icon::FOLDER_OPEN)).clicked()
                && let Some(p) = rfd::FileDialog::new().pick_file()
                && let Err(e) = self.open_file(&p, None)
                && let Some(f) = self.files.get_mut(self.active)
            {
                f.error = Some(e);
            }
            if ui
                .button(format!("{}  Save", icon::FLOPPY_DISK))
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.save(self.active, false);
            }
            if ui.button("Save as…").clicked() {
                self.save(self.active, true);
            }
            ui.checkbox(&mut self.wrap, "Word wrap");
            if let Some(f) = self.files.get_mut(self.active) {
                egui::ComboBox::from_id_salt("editor-syntax")
                    .selected_text(syntax_label(f.syntax))
                    .show_ui(ui, |ui| {
                        for s in [
                            Syntax::None,
                            Syntax::Standard,
                            Syntax::Shell,
                            Syntax::Cisco,
                            Syntax::Perl,
                            Syntax::Sql,
                        ] {
                            ui.selectable_value(&mut f.syntax, s, syntax_label(s));
                        }
                    });
            }
        });
        ui.horizontal_wrapped(|ui| {
            let mut close = None;
            for (i, f) in self.files.iter().enumerate() {
                let r = ui
                    .selectable_label(i == self.active, f.title())
                    .on_hover_text(&f.label);
                if r.clicked() {
                    self.active = i;
                }
                if r.middle_clicked() || ui.small_button(icon::X).on_hover_text("Close").clicked() {
                    close = Some(i);
                }
            }
            if let Some(i) = close {
                if self.files[i].dirty() {
                    self.confirm_close = Some(i);
                } else {
                    self.close(i);
                }
            }
        });
        ui.separator();
        self.find_bar(ui);
        let wrap = self.wrap;
        let select = self.select.take();
        let Some(f) = self.files.get_mut(self.active) else {
            return;
        };
        if let Some(e) = &f.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        let syntax = f.syntax;
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
            let mut job = highlighted(ui, text.as_str(), syntax);
            job.wrap.max_width = if wrap { width } else { f32::INFINITY };
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let lines = f.text.lines().count().max(1) + usize::from(f.text.ends_with('\n'));
        let output = egui::ScrollArea::both()
            .id_salt("editor-scroll")
            .auto_shrink([false, false])
            .max_height(ui.available_height() - 22.0)
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    // Line numbers (rows match only without word wrap).
                    if !wrap {
                        let numbers: String = (1..=lines).map(|n| format!("{n}\n")).collect();
                        ui.add(egui::Label::new(
                            egui::RichText::new(numbers.trim_end()).monospace().weak(),
                        ));
                    }
                    egui::TextEdit::multiline(&mut f.text)
                        .id(egui::Id::new(("editor-text", f.label.as_str())))
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(24)
                        .lock_focus(true)
                        .layouter(&mut layouter)
                        .show(ui)
                })
                .inner
            })
            .inner;
        if let Some((a, b)) = select {
            let mut state = output.state.clone();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(a),
                    egui::text::CCursor::new(b),
                )));
            state.store(ui.ctx(), output.response.id);
            output.response.request_focus();
        }
        if let Some(r) = output.cursor_range {
            self.cursor = r.primary.index.0;
        }
        // Status bar: line / column of the cursor.
        let status = output.cursor_range.map(|r| {
            let upto: String = f.text.chars().take(r.primary.index.0).collect();
            let line = upto.matches('\n').count() + 1;
            let col = upto.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
            format!("Line {line}, column {col}")
        });
        ui.horizontal(|ui| {
            ui.weak(status.unwrap_or_default());
            ui.weak(format!("{} lines", f.text.lines().count().max(1)));
            if f.dirty() {
                ui.weak("modified");
            }
        });
    }

    fn find_next(&mut self) {
        let Some(find) = &mut self.find else {
            self.find = Some(Find::default());
            return;
        };
        let Some(f) = self.files.get(self.active) else {
            return;
        };
        match find_next(&f.text, &find.needle, self.cursor, find.case) {
            Some(range) => {
                self.cursor = range.1;
                self.select = Some(range);
                find.message = None;
            }
            None => find.message = Some("Not found".into()),
        }
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        let Some(find) = &mut self.find else { return };
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label("Find:");
            let r = ui.add(egui::TextEdit::singleline(&mut find.needle).desired_width(160.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                action = Some(0);
            }
            ui.label("Replace with:");
            ui.add(egui::TextEdit::singleline(&mut find.replace).desired_width(140.0));
            ui.checkbox(&mut find.case, "Match case");
            if ui.button("Find next").on_hover_text("F3").clicked() {
                action = Some(0);
            }
            if ui.button("Replace").clicked() {
                action = Some(1);
            }
            if ui.button("Replace all").clicked() {
                action = Some(2);
            }
            if ui.button(icon::X).on_hover_text("Close").clicked() {
                action = Some(3);
            }
            if let Some(m) = &find.message {
                ui.weak(m);
            }
        });
        match action {
            Some(0) => self.find_next(),
            Some(1) => {
                // Replace the match at the cursor, then find the next one.
                let (needle, with, case) = (find.needle.clone(), find.replace.clone(), find.case);
                if let Some(f) = self.files.get_mut(self.active) {
                    let len = needle.chars().count();
                    let start = self.cursor.saturating_sub(len);
                    if let Some((a, b)) =
                        find_next(&f.text, &needle, start, case).filter(|(a, _)| *a == start)
                    {
                        let chars: Vec<char> = f.text.chars().collect();
                        let mut text: String = chars[..a].iter().collect();
                        text.push_str(&with);
                        text.extend(&chars[b..]);
                        f.text = text;
                        self.cursor = a + with.chars().count();
                    }
                }
                self.find_next();
            }
            Some(2) => {
                let (needle, with, case) = (find.needle.clone(), find.replace.clone(), find.case);
                if let Some(f) = self.files.get_mut(self.active) {
                    let (text, n) = replace_all(&f.text, &needle, &with, case);
                    f.text = text;
                    if let Some(find) = &mut self.find {
                        find.message = Some(format!("{n} replaced"));
                    }
                }
            }
            Some(_) => self.find = None,
            None => {}
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(i) = self.confirm_close else { return };
        let title = self.files.get(i).map(File::title).unwrap_or_default();
        let mut answer = None;
        egui::Window::new("Unsaved changes")
            .id(egui::Id::new("editor-confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Save the changes to {}?",
                    title.trim_end_matches(" *")
                ));
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        answer = Some(1);
                    }
                    if ui.button("Don't save").clicked() {
                        answer = Some(2);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(0);
                    }
                });
            });
        match answer {
            Some(1) => {
                self.save(i, false);
                if self.files.get(i).is_some_and(|f| !f.dirty()) {
                    self.close(i);
                }
                self.confirm_close = None;
            }
            Some(2) => {
                self.close(i);
                self.confirm_close = None;
            }
            Some(_) => self.confirm_close = None,
            None => {}
        }
    }
}

fn syntax_label(s: Syntax) -> &'static str {
    match s {
        Syntax::None => "Plain text",
        Syntax::Standard => "Standard keywords",
        Syntax::Shell => "Unix shell script",
        Syntax::Cisco => "Cisco",
        Syntax::Perl => "Perl",
        Syntax::Sql => "SQL",
    }
}

/// Colors of a highlight kind in the current skin.
fn kind_color(ui: &egui::Ui, kind: Kind) -> egui::Color32 {
    let dark = ui.visuals().dark_mode;
    let pick = |light: [u8; 3], dark_c: [u8; 3]| {
        let [r, g, b] = if dark { dark_c } else { light };
        egui::Color32::from_rgb(r, g, b)
    };
    match kind {
        Kind::Error => pick([200, 30, 30], [255, 110, 110]),
        Kind::Warning => pick([170, 110, 0], [230, 190, 80]),
        Kind::Ok | Kind::Text => pick([20, 130, 50], [120, 210, 130]),
        Kind::Keyword => pick([150, 40, 160], [220, 140, 230]),
        Kind::Address | Kind::Variable | Kind::Number => pick([0, 120, 160], [110, 200, 230]),
        Kind::Comment => pick([120, 120, 120], [140, 140, 140]),
    }
}

/// The text as a colored layout, line by line.
fn highlighted(ui: &egui::Ui, text: &str, syntax: Syntax) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = egui::TextFormat::simple(font.clone(), ui.visuals().text_color());
    let mut job = egui::text::LayoutJob::default();
    for line in text.split_inclusive('\n') {
        let chars: Vec<char> = line.chars().collect();
        let mut pos = 0;
        let byte_at = |i: usize| chars[..i].iter().map(|c| c.len_utf8()).sum::<usize>();
        for span in spans(syntax, &chars) {
            if span.start > pos {
                job.append(&line[byte_at(pos)..byte_at(span.start)], 0.0, plain.clone());
            }
            let format = egui::TextFormat::simple(font.clone(), kind_color(ui, span.kind));
            job.append(&line[byte_at(span.start)..byte_at(span.end)], 0.0, format);
            pos = span.end;
        }
        if pos < chars.len() {
            job.append(&line[byte_at(pos)..], 0.0, plain.clone());
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_and_reading() {
        assert_eq!(syntax_for("/etc/init.d/run.sh"), Syntax::Shell);
        assert_eq!(syntax_for("q.SQL"), Syntax::Sql);
        assert_eq!(syntax_for("/home/u/.bashrc"), Syntax::Shell);
        assert_eq!(syntax_for("notes.txt"), Syntax::Standard);
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("a.txt");
        std::fs::write(&text, "héllo\n").unwrap();
        assert_eq!(read_text(&text).unwrap(), "héllo\n");
        let bin = dir.path().join("b.bin");
        std::fs::write(&bin, [1u8, 0, 2]).unwrap();
        assert!(read_text(&bin).is_err());
        let mut ed = TextEditor::default();
        ed.open_file(&text, Some("/srv/a.txt")).unwrap();
        ed.open_file(&text, None).unwrap();
        assert_eq!(ed.files.len(), 1, "the same file opens once");
        assert_eq!(ed.files[0].title(), "a.txt");
        ed.files[0].text.push('x');
        assert_eq!(ed.files[0].title(), "a.txt *");
        ed.save(0, false);
        assert_eq!(std::fs::read_to_string(&text).unwrap(), "héllo\nx");
        assert!(!ed.files[0].dirty());
    }

    #[test]
    fn find_and_replace() {
        let t = "Foo bar foo baz";
        assert_eq!(find_next(t, "foo", 0, true), Some((8, 11)));
        assert_eq!(find_next(t, "foo", 0, false), Some((0, 3)));
        assert_eq!(find_next(t, "foo", 1, false), Some((8, 11)));
        assert_eq!(find_next(t, "foo", 9, false), Some((0, 3)), "wraps around");
        assert_eq!(find_next(t, "qux", 0, false), None);
        assert_eq!(
            find_next("éa éa", "éa", 1, true),
            Some((3, 5)),
            "characters, not bytes"
        );
        assert_eq!(replace_all(t, "foo", "X", false), ("X bar X baz".into(), 2));
        assert_eq!(replace_all("aaa", "a", "aa", true), ("aaaaaa".into(), 3));
        assert_eq!(replace_all("abc", "", "x", true), ("abc".into(), 0));
    }
}
