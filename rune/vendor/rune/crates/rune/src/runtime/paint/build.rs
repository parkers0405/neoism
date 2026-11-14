//! Frame paint pass: `Scene` presented poses, clips, layers and text → GPU `Instance`s.
use super::*;

impl<S: 'static> Runtime<S> {
    pub(super) fn build(&mut self) -> bool {
        let (_paint_time, _blend) = (crate::frame::profile::Scope::new(crate::frame::profile::Phase::Paint), crate::theme::scope(self.srgb_blend));
        let s = self.env.scale;
        let snap_px = |v: f32| grid(v, s) * s;
        self.instances.clear();
        let mut pinned_pages = 0u32;
        for f in &self.flights {
            for q in &f.glyphs {
                if q.params[1] == KIND_GLYPH {
                    pinned_pages |= 1u32 << (q.params[2] as u32).min(31);
                }
            }
        }
        self.text.begin_frame_pinned(pinned_pages);
        self.marks.clear();
        self.icon_aprons.clear();
        self.surfaces.clear();
        self.materials.clear();
        self.hits.clear();
        self.clear_help_hits();
        self.clear_drop_hits();
        self.loops_active = false;
        self.static_scale_settling = false;
        self.videos_active = false;
        let now = crate::time::Instant::now();
        let video_clock = match self.wall_start {
            None => crate::video::Clock::Virtual(self.clock),
            Some(_) => crate::video::Clock::Display(self.frame_target.filter(|t| *t > now).unwrap_or(now)),
        };
        let mut video_signature = 0u64;
        self.loop_wake = None;
        self.overlay_hits.clear();
        let n_nodes = self.nodes.len();
        self.inh_scratch.clear();
        self.turn_scratch.clear();
        self.unturned_drawn.clear();
        self.inh_scratch.resize(n_nodes, Inh::default());
        self.drawn.clear();
        self.drawn.resize(n_nodes, [0.0; 4]);
        self.drawn_scale.clear();
        self.drawn_scale.resize(n_nodes, 1.0);
        self.seen.clear();
        self.seen.resize(n_nodes, false);
        self.stuck.clear();
        self.sticky_bands.clear();
            self.sticky_frame.clear();
        if self.has_sticky {
            self.stuck.resize(n_nodes, 0.0);
            self.sticky_frame.resize(n_nodes, sticky::UNBOUNDED);
        }
        let sel_ranges = self.selection_ranges();
        self.ime_area = None;
        let now = self.t;
        let vp = [0.0, 0.0, self.viewport.0 * s, self.viewport.1 * s];
        let mut glyph_count = 0usize;
        let mut backdrop_blurs = 0usize;
        let mut ambient: Option<f32> = None;
        // Retain scope capacity across frames. Records preserve absolute instance indices
        // for images, video insertions, retained springs and compositor list ranges.
        let mut layers = std::mem::take(&mut self.layer_scratch);
        layers.clear();

        // One linear pass, including retained exits, Live opacity, presence and loops. Reuse
        // inherited scratch for hover groups so activation samples exactly the paint values.
        self.dissolve_scratch.clear();
        if self.has_dissolve { self.dissolve_scratch.resize(n_nodes, false); }
        for index in 0..if self.has_dissolve { n_nodes } else { 0 } {
            let i = self.layer_paint_order.get(index).copied().unwrap_or(index);
            let n = &self.nodes[i];
            let inherited_group = n.parent.map_or(0.0, |p| self.inh_scratch[p].group);
            self.inh_scratch[i].group = if n.vis.hover().a > 0.0 || n.vis_flags.hover_group() {
                self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0)
            } else { inherited_group };
            let Some(parent) = n.parent.filter(|&p| self.nodes[p].vis_flags.blend() == crate::render::Blend::PlusLighter) else { continue };
            let Some(p) = self.scene.presented(n.id) else { continue };
            let mut opacity = p.opacity * n.vis.layer_opacity;
            opacity *= self.group_factor(n, inherited_group);
            if fades(n.looping.as_deref().copied()) && let Some(track) = self.loops.get(&n.id) {
                opacity *= track.sample_at(self.scene.store().node_time(n.id), self.clock, self.reduced_motion);
            }
            self.dissolve_scratch[parent] |= opacity > 0.001 && opacity < 1.0;
        }

        let paint_state_hash = self.prepare_cache_layers(&sel_ranges);

        self.place_growth_rows();

        // Exiting descendants are appended to storage. Bring them back into their subtree
        // only when isolation requires it; the ordinary painter stream stays byte-identical.
        // Matched flights paint last (see `update_layer_paint_order`).
        let flying = !self.flying.is_empty();
        let tree_order = ((flying || self.has_sticky || self.has_raise || self.has_paint_order) && !self.layer_paint_order.is_empty()) || self.layer_paint_order.windows(2).any(|pair| {
            let n = &self.nodes[pair[0]];
            self.nodes[pair[1]].parent == Some(pair[0]) && ((self.has_dissolve && self.dissolve_scratch[pair[0]]) || n.vis_flags.cache() || n.vis.clip_radius.is_some() || n.vis.group_opacity.is_some() || n.group_fade
                || n.vis.layer_opacity < 1.0 || fades(n.looping.as_deref().copied())
                || self.scene.presented(n.id).is_some_and(|p| (p.opacity > 0.0 && p.opacity < 1.0) || p.blur > 0.0 || p.transform.rotate.abs() > 1e-6))
        });
        let default_ring = self.default_ring_owner();
        let mut active_cache: Option<ActiveCache> = None;
        self.pixel_snap.begin_frame(self.t, [vp[2], vp[3]]);
        for paint_index in 0..n_nodes {
            let i = if tree_order { self.layer_paint_order[paint_index] } else { paint_index };
            if let Some(rank) = self.paint_rank.as_mut() {
                rank.resize(n_nodes, 0);
                rank[i] = paint_index as u32;
            }
            let node_id = self.nodes[i].id;
            let node_parent = self.nodes[i].parent;
            // A flight paints outside every ancestor's layer, above the whole window.
            let lift = flying && self.flying.iter().any(|f| f.0 == node_id);
            let raised = self.nodes[i].vis_flags.raise() && !lift;
            if lift || raised {
                while let Some((_, start, surface_start)) = layers.pop() {
                    if active_cache.is_some_and(|ac| ac.instance_start == start) {
                        let ac = active_cache.take().unwrap();
                        self.close_cached_layer(ac, start, surface_start, &mut glyph_count);
                    } else {
                        crate::render::end_layer(&mut self.instances, &mut self.surfaces, &mut self.marks, start, surface_start);
                    }
                }
            }
            while let Some(&(ancestor, start, surface_start)) = layers.last() {
                let mut parent = node_parent;
                while parent.is_some_and(|p| p != ancestor) { parent = self.nodes[parent.unwrap()].parent; }
                if parent == Some(ancestor) { break; }
                if active_cache.is_some_and(|ac| ac.instance_start == start) {
                    let ac = active_cache.take().unwrap();
                    self.close_cached_layer(ac, start, surface_start, &mut glyph_count);
                } else {
                    crate::render::end_layer(&mut self.instances, &mut self.surfaces, &mut self.marks, start, surface_start);
                }
                layers.pop();
            }
            let n = &self.nodes[i];
            let Some(p) = self.scene.presented(n.id) else { continue };
            let mut pinh = n.parent.map_or_else(|| self.root_inh(), |pr| self.inh_scratch[pr]);
            // A stuck header's title (its first child) fades as the next header comes up.
            if pinh.first_fade < 1.0 && n.parent.is_some_and(|pr| pr + 1 == i) {
                pinh.opacity *= pinh.first_fade;
                pinh.paint_opacity *= pinh.first_fade;
            }
            pinh.first_fade = 1.0;
            // `El::stuck_reveal`: shown as far as its sticky parent has held.
            if let Some(span) = n.sticky.as_ref().and_then(|s| s.reveal)
                && let Some(pr) = n.parent
            {
                let held = if self.has_sticky { self.stuck.get(pr).map_or(0.0, |s| s.abs()) } else { 0.0 };
                let shown = (held / span).clamp(0.0, 1.0);
                pinh.opacity *= shown;
                pinh.paint_opacity *= shown;
            }
            pinh.dy += self.row_dy[i];
            pinh.snap.rigid[1] += self.row_dy[i];
            if let Some(a) = &n.above
                && let Some(j) = anchor_index(&self.nodes, i, a)
            {
                self.anchored_inh(i, j, a, &p, &mut pinh);
            }
            // `El::stuck_fade`: how much of a stuck header's title shows.
            let mut kids_fade = 1.0f32;
            if self.has_sticky && let Some((edge, inset, tail)) = self.sticky_of(i) {
                // Worked out from this frame's offsets, which the viewport above already applied.
                // Exclude this node's own `prop::OFFSET` (`live_offset`) so it translates from the
                // stuck place rather than being clamped away by the sticky edge.
                let (frame, viewport) = self.sticky_frame_of(i, edge);
                let own_off = self.scene.get(n.id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
                let (top, left) = (p.rect.y - own_off.y + pinh.dy, p.rect.x - own_off.x + pinh.dx);
                let h = if self.row_h[i] >= 0.0 { self.row_h[i] } else { p.rect.h };
                let shift = sticky::shift(edge, if tail { inset - h } else { inset }, [top, top + h, left, left + p.rect.w], frame);
                self.stuck[i] = shift;
                if edge == crate::el::StickyEdge::Top { kids_fade = self.stuck_fade(i, top + shift, pinh.dy); }
                // A header whose section's rows fade under it keeps the clip they lost.
                if let Some(&(_, top)) = self.sticky_bands.iter().find(|b| Some(b.0) == node_parent) {
                    pinh.paint_clip[1] = top;
                }
                if edge == crate::el::StickyEdge::Left { pinh.dx += shift } else { pinh.dy += shift }
                pinh.snap.rigid[(edge != crate::el::StickyEdge::Left) as usize] += shift;
                // A sticky element stays crisp against the viewport edge it sticks to while rows
                // scrolling under its fill fade out.
                let slot = match edge {
                    crate::el::StickyEdge::Left => 0,
                    crate::el::StickyEdge::Top => 1,
                    crate::el::StickyEdge::Bottom => 3,
                };
                if let Some(viewport) = viewport.filter(|&v| self.nodes[v].scroll_edges.fade > 0.0) {
                    let (width, origin) = self.sticky_fade(n.parent.unwrap(), viewport, slot, false);
                    pinh.fade[slot] = width;
                    pinh.fade_origin[slot] = origin;
                    let (width, origin) = self.sticky_fade(n.parent.unwrap(), viewport, slot, true);
                    pinh.paint_fade[slot] = width;
                    pinh.paint_fade_origin[slot] = origin;
                }
            }
            if raised {
                // Out of its ancestors' layers: their opacity applies directly instead.
                pinh.paint_opacity = pinh.opacity;
                pinh.paint_clip = pinh.clip;
                pinh.paint_fade = pinh.fade;
                pinh.paint_fade_origin = pinh.fade_origin;
                pinh.in_cache = false;
                pinh.skip_paint = false;
                // Rounded clips and turns consumed by an ancestor's layer must be reapplied
                // outside that layer too. Reopen only the masks and turns, not opacity or blur.
                pinh.rounded = None;
                let mut chain = Vec::new();
                let mut parent = node_parent;
                while let Some(p) = parent {
                    chain.push(p);
                    parent = self.nodes[p].parent;
                }
                for &p in chain.iter().rev() {
                    let ancestor = &self.nodes[p];
                    let k = self.inh_scratch[p].turn;
                    if k != 0 && ancestor.parent.map_or(0, |pp| self.inh_scratch[pp].turn) != k {
                        let t = self.turn_scratch[k as usize - 1];
                        let start = self.instances.len();
                        let marker = Instance::begin_layer(t.rect, t.outer_clip, Default::default()).turned(t.angle, t.pivot);
                        self.instances.push(marker);
                        layers.push((p, start, self.surfaces.len()));
                    }
                    if let Some(radius) = ancestor.vis.clip_radius {
                        let [x, y, w, h] = self.unturned_drawn.get(p).filter(|_| k != 0)
                            .map_or_else(|| self.drawn[p].map(|v| v * s), |&(b, _)| [b[0] * s, b[1] * s, (b[2] - b[0]) * s, (b[3] - b[1]) * s]);
                        let radii = ancestor.vis.clip_corners.unwrap_or_else(|| {
                            if ancestor.vis.radii() != [0.0; 4] {
                                self.scene.presented(ancestor.id).map_or(ancestor.vis.radii(), |p| p.radius.0)
                            } else { [radius; 4] }
                        }).map(|radius| {
                            let radius = (radius.max(0.0) * self.inh_scratch[p].scale * s).min(w.min(h) * 0.5);
                            if ancestor.vis_flags.continuous_corners() { -radius } else { radius }
                        });
                        let start = self.instances.len();
                        let mut marker = Instance::begin_layer([x, y, w, h], [x, y, x + w, y + h], Default::default());
                        marker.round_clip_corners(radii);
                        self.instances.push(marker);
                        layers.push((p, start, self.surfaces.len()));
                    }
                }
            }
            if lift {
                // Keep the destination's clips (consumed ones included) and offsets. Drop its
                // ancestors' opacity, scale and blur: an entering parent's fade or scale would
                // otherwise hide the flight or jump it off the rect it was seen at. The scales
                // holding still above that parent (an app-wide zoom) stay, so it flies at the size
                // it lands at. Inside another flight, also drop that flight's cover offset and clip.
                let [clip, fade, fade_origin] = if self.launches.flying(node_id) {
                    // A launch leaves where its clips don't reach.
                    [crate::render::NO_CLIP, [0.0; 4], pinh.fade_origin]
                } else {
                    pinh.outer.unwrap_or_else(|| {
                        let clip = if pinh.turn != 0 { self.screen_clip_of_parent(n.parent) } else { pinh.clip };
                        [clip, pinh.fade, pinh.fade_origin]
                    })
                };
                let (scale, anchor, anchor_to) = pinh.still;
                pinh = Inh { opacity: 1.0, paint_opacity: 1.0, hit_opacity: 1.0, clip, fade, fade_origin, paint_clip: clip, paint_fade: fade,
                    paint_fade_origin: fade_origin, rounded: None, scale, anchor, anchor_to, blur: 0.0,
                    dx: pinh.dx - pinh.cover[0], dy: pinh.dy - pinh.cover[1], cover: [0.0; 2], outer: None,
                    scale_motion: ScaleMotion::Still, in_cache: false, skip_paint: false, turn: 0, snap: pixel_snap::Inherit { layer: 0, parent: None, ..pinh.snap }, ..pinh };
                let fix = self.launches.offset_fix(&self.scene, node_id, [pinh.dx, pinh.dy], self.commit_gen);
                (pinh.dx, pinh.dy) = (pinh.dx + fix[0], pinh.dy + fix[1]);
            }
            let mark = if n.vis.mark != 0 { n.vis.mark } else { pinh.mark };
            let mut own = p.opacity;
            let mut hit_own = own * self.group_factor(n, 1.0);
            own *= self.group_factor(n, pinh.group);
            let base_opacity = (own * pinh.opacity).clamp(0.0, 1.0);
            let hover_owner = n.vis.hover().a > 0.0 || n.vis_flags.hover_group();
            let group = if hover_owner {
                self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0)
            } else {
                pinh.group
            };
            // Loops and reveals run on the node's clock (`El::time_scale`).
            let track = self.loops.get(&n.id);
            let lnow = if track.is_some() { self.scene.store().node_time(n.id) } else { self.t };
            let loop_value = track.map(|l| l.sample_at(lnow, self.clock, self.reduced_motion));
            if fades(n.looping.as_deref().copied()) && let Some(v) = loop_value {
                own *= v;
                hit_own *= v;
            }
            let effective_opacity = (own * pinh.opacity).clamp(0.0, 1.0);
            let hit_opacity = (hit_own * pinh.hit_opacity).clamp(0.0, 1.0);
            let next = if tree_order { self.layer_paint_order.get(paint_index + 1).copied() } else { (i + 1 < n_nodes).then_some(i + 1) };
            let dissolve = self.has_dissolve && self.dissolve_scratch[i];
            let additive_parent = self.has_dissolve && n.parent.is_some_and(|p| self.dissolve_scratch[p]);
            // `El::layer_opacity` fades paint only; hit testing and what elements report keep
            // `own`.
            let paint_own = own * n.vis.layer_opacity;
            // A fading node with children, or a shape whose pieces overlap, takes its opacity
            // once as a layer rather than each primitive taking it over the ones beneath. So
            // does a layer opacity over two paints of the node's own (a label on its fill).
            let mut isolate = effective_opacity > 0.001 && (p.blur > 0.0 || dissolve
                || additive_parent || (paint_own > 0.001 && paint_own < 1.0
                    && (next.is_some_and(|j| self.nodes[j].parent == Some(i)) || n.vis.shape.as_ref().is_some_and(|s| s.overlaps())
                        || (n.vis.layer_opacity < 1.0 && own_paints(n) >= 2))));
            let (mut clip, mut fade) = (pinh.paint_clip, pinh.paint_fade);
            let mut fade_origin = pinh.paint_fade_origin;
            let mut rounded = pinh.rounded;
            let has_press_scale = n.handlers.get().is_some_and(|h| h.press_scale.is_some());
            let press_k = if has_press_scale { self.scene.get(n.id, P_PRESS_SCALE).unwrap_or(1.0) } else { 1.0 };
            let sequence = track
                .filter(|_| matches!(n.looping.as_ref().map(|l| l.kind), Some(rune_motion::LoopKind::Sequence(_))));
            let static_k = if n.scale_track { self.scene.get(n.id, P_SCALE).unwrap_or(n.static_scale) } else { n.static_scale };
            let own_sc = p.transform.scale.x * press_k * static_k
                * sequence.and_then(|l| l.pose(lnow, self.reduced_motion)).map_or(1.0, |p| p.scale);
            // An `El::scale` or `El::live_scale` an app drives from a slider, wheel or pinch
            // counts as moving until it holds still, then rasterizes crisp at its resting scale.
            let has_live_scale = n.live.iter().any(|b| b.property == crate::live::Property::Scale);
            let tracked_scale = if has_live_scale { static_k * p.transform.scale.x } else { static_k };
            let scale_settling = if tracked_scale != 1.0 || has_live_scale || self.static_scales.contains_key(&n.id) {
                let t = self.t;
                let seen = self.static_scales.entry(n.id).or_insert((tracked_scale, f64::NEG_INFINITY, false));
                if self.settled {
                    *seen = (tracked_scale, f64::NEG_INFINITY, seen.2);
                } else if (seen.0 - tracked_scale).abs() > 1e-5 {
                    *seen = (tracked_scale, t, true);
                }
                let moving = t - seen.1 < STATIC_SCALE_SETTLE;
                if !has_live_scale {
                    self.static_scale_settling |= moving;
                }
                moving
            } else { false };
            let store = self.scene.store();
            let scale_animating = store.is_animating(n.id, prop::SCALE) || store.is_animating(n.id, prop::PRESENCE)
                || (has_press_scale && store.is_animating(n.id, P_PRESS_SCALE))
                || (n.scale_track && store.is_animating(n.id, P_SCALE))
                || sequence.is_some_and(|l| !self.reduced_motion && l.active(lnow));
            let scale_motion = pinh.scale_motion.max(
                if !has_live_scale && (scale_animating || scale_settling) {
                    ScaleMotion::Moving
                } else if has_live_scale && (scale_settling || scale_animating) {
                    ScaleMotion::Live
                } else {
                    ScaleMotion::Still
                });
            // Scale about this node's centre where its ancestors' scales put it, so a scaled child
            // of a scaled parent stays in place.
            let sc = pinh.scale * own_sc;
            let pivot = match n.vis.origin {
                Some([ox, oy]) => [p.rect.x + ox * p.rect.w, p.rect.y + oy * p.rect.h],
                None => [p.anchor.x, p.anchor.y],
            };
            let centre = [pivot[0] + pinh.dx, pivot[1] + pinh.dy];
            let centre_to = [pinh.anchor_to[0] + (centre[0] - pinh.anchor[0]) * pinh.scale,
                pinh.anchor_to[1] + (centre[1] - pinh.anchor[1]) * pinh.scale];
            let (anchor, anchor_to) = if (own_sc - 1.0).abs() > 1e-5 { (centre, centre_to) } else { (pinh.anchor, pinh.anchor_to) };
            // A pressed element still hits at its resting size, so a press near its edge stays inside.
            let press_anchor = centre_to;
            let blur = pinh.blur + p.blur * sc;
            // An effect slot's bleed can reach into view from a rect outside it.
            let filter_margin = blur * s * 3.0 + n.vis.effect.map_or(0.0, |e| e.bleed * sc * s);

            let mut raw_edges = p.rect.edges();
            raw_edges[0] += pinh.dx;
            raw_edges[2] += pinh.dx;
            raw_edges[1] += pinh.dy;
            raw_edges[3] += pinh.dy;
            if self.row_h[i] >= 0.0 {
                raw_edges[3] = raw_edges[1] + self.row_h[i];
            }
            if self.has_sticky {
                let up = n.parent.map_or(sticky::UNBOUNDED, |pr| self.sticky_frame[pr]);
                // Content scrolls without end on a viewport's scrolling axes; non-scrolling axes
                // bound sticky children to this node's content box.
                let sc_y = n.scroll.is_some() && n.scroll_axes.y();
                let sc_x = n.scroll.is_some() && n.scroll_axes.x();
                let v = if sc_y {
                    [raw_edges[1], raw_edges[3], f32::NEG_INFINITY, f32::INFINITY]
                } else {
                    [up[0], up[1], raw_edges[1] + n.inset[1], raw_edges[3] - n.inset[3]]
                };
                let x = if sc_x {
                    [raw_edges[0], raw_edges[2], f32::NEG_INFINITY, f32::INFINITY]
                } else {
                    [up[4], up[5], raw_edges[0] + n.inset[0], raw_edges[2] - n.inset[2]]
                };
                self.sticky_frame[i] = [v[0], v[1], v[2], v[3], x[0], x[1], x[2], x[3]];
            }
            // A product of 1 (0.5 inside 2.0) can still move the box.
            let mut rect_pt = if (sc - 1.0).abs() > 1e-5 || anchor != anchor_to {
                [
                    anchor_to[0] + (raw_edges[0] - anchor[0]) * sc,
                    anchor_to[1] + (raw_edges[1] - anchor[1]) * sc,
                    anchor_to[0] + (raw_edges[2] - anchor[0]) * sc,
                    anchor_to[1] + (raw_edges[3] - anchor[1]) * sc,
                ]
            } else {
                raw_edges
            };
            // Land a sub-quantum size residual before snapping both edges. Otherwise a
            // translating/scaling box can alternate between neighbouring widths at rest.
            for (axis, target) in [n.local.w, n.local.h].into_iter().enumerate() {
                let target = grid(target, s);
                if ((rect_pt[axis + 2] - rect_pt[axis]) - target).abs() * s < 1.0 / 32.0 {
                    rect_pt[axis + 2] = rect_pt[axis] + target;
                }
            }
            // A matched flight scales its laid-out content uniformly to cover the moving rect,
            // centred: `k` and the centring offset in points. A launch flies at its own size.
            let cover = if lift && n.local.w > 0.0 && n.local.h > 0.0 && !self.launches.launched(node_id) {
                let (pw, ph) = (rect_pt[2] - rect_pt[0], rect_pt[3] - rect_pt[1]);
                if n.vis.cold.get().is_some_and(|c| c.flight_crop) {
                    // `El::matched_crop`: at its own size, from the leading edge.
                    Some((1.0, [0.0, (ph - n.local.h) * 0.5]))
                } else {
                    let k = (pw / n.local.w).max(ph / n.local.h).max(1e-3);
                    Some((k, [(pw - n.local.w * k) * 0.5, (ph - n.local.h * k) * 0.5]))
                }
            } else { None };
            let r_exact = rect_pt.map(snap_px);
            // A node the visibility test below culls paints nothing, and any clip it hands down is
            // off screen too, so it keeps its exact edges.
            let rigid = pinh.snap.rigid.map(|v| v * s);
            let can_snap = !pinh.snap.off && !n.vis_flags.no_pixel_snap() && p.transform.rotate == 0.0;
            let snaps = can_snap && (pinh.in_cache || {
                let m = filter_margin + 2.0;
                r_exact[2].max(r_exact[0]) >= clip[0].max(vp[0]) - m && r_exact[0] <= clip[2].min(vp[2]) + m
                    && r_exact[3].max(r_exact[1]) >= clip[1].max(vp[1]) - m && r_exact[1] <= clip[3].min(vp[3]) + m
            });
            let unscaled = (sc - 1.0).abs() <= 1e-5 && anchor == anchor_to;
            let animating = p.flags & (rune_motion::flags::ANIMATED | rune_motion::flags::ANCESTOR_ANIMATED) != 0;
            // A node with its own scale, or an anchored overlay inheriting its anchor's zoom after
            // `anchored_inh` cleared `snap.layer`, heads a scaled snap layer so its subtree snaps
            // onto whole pixels once still.
            let opens_snap_layer = (own_sc - 1.0).abs() > 1e-5 || (n.above.is_some() && !unscaled);
            let snap_layer = if can_snap && opens_snap_layer {
                self.pixel_snap.layer(n.id, (r_exact, raw_edges.map(snap_px)), rigid, pinh.in_cache || n.vis_flags.cache())
            } else {
                pinh.snap.layer
            };
            let rest = if snaps && animating { self.scene.rest(n.id) } else { None };
            let gliding = snaps && (rest.is_some() || !unscaled);
            let (r, snap_rest) = if !snaps {
                (r_exact, None)
            } else if rest.is_none() && unscaled {
                self.pixel_snap.at_rest(n.id, r_exact, rigid, pinh.snap.parent)
            } else {
                let motion = pixel_snap::motion(&p, rest, s, n.above.is_some(), self.row_h[i] >= 0.0);
                let raw = if unscaled { r_exact } else { raw_edges.map(snap_px) };
                self.pixel_snap.snapped(n.id, motion, (r_exact, raw), rigid, (snap_layer, sc), pinh.snap.parent)
            };
            let (w, h) = ((r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0));
            // `El::rotate`: this subtree draws into a layer turned about its origin.
            // The layer takes the clips from above; the node's own clips apply inside, upright.
            let own_turn = p.transform.rotate;
            let own_turn = (own_turn != 0.0 && own_turn.is_finite() && own_turn.abs() > 1e-6 && !lift).then_some(own_turn);
            let turn_pivot = [centre_to[0] * s, centre_to[1] * s];
            let (node_turn, turn_index) = if pinh.turn == 0 && own_turn.is_none() {
                (None, 0)
            } else {
                let mut outer_clip = intersect(pinh.clip, clip);
                if let Some((mask, _)) = rounded { outer_clip = intersect(outer_clip, mask); }
                compose_turn(&mut self.turn_scratch, pinh.turn, own_turn, ([r[0], r[1], w, h], turn_pivot, outer_clip))
            };
            if let Some(angle) = own_turn {
                if !pinh.skip_paint {
                    open_turned_layer(&mut self.instances, &mut layers, (i, self.surfaces.len()), [r[0], r[1], w, h],
                        (angle, turn_pivot), (&mut clip, &mut fade, &mut fade_origin, &mut rounded));
                } else {
                    (clip, fade, fade_origin, rounded) = (NO_CLIP, [0.0; 4], NO_CLIP, None);
                }
                pinh.clip = NO_CLIP;
                pinh.fade = [0.0; 4];
                pinh.fade_origin = NO_CLIP;
            }
            // Where the node shows on screen, upright: the turned box's bounds.
            let mut screen_r = [r_exact[0], r_exact[1], r_exact[2].max(r_exact[0]), r_exact[3].max(r_exact[1])];
            if let Some(t) = node_turn { screen_r = t.bbox(screen_r); }
            self.drawn[i] = [screen_r[0] / s, screen_r[1] / s, (screen_r[2] - screen_r[0]) / s, (screen_r[3] - screen_r[1]) / s];
            if cover.is_none() { self.drawn_scale[i] = sc; }
            let radii = p.radius.0.map(|v| v * sc * s);

            if !p.mask.is_none() {
                let ins = p.mask.inset.0.map(|v| v * sc * s);
                let fea = p.mask.feather.0.map(|v| v * sc * s);
                clip = [
                    (r[0] + ins[0]).max(clip[0]),
                    (r[1] + ins[1]).max(clip[1]),
                    (r[2] - ins[2]).min(clip[2]),
                    (r[3] - ins[3]).min(clip[3]),
                ];
                inherit_fade(&mut fade, &mut fade_origin, [r[0] + ins[0], r[1] + ins[1], r[2] - ins[2], r[3] - ins[3]], fea);
            }
            let m = filter_margin + 2.0;
            let visible = match node_turn {
                None => {
                    let vis_clip = if pinh.in_cache { intersect(pinh.clip, clip) } else { clip };
                    // Under a clip with no area nothing draws, however near its edge.
                    vis_clip[2] > vis_clip[0] && vis_clip[3] > vis_clip[1]
                        && screen_r[2] >= vis_clip[0].max(vp[0]) - m
                        && screen_r[0] <= vis_clip[2].min(vp[2]) + m
                        && screen_r[3] >= vis_clip[1].max(vp[1]) - m
                        && screen_r[1] <= vis_clip[3].min(vp[3]) + m
                }
                Some(t) => {
                    let lc = intersect(pinh.clip, clip);
                    let oc = t.outer_clip;
                    r[2] >= lc[0] - m && r[0] <= lc[2] + m && r[3] >= lc[1] - m && r[1] <= lc[3] + m
                        && screen_r[2] >= oc[0].max(vp[0]) - m
                        && screen_r[0] <= oc[2].min(vp[2]) + m
                        && screen_r[3] >= oc[1].max(vp[1]) - m
                        && screen_r[1] <= oc[3].min(vp[3]) + m
                }
            };
            self.seen[i] = visible;

            // A slide may sweep wholly outside its clip; judge visibility at its resting place.
            let home = match n.looping.as_ref().map(|l| l.kind) {
                Some(rune_motion::LoopKind::Slide { .. } | rune_motion::LoopKind::SlideEased { .. }) => {
                    let o = self.scene.get(n.id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
                    [r[0] - o.x * s, r[1] - o.y * s, r[2] - o.x * s, r[3] - o.y * s]
                }
                _ => r,
            };
            // A loop at its brightest under one 8-bit step can't be seen: let it sleep.
            let loop_vis = match node_turn {
                None => home[2] > clip[0].max(vp[0] - filter_margin) && home[0] < clip[2].min(vp[2] + filter_margin)
                    && home[3] > clip[1].max(vp[1] - filter_margin) && home[1] < clip[3].min(vp[3] + filter_margin),
                Some(t) => {
                    let lc = intersect(pinh.clip, clip);
                    let sh = t.bbox(home);
                    let oc = t.outer_clip;
                    home[2] > lc[0] - filter_margin && home[0] < lc[2] + filter_margin
                        && home[3] > lc[1] - filter_margin && home[1] < lc[3] + filter_margin
                        && sh[2] > oc[0].max(vp[0] - filter_margin) && sh[0] < oc[2].min(vp[2] + filter_margin)
                        && sh[3] > oc[1].max(vp[1] - filter_margin) && sh[1] < oc[3].min(vp[3] + filter_margin)
                }
            };
            if self.surface_visible && base_opacity >= 1.0 / 255.0 && !self.reduced_motion
                && loop_vis
                && let Some(track) = self.loops.get(&n.id)
            {
                let node_k = self.scene.store().node_time_scale(n.id) as f64;
                if let Some(dt) = track.frame_interval().filter(|_| track.active(lnow)) {
                    ambient = Some(ambient.map_or(dt as f32, |a| a.min(dt as f32)));
                    let wake = self.clock + dt;
                    if self.loop_wake.is_none_or(|w| wake < w.0) {
                        self.loop_wake = Some((wake, now + dt * self.time_scale as f64));
                    }
                } else if self.time_scale > 0.0 && node_k > 0.0 {
                    self.loops_active |= track.active(lnow);
                }
                if self.time_scale > 0.0 && node_k > 0.0 && let Some(start) = track.pending_start(lnow) {
                    let window_start = now + (start - lnow) / node_k;
                    let wake = self.clock + (window_start - now) / self.time_scale as f64;
                    if self.loop_wake.is_none_or(|w| wake < w.0) { self.loop_wake = Some((wake, window_start)); }
                }
            }
            if n.vis_flags.cache() && self.cache_live_scratch[i] {
                if let Some(entry) = self.node_caches.get_mut(&n.id) {
                    entry.was_live = true;
                }
            }
            let mut skip_paint = pinh.skip_paint;
            let mut in_cache = pinh.in_cache;
            let mut open_cache = false;
            // A scale changed by view commits may accompany arbitrary content edits. Draw
            // live until it rests instead of trusting a partial content hash or allocating a
            // new raster on each commit. Live-only transforms still composite the old raster.
            let mut committed_scale_moving = false;
            if n.vis_flags.cache() && let Some(entry) = self.node_caches.get_mut(&n.id)
                && entry.commit_gen != self.commit_gen
                && (sc * s - entry.raster_scale).abs() > 1e-4
            {
                if (sc * s - entry.seen_scale).abs() > 1e-4 {
                    entry.seen_scale = sc * s;
                    entry.scale_changed_at = now;
                }
                committed_scale_moving = now - entry.scale_changed_at < STATIC_SCALE_SETTLE;
                if committed_scale_moving {
                    entry.was_live = true;
                    self.static_scale_settling = true;
                }
            }
            if n.vis_flags.cache() && active_cache.is_none() && !lift && !skip_paint && !self.cache_live_scratch[i]
                && !committed_scale_moving && own_turn.is_none()
                && effective_opacity > 0.001 && w > 0.0 && h > 0.0
            {
                let eff_scale = sc * s;
                let est_bounds = if let Some(entry) = self.node_caches.get(&n.id) {
                    let k = eff_scale / entry.raster_scale.max(1e-4);
                    [
                        r[0] + entry.rel_bounds[0] * k,
                        r[1] + entry.rel_bounds[1] * k,
                        r[0] + entry.rel_bounds[2] * k,
                        r[1] + entry.rel_bounds[3] * k,
                    ]
                } else {
                    let sh = n.vis.shadows.iter().enumerate().fold(0.0f32, |m, (k, sh)| {
                        let sh = self.shadow_at(n.id, k, sh);
                        m.max((sh.x.abs() + sh.y.abs() + sh.spread.max(0.0) + sh.blur * 3.0) * eff_scale)
                    });
                    [r[0] - sh, r[1] - sh, r[2] + sh, r[3] + sh]
                };
                let cache_vis = match node_turn {
                    None => est_bounds[2] >= clip[0].max(vp[0]) - m
                        && est_bounds[0] <= clip[2].min(vp[2]) + m
                        && est_bounds[3] >= clip[1].max(vp[1]) - m
                        && est_bounds[1] <= clip[3].min(vp[3]) + m,
                    Some(t) => {
                        let lc = intersect(pinh.clip, clip);
                        let sb = t.bbox(est_bounds);
                        let oc = t.outer_clip;
                        est_bounds[2] >= lc[0] - m && est_bounds[0] <= lc[2] + m
                            && est_bounds[3] >= lc[1] - m && est_bounds[1] <= lc[3] + m
                            && sb[2] >= oc[0].max(vp[0]) - m && sb[0] <= oc[2].min(vp[2]) + m
                            && sb[3] >= oc[1].max(vp[1]) - m && sb[1] <= oc[3].min(vp[3]) + m
                    }
                };
                if !cache_vis {
                    skip_paint = true;
                } else {
                    let est_w = (est_bounds[2] - est_bounds[0]).ceil();
                    let est_h = (est_bounds[3] - est_bounds[1]).ceil();
                    if est_w <= 4096.0 && est_h <= 4096.0 {
                        open_cache = true;
                    }
                }
            }
            // A negative clip radius rounds with continuous corners (`render::corner_extent`).
            let mut own_round = n.vis.clip_corners.or_else(|| {
                n.vis.clip_radius.map(|radius| {
                    if n.vis.radii() != [0.0; 4] {
                        p.radius.0
                    } else {
                        [radius; 4]
                    }
                })
            }).map(|radii| {
                let half = w.min(h) * 0.5;
                let radii = radii.map(|radius| {
                    let r = (radius.max(0.0) * sc * s).min(half);
                    if n.vis_flags.continuous_corners() && r > 0.0 { -r } else { r }
                });
                (r, radii)
            });
            let mut hit_clip = intersect(pinh.clip, clip);
            if let Some((rect, _)) = own_round { hit_clip = intersect(hit_clip, rect); }
            let (mut hit_fade, mut hit_origin) = (pinh.fade, pinh.fade_origin);
            inherit_fade(&mut hit_fade, &mut hit_origin, fade_origin, fade);
            // A rectangle wholly in either central strip misses all four corner squares.
            // Its hard edges already exclude everything the rounded mask would remove.
            for (idx, round) in [&mut rounded, &mut own_round].into_iter().enumerate() {
                if open_cache && idx == 1 { continue; }
                if let Some((mask, radii)) = *round {
                    let e = radii.map(crate::render::corner_extent);
                    let (left, right, top, bottom) = (e[0].max(e[3]), e[1].max(e[2]), e[0].max(e[1]), e[2].max(e[3]));
                    let cut = intersect(clip, mask);
                    if cut[2] <= cut[0] || cut[3] <= cut[1]
                        || (cut[0] >= mask[0] + left && cut[2] <= mask[2] - right)
                        || (cut[1] >= mask[1] + top && cut[3] <= mask[3] - bottom) {
                        clip = cut;
                        *round = None;
                    }
                }
            }
            // A backdrop blur samples what is drawn before it, so it goes ahead of any layer this
            // element opens (a fade still blurs the window) and of its shadows, which skip it.
            let blur_hole = (n.vis.backdrop_blur > 0.0 && !skip_paint && visible && backdrop_blurs < crate::MAX_BACKDROP_BLURS
                && effective_opacity > 0.001 && paint_own * pinh.paint_opacity * n.vis.paint_alpha > 0.001)
                .then(|| paint_slots::push_backdrop_blur(&mut self.instances, n.vis.backdrop_blur, ([r[0], r[1], w, h], radii, n.vis_flags.continuous_corners()),
                    (clip, fade, fade_origin), rounded, sc * s, (paint_own * pinh.paint_opacity * n.vis.paint_alpha).clamp(0.0, 1.0)));
            backdrop_blurs += blur_hole.is_some() as usize;
            // The packed ABI carries one clip rectangle. Preserve a rectangular cut through
            // a rounded corner on a separate outer scope, never recenter the rounded mask.
            if !skip_paint && let Some((mask, _)) = rounded.or(own_round)
                && intersect(clip, mask) != mask {
                let mut marker = Instance::begin_layer([r[0], r[1], w, h], clip, Default::default());
                marker.fade = crate::render::pack_fade(fade, fade_origin, clip);
                layers.push((i, self.instances.len(), self.surfaces.len()));
                self.instances.push(marker);
                clip = NO_CLIP; fade = [0.0; 4]; fade_origin = NO_CLIP;
            }
            // Two rounded rectangles cannot generally be represented by one rounded rectangle.
            // Consume the ancestor on an outer composite, retaining both original corner centres.
            if !skip_paint && rounded.is_some() && own_round.is_some() && rounded != own_round {
                let (mask, radii) = rounded.take().unwrap();
                let mut marker = Instance::begin_layer([r[0], r[1], w, h], mask, Default::default());
                marker.fade = crate::render::pack_fade(fade, fade_origin, mask);
                marker.round_clip_corners(radii);
                layers.push((i, self.instances.len(), self.surfaces.len()));
                self.instances.push(marker);
                clip = NO_CLIP; fade = [0.0; 4]; fade_origin = NO_CLIP;
            }
            if let Some(mask) = own_round { rounded = Some(mask); }
            if let Some((mask, _)) = rounded {
                clip = intersect(clip, mask);
            }
            // `El::cutout_circle`: the subtree draws in a layer whose composite cuts the hole,
            // after the clips from above, which it takes.
            if !skip_paint && let Some(c) = n.vis.cutout {
                let k = sc * s;
                let mut marker = Instance::begin_layer([r[0], r[1], w, h], clip, Default::default());
                marker.fade = crate::render::pack_fade(fade, fade_origin, clip);
                if let Some((_, radii)) = rounded { marker.round_clip_corners(radii); }
                marker.radii = [r[0] + c[0] * k, r[1] + c[1] * k, c[2] * k, c[3] * k];
                layers.push((i, self.instances.len(), self.surfaces.len()));
                self.instances.push(marker);
                clip = NO_CLIP; fade = [0.0; 4]; fade_origin = NO_CLIP; rounded = None;
            }
            // Live/truncated text adds a glyph-only rectangle below. Consume the inherited
            // rounded mask first so the text clip cannot move its corner centres, unless that
            // rectangle sits clear of the corners and the glyphs can skip the mask.
            let glyphs_clear = rounded.is_some_and(|(mask, radii)| {
                self.text_clip(i, (r, r_exact), sc, clip).is_some_and(|(text, ..)| inside_rounded(text, mask, radii))
            });
            isolate |= open_cache || (rounded.is_some() && (n.text.as_ref().is_some_and(|t| (t.truncate_fade || t.live.is_some()) && !glyphs_clear)
                || n.input.as_ref().is_some_and(|i| i.single_line)));
            // A soft clip fades each primitive on its own, so where one paints over another (an
            // image over its placeholder fill, a label over its bubble) the one beneath shows
            // through the fade, and the stack brightens or darkens towards the edge. A stack that
            // reaches into a soft edge composites as one layer, which takes the fade once.
            if !isolate && visible && effective_opacity > 0.001 && fade.iter().any(|&f| f != 0.0)
                && in_fade([r[0], r[1], r[0] + w, r[1] + h], clip, fade, fade_origin)
            {
                let paints = own_paints(n);
                let parent = next.is_some_and(|j| self.nodes[j].parent == Some(i));
                isolate = paints >= 2 || (paints == 1 && parent);
            }
            let o = if isolate { 1.0 } else { (paint_own * pinh.paint_opacity).clamp(0.0, 1.0) };
            if !skip_paint && isolate {
                let start_idx = self.instances.len();
                layers.push((i, start_idx, self.surfaces.len()));
                let mut marker = Instance::begin_layer([r[0], r[1], w, h], clip,
                    crate::render::LayerSpec { opacity: paint_own * pinh.paint_opacity,
                        blend: if dissolve { crate::render::Blend::PlusLighter } else { crate::render::Blend::SourceOver },
                        blur: p.blur * sc * s, clip_radius: 0.0 });
                marker.fade = crate::render::pack_fade(fade, fade_origin, clip);
                if let Some((_, radii)) = rounded { marker.round_clip_corners(radii); }
                self.instances.push(marker);
                // Clip the completed layer, never its inputs (including off-wall blur taps).
                clip = NO_CLIP; fade = [0.0; 4]; fade_origin = NO_CLIP; rounded = None;
                if open_cache {
                    in_cache = true;
                    let eff_scale = sc * s;
                    let paint_hash = {
                        use std::hash::Hasher;
                        let mut h = rune_motion::Fx::default();
                        h.write_u64(paint_state_hash);
                        h.write_u64(self.cache_paint_scratch[i]);
                        h.write_u32(pinh.group.to_bits());
                        h.finish()
                    };
                    let (raster_scale, seen_scale, scale_changed_at, elided, cached_glyphs) =
                        if let Some(entry) = self.node_caches.get(&n.id) {
                            let is_resident = self.text.atlas.resident_caches.get(&n.id.0) == Some(&entry.signature);
                            let env_same = (s - entry.env_scale).abs() <= 1e-4 && !self.settled;
                            let (seen_scale, scale_changed_at) = if !env_same {
                                (eff_scale, f64::NEG_INFINITY)
                            } else if (eff_scale - entry.seen_scale).abs() > 1e-4 {
                                (eff_scale, now)
                            } else {
                                (entry.seen_scale, entry.scale_changed_at)
                            };
                            let scale_diff = (eff_scale - entry.raster_scale).abs() > 1e-4;
                            let same_commit = self.commit_gen == entry.commit_gen;
                            let scale_settling = same_commit && scale_diff && env_same && is_resident && !entry.was_live
                                && (now - scale_changed_at < STATIC_SCALE_SETTLE || scale_motion == ScaleMotion::Moving);
                            self.static_scale_settling |= scale_settling;
                            // A commit can change any paint property. Never use a partial
                            // model hash to skip checking the actual emitted draw output.
                            let content_same = same_commit && (!scale_diff || scale_settling);
                            let can_elide = is_resident
                                && !entry.was_live
                                && content_same
                                && paint_hash == entry.paint_state_hash
                                && !self.cache_check_scratch[i];
                            let raster_scale = if can_elide && scale_settling { entry.raster_scale } else { eff_scale };
                            (raster_scale, seen_scale, scale_changed_at, can_elide, entry.glyphs)
                        } else {
                            (eff_scale, eff_scale, f64::NEG_INFINITY, false, 0)
                        };
                    if elided {
                        skip_paint = true;
                    }
                    active_cache = Some(ActiveCache {
                        node_index: i,
                        node_id: n.id,
                        instance_start: start_idx,
                        origin: [r[0], r[1]],
                        size: [w, h],
                        cur_scale: eff_scale,
                        raster_scale,
                        seen_scale,
                        scale_changed_at,
                        env_scale: s,
                        paint_hash,
                        glyph_start: glyph_count,
                        cached_glyphs,
                        elided,
                    });
                }
            }
            let primitive_start = self.instances.len();
            let surface_start = self.surfaces.len();
            // Include native surface geometry, which lives beside the primitive stream.
            if !skip_paint && n.vis.surface.is_some() {
                for &(_, at, _) in &layers {
                    let b = &mut self.instances[at].rect;
                    let hi = [(b[0] + b[2]).max(r[2].min(clip[2])), (b[1] + b[3]).max(r[3].min(clip[3]))];
                    b[0] = b[0].min(r[0].max(clip[0])); b[1] = b[1].min(r[1].max(clip[1]));
                    b[2] = hi[0] - b[0]; b[3] = hi[1] - b[1];
                }
            }

            let first_instance = self.instances.len();
            let mut faded_slot = None;
            // Native input readiness follows focus, not entrance opacity. An input
            // mounting at opacity zero still needs its IME area in this frame.
            let input_caret = n.input.as_ref().filter(|input| input.edit.focused && self.can_focus(i))
                .filter(|_| !self.flights.iter().any(|f| n.key.as_deref() == Some(&f.source_key)
                    && f.alphas(f.destination, now, self.reduced_motion).0 > 0.0))
                .and_then(|_| input_caret(n, r, sc, s, gliding, &mut self.input_scroll));
            if let Some((_, area)) = input_caret
                && n.input.as_ref().is_some_and(|input| !input.edit.is_secure()) { self.ime_area = Some(area); }
            else if n.ime_cursor && self.ime_area.is_none() && self.allowed(i) && !self.subtree_exiting(i) { self.ime_area = Some(app_ime::area(r, s)); }
            let paint_visible = if in_cache {
                r[2] >= clip[0] - filter_margin - 2.0
                    && r[0] <= clip[2] + filter_margin + 2.0
                    && r[3] >= clip[1] - filter_margin - 2.0
                    && r[1] <= clip[3] + filter_margin + 2.0
            } else {
                visible
            };
            let painted = !skip_paint && effective_opacity > 0.001 && o > 0.001 && paint_visible;
            // Unpulled out of sight, a video still counts, so a scroll across a clip edge doesn't commit.
            if !painted && let Some((surface, _)) = &n.vis.surface { video_signature = crate::video::fold_unpulled(video_signature, surface); }
            if painted {
                // Disabled styling is distributed to each authored paint leaf at build time.
                // Keep this local to emission: it neither propagates to children nor opens scopes.
                let o = o * n.vis.paint_alpha;
                let srgb = self.srgb_blend;
                // Subtree blur runs once after rasterization, never on each primitive as well.
                let blur = 0.0;
                let k_px = sc * s;
                let native_material = match n.material {
                    // A dimmed-out element takes no native view: its fallback draws nothing.
                    Some(material) if self.materials.len() < crate::MAX_NATIVE_MATERIALS
                        && effective_opacity * n.vis.paint_alpha > 0.001 => {
                        let radius = crate::system_material::clamp_radius(radii, w, h);
                        let continuous = n.vis_flags.continuous_corners();
                        self.materials.push(crate::system_material::MaterialDraw {
                            id: n.id, material, rect: [r[0], r[1], w, h], radius, continuous,
                            opacity: effective_opacity * n.vis.paint_alpha,
                        });
                        self.native_materials.then_some(if continuous { -radius } else { radius })
                    }
                    _ => None,
                };
                let stuck_k = if self.has_sticky { (self.stuck[i].abs() / sticky::SHADOW_RAMP).min(1.0) } else { 0.0 };
                let stuck_shadows: (f32, &[crate::el::Shadow]) = match &n.sticky {
                    Some(s) if stuck_k > 0.0 => (stuck_k, &s.shadows),
                    _ => (0.0, &[]),
                };
                // `El::focus_within_flat`: focus inside drops its own shadows and hover fill.
                let focus_flat = n.focus.flat_within && self.focus_within(i);
                let solid = !n.vis.shadows.is_empty() && native_material.is_none() && blur_hole.is_none() && n.material.is_none()
                    && self.color(n.id, P_FILL, n.vis.fill).a >= 1.0;
                // A box that paints itself solid hides the middle of its shadows (bands.rs). Dimmed
                // (`El::dim`), it leaves that middle clear, as CSS clips box-shadow to outside the box.
                let opaque = solid && o >= 1.0 && n.vis.gradient.is_none() && n.vis.radial.is_none() && n.vis.surface.is_none();
                let dim_hole = (solid && o < 1.0).then(|| crate::system_material::clamp_radius(radii, w, h) * if n.vis_flags.continuous_corners() { -1.0 } else { 1.0 });
                let cast = shadow_paint::Caster {
                    rect: [r[0], r[1], w, h], radii, shape: if n.vis_flags.continuous_corners() { radii.map(|v| -v) } else { radii },
                    k_px, scale: s, opacity: o, srgb, clip, fade, hole: native_material.or(blur_hole).or(dim_hole), opaque,
                };
                for (k, sh) in n.vis.shadows.iter().chain(stuck_shadows.1).enumerate() {
                    let c = if k >= n.vis.shadows.len() { sh.color.alpha(sh.color.a * stuck_shadows.0) }
                        else if k < P_SHADOWS.len() { self.color(n.id, P_SHADOWS[k], sh.color) } else { sh.color };
                    if c.a <= 0.0 || sh.inset || (focus_flat && k < n.vis.shadows.len()) {
                        continue;
                    }
                    let sh = &if k < n.vis.shadows.len() { self.shadow_at(n.id, k, sh) } else { *sh };
                    shadow_paint::push_shadow(&mut self.instances, sh, c, &cast);
                }
                // Shadows keep circular corners; the element's own shapes take the sign, and
                // negative radii draw continuous corners (shader.wgsl `sd_rounded`).
                let radii = if n.vis_flags.continuous_corners() { radii.map(|v| -v) } else { radii };
                let mut overlay = [0.0f32; 4];
                if let Some(c) = n.focus.highlight && self.highlight_visible(i) {
                    overlay = over(premul(c, 1.0, srgb), overlay);
                }
                if let Some(c) = n.focus.fill && self.focus_ring_visible(i) {
                    overlay = over(premul(c, 1.0, srgb), overlay);
                }
                if !focus_flat && (n.vis.hover().a > 0.0 || self.scene.store().is_animating(n.id, P_HOVER_C)) {
                    let hov_v = self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0);
                    if hov_v > 0.0 {
                        overlay = over(premul(self.color(n.id, P_HOVER_C, n.vis.hover()), hov_v, srgb), overlay);
                    }
                }
                if n.vis.press().a > 0.0 || self.scene.store().is_animating(n.id, P_PRESS_C) {
                    let prs_v = self.scene.get(n.id, P_PRESS).unwrap_or(0.0).clamp(0.0, 1.0);
                    if prs_v > 0.0 {
                        overlay = over(premul(self.color(n.id, P_PRESS_C, n.vis.press()), prs_v, srgb), overlay);
                    }
                }
                let base = match n.material {
                    _ if native_material.is_some() => [0.0; 4],
                    Some(_) => premul(self.color(n.id, P_FILL, crate::system_material::fallback_fill(n.vis.fill, self.env.system_dark)), 1.0, srgb),
                    None => premul(self.color(n.id, P_FILL, n.vis.fill), 1.0, srgb),
                };
                let layered = n.vis.gradient.is_some() || n.vis.radial.is_some() || n.vis.surface.is_some();
                let fill = if layered { base } else { over(overlay, base) }.map(|c| c * o);
                let bw = pixel_snap::line_width(n.vis.border_width() * s) * sc;
                let border = if bw > 0.0 { premul(self.color(n.id, P_BORDER, n.vis.border()), o, srgb) } else { [0.0; 4] };
                // A gradient border draws as KIND_GRADIENT with a width: `border` top, `uv` bottom.
                // After a switch back to `border`, it stays a gradient until both stops meet.
                let store = self.scene.store();
                let springing = store.has(n.id, P_BORDER_BOTTOM)
                    && (store.is_animating(n.id, P_BORDER_BOTTOM) || store.is_animating(n.id, P_BORDER));
                let border_bottom = (bw > 0.0 && (n.vis.border_bottom.is_some() || springing))
                    .then(|| premul(self.color(n.id, P_BORDER_BOTTOM, n.vis.border_bottom.unwrap_or(n.vis.border())), o, srgb))
                    .filter(|b| n.vis.border_bottom.is_some() || *b != border);
                let (border_kind, border_uv) = border_bottom.map_or((KIND_RECT, [0.0; 4]), |b| (KIND_GRADIENT, b));
                let border_a = border[3].max(border_uv[3]);
                // A dashed ring is its own instance over the fill, which then draws without a ring.
                let dashed = n.vis.border_dash.filter(|_| bw > 0.0 && border_a > 0.0005).map(|[dash, gap]| Instance {
                    rect: [r[0], r[1], w, h],
                    radii,
                    fill: border,
                    border: border_bottom.unwrap_or(border),
                    params: [bw, KIND_DASHED, blur * s, 1.0],
                    uv: [dash * k_px, gap * k_px, n.vis.border_dash_phase * k_px, 0.0],
                    clip,
                    fade: crate::render::pack_fade(fade, clip, clip),
                });
                // Per-side widths (`El::border_sides`), each snapped as `bw` is; the widest is `bw`.
                let sides = n.vis.border_sides.filter(|_| bw > 0.0).map(|ws| ws.map(|v| pixel_snap::line_width(v * s) * sc));
                let padding = sides.unwrap_or([bw; 4]);
                let (bw, border, border_kind, border_uv, border_a) =
                    if n.vis.border_dash.is_some() { (0.0, [0.0; 4], KIND_RECT, [0.0; 4], 0.0) } else { (bw, border, border_kind, border_uv, border_a) };
                // The border's own instance, `o` in `params.w` so the shader knows how much it
                // hides the fill under it (shader.wgsl `border_over`). With inset shadows it draws
                // apart, over them, as CSS layers an inset box-shadow between background and border.
                let ring = Instance {
                    rect: [r[0], r[1], w, h],
                    radii,
                    fill: [0.0; 4],
                    border,
                    params: [bw, border_kind, blur * s, o],
                    uv: if border_kind == KIND_RECT { sides.unwrap_or([0.0; 4]) } else { border_uv },
                    clip,
                    fade: crate::render::pack_fade(fade, clip, clip),
                };
                let apart = bw > 0.0 && border_a > 0.0005 && n.vis.shadows.iter().chain(stuck_shadows.1).any(|sh| sh.inset);
                // Dimmed, an opaque fill and its gradient draw as one opaque gradient, so they fade
                // together as a layer at that opacity would (CSS `opacity`), not one through the other.
                let flat = layered && o < 1.0 && base[3] >= 1.0 && n.vis.gradient.is_some();
                if layered {
                    // Fill, gradient and overlays first, then the border on top, as in CSS.
                    if fill[3] > 0.0005 && !flat {
                        self.instances.push(Instance {
                            rect: [r[0], r[1], w, h],
                            radii,
                            fill,
                            border: [0.0; 4],
                            params: [0.0, KIND_RECT, blur * s, 1.0],
                            uv: [0.0; 4],
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                    let g = n.vis.gradient.as_deref().copied().unwrap_or_default();
                    let stop = |c| if flat { over(premul(c, 1.0, srgb), base).map(|v| v * o) } else { premul(c, o, srgb) };
                    let top = stop(self.color(n.id, P_GRADIENT[0], g[0]));
                    let bottom = stop(self.color(n.id, P_GRADIENT[1], g[1]));
                    if top[3] > 0.0005 || bottom[3] > 0.0005 {
                        self.instances.push(Instance {
                            rect: [r[0], r[1], w, h],
                            radii,
                            fill: top,
                            border: [0.0; 4],
                            params: [0.0, KIND_GRADIENT, 0.0, 1.0],
                            uv: bottom,
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                    if let Some(rg) = &n.vis.radial {
                        let segs = rg.stops.len().saturating_sub(1);
                        let rmax = radii[0].max(radii[1]).max(radii[2]).max(radii[3]).min(w * 0.5).min(h * 0.5);
                        let pad = 1.0 + blur * s * 2.0;
                        for seg in 0..segs {
                            let c0 = rg.stops[seg].1;
                            let c0 = if seg < P_RADIAL.len() { self.color(n.id, P_RADIAL[seg], c0) } else { c0 };
                            let c1 = rg.stops[seg + 1].1;
                            let c1 = if seg + 1 < P_RADIAL.len() { self.color(n.id, P_RADIAL[seg + 1], c1) } else { c1 };
                            let c0 = premul(c0, o, srgb);
                            let c1 = premul(c1, o, srgb);
                            if c0[3] <= 0.0005 && c1[3] <= 0.0005 {
                                continue;
                            }
                            let t0_val = rg.stops[seg].0;
                            let t1_val = rg.stops[seg + 1].0;
                            let (seg_rect, seg_radii, uv) = match rg.geometry {
                                crate::el::GradientGeometry::Radial { center, radius } => {
                                    let cx = center[0] * w;
                                    let cy = center[1] * h;
                                    let rx = (radius[0] * k_px).max(1e-3);
                                    let ry = (radius[1] * k_px).max(1e-3);
                                    if seg + 1 < segs {
                                        let mut lx0 = (cx - t1_val * rx - pad).floor().max(0.0);
                                        let mut ly0 = (cy - t1_val * ry - pad).floor().max(0.0);
                                        let mut lx1 = (cx + t1_val * rx + pad).ceil().min(w);
                                        let mut ly1 = (cy + t1_val * ry + pad).ceil().min(h);
                                        if rmax > 0.0 {
                                            if lx0 < rmax && (ly0 < rmax || ly1 > h - rmax) { lx0 = 0.0; }
                                            if lx1 > w - rmax && (ly0 < rmax || ly1 > h - rmax) { lx1 = w; }
                                            if ly0 < rmax && (lx0 < rmax || lx1 > w - rmax) { ly0 = 0.0; }
                                            if ly1 > h - rmax && (lx0 < rmax || lx1 > w - rmax) { ly1 = h; }
                                            if lx0 == 0.0 && (ly0 == 0.0 || ly1 == h) { lx1 = lx1.max((2.0 * rmax).min(w)); }
                                            if lx1 == w && (ly0 == 0.0 || ly1 == h) { lx0 = lx0.min((w - 2.0 * rmax).max(0.0)); }
                                            if ly0 == 0.0 && (lx0 == 0.0 || lx1 == w) { ly1 = ly1.max((2.0 * rmax).min(h)); }
                                            if ly1 == h && (lx0 == 0.0 || lx1 == w) { ly0 = ly0.min((h - 2.0 * rmax).max(0.0)); }
                                        }
                                        if lx1 <= lx0 || ly1 <= ly0 {
                                            continue;
                                        }
                                        let br = [
                                            if lx0 == 0.0 && ly0 == 0.0 { radii[0] } else { 0.0 },
                                            if lx1 == w && ly0 == 0.0 { radii[1] } else { 0.0 },
                                            if lx1 == w && ly1 == h { radii[2] } else { 0.0 },
                                            if lx0 == 0.0 && ly1 == h { radii[3] } else { 0.0 },
                                        ];
                                        ([r[0] + lx0, r[1] + ly0, lx1 - lx0, ly1 - ly0], br, [cx - lx0, cy - ly0, rx, ry])
                                    } else {
                                        ([r[0], r[1], w, h], radii, [cx, cy, rx, ry])
                                    }
                                }
                                crate::el::GradientGeometry::Linear { angle } => {
                                    let (sin, cos) = angle.sin_cos();
                                    let (dx, dy) = (sin, -cos);
                                    let span = (dx.abs() * w + dy.abs() * h).max(1e-3);
                                    ([r[0], r[1], w, h], radii, [dx / span, dy / span, 0.0, 0.0])
                                }
                            };
                            let t0 = t0_val + if seg > 0 { 2.0 } else { 0.0 };
                            let t1 = t1_val + if seg + 1 < segs { 2.0 } else { 0.0 };
                            self.instances.push(Instance {
                                rect: seg_rect,
                                radii: seg_radii,
                                fill: c0,
                                border: c1,
                                params: [t0, KIND_RADIAL, blur * s, t1],
                                uv,
                                clip,
                                fade: crate::render::pack_fade(fade, clip, clip),
                            });
                        }
                    }
                    if let Some((surface, fit)) = &n.vis.surface {
                        if let Some(pulled) = surface.pull_video(video_clock, &self.live_wake) {
                            self.videos_active |= pulled.active;
                            video_signature = crate::video::fold(video_signature, pulled.generation, surface.key());
                        }
                        self.surfaces.push(crate::native_surface::SurfaceDraw {
                            surface: surface.placed(n.vis.surface_place), fit: *fit, before_instance: self.instances.len() as u32,
                            rect: [r[0], r[1], w, h], radii, clip, fade: crate::render::pack_fade(fade, fade_origin, clip), opacity: o,
                        });
                    }
                    let ov = overlay.map(|c| c * o);
                    if apart && ov[3] > 0.0005 {
                        self.instances.push(border_paint::fill_under(Instance { fill: ov, ..ring }, false));
                    } else if !apart && (ov[3] > 0.0005 || border_a > 0.0005) {
                        self.instances.push(Instance { fill: ov, ..ring });
                    }
                } else if apart {
                    if fill[3] > 0.0005 { self.instances.push(border_paint::fill_under(Instance { fill, ..ring }, true)); }
                } else if fill[3] > 0.0005 || border_a > 0.0005 {
                    if fill == [0.0; 4] && border_kind == KIND_RECT && bw > 0.0 && sides.is_none() {
                        bands::push_ring(&mut self.instances, ring);
                    } else {
                        self.instances.push(Instance { fill, ..ring });
                    }
                }
                // Inset shadows draw inside the border (CSS's padding box), under it.
                let (inner, inner_radii) = border_paint::inset_box([r[0], r[1], w, h], radii, padding);
                for (k, sh) in n.vis.shadows.iter().chain(stuck_shadows.1).enumerate() {
                    if !sh.inset || (focus_flat && k < n.vis.shadows.len()) {
                        continue;
                    }
                    let c = if k >= n.vis.shadows.len() { sh.color.alpha(sh.color.a * stuck_shadows.0) }
                        else if k < P_SHADOWS.len() { self.color(n.id, P_SHADOWS[k], sh.color) } else { sh.color };
                    if c.a <= 0.0 {
                        continue;
                    }
                    let sh = &if k < n.vis.shadows.len() { self.shadow_at(n.id, k, sh) } else { *sh };
                    self.instances.push(border_paint::inset_shadow(sh, premul(c, o, srgb), (inner, inner_radii), (k_px, s), clip, fade));
                }
                if apart {
                    if sides.is_none() && border_kind == KIND_RECT { bands::push_ring(&mut self.instances, ring); } else { self.instances.push(ring); }
                }
                self.instances.extend(dashed);
                if let Some(effect) = n.vis.effect {
                    let fade = crate::render::pack_fade(fade, clip, clip);
                    faded_slot = paint_slots::push_effect(&mut self.instances, (effect, mark), ([r[0], r[1], w, h], radii, clip, fade), k_px, o);
                }
                let focus_ring = if default_ring == Some(i) { Some(self.default_focus_ring()) }
                    else if n.focus.within { n.focus.ring.filter(|_| self.focus_within(i)) }
                    else { self.focus_ring_visible(i).then_some(n.focus.ring).flatten() };
                if let Some(ol) = focus_ring.or(n.vis.outline) {
                    let c = if focus_ring.is_some() { ol.color } else { self.color(n.id, P_OUTLINE, ol.color) };
                    let ow = pixel_snap::line_width(ol.width * s) * sc;
                    let g = pixel_snap::line_width(ol.offset * s) * sc + ow;
                    if c.a > 0.0 && ol.width > 0.0 {
                        self.instances.push(Instance {
                            rect: [r[0] - g, r[1] - g, w + 2.0 * g, h + 2.0 * g],
                            radii: radii.map(|v| if v != 0.0 { v + g.copysign(v) } else { 0.0 }),
                            fill: [0.0; 4],
                            border: premul(c, o, srgb),
                            params: [ow, KIND_RECT, 0.0, 1.0],
                            uv: [0.0; 4],
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                }
                self.paint_content(
                    i,
                    paint_content::LeafBox {
                        r,
                        r_exact,
                        raw_size: [raw_edges[2] - raw_edges[0], raw_edges[3] - raw_edges[1]],
                        radii,
                        sc,
                        o,
                        draw: p.draw,
                        clip,
                        fade,
                        group: pinh.group,
                        loop_value,
                        cover,
                        scale_motion,
                        rest: pinh.still.0,
                        gliding,
                        caret: input_caret.map(|(caret, _)| caret),
                    },
                    &sel_ranges,
                    &mut glyph_count,
                );
            }
            let packed_fade = crate::render::pack_fade(fade, fade_origin, clip);
            for instance in &mut self.instances[first_instance..] {
                instance.fade = packed_fade;
            }
            self.clip_text_instances(i, first_instance, (r, r_exact), sc, clip, fade, fade_origin);
            let n = &self.nodes[i];
            if let Some((_, radii)) = rounded {
                for instance in &mut self.instances[primitive_start..] {
                    if glyphs_clear && instance.params[1] == KIND_GLYPH { continue; }
                    instance.round_clip_corners(radii);
                }
                let packed = crate::render::pack_clip_radii(clip, radii);
                for surface in &mut self.surfaces[surface_start..] { surface.fade[3] = packed; }
            }
            // A faded effect slot's layer carries its clip; the slot and the layer's end don't.
            if let Some(at) = faded_slot {
                for instance in &mut self.instances[at..at + 2] { instance.fade = [0; 4]; }
            }
            if self.track_marks {
                self.marks.resize(self.instances.len(), mark);
            }
            // A scroll viewport clips its content with a hard edge unless it sets soft ones.
            if let Some(f) = n.vis.clip.or(n.scroll.map(|_| [0.0; 4])) {
                clip = intersect(r, clip);
                let (own, origin) = self.scroll_edge_fade(i, f, r, sc * s, p.rect.edges()[3]);
                inherit_fade(&mut fade, &mut fade_origin, origin, own);
            }
            let mask = self.row_mask[i];
            if mask >= 0.0 {
                clip[3] = clip[3].min(r[3]);
                // A fold's lifted contents (`GrowTrack::hide_above`) never draw above its top.
                if self.row_lifts.iter().any(|&(ri, _)| ri == i) { clip[1] = clip[1].max(r[1]); }
                inherit_fade(&mut fade, &mut fade_origin, r, [0.0, 0.0, 0.0, mask * s]);
            }
            let mut dy = pinh.dy;
            let snap_off = pinh.snap.off || n.vis_flags.no_pixel_snap() || turn_index != 0;
            let mut snap_rigid = pinh.snap.rigid;
            if n.scroll.is_some()
                && let Some(st) = self.scrolls.get(&n.id)
            {
                let offset = self.stuck_offset(i, p.rect.edges()[3]).unwrap_or(st.offset);
                let held = self.bottom_offset(i, p.rect.edges()[3]).is_some_and(|b| (offset - b).abs() < 0.5);
                let exact = offset as f32 + st.slack.height();
                // Hold a reversal across a rounding boundary, not a full pixel of real travel:
                // `grid` rounds to 1/64 of a device pixel. Always land at the layout offset.
                let rounded = grid(exact, s);
                let step = (rounded - st.drawn[1]).signum() as i8;
                let back = st.pinned() && rounded != grid(st.offset as f32, s) && step == -st.drawn_step && ((exact - st.drawn[1]) * s).abs() < 1.0 / 64.0;
                let drawn = if back { st.drawn[1] } else { rounded };
                let moved = if drawn == st.drawn[1] { st.drawn_step } else { step };
                dy -= drawn;
                snap_rigid[1] -= drawn;
                if let Some(st) = self.scrolls.get_mut(&self.nodes[i].id) { (st.held, st.drawn[1], st.drawn_step) = (held, drawn, moved); }
            }
            let (mut scale, mut anchor, mut anchor_to, mut dx) = (sc, anchor, anchor_to, pinh.dx);
            if n.scroll_axes.x() && let Some(st) = self.scrolls.get_mut(&self.nodes[i].id) {
                st.drawn[0] = grid(st.x.shift() as f32, s);
                dx -= st.drawn[0];
                snap_rigid[0] -= st.drawn[0];
            }
            if open_cache && sc > 1e-4 {
                dx += (r_exact[0] / s - rect_pt[0]) / sc;
                dy += (r_exact[1] / s - rect_pt[1]) / sc;
            }
            let (mut cover_offset, mut outer) = (pinh.cover, pinh.outer);
            let scale_motion = if cover.is_some() { ScaleMotion::Moving } else { scale_motion };
            if let Some((k, c)) = cover {
                // Children ride this node's presented origin at their rest layout. Scale them by
                // `k` about the drawn origin, centred, and clip them to the moving rect.
                clip = intersect(clip, r);
                hit_clip = intersect(hit_clip, r);
                let plain = [dx, dy];
                dx = rect_pt[0] - p.rect.x + c[0] / k;
                dy += rect_pt[1] - p.rect.y - pinh.dy + c[1] / k;
                (scale, anchor, anchor_to) = (k, [rect_pt[0], rect_pt[1]], [rect_pt[0], rect_pt[1]]);
                cover_offset = [dx - plain[0], dy - plain[1]];
                outer = Some([pinh.clip, pinh.fade, pinh.fade_origin]);
            }
            inherit_fade(&mut hit_fade, &mut hit_origin, fade_origin, fade);
            let full_clip = intersect(hit_clip, clip);
            if node_turn.is_some() {
                if self.unturned_drawn.is_empty() {
                    self.unturned_drawn.resize(n_nodes, ([0.0; 4], NO_CLIP));
                }
                self.unturned_drawn[i] = (r.map(|v| v / s), full_clip);
            }
            self.inh_scratch[i] = Inh { opacity: effective_opacity, paint_opacity: o, first_fade: kids_fade,
                clip: full_clip, fade: hit_fade, fade_origin: hit_origin,
                paint_clip: clip, paint_fade: fade, paint_fade_origin: fade_origin, rounded, scale, anchor, anchor_to, blur, dy, dx,
                cover: cover_offset, outer, mark, group,
                hit_opacity: hit_opacity * kids_fade, group_rect: if hover_owner { rect_pt } else { pinh.group_rect },
                scale_motion, still: if scale_motion == ScaleMotion::Still { (scale, anchor, anchor_to) } else { pinh.still },
                in_cache, skip_paint, turn: turn_index,
                snap: pixel_snap::Inherit { off: snap_off, rigid: snap_rigid, layer: if cover.is_some() { 0 } else { snap_layer }, parent: snap_rest.filter(|_| cover.is_none() && snap_rigid == pinh.snap.rigid && pixel_snap::frames(&n.vis)) } };
            // Shown only under its group's hover, it takes presses where the pointer hovers the
            // group: a press in the same frame as the move that reveals it finds it, as a page's
            // `:hover` applies before the press does.
            let (hit_opacity, hit_rect) = if effective_opacity > 0.001 { (effective_opacity, rect_pt) } else { (hit_opacity, intersect(rect_pt, pinh.group_rect)) };
            self.register_node_hits(i, pinh.clip, (hit_opacity, visible), hit_rect, (press_k, press_anchor));
            if self.has_sticky {
                self.fade_below_header(i, s);
            }
        }
        while let Some((_, start, surface_start)) = layers.pop() {
            if active_cache.is_some_and(|ac| ac.instance_start == start) {
                let ac = active_cache.take().unwrap();
                self.close_cached_layer(ac, start, surface_start, &mut glyph_count);
            } else {
                crate::render::end_layer(&mut self.instances, &mut self.surfaces, &mut self.marks, start, surface_start);
            }
        }
        // A moving chip can enter a stationary pointer. Re-evaluate against this frame's
        // geometry without inventing pointer-move or drag events on animation frames. Tooltips
        // follow the hold this decides.
        let queued = self.queue.len();
        let held = self.pointer.map(|(x, y)| self.refresh_hover(x, y));
        self.update_help();
        self.layer_scratch = layers;
        self.built = true;
        self.land_unseen_entrances();
        self.built_ids.clear();
        self.built_ids.extend(self.nodes.iter().map(|n| n.id));
        if video_signature != self.video_signature {
            // A video loaded, ended or failed: views reading its state re-run next frame.
            self.video_signature = video_signature;
            self.dirty = true;
            self.videos_active = true;
        }
        if self.track_marks { self.marks.resize(self.instances.len(), 0); }
        self.stats.glyphs = glyph_count;
        self.ambient = ambient;
        // A live transform can change hover paint with no handler and no running spring.
        // Retarget from the new geometry and schedule the frame that samples that paint. A
        // commit that moved the indices left keyboard focus hover (`El::hover_group_focus`) to
        // this pass too.
        let interaction = (self.pointer.is_some() || self.focus.keyboard()) && self.update_interaction(held);
        self.queue.len() > queued || interaction
    }
}
