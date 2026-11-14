//! Counting a run of presses into [`Drag::clicks`], as AppKit's `clickCount` does.
use super::*;

/// The last press: its element, time (clock seconds), window point and count.
#[derive(Default)]
pub(super) struct PressClicks { last: Option<(String, f64, f32, f32, u32)> }

impl PressClicks {
    /// Counts a press on `path` at `now`: one more than the last press when that was on the same
    /// element, within `interval` of it and within the click slop, else 1.
    pub(super) fn press(&mut self, path: &str, now: f64, (x, y): (f32, f32), interval: f64) -> u32 {
        let clicks = match &self.last {
            Some((p, t, px, py, n)) if p == path && now - t <= interval && (x - px).abs() <= CLICK_SLOP && (y - py).abs() <= CLICK_SLOP => n + 1,
            _ => 1,
        };
        self.last = Some((path.to_owned(), now, x, y, clicks));
        clicks
    }

    /// A press that dragged, another button's press or a reset ends the run.
    pub(super) fn clear(&mut self) { self.last = None; }
}
