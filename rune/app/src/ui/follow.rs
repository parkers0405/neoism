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

/// An app-stepped scroll glide: a critically damped spring of `SPRING`'s response, advanced
/// on the frame clock and placed with `Tx::scroll_to_instant`, so it animates even in commits
/// Rune would land at once (a focused composer under a chat).
#[derive(Clone, Copy, Debug, Default)]
pub struct Glide {
    pub pos: f32,
    pub vel: f32,
    pub target: f32,
    clock: Option<f64>,
}

impl Glide {
    /// Aim at `target` from where the view is (`from`), keeping a running glide's speed.
    pub fn retarget(current: Option<Glide>, from: f32, target: f32) -> Glide {
        let (pos, vel, clock) = current.map_or((from, 0.0, None), |g| (g.pos, g.vel, g.clock));
        Glide { pos, vel, target, clock }
    }

    /// Advance to `clock` (seconds); `false` once it has come to rest on its target.
    pub fn step(&mut self, clock: f64) -> bool {
        let dt = self.clock.map_or(1.0 / 120.0, |last| (clock - last).clamp(0.0, 0.05)) as f32;
        self.clock = Some(clock);
        // Response 0.39 s ⇒ ω = 2π / 0.39; critically damped, integrated in small substeps.
        let omega = std::f32::consts::TAU / 0.39;
        let steps = ((dt / 0.004).ceil() as usize).max(1);
        let h = dt / steps as f32;
        for _ in 0..steps {
            let accel = -2.0 * omega * self.vel - omega * omega * (self.pos - self.target);
            self.vel += accel * h;
            self.pos += self.vel * h;
        }
        if (self.pos - self.target).abs() < 0.25 && self.vel.abs() < 2.0 {
            self.pos = self.target;
            return false;
        }
        true
    }
}

/// Trackpad gain on vertical reading surfaces (`ui/wheel.ts` `TOUCHPAD_SCROLL_GAIN`).
pub const TOUCHPAD_GAIN: f32 = 2.0;

/// The original precision-trackpad scroll (`scroll_timeline_pixels_with_inertia`) moved 1:1 and
/// coasted on: each delta added `delta x 7` px/s of velocity that decayed with tau 0.28 s, i.e.
/// about `7 x 0.28` more of the delta after the fingers lift. Here a trackpad delta extends the
/// glide's target by that whole distance, and Rune glides there on its own clock
/// (`Tx::scroll_to_with`): no app-stepped scroll, so virtual rows keep their anchor.
pub const TRACKPAD_REACH: f32 = 1.0 + 7.0 * 0.28;
/// The trackpad glide: quick enough to read as direct, long enough to carry the coast.
pub const TRACKPAD_SPRING: Spring = Spring::new(0.28, 0.0);

/// A wheel target still in flight (set within the last `TARGET_LIFE` and not reached), else
/// where the view is: consecutive deltas extend one glide rather than restarting from mid-flight.
pub fn wheel_base(target: Option<(f32, Instant)>, offset: f32) -> f32 {
    target.filter(|(t, at)| at.elapsed().as_secs_f32() < TARGET_LIFE && (t - offset).abs() > 0.5).map_or(offset, |(t, _)| t)
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
    fn lists_keep_their_margin() {
        let m = Reveal::Margin(2);
        assert_eq!(reveal_top(5, 8, 20, 0, m), 0);
        assert_eq!(reveal_top(6, 8, 20, 0, m), 1);
        assert_eq!(reveal_top(1, 8, 20, 3, m), 0);
    }
}
