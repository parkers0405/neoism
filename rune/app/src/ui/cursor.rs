//! The original cursors: a block caret in text fields (`0.6 em` wide, `prompt_caret_rect`) and
//! the keyboard-focus bar on tabs, both animated by Neovide's trail
//! (`shared/src/panels/trail_cursor.rs`: four corner springs, leading corners faster than
//! trailing ones) and blinking on the desktop's half-period clock (`ui/cursor-blink.ts`).

use std::time::{Duration, Instant};

const ANIMATION_LENGTH: f32 = 0.10;
const SHORT_ANIMATION_LENGTH: f32 = 0.04;
const TRAIL_SIZE: f32 = 0.8;
/// Half period: visible for one interval, hidden for the next.
pub const BLINK_INTERVAL: Duration = Duration::from_millis(530);
/// How long the cursor may be targetless and still trail from its old place when it returns.
const HIDDEN_GRACE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Default)]
struct Spring {
    position: f32,
    velocity: f32,
}

impl Spring {
    /// Critically damped toward zero over `length` seconds; false once settled.
    fn update(&mut self, dt: f32, length: f32) -> bool {
        if length <= dt || self.position == 0.0 {
            *self = Spring::default();
            return false;
        }
        let omega = 4.0 / length;
        let (a, b) = (self.position, self.position * omega + self.velocity);
        let c = (-omega * dt).exp();
        self.position = (a + b * dt) * c;
        self.velocity = c * (-a * omega - b * dt * omega + b);
        if self.position.abs() < 0.01 { *self = Spring::default(); false } else { true }
    }
}

#[derive(Clone, Copy)]
struct Corner {
    x: Spring,
    y: Spring,
    pos: [f32; 2],
    rel: [f32; 2],
    previous_destination: [f32; 2],
    length: f32,
}

impl Corner {
    fn new(rel: [f32; 2]) -> Corner {
        Corner { x: Spring::default(), y: Spring::default(), pos: [0.0; 2], rel, previous_destination: [-1e6; 2], length: ANIMATION_LENGTH }
    }

    fn destination(&self, center: [f32; 2], size: [f32; 2]) -> [f32; 2] {
        [center[0] + self.rel[0] * size[0], center[1] + self.rel[1] * size[1]]
    }

    fn update(&mut self, center: [f32; 2], size: [f32; 2], dt: f32, immediate: bool) -> bool {
        let dest = self.destination(center, size);
        if (dest[0] - self.previous_destination[0]).abs() > 0.01 || (dest[1] - self.previous_destination[1]).abs() > 0.01 {
            self.x.position = dest[0] - self.pos[0];
            self.y.position = dest[1] - self.pos[1];
            self.previous_destination = dest;
        }
        if immediate {
            self.pos = dest;
            self.x = Spring::default();
            self.y = Spring::default();
            return false;
        }
        let moving = self.x.update(dt, self.length) | self.y.update(dt, self.length);
        self.pos = [dest[0] - self.x.position, dest[1] - self.y.position];
        moving
    }

    /// How far this corner points along the direction of travel.
    fn alignment(&self, center: [f32; 2], size: [f32; 2]) -> f32 {
        let dest = self.destination(center, size);
        let rel_len = self.rel[0].hypot(self.rel[1]).max(1e-6);
        let (dx, dy) = (dest[0] - self.pos[0], dest[1] - self.pos[1]);
        let travel = dx.hypot(dy).max(1e-6);
        dx / travel * self.rel[0] / rel_len + dy / travel * self.rel[1] / rel_len
    }
}

/// One animated cursor: a target rect it trails toward, and the quad drawn this frame.
pub struct Trail {
    corners: [Corner; 4],
    center: [f32; 2],
    size: [f32; 2],
    previous_center: [f32; 2],
    jump_from: [f32; 2],
    jumped: bool,
    first: bool,
    animating: bool,
    last_tick: Option<Instant>,
    /// When the target last moved: the cursor stays lit for a blink interval after.
    pub moved_at: Option<Instant>,
}

impl Default for Trail {
    fn default() -> Trail {
        Trail {
            corners: [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]].map(Corner::new),
            center: [0.0; 2],
            size: [0.0; 2],
            previous_center: [-1e6; 2],
            jump_from: [-1e6; 2],
            jumped: false,
            first: true,
            animating: false,
            last_tick: None,
            moved_at: None,
        }
    }
}

impl Trail {
    pub fn quad(&self) -> [[f32; 2]; 4] { self.corners.map(|c| c.pos) }

    /// Carry the cursor with what scrolled under it: the target and every corner move by
    /// `by` at once, at rest or mid-trail, and no new trail starts.
    pub fn shift(&mut self, by: [f32; 2]) {
        for axis in 0..2 {
            for corner in &mut self.corners {
                corner.pos[axis] += by[axis];
                corner.previous_destination[axis] += by[axis];
            }
            self.center[axis] += by[axis];
            self.previous_center[axis] += by[axis];
            self.jump_from[axis] += by[axis];
        }
    }
    pub fn animating(&self) -> bool { self.animating }

    /// Aim at `rect` (`[x, y, w, h]`); a changed target starts a trail from where it is now.
    pub fn target(&mut self, rect: [f32; 4]) {
        let center = [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5];
        self.size = [rect[2], rect[3]];
        self.center = center;
        if (center[0] - self.previous_center[0]).abs() > 0.01 || (center[1] - self.previous_center[1]).abs() > 0.01 {
            self.jump_from = self.previous_center;
            self.previous_center = center;
            self.moved_at = Some(Instant::now());
            self.jumped = true;
        }
    }

    /// Jump straight to the target (first sight, a field that moved without its caret moving).
    pub fn snap(&mut self) {
        for corner in &mut self.corners { corner.update(self.center, self.size, 0.0, true); }
        self.jumped = false;
        self.first = false;
        self.animating = false;
    }

    /// Advance the springs to `now`. Returns whether the cursor is still moving.
    pub fn tick(&mut self, now: Instant) -> bool {
        let dt = if self.animating { self.last_tick.map_or(0.0, |t| now.duration_since(t).as_secs_f32()) } else { 0.0 }.clamp(0.0, 0.05);
        self.last_tick = Some(now);
        let immediate = std::mem::take(&mut self.first);
        if self.jumped && !immediate { self.plan_jump(); }
        self.jumped = false;
        let (center, size) = (self.center, self.size);
        self.animating = self.corners.iter_mut().fold(false, |moving, c| c.update(center, size, dt, immediate) | moving);
        self.animating
    }

    /// Corners leading the move settle fastest; a short horizontal hop (typing) is quicker.
    fn plan_jump(&mut self) {
        let [w, h] = self.size;
        let jump_x = if w > 0.0 { ((self.center[0] - self.jump_from[0]) / w).abs() } else { 0.0 };
        let jump_y = if h > 0.0 { ((self.center[1] - self.jump_from[1]) / h).abs() } else { 0.0 };
        let base = if jump_x <= 2.001 && jump_y < 0.001 { SHORT_ANIMATION_LENGTH } else { ANIMATION_LENGTH };
        let mut order: Vec<(usize, f32)> = self.corners.iter().enumerate().map(|(i, c)| (i, c.alignment(self.center, self.size))).collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let leading = base * (1.0 - TRAIL_SIZE);
        let mid = (leading + base) / 2.0;
        for (rank, (i, _)) in order.into_iter().enumerate() {
            self.corners[i].length = match rank { 0 => base, 1 => mid, _ => leading };
        }
    }

    /// Lit while moving, for one interval after a move, then on even half-periods.
    pub fn blink_visible(&self, now: Instant, epoch: Instant) -> bool {
        if self.animating { return true; }
        if self.moved_at.is_some_and(|t| now.duration_since(t) < BLINK_INTERVAL) { return true; }
        (now.duration_since(epoch).as_millis() / BLINK_INTERVAL.as_millis()) % 2 == 0
    }

    /// When the blink state next changes.
    pub fn next_blink(&self, now: Instant, epoch: Instant) -> Duration {
        let interval = BLINK_INTERVAL.as_millis();
        let phase_left = interval - now.duration_since(epoch).as_millis() % interval;
        let hold_left = self.moved_at.map_or(0, |t| interval.saturating_sub(now.duration_since(t).as_millis()));
        Duration::from_millis(if hold_left > 0 { hold_left.min(phase_left) } else { phase_left } as u64 + 1)
    }
}

/// The one window cursor (`ChromeFocusCursor` + `ComposerCaret`): it sits on the focused tab,
/// workspace or sidebar row, and trails into the text caret when focus returns to content.
pub struct Cursor {
    /// Rune's caret line for the focused field, as the last view read it, and its font size.
    pub caret_line: std::cell::Cell<Option<[f32; 4]>>,
    pub caret_font: std::cell::Cell<f32>,
    pub trail: Trail,
    shown: Option<[f32; 4]>,
    /// When the target went away: a brief gap (focus moving into a field whose caret is not
    /// measured yet) keeps the trail, so it still travels from where it was.
    hidden_at: Option<Instant>,
    /// The scroll view the cursor was clipped to when its current move began, and the one it
    /// is in now: a move between two (a focus-zone change) crosses the chrome unclipped.
    pub move_clip: Option<[f32; 4]>,
    pub clip: Option<[f32; 4]>,
    epoch: Instant,
    pub visible: bool,
    /// The blink, as the block's live opacity: a half period repaints the block without
    /// re-running the views.
    pub blink: rune::Live<f32>,
    /// Where the views last built the block (its quad's top-left corner), the quad's shape
    /// from there, and how far it has travelled since without changing shape: a live offset,
    /// so a cursor riding a scroll repaints alone (`Cursor::place`).
    pub base: [f32; 2],
    shape: Option<Drawn>,
    pub offset: rune::Live<[f32; 2]>,
    /// The scrolled surface the cursor sits in and its offset at the last tick (`Cursor::carry`).
    riding: Option<(String, f32)>,
    /// The quad's corner when it last moved while off its base: once it rests, the views run
    /// once to put it back on whole points.
    resting: Option<[f32; 2]>,
}

/// What the views drew the block from, besides its place.
#[derive(Clone, Copy, PartialEq)]
struct Drawn {
    corners: [[f32; 2]; 4],
    placed: bool,
    animating: bool,
    clip: Option<[f32; 4]>,
}

impl Default for Cursor {
    fn default() -> Cursor {
        Cursor { caret_line: Default::default(), caret_font: std::cell::Cell::new(15.0), trail: Trail::default(), shown: None, hidden_at: None, move_clip: None, clip: None, epoch: Instant::now(), visible: false, blink: rune::Live::new(1.0),
            base: [0.0; 2], shape: None, offset: rune::Live::new([0.0, 0.0]), riding: None, resting: None }
    }
}

impl Cursor {
    /// Record the clip after a tick: a new move remembers the clip it started from.
    pub fn track_clip(&mut self, clip: Option<[f32; 4]>, moved: bool) {
        if moved { self.move_clip = self.clip; }
        self.clip = clip;
    }

    /// The cursor sits in the scrolled surface `key`, now at `offset`. When that surface has
    /// scrolled since the last tick the cursor rides along: it is part of the text, so it moves
    /// with it at once instead of trailing after it a frame late on its springs.
    pub fn carry(&mut self, key: Option<(&str, f32)>) {
        let rode = match (&self.riding, key) {
            (Some((was, from)), Some((key, to))) if was == key => from - to,
            _ => 0.0,
        };
        self.riding = key.map(|(key, offset)| (key.to_owned(), offset));
        if rode == 0.0 { return; }
        self.trail.shift([0.0, rode]);
        if let Some(shown) = &mut self.shown { shown[1] += rode; }
    }

    /// Settle how this frame draws the block. A quad that only travelled since the views last
    /// built it (riding a scroll) moves on its live offset; a new shape, a cursor appearing
    /// or leaving, a new clip, or the end of a trail re-runs the views. Returns whether they
    /// must re-run.
    pub fn place(&mut self, clip: Option<[f32; 4]>) -> bool {
        let quad = self.trail.quad();
        let origin = quad.iter().fold([f32::MAX; 2], |o, p| [o[0].min(p[0]), o[1].min(p[1])]);
        let drawn = Drawn { corners: quad.map(|p| [p[0] - origin[0], p[1] - origin[1]]), placed: self.placed(), animating: self.trail.animating(), clip };
        let same = self.shape.is_some_and(|was| was.placed == drawn.placed && was.animating == drawn.animating && was.clip == drawn.clip
            && was.corners.iter().zip(&drawn.corners).all(|(a, b)| (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01));
        let travelled = [origin[0] - self.base[0], origin[1] - self.base[1]];
        // At rest again after riding: one view run puts the block back on its own layout.
        let rested = same && travelled != [0.0, 0.0] && self.resting == Some(origin);
        self.resting = (travelled != [0.0, 0.0]).then_some(origin);
        if same && !rested {
            self.offset.set(travelled);
            return false;
        }
        self.shape = Some(drawn);
        self.base = origin;
        self.resting = None;
        self.offset.set([0.0, 0.0]);
        true
    }

    /// Whether the move in flight is crossing from one clip region to another.
    pub fn crossing(&self) -> bool { self.trail.animating() && self.move_clip != self.clip }

    /// Whether there is a cursor to draw (lit or in its blink's dark half).
    pub fn placed(&self) -> bool { self.shown.is_some() && self.hidden_at.is_none() }

    /// The text caret block, when a field is focused.
    pub fn caret_block(&self) -> Option<[f32; 4]> {
        self.caret_line.get().map(|line| block_for_caret(line, self.caret_font.get()))
    }

    /// Trail toward `target` as of `now`. Returns when to run again (`ZERO` while moving), or
    /// `None` when there is nothing to show.
    pub fn tick(&mut self, target: Option<[f32; 4]>, now: Instant) -> Option<Duration> {
        let Some(target) = target else {
            self.hidden_at.get_or_insert(now);
            self.visible = false;
            return None;
        };
        // Gone for longer than a frame or two: the cursor reappears in place, not trailing.
        let fresh = self.shown.is_none() || self.hidden_at.take().is_some_and(|t| now.duration_since(t) > HIDDEN_GRACE);
        if fresh {
            self.trail = Trail::default();
            self.shown = None;
        }
        if self.shown != Some(target) {
            self.trail.target(target);
            if fresh { self.trail.snap(); }
            self.shown = Some(target);
        }
        let moving = self.trail.tick(now);
        self.visible = self.trail.blink_visible(now, self.epoch);
        self.blink.set(if self.visible { 1.0 } else { 0.0 });
        Some(if moving { Duration::ZERO } else { self.trail.next_blink(now, self.epoch) })
    }
}

/// The chrome focus bar on a tab (`buffer_tabs::keyboard_cursor_rect`): 3 pt wide, inset to the
/// tab's left padding, 8 pt shorter than the row. The `+` slot puts it at the slot's left edge.
pub fn tab_bar(rect: [f32; 4], padding: f32, scale: f32, plus: bool) -> [f32; 4] {
    let width = (3.0 * scale).max(2.0);
    let height = (rect[3] - 8.0 * scale).max(8.0).min(rect[3]);
    let x = rect[0] + if plus { 0.0 } else { (padding - width).max(0.0) };
    [x, rect[1] + (rect[3] - height) / 2.0, width, height]
}

/// The original text caret block for a Rune caret line `[x, y, w, h]`: `0.6 em` wide.
pub fn block_for_caret(line: [f32; 4], font_size: f32) -> [f32; 4] {
    // Rune centres its caret line on the insertion boundary; the block starts at the boundary.
    [line[0] + line[2] * 0.5, line[1], (font_size * 0.6).max(2.0), line[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_settles_on_its_target() {
        let mut t = Trail::default();
        t.target([0.0, 0.0, 9.0, 20.0]);
        t.tick(Instant::now());
        assert_eq!(t.quad()[0], [0.0, 0.0], "first sight snaps");
        t.target([90.0, 40.0, 9.0, 20.0]);
        let mut now = Instant::now();
        assert!(t.tick(now) || t.animating() || t.quad()[2] != [99.0, 60.0]);
        for _ in 0..120 { now += Duration::from_millis(8); t.tick(now); }
        assert!(!t.animating());
        assert_eq!(t.quad()[2], [99.0, 60.0]);
    }

    #[test]
    fn typing_hops_use_the_short_trail() {
        let mut t = Trail::default();
        t.target([0.0, 0.0, 9.0, 20.0]);
        t.tick(Instant::now());
        t.target([9.0, 0.0, 9.0, 20.0]);
        t.tick(Instant::now());
        assert!(t.corners.iter().all(|c| c.length <= SHORT_ANIMATION_LENGTH + 1e-6));
    }

    #[test]
    fn a_cursor_in_a_scrolled_file_rides_the_scroll_without_a_trail() {
        let mut cursor = Cursor::default();
        let now = Instant::now();
        cursor.carry(Some(("src/main.rs", 100.0)));
        cursor.tick(Some([200.0, 300.0, 9.0, 23.0]), now);
        let rest = cursor.trail.quad();
        assert!(!cursor.trail.animating());
        // The file scrolls 40 points down: the target moves 40 up, and so does the block, whole.
        cursor.carry(Some(("src/main.rs", 140.0)));
        cursor.tick(Some([200.0, 260.0, 9.0, 23.0]), now);
        assert!(!cursor.trail.animating(), "no trail: it moved with the text");
        for (was, is) in rest.iter().zip(cursor.trail.quad()) {
            assert!((is[0] - was[0]).abs() < 0.01 && (is[1] - (was[1] - 40.0)).abs() < 0.01);
        }
        // Another file, or a cursor that moved in the text, is not a ride.
        cursor.carry(Some(("src/other.rs", 0.0)));
        cursor.tick(Some([200.0, 400.0, 9.0, 23.0]), now);
        assert!(cursor.trail.animating(), "a real move still trails");
    }

    #[test]
    fn a_riding_block_moves_on_its_live_offset_and_comes_home_once_at_rest() {
        let mut cursor = Cursor::default();
        let now = Instant::now();
        cursor.carry(Some(("f", 0.0)));
        cursor.tick(Some([200.0, 300.0, 9.0, 23.0]), now);
        assert!(cursor.place(None), "first sight builds it");
        assert!(!cursor.place(None), "nothing changed: nothing re-runs");
        // Riding a scroll: the same shape somewhere else is a live offset, not a view run.
        cursor.carry(Some(("f", 40.0)));
        cursor.tick(Some([200.0, 260.0, 9.0, 23.0]), now);
        assert!(!cursor.place(None));
        assert_eq!(cursor.offset.get(), [0.0, -40.0]);
        cursor.carry(Some(("f", 70.0)));
        cursor.tick(Some([200.0, 230.0, 9.0, 23.0]), now);
        assert!(!cursor.place(None));
        assert_eq!(cursor.offset.get(), [0.0, -70.0]);
        // Still for a frame: one view run puts the block back on its own layout.
        assert!(cursor.place(None));
        assert_eq!(cursor.offset.get(), [0.0, 0.0]);
        assert_eq!(cursor.base, [200.0, 230.0]);
        assert!(!cursor.place(None));
        // A new clip is the views' business.
        assert!(cursor.place(Some([0.0, 0.0, 10.0, 10.0])));
    }

}

/// Text selection behind the hidden native caret.
pub fn selection(app: &crate::app::App) -> rune::Color {
    super::theme::rgb(app.pal.blue).alpha(0.4)
}
