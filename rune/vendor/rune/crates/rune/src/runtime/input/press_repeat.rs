//! A held press whose drag handler asked to hear it again ([`Tx::drag_repeat_after`]): a frame
//! at or after the delay sends the handler the pointer as a `Phase::Move` its `Tx` marks
//! as the repeat.
use super::*;

/// The held press's element and when (clock seconds) its drag hears the pointer again.
#[derive(Default)]
pub(super) struct PressRepeat { due: Option<(String, f64)> }

impl<S: 'static> Runtime<S> {
    /// After `path`'s drag handler asked for a repeat `after` from now: due then, while the
    /// press it heard is still held.
    pub(super) fn arm_press_repeat(&mut self, path: &str, after: std::time::Duration) {
        if self.press.path() != Some(path) { return; }
        self.press_repeat.due = Some((path.to_owned(), self.clock + after.as_secs_f64()));
        self.schedule_wakes([after]);
    }

    /// The release or a cancel ends the repeat.
    pub(super) fn end_press_repeat(&mut self) { self.press_repeat.due = None; }

    pub(super) fn press_repeat_waiting(&self) -> bool {
        self.press_repeat.due.as_ref().is_some_and(|(_, due)| self.clock + 1e-6 < *due)
    }

    /// Sends the repeat due by this frame to the drag still holding its press.
    pub(super) fn fire_press_repeat(&mut self) {
        if self.press_repeat.due.is_none() || self.press_repeat_waiting() { return; }
        let Some((path, _)) = self.press_repeat.due.take() else { return };
        let Some(held) = self.press.held().filter(|h| h.path == path) else { return };
        let (d, at, button) = (held.drag, held.at, held.button);
        let Some(i) = self.live_index(&path) else { return };
        let (x, y) = self.pointer.unwrap_or(at);
        let [lx, ly] = self.to_local(i, x, y);
        let (dx, dy) = (x - at.0, y - at.1);
        self.queue.push(Action::DragRepeat(path, Drag { phase: Phase::Move, x: lx, y: ly, dx, dy, window: [x, y], screen: self.screen_at(x, y), ..d }, button));
    }
}
