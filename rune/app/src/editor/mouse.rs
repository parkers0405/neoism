//! Mouse selection and edge scrolling use the same wrapped-row hit test as the code grid.
use super::{
    document::Mode,
    view,
    vim::{self, Pos},
};
use crate::app::App;
use rune::{Drag, Phase, Tx};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Mouse {
    drag: Option<Selection>,
    tick: Option<Instant>,
}
struct Selection {
    point: [f32; 2],
    anchor: (Pos, Pos),
    clicks: u32,
}

fn span(line: &str, col: usize) -> (usize, usize) {
    let col = col.min(line.len());
    let class = |c: char| {
        if c.is_whitespace() {
            0
        } else if c.is_alphanumeric() || c == '_' {
            1
        } else {
            2
        }
    };
    let kind = line[col..].chars().next().map(class);
    let start = line[..col]
        .char_indices()
        .rev()
        .take_while(|(_, c)| Some(class(*c)) == kind)
        .last()
        .map_or(col, |(i, _)| i);
    let end = line[col..]
        .char_indices()
        .take_while(|(_, c)| Some(class(*c)) == kind)
        .last()
        .map_or(col, |(i, c)| col + i + c.len_utf8());
    (start, vim::prev_char_boundary(line, end).max(start))
}

impl App {
    pub fn editor_drag(&mut self, d: &Drag, tx: &mut Tx) {
        if d.phase == Phase::Up {
            if let Some(b) = self.active_buffer_mut() {
                b.mouse.drag = None;
            }
            return;
        }
        let point = d.window_point();
        let Some((line, col)) = view::hit_window(self, point[0], point[1], false) else {
            return;
        };
        self.focus = crate::app::Focus::Content;
        if d.phase == Phase::Down {
            self.editor_click(line, col, false);
            let Some(b) = self.active_buffer_mut() else {
                return;
            };
            let (start, end) = if d.clicks == 2 {
                span(&b.doc.lines[line], col)
            } else if d.clicks >= 3 {
                (0, b.doc.lines[line].len())
            } else {
                (col, col)
            };
            let anchor = (Pos { line, col: start }, Pos { line, col: end });
            if d.clicks >= 2 {
                b.doc.mode = Mode::Visual;
                b.doc.visual_anchor = Some(anchor.0);
                b.doc.vim.visual_linewise = d.clicks >= 3;
                b.doc.vim.visual_block = false;
                b.doc.set_cursor(anchor.1);
            }
            b.mouse.drag = Some(Selection {
                point: d.window_point(),
                anchor,
                clicks: d.clicks,
            });
            b.mouse.tick = Some(Instant::now());
        } else {
            if let Some(b) = self.active_buffer_mut() {
                if let Some(drag) = &mut b.mouse.drag {
                    drag.point = d.window_point();
                }
            }
            self.editor_extend(line, col);
        }
        tx.wake_after(Duration::from_millis(16));
    }

    fn editor_extend(&mut self, line: usize, col: usize) {
        let Some(b) = self.active_buffer_mut() else {
            return;
        };
        let Some(drag) = &b.mouse.drag else { return };
        let pos = Pos { line, col };
        if drag.clicks == 1 && pos == drag.anchor.0 && b.doc.mode != Mode::Visual {
            return;
        }
        let (start, end) = if drag.clicks == 2 {
            span(&b.doc.lines[line], col)
        } else {
            (col, col)
        };
        let backwards = pos < drag.anchor.0;
        b.doc.mode = Mode::Visual;
        b.doc.vim.visual_linewise = drag.clicks >= 3;
        b.doc.vim.visual_block = false;
        b.doc.visual_anchor = Some(if backwards {
            drag.anchor.1
        } else {
            drag.anchor.0
        });
        b.doc.set_cursor(Pos {
            line,
            col: if backwards { start } else { end },
        });
    }

    pub fn editor_mouse_tick(&mut self, tx: &mut Tx) -> bool {
        let Some(rect) = self.active_buffer().and_then(|b| b.view_rect) else {
            return false;
        };
        let Some(b) = self.active_buffer_mut() else {
            return false;
        };
        let Some(drag) = &b.mouse.drag else {
            return false;
        };
        let point = drag.point;
        let now = Instant::now();
        let dt = b
            .mouse
            .tick
            .replace(now)
            .map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.05));
        let path = b.path.clone();
        let outside = if point[1] < rect[1] {
            point[1] - rect[1]
        } else if point[1] > rect[1] + rect[3] {
            point[1] - rect[1] - rect[3]
        } else {
            0.0
        };
        tx.wake_after(Duration::from_millis(16));
        if outside == 0.0 {
            return false;
        }
        let (offset, max, _) = self.active_buffer().unwrap().scroll.get();
        let target = (offset
            + outside.signum() * (outside.abs() * 12.0).clamp(80.0, 1600.0) * dt)
            .clamp(0.0, max);
        tx.scroll_to(view::scroll_key(&self.workspace_id, &path), 0.0, target);
        self.active_buffer_mut().unwrap().wheel_target = None;
        if let Some((line, col)) = view::hit(
            self,
            point[0] - rect[0],
            (point[1] - rect[1]).clamp(0.0, rect[3] - 1.0) + target,
        ) {
            self.editor_extend(line, col);
        }
        target != offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word_selection_respects_unicode_and_punctuation() {
        assert_eq!(span("let café = 1", 6), (4, 7));
        assert_eq!(span("foo::bar", 3), (3, 4));
        assert_eq!(span("a  b", 1), (1, 2));
    }
}
