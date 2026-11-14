//! On-demand unbuilt virtual-row building, copy collection, ⌘A span resolution, and programmatic
//! selection requests on unbuilt rows.
use super::{
    EndRef, MAX_VIRTUAL_COPY_BYTES, MAX_VIRTUAL_COPY_ROWS, OffscreenText,
    collect_virtual_row_selection, offscreen_piece,
};
use crate::el::{SelectRequest, SelectionCopy};
use crate::runtime::selectable::{CopyJoin, SelectedText, TextSelection, Unit, floor_boundary};
use crate::runtime::{Node, Runtime, playground_host};

impl<S: 'static> Runtime<S> {
    pub(crate) fn copy_virtual_text(&self) -> Option<String> {
        let h = self.selection_holder_end()?;
        let f = self.focus_holder_end(h)?;
        let node_for = |e: EndRef| match e {
            EndRef::Mounted(i) => Some(i),
            EndRef::Offscreen(vi, ti) => self.live(&self.offscreen_text(vi, ti).list_path),
        };
        let (n0, n1) = (node_for(h)?, node_for(f)?);
        let around = |a: usize, j: usize| std::iter::successors(Some(j), |&p| self.nodes[p].parent).any(|p| p == a);
        let provider = std::iter::successors(Some(n0), |&p| self.nodes[p].parent)
            .find(|&p| self.nodes[p].handlers.selection_copy.is_some() && around(p, n1));
        if let Some(p) = provider
            && let Some(copy_fn) = self.nodes[p].handlers.selection_copy.clone()
        {
            let sel = self.collect_virtual_selection(Some(p))?;
            if let Some(text) = copy_fn(&sel) {
                return Some(text);
            }
            return Some(sel.text);
        }
        self.collect_virtual_selection(None).map(|s| s.text)
    }

    /// Builds an unmounted row `r` of `list_path` on demand (view only, no layout/mount/GPU) and
    /// collects its selectable texts.
    pub(super) fn build_unmounted_row(
        &self,
        list_path: &str,
        r: usize,
        texts: &mut Vec<OffscreenText>,
        providers: &mut Vec<(String, SelectionCopy)>,
    ) {
        let Some(v) = self.virtuals.get(list_path) else { return };
        let Some(key) = v.keys.get(r) else { return };
        if let Some(rows) = &v.selection.source {
            let state = playground_host::handler_states_ref(&self.state, self.playground.as_deref());
            let mut row = (rows.build)(state, r).key(key.clone());
            if !crate::input::key_diagnostics::clean(&row) {
                let _ = crate::input::key_diagnostics::repair_at(&mut row, &format!("{list_path}/{key}"));
            }
            collect_virtual_row_selection(
                &row, list_path, key, r,
                v.selection.group_path.as_deref(), v.selection.inert,
                texts, providers,
            );
        } else if let Some(row) = v.selection.unmounted.get(&r) {
            collect_virtual_row_selection(
                row, list_path, key, r,
                v.selection.group_path.as_deref(), v.selection.inert,
                texts, providers,
            );
        }
    }

    /// Pins row `r` of `list_path` in `v.pinned_rows` and ensures its offscreen texts are built in
    /// `v.selection.texts`.
    pub(crate) fn pin_offscreen_row(&mut self, list_path: &str, r: usize) {
        let Some(v) = self.virtuals.get(list_path) else { return };
        if v.range.contains(&r) || r >= v.keys.len() {
            return;
        }
        let row_key = v.keys[r].clone();
        let already = v.selection.texts.iter().any(|t| t.row_key == row_key);
        let (mut texts, mut providers) = (Vec::new(), Vec::new());
        if !already {
            self.build_unmounted_row(list_path, r, &mut texts, &mut providers);
        }
        if let Some(v) = self.virtuals.get_mut(list_path) {
            if !v.pinned_rows.contains(&row_key) {
                v.pinned_rows.push(row_key);
            }
            if !already {
                v.selection.texts.extend(texts);
                v.selection.copy_providers.extend(providers);
            }
        }
    }

    /// Collects the full [`TextSelection`] when the selection touches offscreen or unbuilt rows,
    /// bounded by [`MAX_VIRTUAL_COPY_ROWS`] and [`MAX_VIRTUAL_COPY_BYTES`].
    pub(crate) fn collect_virtual_selection(&self, base: Option<usize>) -> Option<TextSelection> {
        let h = self.selection_holder_end()?;
        let f = self.focus_holder_end(h)?;
        let sel = &self.text_sel;
        let base_path = base.map(|b| self.nodes[b].path.as_str());

        if h == f {
            let (a, b) = (sel.anchor.min(sel.focus), sel.anchor.max(sel.focus));
            return match h {
                EndRef::Mounted(i) => {
                    if a >= b || b > self.content_of(i).len() { return None; }
                    let piece = self.selected_piece(i, a..b, base);
                    let text = piece.text().to_owned();
                    Some(TextSelection { texts: vec![piece], text, backward: self.selection_backward() })
                }
                EndRef::Offscreen(vi, ti) => {
                    let t = self.offscreen_text(vi, ti);
                    if a >= b || b > t.spec.content.len() { return None; }
                    let piece = offscreen_piece(t, a..b, base_path);
                    let text = piece.text().to_owned();
                    Some(TextSelection { texts: vec![piece], text, backward: self.selection_backward() })
                }
            };
        }

        let ((first_end, first_byte), (last_end, last_byte)) = if self.tree_order(h) < self.tree_order(f) {
            ((h, sel.anchor), (f, sel.focus))
        } else {
            ((f, sel.focus), (h, sel.anchor))
        };
        let (first_ord, last_ord) = (self.tree_order(first_end), self.tree_order(last_end));
        let g_path = self.end_group_path(h)?;

        // Both ends in the same offscreen row (e.g. a row with its own `selection_group`).
        if let (EndRef::Offscreen(vi0, ti0), EndRef::Offscreen(vi1, ti1)) = (first_end, last_end)
            && vi0 == vi1
        {
            let (_, v) = self.virtuals.iter().nth(vi0)?;
            let row_key = &v.selection.texts[ti0].row_key;
            if self.offscreen_text(vi1, ti1).row_key == *row_key {
                let (mut texts, mut out) = (Vec::new(), CopyJoin::default());
                for (ti, t) in v.selection.texts.iter().enumerate() {
                    if &t.row_key != row_key || t.group_path.as_deref() != Some(g_path) { continue; }
                    let end_ref = EndRef::Offscreen(vi0, ti);
                    let ord = self.tree_order(end_ref);
                    if ord < first_ord || ord > last_ord { continue; }
                    let len = t.spec.content.len();
                    let a = if end_ref == first_end { first_byte.min(len) } else { 0 };
                    let b = if end_ref == last_end { last_byte.min(len) } else { len };
                    if a >= b { continue; }
                    let piece = offscreen_piece(t, a..b, base_path);
                    let hit_cap = out.push(&t.spec, a..b, piece.text(), t.tab_before, MAX_VIRTUAL_COPY_BYTES);
                    texts.push(piece);
                    if hit_cap { break; }
                }
                let text = out.finish();
                return (!text.is_empty()).then_some(TextSelection { texts, text, backward: self.selection_backward() });
            }
        }

        // General group selection spanning a virtual list.
        let list_path = self.virtual_end_of(first_end, 0)
            .or_else(|| self.virtual_end_of(last_end, 0))
            .map(|e| e.list_path)
            .or_else(|| {
                let g = self.live(g_path)?;
                let inside = |j: usize| j == g || std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == g);
                (g..self.nodes.len())
                    .take_while(|&j| inside(j))
                    .find_map(|j| self.virtuals.contains_key(&self.nodes[j].path).then(|| self.nodes[j].path.clone()))
            })?;
        let list_node = self.live(&list_path)?;
        let v = self.virtuals.get(&list_path)?;
        let g = self.live(g_path)?;

        let (mut texts, mut out) = (Vec::new(), CopyJoin::default());
        let (mut unbuilt_rows, mut prev_mounted, mut hit_cap) = (0usize, None, false);

        let append_mounted = |j: usize, out: &mut CopyJoin, texts: &mut Vec<SelectedText>, prev_mounted: &mut Option<usize>| -> bool {
            let end_ref = EndRef::Mounted(j);
            let ord = self.tree_order(end_ref);
            if ord < first_ord || ord > last_ord { return false; }
            let len = self.content_of(j).len();
            let a = if end_ref == first_end { first_byte.min(len) } else { 0 };
            let b = if end_ref == last_end { last_byte.min(len) } else { len };
            if a >= b { return false; }
            let piece = self.selected_piece(j, a..b, base);
            let Some(spec) = self.nodes[j].text.as_ref() else { return false };
            let beside = prev_mounted.is_some_and(|p| self.beside(p, j));
            let capped = out.push(spec, a..b, piece.text(), beside, MAX_VIRTUAL_COPY_BYTES);
            texts.push(piece);
            *prev_mounted = Some(j);
            capped
        };

        // 1. Mounted group members before `list_node`.
        let inside_g = |j: usize| std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == g);
        for j in (g + 1..list_node).filter(|&j| inside_g(j) && self.member(j, g)) {
            if append_mounted(j, &mut out, &mut texts, &mut prev_mounted) {
                hit_cap = true;
                break;
            }
        }

        // 2. Rows of the virtual list in `[row_start..=row_end]`.
        if !hit_cap && !v.keys.is_empty() {
            let row_start = self.virtual_end_of(first_end, 0)
                .filter(|e| e.list_path == list_path)
                .map_or(0, |e| e.row_index);
            let row_end = self.virtual_end_of(last_end, 0)
                .filter(|e| e.list_path == list_path)
                .map_or(v.keys.len() - 1, |e| e.row_index.min(v.keys.len() - 1));

            let inside_list = |j: usize| std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == list_node);
            let list_end = (list_node + 1..self.nodes.len()).take_while(|&j| inside_list(j)).last().unwrap_or(list_node);

            for r in row_start..=row_end {
                if v.range.contains(&r) {
                    let row_path = format!("{list_path}/{}", v.keys[r]);
                    let Some(row_node) = (list_node + 1..=list_end)
                        .find(|&j| self.nodes[j].parent == Some(list_node) && self.nodes[j].path == row_path)
                    else { continue };
                    let inside_row = |j: usize| {
                        j == row_node || std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == row_node)
                    };
                    for j in (row_node..=list_end).take_while(|&j| inside_row(j)).filter(|&j| self.member(j, g)) {
                        if append_mounted(j, &mut out, &mut texts, &mut prev_mounted) {
                            hit_cap = true;
                            break;
                        }
                    }
                    if hit_cap { break; }
                } else {
                    prev_mounted = None;
                    let row_key = &v.keys[r];
                    let mut built_texts = Vec::new();
                    let row_texts: Vec<&OffscreenText> = if v.selection.texts.iter().any(|t| &t.row_key == row_key) {
                        v.selection.texts.iter().filter(|t| &t.row_key == row_key).collect()
                    } else {
                        if unbuilt_rows >= MAX_VIRTUAL_COPY_ROWS || out.text().len() >= MAX_VIRTUAL_COPY_BYTES {
                            hit_cap = true;
                            break;
                        }
                        unbuilt_rows += 1;
                        let mut ignored = Vec::new();
                        self.build_unmounted_row(&list_path, r, &mut built_texts, &mut ignored);
                        built_texts.iter().collect()
                    };
                    let mut first_in_row = true;
                    for t in row_texts {
                        if t.group_path.as_deref() != Some(g_path) { continue; }
                        let ord = (list_node * 2 + 1, r, t.order_in_row);
                        if ord < first_ord || ord > last_ord { continue; }
                        let len = t.spec.content.len();
                        let a = if ord == first_ord { first_byte.min(len) } else { 0 };
                        let b = if ord == last_ord { last_byte.min(len) } else { len };
                        if a >= b { continue; }
                        let piece = offscreen_piece(t, a..b, base_path);
                        let capped = out.push(&t.spec, a..b, piece.text(), !first_in_row && t.tab_before, MAX_VIRTUAL_COPY_BYTES);
                        texts.push(piece);
                        first_in_row = false;
                        if capped {
                            hit_cap = true;
                            break;
                        }
                    }
                    if hit_cap { break; }
                }
            }

            // 3. Mounted group members after the virtual list's subtree.
            if !hit_cap {
                for j in (list_end + 1..self.nodes.len()).take_while(|&j| inside_g(j)).filter(|&j| self.member(j, g)) {
                    if append_mounted(j, &mut out, &mut texts, &mut prev_mounted) {
                        break;
                    }
                }
            }
        }

        let text = out.finish();
        (!text.is_empty()).then_some(TextSelection { texts, text, backward: self.selection_backward() })
    }

    /// Finds the first and last selectable texts inside mounted node `root` (including unbuilt rows
    /// of any virtual list under `root`), pinning offscreen end rows as needed.
    pub(crate) fn virtual_span_under(&mut self, root: usize) -> Option<(EndRef, EndRef)> {
        let inside = |nodes: &[Node], j: usize| {
            j == root || std::iter::successors(nodes[j].parent, |&p| nodes[p].parent).any(|p| p == root)
        };
        let list_path = (root..self.nodes.len())
            .take_while(|&j| inside(&self.nodes, j))
            .find_map(|j| {
                self.virtuals.get(&self.nodes[j].path)
                    .filter(|v| !v.keys.is_empty())
                    .map(|_| self.nodes[j].path.clone())
            })?;
        let list_node = self.live(&list_path)?;
        let g_path = if self.nodes[root].selection_group {
            Some(self.nodes[root].path.clone())
        } else {
            self.group_of(root).map(|g| self.nodes[g].path.clone())
        };

        // 1. Find the first selectable text under `root`.
        let first_before = (root..list_node)
            .filter(|&j| inside(&self.nodes, j) && self.is_selectable(j) && !self.subtree_exiting(j) && self.allowed(j))
            .find(|&j| match &g_path {
                Some(gp) => self.group_of(j).is_some_and(|g| &self.nodes[g].path == gp),
                None => true,
            })
            .map(EndRef::Mounted);

        let mut first_row_pin: Option<(usize, String)> = None;
        let first_end = if let Some(e) = first_before {
            Some(e)
        } else {
            let count = self.virtuals[&list_path].keys.len();
            let mut found = None;
            for r in 0..count {
                let range = self.virtuals[&list_path].range.clone();
                if range.contains(&r) {
                    let row_path = format!("{list_path}/{}", self.virtuals[&list_path].keys[r]);
                    if let Some(j) = (list_node + 1..self.nodes.len())
                        .take_while(|&j| inside(&self.nodes, j))
                        .filter(|&j| {
                            (self.nodes[j].path == row_path
                                || self.nodes[j].path.strip_prefix(&row_path).is_some_and(|s| s.starts_with('/')))
                                && self.is_selectable(j) && !self.subtree_exiting(j) && self.allowed(j)
                        })
                        .find(|&j| match &g_path {
                            Some(gp) => self.group_of(j).is_some_and(|g| &self.nodes[g].path == gp),
                            None => true,
                        })
                    {
                        found = Some(EndRef::Mounted(j));
                        break;
                    }
                } else {
                    let (mut texts, mut providers) = (Vec::new(), Vec::new());
                    self.build_unmounted_row(&list_path, r, &mut texts, &mut providers);
                    if let Some(t) = texts.iter().find(|t| match &g_path {
                        Some(gp) => t.group_path.as_deref() == Some(gp.as_str()),
                        None => true,
                    }) {
                        first_row_pin = Some((r, t.path.clone()));
                        break;
                    }
                }
            }
            if let Some((r, path)) = first_row_pin {
                self.pin_offscreen_row(&list_path, r);
                found = self.resolve_end(&path, None);
            }
            found
        }?;

        let target_group = g_path.or_else(|| self.end_group_path(first_end).map(str::to_owned));
        let Some(gp) = target_group else {
            return Some((first_end, first_end));
        };

        // 2. Find the last selectable text under `root` in `gp`.
        let inside_list = |j: usize| std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == list_node);
        let list_end = (list_node + 1..self.nodes.len()).take_while(|&j| inside_list(j)).last().unwrap_or(list_node);
        let last_after = (list_end + 1..self.nodes.len())
            .take_while(|&j| inside(&self.nodes, j))
            .filter(|&j| {
                self.is_selectable(j) && !self.subtree_exiting(j) && self.allowed(j)
                    && self.group_of(j).is_some_and(|g| self.nodes[g].path == gp)
            })
            .last()
            .map(EndRef::Mounted);

        let last_end = if let Some(e) = last_after {
            e
        } else {
            let count = self.virtuals[&list_path].keys.len();
            let mut last_row_pin: Option<(usize, String)> = None;
            let mut found = None;
            for r in (0..count).rev() {
                let range = self.virtuals[&list_path].range.clone();
                if range.contains(&r) {
                    let row_path = format!("{list_path}/{}", self.virtuals[&list_path].keys[r]);
                    if let Some(j) = (list_node + 1..=list_end)
                        .filter(|&j| {
                            (self.nodes[j].path == row_path
                                || self.nodes[j].path.strip_prefix(&row_path).is_some_and(|s| s.starts_with('/')))
                                && self.is_selectable(j) && !self.subtree_exiting(j) && self.allowed(j)
                                && self.group_of(j).is_some_and(|g| self.nodes[g].path == gp)
                        })
                        .last()
                    {
                        found = Some(EndRef::Mounted(j));
                        break;
                    }
                } else {
                    let (mut texts, mut providers) = (Vec::new(), Vec::new());
                    self.build_unmounted_row(&list_path, r, &mut texts, &mut providers);
                    if let Some(t) = texts.iter().rev().find(|t| t.group_path.as_deref() == Some(gp.as_str())) {
                        last_row_pin = Some((r, t.path.clone()));
                        break;
                    }
                }
            }
            if let Some((r, path)) = last_row_pin {
                self.pin_offscreen_row(&list_path, r);
                found = self.resolve_end(&path, None);
            }
            found.unwrap_or(first_end)
        };

        let first_path = self.end_path(first_end).to_owned();
        let last_path = self.end_path(last_end).to_owned();
        Some((self.resolve_end(&first_path, None)?, self.resolve_end(&last_path, None)?))
    }

    /// Carries out a `Tx::select_text`, `select_all_text` or `clear_text_selection`.
    pub(crate) fn select_request(&mut self, request: SelectRequest) {
        let named = |key: &str| (0..self.nodes.len()).find(|&i| self.nodes[i].named(key) && !self.subtree_exiting(i) && self.allowed(i));
        match request {
            SelectRequest::Clear => self.clear_text_selection(),
            SelectRequest::Range(key, range) => {
                let Some(i) = named(&key).filter(|&i| self.is_selectable(i)) else {
                    self.select_unmounted_request(&SelectRequest::Range(key, range));
                    return;
                };
                let content = self.content_of(i).to_owned();
                let (a, b) = (floor_boundary(&content, range.start), floor_boundary(&content, range.end.max(range.start)));
                let path = self.nodes[i].path.clone();
                let sel = &mut self.text_sel;
                (sel.path, sel.content, sel.anchor, sel.focus, sel.origin, sel.unit, sel.far) =
                    (Some(path), content, a, b, (a, b), Unit::Char, None);
                (sel.anchor_end, sel.focus_end, sel.dragging) = (None, None, false);
                self.seal_between();
            }
            SelectRequest::All(key) => {
                let Some(i) = named(&key) else {
                    self.select_unmounted_request(&SelectRequest::All(key));
                    return;
                };
                if let Some((first, last)) = self.virtual_span_under(i) {
                    self.select_span_ends(first, last);
                    return;
                }
                if self.nodes[i].selection_group {
                    let members = self.group_texts(i);
                    if let (Some(&first), Some(&last)) = (members.first(), members.last()) {
                        self.select_span(first, last);
                        return;
                    }
                }
                let inside = |j: usize| j == i || std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == i);
                let texts = || (i..self.nodes.len()).take_while(|&j| inside(j))
                    .filter(|&j| self.is_selectable(j) && !self.subtree_exiting(j) && self.allowed(j));
                let Some(first) = texts().next() else { return };
                let last = match self.group_of(first) {
                    Some(g) => texts().filter(|&j| self.group_of(j) == Some(g)).last().unwrap_or(first),
                    None => first,
                };
                self.select_span(first, last);
            }
        }
    }

    /// `select_request`, recording what an `El::on_selection_change` callback changed as already
    /// heard so it doesn't fire again on its own request.
    pub(crate) fn apply_select_request(&mut self, request: SelectRequest, from_change: bool) {
        self.select_request(request);
        if from_change {
            let _ = self.selection_changes();
        }
    }

    /// Handles `Tx::select_text` / `Tx::select_all_text` when `selector` names an element in an
    /// unbuilt virtual row.
    pub(crate) fn select_unmounted_request(&mut self, request: &SelectRequest) -> bool {
        let selector = match request {
            SelectRequest::Clear => return false,
            SelectRequest::Range(k, _) | SelectRequest::All(k) => k.as_str(),
        };
        let lists: Vec<String> = self.virtuals.keys()
            .filter(|lp| self.virtual_list_allowed(lp))
            .cloned()
            .collect();

        for list_path in lists {
            let Some(v) = self.virtuals.get(&list_path) else { continue };
            let count = v.keys.len();
            let range = v.range.clone();
            let mut candidate_rows: Vec<usize> = (0..count)
                .filter(|r| !range.contains(r))
                .filter(|&r| {
                    let rk = &v.keys[r];
                    selector == rk
                        || selector.strip_prefix(rk.as_str()).is_some_and(|s| s.starts_with('/'))
                        || selector.rsplit('/').any(|seg| seg == rk)
                })
                .collect();
            if candidate_rows.is_empty() {
                candidate_rows.extend((0..count).filter(|r| !range.contains(r)).take(MAX_VIRTUAL_COPY_ROWS));
            }
            for r in candidate_rows {
                let (mut texts, mut providers) = (Vec::new(), Vec::new());
                self.build_unmounted_row(&list_path, r, &mut texts, &mut providers);
                match request {
                    SelectRequest::Range(_, byte_range) => {
                        if let Some(t) = texts.iter().find(|t| t.path == selector || crate::flight::selects(selector, &t.path, t.key.as_deref())) {
                            let target_path = t.path.clone();
                            let content = t.spec.content.clone();
                            let (a, b) = (
                                floor_boundary(&content, byte_range.start),
                                floor_boundary(&content, byte_range.end.max(byte_range.start)),
                            );
                            self.pin_offscreen_row(&list_path, r);
                            let Some(end_ref) = self.resolve_end(&target_path, None) else { return false };
                            let v_end = self.virtual_end_of(end_ref, a);
                            let sel = &mut self.text_sel;
                            (sel.path, sel.content, sel.anchor, sel.focus, sel.origin, sel.unit, sel.far) =
                                (Some(target_path), content, a, b, (a, b), Unit::Char, None);
                            sel.anchor_end = v_end;
                            sel.focus_end = None;
                            sel.dragging = false;
                            self.seal_between();
                            return true;
                        }
                    }
                    SelectRequest::All(_) => {
                        let row_path = format!("{list_path}/{}", self.virtuals[&list_path].keys[r]);
                        let row_key = &self.virtuals[&list_path].keys[r];
                        let row_matches = row_path == selector || crate::flight::selects(selector, &row_path, Some(row_key));
                        let matching: Vec<&OffscreenText> = if row_matches {
                            texts.iter().collect()
                        } else {
                            texts.iter().filter(|t| t.path == selector || crate::flight::selects(selector, &t.path, t.key.as_deref())).collect()
                        };
                        if let Some(&first_t) = matching.first() {
                            let gp = first_t.group_path.as_deref();
                            let last_t = match gp {
                                Some(g) => matching.iter().copied().filter(|t| t.group_path.as_deref() == Some(g)).last().unwrap_or(first_t),
                                None => first_t,
                            };
                            let (p0, p1) = (first_t.path.clone(), last_t.path.clone());
                            self.pin_offscreen_row(&list_path, r);
                            let (Some(e0), Some(e1)) = (self.resolve_end(&p0, None), self.resolve_end(&p1, None)) else { return false };
                            self.select_span_ends(e0, e1);
                            return true;
                        }
                    }
                    SelectRequest::Clear => {}
                }
            }
        }
        false
    }
}
