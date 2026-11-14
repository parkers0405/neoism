//! `El::cache` offscreen raster caching: subtree eligibility and state hashing before paint,
//! and bounds/signature sealing when a cached layer closes.
use super::*;

#[derive(Clone, Debug)]
pub(super) struct NodeCache {
    pub(super) signature: u64,
    pub(super) raster_scale: f32,
    pub(super) seen_scale: f32,
    pub(super) scale_changed_at: f64,
    pub(super) env_scale: f32,
    pub(super) was_live: bool,
    pub(super) rel_bounds: [f32; 4],
    pub(super) glyphs: usize,
    pub(super) commit_gen: u64,
    pub(super) paint_state_hash: u64,
}

#[derive(Clone, Copy)]
pub(super) struct ActiveCache {
    pub(super) node_index: usize,
    pub(super) node_id: NodeId,
    pub(super) instance_start: usize,
    pub(super) origin: [f32; 2],
    pub(super) size: [f32; 2],
    pub(super) cur_scale: f32,
    pub(super) raster_scale: f32,
    pub(super) seen_scale: f32,
    pub(super) scale_changed_at: f64,
    pub(super) env_scale: f32,
    /// Everything outside commits that paints the subtree: hover, press, links, an outside
    /// hover group and the window-wide state in `build`'s paint hash.
    pub(super) paint_hash: u64,
    pub(super) glyph_start: usize,
    pub(super) cached_glyphs: usize,
    pub(super) elided: bool,
}

impl<S: 'static> Runtime<S> {
    pub(super) fn prepare_cache_layers(&mut self, sel_ranges: &[(usize, usize, usize)]) -> u64 {
        let (s, n_nodes) = (self.env.scale, self.nodes.len());
        self.cache_live_scratch.clear();
        self.cache_check_scratch.clear();
        self.cache_paint_scratch.clear();
        self.cache_inside_scratch.clear();
        let mut paint_state_hash = 0u64;
        if self.has_cache {
            use std::hash::{Hash, Hasher};
            let mut h = rune_motion::Fx::default();
            self.hovered.hash(&mut h);
            self.hover_within.hash(&mut h);
            self.focus.path().hash(&mut h);
            self.focus.keyboard().hash(&mut h);
            self.focus.hovered().hash(&mut h);
            if let Some(ring) = self.defaults.focus_ring {
                let c = ring.color;
                [ring.width, ring.offset, c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut h);
            }
            (self.srgb_blend, self.text.smoothing, self.text.families(), crate::text::icon::swap_id()).hash(&mut h);
            h.write_u32(s.to_bits());
            sel_ranges.hash(&mut h);
            let mut scroll_mix = 0u64;
            for (&id, st) in &self.scrolls {
                let mut sh = rune_motion::Fx::default();
                sh.write_u64(id.0);
                sh.write_u64(st.offset.to_bits());
                sh.write_u64(st.x.offset.to_bits());
                sh.write_u32(st.held as u32);
                sh.write_u32(st.slack.height().to_bits());
                scroll_mix = scroll_mix.wrapping_add(sh.finish());
            }
            for (path, &off) in &self.input_scroll {
                let mut sh = rune_motion::Fx::default();
                path.hash(&mut sh);
                sh.write_u32(off.to_bits());
                scroll_mix = scroll_mix.wrapping_add(sh.finish());
            }
            h.write_u64(scroll_mix);
            paint_state_hash = h.finish();

            self.cache_live_scratch.resize(n_nodes, false);
            self.cache_check_scratch.resize(n_nodes, false);
            self.cache_paint_scratch.resize(n_nodes, 0);
            // Only cached subtrees read these, so skip everything outside them.
            self.cache_inside_scratch.resize(n_nodes, false);
            for i in 0..n_nodes {
                let n = &self.nodes[i];
                self.cache_inside_scratch[i] = n.vis_flags.cache() || n.parent.is_some_and(|p| self.cache_inside_scratch[p]);
            }
            let store = self.scene.store();
            for i in (0..n_nodes).rev() {
                if !self.cache_inside_scratch[i] { continue; }
                let n = &self.nodes[i];
                let live = store.is_node_animating(n.id)
                    // Sticky and anchored descendants move within a raster without motion tracks.
                    || self.sticky_of(i).is_some()
                    || n.above.is_some()
                    || (!self.reduced_motion && self.loops.get(&n.id).is_some_and(|l| l.active(store.node_time(n.id))))
                    || self.grows.get(&n.id).is_some_and(|g| g.moving())
                    || self.scrolls.get(&n.id).is_some_and(|st| st.slack.height() > 0.0)
                    || self.reveals.get(&n.id).is_some_and(|r| !r.words.is_empty())
                    || self.subtree_exiting(i)
                    || (!self.flights.is_empty() && self.flights.iter().any(|f| f.departs(&n.path, n.key.as_deref()) || f.lands(&n.path, n.key.as_deref())))
                    || n.vis.effect.is_some()
                    // A publication can replace texels without moving the atlas slot, so
                    // identical image primitives do not imply identical raster contents.
                    || n.vis.pixels.is_some()
                    || n.vis.surface.is_some()
                    || (n.material.is_some() && self.native_materials)
                    // Never retain a raster of a secret, even when the field is unfocused.
                    || n.text.as_ref().is_some_and(|t| t.redacted)
                    || n.input.as_ref().is_some_and(|inp| inp.edit.focused || inp.edit.is_secure())
                    || n.text.as_ref().is_some_and(|t| t.advance_in.is_some());
                let check = n.text.as_ref().is_some_and(|t| t.live.is_some())
                    || n.live.iter().any(|b| !n.vis_flags.cache() || !matches!(b.property, crate::live::Property::Offset | crate::live::Property::Scale));
                self.cache_live_scratch[i] |= live;
                self.cache_check_scratch[i] |= check;
                // Hover, press and link paint land without a commit (an instant track or a
                // pointer move inside one text), so hash what this frame samples.
                let links = n.text.as_ref().is_some_and(|t| !t.links.is_empty());
                if n.vis.hover().a > 0.0 || n.vis_flags.hover_group() || n.vis.press().a > 0.0 || n.handlers.get().is_some_and(|h| h.press_scale.is_some()) || links {
                    let mut ph = rune_motion::Fx::default();
                    ph.write_u64(n.id.0);
                    for prop in [P_HOVER, P_PRESS, P_PRESS_SCALE] {
                        ph.write_u32(self.scene.get(n.id, prop).unwrap_or(0.0).to_bits());
                    }
                    if links { self.link_states(i).hash(&mut ph); }
                    self.cache_paint_scratch[i] = self.cache_paint_scratch[i].wrapping_add(ph.finish());
                }
                if let Some(p) = n.parent {
                    self.cache_paint_scratch[p] = self.cache_paint_scratch[p].wrapping_add(self.cache_paint_scratch[i]);
                    // A lifted descendant draws outside this raster; don't elide its inputs.
                    self.cache_live_scratch[p] |= self.cache_live_scratch[i] || n.vis_flags.raise();
                    self.cache_check_scratch[p] |= self.cache_check_scratch[i] || !n.live.is_empty();
                }
            }
        }
        paint_state_hash
    }

    pub(super) fn close_cached_layer(
        &mut self,
        ac: ActiveCache,
        start: usize,
        surface_start: usize,
        glyph_count: &mut usize,
    ) {
        use std::hash::Hasher;
        let layer_grow = (3.0 * crate::render::layer_blur::sigma(self.instances[start].params[2])).ceil();
        if ac.elided {
            let entry = self.node_caches.get_mut(&ac.node_id).expect("elided cache entry");
            entry.seen_scale = ac.seen_scale;
            entry.scale_changed_at = ac.scale_changed_at;
            let k = if (ac.cur_scale - entry.raster_scale).abs() <= 1e-4 {
                1.0
            } else {
                ac.cur_scale / entry.raster_scale.max(1e-4)
            };
            let b0 = (ac.origin[0] + entry.rel_bounds[0] * k).floor();
            let b1 = (ac.origin[1] + entry.rel_bounds[1] * k).floor();
            let b2 = (ac.origin[0] + entry.rel_bounds[2] * k).ceil();
            let b3 = (ac.origin[1] + entry.rel_bounds[3] * k).ceil();
            let sig = entry.signature;
            self.seal_cache_marker(start, [b0, b1, (b2 - b0).max(0.0), (b3 - b1).max(0.0)], ac.node_id, sig);
            *glyph_count += ac.cached_glyphs;
            return;
        }

        if self.instances.len() == start + 1 && self.surfaces.len() == surface_start {
            self.instances.pop();
            if self.track_marks {
                self.marks.truncate(self.instances.len());
            }
            self.evict_cache(ac.node_id);
            return;
        }

        let mut bounds = [ac.origin[0], ac.origin[1], ac.origin[0] + ac.size[0], ac.origin[1] + ac.size[1]];
        let mut h = rune_motion::Fx::default();
        h.write_u64(ac.node_id.0);
        // Blur is baked into the target, unlike opacity and clipping on the composite.
        h.write_u32(self.instances[start].params[2].to_bits());
        h.write_u8(self.srgb_blend as u8 | (self.text.smoothing as u8) << 1);
        h.write_u32(ac.raster_scale.to_bits());
        h.write_u32(ac.size[0].to_bits());
        h.write_u32(ac.size[1].to_bits());
        if let Some(key) = self.nodes[ac.node_index].vis.cache_key {
            h.write_u8(1);
            h.write_u64(key);
        } else {
            h.write_u8(0);
        }
        let q = |v: f32| (v * 64.0).round() as i64;
        let mut turn_stack: Vec<Option<Turn>> = Vec::new();
        for inst in &self.instances[start + 1..] {
            if inst.params[1] == crate::render::KIND_LAYER_END {
                turn_stack.pop();
            } else {
                let active_turn = turn_stack.last().copied().flatten();
                let (mut c0, mut c1, mut c2, mut c3) = if inst.params[1] == crate::render::KIND_LAYER {
                    let next = if inst.border[1] != 0.0 {
                        let own = Turn::about(inst.border[1], [inst.border[2], inst.border[3]], inst.rect, NO_CLIP);
                        Some(active_turn.map_or(own, |t| t.after(own)))
                    } else {
                        active_turn
                    };
                    turn_stack.push(next);
                    let g = (3.0 * crate::render::layer_blur::sigma(inst.params[2])).ceil();
                    let mut r = [inst.rect[0] - g, inst.rect[1] - g, inst.rect[0] + inst.rect[2] + g, inst.rect[1] + inst.rect[3] + g];
                    if inst.border[1] != 0.0 {
                        r = Turn::about(inst.border[1], [inst.border[2], inst.border[3]], inst.rect, NO_CLIP).bbox(r);
                    }
                    (
                        r[0].max(inst.clip[0]),
                        r[1].max(inst.clip[1]),
                        r[2].min(inst.clip[2]),
                        r[3].min(inst.clip[3]),
                    )
                } else {
                    let mut rect = inst.rect;
                    if inst.params[1] == KIND_GLYPH && inst.params[0] != 0.0 {
                        let (sin, cos) = inst.params[0].sin_cos();
                        let width = cos.abs() * rect[2] + sin.abs() * rect[3];
                        let height = sin.abs() * rect[2] + cos.abs() * rect[3];
                        rect = [rect[0] + (rect[2] - width) * 0.5, rect[1] + (rect[3] - height) * 0.5, width, height];
                    }
                    let grow = match inst.params[1] {
                        KIND_GLYPH => 0.0,
                        KIND_SHADOW => inst.params[2] * 3.0 + 1.0,
                        KIND_EFFECT => inst.params[2] + 1.0,
                        KIND_RECT | KIND_GRADIENT | KIND_DASHED | KIND_RADIAL => 1.0 + inst.params[2] * 2.0,
                        _ => 1.0,
                    };
                    (
                        (rect[0] - grow).max(inst.clip[0]),
                        (rect[1] - grow).max(inst.clip[1]),
                        (rect[0] + rect[2] + grow).min(inst.clip[2]),
                        (rect[1] + rect[3] + grow).min(inst.clip[3]),
                    )
                };
                if c2 > c0 && c3 > c1 {
                    if let Some(t) = active_turn {
                        [c0, c1, c2, c3] = t.bbox([c0, c1, c2, c3]);
                    }
                    bounds[0] = bounds[0].min(c0);
                    bounds[1] = bounds[1].min(c1);
                    bounds[2] = bounds[2].max(c2);
                    bounds[3] = bounds[3].max(c3);
                }
            }
            h.write_i64(q(inst.rect[0] - ac.origin[0]));
            h.write_i64(q(inst.rect[1] - ac.origin[1]));
            h.write_i64(q(inst.rect[2]));
            h.write_i64(q(inst.rect[3]));
            for r in inst.radii { h.write_u32(r.to_bits()); }
            for f in inst.fill { h.write_u32(f.to_bits()); }
            for b in inst.border { h.write_u32(b.to_bits()); }
            for p in inst.params { h.write_u32(p.to_bits()); }
            for u in inst.uv { h.write_u32(u.to_bits()); }
            for k in 0..4 {
                let cv = inst.clip[k];
                if cv.abs() < 1e6 {
                    h.write_i64(q(cv - ac.origin[k & 1]));
                } else {
                    h.write_u32(cv.to_bits());
                }
            }
            for fd in inst.fade { h.write_u32(fd); }
        }
        let b0 = (bounds[0] - layer_grow).floor();
        let b1 = (bounds[1] - layer_grow).floor();
        let b2 = (bounds[2] + layer_grow).ceil();
        let b3 = (bounds[3] + layer_grow).ceil();
        let bw = (b2 - b0).max(0.0);
        let bh = (b3 - b1).max(0.0);
        if bw > 4096.0 || bh > 4096.0 {
            self.evict_cache(ac.node_id);
            crate::render::end_layer(&mut self.instances, &mut self.surfaces, &mut self.marks, start, surface_start);
            return;
        }
        let mut sig = h.finish();
        if sig == 0 { sig = 1; }
        let rel_bounds = [b0 - ac.origin[0], b1 - ac.origin[1], b2 - ac.origin[0], b3 - ac.origin[1]];
        let glyphs = *glyph_count - ac.glyph_start;
        let is_resident = self.text.atlas.resident_caches.get(&ac.node_id.0) == Some(&sig);
        self.node_caches.insert(ac.node_id, NodeCache {
            signature: sig,
            raster_scale: ac.raster_scale,
            seen_scale: ac.seen_scale,
            scale_changed_at: ac.scale_changed_at,
            env_scale: ac.env_scale,
            was_live: false,
            rel_bounds,
            glyphs,
            commit_gen: self.commit_gen,
            paint_state_hash: ac.paint_hash,
        });
        if is_resident && self.surfaces.len() == surface_start {
            self.instances.truncate(start + 1);
        }
        self.seal_cache_marker(start, [b0, b1, bw, bh], ac.node_id, sig);
    }

    /// Finishes a cached layer's marker at `rect`, naming its cache entry and signature, and
    /// closes the layer.
    fn seal_cache_marker(&mut self, start: usize, rect: [f32; 4], id: NodeId, sig: u64) {
        let marker = &mut self.instances[start];
        marker.rect = rect;
        marker.params[3] = 1.0;
        marker.radii = crate::render::pack_u64_f32(id.0);
        marker.uv = crate::render::pack_u64_f32(sig);
        self.instances.push(Instance::end_layer());
        if self.track_marks {
            let mark = self.marks.get(start).copied().unwrap_or(0);
            self.marks.resize(self.instances.len(), mark);
        }
    }

    fn evict_cache(&mut self, id: NodeId) {
        self.node_caches.remove(&id);
        self.text.atlas.resident_caches.remove(&id.0);
    }
}
