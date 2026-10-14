//! Terminal widget: paints a `tc_term::Terminal` grid and turns egui input
//! events into bytes for the tab. CPU-side text painting for now; a wgpu
//! glyph-atlas renderer is tracked in docs/PARITY.md.

use crate::tab::Tab;
use egui::{Color32, FontId, PointerButton, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use tc_core::settings::TerminalSettings;
use tc_term::{
    CursorShape, Key, Modifiers, MouseAction, MouseButton, MouseEvent, Rgb, SelectionMode,
    encode_key,
};

/// Selected cells (fixed so they stay readable on any theme).
const SELECTION_BG: Color32 = Color32::from_rgb(0x26, 0x4f, 0x78);
const SELECTION_FG: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);

fn color(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// What the widget produced this frame. Input is returned rather than sent
/// so the app can route it (one tab, or every tab in MultiExec mode).
#[derive(Default)]
pub struct TermOutput {
    pub clicked: bool,
    pub input: Vec<Vec<u8>>,
    /// Text to paste (the tab applies bracketing and its paste delay).
    pub paste: Option<String>,
    /// Lines scrolled with the wheel ("Synchronize mouse wheel scrolling").
    pub scrolled: Option<isize>,
}

pub fn show(
    ui: &mut egui::Ui,
    tab: &mut Tab,
    global_font_size: f32,
    active: bool,
    opts: &TerminalSettings,
) -> TermOutput {
    ui.push_id(tab.id, |ui| {
        show_inner(ui, tab, global_font_size, active, opts)
    })
    .inner
}

fn show_inner(
    ui: &mut egui::Ui,
    tab: &mut Tab,
    global_font_size: f32,
    active: bool,
    opts: &TerminalSettings,
) -> TermOutput {
    let mut out = TermOutput::default();
    let font = FontId::monospace(tab.font_size.unwrap_or(global_font_size));
    let (cell_w, cell_h) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));

    let available = ui.available_size();
    let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
    let cols = ((rect.width() / cell_w).floor() as u16).max(2);
    let rows = ((rect.height() / cell_h).floor() as u16).max(1);
    tab.resize(rows, cols);

    // Focus follows the active terminal unless another widget holds it.
    let nothing_focused = ui.memory(|m| m.focused().is_none());
    if response.secondary_clicked() && tab.paste_right_click {
        // "Paste using right-click": ask the platform for the clipboard;
        // it arrives as an Event::Paste on the focused terminal.
        out.clicked = true;
        response.request_focus();
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    } else if response.clicked() || response.drag_started() {
        out.clicked = true;
        response.request_focus();
    } else if active && nothing_focused {
        response.request_focus();
    }
    // Keep Tab / arrows / Esc inside the terminal instead of moving UI focus.
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        );
    });

    tab.term.word_delimiters.clone_from(&opts.word_delimiters);
    tab.term.syntax = tab
        .syntax
        .unwrap_or_else(|| tc_term::Syntax::from_index(opts.syntax_highlighting));
    let cell_at = |pos: Pos2| -> (u16, u16) {
        let row = ((pos.y - rect.top()) / cell_h)
            .floor()
            .clamp(0.0, f32::from(rows - 1));
        let col = ((pos.x - rect.left()) / cell_w)
            .floor()
            .clamp(0.0, f32::from(cols - 1));
        (row as u16, col as u16)
    };
    let mods = ui.input(|i| i.modifiers);
    // Programs that ask for the mouse get it; Shift forces a selection.
    let reporting = tab.term.mouse_reporting() && !mods.shift;
    if reporting {
        out.input.extend(mouse_reports(ui, tab, rect, &cell_at));
    } else {
        selection(ui, tab, &response, &cell_at, opts.copy_on_select);
    }

    if response.has_focus() {
        let (input, paste) = handle_input(ui, tab, opts.warn_multiline_paste);
        out.input.extend(input);
        out.paste = paste;
    }
    if response.hovered() && !reporting {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let lines = (scroll / cell_h).round() as isize * 3;
            if tab.term.alternate_screen() {
                // less / man / vim without mouse mode: the wheel scrolls
                // the program, like xterm's alternate scroll.
                let key = if lines > 0 { Key::Up } else { Key::Down };
                let app_cursor = tab.term.application_cursor();
                if let Some(bytes) = encode_key(key, Modifiers::default(), app_cursor) {
                    for _ in 0..lines.unsigned_abs() {
                        out.input.push(bytes.clone());
                    }
                }
            } else {
                scroll_by(tab, lines);
                out.scrolled = Some(lines);
            }
        }
    }

    paint(
        ui,
        rect,
        tab,
        &font,
        vec2(cell_w, cell_h),
        response.has_focus(),
    );
    out
}

/// Scroll the view through the history by `lines` (positive = up).
pub fn scroll_by(tab: &mut Tab, lines: isize) {
    let target = (tab.term.scrollback() as isize + lines).max(0) as usize;
    tab.term.set_scrollback(target);
}

/// Mouse selection (char / Ctrl+Alt block drag, word double click, line
/// triple click); "Copy on select" copies when the gesture ends.
fn selection(
    ui: &egui::Ui,
    tab: &mut Tab,
    response: &egui::Response,
    cell_at: &dyn Fn(Pos2) -> (u16, u16),
    copy_on_select: bool,
) {
    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    let Some(pos) = pointer else {
        return;
    };
    let (row, col) = cell_at(pos);
    let mut done = false;
    if response.triple_clicked() {
        tab.term.select_start(row, col, SelectionMode::Line);
        done = true;
    } else if response.double_clicked() {
        tab.term.select_start(row, col, SelectionMode::Word);
        done = true;
    } else if response.drag_started_by(PointerButton::Primary) {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
        let (r, c) = cell_at(origin);
        let mods = ui.input(|i| i.modifiers);
        let mode = if mods.ctrl && mods.alt {
            SelectionMode::Block
        } else {
            SelectionMode::Char
        };
        tab.term.select_start(r, c, mode);
        tab.term.select_extend(row, col);
    } else if response.dragged_by(PointerButton::Primary) {
        tab.term.select_extend(row, col);
    } else if response.drag_stopped_by(PointerButton::Primary) {
        tab.term.select_extend(row, col);
        done = true;
    } else if response.clicked() {
        tab.term.clear_selection();
    }
    if done
        && copy_on_select
        && let Some(text) = tab.term.selection_text().filter(|t| !t.is_empty())
    {
        ui.ctx().copy_text(text);
    }
}

/// Mouse events for programs in mouse mode (vim, htop, tmux…).
fn mouse_reports(
    ui: &egui::Ui,
    tab: &Tab,
    rect: Rect,
    cell_at: &dyn Fn(Pos2) -> (u16, u16),
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let held = ui.input(|i| {
        if i.pointer.primary_down() {
            MouseButton::Left
        } else if i.pointer.middle_down() {
            MouseButton::Middle
        } else if i.pointer.secondary_down() {
            MouseButton::Right
        } else {
            MouseButton::None
        }
    });
    let events = ui.input(|i| i.events.clone());
    let to_mods = |m: egui::Modifiers| Modifiers {
        ctrl: m.ctrl,
        alt: m.alt,
        shift: m.shift,
    };
    for event in events {
        let (pos, button, action, mods) = match event {
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            } => {
                let button = match button {
                    PointerButton::Primary => MouseButton::Left,
                    PointerButton::Middle => MouseButton::Middle,
                    PointerButton::Secondary => MouseButton::Right,
                    _ => continue,
                };
                let action = if pressed {
                    MouseAction::Press
                } else {
                    MouseAction::Release
                };
                (pos, button, action, to_mods(modifiers))
            }
            egui::Event::PointerMoved(pos) => {
                (pos, held, MouseAction::Motion, Modifiers::default())
            }
            egui::Event::MouseWheel {
                delta, modifiers, ..
            } if delta.y != 0.0 => {
                let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
                    continue;
                };
                let button = if delta.y > 0.0 {
                    MouseButton::WheelUp
                } else {
                    MouseButton::WheelDown
                };
                (pos, button, MouseAction::Press, to_mods(modifiers))
            }
            _ => continue,
        };
        if !rect.contains(pos) {
            continue;
        }
        let (row, col) = cell_at(pos);
        let ev = MouseEvent {
            button,
            action,
            row,
            col,
            mods,
        };
        if let Some(bytes) = tab.term.encode_mouse(ev) {
            out.push(bytes);
        }
    }
    out
}

fn paint(ui: &egui::Ui, rect: Rect, tab: &Tab, font: &FontId, cell: Vec2, focused: bool) {
    let painter = ui.painter_at(rect);
    let background = tab.term.theme.background;
    painter.rect_filled(rect, 0.0, color(background));

    let (rows, cols) = tab.term.size();
    let mut run = String::new();
    for row in 0..rows {
        let y = rect.top() + f32::from(row) * cell.y;
        let highlight = tab.term.highlight_row(row);
        let fg_at = |col: u16, c: &tc_term::CellView| {
            highlight
                .get(usize::from(col))
                .copied()
                .flatten()
                .unwrap_or(c.fg)
        };
        let mut col = 0;
        while col < cols {
            let Some(first) = tab.term.cell(row, col) else {
                break;
            };
            let first_fg = fg_at(col, &first);
            // Collect a run of cells sharing the same style.
            let start = col;
            let selected = tab.term.is_selected(row, col);
            run.clear();
            while col < cols {
                let Some(c) = tab.term.cell(row, col) else {
                    break;
                };
                if fg_at(col, &c) != first_fg
                    || c.bg != first.bg
                    || c.bold != first.bold
                    || c.underline != first.underline
                    || tab.term.is_selected(row, col) != selected
                {
                    break;
                }
                if !c.wide_continuation {
                    run.push_str(if c.text.is_empty() { " " } else { c.text });
                }
                col += 1;
            }
            let x = rect.left() + f32::from(start) * cell.x;
            let width = f32::from(col - start) * cell.x;
            let run_rect = Rect::from_min_size(pos2(x, y), vec2(width, cell.y));
            if selected {
                painter.rect_filled(run_rect, 0.0, SELECTION_BG);
            } else if first.bg != background {
                painter.rect_filled(run_rect, 0.0, color(first.bg));
            }
            if !run.trim().is_empty() {
                let fg = if selected {
                    SELECTION_FG
                } else {
                    color(first_fg)
                };
                painter.text(pos2(x, y), egui::Align2::LEFT_TOP, &run, font.clone(), fg);
                if first.underline {
                    let uy = y + cell.y - 1.0;
                    painter.line_segment([pos2(x, uy), pos2(x + width, uy)], Stroke::new(1.0, fg));
                }
            }
        }
    }

    if let Some((row, col)) = tab.term.cursor() {
        let min = pos2(
            rect.left() + f32::from(col) * cell.x,
            rect.top() + f32::from(row) * cell.y,
        );
        let theme = &tab.term.theme;
        let Rgb(r, g, b) = theme.cursor;
        let c = Color32::from_rgba_unmultiplied(r, g, b, 170);
        // Blinking cursors toggle every 0.5 s while focused.
        let visible = !(theme.cursor_blink && focused) || {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
            (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0
        };
        let cursor = match theme.cursor_shape {
            CursorShape::Block => Rect::from_min_size(min, cell),
            CursorShape::Underline => {
                Rect::from_min_size(min + vec2(0.0, cell.y - 2.0), vec2(cell.x, 2.0))
            }
            CursorShape::Line => Rect::from_min_size(min, vec2(2.0, cell.y)),
        };
        if !focused {
            let outline = Rect::from_min_size(min, cell);
            painter.rect_stroke(outline, 0.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
        } else if visible {
            painter.rect_filled(cursor, 0.0, c);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clip {
    Copy,
    Cut,
    Paste,
}

/// Keys held when egui reported a clipboard event.
#[derive(Debug, Clone, Copy, Default)]
struct ClipKeys {
    shift: bool,
    ctrl: bool,
    /// macOS Command.
    cmd: bool,
    insert: bool,
    delete: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum ClipAction {
    CopySelection,
    Paste,
    Send(Vec<u8>),
    DeleteKey,
    Nothing,
}

/// egui turns Ctrl+C / X / V into clipboard events. In a terminal those
/// are control characters (^C interrupts, ^X / ^V for nano, readline…);
/// copy is Ctrl+Insert or Ctrl+Shift+C, paste Shift+Insert or Ctrl+Shift+V,
/// and Command+C / V on macOS.
fn clipboard_action(kind: Clip, k: ClipKeys) -> ClipAction {
    match kind {
        Clip::Copy if k.shift || k.cmd || k.insert => ClipAction::CopySelection,
        Clip::Copy => ClipAction::Send(vec![0x03]),
        Clip::Cut if k.delete => ClipAction::DeleteKey, // Shift+Delete
        Clip::Cut if k.cmd => ClipAction::Nothing,
        Clip::Cut => ClipAction::Send(vec![0x18]),
        Clip::Paste if k.ctrl && !k.shift && !k.cmd && !k.insert => ClipAction::Send(vec![0x16]),
        Clip::Paste => ClipAction::Paste,
    }
}

/// Bytes for pasted text: line endings as Enter, bracketed if asked.
pub fn paste_bytes(tab: &Tab, text: &str) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if tab.term.bracketed_paste() {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

fn handle_input(
    ui: &egui::Ui,
    tab: &mut Tab,
    warn_multiline: bool,
) -> (Vec<Vec<u8>>, Option<String>) {
    let mut sent = Vec::new();
    let mut paste = None;
    let (events, mods, insert_down, delete_down) = ui.input(|i| {
        (
            i.events.clone(),
            i.modifiers,
            i.key_down(egui::Key::Insert),
            i.key_down(egui::Key::Delete),
        )
    });
    let app_cursor = tab.term.application_cursor();
    for event in events {
        match event {
            egui::Event::Text(text) => sent.push(text.into_bytes()),
            // egui turns Ctrl+C / Ctrl+X / Ctrl+V into clipboard events; a
            // terminal sends them as control characters, and copies /
            // pastes with Ctrl+Insert, Shift+Insert, Ctrl+Shift+C / V
            // (Cmd+C / V on macOS).
            egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) => {
                let keys = ClipKeys {
                    shift: mods.shift,
                    ctrl: mods.ctrl,
                    cmd: mods.mac_cmd,
                    insert: insert_down,
                    delete: delete_down,
                };
                let (kind, text) = match event {
                    egui::Event::Copy => (Clip::Copy, None),
                    egui::Event::Cut => (Clip::Cut, None),
                    egui::Event::Paste(t) => (Clip::Paste, Some(t)),
                    _ => unreachable!(),
                };
                match clipboard_action(kind, keys) {
                    ClipAction::CopySelection => {
                        if let Some(t) = tab.term.selection_text().filter(|t| !t.is_empty()) {
                            ui.ctx().copy_text(t);
                        }
                    }
                    ClipAction::Send(bytes) => sent.push(bytes),
                    ClipAction::DeleteKey => {
                        if let Some(b) = encode_key(Key::Delete, Modifiers::default(), app_cursor) {
                            sent.push(b);
                        }
                    }
                    ClipAction::Paste => {
                        let text = text.unwrap_or_default();
                        if warn_multiline && text.trim_end().contains('\n') {
                            tab.pending_paste = Some(text);
                        } else {
                            paste = Some(text);
                        }
                    }
                    ClipAction::Nothing => {}
                }
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if modifiers.shift && !modifiers.ctrl {
                    let page = usize::from(tab.term.size().0);
                    match key {
                        egui::Key::PageUp => {
                            tab.term.set_scrollback(tab.term.scrollback() + page);
                            continue;
                        }
                        egui::Key::PageDown => {
                            tab.term
                                .set_scrollback(tab.term.scrollback().saturating_sub(page));
                            continue;
                        }
                        _ => {}
                    }
                }
                // Session "Backspace sends ^H" swaps DEL and ^H.
                let swap_bs = key == egui::Key::Backspace && tab.backspace_sends_ctrl_h;
                let mods = Modifiers {
                    ctrl: modifiers.ctrl != swap_bs,
                    alt: modifiers.alt,
                    shift: modifiers.shift,
                };
                if let Some(k) = map_key(key, mods)
                    && let Some(bytes) = encode_key(k, mods, app_cursor)
                {
                    sent.push(bytes);
                }
            }
            _ => {}
        }
    }
    (sent, paste)
}

/// Map egui keys to terminal keys. Printable characters arrive as
/// `Event::Text`, so letters are only mapped when Ctrl/Alt suppress the text.
fn map_key(key: egui::Key, mods: Modifiers) -> Option<Key> {
    use egui::Key as K;
    Some(match key {
        K::Enter => Key::Enter,
        K::Backspace => Key::Backspace,
        K::Tab => Key::Tab,
        K::Escape => Key::Escape,
        K::ArrowUp => Key::Up,
        K::ArrowDown => Key::Down,
        K::ArrowLeft => Key::Left,
        K::ArrowRight => Key::Right,
        K::Home => Key::Home,
        K::End => Key::End,
        K::PageUp => Key::PageUp,
        K::PageDown => Key::PageDown,
        K::Insert => Key::Insert,
        K::Delete => Key::Delete,
        K::F1 => Key::F(1),
        K::F2 => Key::F(2),
        K::F3 => Key::F(3),
        K::F4 => Key::F(4),
        K::F5 => Key::F(5),
        K::F6 => Key::F(6),
        K::F7 => Key::F(7),
        K::F8 => Key::F(8),
        K::F9 => Key::F(9),
        K::F10 => Key::F(10),
        K::F11 => Key::F(11),
        K::F12 => Key::F(12),
        _ if mods.ctrl || mods.alt => {
            let name = key.name();
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_keys_reach_the_program_and_copy_paste_have_their_own_keys() {
        let ctrl = ClipKeys {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            clipboard_action(Clip::Copy, ctrl),
            ClipAction::Send(vec![0x03])
        );
        assert_eq!(
            clipboard_action(Clip::Cut, ctrl),
            ClipAction::Send(vec![0x18])
        );
        assert_eq!(
            clipboard_action(Clip::Paste, ctrl),
            ClipAction::Send(vec![0x16])
        );

        let ctrl_shift = ClipKeys {
            shift: true,
            ..ctrl
        };
        assert_eq!(
            clipboard_action(Clip::Copy, ctrl_shift),
            ClipAction::CopySelection
        );
        assert_eq!(clipboard_action(Clip::Paste, ctrl_shift), ClipAction::Paste);

        let ctrl_insert = ClipKeys {
            insert: true,
            ..ctrl
        };
        assert_eq!(
            clipboard_action(Clip::Copy, ctrl_insert),
            ClipAction::CopySelection
        );
        let shift_insert = ClipKeys {
            shift: true,
            insert: true,
            ..Default::default()
        };
        assert_eq!(
            clipboard_action(Clip::Paste, shift_insert),
            ClipAction::Paste
        );
        let shift_delete = ClipKeys {
            shift: true,
            delete: true,
            ..Default::default()
        };
        assert_eq!(
            clipboard_action(Clip::Cut, shift_delete),
            ClipAction::DeleteKey
        );

        let cmd = ClipKeys {
            cmd: true,
            ..Default::default()
        };
        assert_eq!(clipboard_action(Clip::Copy, cmd), ClipAction::CopySelection);
        assert_eq!(clipboard_action(Clip::Paste, cmd), ClipAction::Paste);
        // Right-click paste arrives with no keys held.
        assert_eq!(
            clipboard_action(Clip::Paste, ClipKeys::default()),
            ClipAction::Paste
        );
    }
}
