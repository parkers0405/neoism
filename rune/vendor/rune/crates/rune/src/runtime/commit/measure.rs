use super::*;
use crate::el::ElFlags;

pub(super) fn collect_virtual_inputs(el: &El, parent: &str, index: usize, inert: bool, inputs: &mut HashMap<String, Box<InputSpec>>) {
    // Most offscreen rows hold no focused field; find that out before building any paths.
    if !crate::memo::has_focused_input(el) {
        return;
    }
    let inert = inert || el.flags.has(ElFlags::INERT);
    let input = el.cold.input.as_ref().filter(|i| i.edit.focused && i.access.is_some());
    // A memo's shared children are skipped whole when none is a focused field.
    let skip = el.memo_children().filter(|m| !m.focused_input).map_or(0, |m| m.children.len());
    if input.is_none() && el.children.is_empty() && skip == el.memo_children().map_or(0, |m| m.children.len()) {
        return;
    }
    let segment = el.key.clone().unwrap_or_else(|| format!("#{index}"));
    let path = format!("{parent}/{segment}");
    if let Some(input) = input {
        let mut input = input.clone();
        if inert { input.edit.focused = false; }
        inputs.insert(path.clone(), input);
    }
    for (i, child) in el.all_children().enumerate().skip(skip) {
        collect_virtual_inputs(child, &path, i, inert, inputs);
    }
}

impl<S: 'static> Runtime<S> {
    pub(super) fn measure(&mut self, tx: &Tx) -> Measured {
        self.measure_pass(tx, true)
    }

    pub(super) fn measure_retry(&mut self, tx: &Tx) -> Measured {
        let _blend = crate::theme::scope(self.srgb_blend);
        let _bars = crate::platform::scroll_bars::scope(self.scroll_bars.state, self.scroll_bars.runtime_id);
        let selection = self.selection_by_path();
        Measured { selection, ..self.measurement(true).finish_from(tx, 1) }
    }

    /// `committing` when the result is applied, so the last commit's text layouts can go.
    pub(super) fn measure_pass(&mut self, tx: &Tx, committing: bool) -> Measured {
        let _blend = crate::theme::scope(self.srgb_blend);
        let _bars = crate::platform::scroll_bars::scope(self.scroll_bars.state, self.scroll_bars.runtime_id);
        let selection = self.selection_by_path();
        Measured { selection, ..self.measurement(committing).finish(tx) }
    }

    pub(super) fn measurement(&mut self, committing: bool) -> Measurement<'_, S> {
        let hold_windows = self.checking_live_only();
        Measurement {
            state: &self.state,
            view: self.view,
            playground: self.playground.as_deref(),
            env: self.env,
            viewport: self.viewport,
            fit_height: self.fit_height,
            system_reduced_motion: self.system_reduced_motion,
            titlebar_inset: self.titlebar_inset,
            chrome: self.titlebar.insets,
            caption: self.titlebar.caption,
            system_accent: self.system_accent,
            system_scroll_bars: self.scroll_bars.state,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            defaults: self.defaults,
            titlebar: self.titlebar_set(),
            caret: self.caret.clone(),
            caret_read: false,
            layout_seen: self.layout_seen.rects.clone(),
            layout_reads: Vec::new(),
            commits: self.commits,
            nodes: &mut self.nodes,
            committing,
            spare_nodes: &mut self.spare_nodes,
            text: &mut self.text,
            layout: &mut self.layout,
            node_index: &mut self.node_index,
            path_ids: &mut self.path_ids,
            memo: &mut self.memo,
            next_id: &mut self.next_id,
            scrolls: &mut self.scrolls,
            virtuals: &mut self.virtuals,
            virtual_culled: &mut self.virtual_culled,
            virtual_inputs: &mut self.virtual_inputs,
            stats: &mut self.stats,
            key_warnings: &mut self.key_warnings,
            focused: self.focus.path(),
            state_unchanged: std::mem::take(&mut self.state_unchanged),
            retry: false,
            hold_windows,
        }
    }

    /// Forgets the ids of paths that left with nothing kept under their id, so a session that
    /// mounts many distinct keys (a long virtual list scrolled end to end) doesn't keep them all.
    /// A path whose id still holds scroll, growth or reveal state keeps it, so it comes back
    /// with that state as before.
    pub(super) fn prune_path_ids(&mut self) {
        if self.path_ids.len() <= self.node_index.len() { return; }
        let Self { path_ids, node_index, scrolls, stacks, grows, reveals, .. } = self;
        path_ids.retain(|_, id| {
            node_index.contains_key(id) || scrolls.contains_key(id) || stacks.contains_key(id) || grows.contains_key(id) || reveals.contains_key(id)
        });
    }
}

impl<S: 'static> Measurement<'_, S> {
    pub(super) fn node_id(&mut self, path: &str) -> NodeId {
        if let Some(&id) = self.path_ids.get(path) { return id; }
        let id = NodeId(*self.next_id);
        *self.next_id += 1;
        self.path_ids.insert(path.to_owned(), id);
        id
    }

    /// Finishes all row measurements before scene/growth mutation. Newly measured estimates
    /// above the viewport adjust the scroll offset by exactly the same amount.
    pub(super) fn measure_virtuals(&mut self, nodes: &[Node], settle: bool) -> bool {
        let mut retry = false;
        for (path, v) in self.virtuals.iter_mut() {
            let Some((ci, _)) = nodes.iter().enumerate().find(|(_, n)| n.path == *path) else { continue };
            for n in nodes.iter().filter(|n| n.parent == Some(ci)) {
                v.set_height(n.key.as_deref().unwrap(), n.local.h);
            }
            let Some(scroll) = nodes.iter().find(|n| n.path == v.scroll_path) else { continue };
            v.viewport = scroll.local.h;
            v.top = (nodes[ci].target[1] - scroll.target[1]).max(0.0);
            let st = self.scrolls.get_mut(&scroll.id);
            let sticky = scroll.scroll.unwrap().stick_bottom;
            let pinned = (settle && sticky) || st.as_ref().map_or(sticky, |s| s.pinned());
            let offset = if let Some(st) = st {
                if !pinned { st.offset = (st.offset + f64::from(v.anchor_delta())).max(0.0); }
                st.offset as f32
            } else { 0.0 };
            // Rows that shrank or went leave the offset past the end until scrolling clamps it;
            // what shows then is the end.
            let offset = (offset - v.top).clamp(0.0, v.max_offset());
            let end = v.row_end(v.range.end);
            let mounted_height = (end - v.before).max(0.0);
            // Never present a blank caused by an estimate. Expand and measure again first.
            retry |= if pinned {
                v.range.start > 0 && mounted_height < v.viewport
            } else {
                (v.range.start > 0 && offset < v.pad_top + v.before)
                    || (v.range.end < v.keys.len() && offset + v.viewport > v.pad_top + end)
            };
        }
        retry
    }

    /// Checks the view's keys and builds the virtual windows, index-built rows included.
    pub(super) fn build_tree(&mut self, mut root: El, tx: &Tx, force_all: bool) -> El {
        // Two siblings with one key would share a layout node, which Taffy panics on. Rename the
        // later ones so every build, debug or release, lays out; --tree still lists them.
        let duplicates = crate::input::key_diagnostics::repair(&mut root);
        match self.key_warnings.as_mut() {
            Some(warnings) => *warnings = duplicates.into_iter().map(|d| d.message).collect(),
            None => crate::input::key_diagnostics::warn_once(&duplicates),
        }
        // Index-built rows build here, so memos inside them share the view's cache. A window that
        // fits its content lays its root out at its natural height, which nothing bounds.
        let bound = if self.fit_height { f32::INFINITY } else { self.viewport.1 };
        self.window_tree(&mut root, "", 0, None, tx, force_all, bound, false, None);
        root
    }

    /// The app's view, or with the playground attached, the view it composes (the bar, or the
    /// playground around its stage).
    fn root_view(&self, cx: &mut Cx<S>) -> El {
        let root = match self.playground {
            Some(playground) => playground.view(self.state, self.view, cx, self.viewport),
            None => crate::playground::call(self.view, self.state, cx),
        };
        // The Windows caption buttons go over everything the view drew, in a frame around it.
        let Some(caption) = &self.caption else { return root };
        let symbol = cx.caption_color.unwrap_or(if self.env.system_dark { Color::WHITE } else { Color::BLACK });
        crate::titlebar::frame(root, caption, symbol, self.env.scale, self.fit_height)
    }

    /// What a memo scope hashes of the window's controls (`Cx::titlebar_inset`,
    /// `Cx::chrome_insets`), so memos that read them rebuild when they change.
    pub(super) fn chrome_bits(&self) -> [u32; 4] {
        let [leading, trailing, height] = self.chrome.bits();
        [self.titlebar_inset.to_bits(), leading, trailing, height]
    }

    pub(super) fn finish(self, tx: &Tx) -> Measured {
        self.finish_from(tx, 0)
    }

    pub(super) fn finish_from(mut self, tx: &Tx, mut passes: usize) -> Measured {
        let (layout, nodes, memo_pending, memo_reused, taken_paths) = loop {
            passes += 1;
            self.retry = passes > 1;
            self.virtual_culled.clear();
            self.virtual_inputs.clear();
            // Degenerate zero-height runs fall back to a full measurement instead of
            // an unbounded retry loop. Normal transcripts need one pass.
            let (layout, mut nodes, pending, mut reused, mut taken) = self.measure_view(tx, passes > 8);
            if !self.measure_virtuals(&nodes, tx.settle) { break (layout, nodes, pending, reused, taken); }
            memo_reuse::restore_moved_nodes(self.nodes, &mut nodes, &reused, &taken);
            nodes.clear();
            *self.spare_nodes = nodes;
            reused.clear();
            taken.clear();
            self.memo.pending_scratch = pending;
            self.memo.reused_scratch = reused;
            self.memo.taken_paths_scratch = taken;
            *self.layout = layout;
        };
        // Exiting virtual subtrees still use their origin and measured extent while
        // fading. Runtime prunes this metadata with the retained scene nodes.
        Measured {
            layout,
            nodes,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            defaults: self.defaults,
            titlebar: self.titlebar,
            caret_read: self.caret_read,
            layout_reads: self.layout_reads,
            memo_flattened: Some(memo_pending),
            memo_reused,
            taken_paths,
            selection: Vec::new(),
        }
    }

    /// The last commit's node at `path`.
    fn committed_node(&self, path: &str) -> Option<usize> {
        // The index can describe a retried pass's nodes rather than the last commit's: check.
        let id = *self.path_ids.get(path)?;
        let at = self.node_index.get(&id).copied().filter(|&i| self.nodes.get(i).is_some_and(|n| n.id == id));
        at.or_else(|| self.nodes.iter().position(|n| n.path == path))
    }

    /// `bound` is the most `el` can be tall (see [`crate::el::virtual_list::height_bound`]); `scroll`
    /// is the scroll viewport `el` is the content of, whether it sticks to the bottom, and its bound.
    #[allow(clippy::too_many_arguments)]
    fn window_tree(&mut self, el: &mut El, parent: &str, index: usize, scroll: Option<(&str, bool, f32)>, tx: &Tx, force_all: bool, bound: f32, inert: bool, group: Option<&str>) {
        if !el.flags.has(ElFlags::HAS_VIRTUAL) {
            return;
        }
        el.expand_memo();
        let inert = inert || el.flags.has(ElFlags::INERT);
        let bound = crate::el::virtual_list::height_bound(&el.style, bound);
        let path = node_path(parent, el.key.as_deref(), index);
        if let Some(estimate) = el.cold.virtual_estimate {
            assert!(el.cold.growth.is_some(), "`.virtualize(..)` at {path:?} needs `.growth(GrowthSpec)` on the same element");
            let (scroll_path, sticky, scroll_bound) = scroll.expect("virtual stack must be the direct child of a scroll viewport");
            let old_scroll = self.path_ids.get(scroll_path).and_then(|id| self.scrolls.get(id));
            let pinned = (tx.settle && sticky) || old_scroll.map_or(sticky, |s| s.pinned());
            let mut offset = old_scroll.map_or(0.0, |s| s.offset as f32);
            // A new viewport has no height yet: its bound from the tree, else the window's.
            let first = if scroll_bound.is_finite() { scroll_bound } else { self.viewport.1 };
            let height = self.committed_node(scroll_path).map_or(first, |i| self.nodes[i].local.h);
            let padding = |p: LengthPercentage| match p.expand() {
                taffy::style::ExpandedLengthPercentage::Length(x) => x,
                _ => panic!("virtual stack padding must be in points"),
            };
            let list = self.committed_node(&path);
            let v = self.virtuals.entry(path.clone()).or_default();
            v.set_estimate(estimate);
            v.scroll_path = scroll_path.to_owned();
            // A retry keeps the viewport the last pass measured; `self.nodes` is the last commit's.
            if !self.retry { v.viewport = height; }
            offset = (offset - v.top).max(0.0);
            let el_pad = el.style.padding();
            v.pad_top = padding(el_pad.top);
            v.pad_bottom = padding(el_pad.bottom);
            v.set_gap(crate::el::virtual_list::gap_y(&el.style));
            let source = el.cold.get_mut_if_some().and_then(|c| c.virtual_rows.take());
            let state = playground_host::handler_states_ref(self.state, self.playground);
            if source.is_some() {
                assert!(el.children.is_empty(), "`.virtual_rows(..)` at {path:?} builds its own rows; it takes no children");
            }
            v.indexed = source.is_some();
            let version = el.cold.virtual_rows_version.filter(|_| source.is_some());
            let same = match &source {
                Some(rows) => rows.count == v.keys.len()
                    && (self.state_unchanged || (version.is_some() && version == v.keys_version) || (0..rows.count).all(|i| (rows.key)(state, i) == v.keys[i])),
                None => v.same_keys(el.children.iter().map(|c| c.key.as_deref())),
            };
            v.keys_version = version;
            // Most commits keep the same rows; only a change pays for sets of every key.
            let anchor = if same {
                if pinned { None } else { v.first_visible(offset, |_| true) }
            } else {
                let mut keys: Vec<_> = match &source {
                    Some(rows) => (0..rows.count).map(|i| (rows.key)(state, i).into_owned()).collect(),
                    None => el.children.iter().map(|c| c.key.clone().expect("virtual rows require stable keys")).collect(),
                };
                // Rows sharing a key would share a height and a node: rename the later ones, as
                // the view's siblings are, so the list still lays out.
                let renamed = crate::input::key_diagnostics::rename_duplicates(keys.iter_mut());
                if !renamed.is_empty() {
                    let duplicates: Vec<_> = renamed.iter().map(|(_, key)| {
                        let key = key.clone();
                        let message = format!("duplicate virtual row key {key:?} under {path:?}; give rows unique keys");
                        crate::input::key_diagnostics::Duplicate { key, message }
                    }).collect();
                    if source.is_none() {
                        for (i, _) in &renamed { el.children[*i].key = Some(keys[*i].clone()); }
                    }
                    match self.key_warnings.as_mut() {
                        Some(out) => out.extend(duplicates.into_iter().map(|d| d.message)),
                        None => crate::input::key_diagnostics::warn_once(&duplicates),
                    }
                }
                // A reorder keeps each row where it was drawn, by its offset or by sliding it
                // (`row_reorder.rs`). Anchoring to the formerly first row as it changes slots
                // would scroll the entire list a second time. Inserts/removals still preserve
                // the reader's anchor, and so does a reorder that lands at once.
                let slides = !tx.instant && !tx.settle && !tx.relayout && !self.reduced_motion;
                let reorder = keys.len() == v.keys.len() && keys.iter().all(|key| v.has(key))
                    && (slides || list.is_some_and(|list| self.nodes.iter().any(|n| n.vis.offset != [0.0; 2] && n.parent == Some(list))));
                v.set_keys(keys, self.commits, (!pinned && !reorder).then_some(offset))
            };
            v.anchor = anchor;
            let delta = v.anchor_delta();
            offset = (offset + delta).max(0.0);
            if let Some(st) = self.path_ids.get(scroll_path).and_then(|id| self.scrolls.get_mut(id)) {
                if !pinned { st.offset = (st.offset + f64::from(delta)).max(0.0); }
            }
            // Rows new since the last commit spread over frames, unless this pass is a retry to
            // fill the viewport or a settle. A list's first showing is all new rows: it mounts
            // its viewport and a budget of overscan, and the rest fills over the next frames.
            let budget = v.indexed && !self.retry && !tx.settle;
            let shown = std::mem::take(&mut v.shown);
            // A live-only check's commit keeps the rows mounted now while they still cover the
            // viewport, as skipping the commit would: re-picking the overscan for a viewport that
            // changed since would build rows the handler had nothing to do with.
            if self.hold_windows && same && !self.retry && !force_all && !v.needs_window(offset, pinned) {
                v.hold(offset, pinned);
            } else {
                v.select(offset, pinned, budget.then_some(&shown));
            }
            if force_all { v.range = 0..v.keys.len(); v.before = 0.0; v.after = 0.0; }
            // Keys are unique, so a key is mounted when its row's index is in the range.
            for key in shown.iter().filter(|k| v.index_of(k).is_some_and(|i| !v.range.contains(&i))) {
                self.virtual_culled.insert(format!("{path}/{key}"));
            }
            v.shown = shown;
            el.style.set_pad_top(v.pad_top + v.before);
            el.style.set_pad_bottom(v.pad_bottom + v.after);
            self.populate_virtual_children(el, &path, source, inert, group);
        }
        let bounds = crate::el::virtual_list::child_bounds(&el.style, el.cold.scroll.is_some(), bound, &el.children);
        let scroll = el.cold.scroll.map(|s| (path.as_str(), s.stick_bottom, bound));
        let child_group = if el.flags.has(ElFlags::SELECTION_GROUP) { Some(path.as_str()) } else { group };
        for (i, child) in el.children.iter_mut().enumerate() {
            if !child.flags.has(ElFlags::HAS_VIRTUAL) { continue; }
            let bound = bounds.of(child);
            self.window_tree(child, &path, i, scroll, tx, force_all, bound, inert, child_group);
        }
    }

    fn measure_view(&mut self, tx: &Tx, force_all: bool) -> (LayoutEngine, Vec<Node>, Vec<memo_reuse::PendingFlat>, Vec<memo_reuse::ReusedSpan>, Vec<(u32, u32)>) {
        let scale = self.env.scale;
        let fresh_cx = |this: &Self| Cx::<S> {
            env: this.env,
            titlebar_inset: this.titlebar_inset,
            chrome: this.chrome,
            caption_color: None,
            titlebar: Default::default(),
            system_accent: this.system_accent,
            system_scroll_bars: this.system_scroll_bars,
            system_reduced_motion: this.system_reduced_motion,
            reduced_motion: this.system_reduced_motion,
            time_scale: this.time_scale,
            defaults: Default::default(),
            caret: this.caret.clone(),
            caret_read: std::cell::Cell::new(false),
            layout: this.layout_seen.clone(),
            layout_reads: Default::default(),
            _s: PhantomData,
        };
        let mut cx = fresh_cx(self);
        #[cfg(feature = "hot")]
        let emptied = self.memo.emptied();
        let memo = crate::memo::enter(std::mem::take(self.memo), self.env, self.system_accent, self.system_reduced_motion, crate::theme::blends_srgb(), self.chrome_bits(), cx.caret.clone(), cx.layout.clone());
        let view_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::View);
        #[cfg(not(feature = "hot"))]
        let mut root = { let root = self.root_view(&mut cx); self.build_tree(root, tx, force_all) };
        // Index-built rows run the app's closures in `build_tree`, so a patch's panic there
        // falls back like one in the view.
        #[cfg(feature = "hot")]
        let (mut root, memo) = {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let root = self.root_view(&mut cx);
                self.build_tree(root, tx, force_all)
            })) {
                Ok(root) => (root, memo),
                Err(_) => {
                    eprintln!("rune-hot: view panicked; rendering the original view");
                    // The failed build's memo scope, cache and context writes go; the startup
                    // view builds from scratch against the retained state.
                    drop(memo);
                    self.virtual_culled.clear();
                    self.virtual_inputs.clear();
                    cx = fresh_cx(self);
                    let memo = crate::memo::enter(emptied, self.env, self.system_accent, self.system_reduced_motion, crate::theme::blends_srgb(), self.chrome_bits(), cx.caret.clone(), cx.layout.clone());
                    let root = crate::hot::original_only(|| self.root_view(&mut cx));
                    (crate::hot::original_only(|| self.build_tree(root, tx, force_all)), memo)
                }
            }
        };
        // Both paths finish the memo here, after the rows are built.
        let (memo, replayed) = memo.finish();
        drop(view_time);
        *self.memo = memo;
        self.reduced_motion = cx.reduced_motion;
        self.time_scale = cx.time_scale;
        self.defaults = cx.defaults;
        self.titlebar = cx.titlebar;
        // Reads made by memos this build reused count as made again.
        self.caret_read = cx.caret_read.get() || replayed.caret;
        self.layout_reads = cx.layout_reads.take();
        for read in replayed.layout { layout_read::note_read_at(&mut self.layout_reads, &read.key, read.same, read.at); }

        let mut taffy = std::mem::replace(self.layout, LayoutEngine::with_capacity(0));
        taffy.begin_commit();
        let mut nodes = std::mem::take(self.spare_nodes);
        nodes.clear();
        nodes.reserve(self.nodes.len().max(32));
        // Root constraints are part of the retained style, not a dirty write after flattening.
        // A window that fits its content lays the root out at its natural height.
        root.style.set_size(relayout::root_size(self.viewport, self.fit_height));
        let (pending, reused, taken_paths, root_id) = {
            let mut pending = std::mem::take(&mut self.memo.pending_scratch);
            pending.clear();
            let mut reused = std::mem::take(&mut self.memo.reused_scratch);
            reused.clear();
            let mut taken_paths = std::mem::take(&mut self.memo.taken_paths_scratch);
            taken_paths.clear();
            let mut path_buf = std::mem::take(&mut self.memo.path_scratch);
            path_buf.clear();
            let mut memo_ctx = memo_reuse::MemoFlattenCtx {
                old_nodes: &mut *self.nodes,
                old_present_len: self.memo.present_len,
                old_node_index: &*self.node_index,
                old_cursor: 0,
                committing: self.committing,
                committed: &self.memo.flattened,
                pending,
                reused,
                taken_paths,
                path_ids: &mut *self.path_ids,
                next_id: &mut *self.next_id,
                foreground: self.defaults.foreground,
            };
            let (root_id, _) = flatten_with_buf(root, None, &mut path_buf, 0, motion::DEFAULT, RowLayout::Free, true, &mut taffy, &mut nodes, scale, &mut memo_ctx);
            let (pending, reused, taken_paths) = (memo_ctx.pending, memo_ctx.reused, memo_ctx.taken_paths);
            path_buf.clear();
            self.memo.path_scratch = path_buf;
            (pending, reused, taken_paths, root_id)
        };
        if self.committing { release_kept_layouts(self.nodes, &nodes, &taffy, self.text, &reused); }
        taffy.finish_flatten();
        let (taffy, nodes) = self.solve(taffy, root_id, nodes);
        (taffy, nodes, pending, reused, taken_paths)
    }

    pub(super) fn populate_virtual_children(
        &mut self,
        el: &mut El,
        path: &str,
        source: Option<Rc<crate::el::virtual_rows::RowSource>>,
        inert: bool,
        group: Option<&str>,
    ) {
        let state = playground_host::handler_states_ref(self.state, self.playground);
        let v = self.virtuals.get_mut(path).unwrap();
        let list_group = if el.flags.has(ElFlags::SELECTION_GROUP) { Some(path.to_owned()) } else { group.map(str::to_owned) };
        v.selection.group_path = list_group.clone();
        v.selection.inert = inert;
        v.selection.texts.clear();
        v.selection.copy_providers.clear();
        v.selection.unmounted.clear();
        if let Some(rows) = source {
            let quiet: Vec<bool> = v.range.clone().map(|i| v.mounts_quietly(i, self.commits)).collect();
            let key_warnings = &mut *self.key_warnings;
            let keys = &v.keys;
            let mut build = |i: usize| {
                let key = &keys[i];
                let tag = crate::memo::virtual_row_tag(path, key);
                let mut row = crate::memo::in_virtual_row(tag, || (rows.build)(state, i)).key(key.clone());
                // The view's key repair ran before these rows existed.
                if crate::input::key_diagnostics::clean(&row) { return row; }
                let duplicates = crate::input::key_diagnostics::repair_at(&mut row, &format!("{path}/{key}"));
                match key_warnings.as_mut() {
                    Some(out) => out.extend(duplicates.into_iter().map(|d| d.message)),
                    None => crate::input::key_diagnostics::warn_once(&duplicates),
                }
                row
            };
            // A focused field's row and a selection's pinned end rows are built unmounted.
            let under = |f: &str| f.strip_prefix(path).is_some_and(|rest| rest.starts_with('/'));
            let focused = v.focused_row.as_deref().filter(|_| self.focused.is_some_and(under))
                .and_then(|key| v.index_of(key)).filter(|i| !v.range.contains(i));
            let pinned: Vec<usize> = v.pinned_rows.iter()
                .filter_map(|key| v.index_of(key))
                .filter(|i| !v.range.contains(i))
                .collect();
            if let Some(i) = focused {
                let row = build(i);
                collect_virtual_inputs(&row, path, i, inert, self.virtual_inputs);
                if pinned.contains(&i) {
                    selectable::virtual_selection::collect_virtual_row_selection(
                        &row, path, &v.keys[i], i, list_group.as_deref(), inert,
                        &mut v.selection.texts, &mut v.selection.copy_providers,
                    );
                }
            }
            for i in pinned {
                if Some(i) == focused { continue; }
                let row = build(i);
                selectable::virtual_selection::collect_virtual_row_selection(
                    &row, path, &v.keys[i], i, list_group.as_deref(), inert,
                    &mut v.selection.texts, &mut v.selection.copy_providers,
                );
            }
            el.children = v.range.clone().zip(quiet).map(|(i, quiet)| {
                let mut row = build(i);
                row.flags.set(ElFlags::VIRTUAL_MOUNT, quiet);
                row
            }).collect();
            v.selection.source = Some(rows);
        } else {
            v.selection.source = None;
            if !self.retry && el.cold.focus.reveal_selected {
                let selected = el.children.iter().find(|c| c.cold.focus.selected).and_then(|c| c.key.clone());
                if selected != v.selected_row {
                    v.reveal_selected = selected.clone();
                    v.selected_row = selected;
                }
            }
            #[cfg(debug_assertions)]
            crate::tooling::perf_warn::overbuilt(path, el.cold.virtual_source, el.children.len(), v.range.len());
            let pinned: Vec<usize> = v.pinned_rows.iter()
                .filter_map(|key| v.index_of(key))
                .filter(|i| !v.range.contains(i))
                .collect();
            el.children = std::mem::take(&mut el.children).into_iter().enumerate().filter_map(|(i, mut child)| {
                if !v.range.contains(&i) {
                    collect_virtual_inputs(&child, path, i, inert, self.virtual_inputs);
                    if pinned.contains(&i) {
                        selectable::virtual_selection::collect_virtual_row_selection(
                            &child, path, &v.keys[i], i, list_group.as_deref(), inert,
                            &mut v.selection.texts, &mut v.selection.copy_providers,
                        );
                    }
                    v.selection.unmounted.insert(i, child);
                    return None;
                }
                child.flags.set(ElFlags::VIRTUAL_MOUNT, v.mounts_quietly(i, self.commits));
                Some(child)
            }).collect();
        }
    }
}

impl<S: 'static> Runtime<S> {
    /// Records which row of each virtual list holds focus, by key, so the row is still built
    /// after it scrolls out. Walks the node tree, since a key may itself contain `/`.
    pub(super) fn track_focused_rows(&mut self) {
        if self.virtuals.is_empty() { return; }
        let focused = self.focus.path().and_then(|f| self.nodes.iter().position(|n| n.path == f));
        for (path, v) in self.virtuals.iter_mut() {
            let Some(rest) = self.focus.path().and_then(|f| f.strip_prefix(path.as_str())).and_then(|r| r.strip_prefix('/')) else {
                v.focused_row = None;
                continue;
            };
            if let Some(mut i) = focused {
                while let Some(p) = self.nodes[i].parent {
                    if self.nodes[p].path == *path {
                        v.focused_row = self.nodes[i].key.clone();
                        break;
                    }
                    i = p;
                }
            } else {
                let matches = |k: &str| rest == k || rest.strip_prefix(k).is_some_and(|t| t.starts_with('/'));
                if !v.focused_row.as_deref().is_some_and(matches) {
                    v.focused_row = v.keys.iter().find(|k| matches(k)).cloned();
                }
            }
        }
    }
}
