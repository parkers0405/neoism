//! Choreography: stagger, cue-based sequences and phase cycling, run by the render thread.
//!
//! Beats start on *events*, not on summed durations: "when that one arrives", "when it settles",
//! "not before 0.3 s". A spring's physical tail is not its choreographic span (psychopomp
//! `DSL.md`: `settle_in` is ready after 500 ms while its spring is still settling). Cue times
//! that are known in advance (a start, a logical arrival) are scheduled exactly through the
//! store's delays, so a sequence lands on the same frames at any frame rate.

use crate::curve::Anim;
use crate::easing::Easing;
use crate::props::{NodeId, Prop, PropClass, PropId};
use crate::store::{AnimationStore, TrackId};
use crate::value::{Animatable, lanes};

/// Where a stagger ripples from (Motion's `stagger(gap, { from })`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StaggerFrom {
    First,
    Last,
    Center,
    Index(usize),
}

/// Delays for a ripple across `count` items.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stagger {
    pub gap: f32,
    pub from: StaggerFrom,
    pub start: f32,
    /// Optional easing of delays across the ripple (Motion's `ease` option).
    pub ease: Option<Easing>,
}

impl Stagger {
    pub fn new(gap: f32) -> Stagger {
        Stagger { gap, from: StaggerFrom::First, start: 0.0, ease: None }
    }
    pub fn from(mut self, from: StaggerFrom) -> Stagger {
        self.from = from;
        self
    }
    pub fn start(mut self, seconds: f32) -> Stagger {
        self.start = seconds;
        self
    }
    pub fn ease(mut self, easing: Easing) -> Stagger {
        self.ease = Some(easing);
        self
    }

    /// The delay of item `index` out of `count`.
    pub fn delay(&self, index: usize, count: usize) -> f32 {
        let count = count.max(1);
        let origin = match self.from {
            StaggerFrom::First => 0.0,
            StaggerFrom::Last => (count - 1) as f32,
            StaggerFrom::Center => (count - 1) as f32 / 2.0,
            StaggerFrom::Index(i) => i.min(count - 1) as f32,
        };
        let distance = (index as f32 - origin).abs();
        let max = (0..count).map(|i| (i as f32 - origin).abs()).fold(0.0, f32::max);
        let d = match self.ease {
            Some(e) if max > 0.0 => e.eval(distance / max) * max,
            _ => distance,
        };
        self.start + d * self.gap
    }
}

/// Shorthand for [`Stagger::delay`].
pub fn stagger(index: usize, count: usize, gap: f32, from: StaggerFrom) -> f32 {
    Stagger::new(gap).from(from).delay(index, count)
}

/// A handle to a step of a [`Choreography`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StepId(u32);

/// What a cue waits for on another step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// The step started.
    Started,
    /// The step's animations logically completed (springs: their duration; tweens: their end).
    Arrived,
    /// Every track the step animated is physically at rest (or was taken over).
    Settled,
}

/// When a step may start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    after: Option<(StepId, When)>,
    delay: f32,
    not_before: f32,
}

impl Cue {
    /// As soon as the choreography starts.
    pub fn now() -> Cue {
        Cue { after: None, delay: 0.0, not_before: 0.0 }
    }
    /// `t` seconds after the choreography starts.
    pub fn at(t: f32) -> Cue {
        Cue { delay: t, ..Cue::now() }
    }
    pub fn started(step: StepId) -> Cue {
        Cue { after: Some((step, When::Started)), ..Cue::now() }
    }
    pub fn arrived(step: StepId) -> Cue {
        Cue { after: Some((step, When::Arrived)), ..Cue::now() }
    }
    pub fn settled(step: StepId) -> Cue {
        Cue { after: Some((step, When::Settled)), ..Cue::now() }
    }
    /// Wait a further `seconds` after the cue.
    pub fn after(mut self, seconds: f32) -> Cue {
        self.delay += seconds;
        self
    }
    /// Never earlier than `t` seconds after the choreography starts (a causal guard).
    pub fn not_before(mut self, t: f32) -> Cue {
        self.not_before = self.not_before.max(t);
        self
    }
}

#[derive(Clone, Copy, Debug)]
struct Op {
    node: NodeId,
    prop: PropId,
    class: PropClass,
    first: u32,
    lanes: u8,
    anim: Anim,
}

/// A list of property animations, recorded typed and applied untyped.
#[derive(Clone, Debug, Default)]
pub struct Ops {
    ops: Vec<Op>,
    targets: Vec<f32>,
}

impl Ops {
    /// Animate `prop` of `node` to `target`. (If the property has never been set, it is set:
    /// set starting values with `store.set` first.)
    pub fn animate<T: Animatable>(&mut self, node: NodeId, prop: Prop<T>, target: T, anim: impl Into<Anim>) -> &mut Self {
        let l = lanes(&target);
        let first = self.targets.len() as u32;
        self.targets.extend_from_slice(&l[..T::LANES]);
        self.ops.push(Op { node, prop: prop.id, class: prop.class, first, lanes: T::LANES as u8, anim: anim.into() });
        self
    }

    fn apply(&self, store: &mut AnimationStore, delay: f32, set: bool, out: &mut Vec<(TrackId, u32)>) {
        for op in &self.ops {
            let tg = &self.targets[op.first as usize..op.first as usize + op.lanes as usize];
            let t = store.ensure(op.node, op.prop, op.class, tg) as usize;
            if set {
                store.set_raw(t as u32, tg);
            } else {
                // `delay` is store time; the op's node may run on a scaled clock.
                let k = store.node_time_scale(op.node);
                let delay = if k > 0.0 { delay * k } else { delay };
                let anim = Anim { delay: op.anim.delay + delay, ..op.anim };
                store.animate_raw(t, tg, anim, None);
            }
            let id = store.id_of(t);
            out.push((id, store.epoch(id).unwrap_or(0)));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Waiting,
    /// Started (possibly with a scheduled delay) at absolute `start`; its tracks are `tracks`.
    Running { start: f64, first: u32, count: u32 },
}

#[derive(Clone, Debug)]
struct Step {
    cue: Cue,
    ops: Ops,
    state: State,
}

/// A set of steps that start on cues. Call [`tick`](Self::tick) each frame after setting the
/// store's time and before `store.tick()`.
#[derive(Clone, Debug, Default)]
pub struct Choreography {
    steps: Vec<Step>,
    tracks: Vec<(TrackId, u32)>,
    t0: Option<f64>,
}

impl Choreography {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a step: `c.step(Cue::arrived(a).after(0.05), |s| { s.animate(id, prop::OPACITY, 1.0, SETTLE); })`.
    pub fn step(&mut self, cue: Cue, build: impl FnOnce(&mut Ops)) -> StepId {
        let mut ops = Ops::default();
        build(&mut ops);
        self.steps.push(Step { cue, ops, state: State::Waiting });
        StepId((self.steps.len() - 1) as u32)
    }

    /// A ripple: one step per item, each `stagger.delay(i, n)` after `cue`. Returns the steps.
    pub fn stagger<I>(&mut self, cue: Cue, stagger: Stagger, items: I, mut each: impl FnMut(&mut Ops, I::Item)) -> Vec<StepId>
    where
        I: IntoIterator,
        I::IntoIter: ExactSizeIterator,
    {
        let it = items.into_iter();
        let n = it.len();
        it.enumerate()
            .map(|(i, item)| {
                let c = cue.after(stagger.delay(i, n));
                self.step(c, |ops| each(ops, item))
            })
            .collect()
    }

    /// When a step started (or will start, if scheduled with a delay).
    pub fn start_time(&self, step: StepId) -> Option<f64> {
        match self.steps.get(step.0 as usize)?.state {
            State::Running { start, .. } => Some(start),
            State::Waiting => None,
        }
    }

    /// Every step started and every track it animated has settled.
    pub fn is_done(&self, store: &AnimationStore) -> bool {
        self.steps.iter().all(|s| matches!(s.state, State::Running { .. }))
            && all_settled(store, &self.tracks)
    }

    /// Starts every step whose cue is due or now known. Cheap when nothing is waiting.
    pub fn tick(&mut self, store: &mut AnimationStore) {
        let now = store.now();
        let t0 = *self.t0.get_or_insert(now);
        loop {
            let mut progressed = false;
            for k in 0..self.steps.len() {
                if self.steps[k].state != State::Waiting {
                    continue;
                }
                let cue = self.steps[k].cue;
                let base = match cue.after {
                    None => Some(t0),
                    Some((s, when)) => self.cue_time(store, s, when, now),
                };
                let Some(base) = base else { continue };
                let at = (base + cue.delay as f64).max(t0 + cue.not_before as f64);
                let delay = (at - now).max(0.0) as f32;
                let first = self.tracks.len() as u32;
                self.steps[k].ops.apply(store, delay, false, &mut self.tracks);
                let count = self.tracks.len() as u32 - first;
                self.steps[k].state = State::Running { start: now + delay as f64, first, count };
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
    }

    fn cue_time(&self, store: &AnimationStore, s: StepId, when: When, now: f64) -> Option<f64> {
        let State::Running { start, first, count } = self.steps.get(s.0 as usize)?.state else { return None };
        let tracks = &self.tracks[first as usize..(first + count) as usize];
        match when {
            When::Started => Some(start),
            // A frozen step hasn't a known arrival yet: keep waiting.
            When::Arrived => Some(arrival(store, tracks, start)).filter(|t| t.is_finite()),
            When::Settled => all_settled(store, tracks).then_some(now),
        }
    }
}

/// Every track is at rest, or was taken over (its epoch moved on).
fn all_settled(store: &AnimationStore, tracks: &[(TrackId, u32)]) -> bool {
    tracks.iter().all(|&(t, epoch)| store.is_settled(t) || store.epoch(t) != Some(epoch))
}

/// The latest logical end among `tracks`, and never before `from`.
fn arrival(store: &AnimationStore, tracks: &[(TrackId, u32)], from: f64) -> f64 {
    tracks.iter().filter_map(|&(t, _)| store.logical_end(t)).fold(from, f64::max)
}

/// SwiftUI `PhaseAnimator`: cycles through phases, animating each from wherever the last one is,
/// advancing when the current phase arrives (or settles). The first phase is the resting pose and
/// is shown without animation. Under Reduce Motion it holds that pose.
#[derive(Clone, Debug)]
pub struct PhaseAnimator {
    phases: Vec<Ops>,
    current: usize,
    advance: When,
    repeat: bool,
    hold: f32,
    tracks: Vec<(TrackId, u32)>,
    started: bool,
    next_at: Option<f64>,
}

impl Default for PhaseAnimator {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseAnimator {
    pub fn new() -> Self {
        PhaseAnimator {
            phases: Vec::new(),
            current: 0,
            advance: When::Arrived,
            repeat: true,
            hold: 0.0,
            tracks: Vec::new(),
            started: false,
            next_at: None,
        }
    }
    pub fn phase(mut self, build: impl FnOnce(&mut Ops)) -> Self {
        let mut ops = Ops::default();
        build(&mut ops);
        self.phases.push(ops);
        self
    }
    /// Advance on `When::Arrived` (default) or `When::Settled`.
    pub fn advance_on(mut self, when: When) -> Self {
        self.advance = when;
        self
    }
    /// Pause on each phase for `seconds` after it arrives.
    pub fn hold(mut self, seconds: f32) -> Self {
        self.hold = seconds;
        self
    }
    /// Stop after the last phase instead of cycling back to the first.
    pub fn once(mut self) -> Self {
        self.repeat = false;
        self
    }
    pub fn current(&self) -> usize {
        self.current
    }

    pub fn tick(&mut self, store: &mut AnimationStore) {
        if self.phases.is_empty() {
            return;
        }
        let now = store.now();
        if !self.started {
            self.started = true;
            self.tracks.clear();
            self.phases[0].apply(store, 0.0, true, &mut self.tracks);
            self.next_at = Some(now);
        }
        if store.reduced_motion() {
            return;
        }
        let Some(mut at) = self.next_at else {
            // Waiting for a settle.
            if all_settled(store, &self.tracks) {
                self.next_at = Some(now + self.hold as f64);
            }
            return;
        };
        if at == f64::INFINITY {
            // The phase was frozen when it started (`set_node_time_scale(_, 0.0)`): ask again.
            at = arrival(store, &self.tracks, now) + self.hold as f64;
            self.next_at = Some(at);
        }
        if now < at {
            return;
        }
        let next = self.current + 1;
        if next >= self.phases.len() && !self.repeat {
            return;
        }
        self.current = next % self.phases.len();
        self.tracks.clear();
        self.phases[self.current].apply(store, 0.0, false, &mut self.tracks);
        self.next_at = match self.advance {
            When::Arrived | When::Started => Some(arrival(store, &self.tracks, now) + self.hold as f64),
            When::Settled => None,
        };
    }
}
