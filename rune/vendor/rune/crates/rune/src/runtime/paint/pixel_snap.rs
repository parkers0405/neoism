//! Paint-time pixel snapping (`El::pixel_snap`). Layout stays fractional; only where a box's
//! fill, border and clips land on the device grid changes, so a 1.1 pt gap at 2× still draws
//! hard edges.
//!
//! Edges snap, not origin plus size: each of a box's four edges rounds to the nearest device
//! pixel, so two boxes that share an edge round it the same way and meet with no seam or
//! overlap. A box at least half a pixel across keeps at least one pixel; a thinner one keeps
//! its exact edges on that axis rather than vanish.
//!
//! Motion: a moving box's edges are corrected toward the rounding of where it comes to rest
//! (every track at its target), not of where it is, so it glides at full sub-pixel precision
//! with no stepping. When the rest changes, each edge eases from the correction it drew with
//! at that moment to the new rest's, in proportion to how far it has left to travel, so a
//! motion starts where the box was and its last frame is the snapped resting rect: no jump at
//! either end. Boxes that move together share their progress, so a child keeps its place
//! against its parent's edges as both glide. Translations that are not animation tracks
//! (scroll offsets, sticky shifts, rows growing above) move a whole subtree together: they snap
//! as one rounded translation, so the rows of a scrolling list stay crisp and keep their
//! spacing every frame. A scaled subtree doesn't snap while the scale moves its edges by a
//! pixel or more; over the last pixel of a scale settling at 1 the snap fades in, so it lands
//! without a jump either. Turned subtrees don't snap.
//!
//! Scaled layers: a node with its own scale (a zoomed canvas, a pressed button) heads a layer,
//! and its subtree snaps against the grid where the scale puts it once the layer holds still.
//! While the layer moves (a zoom, a pinch, a spring), its content draws at its exact sub-pixel
//! edges, except that each box lets go of its resting correction over the first pixel that box
//! moves, so content at the pivot stays put. Once the layer holds still the correction to the
//! grid eases in over [`SETTLE`], so a zoom ends with no pop. Nothing depends on where a box
//! sits against the pivot: whether the layer moves is how far it moves points of the window,
//! and how near its scale is to 1 is one value for the whole layer, how far the scale moves the
//! layer's edges. Two layers that draw a point in the same place snap it alike, wherever each
//! is anchored, except while a scale is within a pixel of 1, where each lands on the unscaled
//! grid.
//!
//! Centring: a box whose resting edges sit centred in its parent's on an axis keeps equal
//! whole-pixel gaps to its parent's resting rounding there, and gives up to a pixel of its size
//! for it, as a 16 pt knob in a 20 pt track at 1.25× can't sit 2.5 px from each edge on whole
//! pixels. The gap rounds by size, not position, so the same control draws alike wherever it
//! sits. It is decided at rest, so a glide eases into and out of it.

use std::collections::HashMap;

use rune_motion::{NodeId, Presented, Rect};

/// How far each edge of a moving node is from where it comes to rest (device px), and that
/// resting rect (points), which keys its glide.
#[derive(Clone, Copy)]
pub(super) struct Motion {
    by: [f32; 4],
    rest: [f32; 4],
    /// The scale `by` is drawn at: 1 for the unscaled rect, a layer's scale for the drawn one.
    k: f32,
}

/// `p`'s motion toward `rest`, the rect the scene gives while the node is moving. An anchored
/// element is placed from its anchor, not its frame, so its frame's motion doesn't move it; a
/// growth row draws its bottom edge at its top plus its current height.
pub(super) fn motion(p: &Presented, rest: Option<Rect>, s: f32, anchored: bool, growth_row: bool) -> Option<Motion> {
    let rest = rest.filter(|_| !anchored)?;
    let (a, b) = (p.rect.edges(), rest.edges());
    let mut by: [f32; 4] = core::array::from_fn(|k| (a[k] - b[k]) * s);
    if growth_row {
        by[3] = by[1];
    }
    Some(Motion { by, rest: b, k: 1.0 })
}

/// How far a gliding box's top (device px) sits below its nearest whole pixel: its glyphs lay
/// out from that pixel, as at rest, and move down by this ([`crate::text::TextSystem::set_glide`]),
/// so they keep their place against the box instead of stepping a pixel at a time.
pub(super) fn text_glide(top: f32) -> f32 {
    let g = top - top.round();
    if g.abs() < 1.0 / 64.0 { 0.0 } else { g }
}

/// One moving node's glide: its edges' corrections go from `from` to `to` (the resting rect's
/// rounding) as their motion goes from `span` to 0.
#[derive(Clone, Copy)]
struct Glide {
    rest: [f32; 4],
    k: f32,
    /// Resting edges in device px, less the rigid translation.
    rest_px: [f32; 4],
    from: [f32; 4],
    to: [f32; 4],
    span: [f32; 4],
    seen: u32,
    /// The parent's glide left and resting edges last frame, which edges with no span follow.
    follow: [f32; 4],
    parent: [f32; 4],
}

impl Glide {
    /// Edge `k`'s correction with `m` px left to travel. Past the start (a retarget that carries
    /// it further away) it holds the starting correction.
    fn at(&self, k: usize, to: f32, m: f32, follow: f32) -> f32 {
        to + (self.from[k] - to) * self.left(k, m, follow)
    }

    /// How much of edge `k`'s glide is left, 1 to 0. An edge that doesn't move goes with its
    /// parent's (`follow`), so its centring eases as the parent's edges glide around it.
    fn left(&self, k: usize, m: f32, follow: f32) -> f32 {
        if self.span[k].abs() > 1.0 / 64.0 { (m / self.span[k]).clamp(-1.0, 1.0) } else { follow }
    }
}

/// What a node hands its subtree for snapping.
#[derive(Clone, Copy, Default)]
pub(super) struct Inherit {
    /// `El::pixel_snap(false)` above, or a turn: this subtree draws at its exact edges.
    pub off: bool,
    /// The translation in points from scrolls, sticky shifts and growth rows above, which
    /// snaps as a whole.
    pub rigid: [f32; 2],
    /// The nearest scaled layer above or here, as an index into this frame's layers plus one
    /// (0: none).
    pub layer: u32,
    /// The parent's edges, for centring. None when it paints nothing to centre against, its
    /// children don't share its rigid translation, or it doesn't snap on the plain grid.
    pub parent: Option<Parent>,
}

/// A box's edges (device px, less its rigid translation): `exact`, and `at` where it draws them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rest {
    exact: [f32; 4],
    at: [f32; 4],
}

/// A box's edges where it comes to rest, which its children centre against, where it is now,
/// which a child that starts to glide measures the correction it drew with against, and how
/// much of each edge's glide is left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Parent {
    rest: Rest,
    now: Rest,
    left: [f32; 4],
}

/// The glides of the nodes moving now and the scaled layers, owned by the runtime.
#[derive(Default)]
pub(super) struct Glides {
    glides: HashMap<NodeId, Glide>,
    layers: HashMap<NodeId, Layer>,
    /// This frame's layers, which `Inherit::layer` indexes from 1.
    now: Vec<LayerSnap>,
    frame: u32,
    t: f64,
    /// The window (device px).
    size: [f32; 2],
    /// A layer's correction still has a visible way to go.
    easing: bool,
}

/// Frames a glide outlives its node's last paint (a cached subtree, a removed node).
const KEEP: u32 = 120;

/// Seconds a still scaled layer's correction to the grid takes to ease in.
pub(super) const SETTLE: f64 = 0.1;

/// A node with its own scale, which heads a scaled layer.
#[derive(Clone, Copy)]
struct Layer {
    /// Its drawn rect (device px, less its rigid translation) last frame, and when it last
    /// moved.
    rect: [f32; 4],
    t: f64,
    seen: u32,
    moving: bool,
    /// When it came to rest.
    since: f64,
    /// Rests it has left, each its rect there and the weight its correction was drawn with
    /// when the layer left it: the two heaviest, fading out.
    outs: [([f32; 4], f32); 2],
}

/// A scaled layer's snapping this frame, handed down its subtree.
#[derive(Clone, Copy)]
struct LayerSnap {
    /// Weight of the snap where the layer is now: 0 while it moves, easing to 1 once it holds.
    to: f32,
    /// Weight of the snap to the unscaled grid: 1 while the scale moves the layer's edges by
    /// no more than 1/16 px, fading out over the pixel before, so a scale settling at 1 lands
    /// without a jump.
    near: f32,
    /// The corrections drawn at rests the layer left, fading out: each maps a device px
    /// coordinate now to where it was at that rest (`x * a + b` per axis), with its weight.
    outs: [([f32; 4], f32); 2],
    /// The layer's rigid translation (device px).
    rigid: [f32; 2],
}

const IDENTITY: [f32; 4] = [1.0, 0.0, 1.0, 0.0];

const NO_LAYER: LayerSnap = LayerSnap { to: 0.0, near: 0.0, outs: [(IDENTITY, 0.0); 2], rigid: [0.0; 2] };

/// The largest distance between matching edges.
fn far(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

/// The map `x * a + b` per axis that takes `from`'s edges to `to`'s.
fn affine(from: [f32; 4], to: [f32; 4]) -> [f32; 4] {
    let mut map = IDENTITY;
    for a in 0..2 {
        let (w, to_w) = (from[a + 2] - from[a], to[a + 2] - to[a]);
        let k = if w.abs() > 1e-3 { to_w / w } else { 1.0 };
        (map[a * 2], map[a * 2 + 1]) = (k, to[a] - from[a] * k);
    }
    map
}

/// How far `map` moves a point of a window `size` across at most (device px).
fn reach(map: [f32; 4], size: [f32; 2]) -> f32 {
    (0..2).map(|a| map[a * 2 + 1].abs().max(((map[a * 2] - 1.0) * size[a] + map[a * 2 + 1]).abs())).fold(0.0, f32::max)
}

impl Glides {
    /// Called once before each paint pass, at animation time `t`, in a window `size` across
    /// (device px).
    pub(super) fn begin_frame(&mut self, t: f64, size: [f32; 2]) {
        self.frame = self.frame.wrapping_add(1);
        (self.t, self.size) = (t, size);
        self.now.clear();
        self.easing = false;
        let frame = self.frame;
        if !self.glides.is_empty() {
            self.glides.retain(|_, g| frame.wrapping_sub(g.seen) < KEEP);
        }
        if !self.layers.is_empty() {
            self.layers.retain(|_, l| frame.wrapping_sub(l.seen) < KEEP);
        }
    }

    /// Whether a scaled layer's correction is still easing in or out: the last paint asks for
    /// another frame.
    pub(super) fn easing(&self) -> bool {
        self.easing
    }

    /// Node `id` has its own scale and heads a layer: `r` is its drawn rect and `unscaled` the
    /// same rect before the scales on it and above it (device px edges), and `rigid` its rigid
    /// translation (device px). `cached` content snaps without easing, as its raster isn't
    /// redrawn while the correction eases. The returned index goes in `Inherit::layer`.
    pub(super) fn layer(&mut self, id: NodeId, (r, unscaled): ([f32; 4], [f32; 4]), rigid: [f32; 2], cached: bool) -> u32 {
        let cur: [f32; 4] = core::array::from_fn(|k| r[k] - rigid[k % 2]);
        let (t, frame, size) = (self.t, self.frame, self.size);
        let l = self.layers.entry(id).or_insert(Layer { rect: cur, t, seen: frame, moving: false, since: f64::NEG_INFINITY, outs: [(cur, 0.0); 2] });
        let eased = |since: f64| {
            let x = if cached { 1.0 } else { ((t - since) / SETTLE).clamp(0.0, 1.0) as f32 };
            x * x * (3.0 - 2.0 * x)
        };
        let to;
        if reach(affine(l.rect, cur), size) > 1.0 / 256.0 {
            if !l.moving {
                // It drew `f` of this rest's correction, and `1 - f` of what the rests before it
                // still drew.
                let f = eased(l.since);
                let [a, b] = l.outs.map(|(rest, w)| (rest, w * (1.0 - f)));
                let mut outs = [(l.rect, f), a, b];
                outs.sort_by(|x, y| y.1.total_cmp(&x.1));
                l.outs = [outs[0], outs[1]];
                l.moving = true;
            }
            (l.rect, l.t) = (cur, t);
            to = 0.0;
        } else {
            if l.moving {
                // Still since its last paint, which already drew it here.
                l.moving = false;
                l.since = l.t;
            }
            to = eased(l.since);
            if to == 1.0 {
                l.outs = [(cur, 0.0); 2];
            }
        }
        l.seen = frame;
        let outs = l.outs.map(|(rest, w)| if w > 0.0 { (affine(cur, rest), w * (1.0 - to)) } else { (IDENTITY, 0.0) });
        let near = ((1.0 - far(r, unscaled)) / (1.0 - 1.0 / 16.0)).clamp(0.0, 1.0);
        self.now.push(LayerSnap { to, near, outs, rigid });
        self.now.len() as u32
    }

    /// Node `id` is at rest and unscaled: its paint rect `r` (device px edges) snapped, with
    /// `rigid` from `Inherit::rigid` in device px and `parent` from `Inherit::parent`, and its
    /// resting edges for its children.
    #[inline]
    pub(super) fn at_rest(&mut self, id: NodeId, r: [f32; 4], rigid: [f32; 2], parent: Option<Parent>) -> ([f32; 4], Option<Parent>) {
        // Centred in a parent that still glides, it goes with the parent's glide.
        let gliding = |p: &Parent| p.left != [0.0; 4] && (0..2).any(|a| resting(p.rest, a, (r[a] - rigid[a], r[a + 2] - rigid[a])).is_some());
        if let Some(p) = parent.filter(gliding) {
            let (out, parent) = self.glide(id, Motion { by: [0.0; 4], rest: r, k: 1.0 }, r, rigid, Some(p));
            return (out, Some(parent));
        }
        if !self.glides.is_empty() {
            self.glides.remove(&id);
        }
        let (out, rest) = snap(r, [0.0; 4], rigid, parent.map(|p| p.rest), |_, c| c);
        (out, Some(Parent { rest, now: rest, left: [0.0; 4] }))
    }

    /// Node `id`'s paint rect `r` (device px edges) snapped. `unscaled` is the same rect before
    /// the scales on it and above it, `motion` from [`motion`], and `rigid` and `parent` as in
    /// [`Self::at_rest`]. `layer` is `Inherit::layer` and `sc` the node's scale. Its resting
    /// edges come back only where it snaps on the plain grid.
    pub(super) fn snapped(&mut self, id: NodeId, motion: Option<Motion>, (r, unscaled): ([f32; 4], [f32; 4]), rigid: [f32; 2], (layer, sc): (u32, f32), parent: Option<Parent>) -> ([f32; 4], Option<Parent>) {
        // Snapping to the unscaled grid fades in over the last pixel scaling moves the edges, so
        // a scale settling at 1 lands on the grid without a jump. In a layer that is how far
        // its scale moves the layer's edges, the same for every box in it.
        let l = layer.checked_sub(1).map_or(NO_LAYER, |i| self.now[i as usize]);
        let weight = if layer == 0 { ((1.0 - far(r, unscaled)) / (1.0 - 1.0 / 16.0)).clamp(0.0, 1.0) } else { l.near };
        let mut out = r;
        let plain = layer == 0 && weight >= 1.0;
        let parent = parent.filter(|_| plain);
        let mut rest = None;
        if weight > 0.0 && l.to < 1.0 {
            let (snapped, at) = match motion {
                None => self.at_rest(id, unscaled, rigid, parent),
                Some(m) if l.to == 0.0 => {
                    let (out, at) = self.glide(id, m, unscaled, rigid, parent);
                    (out, Some(at))
                }
                Some(m) => (snap(unscaled, m.by, rigid, None, |_, c| c).0, None),
            };
            rest = at.filter(|_| plain);
            for k in 0..4 {
                out[k] += (snapped[k] - r[k]) * weight * (1.0 - l.to);
            }
        }
        if layer == 0 {
            return (out, rest);
        }
        // Translations inside the layer are scaled with it.
        let lr = l.rigid;
        let rigid: [f32; 2] = core::array::from_fn(|a| lr[a] + (rigid[a] - lr[a]) * sc);
        let mut left = 0.0f32;
        for (map, w) in l.outs {
            if w <= 0.0 || weight >= 1.0 {
                continue;
            }
            let m: [f32; 4] = core::array::from_fn(|k| {
                let a = k % 2;
                (r[k] - lr[a]) * map[a * 2] + map[a * 2 + 1] + lr[a]
            });
            // Each box lets go of it over the first pixel it moves from that rest.
            let w = w * (1.0 - weight) * (1.0 - far(r, m)).clamp(0.0, 1.0);
            let (at, _) = snap(m, [0.0; 4], rigid, None, |_, c| c);
            for k in 0..4 {
                let c = (at[k] - m[k]) * w;
                out[k] += c;
                left = left.max(c.abs());
            }
        }
        let at = match motion {
            Some(m) if l.to > 0.0 => self.glide(id, Motion { by: m.by.map(|v| v * sc), k: sc, ..m }, r, rigid, None).0,
            _ if l.to > 0.0 => self.at_rest(id, r, rigid, None).0,
            // Moving: where holding still would take it, to know if it has anywhere to go.
            _ => snap(r, [0.0; 4], rigid, None, |_, c| c).0,
        };
        if l.to == 1.0 {
            return (at, None);
        }
        for k in 0..4 {
            out[k] += (at[k] - r[k]) * l.to;
            left = left.max((at[k] - r[k]).abs() * (1.0 - l.to));
        }
        self.easing |= left > 1.0 / 128.0;
        (out, None)
    }

    fn glide(&mut self, id: NodeId, m: Motion, r: [f32; 4], rigid: [f32; 2], parent: Option<Parent>) -> ([f32; 4], Parent) {
        let free: [f32; 4] = core::array::from_fn(|k| r[k] - rigid[k % 2]);
        let (follow, parent_rest) = parent.map_or(([0.0; 4], [0.0; 4]), |p| (p.left, p.rest.exact));
        let mut g = match self.glides.get(&id) {
            Some(g) if g.rest == m.rest && g.k == m.k => {
                let mut g = *g;
                if g.parent != parent_rest {
                    // The parent retargets and starts its glide over: an edge that follows it
                    // starts over from what it drew.
                    for k in (0..4).filter(|&k| g.span[k].abs() <= 1.0 / 64.0) {
                        g.from[k] = g.at(k, g.to[k], 0.0, g.follow[k]);
                    }
                }
                g
            }
            prev => {
                // A new rest: start from the correction drawn with now, so nothing jumps.
                let from = match prev {
                    Some(g) => core::array::from_fn(|k| g.at(k, g.to[k], free[k] - g.rest_px[k], g.follow[k])),
                    None => start(r, rigid, parent.map(|p| p.now)),
                };
                let rest_px = core::array::from_fn(|k| free[k] - m.by[k]);
                Glide { rest: m.rest, k: m.k, rest_px, from, to: from, span: m.by, seen: 0, follow, parent: parent_rest }
            }
        };
        let (out, rest) = snap(r, m.by, rigid, parent.map(|p| p.rest), |k, c| {
            g.to[k] = c;
            g.at(k, c, m.by[k], follow[k])
        });
        (g.seen, g.follow, g.parent) = (self.frame, follow, parent_rest);
        self.glides.insert(id, g);
        let now = Rest { exact: free, at: core::array::from_fn(|k| out[k] - round(rigid[k % 2])) };
        (out, Parent { rest, now, left: core::array::from_fn(|k| g.left(k, m.by[k], follow[k])) })
    }
}

/// Rounds half up on the device grid, the same way at every position.
fn round(v: f32) -> f32 {
    (v + 0.5).floor()
}

/// `r` (device px edges, `x0, y0, x1, y1`) snapped, and its resting edges. `motion` is how far
/// each edge is from where it comes to rest and `rigid` the translation that snaps as a whole
/// (both device px). On an axis where its rest is centred in `parent`'s, its gaps to the
/// parent's resting rounding match. Each edge's correction toward its resting rounding passes
/// through `ease(edge, correction)`.
#[inline]
pub(super) fn snap(r: [f32; 4], motion: [f32; 4], rigid: [f32; 2], parent: Option<Rest>, mut ease: impl FnMut(usize, f32) -> f32) -> ([f32; 4], Rest) {
    let mut out = r;
    let mut rest = Rest { exact: [0.0; 4], at: [0.0; 4] };
    // A scaled layer's edges and its scaled rigid translation each land a little off the 1/64 px
    // grid, so their difference wobbles by up to 1/128 px frame to frame even while the content
    // holds still against the scroll. Back on the grid it holds, and an edge resting on a half
    // pixel rounds the same way every frame instead of flickering a pixel.
    let grid = |v: f32| (v * 64.0).round() / 64.0;
    for axis in 0..2 {
        let (a, b) = (axis, axis + 2);
        let (rest_a, rest_b) = (grid(r[a] - motion[a] - rigid[axis]), grid(r[b] - motion[b] - rigid[axis]));
        (rest.exact[a], rest.exact[b]) = (rest_a, rest_b);
        (rest.at[a], rest.at[b]) = (rest_a, rest_b);
        if rest_b - rest_a < 0.5 {
            continue;
        }
        let shift = round(rigid[axis]);
        let (lo, hi) = parent.and_then(|p| resting(p, axis, (rest_a, rest_b))).unwrap_or_else(|| {
            let lo = round(rest_a);
            (lo, round(rest_b).max(lo + 1.0))
        });
        (rest.at[a], rest.at[b]) = (lo, hi);
        for (k, rest, at) in [(a, rest_a, lo), (b, rest_b, hi)] {
            // Built from the rounded edge, so a box at rest lands exactly on whole pixels.
            let (c, eased) = (at - rest, ease(k, at - rest));
            out[k] = if eased == c { at } else { rest + eased } + shift + motion[k];
        }
    }
    (out, rest)
}

/// A border's or outline's width, or an outline's offset, `w` device px, snapped as Chromium
/// snaps CSS border and outline widths: under a pixel it draws one whole pixel, above that it
/// floors to whole pixels. So 0.5 pt is one crisp pixel at 1×, 1.25× and 1.5×, and 1 pt is one
/// pixel at 1.25× and 1.5×. Zero stays zero; the sign stays.
pub(super) fn line_width(w: f32) -> f32 {
    if w == 0.0 || !w.is_finite() {
        return w;
    }
    // Scene values sit on a 1/64 px grid, so 2.99 px is 3.
    (w.abs() + 1.0 / 64.0).floor().max(1.0).copysign(w)
}

/// Whether a box paints something its children's gaps show against. Elsewhere centring would
/// cost a pixel of size for nothing.
pub(super) fn frames(v: &crate::el::Visual) -> bool {
    let border = v.border_width() > 0.0 && (v.border().a > 0.0 || v.border_bottom.is_some());
    v.fill.a > 0.0 || v.hover().a > 0.0 || v.press().a > 0.0 || v.gradient.is_some() || v.radial.is_some() || border
}

/// The correction a box at rest last frame drew with, now that it starts to glide from `r`:
/// its rounding here, eased toward its centred one over the first pixel it is off centre.
fn start(r: [f32; 4], rigid: [f32; 2], parent: Option<Rest>) -> [f32; 4] {
    let mut now = [0.0; 4];
    snap(r, [0.0; 4], rigid, None, |k, c| {
        now[k] = c;
        c
    });
    let Some(p) = parent else { return now };
    for axis in 0..2 {
        let (a, b) = (axis, axis + 2);
        let (ra, rb) = (r[a] - rigid[axis], r[b] - rigid[axis]);
        if rb - ra < 0.5 {
            continue;
        }
        if let Some((lo, hi)) = centred(p, (a, b), (ra, rb), 1.0) {
            let w = 1.0 - ((ra - p.exact[a]) - (p.exact[b] - rb)).abs();
            now[a] += (lo - ra - now[a]) * w;
            now[b] += (hi - rb - now[b]) * w;
        }
    }
    now
}

/// Resting edges `lo..hi` on `axis` with equal whole-pixel gaps to `parent`'s resting
/// rounding, if the rest `ra..rb` is centred in the parent's and the parent snaps on that axis.
fn resting(parent: Rest, axis: usize, (ra, rb): (f32, f32)) -> Option<(f32, f32)> {
    let (a, b) = (axis, axis + 2);
    if parent.at[a].fract() != 0.0 || parent.at[b].fract() != 0.0 {
        return None;
    }
    // Scene rects sit on a 1/64 px grid, so a centred box can be off by twice that.
    centred(parent, (a, b), (ra, rb), 1.0 / 16.0)
}

/// Edges `lo..hi` on the axis of edges `a` and `b` with equal whole-pixel gaps to `parent`'s
/// drawn edges, if `ra..rb` is centred in its exact ones to within `slack` px.
fn centred(parent: Rest, (a, b): (usize, usize), (ra, rb): (f32, f32), slack: f32) -> Option<(f32, f32)> {
    let ([pa, pb], [qa, qb]) = ([parent.exact[a], parent.exact[b]], [parent.at[a], parent.at[b]]);
    if ((ra - pa) - (pb - rb)).abs() >= slack {
        return None;
    }
    let gap = round((qb - qa - (rb - ra)) / 2.0);
    (qb - qa - 2.0 * gap >= 1.0).then_some((qa + gap, qb - gap))
}
