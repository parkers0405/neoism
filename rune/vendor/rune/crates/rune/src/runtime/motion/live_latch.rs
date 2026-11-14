//! Live values bound to elements: subscribing to their sources, the wake lane, and latching
//! what they publish into the scene each frame.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Install before the first frame. Live updates use this separate coalesced wake lane.
    pub fn set_live_waker(&mut self, waker: Waker) {
        for sub in &self.live_subscriptions { sub.active.store(false, std::sync::atomic::Ordering::Release); }
        self.live_subscriptions.clear();
        self.live_wake = crate::live::Wake::new(waker);
        self.live_wake.visible(self.surface_visible);
        self.subscribe_live();
    }

    pub fn set_live_visible(&mut self, visible: bool) {
        self.live_wake.visible(visible);
        self.surface_visible = visible;
        if !visible { self.loop_wake = None; }
    }
    pub(crate) fn live_visible(&self) -> bool { self.surface_visible }
    pub fn live_pending(&self) -> bool {
        self.surface_visible && (self.live_dirty || self.live_wake.pending.load(std::sync::atomic::Ordering::Acquire))
    }

    pub(super) fn subscribe_live(&mut self) {
        use std::sync::atomic::Ordering;
        for node in &mut self.nodes {
            if let Some(cold) = node.cold.get_mut_if_some() {
                for bound in &mut cold.live { bound.attach(&self.live_clock, self.clock); }
            }
        }
        for sub in &self.live_subscriptions { sub.active.store(false, Ordering::Release); }
        // Pixels only wake: each frame's build uploads their current image, so unlike bound
        // values they leave nothing to sample on a later visible frame.
        let mut sampled = false;
        for node in &self.nodes {
            let text = node.text.as_ref().and_then(|t| t.live.as_ref()).map(|t| &t.source);
            let pixels = node.vis.cold.get().and_then(|v| v.pixels.as_ref()).map(|p| &p.source);
            let has_live = node.cold.get().is_some_and(|c| !c.live.is_empty());
            if !has_live && text.is_none() && pixels.is_none() {
                continue;
            }
            // An element removed outright leaves the scene now but `nodes` only at the next
            // prune. Exiting elements and their children are still drawn, so their text and
            // pixels keep waking until the exit ends (commit already dropped their bindings).
            if !self.scene.contains(node.id) { continue; }
            sampled |= text.is_some() || has_live;
            for source in node.live.iter().map(|b| &b.source).chain(text).chain(pixels) {
                if let Some(sub) = self.live_subscriptions.iter().find(|s| s.source_id == source.id()) {
                    sub.active.store(true, Ordering::Release);
                } else {
                    self.live_subscriptions.push(source.subscribe(&self.live_wake));
                }
            }
        }
        self.live_subscriptions.retain(|s| s.active.load(Ordering::Acquire));
        self.live_dirty = sampled;
    }

    /// Clear before this frame reads anything a source publishes (a picture's size for layout,
    /// a bound value): a concurrent publication either lands in this frame or keeps a wake for
    /// the next. Never clear after reading (lost wake race). Hidden hosts keep the wake for show.
    pub(super) fn take_live_wake(&self) -> bool {
        self.surface_visible && self.live_wake.pending.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    pub(super) fn latch_live(&mut self, pending: bool) -> bool {
        if !self.surface_visible || self.live_subscriptions.is_empty() { return false; }
        if !pending && !self.live_dirty { return false; }
        self.live_dirty = false;
        for node in &mut self.nodes {
            if let Some(spec) = &mut node.text
                && let Some(live) = &spec.live
            {
                let seen = &mut node.cold.live_text_seen;
                if let Some(text) = live.read(seen) && spec.source_text() != &*text {
                    spec.replace_content(text.to_string());
                    spec.apply_case();
                    node.text_layout = Some(self.text.layout_live(&spec.content, spec.style, self.env.scale));
                }
            }
            let Some(cold) = node.cold.get_mut_if_some() else { continue };
            for bound in &mut cold.live {
                let (op, active) = bound.read(&self.live_clock, self.clock);
                self.live_dirty |= active;
                let Some(op) = op else { continue };
                let (enter, leave) = (bound.spring, bound.rest_spring);
                use crate::live::{LatchOp, LiveValue, Property};
                fn apply<T: rune_motion::Animatable + Copy>(
                    scene: &mut Scene,
                    id: NodeId,
                    prop: Prop<T>,
                    enter: Option<Spring>,
                    leave: Option<Spring>,
                    decode: impl Fn(u128) -> T,
                    op: LatchOp,
                    choose_leave: impl Fn(Option<T>, T) -> bool,
                ) {
                    match op {
                        LatchOp::Snap(bits) => scene.set(id, prop, decode(bits)),
                        LatchOp::Set(bits) => {
                            let v = decode(bits);
                            let spring = if choose_leave(scene.get(id, prop), v) { leave.or(enter) } else { enter };
                            match spring {
                                Some(s) => scene.animate(id, prop, v, s),
                                None => scene.set(id, prop, v),
                            }
                        }
                        LatchOp::Pulse { active, rest, hold } => {
                            let rest_v = decode(rest);
                            let Some(enter_s) = enter else {
                                scene.set(id, prop, rest_v);
                                return;
                            };
                            let active_v = decode(active);
                            let leave_s = leave.unwrap_or(enter_s);
                            scene.animate(id, prop, active_v, enter_s);
                            scene.animate(id, prop, rest_v, rune_motion::Anim { delay: (hold as f32).max(1e-4), ..leave_s.into() });
                        }
                    }
                }
                match bound.property {
                    Property::Opacity => apply(
                        &mut self.scene, node.id, prop::OPACITY, enter, leave,
                        |b| f32::decode(b).clamp(0.0, 1.0), op,
                        |cur, to| cur.is_some_and(|c| to < c),
                    ),
                    Property::Blur => apply(
                        &mut self.scene, node.id, prop::BLUR, enter, leave,
                        |b| f32::decode(b).max(0.0), op,
                        |cur, to| cur.is_some_and(|c| to < c),
                    ),
                    Property::Scale => apply(
                        &mut self.scene, node.id, prop::SCALE, enter, leave,
                        |b| f32::decode(b).max(0.0), op,
                        |cur, to| cur.is_some_and(|c| to < c),
                    ),
                    Property::Offset => apply(
                        &mut self.scene, node.id, prop::OFFSET, enter, leave,
                        |b| { let [x, y] = <[f32; 2]>::decode(b); rune_motion::Vec2::new(x, y) }, op,
                        |_, _| false,
                    ),
                    Property::Trim => apply(
                        &mut self.scene, node.id, P_TRIM, enter, leave,
                        |b| { let [s, e] = <[f32; 2]>::decode(b); rune_motion::Vec2::new(s, e) }, op,
                        |_, _| false,
                    ),
                    Property::Fill | Property::TextColor | Property::Border | Property::Shadow | Property::GradientTop | Property::GradientBottom => {
                        let bits = match op {
                            LatchOp::Snap(b) | LatchOp::Set(b) | LatchOp::Pulse { rest: b, .. } => b,
                        };
                        // The renderer uses the exact authored colour after its track settles.
                        let c = Color::decode(bits);
                        let p = match bound.property {
                            Property::Fill => { node.vis.fill = c; P_FILL }
                            Property::TextColor => {
                                if let Some(spec) = &mut node.text { if spec.color != c { spec.color = c; } }
                                else if node.vis.icon.is_some_and(|(_, ic)| ic != c) && let Some((_, ic)) = &mut node.vis.cold_mut().icon { *ic = c; }
                                P_TEXT
                            }
                            Property::Border => { node.vis.set_border_color(c); P_BORDER }
                            Property::Shadow => {
                                if node.vis.shadows.first().is_some_and(|s| s.color != c) { node.vis.cold_mut().shadows[0].color = c; }
                                P_SHADOWS[0]
                            }
                            _ => {
                                let k = usize::from(bound.property == Property::GradientBottom);
                                if node.vis.gradient.as_deref().is_some_and(|g| g[k] != c)
                                    && let Some(g) = node.vis.cold_mut().gradient.as_deref_mut() { g[k] = c; }
                                P_GRADIENT[k]
                            }
                        };
                        apply(&mut self.scene, node.id, p, enter, leave, Color::decode, op, |_, _| false);
                    }
                    Property::Rect => {
                        let frame_of = |b| {
                            let [x, y, w, h] = <[f32; 4]>::decode(b);
                            MRect::new(x, y, w.max(0.0), h.max(0.0))
                        };
                        match (op, enter) {
                            (LatchOp::Snap(b) | LatchOp::Pulse { rest: b, .. }, _) | (LatchOp::Set(b), None) => {
                                self.scene.land_frame(node.id, frame_of(b));
                            }
                            (LatchOp::Set(b), Some(spring)) => {
                                self.scene.set_frame(node.id, frame_of(b), LayoutAnim::new(spring));
                            }
                        }
                    }
                }
            }
        }
        self.live_dirty
    }
}
