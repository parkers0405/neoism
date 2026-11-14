//! Timeline growth: rows of a growth stack ease their heights on one spring with speed limits
//! per second, and positions are derived from the presented heights above (so rows never
//! overlap or open gaps). Streaming rows reveal through a feathered bottom edge.
//!
//! Desktop's constants (research/18, `row-growth.ts @ 39eb86d`) are per frame at 60 fps; here
//! every cap is per second and the integrator runs fixed 1/240 s substeps, so 30, 60 and 120 Hz
//! sample the same curve (guarantee 1). Retargets keep height and velocity (guarantee 2).

use rune_motion::Spring;

/// One streaming-edge variant (research/18 §1). Desktop's per-frame values × 60.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamEdge {
    pub name: &'static str,
    /// Largest feather height (points).
    pub mask_max: f32,
    /// Smallest feather height while growing (points).
    pub floor: f32,
    /// How fast the feather shrinks back (points per second).
    pub shrink: f32,
    /// Multiplier on the near-target slowdown cap.
    pub catch_up: f32,
    /// Multiplier on the growth spring's duration.
    pub duration: f32,
    /// Clip the row at its presented height (else content overflows while it grows).
    pub clip: bool,
    /// A streaming row, and its catch-up once the stream ends, draws past its presented height,
    /// unclipped and unmasked, into space the app keeps below it; other rows still clip.
    pub overflow: bool,
    /// Word fade-in duration in seconds (0 = words appear at once).
    pub word_fade: f32,
}

pub mod edge {
    use super::StreamEdge;
    pub const FEATHER: StreamEdge = StreamEdge {
        name: "feather",
        mask_max: 28.0,
        floor: 18.0,
        shrink: 1.8 * 60.0,
        catch_up: 1.0,
        duration: 1.0,
        clip: true,
        overflow: false,
        word_fade: 0.18,
    };
    pub const FAST_FEATHER: StreamEdge = StreamEdge {
        name: "fast-feather",
        mask_max: 12.0,
        floor: 6.0,
        shrink: 4.0 * 60.0,
        catch_up: 2.4,
        duration: 0.55,
        clip: true,
        overflow: false,
        word_fade: 0.18,
    };
    pub const NO_MASK: StreamEdge = StreamEdge {
        name: "no-mask",
        mask_max: 0.0,
        floor: 0.0,
        shrink: 28.0 * 60.0,
        catch_up: 1.0,
        duration: 1.0,
        clip: false,
        overflow: false,
        word_fade: 0.0,
    };
    pub const WORD_FADE: StreamEdge = StreamEdge {
        name: "word-fade",
        mask_max: 0.0,
        floor: 0.0,
        shrink: 28.0 * 60.0,
        catch_up: 1.0,
        duration: 1.0,
        clip: false,
        overflow: false,
        word_fade: 0.36,
    };
    pub const ALL: [StreamEdge; 4] = [FEATHER, FAST_FEATHER, NO_MASK, WORD_FADE];
}

/// `timeline.growth`: 0.32 s, bounce 0 (`row-growth.ts:6-14 @ 39eb86d`).
pub const GROWTH: Spring = Spring::new(0.32, 0.0);

/// The farthest a whole row (`El::arrives_whole`) glides in the spring's own duration. A
/// critically damped spring opens with an acceleration of its distance over its duration squared,
/// so a farther glide lengthens with the square root of its distance and opens no harder than one
/// this long: a pinned viewport following a tall insert doesn't lurch as it starts.
pub const WHOLE_REACH: f32 = 300.0;

/// Speed limits in points per second (Desktop's per-frame caps × 60).
pub mod caps {
    pub const START: f32 = 1.875 * 60.0;
    /// Ramp factor per second. Desktop ramps 2.2× per 60 fps frame; Rune ramps 1.6× so a
    /// growth starting from rest doesn't read as a velocity snap.
    pub const RAMP_PER_S: f32 = 28.2; // ln(1.6) * 60
    pub const MAX: f32 = 11.5 * 60.0;
    pub const MAX_LONG: f32 = 16.0 * 60.0;
    pub const LONG_DISTANCE: f32 = 240.0;
    pub const CARRYING_TAIL: f32 = 7.5 * 60.0;
    pub const NEAR: f32 = 36.0;
    /// Feather growth limit.
    pub const MASK_GROW: f32 = 5.0 * 60.0;
    /// Hardest braking when a cap drops, in points per second squared (Rune's addition: Desktop
    /// switches caps between frames, which motion-lint reads as a velocity snap).
    pub const MAX_DECEL: f32 = 1800.0;
    /// Speed ceiling near the target: APPROACH × points left (per second) plus APPROACH_MIN, so
    /// every row decays into its target (Rune's addition, for the same reason).
    pub const APPROACH: f32 = 14.0;
    pub const APPROACH_MIN: f32 = 3.0;
}

/// A streaming row's climb (`El::streaming`): one critically damped follower whose stiffness
/// tracks how fast its target has been rising, so a slow stream creeps on steadily between
/// words and lines, a fast one keeps up, and the speed never kicks up from rest per arrival.
/// It trails by `2 × rate / ω` (`2 × TRAIL` once the rate sets ω), held back under the feather.
pub mod pace {
    /// Seconds the arrival rate is averaged over.
    pub const WINDOW: f32 = 0.5;
    /// Half the points a steady stream's climb trails its text by.
    pub const TRAIL: f32 = 4.0;
    /// Least stiffness (1/s) while the stream runs: a slow one creeps rather than stopping.
    pub const MIN: f32 = 2.5;
    /// Least stiffness once the stream has ended, reached over [`LAND_RAMP`] seconds, so the
    /// last of it lands in under half a second.
    pub const LAND: f32 = 18.0;
    pub const LAND_RAMP: f32 = 0.15;
    /// Least speed (points per second) once the stream has ended: it stops rather than creeps.
    pub const LAND_MIN: f32 = 8.0;
    /// Most stiffness, for the fastest stream.
    pub const MAX: f32 = 40.0;
    /// Hardest change of speed (points per second squared).
    pub const ACCEL: f32 = 6000.0;
}

pub const SUBSTEP: f32 = 1.0 / 240.0;

/// How fast a fold's feather deepens once its edge starts moving (points per second): 28 pt in
/// under 50 ms, so the edge softens as the spring picks up rather than in its first frame.
const FOLD_FEATHER_GROW: f32 = 600.0;

/// `El::fold_glide` and `El::fold_feather`: a fold's own spring and the depth of its moving
/// edge's feather (the stack's `StreamEdge::mask_max` when `None`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoldGlide {
    pub spring: Spring,
    pub feather: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GrowTrack {
    pub h: f32,
    pub v: f32,
    pub target: f32,
    /// Seconds since this growth began (drives the cap ramp).
    pub age: f32,
    /// Presented feather height (points).
    pub mask: f32,
    pub streaming: bool,
    /// `El::arrives_whole`: no speed limits, only the spring.
    pub whole: bool,
    /// Commit spring duration floor (`Tx::motion`), so a slower transaction (`REFLOW`) slows the row too.
    pub(crate) duration: f32,
    /// Points between the presented height and the target when it was last set (see [`WHOLE_REACH`]).
    pub(crate) span: f32,
    /// `El::fold_glide`: this row's own spring, in place of the stack's.
    pub(crate) glide: Option<Spring>,
    /// `El::fold_feather`: the glide's feather depth, in place of the stack edge's.
    pub(crate) feather: Option<f32>,
    /// A glide's height at which the row's bottom leaves the window: the part of a change
    /// beyond it lands at once, out of sight.
    pub(crate) limit: f32,
    /// A glide opening past `limit`: where its spring stops before the row lands whole.
    pub(crate) stage: Option<f32>,
    /// A glide whose contents draw `a + b × h` points up from their layout (`lift`): riding its
    /// bottom edge while it shuts under a held header (`hide_above`), and easing back as it
    /// turns around.
    pub(crate) slide: Option<(f32, f32)>,
    /// Points of lifted contents put back above the row as its glide landed whole, for its
    /// viewport's offset to take up (`Runtime::restore_above`).
    pub(crate) restore: f32,
    /// The share of each point it shuts by that a header held over it comes down, when the
    /// viewport's end will stop its bottom short (`Runtime::give_way`).
    pub(crate) give: Option<f32>,
    /// Climbing on the stream's [`pace`]: from its first streaming frame until it rests.
    pub(crate) paced: bool,
    /// Points per second the target has been rising at, averaged over [`pace::WINDOW`].
    pub(crate) rate: f32,
    /// Seconds a paced climb has gone on since its stream ended.
    pub(crate) ended: f32,
}

/// A row within this many points of its height, moving slower than `SETTLE_V` points a second,
/// lands there: its last creep moves it under a hundredth of a point a frame, which no pixel shows,
/// and would keep the window drawing frames that change nothing.
const SETTLE: f32 = 1.0 / 16.0;
const SETTLE_V: f32 = 1.5;

impl GrowTrack {
    pub fn settled(h: f32) -> GrowTrack {
        GrowTrack { h, v: 0.0, target: h, age: 0.0, mask: 0.0, streaming: false, whole: false, duration: 0.0, span: 0.0, glide: None, feather: None, limit: f32::INFINITY, stage: None, slide: None, restore: 0.0, give: None, paced: false, rate: 0.0, ended: 0.0 }
    }

    pub fn moving(&self) -> bool {
        (self.h - self.target).abs() > SETTLE || self.v.abs() > SETTLE_V || self.mask > 0.01
    }

    pub fn retarget(&mut self, target: f32) {
        if (target - self.target).abs() > 1e-4 {
            let lift = self.lift();
            if !self.moving() {
                self.age = 0.0;
            }
            if self.streaming {
                self.rate += (target - self.target).max(0.0) / pace::WINDOW;
            }
            self.target = target;
            self.span = (target - self.h).abs();
            self.stage = None;
            self.give = None;
            if self.glide.is_some() {
                // Only what shows of a long fold moves: shutting, the part below the window goes
                // first; opening, it lands once the spring has carried the row to the window's edge.
                if target < self.h && self.h > self.limit {
                    self.h = self.limit.max(target);
                } else if target > self.h && target > self.limit {
                    self.stage = Some(self.limit.max(self.h));
                }
                self.span = (target - self.h).abs();
            }
            // Lifted contents ease back to their layout over what's left of the glide.
            let d = target - self.h;
            self.slide = (lift > 1.0 / 64.0 && d.abs() > 1.0 / 64.0).then(|| (lift * target / d, -lift / d));
        }
    }

    /// How far up from their layout the row's contents draw.
    pub(crate) fn lift(&self) -> f32 {
        self.slide.map_or(0.0, |(a, b)| (a + b * self.h).max(0.0))
    }

    /// A fold shutting under a header held stuck to its viewport's top, with `above` points of
    /// it hidden above that header: those go at once, out of sight, and its contents ride its
    /// bottom edge from here, so what shows slides up under the header on the fold's spring
    /// rather than racing past. Returns the points cut, which the viewport's offset drops by so
    /// nothing on screen moves.
    pub(crate) fn hide_above(&mut self, above: f32) -> f32 {
        let pinned = self.lift() + self.h;
        let cut = above.min(self.h - self.target).max(0.0);
        self.h -= cut;
        self.span = (self.target - self.h).abs();
        self.slide = Some((pinned, -1.0));
        cut
    }

    /// A fold opening again while its contents draw lifted, its viewport pinned to its end so
    /// it grows from its bottom: its spring stops `rise` points on, where its header has risen
    /// back to the top and what its contents hid below has shown, its contents easing up as
    /// much on the way; the rest then lands at once, all of it lifted, which the end takes up.
    pub(crate) fn ride_up(&mut self, rise: f32) {
        let (from, h) = (self.lift(), self.h);
        let stage = (h + rise.max(0.0)).min(self.target);
        let (d, end) = (stage - h, self.target - stage);
        let b = if d > 1.0 / 64.0 { (end - from) / d } else { 0.0 };
        self.slide = Some((if d > 1.0 / 64.0 { from - b * h } else { end }, b));
        self.stage = Some(stage);
        self.span = d;
    }

    /// A fold shutting over what it held, which its clip folds away (`El::fold_glide`).
    pub(crate) fn shutting_fold(&self) -> bool {
        self.glide.is_some() && self.h > self.target + 0.01
    }

    /// The row's `El::streaming`, `El::arrives_whole` and `El::fold_glide` (which streaming
    /// overrides) with the height at which its bottom leaves the window.
    pub(crate) fn set_kind(&mut self, streaming: bool, whole: bool, glide: Option<(FoldGlide, f32)>) {
        self.streaming = streaming;
        self.paced |= streaming;
        self.whole = whole;
        let glide = glide.filter(|_| !streaming);
        if glide.is_none() {
            // A glide's stop at the window's edge goes with it: the row it hands over to carries
            // on from where it is, rather than landing the rest at once.
            self.stage = None;
        }
        self.glide = glide.map(|g| g.0.spring);
        self.feather = glide.and_then(|g| g.0.feather);
        self.limit = glide.map_or(f32::INFINITY, |g| g.1);
    }

    /// Advance one fixed substep.
    pub fn step(&mut self, spring: Spring, edge: &StreamEdge, carrying_tail: bool) {
        let dt = SUBSTEP;
        let spring = spring.with_duration(spring.duration.max(self.duration) * edge.duration.max(0.05));
        let aim = self.stage.unwrap_or(self.target);
        let rem = aim - self.h;
        if self.stage.is_some() && rem.abs() < 1.0 {
            // The glide carried the row to the window's edge: the rest lands at once, out of view,
            // and contents still lifted come back above it.
            self.restore += self.lift();
            self.slide = None;
            self.h = self.target;
            self.v = 0.0;
            self.stage = None;
        } else if rem.abs() <= SETTLE && self.v.abs() < SETTLE_V {
            self.h = self.target;
            self.v = 0.0;
        } else if self.paced && rem > 0.0 {
            self.climb(rem, carrying_tail);
        } else if let Some(own) = self.glide.filter(|_| !self.streaming) {
            // A fold: its own spring for its own time, however far, from the velocity it has.
            let (x, v) = own.sample(self.h, self.v, aim, dt);
            let landed = (rem >= 0.0 && x >= aim) || (rem <= 0.0 && x <= aim);
            self.h = if landed { aim } else { x };
            self.v = if landed { 0.0 } else { v };
            self.age += dt;
        } else if self.whole {
            // Content that arrived complete has nothing to read as it opens: one critically
            // damped glide on the spring, however far, which a pinned viewport follows.
            let spring = spring.with_duration(spring.duration * (self.span / WHOLE_REACH).sqrt().max(1.0));
            let (x, v) = spring.sample(self.h, self.v, self.target, dt);
            let landed = (rem >= 0.0 && x >= self.target) || (rem <= 0.0 && x <= self.target);
            self.h = if landed { self.target } else { x };
            self.v = if landed { 0.0 } else { v };
            self.age += dt;
        } else {
            let (x, _v) = spring.sample(self.h, self.v, self.target, dt);
            let mut cap = if rem.abs() > caps::LONG_DISTANCE { caps::MAX_LONG } else { caps::MAX };
            if carrying_tail {
                cap = cap.min(caps::CARRYING_TAIL);
            }
            cap = cap.min(caps::START * (caps::RAMP_PER_S * self.age).exp());
            if self.streaming && rem > 0.0 && rem < caps::NEAR {
                // Desktop's near-target slowdown, with its 0.65 px floor tapering over the last
                // 4 points so the row settles instead of stopping dead.
                cap = cap.min((0.085 * rem + 0.65 * (rem / 4.0).min(1.0)) * 60.0 * edge.catch_up);
            }
            // A cap that drops (near the target, carrying the tail) brakes at most MAX_DECEL, and
            // never faster than lets the row still stop exactly at its target.
            let braking = (2.0 * caps::MAX_DECEL * rem.abs()).sqrt();
            cap = cap.max((self.v.abs() - caps::MAX_DECEL * dt).min(braking));
            // The last points ease out exponentially, so a lagging row never arrives at speed.
            cap = cap.min(caps::APPROACH * rem.abs() + caps::APPROACH_MIN);
            let mut d = x - self.h;
            let lim = cap * dt;
            d = d.clamp(-lim, lim);
            // A growth bent toward a nearer target lands there instead of overshooting.
            let landed = (rem >= 0.0 && d >= rem) || (rem <= 0.0 && d <= rem);
            if landed {
                d = rem;
            }
            self.h += d;
            self.v = if landed { 0.0 } else { d / dt };
            self.age += dt;
        }
        // Feather: aims for min(max, max(floor, 1.5 × left to grow)); grows ≤ MASK_GROW/s.
        let left = (self.target - self.h).max(0.0);
        let aim = if left > 0.25 {
            edge.mask_max.min(edge.floor.max(1.5 * left))
        } else {
            0.0
        };
        if self.glide.is_some() && !self.streaming {
            // A fold's edge feathers both ways, opening and shutting: it softens as the spring
            // picks up, holds through a reversal, and goes crisp as the glide lands.
            // Contents riding the bottom edge (`hide_above`) aren't cut by it: nothing to soften.
            let left = (self.target - self.h).abs();
            let riding = self.slide.is_some_and(|(_, b)| b == -1.0);
            let aim = if left > 0.25 && !riding { self.feather.unwrap_or(edge.mask_max).min(1.5 * left) } else { 0.0 };
            self.mask = if aim > self.mask { (self.mask + FOLD_FEATHER_GROW * dt).min(aim) } else { aim };
        } else if aim > self.mask {
            self.mask = (self.mask + caps::MASK_GROW * dt).min(aim);
        } else {
            self.mask = (self.mask - edge.shrink * dt).max(aim);
        }
        self.rate *= (-dt / pace::WINDOW).exp();
        if !self.moving() {
            self.slide = None;
            if !self.streaming {
                self.paced = false;
                self.rate = 0.0;
                self.ended = 0.0;
            }
        }
    }

    /// One substep of a streaming row's climb `rem` points short of its target (see [`pace`]).
    fn climb(&mut self, rem: f32, carrying_tail: bool) {
        let dt = SUBSTEP;
        self.ended = if self.streaming { 0.0 } else { self.ended + dt };
        let landing = (self.ended / pace::LAND_RAMP).min(1.0);
        let w = (self.rate / pace::TRAIL).clamp(pace::MIN + (pace::LAND - pace::MIN) * landing, pace::MAX);
        let a = (w * w * rem - 2.0 * w * self.v).clamp(-pace::ACCEL, pace::ACCEL);
        let mut cap = if rem > caps::LONG_DISTANCE { caps::MAX_LONG } else { caps::MAX };
        if carrying_tail {
            cap = cap.min(caps::CARRYING_TAIL);
        }
        // No faster than braking at ACCEL still stops on the target. Once the stream has ended,
        // no slower than LAND_MIN, so the last of it lands rather than crawling; while it runs,
        // no floor, which would outrun a reveal rising a fraction of a point a word.
        let v = (self.v + a * dt).max(pace::LAND_MIN * landing).min(cap.min((2.0 * pace::ACCEL * rem).sqrt()));
        if v * dt >= rem {
            self.h = self.target;
            self.v = 0.0;
        } else {
            self.h += v * dt;
            self.v = v;
        }
        self.age += dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(fps: f32, dist: f32) -> Vec<(f32, f32)> {
        let mut t = GrowTrack::settled(0.0);
        t.retarget(dist);
        let mut acc = 0.0;
        let mut out = Vec::new();
        let dt = 1.0 / fps;
        for k in 1..=(2.0 * fps) as usize {
            acc += dt;
            while acc >= SUBSTEP - 1e-6 {
                t.step(GROWTH, &edge::FEATHER, false);
                acc -= SUBSTEP;
            }
            out.push((k as f32 * dt, t.h));
        }
        out
    }

    #[test]
    fn same_curve_at_30_60_120_fps() {
        let a = run(30.0, 457.0);
        let c = run(120.0, 457.0);
        for (t, h) in a {
            let (_, h2) = c.iter().copied().find(|(t2, _)| (t2 - t).abs() < 1e-3).unwrap();
            assert!((h - h2).abs() < 0.5, "t={t} {h} vs {h2}");
        }
    }

    #[test]
    fn monotone_and_capped() {
        let r = run(120.0, 900.0);
        let mut prev = 0.0;
        for &(_, h) in &r {
            assert!(h >= prev - 1e-4);
            assert!(h - prev <= caps::MAX_LONG / 120.0 + 0.01);
            prev = h;
        }
        assert!((r.last().unwrap().1 - 900.0).abs() < 0.5);
    }

    #[test]
    fn a_whole_row_glides_on_the_spring_alone() {
        let mut t = GrowTrack { whole: true, ..GrowTrack::settled(0.0) };
        t.retarget(900.0);
        let (mut prev, mut speeds) = (0.0, Vec::new());
        for _ in 0..180 {
            for _ in 0..2 {
                t.step(GROWTH, &edge::FEATHER, true);
            }
            assert!(t.h >= prev - 1e-4 && t.h <= 900.0, "monotone, no overshoot: {}", t.h);
            speeds.push(t.h - prev);
            prev = t.h;
        }
        let at = |s: f32| speeds.iter().take((s * 120.0) as usize).sum::<f32>();
        assert!(at(0.6) > 0.98 * 900.0, "past the capped speed: {} pt in 0.6 s", at(0.6));
        assert!(!t.moving(), "settled within 1.5 s");
        // One glide: speed rises, then falls, once.
        let peak = speeds.iter().enumerate().fold((0, 0.0), |m, (i, &v)| if v > m.1 { (i, v) } else { m }).0;
        assert!(speeds[..=peak].windows(2).all(|w| w[1] >= w[0] - 1e-3), "{speeds:?}");
        // (The settle snap closes the last SETTLE pt.)
        assert!(speeds[peak..].windows(2).all(|w| w[1] <= w[0] + SETTLE), "{speeds:?}");
    }

    #[test]
    fn a_far_whole_glide_opens_no_harder_than_its_reach() {
        // The largest change in per-frame step at 120 Hz.
        let jerk = |dist: f32| {
            let mut t = GrowTrack { whole: true, ..GrowTrack::settled(0.0) };
            t.retarget(dist);
            let (mut prev, mut step, mut most) = (0.0, 0.0, 0.0f32);
            for _ in 0..360 {
                t.step(GROWTH, &edge::FEATHER, false);
                t.step(GROWTH, &edge::FEATHER, false);
                most = most.max((t.h - prev - step).abs());
                (step, prev) = (t.h - prev, t.h);
            }
            assert!(!t.moving(), "{dist} pt settles");
            most
        };
        // The spring's opening acceleration over a reach-long glide, as a step change per frame.
        let omega = std::f32::consts::TAU / GROWTH.duration;
        let most = WHOLE_REACH * omega * omega / (120.0 * 120.0);
        assert!(jerk(WHOLE_REACH / 2.0) < 0.6 * most, "a short glide keeps the spring's duration");
        for dist in [WHOLE_REACH, 900.0, 3600.0] {
            assert!(jerk(dist) <= most, "{dist} pt: {} vs {most}", jerk(dist));
        }
    }

    #[test]
    fn eases_into_target() {
        for streaming in [false, true] {
            let mut t = GrowTrack::settled(0.0);
            t.streaming = streaming;
            t.retarget(300.0);
            let mut speeds = Vec::new();
            let mut prev = 0.0;
            for _ in 0..120 {
                for _ in 0..4 {
                    t.step(GROWTH, &edge::FEATHER, false);
                }
                speeds.push(t.h - prev);
                prev = t.h;
            }
            // motion-lint's stop-dead: a frame at ≥ 0.5 px followed by one under 15% of it.
            for w in speeds.windows(2) {
                assert!(!(w[0] >= 0.5 && w[1] <= 0.15 * w[0]), "{streaming}: {speeds:?}");
            }
        }
    }

    const FOLD: Spring = Spring::new(0.18, 0.0);

    /// A `fold_glide` row whose bottom leaves the window at 900 pt, stepped at 120 Hz on a slow stack spring and a
    /// slow commit floor, neither of which may slow it: (seconds, height) per frame.
    fn fold(from: f32, to: f32, secs: f32) -> (GrowTrack, Vec<(f32, f32)>) {
        let mut t = GrowTrack { whole: true, duration: 1.0, ..GrowTrack::settled(from) };
        t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: None }, 900.0)));
        t.retarget(to);
        let mut out = vec![(0.0, t.h)];
        for k in 1..=(secs * 120.0) as usize {
            t.step(GROWTH, &edge::FEATHER, false);
            t.step(GROWTH, &edge::FEATHER, false);
            out.push((k as f32 / 120.0, t.h));
        }
        (t, out)
    }

    /// The height `secs` into a fold.
    fn at(hs: &[(f32, f32)], secs: f32) -> f32 {
        hs[(secs * 120.0).round() as usize].1
    }

    #[test]
    fn a_fold_glides_on_its_own_spring_however_far() {
        for (from, to) in [(40.0, 640.0), (640.0, 40.0), (40.0, 900.0)] {
            let (t, hs) = fold(from, to, 0.5);
            // A critically damped spring's duration is when it looks done: under 2% to go.
            let left = (at(&hs, FOLD.duration) - to).abs();
            assert!(left < 0.02 * (to - from).abs(), "{from} -> {to}: {left} pt left at 0.18 s");
            assert!(!t.moving(), "{from} -> {to} settles within 0.5 s: {hs:?}");
            assert!(hs.windows(2).all(|w| (w[1].1 - w[0].1) * (to - from) >= -1e-3), "monotone: {hs:?}");
            assert!(hs.iter().all(|&(_, h)| h >= from.min(to) - 1e-3 && h <= from.max(to) + 1e-3), "no overshoot");
        }
    }

    #[test]
    fn a_fold_past_the_window_moves_only_what_shows() {
        // Shutting: the part below the window goes first, then what shows glides.
        let (t, hs) = fold(3040.0, 40.0, 0.5);
        assert_eq!(hs[0].1, 900.0);
        assert!(at(&hs, FOLD.duration) < 40.0 + 18.0, "{}", at(&hs, FOLD.duration));
        assert!(!t.moving() && t.h == 40.0);
        // Opening: the spring carries the row to the window's edge, then the rest lands at once.
        let (t, hs) = fold(40.0, 3040.0, 0.5);
        assert!(!t.moving() && t.h == 3040.0);
        let jump = hs.iter().position(|&(_, h)| h > 900.0).unwrap();
        assert!(hs[jump - 1].1 > 898.0 && hs[jump].1 == 3040.0, "{:?}", &hs[jump - 2..=jump]);
        assert!(at(&hs, FOLD.duration) > 900.0 - 18.0, "{}", at(&hs, FOLD.duration));
        assert!(hs[jump].0 < 0.3, "lands in the spring's own time: {}", hs[jump].0);
    }

    #[test]
    fn a_glide_handed_to_a_whole_row_mid_way_carries_on() {
        let mut t = GrowTrack { whole: true, ..GrowTrack::settled(40.0) };
        t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: None }, 200.0)));
        t.retarget(640.0);
        for _ in 0..10 {
            t.step(GROWTH, &edge::FEATHER, false);
        }
        t.set_kind(false, true, None);
        let mut prev = t.h;
        while t.moving() {
            t.step(GROWTH, &edge::FEATHER, false);
            assert!(t.h - prev < 20.0, "a step of {} pt at {}", t.h - prev, t.h);
            prev = t.h;
        }
        assert!((t.h - 640.0).abs() < 0.01, "{}", t.h);
    }

    #[test]
    fn a_fold_feathers_its_moving_edge_both_ways() {
        for (from, to) in [(40.0, 640.0), (640.0, 40.0)] {
            let mut t = GrowTrack { whole: true, ..GrowTrack::settled(from) };
            t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: Some(20.0) }, f32::INFINITY)));
            t.retarget(to);
            let mut masks = Vec::new();
            for _ in 0..120 {
                t.step(GROWTH, &edge::FEATHER, false);
                t.step(GROWTH, &edge::FEATHER, false);
                masks.push(t.mask);
            }
            let peak = masks.iter().copied().fold(0.0, f32::max);
            assert!((peak - 20.0).abs() < 1e-3, "{from} -> {to}: feathers to its own depth: {masks:?}");
            assert!(masks[0] < 6.0, "{from} -> {to}: softens as the spring picks up: {masks:?}");
            assert!(masks.windows(2).all(|w| (w[1] - w[0]).abs() < 6.0), "{from} -> {to}: no jump in depth: {masks:?}");
            assert_eq!(*masks.last().unwrap(), 0.0, "{from} -> {to}: crisp at rest");
        }
    }

    #[test]
    fn a_fold_reversed_mid_glide_keeps_its_velocity() {
        let mut t = GrowTrack { whole: true, ..GrowTrack::settled(640.0) };
        t.set_kind(false, true, Some((FoldGlide { spring: FOLD, feather: None }, 900.0)));
        t.retarget(40.0);
        for _ in 0..12 {
            t.step(GROWTH, &edge::FEATHER, false);
        }
        let (h, v) = (t.h, t.v);
        assert!(v < 0.0 && h < 640.0 && h > 40.0);
        t.retarget(640.0);
        assert_eq!(t.h, h, "no jump");
        t.step(GROWTH, &edge::FEATHER, false);
        assert!(t.h < h, "carries on shutting for a moment: {h} -> {}", t.h);
        let mut masks = vec![t.mask];
        for _ in 0..120 {
            t.step(GROWTH, &edge::FEATHER, false);
            masks.push(t.mask);
        }
        // The feather holds through the turn and thins as the glide lands.
        assert!(masks.windows(2).all(|w| (w[1] - w[0]).abs() < 3.0), "{masks:?}");
        assert_eq!(*masks.last().unwrap(), 0.0);
        assert!(!t.moving() && t.h == 640.0, "settles within 0.5 s: {} at {} pt/s", t.h, t.v);
    }

    #[test]
    fn retarget_keeps_velocity() {
        let mut t = GrowTrack::settled(0.0);
        t.retarget(100.0);
        for _ in 0..24 {
            t.step(GROWTH, &edge::FEATHER, false);
        }
        let (h, v) = (t.h, t.v);
        t.retarget(200.0);
        t.step(GROWTH, &edge::FEATHER, false);
        assert!((t.v - v).abs() < v.abs() * 0.5 + 60.0, "{v} -> {}", t.v);
        assert!(t.h > h);
    }
}
