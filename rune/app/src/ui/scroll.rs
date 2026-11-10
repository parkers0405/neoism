//! One scroll feel for every reading surface: code and Markdown files, the agent transcript,
//! the terminal, and the side panels.
//!
//! - A mouse wheel notch moves three lines on a critically damped glide (0.39 s response, the
//!   terminal's), and consecutive notches extend the same motion.
//! - Trackpad pixels apply directly and inject bounded, continuously decaying momentum,
//!   matching GPUiX's timeline physics. Reading surfaces apply their fixed gain before this.
//!
//! The motion is stepped on the frame clock and placed with `Tx::scroll_to_instant` relative
//! to where the view is each frame, so it animates even while a composer holds focus (Rune
//! lands its own scroll requests at once then) and stays correct when the view shifts under it
//! (older history prepended, rows measured).

use rune::{Tx, WheelEvent};
use std::collections::HashMap;
use std::rc::Rc;

/// A wheel notch, as Rune reports it, in points.
const NOTCH: f32 = 40.0;
/// The notch glide's response (2π/ω).
const NOTCH_RESPONSE: f32 = 0.39;
/// GPUiX's precision-trackpad velocity injection, bound, decay and stop threshold.
const TRACKPAD_INJECTION: f32 = 7.0;
const TRACKPAD_MAX: f32 = 2800.0;
const TRACKPAD_TAU: f32 = 0.28;
const TRACKPAD_STOP: f32 = 50.0;

/// How a surface reads its scroll position: (offset, max).
pub type Source = Rc<dyn Fn() -> (f32, f32)>;

#[derive(Default)]
pub struct Scroller {
    /// Notch glide: distance still to travel and the glide's velocity (pt/s).
    remaining: f32,
    vel: f32,
    /// Trackpad: direct movement not yet applied and continuously decaying velocity.
    pending: f32,
    coast: f32,
    clock: Option<f64>,
    source: Option<Source>,
    /// The offset last placed and what the view reported then: a placement Rune hasn't applied
    /// yet (the view still reads as before) is where the next step starts from.
    placed: Option<f32>,
    seen: Option<f32>,
    /// The offset the last step started from.
    from: f32,
}

impl Scroller {
    pub fn moving(&self) -> bool {
        self.remaining.abs() > 0.25 || self.vel.abs() > 2.0 || self.pending != 0.0 || self.coast.abs() >= TRACKPAD_STOP
    }

    /// Feed a wheel event: `notch_px` is how far one notch travels on this surface.
    pub fn wheel(&mut self, e: &WheelEvent, notch_px: f32, source: Source) {
        // Release events often have no delta. They must not cancel or restart the glide.
        if e.dy == 0.0 { return; }
        self.source = Some(source);
        if e.precise {
            self.remaining = 0.0;
            self.vel = 0.0;
            let (reported, max) = self.source.as_ref().unwrap()();
            let max = max.max(0.0);
            let offset = (self.offset(reported) + self.pending).clamp(0.0, max);
            let d = -e.dy;
            self.pending += (offset + d).clamp(0.0, max) - offset;
            // Include queued movement in the edge check: an event burst at an end must not
            // build hidden velocity or eat the first movement back in the other direction.
            if (d > 0.0 && offset < max - 0.5) || (d < 0.0 && offset > 0.5) {
                self.coast = (self.coast + d * TRACKPAD_INJECTION).clamp(-TRACKPAD_MAX, TRACKPAD_MAX);
            } else {
                self.coast = 0.0;
            }
        } else {
            self.coast = 0.0;
            self.remaining += -e.dy / NOTCH * notch_px;
        }
    }

    fn offset(&self, reported: f32) -> f32 {
        // Unchanged since the last frame while a placement is pending: start from the placement.
        // Moved some other way (history prepended, rows measured): follow the view.
        match (self.placed, self.seen) {
            (Some(placed), Some(seen)) if (reported - seen).abs() < 0.01 => placed,
            _ => reported,
        }
    }

    /// Advance to `clock`; the new offset to place, if it moves.
    pub fn step(&mut self, clock: f64) -> Option<f32> {
        let (reported, max) = self.source.as_ref()?();
        let offset = self.offset(reported);
        let dt = self.clock.map_or(1.0 / 120.0, |last| (clock - last).clamp(0.0, 0.05)) as f32;
        self.clock = Some(clock);
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
        if self.coast.abs() >= TRACKPAD_STOP {
            self.coast *= (-dt / TRACKPAD_TAU).exp();
            by += self.coast * dt;
        } else {
            self.coast = 0.0;
        }
        if by == 0.0 { return None; }
        self.from = offset;
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

/// `NEOISM_RUNE_TRACE_SCROLL=1`: each stepped frame's reported offset, content end and placement.
static TRACE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("NEOISM_RUNE_TRACE_SCROLL").is_some());

/// A step a view's owner places itself (`Scrollers::own`): from this offset to that one.
pub struct Move {
    pub key: String,
    pub from: f32,
    pub to: f32,
}

/// Every scroll view's scroller, by its Rune key.
#[derive(Default)]
pub struct Scrollers {
    views: HashMap<String, Scroller>,
    /// Views whose steps are handed to their owner instead of placed here (`Scrollers::own`).
    owned: std::collections::HashSet<String>,
    moves: Vec<Move>,
    /// Views that scroll sideways (`Scrollers::sideways`): their steps place `x`.
    across: std::collections::HashSet<String>,
}

impl Scrollers {
    pub fn wheel(&mut self, key: &str, e: &WheelEvent, notch_px: f32, source: Source) {
        self.views.entry(key.to_owned()).or_default().wheel(e, notch_px, source);
    }

    /// Hand this view's steps to its owner (`take_moves`) rather than placing them at an
    /// absolute offset. A transcript holds a row in place instead: an offset placed in the
    /// frame its rows are measured (an older page just landed) wins over Rune keeping the
    /// reader's row where it was, and the view jumps by the difference.
    pub fn own(&mut self, key: &str) {
        if !self.owned.contains(key) { self.owned.insert(key.to_owned()); }
    }

    /// A sideways delta for a horizontal view (a wide table), in points along it: the same
    /// glide and trackpad momentum as a vertical one, placed on `x`.
    pub fn sideways(&mut self, key: &str, delta: f32, precise: bool, notch_px: f32, source: Source, tx: &mut Tx) {
        if delta == 0.0 { return; }
        if !self.across.contains(key) { self.across.insert(key.to_owned()); }
        self.wheel(key, &WheelEvent::new(0.0, delta).precise(precise), notch_px, source);
        tx.wake_after(std::time::Duration::ZERO);
    }

    /// This frame's steps of owned views.
    pub fn take_moves(&mut self) -> Vec<Move> { std::mem::take(&mut self.moves) }

    /// Glide a view to `target` (a cursor jump, a goto-line), the far part of a long trip cut
    /// at once so only the last `reach` points animate (Neovide's long-jump rule).
    pub fn glide_to(&mut self, key: &str, offset: f32, target: f32, reach: f32, source: Source, tx: &mut Tx) {
        let far = target - offset;
        let start = if far.abs() > reach {
            let near = target - reach * far.signum();
            tx.scroll_to_instant(key.to_owned(), 0.0, near);
            near
        } else { offset };
        let s = self.views.entry(key.to_owned()).or_default();
        s.source = Some(source);
        s.pending = 0.0;
        s.coast = 0.0;
        s.remaining = target - start;
        s.placed = Some(start);
        s.seen = Some(offset);
        tx.wake_after(std::time::Duration::ZERO);
    }

    /// Stop a view's motion (the app moved it itself: a jump, following new output).
    pub fn stop(&mut self, key: &str) { self.views.remove(key); }

    /// Step every moving view one frame. Returns whether any still moves.
    pub fn step(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let mut any = false;
        let Scrollers { views, owned, moves, across } = self;
        views.retain(|key, scroller| {
            let to = scroller.step(clock);
            if *TRACE { if let Some((offset, max)) = scroller.source.as_ref().map(|s| s()) { eprintln!("[scroll] {key} clock={clock:.4} offset={offset:.2} max={max:.2} to={to:?}"); } }
            match to {
                Some(to) if owned.contains(key) => moves.push(Move { key: key.clone(), from: scroller.from, to }),
                Some(to) if across.contains(key) => tx.scroll_to_x(key.clone(), to),
                Some(to) => tx.scroll_to_instant(key.clone(), 0.0, to),
                None => {}
            }
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
    fn trackpad_direct_distance_does_not_depend_on_event_batching() {
        for distance in [0.125, 17.0, 60.0] {
            for parts in [1, 4] {
                let mut s = Scroller { clock: Some(0.0), ..Scroller::default() };
                for _ in 0..parts {
                    s.wheel(&WheelEvent::new(0.0, -distance / parts as f32).precise(true), 60.0, source(1000.0));
                }
                assert_eq!(s.step(0.0), Some(1000.0 + distance));
            }
        }
    }

    #[test]
    fn trackpad_glides_without_waiting_for_release_and_stops_without_a_tail() {
        let mut s = Scroller { clock: Some(0.0), ..Scroller::default() };
        s.wheel(&WheelEvent::new(0.0, -100.0).precise(true), 60.0, source(1000.0));
        assert_eq!(s.step(0.0), Some(1100.0));
        let dt = 1.0 / 120.0;
        let expected = 1100.0 + 700.0 * (-dt / 0.28_f32).exp() * dt;
        assert!((s.step(dt as f64).unwrap() - expected).abs() < 0.001);
        s.wheel(&WheelEvent::new(0.0, 0.0).precise(true).phase(rune::GesturePhase::Ended), 60.0, source(1000.0));
        assert!(s.step(2.0 * dt as f64).unwrap() > expected);
        for frame in 3..240 { s.step(frame as f64 / 120.0); }
        assert!(!s.moving());
        assert_eq!(s.step(2.0), None);
    }

    #[test]
    fn trackpad_velocity_is_bounded() {
        let mut s = Scroller { clock: Some(0.0), ..Scroller::default() };
        s.wheel(&WheelEvent::new(0.0, -1000.0).precise(true), 60.0, source(1000.0));
        assert_eq!(s.step(0.0), Some(2000.0));
        let dt = 1.0 / 120.0;
        let expected = 2000.0 + 2800.0 * (-dt / 0.28_f32).exp() * dt;
        assert!((s.step(dt as f64).unwrap() - expected).abs() < 0.001);
    }

    #[test]
    fn trackpad_below_stop_threshold_only_moves_directly() {
        let mut s = Scroller { clock: Some(0.0), ..Scroller::default() };
        s.wheel(&WheelEvent::new(0.0, -7.0).precise(true), 60.0, source(1000.0));
        assert_eq!(s.step(0.0), Some(1007.0));
        assert!(!s.moving());
        assert_eq!(s.step(1.0 / 120.0), None);
    }

    #[test]
    fn trackpad_edge_bursts_do_not_delay_reversing() {
        let mut s = Scroller { clock: Some(0.0), ..Scroller::default() };
        for _ in 0..20 {
            s.wheel(&WheelEvent::new(0.0, 100.0).precise(true), 60.0, source(0.0));
        }
        s.wheel(&WheelEvent::new(0.0, -10.0).precise(true), 60.0, source(0.0));
        assert_eq!(s.step(0.0), Some(10.0));
        assert!(s.step(1.0 / 120.0).unwrap() > 10.0);
    }

    #[test]
    fn an_owned_views_steps_are_handed_over_not_placed() {
        let mut scrollers = Scrollers::default();
        let mut tx = Tx::default();
        scrollers.own("transcript");
        scrollers.wheel("transcript", &WheelEvent::new(0.0, 40.0), 60.0, source(1000.0));
        scrollers.wheel("list", &WheelEvent::new(0.0, 40.0), 60.0, source(1000.0));
        assert!(scrollers.step(1.0 / 144.0, &mut tx));
        let moves = scrollers.take_moves();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].key, "transcript");
        assert_eq!(moves[0].from, 1000.0);
        assert!(moves[0].to < 1000.0, "a notch up moves toward the start");
        assert!(scrollers.take_moves().is_empty());
    }

    #[test]
    fn a_sideways_delta_glides_like_a_vertical_one() {
        let mut scrollers = Scrollers::default();
        let mut tx = Tx::default();
        scrollers.sideways("table", -80.0, true, 60.0, source(100.0), &mut tx);
        assert!(scrollers.across.contains("table"));
        // The fingers' distance lands at once and momentum carries on after it.
        assert_eq!(scrollers.views["table"].pending, 80.0);
        assert!(scrollers.step(1.0 / 144.0, &mut tx));
        assert!(scrollers.views["table"].coast > 0.0);
        // No delta, no motion (a gesture's release event).
        scrollers.sideways("other", 0.0, true, 60.0, source(0.0), &mut tx);
        assert!(!scrollers.views.contains_key("other"));
    }
}
