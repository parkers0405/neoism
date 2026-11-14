//! The presented tree the render thread owns: presence (exits that stay mounted), layout animation
//! by edges, screen-space travel, matched geometry, and the per-frame [`Presented`] output.

use crate::curve::{Anim, Curve};
use crate::props::{NodeId, Prop, PropId, prop};
use crate::snap::snap_rect;
use crate::spring::{Spring, SpringPair};
use crate::store::{AnimationStore, FxMap, MotionEvent, TrackId};
use crate::trace::{Intent, MotionTrace, flags};
use crate::transition::{Effect, Reveal, Transition};
use crate::value::{Animatable, Color, Corners, MAX_LANES, Rect, Sides, Transform, Vec2, lanes};

mod presence;
mod props;
mod trailing;
mod travel;

const NONE: u32 = u32::MAX;
const B: usize = prop::BUILTIN_COUNT;

/// Width-leads ratio for the X axis spring (native morph: `width_vd = 0.78 · vd`).
pub const WIDTH_LEAD: f32 = 0.78;

/// Where a node is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Entering,
    Present,
    /// Removed by the app; still drawn until its exit settles. `generation` guards against stale exits.
    Exiting { generation: u32 },
}

/// Whether an insert is part of the first build (no animation, Kit rule 6) or a later change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mount {
    Initial,
    Animated,
}

/// The coordinate space layout motion is tracked in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Travel {
    /// Edges animate in parent space; an unchanged child rides its parent.
    #[default]
    Parent,
    /// The node travels one straight screen-space path from where it was seen to where it lands,
    /// whatever its ancestors do (Kit rule 12). Interpolates start and end root-space rects with
    /// one progress per axis.
    Screen,
}

/// How a frame change animates: per-axis curves, directional edge lead, and travel space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutAnim {
    pub x: Anim,
    pub y: Anim,
    pub lead: Option<(Anim, Anim)>,
    pub travel: Travel,
}

impl LayoutAnim {
    pub fn new(anim: impl Into<Anim>) -> LayoutAnim {
        let a = anim.into();
        LayoutAnim { x: a, y: a, lead: None, travel: Travel::Parent }
    }
    pub fn axes(x: impl Into<Anim>, y: impl Into<Anim>) -> LayoutAnim {
        LayoutAnim { x: x.into(), y: y.into(), lead: None, travel: Travel::Parent }
    }
    /// The X axis springs faster (duration × 0.78), so a widening surface fits its content first
    /// (Kit rule 9: "the width should animate much faster").
    pub fn width_leads(mut self) -> LayoutAnim {
        if let Curve::Spring(s) = self.x.curve {
            self.x.curve = Curve::Spring(s.with_duration(s.duration * WIDTH_LEAD));
        }
        self
    }
    /// When an axis travels, its leading edge in the direction of travel uses `lead` and its
    /// trailing edge uses `trail` (width leads along the travel direction).
    pub fn lead(mut self, lead: impl Into<Anim>, trail: impl Into<Anim>) -> LayoutAnim {
        self.lead = Some((lead.into(), trail.into()));
        self
    }
    pub fn screen(mut self) -> LayoutAnim {
        self.travel = Travel::Screen;
        self
    }
    pub const INSTANT: LayoutAnim =
        LayoutAnim { x: Anim::INSTANT, y: Anim::INSTANT, lead: None, travel: Travel::Parent };
}

impl From<Spring> for LayoutAnim {
    fn from(s: Spring) -> Self {
        LayoutAnim::new(s)
    }
}
impl From<Anim> for LayoutAnim {
    fn from(a: Anim) -> Self {
        LayoutAnim::new(a)
    }
}
impl From<SpringPair> for LayoutAnim {
    fn from(p: SpringPair) -> Self {
        LayoutAnim::new(p.enter)
    }
}

/// What the app side sends when a keyed node appears.
#[derive(Clone, Copy, Debug)]
pub struct NodeSpec {
    pub parent: Option<NodeId>,
    /// Layout frame in parent space, at the target state.
    pub frame: Rect,
    pub opacity: f32,
    pub radius: Corners,
    pub fill: Color,
    pub transition: Transition,
}

impl NodeSpec {
    pub fn new(frame: Rect) -> NodeSpec {
        NodeSpec {
            parent: None,
            frame,
            opacity: 1.0,
            radius: Corners::ZERO,
            fill: Color::TRANSPARENT,
            transition: Transition::opacity(),
        }
    }
    pub fn parent(mut self, parent: NodeId) -> Self {
        self.parent = Some(parent);
        self
    }
    pub fn opacity(mut self, o: f32) -> Self {
        self.opacity = o;
        self
    }
    pub fn radius(mut self, r: impl Into<Corners>) -> Self {
        self.radius = r.into();
        self
    }
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = c;
        self
    }
    pub fn transition(mut self, t: Transition) -> Self {
        self.transition = t;
        self
    }
}

impl From<f32> for Corners {
    fn from(r: f32) -> Corners {
        Corners::all(r)
    }
}

/// A soft clip in the node's own coordinates: how far each side is pulled in, and how wide the
/// gradient ramp on that side is. The renderer evaluates it as an SDF ramp (Kit rule 11).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mask {
    pub inset: Sides,
    pub feather: Sides,
}

impl Mask {
    pub fn is_none(&self) -> bool {
        self.inset == Sides::ZERO && self.feather == Sides::ZERO
    }
}

/// Everything the renderer needs to draw one node this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presented {
    pub node: NodeId,
    pub parent: Option<NodeId>,
    /// Root-space rect including every ancestor's (and this node's) translation, both edges
    /// snapped to 1/64 device px. Shapes, borders and shadows are drawn at this size.
    pub rect: Rect,
    /// This node's own opacity (presence × property). Ancestors apply theirs as group opacity.
    pub opacity: f32,
    /// Scale and rotation about `anchor` (translation is already in `rect`). Ancestors' scale and
    /// rotation compose down the tree, like `CALayer`.
    pub transform: Transform,
    /// Root-space pivot for `transform`.
    pub anchor: Vec2,
    /// Own plus presence Gaussian blur sigma in points. Zero means "attach no filter".
    pub blur: f32,
    pub radius: Corners,
    pub fill: Color,
    pub mask: Mask,
    pub phase: Phase,
    /// False while exiting with `pop_layout`: the app's layout should no longer reserve space.
    pub in_flow: bool,
    /// motion-lint flags (animated, ancestorAnimated, clipped, filtered) and intent bits
    /// (own `<< 8`, ancestor `<< 16`).
    pub flags: u32,
    /// How much of this node's own shape is drawn, 0 to 1 ([`crate::Transition::draw`]).
    pub draw: f32,
}

impl Presented {
    /// The 2×3 affine for `transform` about `anchor`.
    pub fn affine(&self) -> [f32; 6] {
        self.transform.to_affine(self.anchor)
    }
}

/// One frame's output.
pub struct FrameOutput<'a> {
    /// Nodes whose presented values changed, parents before children.
    pub changed: &'a [Presented],
    /// Nodes whose exit settled this frame: draw nothing for them from now on.
    pub exited: &'a [NodeId],
    pub events: &'a [MotionEvent],
    /// Anything still moving? If not, the display link can pause.
    pub animating: bool,
}

#[derive(Clone, Copy)]
struct Path {
    start: Rect,
    end_local: Rect,
    /// Its parent's root-space target origin as the last frame drew it.
    origin: Vec2,
}

// Effect state lanes: opacity, scale, offset x/y, size-fraction offset x/y, blur, hidden reveal,
// drawn shape.
type Fx = [f32; 9];
const FX_ID: Fx = [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];

fn fx_of(e: &Effect) -> Fx {
    let hidden = if e.reveal.is_some() { 1.0 } else { 0.0 };
    [e.opacity, e.scale, e.offset.x, e.offset.y, e.offset_fraction.x, e.offset_fraction.y, e.blur, hidden, e.draw]
}

#[derive(Clone)]
struct Node {
    id: NodeId,
    parent: u32,
    first_child: u32,
    last_child: u32,
    next_sibling: u32,
    prev_sibling: u32,
    alive: bool,
    frame: Rect,
    opacity: f32,
    radius: Corners,
    fill: Color,
    transition: Transition,
    phase: Phase,
    generation: u32,
    /// Presence effect at p = 0 and p = 1 (normally the effect and identity; re-solved on a
    /// mid-flight reversal so nothing jumps).
    fx_a: Fx,
    fx_b: Fx,
    anchor: Vec2,
    reveal: Option<Reveal>,
    path: Option<Path>,
    /// `set_ride_trailing`: x is kept against the parent's trailing edge while its width moves.
    ride_trailing: bool,
    tracks: [Option<TrackId>; B],
    dirty: bool,
    /// Root-space layout rect this frame (unsnapped) and the origin children ride (with offsets).
    world: Rect,
    origin: Vec2,
    last: Option<Presented>,
}

/// Where a node comes to rest (see [`Scene::rest`]), beside `Scene::nodes` so the nodes the
/// renderer reads every frame stay the same size.
#[derive(Clone, Copy)]
struct Rest {
    /// Root-space presented rect with every track at its target, and the origin children rest on.
    rect: Rect,
    origin: Vec2,
    /// `rect` differs from where the node is this frame.
    moving: bool,
}

const REST_ZERO: Rest = Rest { rect: Rect::ZERO, origin: Vec2::ZERO, moving: false };

/// A small dual number: value and time derivative, for exact velocity of composed geometry.
#[derive(Clone, Copy, Default)]
struct D {
    x: f32,
    v: f32,
}
impl D {
    fn c(x: f32) -> D {
        D { x, v: 0.0 }
    }
    fn add(self, o: D) -> D {
        D { x: self.x + o.x, v: self.v + o.v }
    }
    fn sub(self, o: D) -> D {
        D { x: self.x - o.x, v: self.v - o.v }
    }
    fn mul(self, o: D) -> D {
        D { x: self.x * o.x, v: self.v * o.x + self.x * o.v }
    }
}

/// The render thread's animated tree.
pub struct Scene {
    store: AnimationStore,
    nodes: Vec<Node>,
    rests: Vec<Rest>,
    free: Vec<u32>,
    index: FxMap<u64, u32>,
    root_head: u32,
    root_tail: u32,
    order: Vec<u32>,
    order_dirty: bool,
    out: Vec<Presented>,
    /// What [`Scene::present`] presented after the motion trace wrote its frame, for the next.
    late: Vec<Presented>,
    exited: Vec<NodeId>,
    exited_seen: usize,
    to_remove: Vec<u32>,
    travelling: Vec<u32>,
    trace: Option<MotionTrace>,
    intents: Vec<(NodeId, u32, f32)>,
    /// `keep_flights`: landings leave motion already on its way to the same value.
    keep: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    pub fn new() -> Scene {
        Scene {
            store: AnimationStore::new(),
            nodes: Vec::new(),
            rests: Vec::new(),
            free: Vec::new(),
            index: FxMap::default(),
            root_head: NONE,
            root_tail: NONE,
            order: Vec::new(),
            order_dirty: false,
            out: Vec::new(),
            late: Vec::new(),
            exited: Vec::new(),
            exited_seen: 0,
            to_remove: Vec::new(),
            travelling: Vec::new(),
            trace: None,
            intents: Vec::new(),
            keep: false,
        }
    }

    /// While on, landing a property ([`Scene::set`], or [`Scene::set_frame`] with
    /// [`LayoutAnim::INSTANT`]) on the value it's already moving to leaves that motion running:
    /// an instant change lands what it changes, not what an earlier change set going. Off, a
    /// landing ends any motion, as settling needs.
    pub fn keep_flights(&mut self, on: bool) {
        self.keep = on;
    }

    /// `prop` is moving and its target is `value`, so a landing under `keep_flights` leaves it.
    fn in_flight_to<T: Animatable>(&self, id: NodeId, prop: Prop<T>, value: T) -> bool {
        if !self.keep || !self.store.is_animating(id, prop) {
            return false;
        }
        let Some(to) = self.store.target(id, prop) else { return false };
        let (mut a, mut b) = ([0.0; MAX_LANES], [0.0; MAX_LANES]);
        to.write_lanes(&mut a[..T::LANES]);
        value.write_lanes(&mut b[..T::LANES]);
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 1e-3)
    }

    pub fn store(&self) -> &AnimationStore {
        &self.store
    }
    /// For choreography and custom properties. Tracks of scene nodes are picked up automatically.
    pub fn store_mut(&mut self) -> &mut AnimationStore {
        &mut self.store
    }

    pub fn set_time(&mut self, t: f64) {
        self.store.set_time(t);
    }
    /// Runs the node's animations at `scale` times the scene's speed; see
    /// [`AnimationStore::set_node_time_scale`].
    pub fn set_time_scale(&mut self, id: NodeId, scale: f32) {
        self.store.set_node_time_scale(id, scale);
    }
    pub fn now(&self) -> f64 {
        self.store.now()
    }
    pub fn set_pixel_scale(&mut self, s: f32) {
        let previous = self.store.pixel_scale();
        self.store.set_pixel_scale(s);
        if self.store.pixel_scale() != previous {
            for node in self.nodes.iter_mut().filter(|n| n.alive) { node.dirty = true; }
        }
    }
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.store.set_reduced_motion(on);
        for n in self.nodes.iter_mut().filter(|n| n.alive) {
            n.dirty = true;
        }
    }

    /// Records every frame as motion-lint JSONL.
    pub fn set_trace(&mut self, trace: Option<MotionTrace>) {
        self.trace = trace;
        for node in self.nodes.iter_mut().filter(|n| n.alive) { node.dirty = true; }
    }
    pub fn tracing(&self) -> bool { self.trace.is_some() }
    pub fn trace_mut(&mut self) -> Option<&mut MotionTrace> { self.trace.as_mut() }
    /// Updates metadata even if the node's geometry has not changed.
    pub fn trace_name(&mut self, id: NodeId, name: Option<&'static str>) {
        if self.trace.as_mut().is_some_and(|trace| trace.name(id, name)) {
            if let Some(i) = self.slot(id) { self.nodes[i].dirty = true; }
        }
    }
    pub fn trace_namespace(&mut self, id: NodeId, namespace: Option<(&'static str, &'static str)>) {
        if self.trace.as_mut().is_some_and(|trace| trace.namespace(id, namespace)) {
            if let Some(i) = self.slot(id) { self.nodes[i].dirty = true; }
        }
    }
    pub fn take_trace(&mut self) -> Option<MotionTrace> {
        self.trace.take()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.index.contains_key(&id.0)
    }
    pub fn phase(&self, id: NodeId) -> Option<Phase> {
        self.slot(id).map(|i| self.nodes[i].phase)
    }
    /// The last presented snapshot (hit-test against what the user sees). Not tracked.
    pub fn presented(&self, id: NodeId) -> Option<Presented> {
        self.slot(id).and_then(|i| self.nodes[i].last)
    }
    /// The rect the node comes to rest at once every track (its own and its ancestors') reaches
    /// its target, snapped like [`Presented::rect`], while that differs from where it is: the
    /// difference from [`presented`](Self::presented) is the motion still in flight. Only a
    /// node whose [`Presented::flags`] has `ANIMATED` or `ANCESTOR_ANIMATED` can be moving.
    pub fn rest(&self, id: NodeId) -> Option<Rect> {
        let r = &self.rests[self.slot(id)?];
        r.moving.then(|| snap_rect(r.rect, self.store.pixel_scale()))
    }
    /// Root-space layout rect this frame (without this node's own offset or presence offset),
    /// snapped to the same 1/64 device-px grid as [`Presented::rect`].
    pub fn world(&self, id: NodeId) -> Option<Rect> {
        self.slot(id).map(|i| snap_rect(self.nodes[i].world, self.store.pixel_scale()))
    }
    pub fn len(&self) -> usize {
        self.index.len()
    }
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    fn slot(&self, id: NodeId) -> Option<usize> {
        self.index.get(&id.0).map(|&i| i as usize)
    }

    /// Lane values and velocities of track `t`, sampled exactly at `now()`.
    fn sample(&self, t: TrackId) -> Option<([f32; MAX_LANES], [f32; MAX_LANES])> {
        let mut x = [0.0; MAX_LANES];
        let mut v = [0.0; MAX_LANES];
        self.store.sample_exact(t, &mut x, &mut v).then_some((x, v))
    }

    // ------------------------------------------------------------------------------------------
    // Layout

    /// The node's layout frame (parent space) changed. Each moving edge animates; a stationary
    /// edge gets no motion at all. With `Travel::Screen` the node takes one straight root-space
    /// path instead, whatever its ancestors do.
    pub fn set_frame(&mut self, id: NodeId, frame: Rect, anim: impl Into<LayoutAnim>) {
        self.set_frame_drawn(id, frame, anim.into(), [Vec2::ZERO; 2]);
    }

    /// [`set_frame`](Self::set_frame) for a node drawn `drawn[0]` away from its scene rect,
    /// moving `drawn[1]` faster: a screen path starts there, at that speed.
    pub(super) fn set_frame_drawn(&mut self, id: NodeId, frame: Rect, anim: LayoutAnim, drawn: [Vec2; 2]) {
        let Some(i) = self.slot(id) else { return };
        let old = self.nodes[i].frame;
        if old == frame && self.nodes[i].path.is_none() && self.nodes[i].tracks[prop::FRAME.id.0 as usize].is_none() {
            return;
        }
        if frame.w != old.w {
            self.rebase_trailing(i, frame.w - old.w);
        }
        let had_path = self.nodes[i].path.is_some();
        // An explicit landing also cancels motion toward an unchanged target. Comparing only
        // target frames here used to leave a spring running after Runtime::settle_all().
        if anim == LayoutAnim::INSTANT && !had_path && self.in_flight_to(id, prop::FRAME, frame) {
            return;
        }
        if anim == LayoutAnim::INSTANT && (had_path || self.store.is_animating(id, prop::FRAME)) {
            self.end_path(i);
            self.nodes[i].frame = frame;
            self.track_set(i, prop::FRAME, frame);
            self.nodes[i].dirty = true;
            return;
        }
        if old == frame && !had_path {
            return;
        }
        let ms = match anim.x.curve {
            Curve::Spring(s) => s.duration,
            c => c.logical_duration(),
        };
        self.intent(id, Intent::POSITION | Intent::SIZE, ms);
        if anim.travel == Travel::Screen && !self.store.reduced_motion() {
            // A path restarts from where it was drawn: under the origin the last frame drew it
            // against, which an ancestor laid out elsewhere in this commit has already moved.
            let (mut start, mut vel) = match self.nodes[i].path {
                Some(path) => {
                    let e = self.path_dual(i, path, path.origin);
                    (e.map(|d| d.x), e.map(|d| d.v))
                }
                None => self.exact_world(i),
            };
            for k in 0..4 {
                let [offset, speed] = drawn.map(|v| if k % 2 == 0 { v.x } else { v.y });
                start[k] += offset;
                vel[k] += speed;
            }
            self.nodes[i].frame = frame;
            self.start_path(i, start, vel, anim);
            return;
        }
        if had_path {
            // Leave screen travel: continue in parent space from the exact current place.
            let (w, wv) = self.exact_world(i);
            let (mut po, mut pv) = self.parent_origin_exact(i);
            let trail = self.trail_dual(i);
            po.x += trail.x;
            pv.x += trail.v;
            let local = Rect::from_edges(w[0] - po.x, w[1] - po.y, w[2] - po.x, w[3] - po.y);
            let lv = [wv[0] - pv.x, wv[1] - pv.y, wv[2] - pv.x, wv[3] - pv.y];
            self.end_path(i);
            self.track_set(i, prop::FRAME, local);
            let t = self.nodes[i].tracks[prop::FRAME.id.0 as usize].unwrap();
            // Give the lanes their velocity, then retarget per axis.
            let tid = t.index_usize();
            self.store.animate_raw(tid, &local.edges(), Anim::new(Curve::Spring(Spring::SMOOTH)), Some(&lv));
        } else if self.nodes[i].tracks[prop::FRAME.id.0 as usize].is_none() {
            self.track_set(i, prop::FRAME, old);
        }
        self.nodes[i].frame = frame;
        let t = self.nodes[i].tracks[prop::FRAME.id.0 as usize].unwrap().index_usize();
        let mut cur = self.store.lane_targets_4(t);
        let e = frame.edges();
        if let Some((lead, trail)) = anim.lead {
            for axis in 0..2 {
                let (a, b) = (axis, axis + 2);
                let (sa, sb) = if e[a] + e[b] > cur[a] + cur[b] { (trail, lead) } else { (lead, trail) };
                let mut tg = cur;
                tg[a] = e[a];
                self.store.animate_raw(t, &tg, sa, None);
                cur[a] = e[a];
                let mut tg = cur;
                tg[b] = e[b];
                self.store.animate_raw(t, &tg, sb, None);
                cur[b] = e[b];
            }
        } else {
            // X edges on the X curve, Y edges on the Y curve; unchanged lanes are skipped.
            self.store.animate_raw(t, &[e[0], cur[1], e[2], cur[3]], anim.x, None);
            self.store.animate_raw(t, &e, anim.y, None);
        }
        self.nodes[i].dirty = true;
    }

    fn parent_target_origin(&self, i: usize) -> Vec2 {
        let mut o = Vec2::ZERO;
        let mut p = self.nodes[i].parent;
        while p != NONE {
            let f = self.nodes[p as usize].frame;
            o.x += f.x;
            o.y += f.y;
            p = self.nodes[p as usize].parent;
        }
        o
    }

    /// Root-space layout edges of node `i` at `now()`, with velocities, sampled exactly.
    fn exact_world(&self, i: usize) -> ([f32; 4], [f32; 4]) {
        let e = self.world_dual(i);
        (e.map(|d| d.x), e.map(|d| d.v))
    }

    /// Trace-only check of presented scene edges against unsnapped analytic tracks at `now`.
    /// Returns (edge samples, maximum device-pixel error). Renderer-owned growth/scroll and
    /// post-layout scale transforms are outside this metric.
    pub fn sampling_error(&self) -> (usize, f32) {
        let mut count = 0;
        let mut error = 0.0f32;
        for &slot in &self.order {
            let i = slot as usize;
            let Some(p) = self.nodes[i].last else { continue };
            if !self.nodes[i].alive { continue; }
            let w = self.world_dual(i);
            let o = self.origin_dual(i);
            let ideal = [o[0].x, o[1].x, o[0].x + w[2].x - w[0].x, o[1].x + w[3].x - w[1].x];
            for (shown, exact) in p.rect.edges().into_iter().zip(ideal) {
                error = error.max((shown - exact).abs() * self.store.pixel_scale());
                count += 1;
            }
        }
        (count, error)
    }

    fn parent_origin_exact(&self, i: usize) -> (Vec2, Vec2) {
        let p = self.nodes[i].parent;
        if p == NONE {
            return (Vec2::ZERO, Vec2::ZERO);
        }
        let o = self.origin_dual(p as usize);
        (Vec2::new(o[0].x, o[1].x), Vec2::new(o[0].v, o[1].v))
    }

    /// Overwrites `out` with the property's exact lanes, if its track has a sample.
    fn lanes_dual(&self, i: usize, p: PropId, out: &mut [D]) {
        let Some((x, v)) = self.nodes[i].tracks[p.0 as usize].and_then(|t| self.sample(t)) else { return };
        for (k, o) in out.iter_mut().enumerate() {
            *o = D { x: x[k], v: v[k] };
        }
    }

    fn world_dual(&self, i: usize) -> [D; 4] {
        let n = &self.nodes[i];
        if let Some(path) = n.path {
            return self.path_dual(i, path, self.parent_target_origin(i));
        }
        self.laid_dual(i)
    }

    /// Where node `i` is on its screen path, ending at its layout under parent origin `origin`.
    fn path_dual(&self, i: usize, path: Path, origin: Vec2) -> [D; 4] {
        let mut p = [D::c(1.0); 2];
        self.lanes_dual(i, prop::TRAVEL.id, &mut p);
        let s = path.start.edges();
        let end = path.end_local.translated(origin).edges();
        core::array::from_fn(|k| D::c(s[k]).add(D::c(end[k] - s[k]).mul(p[k % 2])))
    }

    /// The origin children of `i` ride: layout origin plus offsets (own and presence).
    fn origin_dual(&self, i: usize) -> [D; 2] {
        let w = self.world_dual(i);
        let mut off = [D::c(0.0); 2];
        self.lanes_dual(i, prop::OFFSET.id, &mut off);
        let mut p = [D::c(1.0)];
        self.lanes_dual(i, prop::PRESENCE.id, &mut p);
        let n = &self.nodes[i];
        let fx = |k: usize| D::c(n.fx_a[k]).mul(D::c(1.0).sub(p[0])).add(D::c(n.fx_b[k]).mul(p[0]));
        let size = [w[2].sub(w[0]), w[3].sub(w[1])];
        let reduced = self.store.reduced_motion();
        core::array::from_fn(|a| {
            let mut o = w[a].add(off[a]);
            if !reduced {
                o = o.add(fx(2 + a)).add(fx(4 + a).mul(size[a]));
            }
            o
        })
    }

    // ------------------------------------------------------------------------------------------
    // The frame

    /// `set_time(t)`, then [`frame`](Self::frame).
    pub fn frame_at(&mut self, t: f64) -> FrameOutput<'_> {
        self.set_time(t);
        self.frame()
    }

    /// Samples every active track at `now()`, finishes exits, and returns the presented values of
    /// every node that changed. Apply this frame's commits (insert/remove/set_frame/animate)
    /// between `set_time` and `frame`.
    pub fn frame(&mut self) -> FrameOutput<'_> {
        self.store.tick();
        // Keep removals made since the last frame (matched geometry), drop last frame's.
        self.exited.drain(..self.exited_seen);
        self.to_remove.clear();

        // Presence settles: finish entrances, retire exits whose generation is current.
        for ev in self.store.events() {
            if let MotionEvent::Settled { node, prop: p, .. } = *ev {
                if p != prop::PRESENCE.id {
                    continue;
                }
                let Some(&i) = self.index.get(&node.0) else { continue };
                let n = &mut self.nodes[i as usize];
                match n.phase {
                    Phase::Entering => n.phase = Phase::Present,
                    Phase::Exiting { generation } if generation == n.generation => self.to_remove.push(i),
                    _ => {}
                }
            }
        }
        // An exit nobody can see is over. Presence settles at 1e-5, ~40 frames after the node
        // fades below one 8-bit step; retire it there so whatever waits on the unmount (focus,
        // hit-testing, the next layer) doesn't wait on an invisible tail.
        let mut retire = core::mem::take(&mut self.to_remove);
        for (t, node, p) in self.store.touched_tracks() {
            if p != prop::PRESENCE.id {
                continue;
            }
            let Some(&i) = self.index.get(&node.0) else { continue };
            let n = &self.nodes[i as usize];
            let Phase::Exiting { generation } = n.phase else { continue };
            if generation != n.generation || retire.contains(&i) {
                continue;
            }
            let Some(&[presence, ..]) = self.store.values_of(t) else { continue };
            if n.fx_a[0] + (n.fx_b[0] - n.fx_a[0]) * presence < 1.0 / 255.0 {
                retire.push(i);
            }
        }
        self.to_remove = retire;
        // Mark what moved; refresh the track cache from the store.
        for (tid, node, p) in self.store.touched_tracks() {
            if let Some(&i) = self.index.get(&node.0) {
                let n = &mut self.nodes[i as usize];
                n.dirty = true;
                if (p.0 as usize) < B {
                    n.tracks[p.0 as usize] = Some(tid);
                }
            }
        }
        // Screen travel ends once it has arrived and no ancestor is still moving or transitioning.
        let mut k = 0;
        while k < self.travelling.len() {
            let i = self.travelling[k] as usize;
            let travel = self.nodes[i].tracks[prop::TRAVEL.id.0 as usize];
            let arrived = travel.is_none_or(|t| !self.store.is_active(t));
            if arrived && !self.ancestor_moving(i) {
                let end_local = self.nodes[i].path.map(|p| p.end_local).unwrap_or(self.nodes[i].frame);
                self.end_path(i);
                self.track_set(i, prop::FRAME, end_local);
                continue;
            }
            let po = self.parent_target_origin(i);
            if let Some(path) = &mut self.nodes[i].path {
                path.origin = po;
            }
            k += 1;
        }
        if self.order_dirty {
            self.rebuild_order();
        }
        self.out.clear();
        // Written first, so this frame's own values for the same nodes win.
        self.out.append(&mut self.late);
        self.present_dirty();
        for k in 0..self.to_remove.len() {
            let i = self.to_remove[k] as usize;
            if self.nodes[i].alive {
                self.remove_now(i);
            }
        }
        self.exited_seen = self.exited.len();
        // A node still travelling waits on an active track of its own or an ancestor's, so this
        // covers it, and a frozen one doesn't keep the frames coming.
        let animating = self.store.running();
        if let Some(trace) = self.trace.as_mut() {
            trace.frame(self.store.now(), &self.out, &self.exited, self.store.events(), &self.intents);
            self.intents.clear();
        }
        FrameOutput { changed: &self.out, exited: &self.exited, events: self.store.events(), animating }
    }

    /// Presents what was set or animated since [`frame`](Self::frame), without stepping time,
    /// so a value worked out from something that frame moved afterwards (a scroll offset)
    /// shows in the same frame. The motion trace, already written for that frame, records them
    /// with the next.
    pub fn present(&mut self) {
        if self.order_dirty {
            self.rebuild_order();
        }
        let from = self.out.len();
        self.present_dirty();
        if self.trace.is_some() {
            self.late.extend_from_slice(&self.out[from..]);
        }
    }

    /// Recomputes the presented values of dirty nodes and their subtrees, into `out`.
    fn present_dirty(&mut self) {
        let scale = self.store.pixel_scale();
        let reduced = self.store.reduced_motion();
        for k in 0..self.order.len() {
            let i = self.order[k] as usize;
            let parent = self.nodes[i].parent;
            let parent_dirty = parent != NONE && self.nodes[parent as usize].dirty;
            let own = self.nodes[i].dirty;
            if parent_dirty {
                self.nodes[i].dirty = true;
            }
            if self.nodes[i].dirty {
                let p = self.compute(i, scale, reduced, own, parent_dirty);
                self.nodes[i].last = Some(p);
                self.out.push(p);
            }
        }
        for k in 0..self.order.len() {
            let i = self.order[k] as usize;
            self.nodes[i].dirty = false;
        }
    }

    /// An ancestor still moving or changing how it draws its subtree: a node ending screen travel
    /// under it would jump onto its mid-transition offset, scale, opacity or blur.
    fn ancestor_moving(&self, i: usize) -> bool {
        let mut p = self.nodes[i].parent;
        while p != NONE {
            let n = &self.nodes[p as usize];
            for prop in [prop::FRAME.id, prop::TRAVEL.id, prop::OFFSET.id, prop::PRESENCE.id, prop::SCALE.id, prop::OPACITY.id, prop::BLUR.id] {
                if n.tracks[prop.0 as usize].is_some_and(|t| self.store.is_active(t)) {
                    return true;
                }
            }
            p = n.parent;
        }
        false
    }

    fn values(&self, i: usize, p: PropId) -> Option<&[f32]> {
        self.nodes[i].tracks[p.0 as usize].and_then(|t| self.store.values_of(t))
    }

    fn targets<const N: usize>(&self, i: usize, p: PropId) -> Option<[f32; N]> {
        let mut out = [0.0; N];
        let t = self.nodes[i].tracks[p.0 as usize]?;
        self.store.targets_of(t, &mut out).then_some(out)
    }

    /// Where `compute` would put node `i` with every track at its target. It repeats the same
    /// arithmetic so that, at rest, the two rects are bit-identical.
    fn rest_of(&self, i: usize, reduced: bool) -> Rect {
        let n = &self.nodes[i];
        let parent = n.parent;
        let world = match n.path {
            Some(path) => {
                let p = self.targets::<2>(i, prop::TRAVEL.id).unwrap_or([1.0, 1.0]);
                let s = path.start.edges();
                let e = path.end_local.translated(self.parent_target_origin(i)).edges();
                let ed: [f32; 4] = core::array::from_fn(|k| s[k] + (e[k] - s[k]) * p[k % 2]);
                Rect::from_edges(ed[0], ed[1], ed[2], ed[3])
            }
            None => {
                let origin = if parent == NONE { Vec2::ZERO } else { self.rests[parent as usize].origin };
                let local = self.targets::<4>(i, prop::FRAME.id).map_or(n.frame, |v| Rect::read_lanes(&v));
                local.translated(origin)
            }
        };
        let p = self.targets::<1>(i, prop::PRESENCE.id).map_or(1.0, |v| v[0]);
        let s = lerp_fx(&n.fx_a, &n.fx_b, p);
        let s = if reduced { [s[0], 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] } else { s };
        let own = self.targets::<2>(i, prop::OFFSET.id).map_or(Vec2::ZERO, |v| Vec2::read_lanes(&v));
        world.translated(Vec2::new(own.x + s[2] + s[4] * world.w, own.y + s[3] + s[5] * world.h))
    }

    fn compute(&mut self, i: usize, scale: f32, reduced: bool, own_dirty: bool, parent_dirty: bool) -> Presented {
        let parent = self.nodes[i].parent;
        let parent_origin = if parent == NONE { Vec2::ZERO } else { self.nodes[parent as usize].origin };
        let n = &self.nodes[i];
        let world = match n.path {
            Some(path) => {
                let p = self.values(i, prop::TRAVEL.id).map_or([1.0, 1.0], |v| [v[0], v[1]]);
                let s = path.start.edges();
                let e = path.end_local.translated(self.parent_target_origin(i)).edges();
                let ed: [f32; 4] = core::array::from_fn(|k| s[k] + (e[k] - s[k]) * p[k % 2]);
                Rect::from_edges(ed[0], ed[1], ed[2], ed[3])
            }
            None => {
                let local = self.values(i, prop::FRAME.id).map_or(n.frame, Rect::read_lanes);
                let mut world = local.translated(parent_origin);
                if n.ride_trailing && parent != NONE {
                    let p = &self.nodes[parent as usize];
                    world.x += p.world.w - p.frame.w;
                }
                world
            }
        };
        let p = self.values(i, prop::PRESENCE.id).map_or(1.0, |v| v[0]);
        let s = lerp_fx(&n.fx_a, &n.fx_b, p);
        let s = if reduced { [s[0], 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] } else { s };
        let own_opacity = self.values(i, prop::OPACITY.id).map_or(n.opacity, |v| v[0]);
        let own_scale = self.values(i, prop::SCALE.id).map_or(1.0, |v| v[0]);
        let own_offset = self.values(i, prop::OFFSET.id).map_or(Vec2::ZERO, Vec2::read_lanes);
        let rotation = self.values(i, prop::ROTATION.id).map_or(0.0, |v| v[0]);
        let own_blur = self.values(i, prop::BLUR.id).map_or(0.0, |v| v[0]);
        let radius = self.values(i, prop::RADIUS.id).map_or(n.radius, Corners::read_lanes);
        let fill = self.values(i, prop::FILL.id).map_or(n.fill, Color::read_lanes);
        let own_mask = self.values(i, prop::MASK.id).map_or(Sides::ZERO, Sides::read_lanes);

        let offset = Vec2::new(
            own_offset.x + s[2] + s[4] * world.w,
            own_offset.y + s[3] + s[5] * world.h,
        );
        let shown = world.translated(offset);
        let sc = (own_scale * s[1]).max(0.0);
        let mut blur = (own_blur + s[6]).max(0.0);
        if blur < 1.0 / 64.0 {
            blur = 0.0;
        }
        let mut mask = Mask { inset: own_mask, feather: Sides::ZERO };
        if let Some(rv) = n.reveal {
            let hidden = s[7].clamp(0.0, 1.0);
            if hidden > 0.0 {
                let (side, len) = match rv.edge {
                    crate::Edge::Leading => (0, shown.w),
                    crate::Edge::Top => (1, shown.h),
                    crate::Edge::Trailing => (2, shown.w),
                    crate::Edge::Bottom => (3, shown.h),
                };
                // The content is hidden behind `edge`: the opposite side's inset grows.
                let opposite = (side + 2) % 4;
                mask.inset.0[opposite] += hidden * len;
                // The ramp fades in with the wipe instead of popping (min(1, travel / feather)).
                mask.feather.0[opposite] = rv.feather * (hidden * len / rv.feather.max(1e-3)).min(1.0);
            }
        }
        let anchor = Vec2::new(shown.x + n.anchor.x * shown.w, shown.y + n.anchor.y * shown.h);
        let mut fl = 0u32;
        let mut own_bits = 0u32;
        for (k, t) in n.tracks.iter().enumerate() {
            if t.is_some_and(|t| self.store.is_active(t)) {
                own_bits |= Intent::for_prop(PropId(k as u16));
            }
        }
        if n.path.is_some() {
            own_bits |= Intent::POSITION | Intent::SIZE;
        }
        if own_bits != 0 || (own_dirty && !parent_dirty) {
            fl |= flags::ANIMATED;
        }
        if parent_dirty {
            fl |= flags::ANCESTOR_ANIMATED;
            let pb = self.nodes[parent as usize].last.map_or(0, |p| (p.flags >> 8) & 0xff | (p.flags >> 16) & 0xff);
            fl |= pb << 16;
        }
        fl |= own_bits << 8;
        if !mask.is_none() {
            fl |= flags::CLIPPED;
        }
        if blur > 0.0 {
            fl |= flags::FILTERED;
        }
        let in_flow = !matches!(n.phase, Phase::Exiting { .. }) || !n.transition.pop_layout;
        let out = Presented {
            node: n.id,
            parent: (parent != NONE).then(|| self.nodes[parent as usize].id),
            rect: snap_rect(shown, scale),
            opacity: (own_opacity * s[0]).clamp(0.0, 1.0),
            transform: Transform { translate: Vec2::ZERO, scale: Vec2::new(sc, sc), rotate: rotation },
            anchor,
            blur,
            radius,
            fill,
            mask,
            phase: n.phase,
            in_flow,
            flags: fl,
            draw: s[8].clamp(0.0, 1.0),
        };
        // Where it is is where it rests while nothing that places it is in flight. A node riding
        // its parent's trailing edge also moves with the parent's width.
        let still = !self.nodes[i].ride_trailing
            && (parent == NONE || self.rests[parent as usize].origin == self.nodes[parent as usize].origin) && [prop::TRAVEL.id, prop::FRAME.id, prop::PRESENCE.id, prop::OFFSET.id]
            .iter()
            .all(|p| self.nodes[i].tracks[p.0 as usize].is_none_or(|t| self.store.is_settled(t)));
        let rest = if still { shown } else { self.rest_of(i, reduced) };
        let n = &mut self.nodes[i];
        n.world = world;
        n.origin = Vec2::new(shown.x, shown.y);
        self.rests[i] = Rest { rect: rest, origin: Vec2::new(rest.x, rest.y), moving: rest != shown };
        out
    }

    /// Unmounts a node and its subtree now (no exit), reporting them in `exited`.
    fn remove_now(&mut self, i: usize) {
        // Children first.
        let mut c = self.nodes[i].first_child;
        while c != NONE {
            let next = self.nodes[c as usize].next_sibling;
            self.remove_now(c as usize);
            c = next;
        }
        let id = self.nodes[i].id;
        // Unlink from the parent's (or root) child list.
        let parent = self.nodes[i].parent;
        let next = self.nodes[i].next_sibling;
        let prev = self.nodes[i].prev_sibling;
        if prev == NONE {
            if parent == NONE {
                self.root_head = next;
            } else {
                self.nodes[parent as usize].first_child = next;
            }
        } else {
            self.nodes[prev as usize].next_sibling = next;
        }
        if next == NONE {
            if parent == NONE { self.root_tail = prev; } else { self.nodes[parent as usize].last_child = prev; }
        } else {
            self.nodes[next as usize].prev_sibling = prev;
        }
        self.travelling.retain(|&x| x as usize != i);
        self.store.remove_node(id);
        self.index.remove(&id.0);
        let n = &mut self.nodes[i];
        n.alive = false;
        n.dirty = false;
        n.first_child = NONE;
        n.last_child = NONE;
        n.next_sibling = NONE;
        n.prev_sibling = NONE;
        n.last = None;
        self.free.push(i as u32);
        self.exited.push(id);
        self.order_dirty = true;
    }

    fn rebuild_order(&mut self) {
        self.order.clear();
        let mut r = self.root_head;
        while r != NONE {
            self.order.push(r);
            r = self.nodes[r as usize].next_sibling;
        }
        let mut k = 0;
        while k < self.order.len() {
            let mut c = self.nodes[self.order[k] as usize].first_child;
            while c != NONE {
                self.order.push(c);
                c = self.nodes[c as usize].next_sibling;
            }
            k += 1;
        }
        self.order_dirty = false;
    }
}

/// Exact at both ends: `a` at p = 0, `b` at p = 1.
fn lerp_fx(a: &Fx, b: &Fx, p: f32) -> Fx {
    core::array::from_fn(|k| a[k] * (1.0 - p) + b[k] * p)
}
