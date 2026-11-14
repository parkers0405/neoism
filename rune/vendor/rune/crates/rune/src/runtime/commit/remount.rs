//! Remounting [`El::virtual_rows`](crate::El::virtual_rows) windows when a commit only shifted a
//! virtual scroll window or filled overscan (`CommitWhy::VIRTUAL`).
//!
//! With app state, scale, blend mode and layout reads unchanged, non-list nodes and rows already
//! mounted stay as they were: they move directly out of `self.nodes` into the new pass's node
//! buffer without running `view` or calling `RowSource::build` on staying rows. Only newly
//! entering rows (and unmounted focused or selection-pinned rows) build and flatten.

use std::ops::Range;
use std::rc::Rc;

use super::*;

pub(in crate::runtime) enum RemountOutcome {
    Measured(Measured),
    Retry,
    Fallback,
}

struct ListPlan {
    path: String,
    old_list_idx: usize,
    list_subtree_end: usize,
    range: Range<usize>,
    pad_top: f32,
    pad_bottom: f32,
    mounted_rows: HashMap<String, (usize, usize)>,
    focused: Option<usize>,
    pinned: Vec<usize>,
    rows: Rc<crate::el::virtual_rows::RowSource>,
    list_group: Option<String>,
    inert: bool,
    quiet: Vec<bool>,
    built_rows: Vec<(usize, El)>,
}

impl<S: 'static> Runtime<S> {
    /// Whether this commit only shifts or fills `virtual_rows` windows, so `view` and already
    /// mounted subtrees need not rebuild.
    pub(in crate::runtime) fn virtual_only(&self, tx: &Tx) -> bool {
        #[cfg(feature = "hot")]
        if self.hot.is_some() {
            return false;
        }
        if self.relayout
            || !self.state_unchanged
            || tx.settle
            || self.checking_live_only()
            || self.reuse.read_moved
            || self.reuse.scale != self.env.scale
            || self.reuse.srgb != Some(self.srgb_blend)
            || self.playground.is_some()
            || self.patch_pending()
            || self.virtuals.is_empty()
            || self.nodes.is_empty()
            || self.layout.live_count() != self.nodes.len()
        {
            return false;
        }
        if !self.nodes.iter().all(|n| {
            relayout::reusable(n) && n.text.as_ref().is_none_or(|t| !t.redacted && t.live.is_none())
        }) {
            return false;
        }
        for (path, v) in &self.virtuals {
            if !v.indexed || !v.selection.source.as_ref().is_some_and(|s| s.count == v.keys.len()) {
                return false;
            }
            let Some(&list_id) = self.path_ids.get(path) else { return false };
            let Some(&list_idx) = self.node_index.get(&list_id) else { return false };
            if !self.nodes.get(list_idx).is_some_and(|n| n.id == list_id && n.path == *path && n.growth.is_some()) {
                return false;
            }
            let Some(&scroll_id) = self.path_ids.get(&v.scroll_path) else { return false };
            let Some(&scroll_idx) = self.node_index.get(&scroll_id) else { return false };
            if !self.nodes.get(scroll_idx).is_some_and(|n| {
                n.id == scroll_id
                    && n.path == v.scroll_path
                    && n.scroll.is_some()
                    && !n.scroll_axes.x()
                    && n.growth.is_none()
            }) {
                return false;
            }
            if self.virtuals.keys().any(|other| {
                other != path && other.strip_prefix(path.as_str()).is_some_and(|r| r.starts_with('/'))
            }) {
                return false;
            }
        }
        true
    }
}

fn move_slice(
    old_nodes: &mut [Node],
    range: Range<usize>,
    nodes: &mut Vec<Node>,
    old_to_new: &mut [u32],
    taffy: &mut LayoutEngine,
    reused: &mut Vec<memo_reuse::ReusedSpan>,
    root_virtual_mount: Option<bool>,
) {
    if range.is_empty() {
        return;
    }
    let start = range.start;
    let len = range.len();
    let new_start = nodes.len();
    for (k, old_idx) in range.enumerate() {
        let new_idx = new_start + k;
        old_to_new[old_idx] = new_idx as u32;
        let slot = &mut old_nodes[old_idx];
        taffy.reuse_slot(slot.taffy, new_idx as u32);
        let old_parent = slot.parent;
        let stub = Node::moved_stub(slot);
        let mut n = std::mem::replace(slot, stub);
        n.text_layout = None;
        n.elided = None;
        n.clear_run_layout();
        n.scale_track = false;
        n.group_fade = false;
        n.virtual_mount = if k == 0 { root_virtual_mount.unwrap_or(false) } else { false };
        n.parent = old_parent.map(|p| old_to_new[p] as usize);
        n.target = [0.0; 4];
        n.local = MRect::ZERO;
        n.inset = [0.0; 4];
        nodes.push(n);
    }
    reused.push(memo_reuse::ReusedSpan {
        new_start,
        old_start: start,
        len,
        same_path: true,
        moved: true,
    });
}

fn build_indexed_row(
    rows: &crate::el::virtual_rows::RowSource,
    state: crate::states::StatesRef<'_>,
    path: &str,
    key: &str,
    i: usize,
    key_warnings: &mut Option<Vec<String>>,
) -> El {
    let tag = crate::memo::virtual_row_tag(path, key);
    let mut row = crate::memo::in_virtual_row(tag, || (rows.build)(state, i)).key(key.to_owned());
    if crate::input::key_diagnostics::clean(&row) {
        return row;
    }
    let duplicates = crate::input::key_diagnostics::repair_at(&mut row, &format!("{path}/{key}"));
    match key_warnings.as_mut() {
        Some(out) => out.extend(duplicates.into_iter().map(|d| d.message)),
        None => crate::input::key_diagnostics::warn_once(&duplicates),
    }
    row
}

impl<S: 'static> Measurement<'_, S> {
    pub(in crate::runtime) fn remount_virtuals(mut self) -> RemountOutcome {
        self.virtual_culled.clear();
        self.virtual_inputs.clear();

        let mut plans = Vec::with_capacity(self.virtuals.len());
        for (path, v) in self.virtuals.iter_mut() {
            let list_id = self.path_ids[path];
            let old_list_idx = self.node_index[&list_id];
            let scroll_id = self.path_ids[&v.scroll_path];
            let scroll_idx = self.node_index[&scroll_id];

            let mut list_subtree_end = old_list_idx + 1;
            while list_subtree_end < self.nodes.len()
                && self.nodes[list_subtree_end].parent.is_some_and(|p| p >= old_list_idx)
            {
                list_subtree_end += 1;
            }
            let mut mounted_rows = HashMap::default();
            let mut cur = old_list_idx + 1;
            while cur < list_subtree_end {
                debug_assert_eq!(self.nodes[cur].parent, Some(old_list_idx));
                let mut row_end = cur + 1;
                while row_end < list_subtree_end
                    && self.nodes[row_end].parent.is_some_and(|p| p >= cur)
                {
                    row_end += 1;
                }
                if let Some(k) = &self.nodes[cur].key {
                    mounted_rows.insert(k.clone(), (cur, row_end));
                }
                cur = row_end;
            }

            let sticky = self.nodes[scroll_idx].scroll.unwrap().stick_bottom;
            let old_scroll = self.scrolls.get(&scroll_id);
            let pinned = old_scroll.map_or(sticky, |s| s.pinned());
            let mut offset = old_scroll.map_or(0.0, |s| s.offset as f32);
            v.viewport = self.nodes[scroll_idx].local.h;
            offset = (offset - v.top).max(0.0);
            v.anchor = if pinned { None } else { v.first_visible(offset, |_| true) };
            let delta = v.anchor_delta();
            offset = (offset + delta).max(0.0);
            if let Some(st) = self.scrolls.get_mut(&scroll_id) && !pinned {
                st.offset = (st.offset + f64::from(delta)).max(0.0);
            }
            let shown = std::mem::take(&mut v.shown);
            v.select(offset, pinned, Some(&shown));
            for key in shown.iter().filter(|k| v.index_of(k).is_some_and(|i| !v.range.contains(&i))) {
                self.virtual_culled.insert(format!("{path}/{key}"));
            }
            v.shown = shown;

            let list_group = v.selection.group_path.clone();
            let inert = v.selection.inert;
            v.selection.texts.clear();
            v.selection.copy_providers.clear();
            v.selection.unmounted.clear();
            let rows = v.selection.source.as_ref().unwrap().clone();

            let under = |f: &str| f.strip_prefix(path.as_str()).is_some_and(|rest| rest.starts_with('/'));
            let focused = v
                .focused_row
                .as_deref()
                .filter(|_| self.focused.is_some_and(under))
                .and_then(|key| v.index_of(key))
                .filter(|i| !v.range.contains(i));
            let pinned_rows: Vec<usize> = v
                .pinned_rows
                .iter()
                .filter_map(|key| v.index_of(key))
                .filter(|i| !v.range.contains(i))
                .collect();

            plans.push(ListPlan {
                path: path.clone(),
                old_list_idx,
                list_subtree_end,
                range: v.range.clone(),
                pad_top: v.pad_top + v.before,
                pad_bottom: v.pad_bottom + v.after,
                mounted_rows,
                focused,
                pinned: pinned_rows,
                rows,
                list_group,
                inert,
                quiet: Vec::new(),
                built_rows: Vec::new(),
            });
        }
        plans.sort_unstable_by_key(|p| p.old_list_idx);

        let mut kept_rows = HashSet::default();
        for plan in &plans {
            let keys = &self.virtuals[&plan.path].keys;
            for row_i in plan.range.clone() {
                let key = &keys[row_i];
                if plan.mounted_rows.contains_key(key) {
                    kept_rows.insert(crate::memo::virtual_row_tag(&plan.path, key));
                }
            }
        }

        let memo = crate::memo::enter_partial(
            std::mem::take(self.memo),
            &kept_rows,
            self.env,
            self.system_accent,
            self.system_reduced_motion,
            crate::theme::blends_srgb(),
            self.chrome_bits(),
            self.caret.clone(),
            self.layout_seen.clone(),
        );
        let view_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::View);
        let state = playground_host::handler_states_ref(self.state, self.playground);
        let mut nested_virtual = false;
        for plan in &mut plans {
            let v = self.virtuals.get_mut(&plan.path).unwrap();
            if let Some(i) = plan.focused {
                let row = build_indexed_row(&plan.rows, state, &plan.path, &v.keys[i], i, self.key_warnings);
                measure::collect_virtual_inputs(&row, &plan.path, i, plan.inert, self.virtual_inputs);
                if plan.pinned.contains(&i) {
                    selectable::virtual_selection::collect_virtual_row_selection(
                        &row,
                        &plan.path,
                        &v.keys[i],
                        i,
                        plan.list_group.as_deref(),
                        plan.inert,
                        &mut v.selection.texts,
                        &mut v.selection.copy_providers,
                    );
                }
            }
            for &i in &plan.pinned {
                if Some(i) == plan.focused {
                    continue;
                }
                let row = build_indexed_row(&plan.rows, state, &plan.path, &v.keys[i], i, self.key_warnings);
                selectable::virtual_selection::collect_virtual_row_selection(
                    &row,
                    &plan.path,
                    &v.keys[i],
                    i,
                    plan.list_group.as_deref(),
                    plan.inert,
                    &mut v.selection.texts,
                    &mut v.selection.copy_providers,
                );
            }
            for (pos, row_i) in plan.range.clone().enumerate() {
                let key = &v.keys[row_i];
                if !plan.mounted_rows.contains_key(key) {
                    let row = build_indexed_row(&plan.rows, state, &plan.path, key, row_i, self.key_warnings);
                    if row.flags.has(crate::el::ElFlags::HAS_VIRTUAL) {
                        nested_virtual = true;
                        break;
                    }
                    plan.built_rows.push((pos, row));
                }
            }
            if nested_virtual {
                break;
            }
        }
        let (memo, replayed) = memo.finish();
        drop(view_time);
        *self.memo = memo;
        if nested_virtual {
            return RemountOutcome::Fallback;
        }
        self.caret_read |= replayed.caret;
        for read in replayed.layout {
            layout_read::note_read_at(&mut self.layout_reads, &read.key, read.same, read.at);
        }

        for plan in &mut plans {
            let v = self.virtuals.get_mut(&plan.path).unwrap();
            plan.quiet = plan.range.clone().map(|i| v.mounts_quietly(i, self.commits)).collect();
            for (pos, row) in &mut plan.built_rows {
                row.flags.set(crate::el::ElFlags::VIRTUAL_MOUNT, plan.quiet[*pos]);
            }
        }

        let mut on_spine = vec![false; self.nodes.len()];
        for plan in &plans {
            on_spine[plan.old_list_idx] = true;
            let mut cur = self.nodes[plan.old_list_idx].parent;
            while let Some(p) = cur {
                if on_spine[p] {
                    break;
                }
                on_spine[p] = true;
                cur = self.nodes[p].parent;
            }
        }

        let scale = self.env.scale;
        let mut taffy = std::mem::replace(self.layout, LayoutEngine::with_capacity(0));
        taffy.begin_commit();
        let mut nodes = std::mem::take(self.spare_nodes);
        nodes.clear();
        nodes.reserve(self.nodes.len().max(32));
        let mut pending = std::mem::take(&mut self.memo.pending_scratch);
        pending.clear();
        let mut reused = std::mem::take(&mut self.memo.reused_scratch);
        reused.clear();
        let mut path_buf = std::mem::take(&mut self.memo.path_scratch);
        path_buf.clear();
        let empty_committed = HashMap::default();

        let mut old_to_new = vec![u32::MAX; self.nodes.len()];
        let mut plan_iter = plans.into_iter().peekable();
        let mut old_idx = 0usize;
        while old_idx < self.nodes.len() {
            if plan_iter.peek().is_some_and(|p| p.old_list_idx == old_idx) {
                let plan = plan_iter.next().unwrap();
                let new_list_idx = nodes.len();
                old_to_new[old_idx] = new_list_idx as u32;
                let (list_id, list_taffy, list_motion, list_in_row) = {
                    let slot = &mut self.nodes[old_idx];
                    let old_parent = slot.parent;
                    let stub = Node::moved_stub(slot);
                    let mut n = std::mem::replace(slot, stub);
                    n.text_layout = None;
                    n.elided = None;
                    n.clear_run_layout();
                    n.scale_track = false;
                    n.group_fade = false;
                    n.virtual_mount = false;
                    n.parent = old_parent.map(|p| old_to_new[p] as usize);
                    n.target = [0.0; 4];
                    n.local = MRect::ZERO;
                    n.inset = [0.0; 4];
                    let info = (n.id, n.taffy, n.motion, n.in_row);
                    nodes.push(n);
                    info
                };
                reused.push(memo_reuse::ReusedSpan {
                    new_start: new_list_idx,
                    old_start: old_idx,
                    len: 1,
                    same_path: true,
                    moved: true,
                });

                let mut list_style = taffy.style(list_taffy).clone();
                list_style.set_pad_top(plan.pad_top);
                list_style.set_pad_bottom(plan.pad_bottom);
                let parent_row = matches!(
                    list_style.flex_direction(),
                    FlexDirection::Row | FlexDirection::RowReverse
                );

                let mut row_slots = Vec::with_capacity(plan.range.len());
                let mut any_row_dirty = false;
                let mut built_iter = plan.built_rows.into_iter().peekable();
                let keys = &self.virtuals[&plan.path].keys;
                for (pos, row_i) in plan.range.clone().enumerate() {
                    if built_iter.peek().is_some_and(|(p, _)| *p == pos) {
                        let (_, row_el) = built_iter.next().unwrap();
                        path_buf.clear();
                        path_buf.push_str(&nodes[new_list_idx].path);
                        let mut memo_ctx = memo_reuse::MemoFlattenCtx {
                            old_nodes: &mut *self.nodes,
                            old_present_len: 0,
                            old_node_index: &*self.node_index,
                            old_cursor: 0,
                            committing: self.committing,
                            committed: &empty_committed,
                            pending,
                            reused,
                            taken_paths: Vec::new(),
                            path_ids: &mut *self.path_ids,
                            next_id: &mut *self.next_id,
                            foreground: self.defaults.foreground,
                        };
                        let (kid_slot, kid_dirty) = flatten_with_buf(
                            row_el,
                            Some(new_list_idx),
                            &mut path_buf,
                            pos,
                            list_motion,
                            list_in_row,
                            parent_row,
                            &mut taffy,
                            &mut nodes,
                            scale,
                            &mut memo_ctx,
                        );
                        pending = memo_ctx.pending;
                        reused = memo_ctx.reused;
                        row_slots.push(kid_slot);
                        any_row_dirty |= kid_dirty;
                    } else {
                        let &(rs, re) = &plan.mounted_rows[&keys[row_i]];
                        let row_taffy = self.nodes[rs].taffy;
                        move_slice(
                            self.nodes,
                            rs..re,
                            &mut nodes,
                            &mut old_to_new,
                            &mut taffy,
                            &mut reused,
                            Some(plan.quiet[pos]),
                        );
                        row_slots.push(row_taffy);
                    }
                }

                let child_mark = taffy.begin_children();
                for s in row_slots {
                    taffy.push_child(s);
                }
                let (slot, dirty) = taffy.sync_node(
                    list_id.0,
                    new_list_idx as u32,
                    list_style,
                    None,
                    false,
                    child_mark,
                    any_row_dirty,
                );
                debug_assert_eq!(slot, list_taffy);
                if dirty {
                    taffy.mark_dirty_and_ancestors(
                        list_taffy,
                        std::iter::successors(nodes[new_list_idx].parent, |&p| nodes[p].parent)
                            .map(|p| nodes[p].taffy),
                    );
                }
                old_idx = plan.list_subtree_end;
            } else if on_spine[old_idx] {
                move_slice(
                    self.nodes,
                    old_idx..old_idx + 1,
                    &mut nodes,
                    &mut old_to_new,
                    &mut taffy,
                    &mut reused,
                    None,
                );
                old_idx += 1;
            } else {
                let mut sub_end = old_idx + 1;
                while sub_end < self.nodes.len()
                    && self.nodes[sub_end].parent.is_some_and(|p| p >= old_idx)
                {
                    sub_end += 1;
                }
                move_slice(
                    self.nodes,
                    old_idx..sub_end,
                    &mut nodes,
                    &mut old_to_new,
                    &mut taffy,
                    &mut reused,
                    None,
                );
                old_idx = sub_end;
            }
        }

        path_buf.clear();
        self.memo.path_scratch = path_buf;
        taffy.finish_flatten();
        let root_id = nodes[0].taffy;
        let (layout, mut nodes) = self.solve(taffy, root_id, nodes);
        if self.measure_virtuals(&nodes, false) {
            memo_reuse::restore_moved_nodes(self.nodes, &mut nodes, &reused, &[]);
            nodes.clear();
            *self.spare_nodes = nodes;
            pending.clear();
            reused.clear();
            self.memo.pending_scratch = pending;
            self.memo.reused_scratch = reused;
            *self.layout = layout;
            return RemountOutcome::Retry;
        }
        RemountOutcome::Measured(Measured {
            layout,
            nodes,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            defaults: self.defaults,
            titlebar: self.titlebar,
            caret_read: self.caret_read,
            layout_reads: self.layout_reads,
            memo_flattened: Some(pending),
            memo_reused: reused,
            taken_paths: Vec::new(),
            selection: Vec::new(),
        })
    }
}
