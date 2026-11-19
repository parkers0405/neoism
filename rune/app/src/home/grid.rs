//! Ghostty's grid for full-screen programs, REPLs and shells without prompt marks
//! (`TerminalGridView` in GPUiX): every visible row, cell backgrounds, and the cursor. Keys go to
//! the PTY through `App::terminal_key`; wheel and pointer become reports when the program tracks
//! the mouse.

use super::view::rows_view;
use super::{Cells, Terminal};
use crate::app::App;
use crate::ghostty::{CursorShape, MouseAction, MouseButton, MouseInput};
use crate::ui::theme::rgb;
use rune::{Drag, El, Phase, Spring, Tx, WheelEvent, el};

/// The cursor glides between cells (a short Neovide-like trail).
const CURSOR_SPRING: Spring = Spring::new(0.08, 0.0);

/// Protocol scrolling is discrete, but input is not. This state belongs to the terminal,
/// not the wheel callback (which is rebuilt with the view). It generates no momentum.
#[derive(Default)]
pub(super) struct GridWheel {
    residual: f32,
    session: Option<String>,
    modes: u8,
}

impl Terminal {
    pub(super) fn sync_grid_wheel(&mut self) {
        let modes = u8::from(self.raw()) | u8::from(self.shell.mouse_tracking()) << 1
            | u8::from(self.shell.alt()) << 2 | u8::from(self.shell.alternate_scroll()) << 3;
        if self.grid_wheel.session != self.shell.session || self.grid_wheel.modes != modes {
            self.grid_wheel.residual = 0.0;
            self.grid_wheel.session.clone_from(&self.shell.session);
            self.grid_wheel.modes = modes;
        }
    }

    pub(super) fn grid_wheel_bytes(&mut self, e: WheelEvent, row_height: f32, col: u16, row: u16) -> Vec<u8> {
        self.sync_grid_wheel();
        if self.shell.session.is_none() || !(self.shell.mouse_tracking() || self.shell.alt() && self.shell.alternate_scroll()) {
            self.grid_wheel.residual = 0.0;
            return Vec::new();
        }
        let steps = self.grid_wheel.steps(e, row_height);
        // Rune's positive dy moves content down: wheel-up (button 4), not wheel-down.
        let button = if steps > 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
        let mut bytes = Vec::new();
        for _ in 0..steps.unsigned_abs().min(200) {
            let report = self.shell.encode_mouse(&MouseInput { action: MouseAction::Press, button: Some(button), col, row, ctrl: false, alt: false });
            match report {
                Some(report) => bytes.extend(report),
                None if self.shell.alt() && self.shell.alternate_scroll() => {
                    let key = if steps > 0 { "up" } else { "down" };
                    bytes.extend(self.shell.encode_key(&crate::ghostty::KeyInput { key, text: "", shift: false, ctrl: false, alt: false, logo: false }));
                }
                None => break,
            }
        }
        bytes
    }
}

impl GridWheel {
    fn steps(&mut self, e: WheelEvent, row_height: f32) -> i32 {
        if !e.precise {
            self.residual = 0.0;
            // Rune normalizes one line/notch to 40 points. Preserve three rows per notch.
            return (e.dy / 40.0 * 3.0).round() as i32;
        }
        if e.phase == rune::GesturePhase::Began { self.residual = 0.0; }
        if e.dy == 0.0 || !e.dy.is_finite() || !row_height.is_finite() || row_height <= 0.0 { return 0; }
        let rows = e.dy / row_height;
        // Reversing must not first pay off motion left over in the opposite direction.
        if self.residual * rows < 0.0 { self.residual = 0.0; }
        let total = self.residual + rows;
        let whole = total.trunc();
        self.residual = total - whole;
        whole as i32
    }
}

pub fn view(app: &App, term: &Terminal) -> El {
    let p = app.pal;
    let cells = Cells::of(app);
    let s = app.ty.scale();
    // Full-screen programs get the whole pane; a REPL keeps the blocks' inset.
    let inset = if term.shell.alt() { 0.0 } else { cells.inset };
    let Some(screen) = &term.screen else { return el() };
    let mut grid = el().key("terminal-grid").w_full().h_full()
        .child(rows_view(app, &screen.rows, cells, |c| c, !term.shell.mouse_tracking()).absolute(0.0, 0.0));
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
    // The tab this grid belongs to: in a split, the pane under the pointer, not the focused one.
    let tab_id = app.active_tab().id;
    let cell_at = move |x: f32, y: f32| ((x / cells.cell).max(0.0) as u16, (y / cells.row).max(0.0) as u16);
    grid = grid
        .on_wheel(move |a: &mut App, e: WheelEvent, _: &mut Tx| {
            let (col, row) = cell_at(e.x, e.y);
            a.grid_wheel(tab_id, e, cells.row, col, row);
            true
        });
    if term.shell.mouse_tracking() {
        grid = grid.on_drag(move |a: &mut App, d: &mut Drag, _: &mut Tx| {
            let action = match d.phase {
                Phase::Down => MouseAction::Press,
                Phase::Move => MouseAction::Motion,
                Phase::Up => MouseAction::Release,
            };
            let (col, row) = cell_at(d.x, d.y);
            a.grid_mouse(tab_id, action, col, row);
        });
    }
    el().w_full().h_full().pl(inset).pr(inset).child(grid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn precise(dy: f32) -> WheelEvent { WheelEvent::new(0.0, dy).precise(true) }

    fn terminal() -> Terminal {
        let app = App::new();
        let mut term = Terminal::new("/".into(), app.terminal_colors());
        assert!(term.shell.error.is_none(), "{:?}", term.shell.error);
        term.shell.attach("wheel-test".into());
        term.grid = true;
        // A TUI may track the mouse on the normal screen too (not just the alternate screen).
        term.shell.feed(b"\x1b[?1000h\x1b[?1006h", None);
        assert!(!term.shell.alt());
        term
    }

    #[test]
    fn precise_events_accumulate_across_syncs_into_directional_mouse_reports() {
        let mut term = terminal();
        for _ in 0..7 {
            assert!(term.grid_wheel_bytes(precise(3.0), 24.0, 2, 3).is_empty());
            term.sync(); // Rebuilding the view must not lose the residual.
        }
        assert!(term.grid_wheel_bytes(precise(0.0), 24.0, 2, 3).is_empty());
        assert_eq!(term.grid_wheel_bytes(precise(3.0), 24.0, 2, 3), b"\x1b[<64;3;4M");
        assert_eq!(term.grid_wheel_bytes(precise(-24.0), 24.0, 2, 3), b"\x1b[<65;3;4M");
    }

    #[test]
    fn reversal_drops_old_direction_and_retains_new_fraction() {
        let mut wheel = GridWheel::default();
        assert_eq!(wheel.steps(precise(18.0), 24.0), 0);
        assert_eq!(wheel.steps(precise(-18.0), 24.0), 0);
        assert_eq!(wheel.steps(precise(-6.0), 24.0), -1);
        assert_eq!(wheel.steps(precise(-60.0), 24.0), -2);
        assert_eq!(wheel.steps(precise(-12.0), 24.0), -1);
    }

    #[test]
    fn notches_keep_three_rows_and_do_not_inherit_precise_residual() {
        let mut wheel = GridWheel::default();
        assert_eq!(wheel.steps(precise(18.0), 24.0), 0);
        assert_eq!(wheel.steps(WheelEvent::new(0.0, 40.0), 24.0), 3);
        assert_eq!(wheel.steps(WheelEvent::new(0.0, -80.0), 24.0), -6);
        assert_eq!(wheel.steps(precise(6.0), 24.0), 0);
        assert_eq!(wheel.steps(precise(18.0).phase(rune::GesturePhase::Began), 24.0), 0);
    }

    #[test]
    fn terminals_sessions_and_modes_do_not_share_residual() {
        let mut a = terminal();
        let mut b = terminal();
        assert!(a.grid_wheel_bytes(precise(18.0), 24.0, 0, 0).is_empty());
        assert!(b.grid_wheel_bytes(precise(6.0), 24.0, 0, 0).is_empty());
        assert_eq!(a.grid_wheel_bytes(precise(6.0), 24.0, 0, 0), b"\x1b[<64;1;1M");
        a.grid_wheel_bytes(precise(18.0), 24.0, 0, 0);
        a.shell.feed(b"\x1b[?1000l", None);
        a.sync();
        assert!(a.grid_wheel_bytes(precise(24.0), 24.0, 0, 0).is_empty());
        a.shell.feed(b"\x1b[?1000h", None);
        assert!(a.grid_wheel_bytes(precise(6.0), 24.0, 0, 0).is_empty());
        a.shell.attach("replacement".into());
        a.shell.feed(b"\x1b[?1000h\x1b[?1006h", None);
        assert!(a.grid_wheel_bytes(precise(18.0), 24.0, 0, 0).is_empty());
    }

    #[test]
    fn alternate_scroll_accumulates_into_arrows_without_mouse_tracking() {
        let mut term = terminal();
        term.shell.feed(b"\x1b[?1000l\x1b[?1049h\x1b[?1007h\x1b[?1l", None);
        assert!(term.grid_wheel_bytes(precise(12.0), 24.0, 0, 0).is_empty());
        assert_eq!(term.grid_wheel_bytes(precise(12.0), 24.0, 0, 0), b"\x1b[A");
        assert_eq!(term.grid_wheel_bytes(precise(-24.0), 24.0, 0, 0), b"\x1b[B");
    }
}
