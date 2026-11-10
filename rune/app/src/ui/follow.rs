//! Keeping the keyboard's row in view (`editor/code/viewport.rs`, `command_palette::fuzzy::
//! scrolloff_for`). Code centre-locks the cursor row mid-view and only falls back to a small
//! edge margin where centring would pull past the top or bottom; lists keep a fixed margin of
//! rows. Everything works in whole rows, so an unchanged wanted row never moves the view.
//!
//! A held key keeps extending the glide's *target* (not its mid-flight offset), so the view
//! never falls behind the cursor; a jump past 1.25 viewports lands at once.

use rune::{Spring, Tx};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

/// The original scroll spring: critically damped, ~0.39 s.
pub const SPRING: Spring = Spring::new(0.39, 0.0);
/// Menu lists (`command_palette` / `finder` `LIST_SCROLL_ANIMATION_LENGTH` = 0.30 s): a
/// critically damped spring of omega 4 / 0.30, i.e. a zero-bounce Rune spring of 2π/omega.
pub const LIST_SPRING: Spring = Spring::new(0.471, 0.0);
/// Their selection highlight (`CURSOR_ANIMATION_LENGTH` = 0.12 s): omega 4 / 0.12.
pub const CURSOR_SPRING: Spring = Spring::new(0.188, 0.0);
/// How long a set target still counts as the glide's destination.
const TARGET_LIFE: f32 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// Pin the row mid-view; `edge` rows of margin where the buffer's ends stop centring.
    Center { edge: i64 },
    /// Keep this many rows between the row and either edge.
    Margin(i64),
}

/// The top row that reveals `row` in a `rows_fit`-row view now topped at `current_top`.
pub fn reveal_top(row: i64, rows_fit: i64, max_top: i64, current_top: i64, reveal: Reveal) -> i64 {
    let rows_fit = rows_fit.max(1);
    let edge_margin = |margin: i64| {
        let margin = margin.min((rows_fit - 1) / 2).max(0);
        if row < current_top + margin { (row - margin).clamp(0, max_top) }
        else if row > current_top + rows_fit - 1 - margin { (row + margin - rows_fit + 1).clamp(0, max_top) }
        else { current_top.clamp(0, max_top) }
    };
    match reveal {
        Reveal::Center { edge } => {
            let centered = row - (rows_fit - 1) / 2;
            let top = centered.clamp(0, max_top);
            if top == centered { top } else { edge_margin(edge) }
        }
        Reveal::Margin(margin) => edge_margin(margin),
    }
}

/// One scroll view that follows a keyboard row: its mirrored scroll state and pending target.
#[derive(Default)]
pub struct Follow {
    /// (offset, max, viewport) as the scroll view last reported.
    pub scroll: Rc<Cell<(f32, f32, f32)>>,
    target: Option<(f32, Instant)>,
    /// A keyboard move asked for the row to be revealed on the next frame.
    pub pending: bool,
    /// Wheel distance not yet worth a whole row (row-stepped lists).
    wheel: f32,
}

impl Follow {
    pub fn request(&mut self) { self.pending = true; }

    /// The offset a glide is heading to, else where the view is.
    pub fn base(&self) -> f32 {
        let offset = self.scroll.get().0;
        self.target.filter(|(t, at)| at.elapsed().as_secs_f32() < TARGET_LIFE && (t - offset).abs() > 0.5)
            .map_or(offset, |(t, _)| t)
    }

    /// Record a target set elsewhere (a wheel notch), so key follow continues from it.
    pub fn set_target(&mut self, target: f32) { self.target = Some((target, Instant::now())); }

    /// Reveal `row` (uniform `row_h` rows, `content_h` tall) in the scroll view `key`, if a move
    /// asked for it. Waits (stays pending) until the view has a height.
    pub fn apply(&mut self, key: &str, row: usize, row_h: f32, content_h: f32, reveal: Reveal, tx: &mut Tx) {
        self.apply_with(key, row, row_h, content_h, reveal, SPRING, true, tx);
    }

    /// [`Self::apply`] on `spring`; without `teleport` even a long jump glides (the menus'
    /// list springs accumulate whole rows however far the selection went).
    #[allow(clippy::too_many_arguments)]
    pub fn apply_with(&mut self, key: &str, row: usize, row_h: f32, content_h: f32, reveal: Reveal, spring: Spring, teleport: bool, tx: &mut Tx) {
        if !self.pending || row_h <= 0.0 { return; }
        let (offset, _, viewport) = self.scroll.get();
        if viewport <= 0.0 { return; }
        self.pending = false;
        let rows_fit = (viewport / row_h).floor().max(1.0) as i64;
        let max_top = ((content_h - viewport).max(0.0) / row_h).ceil() as i64;
        let max_offset = (content_h - viewport).max(0.0);
        let base = self.base();
        let current_top = (base / row_h).round() as i64;
        let top = reveal_top(row as i64, rows_fit, max_top, current_top, reveal);
        if top == current_top { return; }
        let target = (top as f32 * row_h).clamp(0.0, max_offset);
        self.set_target(target);
        if teleport && (target - offset).abs() > viewport * 1.25 { tx.scroll_to_y(key.to_owned(), target); }
        else { tx.scroll_to_with(key.to_owned(), 0.0, target, spring); }
    }

    /// A wheel notch over a row-stepped list (`scroll_pixels`): the distance accumulates into
    /// whole rows and the list glides there on `spring`. Returns the new top row and the rows
    /// that fit, so the caller can keep its selection inside them; `None` when nothing moved.
    pub fn wheel_rows(&mut self, key: &str, dy: f32, row_h: f32, content_h: f32, spring: Spring, tx: &mut Tx) -> Option<(usize, usize)> {
        let viewport = self.scroll.get().2;
        if viewport <= 0.0 || row_h <= 0.0 || content_h <= viewport + 0.5 { return None; }
        self.wheel += dy;
        let mut rows = 0i64;
        while self.wheel.abs() >= row_h {
            let sign = self.wheel.signum();
            self.wheel -= sign * row_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }
        if rows == 0 { return None; }
        let rows_fit = (viewport / row_h).floor().max(1.0) as usize;
        let max_offset = (content_h - viewport).max(0.0);
        let max_top = (max_offset / row_h).ceil() as i64;
        let current_top = (self.base() / row_h).round() as i64;
        let top = (current_top + rows).clamp(0, max_top);
        if top != current_top {
            let target = (top as f32 * row_h).clamp(0.0, max_offset);
            self.set_target(target);
            tx.scroll_to_with(key.to_owned(), 0.0, target, spring);
        }
        Some((top as usize, rows_fit))
    }
}

/// The selection a wheel step leaves inside the rows `top..top + fit`, `scrolloff` rows from
/// either edge (`clamp_selected_to_viewport`).
pub fn clamp_into_view(selected: usize, count: usize, top: usize, fit: usize, scrolloff: usize) -> usize {
    if count == 0 || fit == 0 { return selected; }
    let scrolloff = scrolloff.min(fit.saturating_sub(1) / 2);
    let first = (top + scrolloff).min(count - 1);
    let last = (top + fit.saturating_sub(1).saturating_sub(scrolloff)).min(count - 1);
    selected.clamp(first, last.max(first))
}

impl Follow {
    /// Reveal a span `top..top + h` of content (rows of uneven height) keeping `margin` points
    /// to either edge where the view allows.
    pub fn apply_span(&mut self, key: &str, top: f32, h: f32, margin: f32, tx: &mut Tx) {
        if !self.pending { return; }
        let (offset, max, viewport) = self.scroll.get();
        if viewport <= 0.0 { return; }
        self.pending = false;
        let margin = margin.min(((viewport - h) / 2.0).max(0.0));
        let base = self.base();
        let target = if top - margin < base { top - margin }
            else if top + h + margin > base + viewport { top + h + margin - viewport }
            else { return };
        let target = target.clamp(0.0, max.max(0.0));
        self.set_target(target);
        if (target - offset).abs() > viewport * 1.25 { tx.scroll_to_y(key.to_owned(), target); }
        else { tx.scroll_to_with(key.to_owned(), 0.0, target, SPRING); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_centres_until_the_ends() {
        let c = Reveal::Center { edge: 4 };
        // 21 rows fit: row 50 sits at the middle (top 40).
        assert_eq!(reveal_top(50, 21, 200, 0, c), 40);
        // Near the top centring would go negative: minimal scroll with a 4-row edge.
        assert_eq!(reveal_top(3, 21, 200, 0, c), 0);
        // Near the end it stops at max_top.
        assert_eq!(reveal_top(215, 21, 200, 180, c), 199);
    }

    #[test]
    fn a_wheel_step_keeps_the_selection_in_its_band() {
        // 8 rows fit with 2 of scrolloff: the list moved to row 10, so row 3 is pulled to 12.
        assert_eq!(clamp_into_view(3, 40, 10, 8, 2), 12);
        assert_eq!(clamp_into_view(30, 40, 10, 8, 2), 15);
        assert_eq!(clamp_into_view(13, 40, 10, 8, 2), 13);
        assert_eq!(clamp_into_view(4, 5, 0, 8, 2), 4);
    }

    #[test]
    fn lists_keep_their_margin() {
        let m = Reveal::Margin(2);
        assert_eq!(reveal_top(5, 8, 20, 0, m), 0);
        assert_eq!(reveal_top(6, 8, 20, 0, m), 1);
        assert_eq!(reveal_top(1, 8, 20, 3, m), 0);
    }
}
