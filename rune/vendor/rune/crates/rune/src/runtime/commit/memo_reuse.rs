//! Reusing a [`memo`](crate::memo)'s flattened `Node` slice and Taffy `NodeId` subtree across
//! commits when neither the memo's inputs, its root modifiers nor its inherited flatten state
//! changed, rebasing node paths and ids only when its parent path moved.
use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct InheritedFlatten {
    pub(super) motion: Spring,
    pub(super) in_row: RowLayout,
    pub(super) parent_is_stack: bool,
    pub(super) instant_changes: bool,
    pub(super) inert: bool,
    /// The parent's flow (`El::dir`).
    pub(super) flow: crate::el::Flow,
    pub(super) trace_namespace: Option<(&'static str, &'static str)>,
    pub(super) scale_bits: u32,
    /// The window's foreground (`Cx::foreground`), which a memo's uncoloured text took.
    pub(super) foreground: Option<Color>,
}

pub(crate) struct FlattenedMemo {
    pub(super) start: usize,
    pub(super) len: usize,
    pub(super) root_id: NodeId,
    pub(super) last_id: NodeId,
    pub(super) inherited: InheritedFlatten,
    pub(super) hover_within: Option<Rc<(Anim, Anim)>>,
}

pub(crate) struct PendingFlat {
    pub(super) built_id: u64,
    pub(super) start: usize,
    pub(super) len: usize,
    pub(super) inherited: InheritedFlatten,
    pub(super) hover_within: Option<Rc<(Anim, Anim)>>,
}

#[derive(Clone, Copy)]
pub(crate) struct ReusedSpan {
    pub(super) new_start: usize,
    pub(super) old_start: usize,
    pub(super) len: usize,
    pub(super) same_path: bool,
    pub(super) moved: bool,
}

/// A contiguous sub-range of `nodes` (`new_start .. new_start + len`, from `old_start`) whose
/// per-node `apply` diff is a guaranteed no-op.
#[derive(Clone, Copy)]
pub(crate) struct CleanApplySpan {
    pub(super) new_start: usize,
    pub(super) old_start: usize,
    pub(super) len: usize,
}

pub(super) fn is_in_clean_spans(spans: &[CleanApplySpan], i: usize) -> bool {
    let idx = spans.partition_point(|s| s.new_start <= i);
    idx > 0 && {
        let s = &spans[idx - 1];
        i < s.new_start + s.len
    }
}

impl<S: 'static> Runtime<S> {
    fn node_clean_for_apply(&self, new_n: &Node, old_n: &Node, any_animating: bool) -> bool {
        new_n.id == old_n.id
            && new_n.local == old_n.local
            && (!any_animating || !self.scene.store().is_node_animating(new_n.id))
            && self.scene.phase(new_n.id) == Some(PresencePhase::Present)
            && !old_n.group_fade
            && new_n.cold.get().is_none_or(|c| c.matched.is_none() && c.looping.is_none())
            && (self.flying.is_empty() || !self.flying.iter().any(|f| f.0 == new_n.id))
            && (self.aspect_snap.is_empty() || !self.aspect_snap.contains(&new_n.id))
            && new_n.text.as_ref().is_none_or(|t| {
                t.reveal < 0.0 || self.reveals.get(&new_n.id).is_some_and(|r| r.words.is_empty() && r.len == t.content.len())
            })
    }

    /// Fills `out` with the reused memo slices that can skip `apply`'s per-node diff loop.
    pub(super) fn clean_apply_spans(
        &self,
        nodes: &[Node],
        reused: &[ReusedSpan],
        can_skip: bool,
        out: &mut Vec<CleanApplySpan>,
    ) {
        out.clear();
        if !can_skip || reused.is_empty() {
            return;
        }
        out.reserve(reused.len());
        let any_animating = self.scene.store().active_len() > 0;
        for span in reused {
            if !span.same_path || span.len == 0 || span.old_start + span.len > self.nodes.len() || span.new_start + span.len > nodes.len() {
                continue;
            }
            let r_new = &nodes[span.new_start];
            let r_old = &self.nodes[span.old_start];
            if !self.scene.contains(r_new.id) || self.is_exiting(r_new.id) {
                continue;
            }
            let mut scene_rect = r_new.local;
            if !self.virtuals.is_empty()
                && let Some(v) = r_new.parent.and_then(|p| self.virtuals.get(&nodes[p].path))
            {
                scene_rect.y -= v.before;
            }
            let parent_top_clean = r_new.in_row != RowLayout::Snaps
                || r_new.parent.is_none_or(|p| {
                    nodes.get(p + 1).is_none_or(|first_sib| {
                        let mut sr = first_sib.local;
                        if !self.virtuals.is_empty()
                            && let Some(v) = self.virtuals.get(&nodes[p].path)
                        {
                            sr.y -= v.before;
                        }
                        self.scene.get(first_sib.id, prop::FRAME) == Some(sr)
                    })
                });
            let root_clean = self.node_clean_for_apply(r_new, r_old, any_animating)
                && parent_top_clean
                && r_new.parent.map(|p| nodes[p].id) == r_old.parent.map(|p| self.nodes[p].id)
                && (self.virtuals.is_empty() || !self.virtuals.contains_key(&r_new.path))
                && self.scene.is_resting_at(r_new.id, scene_rect)
                && (self.stacks.is_empty() || self.reorder_jump(nodes, span.new_start) == 0.0)
                && (self.virtuals.is_empty()
                    || !r_new.parent.and_then(|p| self.virtuals.get(&nodes[p].path)).is_some_and(crate::el::virtual_list::VirtualList::replaced_view));

            if !root_clean && !self.flying.is_empty() {
                continue;
            }
            let offset = if root_clean { 0 } else { 1 };
            if offset >= span.len {
                continue;
            }
            let interior_clean = (offset..span.len).all(|k| {
                self.node_clean_for_apply(&nodes[span.new_start + k], &self.nodes[span.old_start + k], any_animating)
            });
            if interior_clean {
                let new_start = span.new_start + offset;
                let old_start = span.old_start + offset;
                let len = span.len - offset;
                if let Some(last) = out.last_mut()
                    && last.new_start + last.len == new_start
                    && last.old_start + last.len == old_start
                {
                    last.len += len;
                } else {
                    out.push(CleanApplySpan { new_start, old_start, len });
                }
            }
        }
    }
}

pub(super) struct MemoFlattenCtx<'a> {
    pub(super) old_nodes: &'a mut [Node],
    pub(super) old_present_len: usize,
    pub(super) old_node_index: &'a HashMap<NodeId, usize>,
    pub(super) old_cursor: usize,
    pub(super) committing: bool,
    pub(super) committed: &'a HashMap<u64, FlattenedMemo>,
    pub(super) pending: Vec<PendingFlat>,
    pub(super) reused: Vec<ReusedSpan>,
    pub(super) taken_paths: Vec<(u32, u32)>,
    pub(super) path_ids: &'a mut HashMap<String, NodeId>,
    pub(super) next_id: &'a mut u64,
    /// What text, icons and text fields without their own colour draw in (`Cx::foreground`).
    pub(super) foreground: Option<Color>,
}

fn opt_rc_eq<T: ?Sized>(a: &Option<Rc<T>>, b: &Option<Rc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn opt_box_eq<T: PartialEq>(a: &Option<Box<T>>, b: &Option<Box<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => **x == **y,
        _ => false,
    }
}

fn same_visual_cold(a: &crate::el::VisualCold, b: &crate::el::VisualCold) -> bool {
    let crate::el::VisualCold {
        surface, surface_place, border_bottom, border_sides, border_layout, border_dash, border_dash_phase, shadows, blur_motion,
        clip, clip_radius, clip_corners, cutout, icon, icon_inked, icon_one_color, gradient, radial, outline, shape,
        pixels, pixels_fit, effect, backdrop_blur, hover_motion, hover_within,
        group_opacity, group_color, hover_alone, hover_share, hover_focus, icon_turn, cache_key, offset_motion, scale_motion,
        origin, rotate_motion, child_exit, enter_delay, motion_intent, instant_motion, clip_to_content, flight_crop,
        hold_landing, offset, paint_alpha, layer_opacity, blur, rotate, mark, mirror_rtl, paint_order,
    } = b;
    (match (&a.surface, surface) {
        (None, None) => true,
        (Some((sa, fa)), Some((sb, fb))) => sa.key() == sb.key() && fa == fb,
        _ => false,
    }) && a.surface_place == *surface_place && a.border_bottom == *border_bottom && a.border_sides == *border_sides && a.border_layout == *border_layout && a.border_dash == *border_dash
        && a.border_dash_phase == *border_dash_phase && a.shadows == *shadows
        && opt_box_eq(&a.blur_motion, blur_motion) && a.clip == *clip
        && a.clip_radius == *clip_radius && a.clip_corners == *clip_corners && a.cutout == *cutout
        && match (&a.icon, icon) {
            (None, None) => true,
            (Some((ia, ca)), Some((ib, cb))) => std::ptr::eq(*ia, *ib) && ca == cb,
            _ => false,
        }
        && a.icon_inked == *icon_inked && a.icon_one_color == *icon_one_color
        && opt_box_eq(&a.gradient, gradient) && opt_box_eq(&a.radial, radial)
        && a.outline == *outline && opt_rc_eq(&a.shape, shape)
        && match (&a.pixels, pixels) {
            (None, None) => true,
            (Some(pa), Some(pb)) => Arc::ptr_eq(&pa.source, &pb.source),
            _ => false,
        }
        && a.pixels_fit == *pixels_fit && a.effect == *effect && a.backdrop_blur == *backdrop_blur
        && opt_rc_eq(&a.hover_motion, hover_motion) && opt_rc_eq(&a.hover_within, hover_within)
        && a.group_opacity == *group_opacity && a.group_color == *group_color && a.hover_alone == *hover_alone && a.hover_share == *hover_share && a.hover_focus == *hover_focus
        && a.icon_turn == *icon_turn && a.cache_key == *cache_key
        && opt_box_eq(&a.offset_motion, offset_motion) && opt_box_eq(&a.scale_motion, scale_motion)
        && a.origin == *origin && opt_box_eq(&a.rotate_motion, rotate_motion)
        && opt_box_eq(&a.child_exit, child_exit) && a.enter_delay == *enter_delay
        && a.motion_intent == *motion_intent && a.instant_motion == *instant_motion
        && a.clip_to_content == *clip_to_content
        && a.flight_crop == *flight_crop
        && a.hold_landing == *hold_landing
        && a.offset == *offset && a.paint_alpha == *paint_alpha && a.layer_opacity == *layer_opacity
        && a.blur == *blur && a.rotate == *rotate && a.mark == *mark && a.mirror_rtl == *mirror_rtl && a.paint_order == *paint_order
}

pub(super) fn same_visual(a: &Visual, b: &Visual) -> bool {
    let Visual { fill, radius, opacity, box_paint, cold } = b;
    a.fill == *fill && a.radius.to_bits() == radius.to_bits() && a.opacity == *opacity
        && (crate::cold::Cold::ptr_eq(&a.box_paint, box_paint) || *a.box_paint == **box_paint)
        && (crate::cold::Cold::ptr_eq(&a.cold, cold) || same_visual_cold(&a.cold, cold))
}

fn same_el_cold(a: &crate::el::ElCold, b: &crate::el::ElCold) -> bool {
    let crate::el::ElCold {
        focus, looping, trace_name, trace_namespace, role, label, live, above, motion, opacity_motion, lead,
        transition, matched, scroll, scroll_axes, sticky, scroll_edges, growth, virtual_estimate, virtual_rows,
        virtual_rows_version, glide, input, time_scale, help, help_cut, help_above, material, aspect_fit, a11y,
        #[cfg(debug_assertions)]
        scroll_source: _,
        #[cfg(debug_assertions)]
        virtual_source: _,
    } = b;
    crate::cold::Cold::ptr_eq(&a.focus, focus)
        && opt_box_eq(&a.looping, looping)
        && a.trace_name == *trace_name
        && a.trace_namespace == *trace_namespace
        && a.role == *role
        && a.label == *label
        && a.live.is_empty()
        && live.is_empty()
        && a.above == *above
        && a.motion == *motion
        && opt_box_eq(&a.opacity_motion, opacity_motion)
        && a.lead == *lead
        && opt_box_eq(&a.transition, transition)
        && a.matched == *matched
        && a.scroll == *scroll
        && a.scroll_axes == *scroll_axes
        && opt_box_eq(&a.sticky, sticky)
        && a.scroll_edges == *scroll_edges
        && opt_box_eq(&a.growth, growth)
        && a.virtual_estimate == *virtual_estimate
        && opt_rc_eq(&a.virtual_rows, virtual_rows)
        && a.virtual_rows_version == *virtual_rows_version
        && a.glide == *glide
        && a.input.is_none()
        && input.is_none()
        && a.time_scale == *time_scale
        && a.help == *help
        && a.help_cut == *help_cut
        && a.help_above == *help_above
        && a.material == *material
        && a.aspect_fit == *aspect_fit
        && a.a11y == *a11y
}

fn same_gestures(
    a: &Option<Box<crate::input::canvas_input::CanvasInput>>,
    b: &Option<Box<crate::input::canvas_input::CanvasInput>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.buttons == y.buttons
                && opt_rc_eq(&x.wheel, &y.wheel)
                && opt_rc_eq(&x.magnify, &y.magnify)
                && opt_rc_eq(&x.rotate, &y.rotate)
                && opt_rc_eq(&x.smart_magnify, &y.smart_magnify)
        }
        _ => false,
    }
}

fn same_drop(
    a: &Option<Box<crate::el::DropSpec>>,
    b: &Option<Box<crate::el::DropSpec>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            opt_rc_eq(&x.on_drop, &y.on_drop)
                && opt_rc_eq(&x.accepts, &y.accepts)
                && opt_rc_eq(&x.on_hover, &y.on_hover)
                && opt_rc_eq(&x.on_reject, &y.on_reject)
                && opt_rc_eq(&x.on_reject_hover, &y.on_reject_hover)
                && opt_rc_eq(&x.on_move, &y.on_move)
        }
        _ => false,
    }
}

fn same_handlers(
    a: &crate::cold::Cold<crate::el::Handlers>,
    b: &crate::cold::Cold<crate::el::Handlers>,
) -> bool {
    if crate::cold::Cold::ptr_eq(a, b) {
        return true;
    }
    let crate::el::Handlers {
        on_paste,
        on_window_focus,
        on_spellcheck,
        on_spellcheck_languages,
        modal,
        on_press,
        on_double_press,
        on_secondary_press,
        on_hover,
        on_hover_change,
        hover_delay,
        on_pointer_move,
        gestures,
        on_drag,
        touch_policy,
        drag_out,
        drag_threshold,
        on_drag_start,
        on_drag_end,
        drop,
        on_at_bottom,
        on_far_from_bottom,
        on_visible_rows,
        on_layout,
        on_screen_layout,
        on_drawn_layout,
        on_visible,
        drawn_rect,
        on_scroll_offset,
        selection_copy,
        on_selection_change,
        cursor,
        press_scale,
        #[cfg(debug_assertions)]
        sites: _,
    } = &**b;
    a.modal.is_none()
        && modal.is_none()
        && match (&a.on_paste, on_paste) {
            (None, None) => true,
            (Some(pa), Some(pb)) => pa.rich == pb.rich && Rc::ptr_eq(&pa.call, &pb.call),
            _ => false,
        }
        && opt_rc_eq(&a.on_window_focus, on_window_focus)
        && match (&a.on_spellcheck, on_spellcheck) {
            (None, None) => true,
            (Some((on_a, fa)), Some((on_b, fb))) => on_a == on_b && Rc::ptr_eq(fa, fb),
            _ => false,
        }
        && opt_rc_eq(&a.on_spellcheck_languages, on_spellcheck_languages)
        && opt_rc_eq(&a.on_press, on_press)
        && opt_rc_eq(&a.on_double_press, on_double_press)
        && opt_rc_eq(&a.on_secondary_press, on_secondary_press)
        && opt_rc_eq(&a.on_hover, on_hover)
        && opt_rc_eq(&a.on_hover_change, on_hover_change)
        && a.hover_delay == *hover_delay
        && opt_rc_eq(&a.on_pointer_move, on_pointer_move)
        && same_gestures(&a.gestures, gestures)
        && opt_rc_eq(&a.on_drag, on_drag)
        && a.touch_policy == *touch_policy
        && opt_rc_eq(&a.drag_out, drag_out)
        && a.drag_threshold == *drag_threshold
        && opt_rc_eq(&a.on_drag_start, on_drag_start)
        && opt_rc_eq(&a.on_drag_end, on_drag_end)
        && same_drop(&a.drop, drop)
        && opt_rc_eq(&a.on_at_bottom, on_at_bottom)
        && match (&a.on_far_from_bottom, on_far_from_bottom) {
            (None, None) => true,
            (Some((da, fa)), Some((db, fb))) => da == db && Rc::ptr_eq(fa, fb),
            _ => false,
        }
        && opt_rc_eq(&a.on_visible_rows, on_visible_rows)
        && opt_rc_eq(&a.on_layout, on_layout)
        && opt_rc_eq(&a.on_screen_layout, on_screen_layout)
        && opt_rc_eq(&a.on_drawn_layout, on_drawn_layout)
        && opt_rc_eq(&a.on_visible, on_visible)
        && match (&a.drawn_rect, drawn_rect) {
            (None, None) => true,
            (Some(la), Some(lb)) => std::sync::Arc::ptr_eq(&la.source, &lb.source),
            _ => false,
        }
        && opt_rc_eq(&a.on_scroll_offset, on_scroll_offset)
        && opt_rc_eq(&a.selection_copy, selection_copy)
        && opt_rc_eq(&a.on_selection_change, on_selection_change)
        && a.cursor == *cursor
        && a.press_scale == *press_scale
}

/// Exhaustively checks that `el` has not been mutated by builders after `Built::placeholder`
/// returned it (apart from `key` and `virtual_mount`, which flattening handles explicitly).
pub(super) fn same_root_el(el: &El, built: &crate::memo::Built) -> bool {
    let base = &built.root;
    let El {
        style,
        vis,
        children: _,
        key: _,
        text,
        handlers,
        memo: _,
        cold,
        scale,
        flags,
        role,
        vis_flags,
        #[cfg(debug_assertions)]
        key_source: _,
    } = base;
    // Flags flattening handles itself, or that only describe the children.
    let ignored = crate::el::ElFlags::HAS_VIRTUAL | crate::el::ElFlags::MEMO_EXTENDED | crate::el::ElFlags::VIRTUAL_MOUNT;
    let mut a = el.flags;
    let mut b = *flags;
    a.set(ignored, false);
    b.set(ignored, false);

    el.children.is_empty()
        && el.memo_children().is_some_and(|mc| Rc::ptr_eq(mc, &built.children))
        && el.role == *role
        && a == b
        && el.vis_flags == *vis_flags
        && el.style == *style
        && same_visual(&el.vis, vis)
        && match (&el.text, text) {
            (None, None) => true,
            (Some(ta), Some(tb)) => crate::cold::SharedBox::ptr_eq(ta, tb),
            _ => false,
        }
        && same_handlers(&el.handlers, handlers)
        && el.scale == *scale
        && cold.live.is_empty()
        && cold.input.is_none()
        && (crate::cold::Cold::ptr_eq(&el.cold, cold) || same_el_cold(&el.cold, cold))
}

pub(super) fn inherited_for(
    parent: Option<usize>,
    nodes: &[Node],
    motion: Spring,
    in_row: RowLayout,
    scale: f32,
    foreground: Option<Color>,
) -> (InheritedFlatten, Option<Rc<(Anim, Anim)>>) {
    let parent_is_stack = parent.is_some_and(|p| nodes[p].growth.is_some());
    let instant_changes = parent.is_some_and(|p| nodes[p].instant_changes);
    let inert = parent.is_some_and(|p| nodes[p].inert);
    let trace_namespace = parent.and_then(|p| nodes[p].trace_namespace);
    let hover_within = parent.and_then(|p| nodes[p].hover_within.clone());
    (
        InheritedFlatten {
            motion,
            in_row,
            parent_is_stack,
            instant_changes,
            inert,
            flow: parent.map_or(crate::el::Flow::Ltr, |p| nodes[p].flow),
            trace_namespace,
            scale_bits: scale.to_bits(),
            foreground,
        },
        hover_within,
    )
}

fn path_matches(old_path: &str, parent_path: &str, key: Option<&str>, index: usize) -> bool {
    let tail = if parent_path.is_empty() {
        old_path
    } else {
        let Some(rest) = old_path.strip_prefix(parent_path).and_then(|r| r.strip_prefix('/')) else {
            return false;
        };
        rest
    };
    match key {
        Some(k) => tail == k,
        None => tail.strip_prefix('#').is_some_and(|n| !n.contains('/') && n.parse::<usize>().ok() == Some(index)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn try_reuse_memo(
    el: &El,
    parent: Option<usize>,
    parent_path: &str,
    index: usize,
    motion: Spring,
    in_row: RowLayout,
    scale: f32,
    taffy: &mut LayoutEngine,
    nodes: &mut Vec<Node>,
    ctx: &mut MemoFlattenCtx<'_>,
) -> Option<SlotId> {
    let built = el.memo_built()?;
    if !built.reusable {
        return None;
    }
    let flat = ctx.committed.get(&built.id)?;
    let end = flat.start.checked_add(flat.len)?;
    if flat.len == 0 || end > ctx.old_present_len || end > ctx.old_nodes.len() {
        return None;
    }
    let old_slice = &ctx.old_nodes[flat.start..end];
    if old_slice[0].is_moved_stub() || old_slice[0].path.is_empty() || old_slice[0].id != flat.root_id || old_slice[flat.len - 1].id != flat.last_id {
        return None;
    }
    let (inherited, hover_within) = inherited_for(parent, nodes, motion, in_row, scale, ctx.foreground);
    // By value: a view that sets `hover_motion_within` above the memo builds a new pair each time.
    if flat.inherited != inherited || flat.hover_within.as_deref() != hover_within.as_deref() {
        return None;
    }
    if el.key != old_slice[0].key || !same_root_el(el, built) {
        return None;
    }
    if !old_slice.iter().all(|old| !old.is_moved_stub() && !old.path.is_empty() && taffy.can_reuse(old.id.0, old.taffy)) {
        return None;
    }

    let old_root_path = old_slice[0].path.as_str();
    let same_path = path_matches(old_root_path, parent_path, el.key.as_deref(), index);
    let new_start = nodes.len();
    let moved = same_path && ctx.committing;
    let root_taffy = old_slice[0].taffy;

    if moved {
        for k in 0..flat.len {
            let new_idx = new_start + k;
            let slot = &mut ctx.old_nodes[flat.start + k];
            taffy.reuse_slot(slot.taffy, new_idx as u32);
            let stub = Node::moved_stub(slot);
            let mut n = std::mem::replace(slot, stub);
            n.text_layout = None;
            n.elided = None;
            n.clear_run_layout();
            n.scale_track = false;
            n.group_fade = false;
            n.virtual_mount = if k == 0 { el.flags.has(crate::el::ElFlags::VIRTUAL_MOUNT) } else { false };
            n.parent = if k == 0 { parent } else { n.parent.map(|p| new_start + (p - flat.start)) };
            n.target = [0.0; 4];
            n.local = MRect::ZERO;
            n.inset = [0.0; 4];
            nodes.push(n);
        }
    } else if same_path {
        for (k, old) in old_slice.iter().enumerate() {
            let new_idx = new_start + k;
            taffy.reuse_slot(old.taffy, new_idx as u32);
            let mut n = Node {
                id: old.id,
                text_layout: None,
                elided: None,
                scale_track: false,
                group_fade: false,
                virtual_mount: if k == 0 { el.flags.has(crate::el::ElFlags::VIRTUAL_MOUNT) } else { false },
                parent: if k == 0 { parent } else { old.parent.map(|p| new_start + (p - flat.start)) },
                target: [0.0; 4],
                local: MRect::ZERO,
                inset: [0.0; 4],
                ..old.clone()
            };
            n.clear_run_layout();
            nodes.push(n);
        }
    } else {
        let new_root_path = node_path(parent_path, el.key.as_deref(), index);
        let mut rebased = Vec::with_capacity(flat.len);
        for (k, old) in old_slice.iter().enumerate() {
            let path = if k == 0 {
                new_root_path.clone()
            } else {
                let tail = &old.path[old_root_path.len()..];
                let mut s = String::with_capacity(new_root_path.len() + tail.len());
                s.push_str(&new_root_path);
                s.push_str(tail);
                s
            };
            if ctx.path_ids.get(&path).is_some_and(|id| taffy.is_used_this_commit(id.0)) {
                return None;
            }
            rebased.push(path);
        }
        let mut new_ids = Vec::with_capacity(flat.len);
        for path in &rebased {
            let id = match ctx.path_ids.get(path.as_str()) {
                Some(&id) => id,
                None => {
                    let id = NodeId(*ctx.next_id);
                    *ctx.next_id += 1;
                    ctx.path_ids.insert(path.clone(), id);
                    id
                }
            };
            new_ids.push(id);
        }
        taffy.rekey_and_reuse_batch(
            old_slice
                .iter()
                .zip(new_ids.iter())
                .enumerate()
                .map(|(k, (old, new_id))| (old.id.0, new_id.0, old.taffy, (new_start + k) as u32)),
        );
        for (k, ((old, path), id)) in old_slice.iter().zip(rebased).zip(new_ids).enumerate() {
            let mut n = Node {
                id,
                path,
                text_layout: None,
                elided: None,
                scale_track: false,
                group_fade: false,
                virtual_mount: if k == 0 { el.flags.has(crate::el::ElFlags::VIRTUAL_MOUNT) } else { false },
                parent: if k == 0 { parent } else { old.parent.map(|p| new_start + (p - flat.start)) },
                target: [0.0; 4],
                local: MRect::ZERO,
                inset: [0.0; 4],
                ..old.clone()
            };
            n.clear_run_layout();
            nodes.push(n);
        }
    }

    if same_path {
        ctx.old_cursor = end;
    }
    ctx.pending.push(PendingFlat {
        built_id: built.id,
        start: new_start,
        len: flat.len,
        inherited,
        hover_within,
    });
    ctx.reused.push(ReusedSpan {
        new_start,
        old_start: flat.start,
        len: flat.len,
        same_path,
        moved,
    });
    Some(root_taffy)
}

/// Puts `moved` slices and individually taken paths back into `old_nodes` with their pre-pass
/// geometry and flags before a virtual-window retry or a layout-read, focus or caret re-measure.
pub(super) fn restore_moved_nodes(
    old_nodes: &mut [Node],
    nodes: &mut [Node],
    reused: &[ReusedSpan],
    taken_paths: &[(u32, u32)],
) {
    for &(oi, ni) in taken_paths {
        old_nodes[oi as usize].path = std::mem::take(&mut nodes[ni as usize].path);
    }
    for span in reused.iter().filter(|s| s.moved) {
        for k in 0..span.len {
            let stub = &old_nodes[span.old_start + k];
            let n = &mut nodes[span.new_start + k];
            n.parent = stub.parent;
            n.target = stub.target;
            n.local = stub.local;
            n.inset = stub.inset;
            n.scale_track = stub.scale_track;
            n.group_fade = stub.group_fade;
            n.text_layout = None;
            n.elided = None;
            n.clear_run_layout();
            std::mem::swap(&mut old_nodes[span.old_start + k], n);
        }
    }
}

impl crate::memo::MemoCache {
    /// Updates flattened subtree spans once `apply` installs `nodes` (`0..present_len`).
    /// `None` when `relayout` reused the previous commit's node array in place.
    pub(crate) fn commit_flattened(
        &mut self,
        nodes: &[Node],
        present_len: usize,
        pending: Option<Vec<PendingFlat>>,
        mut reused: Vec<ReusedSpan>,
        mut taken_paths: Vec<(u32, u32)>,
    ) {
        self.present_len = present_len;
        taken_paths.clear();
        self.taken_paths_scratch = taken_paths;
        let Some(mut pending) = pending else {
            reused.clear();
            self.reused_scratch = reused;
            return;
        };
        // Nested memos inside an outer memo that was reused whole stay live at their rebased index.
        if !self.flattened.is_empty() {
            self.flattened.retain(|_, flat| {
                let Some(span) = reused.iter().find(|s| {
                    flat.start >= s.old_start && flat.start + flat.len <= s.old_start + s.len
                }) else {
                    return false;
                };
                flat.start = span.new_start + (flat.start - span.old_start);
                if !span.same_path {
                    flat.root_id = nodes[flat.start].id;
                    flat.last_id = nodes[flat.start + flat.len - 1].id;
                }
                true
            });
        }
        for p in pending.drain(..) {
            if p.len == 0 || p.start + p.len > present_len {
                continue;
            }
            self.flattened.insert(
                p.built_id,
                FlattenedMemo {
                    start: p.start,
                    len: p.len,
                    root_id: nodes[p.start].id,
                    last_id: nodes[p.start + p.len - 1].id,
                    inherited: p.inherited,
                    hover_within: p.hover_within,
                },
            );
        }
        reused.clear();
        self.pending_scratch = pending;
        self.reused_scratch = reused;
    }

    /// Rebases flattened subtree indices after `prune_exited` compacts `self.nodes`.
    pub(crate) fn on_prune(&mut self, remap: &[Option<usize>]) {
        self.present_len = remap
            .get(..self.present_len.min(remap.len()))
            .map_or(0, |s| s.iter().filter(|r| r.is_some()).count());
        self.flattened.retain(|_, flat| {
            let Some(new_start) = remap.get(flat.start).copied().flatten() else {
                return false;
            };
            let Some(new_last) = remap.get(flat.start + flat.len - 1).copied().flatten() else {
                return false;
            };
            if new_last + 1 != new_start + flat.len {
                return false;
            }
            flat.start = new_start;
            true
        });
    }
}

#[cfg(test)]
#[path = "../memo_reuse_tests.rs"]
mod tests;
