//! One scroll feel for every reading surface: code and Markdown files, the agent transcript,
//! the terminal, and the side panels.
//!
//! - A mouse wheel notch moves three lines on a critically damped glide (0.39 s response, the
//!   terminal's), and consecutive notches extend the same motion.
//! - A trackpad tracks the fingers directly with pointer-style acceleration (slow drags are
//!   precise, fast flicks travel), then coasts when they lift, decaying like a native
//!   momentum scroll; touching again catches it.
//!
//! The motion is stepped on the frame clock and placed with `Tx::scroll_to_instant` relative
//! to where the view is each frame, so it animates even while a composer holds focus (Rune
//! lands its own scroll requests at once then) and stays correct when the view shifts under it
//! (older history prepended, rows measured).

use rune::{GesturePhase, Tx, WheelEvent};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Instant;

/// A wheel notch, as Rune reports it, in points.
const NOTCH: f32 = 40.0;
/// The notch glide's response (2π/ω).
const NOTCH_RESPONSE: f32 = 0.39;
/// Trackpad acceleration: below `SLOW` pt/s the fingers move the content 1:1; by `FAST` it
/// moves `MAX_GAIN` times as far.
const SLOW: f32 = 250.0;
const FAST: f32 = 2600.0;
const MAX_GAIN: f32 = 2.4;
/// Velocity averaged over the last `SAMPLE_WINDOW` seconds of the gesture starts the coast.
const SAMPLE_WINDOW: f32 = 0.075;
/// No trackpad event for this long means the fingers lifted (Wayland may not say so).
const LIFT_GAP: f32 = 0.06;
/// Momentum decays like a native coast: ×0.9975 per millisecond (time constant ≈ 0.4 s).
const COAST_DECAY_PER_MS: f32 = 0.9975;
/// Flicks slower than this don't coast; a coast below `COAST_STOP` ends.
const COAST_MIN: f32 = 120.0;
const COAST_STOP: f32 = 12.0;

/// How a surface reads its scroll position: (offset, max).
pub type Source = Rc<dyn Fn() -> (f32, f32)>;

#[derive(Default)]
pub struct Scroller {
    /// Notch glide: distance still to travel and the glide's velocity (pt/s).
    remaining: f32,
    vel: f32,
    /// Trackpad: finger movement not yet applied, recent (time, distance) samples, and the
    /// momentum velocity after the fingers lift.
    pending: f32,
    samples: VecDeque<(Instant, f32)>,
    touching: Option<Instant>,
    coast: f32,
    clock: Option<f64>,
    source: Option<Source>,
    /// The offset last placed and what the view reported then: a placement Rune hasn't applied
    /// yet (the view still reads as before) is where the next step starts from.
    placed: Option<f32>,
    seen: Option<f32>,
}

impl Scroller {
    pub fn moving(&self) -> bool {
        self.remaining.abs() > 0.25 || self.vel.abs() > 2.0 || self.pending != 0.0 || self.coast != 0.0 || self.touching.is_some()
    }

    /// Feed a wheel event: `notch_px` is how far one notch travels on this surface.
    pub fn wheel(&mut self, e: &WheelEvent, notch_px: f32, source: Source) {
        self.source = Some(source);
        if e.precise {
            let now = Instant::now();
            // Fingers down: whatever was gliding or coasting is caught.
            self.remaining = 0.0;
            self.vel = 0.0;
            self.coast = 0.0;
            let dt = self.samples.back().map_or(1.0 / 120.0, |(t, _)| now.duration_since(*t).as_secs_f32().clamp(1.0 / 500.0, 0.1));
            let speed = e.dy.abs() / dt;
            let t = ((speed - SLOW) / (FAST - SLOW)).clamp(0.0, 1.0);
            let gain = 1.0 + (MAX_GAIN - 1.0) * t * t * (3.0 - 2.0 * t);
            let d = -e.dy * gain;
            self.pending += d;
            self.samples.push_back((now, d));
            while self.samples.front().is_some_and(|(t, _)| now.duration_since(*t).as_secs_f32() > 0.2) { self.samples.pop_front(); }
            self.touching = Some(now);
            if e.phase == GesturePhase::Ended { self.lift(); }
        } else {
            self.coast = 0.0;
            self.touching = None;
            self.remaining += -e.dy / NOTCH * notch_px;
        }
    }

    /// The fingers lifted: carry the gesture's recent speed into a coast.
    fn lift(&mut self) {
        let Some(last) = self.samples.back().map(|(t, _)| *t) else { self.touching = None; return };
        let recent: Vec<(Instant, f32)> = self.samples.iter().copied()
            .filter(|(t, _)| last.duration_since(*t).as_secs_f32() <= SAMPLE_WINDOW).collect();
        let span = recent.first().map_or(0.0, |(t, _)| last.duration_since(*t).as_secs_f32()).max(1.0 / 120.0);
        let v = recent.iter().map(|(_, d)| d).sum::<f32>() / span;
        // A gesture that stopped before lifting doesn't fling.
        let stale = last.elapsed().as_secs_f32() > LIFT_GAP * 2.0;
        self.coast = if v.abs() >= COAST_MIN && !stale { v } else { 0.0 };
        self.samples.clear();
        self.touching = None;
    }

    /// Advance to `clock`; the new offset to place, if it moves.
    pub fn step(&mut self, clock: f64) -> Option<f32> {
        let (reported, max) = self.source.as_ref()?();
        // Unchanged since the last frame while a placement is pending: start from the placement.
        // Moved some other way (history prepended, rows measured): follow the view.
        let offset = match (self.placed, self.seen) {
            (Some(placed), Some(seen)) if (reported - seen).abs() < 0.01 => placed,
            _ => reported,
        };
        let dt = self.clock.map_or(1.0 / 120.0, |last| (clock - last).clamp(0.0, 0.05)) as f32;
        self.clock = Some(clock);
        if self.touching.is_some_and(|t| t.elapsed().as_secs_f32() > LIFT_GAP) { self.lift(); }
        let mut by = std::mem::take(&mut self.pending);
        if self.remaining.abs() > 0.25 || self.vel.abs() > 2.0 {
            let omega = std::f32::consts::TAU / NOTCH_RESPONSE;
            let steps = ((dt / 0.004).ceil() as usize).max(1);
            let h = dt / steps as f32;
            for _ in 0..steps {
                self.vel += (omega * omega * self.remaining - 2.0 * omega * self.vel) * h;
                let d = self.vel * h;
                self.remaining -= d;
                by += d;
            }
            if self.remaining.abs() <= 0.25 && self.vel.abs() <= 2.0 { by += self.remaining; self.remaining = 0.0; self.vel = 0.0; }
        }
        if self.coast != 0.0 {
            by += self.coast * dt;
            self.coast *= COAST_DECAY_PER_MS.powf(dt * 1000.0);
            if self.coast.abs() < COAST_STOP { self.coast = 0.0; }
        }
        if by == 0.0 { return None; }
        let target = (offset + by).clamp(0.0, max.max(0.0));
        self.placed = Some(target);
        self.seen = Some(reported);
        // At an end, whatever pushes past it stops.
        if (target <= 0.0 && by < 0.0) || (target >= max && by > 0.0) {
            self.remaining = 0.0;
            self.vel = 0.0;
            self.coast = 0.0;
        }
        ((target - offset).abs() > 0.01).then_some(target)
    }
}

/// Every scroll view's scroller, by its Rune key.
#[derive(Default)]
pub struct Scrollers(HashMap<String, Scroller>);

impl Scrollers {
    pub fn wheel(&mut self, key: &str, e: &WheelEvent, notch_px: f32, source: Source) {
        self.0.entry(key.to_owned()).or_default().wheel(e, notch_px, source);
    }

    /// Glide a view to `target` (a cursor jump, a goto-line), the far part of a long trip cut
    /// at once so only the last `reach` points animate (Neovide's long-jump rule).
    pub fn glide_to(&mut self, key: &str, offset: f32, target: f32, reach: f32, source: Source, tx: &mut Tx) {
        let far = target - offset;
        let start = if far.abs() > reach {
            let near = target - reach * far.signum();
            tx.scroll_to_instant(key.to_owned(), 0.0, near);
            near
        } else { offset };
        let s = self.0.entry(key.to_owned()).or_default();
        s.source = Some(source);
        s.pending = 0.0;
        s.coast = 0.0;
        s.touching = None;
        s.remaining = target - start;
        s.placed = Some(start);
        s.seen = Some(offset);
        tx.wake_after(std::time::Duration::ZERO);
    }

    /// Stop a view's motion (the app moved it itself: a jump, following new output).
    pub fn stop(&mut self, key: &str) { self.0.remove(key); }

    /// Step every moving view one frame. Returns whether any still moves.
    pub fn step(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let mut any = false;
        self.0.retain(|key, scroller| {
            if let Some(to) = scroller.step(clock) { tx.scroll_to_instant(key.clone(), 0.0, to); }
            let moving = scroller.moving();
            any |= moving;
            moving
        });
        any
    }
}

/// A wheel handler body for a view whose scroll mirror is `source`: horizontal trackpad swipes
/// are left to Rune; everything vertical goes through the shared scroller.
pub fn handle(app_scrollers: &mut Scrollers, key: &str, e: &WheelEvent, notch_px: f32, source: Source, tx: &mut Tx) -> bool {
    if !take(app_scrollers, key, e, notch_px, source, tx) { return false; }
    tx.live_only();
    true
}

/// `handle` for a view whose handler changed state the view reads (it re-runs).
pub fn take(app_scrollers: &mut Scrollers, key: &str, e: &WheelEvent, notch_px: f32, source: Source, tx: &mut Tx) -> bool {
    if e.dy == 0.0 || (e.precise && e.dx.abs() > e.dy.abs()) { return false; }
    app_scrollers.wheel(key, e, notch_px, source);
    tx.wake_after(std::time::Duration::ZERO);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(offset: f32) -> Source { Rc::new(move || (offset, 10_000.0)) }

    #[test]
    fn a_notch_glides_three_lines_and_settles() {
        let mut s = Scroller::default();
        let e = WheelEvent::new(0.0, -40.0);
        s.wheel(&e, 60.0, source(0.0));
        let mut travelled = 0.0;
        for frame in 0..240 {
            if let Some(to) = s.step(frame as f64 / 120.0) { travelled = to; s.source = Some(source(to)); }
        }
        assert!((travelled - 60.0).abs() < 0.5, "travelled {travelled}");
        assert!(!s.moving());
    }

    #[test]
    fn trackpad_accelerates_fast_strokes() {
        let mut slow = Scroller::default();
        slow.wheel(&WheelEvent::new(0.0, -1.0).precise(true), 60.0, source(0.0));
        let mut fast = Scroller::default();
        fast.wheel(&WheelEvent::new(0.0, -60.0).precise(true), 60.0, source(0.0));
        assert!((slow.pending - 1.0).abs() < 0.01);
        assert!(fast.pending > 60.0 * 1.5);
    }
}
