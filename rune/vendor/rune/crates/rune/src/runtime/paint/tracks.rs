//! A node's paint: colours, shadow geometry, hover and press. Most of it never changes, so a
//! property gets a track only when it does, starting from what was last drawn.

use super::*;

/// Finds new nodes in the last build. Both trees are mostly in the same order, so it walks
/// them together and builds a map only on the first node out of step.
#[derive(Default)]
pub(super) struct LastBuild {
    next: usize,
    map: Option<HashMap<NodeId, usize>>,
}

impl LastBuild {
    pub(super) fn find(&mut self, old: &[Node], id: NodeId) -> Option<usize> {
        let at = match old.get(self.next) {
            Some(n) if n.id == id => Some(self.next),
            _ => self.map.get_or_insert_with(|| old.iter().enumerate().map(|(i, n)| (n.id, i)).collect()).get(&id).copied(),
        }?;
        self.next = at + 1;
        Some(at)
    }

    pub(super) fn advance(&mut self, next: usize) {
        self.next = next;
    }
}

/// Resolves `old` against `old_nodes`, falling back to `nodes[i]` when `old` was moved out of
/// `old_nodes` by a `same_path` memo reuse (leaving its paint and text fields unchanged on `nodes[i]`).
#[inline]
pub(super) fn prev_node<'a>(old_nodes: &'a [Node], nodes: &'a [Node], old: Option<usize>, i: usize) -> Option<&'a Node> {
    old.map(|o| if old_nodes[o].is_moved_stub() { &nodes[i] } else { &old_nodes[o] })
}

/// The colour a node's text or icon draws in.
pub(super) fn text_color(n: &Node) -> Option<Color> {
    n.text.as_ref().map(|t| t.color).or(n.vis.icon.map(|(_, c)| c))
}

/// Which of a node's colours a live value owns. A commit leaves those tracks to the latch.
#[derive(Clone, Copy, Default)]
pub(super) struct LiveColors {
    pub(super) fill: bool,
    pub(super) text: bool,
    pub(super) border: bool,
    pub(super) shadow: bool,
    pub(super) gradient: [bool; 2],
}

pub(super) fn live_colors(n: &Node) -> LiveColors {
    let mut c = LiveColors::default();
    for b in n.live.iter() { c.add(b.property); }
    c
}

impl LiveColors {
    pub(super) fn add(&mut self, p: crate::live::Property) {
        use crate::live::Property;
        match p {
            Property::Fill => self.fill = true,
            Property::TextColor => self.text = true,
            Property::Border => self.border = true,
            Property::Shadow => self.shadow = true,
            Property::GradientTop => self.gradient[0] = true,
            Property::GradientBottom => self.gradient[1] = true,
            _ => {}
        }
    }

    /// Puts back on `new` the colours `old` last latched for the bindings it kept, since the
    /// view authored its own placeholder colour.
    pub(super) fn carry(self, old: &Node, new: &mut Node) {
        if self.fill { new.vis.fill = old.vis.fill; }
        if self.border { new.vis.set_border_color(old.vis.border()); }
        if self.text && let Some(c) = text_color(old) {
            if let Some(spec) = &mut new.text { if spec.color != c { spec.color = c; } }
            else if new.vis.icon.is_some_and(|(_, ic)| ic != c) && let Some((_, ic)) = &mut new.vis.cold_mut().icon { *ic = c; }
        }
        if self.shadow && let Some(c) = old.vis.shadows.first().map(|s| s.color)
            && new.vis.shadows.first().is_some_and(|s| s.color != c)
        {
            new.vis.cold_mut().shadows[0].color = c;
        }
        for k in 0..2 {
            if self.gradient[k] && let Some(c) = old.vis.gradient.as_deref().map(|g| g[k])
                && new.vis.gradient.as_deref().is_some_and(|g| g[k] != c)
                && let Some(g) = new.vis.cold_mut().gradient.as_deref_mut()
            {
                g[k] = c;
            }
        }
    }
}

/// Readies `p` (hover, press, press scale), which reads `rest` without a track, to move to
/// `to`: false when it has no track and `to` is `rest`, so there's nothing to do; otherwise it
/// has a track, made at `rest` if it had none.
pub(super) fn ready_from_rest(scene: &mut Scene, id: NodeId, p: Prop<f32>, rest: f32, to: f32) -> bool {
    if scene.store().has(id, p) {
        return true;
    }
    if to == rest {
        return false;
    }
    scene.set(id, p, rest);
    true
}

/// A shadow's geometry as its track holds it.
pub(super) fn shadow_geo(sh: &crate::el::Shadow) -> rune_motion::Rect {
    rune_motion::Rect { x: sh.x, y: sh.y, w: sh.blur, h: sh.spread }
}

impl<S: 'static> Runtime<S> {
    /// Commits `to` on paint property `p` of node `id`, which last drew `from` (None if it's
    /// new). Without a track a node draws its own value (`Runtime::color`, `shadow_at`), so it
    /// needs none until the value changes, and a commit that leaves it changes nothing.
    #[inline]
    pub(super) fn commit_paint<T: rune_motion::Animatable + PartialEq>(&mut self, id: NodeId, p: Prop<T>, from: Option<T>, to: T, s: Spring, instant: bool) {
        if !instant && from == Some(to) {
            // Its track, if any, already heads to `to`: nothing else moves this property.
            return;
        }
        let tracked = self.scene.store().has(id, p);
        if from == Some(to) && !tracked {
            return;
        }
        if !tracked {
            match from {
                Some(from) if !instant => self.scene.set(id, p, from),
                _ => return,
            }
        }
        self.set_or_anim(id, p, to, s, instant);
    }

    pub(super) fn commit_cold_paint(
        &mut self,
        id: NodeId,
        vis: &Visual,
        old: Option<usize>,
        live: LiveColors,
        spring_c: Spring,
        spring_l: Spring,
        instant: bool,
    ) {
        fn prev_vis<'a>(old_nodes: &'a [Node], vis: &'a Visual, old: Option<usize>) -> Option<&'a Visual> {
            let o = old?;
            Some(if old_nodes[o].is_moved_stub() { vis } else { &old_nodes[o].vis })
        }
        for (k, sh) in vis.shadows.iter().take(P_SHADOWS.len()).enumerate() {
            let was = prev_vis(&self.nodes, vis, old).and_then(|o| o.shadows.get(k).copied());
            // An inset ring where a drop shadow was (or the other way) is another shadow, not this
            // one changing: it lands rather than morphing between the two.
            let instant = instant || was.is_some_and(|w| w.inset != sh.inset);
            if !(k == 0 && live.shadow) {
                self.commit_paint(id, P_SHADOWS[k], was.map(|w| w.color), sh.color, spring_c, instant);
            }
            self.commit_paint(id, P_SHADOW_GEO[k], was.map(|w| shadow_geo(&w)), shadow_geo(sh), spring_l, instant);
        }
        let prev_shadows = prev_vis(&self.nodes, vis, old).map_or(0, |o| o.shadows.len()).min(P_SHADOWS.len());
        for k in vis.shadows.len()..prev_shadows {
            self.scene.store_mut().remove(id, P_SHADOWS[k]);
            self.scene.store_mut().remove(id, P_SHADOW_GEO[k]);
        }
        if let Some(&g) = vis.gradient.as_deref() {
            let was = prev_vis(&self.nodes, vis, old).and_then(|o| o.gradient.as_deref().copied());
            for k in 0..2 {
                if !live.gradient[k] {
                    self.commit_paint(id, P_GRADIENT[k], was.map(|w| w[k]), g[k], spring_c, instant);
                }
            }
        }
        if let Some(rg) = &vis.radial {
            for (k, &(_, c)) in rg.stops.iter().take(P_RADIAL.len()).enumerate() {
                let was = prev_vis(&self.nodes, vis, old).and_then(|o| o.radial.as_ref()?.stops.get(k).map(|s| s.1));
                self.commit_paint(id, P_RADIAL[k], was, c, spring_c, instant);
            }
        }
        if let Some(ol) = vis.outline {
            let was = prev_vis(&self.nodes, vis, old).and_then(|o| o.outline.map(|w| w.color));
            self.commit_paint(id, P_OUTLINE, was, ol.color, spring_c, instant);
        }
    }

    /// True when `nodes[i]` has the same parent, frame, visual, text colour, transition and
    /// resting scene state as `self.nodes[old]`, so `apply`'s per-node diff has nothing to move.
    pub(super) fn unchanged_node_resting(
        &self,
        nodes: &[Node],
        i: usize,
        old: Option<usize>,
        scene_rect: MRect,
        parent_virtual: Option<&crate::el::virtual_list::VirtualList>,
    ) -> bool {
        let Some(o_idx) = old else { return false };
        let o = &self.nodes[o_idx];
        if o.is_moved_stub() {
            return false;
        }
        let n = &nodes[i];
        let plain_cold = |nd: &Node| {
            nd.cold.get().is_none_or(|c| {
                c.matched.is_none()
                    && c.looping.is_none()
                    && c.live.is_empty()
                    && c.transition.is_none()
                    && c.opacity_motion.is_none()
            })
        };
        n.local == o.local
            && !o.scale_track
            && n.static_scale == 1.0
            && !o.group_fade
            && n.parent.map(|p| nodes[p].id) == o.parent.map(|p| self.nodes[p].id)
            && plain_cold(n)
            && plain_cold(o)
            && (self.flying.is_empty() || !self.flying.iter().any(|f| f.0 == n.id))
            && (self.aspect_snap.is_empty() || !self.aspect_snap.contains(&n.id))
            && !parent_virtual.is_some_and(crate::el::virtual_list::VirtualList::replaced_view)
            && memo_reuse::same_visual(&n.vis, &o.vis)
            && text_color(n) == text_color(o)
            && n.text.as_ref().is_none_or(|t| {
                t.reveal < 0.0 || self.reveals.get(&n.id).is_some_and(|r| r.words.is_empty() && r.len == t.content.len())
            })
            && self.scene.is_resting_at(n.id, scene_rect)
            && (self.stacks.is_empty()
                || n.parent.is_none_or(|p| nodes[p].growth.is_none())
                || self.reorder_jump(nodes, i) == 0.0)
    }
}

#[cfg(test)]
#[path = "../paint_tracks_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "../live_colors_tests.rs"]
mod live_colors_tests;
