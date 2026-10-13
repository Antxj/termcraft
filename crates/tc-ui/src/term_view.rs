//! Terminal widget: paints a `tc_term::Terminal` grid and turns egui input
//! events into bytes for the tab. CPU-side text painting for now; a wgpu
//! glyph-atlas renderer is tracked in docs/PARITY.md.

use crate::tab::Tab;
use egui::{Color32, FontId, Rect, Sense, Stroke, Vec2, pos2, vec2};
use tc_term::{CursorShape, Key, Modifiers, Rgb, encode_key};

fn color(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

pub fn show(ui: &mut egui::Ui, tab: &mut Tab, global_font_size: f32) {
    let font = FontId::monospace(tab.font_size.unwrap_or(global_font_size));
    let (cell_w, cell_h) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));

    let available = ui.available_size();
    let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
    let cols = ((rect.width() / cell_w).floor() as u16).max(2);
    let rows = ((rect.height() / cell_h).floor() as u16).max(1);
    tab.resize(rows, cols);

    // Focus follows the active terminal unless another widget holds it.
    let nothing_focused = ui.memory(|m| m.focused().is_none());
    if response.clicked() || response.drag_started() || nothing_focused {
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

    if response.has_focus() {
        handle_input(ui, tab);
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let lines = (scroll / cell_h).round() as isize * 3;
            let target = (tab.term.scrollback() as isize + lines).max(0) as usize;
            tab.term.set_scrollback(target);
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
}

fn paint(ui: &egui::Ui, rect: Rect, tab: &Tab, font: &FontId, cell: Vec2, focused: bool) {
    let painter = ui.painter_at(rect);
    let background = tab.term.theme.background;
    painter.rect_filled(rect, 0.0, color(background));

    let (rows, cols) = tab.term.size();
    let mut run = String::new();
    for row in 0..rows {
        let y = rect.top() + f32::from(row) * cell.y;
        let mut col = 0;
        while col < cols {
            let Some(first) = tab.term.cell(row, col) else {
                break;
            };
            // Collect a run of cells sharing the same style.
            let start = col;
            run.clear();
            while col < cols {
                let Some(c) = tab.term.cell(row, col) else {
                    break;
                };
                if c.fg != first.fg
                    || c.bg != first.bg
                    || c.bold != first.bold
                    || c.underline != first.underline
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
            if first.bg != background {
                painter.rect_filled(run_rect, 0.0, color(first.bg));
            }
            if !run.trim().is_empty() {
                let fg = color(first.fg);
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

fn handle_input(ui: &egui::Ui, tab: &mut Tab) {
    let events = ui.input(|i| i.events.clone());
    let app_cursor = tab.term.application_cursor();
    for event in events {
        match event {
            egui::Event::Text(text) => tab.send(text.as_bytes()),
            egui::Event::Paste(text) => {
                let text = text.replace("\r\n", "\r").replace('\n', "\r");
                if tab.term.bracketed_paste() {
                    tab.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
                } else {
                    tab.send(text.as_bytes());
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
                    tab.send(&bytes);
                }
            }
            _ => {}
        }
    }
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
