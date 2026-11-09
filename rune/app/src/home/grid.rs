//! Ghostty's grid for full-screen programs, REPLs and shells without prompt marks
//! (`TerminalGridView` in GPUiX): every visible row, cell backgrounds, and the cursor. Keys go to
//! the PTY through `App::terminal_key`; wheel and pointer become reports when the program tracks
//! the mouse.

use super::view::rows_view;
use super::{Cells, Terminal};
use crate::app::App;
use crate::ghostty::{CursorShape, MouseAction};
use crate::ui::theme::rgb;
use rune::{Drag, El, Phase, Spring, Tx, WheelEvent, el};

/// The cursor glides between cells (a short Neovide-like trail).
const CURSOR_SPRING: Spring = Spring::new(0.08, 0.0);

pub fn view(app: &App, term: &Terminal) -> El {
    let p = app.pal;
    let cells = Cells::of(app);
    let s = app.ty.scale();
    // Full-screen programs get the whole pane; a REPL keeps the blocks' inset.
    let inset = if term.shell.alt() { 0.0 } else { cells.inset };
    let Some(screen) = &term.screen else { return el() };
    let mut grid = el().key("terminal-grid").w_full().h_full()
        .child(rows_view(app, &screen.rows, cells, |c| c).absolute(0.0, 0.0));
    if let Some(cursor) = screen.cursor {
        let (w, h, dy) = match cursor.shape {
            CursorShape::Block => (cells.cell, cells.row, 0.0),
            CursorShape::Beam => (2.0 * s, cells.row, 0.0),
            CursorShape::Underline => (cells.cell, 2.0 * s, cells.row - 2.0 * s),
        };
        let alpha = if cursor.shape == CursorShape::Block { 0.45 } else { 1.0 };
        grid = grid.child(el().key("terminal-cursor").w(w).h(h).fill(rgb(p.accent).alpha(alpha))
            .absolute(0.0, 0.0)
            .offset(cursor.col as f32 * cells.cell, cursor.row as f32 * cells.row + dy)
            .offset_motion(CURSOR_SPRING));
    }
    let cell_at = move |x: f32, y: f32| ((x / cells.cell).max(0.0) as u16, (y / cells.row).max(0.0) as u16);
    grid = grid
        .on_wheel(move |a: &mut App, e: WheelEvent, _: &mut Tx| {
            // A notch is three rows; a trackpad moves by its own distance in rows.
            let rows = if e.precise { e.dy / cells.row } else { e.dy / 40.0 * 3.0 };
            let steps = rows.round() as i32;
            if steps == 0 { return true; }
            let (col, row) = cell_at(e.x, e.y);
            a.grid_wheel(steps, col, row);
            true
        })
        .on_drag(move |a: &mut App, d: &mut Drag, _: &mut Tx| {
            let action = match d.phase {
                Phase::Down => MouseAction::Press,
                Phase::Move => MouseAction::Motion,
                Phase::Up => MouseAction::Release,
            };
            let (col, row) = cell_at(d.x, d.y);
            a.grid_mouse(action, col, row);
        });
    el().w_full().h_full().pl(inset).pr(inset).child(grid)
}
