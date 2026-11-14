//! `Tx`'s held-press repeat: a drag handler hears a press it holds still again, as AppKit
//! repeats a held scroll bar track or stepper.

use std::time::Duration;

use crate::Tx;

impl Tx {
    /// From a drag handler: while this press stays held, send the handler the pointer again as
    /// a [`Phase::Move`](crate::Phase::Move) after `delay`, moved or not. [`Tx::drag_repeat`]
    /// tells that move from the hand's. Ask again from the repeat, with the repeat interval, to
    /// keep going; releasing or cancelling the press ends it.
    ///
    /// ```ignore
    /// .on_drag_any(|d, tx| {
    ///     if d.phase == Phase::Down || tx.drag_repeat() {
    ///         tx.scroll_page("list", 1);
    ///         tx.drag_repeat_after(Duration::from_millis(if tx.drag_repeat() { 60 } else { 300 }));
    ///     }
    /// })
    /// ```
    pub fn drag_repeat_after(&mut self, delay: Duration) { self.drag_repeat_after = Some(delay); }

    /// Whether the drag handler is hearing the repeat it asked for with
    /// [`Tx::drag_repeat_after`]; false for the hand's moves and outside drag handlers.
    pub fn drag_repeat(&self) -> bool { self.drag_repeat }
}
