//! `Runtime::motion_elements`: the part of [`ElementInfo`] a frame-by-frame motion watcher reads,
//! cheap enough to take on every drawn frame. No strings are copied (a watcher caches paths by
//! [`NodeId`] through [`Runtime::element_path`]), text is a hash, and the flags an element takes
//! from its ancestors come from one pass down the tree instead of a walk up from each element.
use super::*;

/// One presented element as [`Runtime::motion_elements`] reports it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionElement {
    pub id: NodeId,
    /// Index into the same list (`out`) of its nearest presented ancestor, as
    /// [`ElementInfo::parent`] is into [`Runtime::elements`]. Always less than its own index.
    pub parent: Option<u32>,
    /// Drawn `[x, y, w, h]` in points, as [`ElementInfo::rect`].
    pub rect: [f32; 4],
    /// The part inside its ancestors' clips, as [`ElementInfo::visible`].
    pub visible: [f32; 4],
    pub opacity: f32,
    pub scale: f32,
    /// [`ElementInfo::loop_phase`], NaN when it has no loop.
    pub loop_phase: f32,
    /// A hash of its words: their length and first and last 64 bytes, so it costs the same for
    /// any length. 0 for none and for secure text.
    pub text_hash: u64,
    pub flags: u16,
}

impl MotionElement {
    /// It or an ancestor is leaving ([`ElementInfo::exiting`]).
    pub const EXITING: u16 = 1;
    /// An ancestor scroll is being scrolled ([`ElementInfo::scrolling`]).
    pub const SCROLLING: u16 = 2;
    /// [`ElementInfo::in_growing_row`] or [`ElementInfo::revealing`].
    pub const GROWING: u16 = 4;
    /// [`ElementInfo::live_intent`] is non-zero.
    pub const LIVE: u16 = 8;
    /// [`ElementInfo::virtual_extent`].
    pub const VIRTUAL: u16 = 16;
    /// Its role is `"text"`.
    pub const TEXT: u16 = 32;
    pub const LOOP: u16 = 64;
    /// It has a visible fill or words.
    pub const PAINTS: u16 = 128;

    /// Whether it has `flag`, one of the consts above.
    pub fn is(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }
}

fn text_hash(text: &str) -> u64 {
    let b = text.as_bytes();
    if b.is_empty() { return 0; }
    let mut h = 0xcbf2_9ce4_8422_2325 ^ b.len() as u64;
    let mut mix = |part: &[u8]| {
        for w in part.chunks(8) {
            let mut v = [0; 8];
            v[..w.len()].copy_from_slice(w);
            h = (h ^ u64::from_le_bytes(v)).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29);
        }
    };
    let head = b.len().min(64);
    mix(&b[..head]);
    mix(&b[b.len().saturating_sub(64).max(head)..]);
    h.max(1)
}

fn intent(n: &Node) -> bool {
    n.live.iter().any(|b| b.spring.is_none() && !b.property.is_color() && !matches!(b.property, crate::live::Property::Blur | crate::live::Property::Trim))
}

impl<S: 'static> Runtime<S> {
    /// Clears `out` and fills it with the presented elements of the last frame, in tree order,
    /// as [`Self::elements`] lists them but without flight sources, links and highlights, and
    /// without the instant-hover-group rule that marks a group-opacity swap as live.
    /// `out` keeps its capacity, so a caller that reuses it allocates nothing per frame.
    pub fn motion_elements(&self, out: &mut Vec<MotionElement>) {
        self.motion_elements_into(out, &mut self.motion_scratch.borrow_mut());
    }

    fn motion_elements_into(&self, out: &mut Vec<MotionElement>, down: &mut Vec<(u32, u16, bool)>) {
        out.clear();
        down.clear();
        // Per node: its index in `out` (or its nearest listed ancestor's), the flags it passes
        // to what's under it, and whether it or an ancestor is leaving.
        let s = self.env.scale;
        for (i, node) in self.nodes.iter().enumerate() {
            // `elements` steps up to an `El::above` target before the parent.
            let up = step_up(&self.nodes, i);
            let mut flags = up.map_or(0, |u| down[u].1);
            let mut pass = flags;
            if intent(node) || self.landing_holds.holds(node.id) { pass |= MotionElement::LIVE; }
            if self.scrolls.get(&node.id).is_some_and(ScrollState::moving) { pass |= MotionElement::SCROLLING; }
            if self.grows.get(&node.id).is_some_and(|g| g.moving()) { pass |= MotionElement::GROWING; }
            let parent = node.parent.map(|p| down[p].0).filter(|&p| p != u32::MAX);
            let exiting = self.is_exiting(node.id) || node.parent.is_some_and(|p| down[p].2);
            down.push((parent.unwrap_or(u32::MAX), pass, exiting));
            if exiting { flags |= MotionElement::EXITING; }
            let Some(p) = self.scene.presented(node.id) else { continue };
            let flight = if self.flights.is_empty() { None } else { self.flights.iter().find(|f| f.lands(&node.path, node.key.as_deref())) };
            if flight.is_some() || self.reveals.get(&node.id).is_some_and(|r| !r.words.is_empty()) { flags |= MotionElement::GROWING; }
            if intent(node) || self.landing_holds.holds(node.id) { flags |= MotionElement::LIVE; }
            let drawn = self.drawn.get(i).copied().unwrap_or([p.rect.x, p.rect.y, p.rect.w, p.rect.h]);
            let rect = flight.map_or(drawn, |f| {
                let (d, c) = (f.destination, f.presented);
                let (zx, zy) = (c.w / d.w.max(1e-3), c.h / d.h.max(1e-3));
                [c.x + (drawn[0] - d.x) * zx, c.y + (drawn[1] - d.y) * zy, drawn[2] * zx, drawn[3] * zy]
            });
            // O(1): the parent's clip and turn were worked out when the frame drew.
            let clip = if self.launches.flying(node.id) { NO_CLIP } else {
                flight.map_or_else(|| self.screen_clip_of_parent(node.parent), |f| f.clip.map(|v| v * s))
            };
            let (x0, y0) = (rect[0].max(clip[0] / s), rect[1].max(clip[1] / s));
            let (x1, y1) = ((rect[0] + rect[2]).min(clip[2] / s), (rect[1] + rect[3]).min(clip[3] / s));
            let secure = node.a11y.secure || node.text.as_ref().is_some_and(|t| t.redacted)
                || node.input.as_ref().is_some_and(|i| i.edit.is_secure());
            let text = match &node.text {
                Some(t) => t.source_text(),
                None if node.role == "text" => node.label.as_deref().unwrap_or(""),
                None => "",
            };
            let text_hash = if secure { 0 } else { text_hash(text) };
            if node.role == "text" { flags |= MotionElement::TEXT; }
            if node.vis.fill.a > 0.01 || text_hash != 0 { flags |= MotionElement::PAINTS; }
            let v = &node.vis;
            if v.fill.a == 0.0 && v.border_width() == 0.0 && v.shadows.is_empty() && node.text.is_none() && v.icon.is_none()
                && !self.virtuals.is_empty() && self.virtuals.contains_key(&node.path)
            {
                flags |= MotionElement::VIRTUAL;
            }
            let loop_phase = self.loops.get(&node.id).map_or(f32::NAN, |l| {
                flags |= MotionElement::LOOP;
                if self.reduced_motion && !matches!(node.looping.as_ref().map(|s| s.kind), Some(rune_motion::LoopKind::Sequence(_))) { 0.0 }
                else { l.phase(self.scene.store().node_time(node.id)) as f32 }
            });
            let inh = self.inh_scratch.get(i);
            down[i].0 = out.len() as u32;
            out.push(MotionElement {
                id: node.id,
                parent,
                rect,
                visible: [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)],
                opacity: flight.map_or_else(|| inh.map_or(p.opacity, |h| h.opacity), |f| f.target_alpha),
                scale: if flight.is_some() { 1.0 } else { inh.map_or(p.transform.scale.x, |h| h.scale) },
                loop_phase,
                text_hash,
                flags,
            });
        }
    }

    /// The path and key of the element `id` names, as [`ElementInfo::path`] and
    /// [`ElementInfo::key`]. An id names one path for the runtime's life, so a watcher can cache
    /// them.
    pub fn element_path(&self, id: NodeId) -> Option<(&str, Option<&str>)> {
        self.node_index.get(&id).map(|&i| (self.nodes[i].path.as_str(), self.nodes[i].key.as_deref()))
    }
}
