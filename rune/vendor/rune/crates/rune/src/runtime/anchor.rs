//! Where `El::anchored` overlays go, how long an `El::anchored_fit` one may be, and the zoom
//! one inherits from its anchor.
use super::*;
use crate::el::{Anchor, Flow, Pin, Side};

/// The node an `El::above` anchor `a` names, among the `before` nodes ahead of its element.
pub(super) fn anchor_index(nodes: &[Node], before: usize, a: &Anchor) -> Option<usize> {
    nodes[..before].iter().position(|n| n.key.as_ref() == Some(&a.key))
}

/// Where a walk from node `i` goes next: the node it is anchored above, else its parent.
pub(super) fn step_up(nodes: &[Node], i: usize) -> Option<usize> {
    nodes[i].above.as_ref().and_then(|a| anchor_index(nodes, i, a)).or(nodes[i].parent)
}

/// How far an anchored overlay stays inside the window.
const MARGIN: f32 = 8.0;

/// Where an anchored overlay of `size` goes next to `t` (x0, y0, x1, y1), flipped to the
/// opposite side when it doesn't fit, then kept 8 points inside the viewport.
pub(crate) fn anchor_position(a: &Anchor, t: [f32; 4], size: (f32, f32), vp: (f32, f32)) -> (f32, f32) {
    let (w, h) = size;
    let margin = MARGIN;
    let along = |lo: f32, hi: f32, len: f32| match a.align {
        Align::Start => lo,
        Align::Center => (lo + hi - len) * 0.5,
        Align::End => hi - len,
    };
    let (mut x, mut y) = match a.side {
        Side::Top => (along(t[0], t[2], w), t[1] - h - a.gap),
        Side::Bottom => (along(t[0], t[2], w), t[3] + a.gap),
        Side::Left => (t[0] - w - a.gap, along(t[1], t[3], h)),
        Side::Right => (t[2] + a.gap, along(t[1], t[3], h)),
    };
    // Flip the side when the other one fits (Floating UI's `flip`).
    match a.side {
        Side::Bottom if y + h > vp.1 - margin && t[1] - h - a.gap >= margin => y = t[1] - h - a.gap,
        Side::Top if y < margin && t[3] + a.gap + h <= vp.1 - margin => y = t[3] + a.gap,
        Side::Right if x + w > vp.0 - margin && t[0] - w - a.gap >= margin => x = t[0] - w - a.gap,
        Side::Left if x < margin && t[2] + a.gap + w <= vp.0 - margin => x = t[2] + a.gap,
        _ => {}
    }
    // Flip the alignment when it overflows and the other end fits, then shift along the side.
    let vertical = matches!(a.side, Side::Top | Side::Bottom);
    let (pos, lo, hi, len, room) = if vertical { (&mut x, t[0], t[2], w, vp.0) } else { (&mut y, t[1], t[3], h, vp.1) };
    match a.align {
        Align::End if *pos < margin && lo + len <= room - margin => *pos = lo,
        Align::Start if *pos + len > room - margin && hi - len >= margin => *pos = hi - len,
        _ => {}
    }
    *pos = pos.clamp(margin, (room - len - margin).max(margin));
    // Along the main axis only keep the start edge inside; a long list may run off the far end
    // (`El::anchored_fit` bounds it instead).
    if vertical { y = y.max(margin) } else { x = x.max(margin) }
    if a.round {
        // `Math.round`: halves go up.
        return ((x + 0.5).floor(), (y + 0.5).floor());
    }
    (x, y)
}

/// The longest an `anchored_fit` overlay `len` long may be next to `t`, or `None` when it fits
/// on its side or the other one as it is. The bound is the larger room (its own side's on a
/// tie), a little short of it so [`anchor_position`] puts the bounded overlay on that side.
fn fit_bound(a: &Anchor, t: [f32; 4], len: f32, vp: (f32, f32)) -> Option<f32> {
    let (before, after) = match a.side {
        Side::Top | Side::Bottom => (t[1] - a.gap - MARGIN, vp.1 - MARGIN - (t[3] + a.gap)),
        Side::Left | Side::Right => (t[0] - a.gap - MARGIN, vp.0 - MARGIN - (t[2] + a.gap)),
    };
    let (own, other) = if matches!(a.side, Side::Top | Side::Left) { (before, after) } else { (after, before) };
    if len <= own || len <= other {
        return None;
    }
    Some((own.max(other) - 0.5).floor().max(0.0))
}

impl<S: 'static> Measurement<'_, S> {
    /// Places every node from `taffy`'s layout: ids, local and window rects, insets, and
    /// anchored overlays beside their anchors.
    pub(super) fn place(&mut self, taffy: &LayoutEngine, nodes: &mut [Node]) {
        self.node_index.clear();
        self.node_index.reserve(nodes.len());
        for i in 0..nodes.len() {
            let id = if nodes[i].id != NodeId(0) {
                debug_assert_eq!(self.path_ids.get(&nodes[i].path).copied(), Some(nodes[i].id));
                nodes[i].id
            } else {
                self.node_id(&nodes[i].path)
            };
            nodes[i].id = id;
            self.node_index.insert(id, i);
            let mut l = taffy.box_layout(nodes[i].taffy);
            // Right to left: laid out left to right, then mirrored inside the parent (`El::dir`).
            if let Some(p) = nodes[i].parent.filter(|&p| nodes[p].flow == Flow::Rtl && nodes[i].pin == Pin::Flowing) {
                l.x = nodes[p].local.w - l.x - l.w;
            }
            if nodes[i].flow == Flow::Rtl {
                l.inset.swap(0, 2);
            }
            nodes[i].local = MRect::new(l.x, l.y, l.w, l.h);
            let (px, py) = nodes[i].parent.map_or((0.0, 0.0), |p| (nodes[p].target[0], nodes[p].target[1]));
            let mut x = px + l.x;
            let mut y = py + l.y;
            if let Some(a) = &nodes[i].above
                && let Some(j) = anchor_index(&nodes, i, a)
            {
                // Anchors inside scroll viewports sit where they are scrolled (and stuck) to.
                let target = sticky::anchored_target(nodes, self.scrolls, j, self.viewport);
                (x, y) = anchor_position(a, target, (l.w, l.h), self.viewport);
                nodes[i].local.x = x - px;
                nodes[i].local.y = y - py;
            }
            nodes[i].target = [x, y, x + l.w, y + l.h];
            nodes[i].inset = l.inset;
        }
    }

    /// Bounds each placed `anchored_fit` overlay that fits on neither side of its anchor to the
    /// larger room, through its style. True when any changed, so layout runs again.
    pub(super) fn fit_anchored(&self, taffy: &mut LayoutEngine, nodes: &[Node]) -> bool {
        let mut changed = false;
        for (i, n) in nodes.iter().enumerate() {
            let Some(a) = n.above.as_ref().filter(|a| a.fit) else { continue };
            let Some(j) = anchor_index(&nodes, i, a) else { continue };
            let target = sticky::anchored_target(nodes, self.scrolls, j, self.viewport);
            // It draws `k` times its layout size beside where the anchor draws.
            let k = inherited_zoom(nodes, i, &|m: &Node| m.static_scale).max(MIN_ZOOM);
            let target = if k == 1.0 { target } else { zoomed_rect(nodes, self.scrolls, j, target, self.viewport) };
            let zoomed;
            let a = if k == 1.0 { a } else { zoomed = zoomed_gap(a, k); &zoomed };
            let vertical = matches!(a.side, Side::Top | Side::Bottom);
            let len = if vertical { n.local.h } else { n.local.w };
            let Some(bound) = fit_bound(a, target, len * k, self.viewport) else { continue };
            let mut style = taffy.style(n.taffy).clone();
            if vertical {
                style.set_max_height(length(bound / k));
            } else {
                style.set_max_width(length(bound / k));
            }
            taffy.set_style_and_mark_ancestors(
                n.taffy,
                style,
                std::iter::successors(n.parent, |&p| nodes[p].parent).map(|p| nodes[p].taffy),
            );
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
#[path = "anchor_zoom_tests.rs"]
mod zoom_tests;
#[cfg(test)]
#[path = "anchor_zoom_motion_tests.rs"]
mod zoom_motion_tests;

/// The smallest zoom an anchored overlay is placed at, so placing it never divides by zero.
const MIN_ZOOM: f32 = 1e-3;

/// The zoom anchored overlay `i` draws at: its anchor's, the product of the `El::scale`s on the
/// anchor's ancestors (through any anchored overlay the anchor is in). It is 1 when the overlay
/// opts out with `El::anchored_unscaled`, or scales itself with `El::scale`, as an app that
/// already zooms its popovers by hand does. `own` reads a node's own `El::scale`.
pub(super) fn inherited_zoom(nodes: &[Node], i: usize, own: &impl Fn(&Node) -> f32) -> f32 {
    let n = &nodes[i];
    let Some(a) = n.above.as_ref().filter(|a| !a.unscaled) else { return 1.0 };
    if n.scale_track || n.static_scale != 1.0 {
        return 1.0;
    }
    let Some(j) = anchor_index(&nodes, i, a) else { return 1.0 };
    let mut k = 1.0;
    let mut at = nodes[j].parent;
    while let Some(p) = at {
        k *= own(&nodes[p]);
        if nodes[p].above.is_some() {
            return k * inherited_zoom(nodes, p, own);
        }
        at = nodes[p].parent;
    }
    k
}

/// `t`, node `j`'s layout rect, where its ancestors' `El::scale`s draw it at rest. An anchored
/// ancestor zooms about its own top-left corner.
fn zoomed_rect(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, j: usize, t: [f32; 4], vp: (f32, f32)) -> [f32; 4] {
    let chain: Vec<usize> = std::iter::successors(nodes[j].parent, |&p| nodes[p].parent).collect();
    // A point `x` draws at `to + (x - from) * k`, as `Inh` composes scales down the tree.
    let (mut k, mut from, mut to) = (1.0f32, [0.0f32; 2], [0.0f32; 2]);
    for &p in chain.iter().rev() {
        let n = &nodes[p];
        if n.above.is_some() {
            // An overlay drops its ancestors' scales.
            (k, from, to) = (1.0, [0.0; 2], [0.0; 2]);
        }
        let own = n.static_scale * inherited_zoom(nodes, p, &|m: &Node| m.static_scale);
        if (own - 1.0).abs() <= 1e-5 {
            continue;
        }
        let r = sticky::anchored_target(nodes, scrolls, p, vp);
        let [ox, oy] = match (n.above.is_some(), n.vis.origin) {
            (true, _) => [0.0, 0.0],
            (false, Some(o)) => o,
            (false, None) => [0.5, 0.5],
        };
        let pivot = [r[0] + ox * (r[2] - r[0]), r[1] + oy * (r[3] - r[1])];
        to = [to[0] + (pivot[0] - from[0]) * k, to[1] + (pivot[1] - from[1]) * k];
        from = pivot;
        k *= own;
    }
    [to[0] + (t[0] - from[0]) * k, to[1] + (t[1] - from[1]) * k, to[0] + (t[2] - from[0]) * k, to[1] + (t[3] - from[1]) * k]
}

/// `a` with its gap at zoom `k`; the window margin stays in window points.
fn zoomed_gap(a: &Anchor, k: f32) -> Anchor {
    Anchor { gap: a.gap * k, ..a.clone() }
}

/// The point on drawn box `d` that touches its anchor `t`: the middle of the edge facing the
/// anchor, moved along it to the end `a.align` names. The overlay zooms about it, so it grows
/// away from its anchor.
fn attach_point(a: &Anchor, t: [f32; 4], d: [f32; 4]) -> [f32; 2] {
    let along = |lo: f32, hi: f32| match a.align {
        Align::Start => lo,
        Align::Center => (lo + hi) * 0.5,
        Align::End => hi,
    };
    let facing = |lo: f32, hi: f32, t_lo: f32, t_hi: f32| if lo + hi > t_lo + t_hi { lo } else { hi };
    match a.side {
        Side::Top | Side::Bottom => [along(d[0], d[2]), facing(d[1], d[3], t[1], t[3])],
        Side::Left | Side::Right => [facing(d[0], d[2], t[0], t[2]), along(d[1], d[3])],
    }
}

impl<S: 'static> Runtime<S> {
    /// Sets up anchored overlay `i`'s inherited paint state beside anchor `j`: at the anchor's
    /// zoom (see [`inherited_zoom`]), placed and flipped by its zoomed size where the anchor was
    /// drawn, and zooming about the point it attaches at. Its own `prop::OFFSET` and transition
    /// still apply on top; its ancestors' positions and scales don't.
    pub(super) fn anchored_inh(&self, i: usize, j: usize, a: &Anchor, p: &rune_motion::Presented, pinh: &mut Inh) {
        let tracked = |m: &Node| if m.scale_track { self.scene.get(m.id, P_SCALE).unwrap_or(m.static_scale) } else { m.static_scale };
        let k = inherited_zoom(&self.nodes, i, &tracked).max(MIN_ZOOM);
        let d = self.drawn[j];
        let target = if d != [0.0; 4] { [d[0], d[1], d[0] + d[2], d[1] + d[3]] } else {
            let t = sticky::anchored_target(&self.nodes, &self.scrolls, j, self.viewport);
            if k == 1.0 { t } else { zoomed_rect(&self.nodes, &self.scrolls, j, t, self.viewport) }
        };
        let zoomed;
        let a = if k == 1.0 { a } else { zoomed = zoomed_gap(a, k); &zoomed };
        let (w, h) = (p.rect.w * k, p.rect.h * k);
        let (ax, ay) = anchor_position(a, target, (w, h), self.viewport);
        let pivot = attach_point(a, target, [ax, ay, ax + w, ay + h]);
        // Placed from the presented size and anchor, so drop the scene's own layout position
        // (this node's `prop::FRAME` x/y and its ancestors' origins) while keeping this node's
        // own `prop::OFFSET` and transition offset: with the frame position spring left in, an
        // End-aligned box growing in width would start from its old x and its end edge would
        // swing out and back. Zoomed about `pivot`, the box's top-left lands on (ax, ay).
        let world = self.scene.world(self.nodes[i].id).unwrap_or(p.rect);
        (pinh.snap.rigid, pinh.snap.layer) = ([0.0; 2], 0);
        if k == 1.0 {
            (pinh.dx, pinh.dy) = (ax - world.x, ay - world.y);
            (pinh.scale, pinh.anchor, pinh.anchor_to) = (1.0, [0.0; 2], [0.0; 2]);
            pinh.still = (1.0, [0.0; 2], [0.0; 2]);
            return;
        }
        pinh.dx = pivot[0] + (ax - pivot[0]) / k - world.x;
        pinh.dy = pivot[1] + (ay - pivot[1]) / k - world.y;
        (pinh.scale, pinh.anchor, pinh.anchor_to) = (k, pivot, pivot);
        // Text rasterizes as the anchor's does: crisp at the zoom once it rests.
        let motion = self.nodes[j].parent.and_then(|jp| self.inh_scratch.get(jp)).map_or(ScaleMotion::Still, |h| h.scale_motion);
        pinh.scale_motion = pinh.scale_motion.max(motion);
        pinh.still = if pinh.scale_motion == ScaleMotion::Still { (k, pivot, pivot) } else { (1.0, [0.0; 2], [0.0; 2]) };
    }
}
