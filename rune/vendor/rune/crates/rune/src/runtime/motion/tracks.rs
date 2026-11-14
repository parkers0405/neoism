//! Render-side growth, scroll and streaming word-reveal tracks, plus animated paint samples.
use super::*;

/// Scroll momentum decay (iOS normal: 0.998 per ms).
pub(super) const SCROLL_DECAY: f32 = 2.002;

fn ease_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x) * (1.0 - x) * (1.0 - x)
}

pub(super) fn word_starts(s: &str, from: usize, out: &mut Vec<u32>) {
    let mut prev_space = true;
    for (i, c) in s[from..].char_indices() {
        let space = c.is_whitespace();
        if !space && prev_space {
            out.push((from + i) as u32);
        }
        prev_space = space;
    }
}

/// The (x, y) scroll offsets of node `i`'s scroll viewports, innermost first.
pub(super) fn scroll_offsets<'a>(scrolls: &'a HashMap<NodeId, ScrollState>, nodes: &'a [Node], i: usize) -> impl Iterator<Item = [f32; 2]> + 'a {
    std::iter::successors(Some(i), |&p| nodes[p].parent.filter(|_| nodes[p].above.is_none()))
        .skip(1)
        .filter_map(|p| scrolls.get(&nodes[p].id).map(|st| [st.x.shift() as f32, st.offset as f32]))
}

/// Marks every ancestor of a marked node.
pub(super) fn mark_ancestors(nodes: &[Node], marked: &mut [bool]) {
    for i in 0..nodes.len() {
        if !marked[i] { continue; }
        let mut p = nodes[i].parent;
        while let Some(j) = p {
            if marked[j] { break; }
            marked[j] = true;
            p = nodes[j].parent;
        }
    }
}

impl<S: 'static> Runtime<S> {
    pub(super) fn set_or_anim<T: rune_motion::Animatable>(&mut self, id: NodeId, p: Prop<T>, val: T, s: Spring, instant: bool) {
        if instant {
            self.scene.set(id, p, val);
        } else {
            self.scene.animate(id, p, val, s);
        }
    }

    pub(super) fn color(&self, id: NodeId, prop: Prop<Color>, rest: Color) -> Color {
        if self.scene.store().is_animating(id, prop) { self.scene.get(id, prop).unwrap_or(rest) } else { rest }
    }

    /// Shadow `k` of node `id` with its geometry where its spring has it.
    pub(super) fn shadow_at(&self, id: NodeId, k: usize, sh: &crate::el::Shadow) -> crate::el::Shadow {
        match P_SHADOW_GEO.get(k) {
            Some(&prop) if self.scene.store().active_len() > 0 && self.scene.store().is_animating(id, prop) => match self.scene.get(id, prop) {
                Some(g) => crate::el::Shadow { x: g.x, y: g.y, blur: g.w.max(0.0), spread: g.h, ..*sh },
                None => *sh,
            },
            _ => *sh,
        }
    }

    pub(super) fn step_growth(&mut self, dt: f32) -> bool {
        if self.stacks.is_empty() {
            return false;
        }
        self.grow_acc += dt;
        let steps = (self.grow_acc / SUBSTEP).floor() as usize;
        self.grow_acc -= steps as f32 * SUBSTEP;
        let now = self.t;
        let mut moving = false;
        let (mut restored, mut gave) = (Vec::new(), Vec::new());
        for (cid, stack) in self.stacks.iter_mut() {
            let Some(&ci) = self.node_index.get(cid) else { continue };
            let Some(spec) = self.nodes[ci].growth.as_deref().copied() else { continue };
            let n_rows = stack.order.len();
            let mut st_growing = !stack.pending_heights.is_empty();
            let mut st_moving = st_growing;
            let (in_runs, run_grown) = Self::step_growth_runs(stack, &mut self.grows, steps.min(240), spec);
            stack.grown += run_grown;
            for (k, id) in stack.order.iter().enumerate() {
                let Some(g) = self.grows.get_mut(id) else { continue };
                let h0 = g.h;
                let in_run = in_runs.contains(id);
                if let Some(&at) = stack.closing.get(id)
                    && now >= at
                {
                    if self.reduced_motion {
                        *g = GrowTrack::settled(0.0);
                    } else {
                        g.retarget(0.0);
                    }
                }
                // A row above the tail (the last row) carries it.
                let carrying = k + 2 == n_rows;
                for substep in 0..if in_run { 0 } else { steps.min(240) } {
                    if !stack.pending_heights.is_empty() {
                        let at = now - f64::from(self.grow_acc) - f64::from((steps.min(240) - substep) as f32 * SUBSTEP);
                        if at >= stack.width_hold_until && let Some(height) = stack.pending_heights.remove(id) {
                            g.retarget(height);
                        }
                    }
                    g.step(spec.spring, &spec.edge, carrying);
                }
                // A row nobody saw last frame draws no feather: it has none to ease, and grows one
                // again as it comes into view.
                if g.mask > 0.0 && self.built
                    && self.node_index.get(id).is_some_and(|&i| !self.seen.get(i).copied().unwrap_or(true))
                {
                    g.mask = 0.0;
                }
                let r = std::mem::take(&mut g.restore);
                if r > 0.0 { restored.push((ci, *id, r)); }
                if let Some(share) = g.give { gave.push((*id, (h0 - g.h) * share)); }
                if !g.moving() { g.give = None; }
                stack.grown += (g.h - h0).max(0.0);
                st_growing |= g.target > g.h + 1.0 / 128.0;
                st_moving |= g.moving() || (g.h - h0).abs() > 1e-5;
                st_moving |= stack.slides.step(*id, g, spec.spring, steps.min(240));
            }
            stack.order.retain(|id| {
                let keep = !stack.closing.contains_key(id) || self.grows.get(id).is_some_and(|g| g.h > 0.01 || g.target > 0.0);
                if !keep {
                    self.grows.remove(id);
                }
                keep
            });
            stack.closing.retain(|id, _| stack.order.contains(id));
            stack.slides.keep(&stack.order);
            stack.growing = st_growing;
            stack.moving = st_moving;
            // Keep the shared clock awake through the hold. No timer thread or remeasurement.
            moving |= st_moving;
        }
        for (ci, id, r) in restored { self.restore_above(ci, id, r); }
        for (id, d) in gave { self.lower_hold(id, d); }
        moving
    }

    /// Presented content extent of node `c` if it is a growth stack (points).
    pub(super) fn stack_extent(&self, c: usize) -> Option<f64> {
        let n = &self.nodes[c];
        n.growth.as_ref()?;
        let stack = self.stacks.get(&n.id)?;
        let mut count = 0usize;
        let rows: f64 = stack.order.iter().filter_map(|id| self.grows.get(id)).map(|g| { count += 1; f64::from(g.h) }).sum();
        let gaps = f64::from(stack.gap) * count.saturating_sub(1) as f64;
        Some(f64::from(stack.pad_top) + rows + gaps + f64::from(stack.pad_bottom))
    }

    /// How far scroll viewport `i`'s content reaches past its bottom, negative while it fits.
    pub(super) fn scroll_overflow(&self, i: usize) -> f64 {
        let view = self.nodes[i].target;
        let content_h = self.scroll_children(i).map(|c| {
            let n = &self.nodes[c];
            f64::from(n.target[1] - view[1]) + self.stack_extent(c).unwrap_or(f64::from(n.target[3] - n.target[1]))
        }).reduce(f64::max).unwrap_or(0.0);
        content_h - f64::from(view[3] - view[1])
    }

    pub(super) fn step_scroll(&mut self, dt: f32) -> bool {
        let slack_dt = std::mem::take(&mut self.slack_dt);
        let mut moving = false;
        for i in 0..self.nodes.len() {
            let Some(sp) = self.nodes[i].scroll else { continue };
            if self.subtree_exiting(i) { continue; }
            let id = self.nodes[i].id;
            let axes = self.nodes[i].scroll_axes;
            if axes.x() { moving |= self.step_scroll_x(i, dt); }
            // A horizontal viewport doesn't move vertically, even with content taller than it.
            let overflow = if axes.y() { self.scroll_overflow(i) } else { 0.0 };
            if let Some(active) = self.step_touch_scroll(id, overflow, dt, false) {
                moving |= active;
                continue;
            }
            let held = (!self.keeps.is_empty() || !self.scroll_reveals.is_empty()) && {
                let path = &self.nodes[i].path;
                (self.keeps.iter().any(|k| k.viewport == *path) || self.scroll_reveals.iter().any(|r| r.viewport == *path))
                    && self.scroll_stack(i).and_then(|c| self.stacks.get(&self.nodes[c].id)).is_some_and(|s| s.growing)
            };
            let laid = if axes.y() { self.ease_to_shrunk_end(i, overflow) } else { None };
            let Some(st) = self.scrolls.get_mut(&id) else { continue };
            st.scrolling = st.direct || st.coast().abs() > 1.0 || st.x.v.abs() > 1.0;
            st.by_hand |= st.scrolling;
            st.direct = false;
            st.max = overflow.max(0.0);
            match st.mode {
                ScrollMode::Pinned if sp.stick_bottom => {
                    // A reader stuck on a shrinking stack's end doesn't rise with its rows' last fraction.
                    st.offset = laid.filter(|&end| st.offset <= end + 1e-6).map_or(st.max, |end| st.max.min(end));
                }
                ScrollMode::Glide { .. } => moving |= scroll_bottom::glide(st, dt),
                ScrollMode::Momentum { v } if v.abs() > 1.0 => {
                    // Exact for exponential decay, so a long frame carries the same distance as
                    // the short frames it stands for.
                    let decay = (-SCROLL_DECAY * dt).exp();
                    st.offset += f64::from(v * (1.0 - decay) / SCROLL_DECAY);
                    let edge = st.offset <= 0.0 || st.offset >= st.max;
                    st.mode = if edge { ScrollMode::Rest } else { ScrollMode::Momentum { v: v * decay } };
                    moving = true;
                }
                ScrollMode::Momentum { .. } => st.mode = ScrollMode::Rest,
                ScrollMode::Pinned | ScrollMode::Rest => {}
            }
            st.offset = st.offset.clamp(0.0, st.max);
            // Landed on a shrinking stack's new end: stick once its rows are within half a point
            // of it, not while they're still short of it. A glide that reaches the end hands it
            // to the pin.
            let shrinking = laid.is_some_and(|end| end < st.max - 0.5 && st.offset < st.max - 1e-3);
            if sp.stick_bottom && !held && !shrinking && st.offset >= st.max - 0.5 && st.coast().abs() <= 1.0 {
                st.pin();
            }
            st.laid = laid;
            if sp.stick_bottom && axes.y() && slack_dt > 0.0 { moving |= self.step_slack(i, slack_dt); }
        }
        moving
    }

    pub(super) fn reveals_active(&mut self) -> bool {
        let store = self.scene.store();
        let mut any = false;
        for (id, st) in self.reveals.iter_mut() {
            let now = store.node_time(*id);
            let fade = st.fade as f64;
            // Words fully shown leave the list (only from the front, which keeps lookups valid).
            let done = st.words.iter().take_while(|w| now - w.1 >= fade).count();
            if done > 0 && done == st.words.len() {
                st.words.clear();
            }
            any |= !st.words.is_empty() && store.node_time_scale(*id) > 0.0;
        }
        any
    }

    pub(super) fn reveal_alpha(st: &RevealState, byte: u32, now: f64) -> f32 {
        let i = st.words.partition_point(|w| w.0 <= byte);
        if i == 0 {
            return 1.0;
        }
        let t0 = st.words[i - 1].1;
        if st.fade <= 0.0 {
            return if now >= t0 { 1.0 } else { 0.0 };
        }
        ease_out(((now - t0) / st.fade as f64) as f32)
    }
}
