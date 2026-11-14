//! The animation store: every animated property of every node, in flat arrays the render thread
//! samples once per frame.
//!
//! - **Struct of arrays.** Lane targets, values, velocities and segments live in parallel `Vec`s;
//!   a track (one property of one node) is a contiguous run of lanes.
//! - **Only active tracks cost anything.** `tick` walks a dense list of active track indices.
//!   Settled tracks snap to `(target, 0)`, leave the list and cost nothing until retargeted.
//! - **No allocation per frame.** Pools for additive layers and delayed events use intrusive free
//!   lists, and the event and touched buffers keep their capacity.
//! - **Ids, not references.** `NodeId` + `Prop<T>` or a generational `TrackId`.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::curve::{Anim, Blend, Curve, Decay, Velocity};
use crate::layer::{Eps, Kind, Layer};
use crate::props::{NodeId, Prop, PropClass, PropId};
use crate::spring::Spring;
use crate::value::{Animatable, MAX_LANES, lanes};

mod started;
mod tracks;
mod typed;

const NONE: u32 = u32::MAX;
/// Most additive layers a lane keeps before it collapses them into one merged segment.
const MAX_ADDITIVE: usize = 8;

/// A generational handle to one track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackId {
    index: u32,
    generation: u32,
}

impl TrackId {
    pub(crate) fn index_usize(self) -> usize {
        self.index as usize
    }
}

/// What happened during a tick (or since the last one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionEvent {
    /// The track reached `(target, 0)` and went dormant.
    Settled { track: TrackId, node: NodeId, prop: PropId },
    /// An animation started with `.completion(token)` finished (`finished`), or was replaced by a
    /// newer animation before it did (`!finished`).
    Completed { token: u64, node: NodeId, prop: PropId, finished: bool },
}

/// How a released gesture continues.
#[derive(Clone, Copy, Debug)]
pub enum Release<T> {
    /// Spring to a target, starting from the release velocity.
    Spring { target: T, spring: Spring },
    /// Coast with exponential decay to the projected rest point.
    Decay(Decay),
    /// Coast, but if the projection leaves `[min, max]`, spring onto the bound with the velocity
    /// the value has when it reaches it (scroll views). If it is already out of bounds
    /// (rubber-banded), spring straight back.
    DecayClamped { decay: Decay, min: T, max: T, spring: Spring },
}

#[derive(Clone, Copy)]
struct Extra {
    layer: Layer,
    next: u32,
}

#[derive(Clone, Copy)]
struct Pending {
    at: f64,
    target: f32,
    /// NaN: inherit.
    velocity: f32,
    curve: Curve,
    blend: Blend,
    next: u32,
}

#[derive(Clone, Copy)]
struct Meta {
    node: NodeId,
    prop: PropId,
    class: PropClass,
    generation: u32,
    alive: bool,
    epoch: u32,
    token: u64,
    logical_at: f64,
    node_next: u32,
}

/// A fast, unkeyed hasher for small fixed-size keys (ids, glyph keys) that no input chooses.
#[derive(Default)]
pub struct Fx(u64);
impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(b as u64);
        }
    }
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64)
    }
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64)
    }
    fn write_u64(&mut self, i: u64) {
        self.add(i)
    }
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64)
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
impl Fx {
    #[inline]
    fn add(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}
pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

/// A clock some nodes' tracks run on instead of the store's: it advances by `dt × scale` each
/// time the store's time moves. Nodes at the same scale share one; `nodes == 0` marks it free.
#[derive(Clone, Copy)]
struct SubClock {
    time: f64,
    scale: f32,
    nodes: u32,
}

/// Optional diagnostics for inherited spatial spring retargets (points per second).
#[derive(Clone, Copy, Debug, Default)]
pub struct RetargetAudit {
    pub lanes: u64,
    pub max_velocity_error: f32,
}

/// All animated properties, sampled together at one presentation time.
pub struct AnimationStore {
    now: f64,
    pixel_scale: f32,
    reduced_motion: bool,

    l_target: Vec<f32>,
    l_value: Vec<f32>,
    l_vel: Vec<f32>,
    l_layer: Vec<Layer>,
    l_more: Vec<u32>,
    l_pending: Vec<u32>,
    l_curve: Vec<Curve>,
    lane_free: Vec<Vec<u32>>,

    t_lane0: Vec<u32>,
    t_n: Vec<u8>,
    t_settle_at: Vec<f64>,
    t_next_due: Vec<f64>,
    t_slot: Vec<u32>,
    t_meta: Vec<Meta>,
    /// The track's clock: 0 is the store's own, otherwise an index into `clocks`.
    t_clock: Vec<u32>,
    /// On a stopped clock and already sampled where it holds, so `tick` skips it.
    t_held: Vec<bool>,
    track_free: Vec<u32>,

    /// Index 0 stands for the store's own clock and is never read.
    clocks: Vec<SubClock>,
    clock_free: Vec<u32>,
    node_clock: FxMap<u64, u32>,

    active: Vec<u32>,
    /// With the generation they had, so a track freed (or its slot reused) since drops out
    /// where these are read, and freeing a track never scans them.
    touched: Vec<TrackId>,
    set_touched: Vec<TrackId>,
    /// Tracks started or retargeted since the last tick, and between the two before (`started`).
    starts: Vec<TrackId>,
    ticked_starts: Vec<TrackId>,

    extra: Vec<Extra>,
    extra_free: u32,
    pending: Vec<Pending>,
    pending_free: u32,

    index: FxMap<(u64, u16), u32>,
    node_head: FxMap<u64, u32>,

    events: Vec<MotionEvent>,
    events_seen: usize,
    retarget_audit: Option<RetargetAudit>,
}

impl Default for AnimationStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimationStore {
    pub fn new() -> Self {
        AnimationStore {
            now: 0.0,
            pixel_scale: 2.0,
            reduced_motion: false,
            l_target: Vec::new(),
            l_value: Vec::new(),
            l_vel: Vec::new(),
            l_layer: Vec::new(),
            l_more: Vec::new(),
            l_pending: Vec::new(),
            l_curve: Vec::new(),
            lane_free: (0..=MAX_LANES).map(|_| Vec::new()).collect(),
            t_lane0: Vec::new(),
            t_n: Vec::new(),
            t_settle_at: Vec::new(),
            t_next_due: Vec::new(),
            t_slot: Vec::new(),
            t_meta: Vec::new(),
            t_clock: Vec::new(),
            t_held: Vec::new(),
            track_free: Vec::new(),
            clocks: Vec::new(),
            clock_free: Vec::new(),
            node_clock: FxMap::default(),
            active: Vec::new(),
            touched: Vec::new(),
            set_touched: Vec::new(),
            starts: Vec::new(),
            ticked_starts: Vec::new(),
            extra: Vec::new(),
            extra_free: NONE,
            pending: Vec::new(),
            pending_free: NONE,
            index: FxMap::default(),
            node_head: FxMap::default(),
            events: Vec::new(),
            events_seen: 0,
            retarget_audit: None,
        }
    }

    /// Pre-sizes for `tracks` tracks of about `lanes_per_track` lanes, so even the first frames
    /// don't allocate.
    pub fn with_capacity(tracks: usize, lanes_per_track: usize) -> Self {
        let mut s = Self::new();
        let lanes = tracks * lanes_per_track;
        s.l_target.reserve(lanes);
        s.l_value.reserve(lanes);
        s.l_vel.reserve(lanes);
        s.l_layer.reserve(lanes);
        s.l_more.reserve(lanes);
        s.l_pending.reserve(lanes);
        s.l_curve.reserve(lanes);
        s.t_lane0.reserve(tracks);
        s.t_n.reserve(tracks);
        s.t_settle_at.reserve(tracks);
        s.t_next_due.reserve(tracks);
        s.t_slot.reserve(tracks);
        s.t_meta.reserve(tracks);
        s.t_clock.reserve(tracks);
        s.t_held.reserve(tracks);
        s.active.reserve(tracks);
        s.touched.reserve(tracks);
        s.events.reserve(tracks * 2);
        s.index.reserve(tracks);
        s.node_head.reserve(tracks);
        s
    }

    // ------------------------------------------------------------------------------------------
    // Time and policy

    /// The presentation time animations are applied at and sampled for (seconds, from a [`crate::Clock`]).
    pub fn now(&self) -> f64 {
        self.now
    }

    /// Opt-in trace diagnostics; disabled in normal rendering. Measures inherited spatial
    /// spring velocity at actual retarget boundaries, excluding deliberate instant cuts.
    pub fn audit_retargets(&mut self, enabled: bool) {
        self.retarget_audit = enabled.then(RetargetAudit::default);
    }

    pub fn take_retarget_audit(&mut self) -> RetargetAudit {
        self.retarget_audit.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Sets the frame's target presentation time. Apply this frame's commits after this, then `tick`.
    pub fn set_time(&mut self, t: f64) {
        let dt = t - self.now;
        self.now = t;
        for c in self.clocks.iter_mut().skip(1).filter(|c| c.nodes > 0 && c.scale != 0.0) {
            c.time += dt * c.scale as f64;
        }
    }

    /// `set_time(t)` then `tick()`.
    pub fn tick_at(&mut self, t: f64) {
        self.set_time(t);
        self.tick();
    }

    // ------------------------------------------------------------------------------------------
    // Per-node time

    /// Runs every track of `node`, and any it creates later, at `scale` times the store's speed:
    /// each `set_time` advances them by `dt × scale`. At 0 they hold exactly where they are,
    /// pending delays included, and [`running`](Self::running) ignores them. Changing the scale
    /// keeps every running animation at its exact place. 1 is the store's own clock.
    pub fn set_node_time_scale(&mut self, node: NodeId, scale: f32) {
        let scale = if scale.is_finite() { scale.max(0.0) } else { 1.0 };
        let old = self.node_clock.get(&node.0).copied().unwrap_or(0);
        if self.clock_scale(old) == scale {
            return;
        }
        let new = if scale == 1.0 { 0 } else { self.clock_at(scale) };
        let delta = self.clock_time(new) - self.clock_time(old);
        if new == 0 {
            self.node_clock.remove(&node.0);
        } else {
            self.clocks[new as usize].nodes += 1;
            self.node_clock.insert(node.0, new);
        }
        let mut t = self.node_head.get(&node.0).copied().unwrap_or(NONE);
        while t != NONE {
            self.rebase(t as usize, delta);
            self.t_clock[t as usize] = new;
            self.t_held[t as usize] = false;
            t = self.t_meta[t as usize].node_next;
        }
        self.release_clock(old);
    }

    /// The scale `node`'s tracks run at relative to the store's (1 unless set).
    pub fn node_time_scale(&self, node: NodeId) -> f32 {
        if self.node_clock.is_empty() {
            return 1.0;
        }
        self.clock_scale(self.node_clock.get(&node.0).copied().unwrap_or(0))
    }

    /// The time `node`'s tracks are sampled at: `now()` unless its time is scaled.
    pub fn node_time(&self, node: NodeId) -> f64 {
        if self.node_clock.is_empty() {
            return self.now;
        }
        self.clock_time(self.node_clock.get(&node.0).copied().unwrap_or(0))
    }

    /// The live clock running at `scale`, or a new one reading `now`.
    fn clock_at(&mut self, scale: f32) -> u32 {
        if let Some(c) = self.clocks.iter().skip(1).position(|c| c.nodes > 0 && c.scale == scale) {
            return c as u32 + 1;
        }
        if self.clocks.is_empty() {
            self.clocks.push(SubClock { time: 0.0, scale: 1.0, nodes: 0 });
        }
        let clock = SubClock { time: self.now, scale, nodes: 0 };
        match self.clock_free.pop() {
            Some(c) => {
                self.clocks[c as usize] = clock;
                c
            }
            None => {
                self.clocks.push(clock);
                self.clocks.len() as u32 - 1
            }
        }
    }

    fn release_clock(&mut self, c: u32) {
        if c == 0 {
            return;
        }
        let clock = &mut self.clocks[c as usize];
        clock.nodes -= 1;
        if clock.nodes == 0 {
            self.clock_free.push(c);
        }
    }

    fn clock_scale(&self, c: u32) -> f32 {
        if c == 0 { 1.0 } else { self.clocks[c as usize].scale }
    }

    fn clock_time(&self, c: u32) -> f64 {
        if c == 0 { self.now } else { self.clocks[c as usize].time }
    }

    #[inline]
    pub(crate) fn time_of(&self, t: usize) -> f64 {
        self.clock_time(self.t_clock[t])
    }

    #[inline]
    fn frozen(&self, t: usize) -> bool {
        let c = self.t_clock[t];
        c != 0 && self.clocks[c as usize].scale == 0.0
    }

    /// Shifts every absolute time of track `t` by `delta`.
    fn rebase(&mut self, t: usize, delta: f64) {
        if delta == 0.0 {
            return;
        }
        let l0 = self.t_lane0[t] as usize;
        for l in l0..l0 + self.t_n[t] as usize {
            self.l_layer[l].t0 += delta;
            self.l_layer[l].end += delta;
            let mut m = self.l_more[l];
            while m != NONE {
                let e = &mut self.extra[m as usize];
                e.layer.t0 += delta;
                e.layer.end += delta;
                m = e.next;
            }
            let mut p = self.l_pending[l];
            while p != NONE {
                self.pending[p as usize].at += delta;
                p = self.pending[p as usize].next;
            }
        }
        self.t_settle_at[t] += delta;
        self.t_next_due[t] += delta;
        self.t_meta[t].logical_at += delta;
    }

    /// Device pixels per point; sets settle thresholds below the 1/64 px snap quantum.
    pub fn set_pixel_scale(&mut self, scale: f32) {
        self.pixel_scale = scale.max(1.0);
    }

    pub fn pixel_scale(&self) -> f32 {
        self.pixel_scale
    }

    /// The OS Reduce Motion setting. Applied per property class to every new animation.
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.reduced_motion = on;
    }

    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    // ------------------------------------------------------------------------------------------
    // The frame

    /// Samples every active track at `now()`, applies due delayed animations, settles finished
    /// tracks. Allocation-free once buffers have grown to the working-set size.
    pub fn tick(&mut self) {
        self.events.drain(..self.events_seen);
        self.touched.clear();
        let meta = &self.t_meta;
        let live = |id: &&TrackId| meta[id.index as usize].alive && meta[id.index as usize].generation == id.generation;
        self.touched.extend(self.set_touched.iter().filter(live));
        self.set_touched.clear();
        std::mem::swap(&mut self.starts, &mut self.ticked_starts);
        self.starts.clear();
        let clocked = !self.node_clock.is_empty();
        let mut i = 0;
        while i < self.active.len() {
            let t = self.active[i] as usize;
            let now = if clocked { self.time_of(t) } else { self.now };
            // Nothing moves on a stopped clock, or on a track that only waits for a delayed
            // start: once sampled where it holds, it isn't touched again until it moves.
            let held = (clocked && self.frozen(t)) || (now >= self.t_settle_at[t] && self.t_next_due[t] > now);
            if held && self.t_held[t] {
                i += 1;
                continue;
            }
            self.t_held[t] = held;
            self.touched.push(TrackId { index: t as u32, generation: self.t_meta[t].generation });
            if self.t_next_due[t] <= now {
                self.apply_due(t, now);
            }
            let l0 = self.t_lane0[t] as usize;
            let n = self.t_n[t] as usize;
            let range = l0..l0 + n;
            let mut stacked = false;
            {
                // One bounds check per track, then straight-line lane sampling.
                let layers = &self.l_layer[range.clone()];
                let more = &self.l_more[range.clone()];
                let targets = &self.l_target[range.clone()];
                let values = &mut self.l_value[range.clone()];
                let vels = &mut self.l_vel[range.clone()];
                for k in 0..n {
                    let (o, v) = layers[k].sample(now);
                    values[k] = targets[k] + o;
                    vels[k] = v;
                    stacked |= more[k] != NONE;
                }
            }
            if stacked {
                for l in range {
                    if self.l_more[l] != NONE {
                        let (o2, v2) = self.sample_extra(l, now);
                        self.l_value[l] += o2;
                        self.l_vel[l] += v2;
                    }
                }
            }
            if now >= self.t_settle_at[t] && self.t_next_due[t] == f64::INFINITY {
                self.settle(t);
                self.deactivate(t, i);
                continue;
            }
            i += 1;
        }
        self.events_seen = self.events.len();
    }

    /// Events since the previous tick (settles, completions).
    pub fn events(&self) -> &[MotionEvent] {
        &self.events
    }

    /// The values presented at the last tick for a track, as raw lanes.
    pub fn values_of(&self, id: TrackId) -> Option<&[f32]> {
        self.contains(id).then(|| self.lane_values(id.index as usize))
    }

    /// Where each lane of a track comes to rest (its latest target, delayed ones included),
    /// into `out`. False when the track is gone.
    pub(crate) fn targets_of(&self, id: TrackId, out: &mut [f32]) -> bool {
        if !self.contains(id) {
            return false;
        }
        let t = id.index as usize;
        let l0 = self.t_lane0[t] as usize;
        for (k, slot) in out.iter_mut().enumerate().take(self.t_n[t] as usize) {
            *slot = self.final_target(l0 + k);
        }
        true
    }

    /// Is the track currently animating?
    pub fn is_active(&self, id: TrackId) -> bool {
        self.contains(id) && self.t_slot[id.index as usize] != NONE
    }

    /// Exact `(value, velocity)` lanes of a track at `now()`, into `x` and `v`.
    pub(crate) fn sample_exact(&self, id: TrackId, x: &mut [f32], v: &mut [f32]) -> bool {
        if !self.contains(id) {
            return false;
        }
        let t = id.index as usize;
        let l0 = self.t_lane0[t] as usize;
        let now = self.time_of(t);
        for i in 0..self.t_n[t] as usize {
            let (a, b) = self.sample_lane(l0 + i, now);
            x[i] = a;
            v[i] = b;
        }
        true
    }

    pub fn active_len(&self) -> usize {
        self.active.len()
    }

    /// Some track is animating on a clock that moves. Tracks held by a stopped clock don't count.
    pub fn running(&self) -> bool {
        if self.node_clock.is_empty() {
            return !self.active.is_empty();
        }
        self.active.iter().any(|&t| !self.frozen(t as usize))
    }

    pub fn track_len(&self) -> usize {
        self.t_meta.iter().filter(|m| m.alive).count()
    }

    // ------------------------------------------------------------------------------------------
    // Untyped core (used by the scene, keyframes and choreography)

    pub(crate) fn ensure(&mut self, node: NodeId, prop: PropId, class: PropClass, init: &[f32]) -> u32 {
        if let Some(&t) = self.index.get(&(node.0, prop.0)) {
            self.check_lanes(t as usize, init.len());
            return t;
        }
        let n = init.len();
        assert!(n > 0 && n <= MAX_LANES);
        let l0 = self.lane_free[n].pop().unwrap_or_else(|| {
            let l0 = self.l_target.len();
            let end = l0 + n;
            self.l_target.resize(end, 0.0);
            self.l_value.resize(end, 0.0);
            self.l_vel.resize(end, 0.0);
            self.l_layer.resize(end, Layer::REST);
            self.l_more.resize(end, NONE);
            self.l_pending.resize(end, NONE);
            self.l_curve.resize(end, Curve::Instant);
            l0 as u32
        });
        for (i, &x) in init.iter().enumerate() {
            let l = l0 as usize + i;
            self.l_target[l] = x;
            self.l_value[l] = x;
            self.l_vel[l] = 0.0;
            self.l_layer[l] = Layer::REST;
            self.l_more[l] = NONE;
            self.l_pending[l] = NONE;
            self.l_curve[l] = Curve::Instant;
        }
        let head = self.node_head.get(&node.0).copied().unwrap_or(NONE);
        let clock = if self.node_clock.is_empty() { 0 } else { self.node_clock.get(&node.0).copied().unwrap_or(0) };
        let t = match self.track_free.pop() {
            Some(t) => {
                let t = t as usize;
                let generation = self.t_meta[t].generation.wrapping_add(1);
                self.t_meta[t] = Meta::new(node, prop, class, generation, head);
                self.t_lane0[t] = l0;
                self.t_n[t] = n as u8;
                self.t_settle_at[t] = f64::NEG_INFINITY;
                self.t_next_due[t] = f64::INFINITY;
                self.t_slot[t] = NONE;
                self.t_clock[t] = clock;
                self.t_held[t] = false;
                t as u32
            }
            None => {
                self.t_meta.push(Meta::new(node, prop, class, 0, head));
                self.t_lane0.push(l0);
                self.t_n.push(n as u8);
                self.t_settle_at.push(f64::NEG_INFINITY);
                self.t_next_due.push(f64::INFINITY);
                self.t_slot.push(NONE);
                self.t_clock.push(clock);
                self.t_held.push(false);
                (self.t_meta.len() - 1) as u32
            }
        };
        self.index.insert((node.0, prop.0), t);
        self.node_head.insert(node.0, t);
        t
    }

    pub(crate) fn id_of(&self, t: usize) -> TrackId {
        TrackId { index: t as u32, generation: self.t_meta[t].generation }
    }

    fn check_lanes(&self, t: usize, n: usize) {
        assert_eq!(
            self.t_n[t] as usize, n,
            "property {:?} of {:?} was created with {} lanes, used with {}",
            self.t_meta[t].prop, self.t_meta[t].node, self.t_n[t], n
        );
    }

    pub(crate) fn lane_targets_4(&self, t: usize) -> [f32; 4] {
        let l0 = self.t_lane0[t] as usize;
        core::array::from_fn(|i| self.final_target(l0 + i))
    }

    fn lane_values(&self, t: usize) -> &[f32] {
        let l0 = self.t_lane0[t] as usize;
        &self.l_value[l0..l0 + self.t_n[t] as usize]
    }

    pub(crate) fn set_raw(&mut self, t: u32, values: &[f32]) {
        let t = t as usize;
        let l0 = self.t_lane0[t] as usize;
        for (i, &x) in values.iter().enumerate() {
            let l = l0 + i;
            self.cancel_pending(l);
            self.free_extra(l);
            self.l_layer[l] = Layer::REST;
            self.l_target[l] = x;
            self.l_value[l] = x;
            self.l_vel[l] = 0.0;
            self.l_curve[l] = Curve::Instant;
        }
        self.t_next_due[t] = f64::INFINITY;
        self.t_settle_at[t] = f64::NEG_INFINITY;
        self.take_token(t, false);
        self.t_meta[t].epoch = self.t_meta[t].epoch.wrapping_add(1);
        if self.t_slot[t] != NONE {
            let slot = self.t_slot[t] as usize;
            self.deactivate(t, slot);
        }
        self.set_touched.push(TrackId { index: t as u32, generation: self.t_meta[t].generation });
    }

    /// Animates lanes of track `t` toward `targets`. `lane_vel` overrides per-lane start velocity.
    /// False when no lane's target changed, so nothing moves.
    pub(crate) fn animate_raw(&mut self, t: usize, targets: &[f32], mut anim: Anim, lane_vel: Option<&[f32]>) -> bool {
        let now = self.time_of(t);
        if self.t_next_due[t] <= now {
            self.apply_due(t, now);
        }
        let class = self.t_meta[t].class;
        if self.reduced_motion {
            anim.curve = class.reduce(anim.curve);
        }
        let eps = class.eps(self.pixel_scale);
        let blend = anim.resolved_blend();
        let l0 = self.t_lane0[t] as usize;
        let mut changed = false;
        for (i, &target) in targets.iter().enumerate() {
            let l = l0 + i;
            let vel = match (lane_vel, anim.velocity) {
                (Some(v), _) => Some(v[i]),
                (None, Velocity::Inherit) => None,
                (None, Velocity::Zero) => Some(0.0),
                (None, Velocity::Uniform(u)) => Some(u),
            };
            let decay = matches!(anim.curve, Curve::Decay(_));
            if vel.is_none() && !decay && self.final_target(l) == target {
                continue; // unchanged target: keep the running trajectory and any pending due time
            }
            changed = true;
            self.cancel_pending(l);
            if anim.delay > 0.0 {
                self.push_pending(l, Pending {
                    at: now + anim.delay as f64,
                    target,
                    velocity: vel.unwrap_or(f32::NAN),
                    curve: anim.curve,
                    blend,
                    next: NONE,
                });
            } else {
                let before = (self.retarget_audit.is_some() && class == PropClass::Spatial
                    && matches!(anim.curve, Curve::Spring(_)) && vel.is_none()
                    && self.l_layer[l].end > now).then(|| self.sample_lane(l, now).1);
                self.start_lane(l, now, target, anim.curve, blend, vel, eps);
                if let Some(before) = before {
                    let error = (self.sample_lane(l, now).1 - before).abs();
                    let audit = self.retarget_audit.as_mut().unwrap();
                    audit.lanes += 1;
                    audit.max_velocity_error = audit.max_velocity_error.max(error);
                }
            }
        }
        if changed {
            let end = now + anim.delay as f64 + anim.curve.logical_duration() as f64;
            self.commit_scheduled(t, end, anim.token);
        } else if anim.token != 0 {
            if self.t_slot[t] == NONE {
                let m = self.t_meta[t];
                self.events.push(MotionEvent::Completed { token: anim.token, node: m.node, prop: m.prop, finished: true });
            } else {
                self.take_token(t, false);
                self.t_meta[t].token = anim.token;
            }
        }
        changed
    }

    /// Starts a new segment on lane `l` at time `at` from its exact state there.
    #[allow(clippy::too_many_arguments)]
    fn start_lane(&mut self, l: usize, at: f64, target: f32, curve: Curve, blend: Blend, vel: Option<f32>, eps: Eps) {
        let (x, v) = self.sample_lane(l, at);
        self.l_curve[l] = curve;
        match curve {
            Curve::Instant => {
                self.free_extra(l);
                self.l_layer[l] = Layer::REST;
                self.l_target[l] = target;
            }
            Curve::Decay(d) => {
                let v = vel.unwrap_or(v);
                self.free_extra(l);
                self.l_target[l] = x + v / d.k();
                self.l_layer[l] = Layer::decay(d.k(), at, v, eps);
            }
            _ if blend == Blend::Additive => {
                let old = self.l_target[l];
                self.l_target[l] = target;
                let layer = Layer::start(&curve, at, old - target, vel.map_or(0.0, |u| u - v), eps);
                self.push_layer(l, layer, at);
            }
            _ => {
                self.free_extra(l);
                self.l_target[l] = target;
                self.l_layer[l] = Layer::start(&curve, at, x - target, vel.unwrap_or(v), eps);
            }
        }
    }

    fn push_layer(&mut self, l: usize, layer: Layer, at: f64) {
        if layer.kind == Kind::Rest {
            return;
        }
        if self.l_layer[l].kind == Kind::Rest || self.l_layer[l].end <= at {
            self.l_layer[l] = layer;
            return;
        }
        let mut count = 1;
        let mut m = self.l_more[l];
        while m != NONE {
            count += 1;
            m = self.extra[m as usize].next;
        }
        if count >= MAX_ADDITIVE {
            // Too many stacked layers: collapse into one merged segment on the new curve. Exact in
            // position and velocity at `at`.
            // The lane's target already moved; the new layer carries the difference.
            let (x, v) = self.sample_lane(l, at);
            let (o, ov) = layer.sample(at);
            let (x, v) = (x + o, v + ov);
            self.free_extra(l);
            let target = self.l_target[l];
            let curve = self.l_curve[l];
            self.l_layer[l] = Layer::start(&curve, at, x - target, v, Eps { pos: 1e-4, vel: 1e-3 });
            return;
        }
        let idx = self.alloc_extra(Extra { layer, next: self.l_more[l] });
        self.l_more[l] = idx;
    }

    #[inline]
    fn sample_lane(&self, l: usize, at: f64) -> (f32, f32) {
        let (mut o, mut v) = self.l_layer[l].sample(at);
        let mut m = self.l_more[l];
        while m != NONE {
            let e = &self.extra[m as usize];
            let (a, b) = e.layer.sample(at);
            o += a;
            v += b;
            m = e.next;
        }
        (self.l_target[l] + o, v)
    }

    /// Sums extra layers and frees the ones that have ended.
    fn sample_extra(&mut self, l: usize, now: f64) -> (f32, f32) {
        let (mut o, mut v) = (0.0, 0.0);
        let mut prev = NONE;
        let mut m = self.l_more[l];
        while m != NONE {
            let e = self.extra[m as usize];
            if now >= e.layer.end {
                if prev == NONE {
                    self.l_more[l] = e.next;
                } else {
                    self.extra[prev as usize].next = e.next;
                }
                self.extra[m as usize].next = self.extra_free;
                self.extra_free = m;
            } else {
                let (a, b) = e.layer.sample(now);
                o += a;
                v += b;
                prev = m;
            }
            m = e.next;
        }
        (o, v)
    }

    fn final_target(&self, l: usize) -> f32 {
        let mut p = self.l_pending[l];
        let mut target = self.l_target[l];
        while p != NONE {
            target = self.pending[p as usize].target;
            p = self.pending[p as usize].next;
        }
        target
    }

    fn apply_due(&mut self, t: usize, now: f64) {
        let l0 = self.t_lane0[t] as usize;
        let n = self.t_n[t] as usize;
        let eps = self.t_meta[t].class.eps(self.pixel_scale);
        for l in l0..l0 + n {
            loop {
                let p = self.l_pending[l];
                if p == NONE || self.pending[p as usize].at > now {
                    break;
                }
                let ev = self.pending[p as usize];
                self.l_pending[l] = ev.next;
                self.pending[p as usize].next = self.pending_free;
                self.pending_free = p;
                let vel = if ev.velocity.is_nan() { None } else { Some(ev.velocity) };
                self.start_lane(l, ev.at, ev.target, ev.curve, ev.blend, vel, eps);
            }
        }
        self.recompute(t);
    }

    /// Recomputes the track's next due time and settle time from its lanes.
    fn recompute(&mut self, t: usize) {
        let l0 = self.t_lane0[t] as usize;
        let n = self.t_n[t] as usize;
        let mut due = f64::INFINITY;
        let mut settle = f64::NEG_INFINITY;
        for l in l0..l0 + n {
            let p = self.l_pending[l];
            if p != NONE {
                due = due.min(self.pending[p as usize].at);
            }
            if self.l_layer[l].kind != Kind::Rest {
                settle = settle.max(self.l_layer[l].end);
            }
            let mut m = self.l_more[l];
            while m != NONE {
                settle = settle.max(self.extra[m as usize].layer.end);
                m = self.extra[m as usize].next;
            }
        }
        self.t_next_due[t] = due;
        self.t_settle_at[t] = settle;
    }

    fn settle(&mut self, t: usize) {
        let l0 = self.t_lane0[t] as usize;
        let n = self.t_n[t] as usize;
        for l in l0..l0 + n {
            self.free_extra(l);
            self.l_layer[l] = Layer::REST;
            self.l_value[l] = self.l_target[l];
            self.l_vel[l] = 0.0;
        }
        let m = self.t_meta[t];
        self.events.push(MotionEvent::Settled { track: self.id_of(t), node: m.node, prop: m.prop });
        self.take_token(t, true);
    }

    fn take_token(&mut self, t: usize, finished: bool) {
        let m = &mut self.t_meta[t];
        if m.token != 0 {
            let token = core::mem::take(&mut m.token);
            let (node, prop) = (m.node, m.prop);
            self.events.push(MotionEvent::Completed { token, node, prop, finished });
        }
    }

    fn activate(&mut self, t: usize) {
        if self.t_slot[t] == NONE {
            self.t_slot[t] = self.active.len() as u32;
            self.active.push(t as u32);
        }
    }

    fn deactivate(&mut self, t: usize, slot: usize) {
        self.active.swap_remove(slot);
        if slot < self.active.len() {
            let moved = self.active[slot] as usize;
            self.t_slot[moved] = slot as u32;
        }
        self.t_slot[t] = NONE;
    }

    fn push_pending(&mut self, l: usize, mut ev: Pending) {
        // Keep the lane's list sorted by time.
        let mut prev = NONE;
        let mut cur = self.l_pending[l];
        while cur != NONE && self.pending[cur as usize].at <= ev.at {
            prev = cur;
            cur = self.pending[cur as usize].next;
        }
        ev.next = cur;
        let idx = if self.pending_free != NONE {
            let i = self.pending_free;
            self.pending_free = self.pending[i as usize].next;
            self.pending[i as usize] = ev;
            i
        } else {
            self.pending.push(ev);
            (self.pending.len() - 1) as u32
        };
        if prev == NONE {
            self.l_pending[l] = idx;
        } else {
            self.pending[prev as usize].next = idx;
        }
    }

    #[allow(clippy::too_many_arguments, reason = "Internal lane scheduler takes a track/lane address and the pending animation fields")]
    pub(crate) fn schedule(&mut self, t: usize, lane: usize, at: f64, target: f32, curve: Curve, blend: Blend, velocity: Option<f32>) {
        let l = self.t_lane0[t] as usize + lane;
        self.push_pending(l, Pending { at, target, velocity: velocity.unwrap_or(f32::NAN), curve, blend, next: NONE });
    }

    pub(crate) fn cancel_track_pending(&mut self, t: usize) {
        let l0 = self.t_lane0[t] as usize;
        for l in l0..l0 + self.t_n[t] as usize {
            self.cancel_pending(l);
        }
    }

    /// After `schedule` calls: recompute timing and wake the track.
    pub(crate) fn commit_scheduled(&mut self, t: usize, logical_at: f64, token: u64) {
        self.take_token(t, false);
        let m = &mut self.t_meta[t];
        m.token = token;
        m.logical_at = logical_at;
        m.epoch = m.epoch.wrapping_add(1);
        self.t_held[t] = false;
        self.recompute(t);
        self.activate(t);
        self.starts.push(self.id_of(t));
    }

    fn cancel_pending(&mut self, l: usize) {
        let mut p = self.l_pending[l];
        while p != NONE {
            let next = self.pending[p as usize].next;
            self.pending[p as usize].next = self.pending_free;
            self.pending_free = p;
            p = next;
        }
        self.l_pending[l] = NONE;
    }

    fn alloc_extra(&mut self, e: Extra) -> u32 {
        if self.extra_free != NONE {
            let i = self.extra_free;
            self.extra_free = self.extra[i as usize].next;
            self.extra[i as usize] = e;
            i
        } else {
            self.extra.push(e);
            (self.extra.len() - 1) as u32
        }
    }

    fn free_extra(&mut self, l: usize) {
        let mut m = self.l_more[l];
        while m != NONE {
            let next = self.extra[m as usize].next;
            self.extra[m as usize].next = self.extra_free;
            self.extra_free = m;
            m = next;
        }
        self.l_more[l] = NONE;
    }

    fn unlink_node(&mut self, node: NodeId, t: u32) {
        let head = self.node_head.get(&node.0).copied().unwrap_or(NONE);
        if head == t {
            let next = self.t_meta[t as usize].node_next;
            if next == NONE {
                self.node_head.remove(&node.0);
            } else {
                self.node_head.insert(node.0, next);
            }
            return;
        }
        let mut cur = head;
        while cur != NONE {
            let next = self.t_meta[cur as usize].node_next;
            if next == t {
                self.t_meta[cur as usize].node_next = self.t_meta[t as usize].node_next;
                return;
            }
            cur = next;
        }
    }

    fn free_track(&mut self, t: usize) {
        if !self.t_meta[t].alive {
            return;
        }
        let l0 = self.t_lane0[t] as usize;
        let n = self.t_n[t] as usize;
        for l in l0..l0 + n {
            self.cancel_pending(l);
            self.free_extra(l);
        }
        if self.t_slot[t] != NONE {
            let slot = self.t_slot[t] as usize;
            self.deactivate(t, slot);
        }
        let m = self.t_meta[t];
        self.index.remove(&(m.node.0, m.prop.0));
        self.lane_free[n].push(l0 as u32);
        // Removal cancels the animation: whoever waits on its completion hears so.
        self.take_token(t, false);
        self.t_meta[t].alive = false;
        self.track_free.push(t as u32);
    }
}

impl Meta {
    fn new(node: NodeId, prop: PropId, class: PropClass, generation: u32, node_next: u32) -> Meta {
        Meta { node, prop, class, generation, alive: true, epoch: 0, token: 0, logical_at: f64::NEG_INFINITY, node_next }
    }
}
