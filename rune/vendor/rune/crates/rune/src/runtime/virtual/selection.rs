//! Virtual-row text selection: pinned ends, identity-based endpoints, and pruning.
#[path = "selection_collect.rs"]
mod collect;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use taffy::style::{Display, FlexDirection};

use super::{
    Held, SelectedText, Unit, kept, same_keyed_path, trim_uncopied,
};
use crate::el::{El, SelectionCopy, TextSpec};
use crate::runtime::Runtime;

/// Maximum number of unbuilt virtual rows built on demand when copying or reading a selection.
/// Past this cap, collection stops building further unbuilt rows and returns the prefix collected
/// so far.
pub(crate) const MAX_VIRTUAL_COPY_ROWS: usize = 10_000;

/// Maximum number of UTF-8 bytes collected when copying or reading a selection across unbuilt
/// virtual rows (4 MiB). Past this cap, collection stops and truncates on a character boundary.
pub(crate) const MAX_VIRTUAL_COPY_BYTES: usize = 4 * 1024 * 1024;

/// Identity of a selection endpoint inside a virtual list: `(row key/index, element path within
/// the row, text offset)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VirtualEnd {
    pub list_path: String,
    pub row_key: String,
    pub row_index: usize,
    pub rel_path: String,
    pub offset: usize,
}

impl VirtualEnd {
    pub fn full_path(&self) -> String {
        if self.rel_path.is_empty() {
            format!("{}/{}", self.list_path, self.row_key)
        } else {
            format!("{}/{}/{}", self.list_path, self.row_key, self.rel_path)
        }
    }
}

/// One selectable text inside an offscreen virtual row.
#[derive(Clone, Debug)]
pub(crate) struct OffscreenText {
    pub list_path: String,
    pub row_key: String,
    pub row_index: usize,
    pub order_in_row: usize,
    pub rel_path: String,
    pub path: String,
    pub key: Option<String>,
    pub group_path: Option<String>,
    pub spec: TextSpec,
    /// True when this text and the previous selectable text in the same row share a horizontal
    /// (`FlexDirection::Row | FlexDirection::RowReverse`) lowest common ancestor (table cells).
    pub tab_before: bool,
}

/// Offscreen selection state retained on a [`crate::el::virtual_list::VirtualList`].
#[derive(Default)]
pub(crate) struct PinnedSelection {
    pub texts: Vec<OffscreenText>,
    pub copy_providers: Vec<(String, SelectionCopy)>,
    pub source: Option<Rc<crate::el::virtual_rows::RowSource>>,
    pub unmounted: HashMap<usize, El>,
    pub group_path: Option<String>,
    pub inert: bool,
}

impl std::fmt::Debug for PinnedSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedSelection")
            .field("texts", &self.texts.len())
            .field("group_path", &self.group_path)
            .field("inert", &self.inert)
            .finish()
    }
}

/// A resolved selection endpoint: either a mounted node index in `Runtime::nodes` or an offscreen
/// text `(virtual_list_iter_index, text_index)` in `PinnedSelection::texts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndRef {
    Mounted(usize),
    Offscreen(usize, usize),
}

/// Collects selectable texts and `selection_copy` providers from an offscreen row `row_el`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_virtual_row_selection(
    row_el: &El,
    list_path: &str,
    row_key: &str,
    row_index: usize,
    list_group: Option<&str>,
    list_inert: bool,
    texts: &mut Vec<OffscreenText>,
    copy_providers: &mut Vec<(String, SelectionCopy)>,
) {
    let mut order_in_row = 0usize;
    let row_path = format!("{list_path}/{row_key}");
    walk_row_el(
        row_el, list_path, row_key, row_index, &row_path, "",
        list_group, list_inert, &mut order_in_row, texts, copy_providers,
    );
}

#[allow(clippy::too_many_arguments)]
fn walk_row_el<'a>(
    el: &'a El,
    list_path: &str,
    row_key: &str,
    row_index: usize,
    path: &'a str,
    rel_path: &str,
    parent_group: Option<&'a str>,
    parent_inert: bool,
    order_in_row: &mut usize,
    texts: &mut Vec<OffscreenText>,
    copy_providers: &mut Vec<(String, SelectionCopy)>,
) {
    let inert = parent_inert || el.flags.has(crate::el::ElFlags::INERT);
    let group = if el.flags.has(crate::el::ElFlags::SELECTION_GROUP) { Some(path) } else { parent_group };
    if !inert && let Some(f) = el.handlers.get().and_then(|h| h.selection_copy.as_ref()) {
        copy_providers.push((path.to_owned(), f.clone()));
    }
    let is_row = el.style.display() == Display::Flex
        && matches!(el.style.flex_direction(), FlexDirection::Row | FlexDirection::RowReverse);
    let mut had_child_text = false;
    if !inert && el.cold.input.is_none() && let Some(t) = &el.text && t.selectable && !t.redacted {
        let mut spec = TextSpec::clone(t);
        spec.apply_case();
        texts.push(OffscreenText {
            list_path: list_path.to_owned(),
            row_key: row_key.to_owned(),
            row_index,
            order_in_row: *order_in_row,
            rel_path: rel_path.to_owned(),
            path: path.to_owned(),
            key: el.key.clone(),
            group_path: group.map(str::to_owned),
            spec,
            tab_before: false,
        });
        *order_in_row += 1;
        had_child_text = true;
    }
    for (i, child) in el.all_children().enumerate() {
        let child_path = crate::runtime::flatten::node_path(path, child.key.as_deref(), i);
        let child_rel = crate::runtime::flatten::node_path(rel_path, child.key.as_deref(), i);
        let before_len = texts.len();
        walk_row_el(
            child, list_path, row_key, row_index, &child_path, &child_rel,
            group, inert, order_in_row, texts, copy_providers,
        );
        if texts.len() > before_len {
            if had_child_text {
                texts[before_len].tab_before = is_row;
            }
            had_child_text = true;
        }
    }
}

pub(super) fn offscreen_piece(t: &OffscreenText, range: Range<usize>, base: Option<&str>) -> SelectedText {
    let path = match base {
        Some(b) if b == t.path => String::new(),
        Some(b) => t.path.strip_prefix(b).and_then(|p| p.strip_prefix('/')).unwrap_or(&t.path).to_owned(),
        None => t.path.clone(),
    };
    let len = t.spec.content.len();
    let (full_a, full_b) = if t.spec.uncopied.is_empty() {
        (0, len)
    } else {
        trim_uncopied(&t.spec.uncopied, (0, len))
    };
    let whole = !range.is_empty() && len > 0 && range.start <= full_a && range.end >= full_b;
    let text = t.spec.copy_slice(range.start as u32, range.end as u32).unwrap_or_default();
    SelectedText { path, key: t.key.clone(), range, len, whole, text }
}

impl<S: 'static> Runtime<S> {
    pub(crate) fn virtual_list_allowed(&self, list_path: &str) -> bool {
        self.live(list_path).is_some()
            && !self.virtuals.get(list_path).is_some_and(|v| v.selection.inert)
    }

    pub(crate) fn offscreen_text(&self, vi: usize, ti: usize) -> &OffscreenText {
        &self.virtuals.iter().nth(vi).unwrap().1.selection.texts[ti]
    }

    pub(crate) fn end_path(&self, end: EndRef) -> &str {
        match end {
            EndRef::Mounted(i) => &self.nodes[i].path,
            EndRef::Offscreen(vi, ti) => &self.offscreen_text(vi, ti).path,
        }
    }

    pub(crate) fn end_content(&self, end: EndRef) -> &str {
        match end {
            EndRef::Mounted(i) => self.content_of(i),
            EndRef::Offscreen(vi, ti) => &self.offscreen_text(vi, ti).spec.content,
        }
    }

    pub(crate) fn end_group_path(&self, end: EndRef) -> Option<&str> {
        match end {
            EndRef::Mounted(i) => self.group_of(i).map(|g| self.nodes[g].path.as_str()),
            EndRef::Offscreen(vi, ti) => self.offscreen_text(vi, ti).group_path.as_deref(),
        }
    }

    pub(crate) fn end_same_group(&self, a: EndRef, b: EndRef) -> bool {
        self.end_group_path(a).is_some_and(|ga| self.end_group_path(b) == Some(ga))
    }

    /// Canonical pre-order key `(list_or_node_slot, row_index, in_row_order)` for any mounted or
    /// offscreen selectable text.
    pub(crate) fn tree_order(&self, end: EndRef) -> (usize, usize, usize) {
        match end {
            EndRef::Offscreen(vi, ti) => {
                let t = self.offscreen_text(vi, ti);
                let list_node = self.live(&t.list_path).unwrap_or(0);
                let row_index = self.virtuals.get(&t.list_path)
                    .and_then(|v| v.index_of(&t.row_key))
                    .unwrap_or(t.row_index);
                (list_node * 2 + 1, row_index, t.order_in_row)
            }
            EndRef::Mounted(i) => {
                let mut cur = i;
                while let Some(p) = self.nodes[cur].parent {
                    if let Some(v) = self.virtuals.get(&self.nodes[p].path) {
                        let row_index = self.nodes[cur].key.as_deref()
                            .and_then(|k| v.index_of(k))
                            .unwrap_or(0);
                        return (p * 2 + 1, row_index, i);
                    }
                    cur = p;
                }
                (i * 2, 0, 0)
            }
        }
    }

    /// Virtual-list row identity of `end`, if `end` lies inside a virtual list.
    pub(crate) fn virtual_end_of(&self, end: EndRef, offset: usize) -> Option<VirtualEnd> {
        match end {
            EndRef::Offscreen(vi, ti) => {
                let t = self.offscreen_text(vi, ti);
                let row_index = self.virtuals.get(&t.list_path)
                    .and_then(|v| v.index_of(&t.row_key))
                    .unwrap_or(t.row_index);
                Some(VirtualEnd {
                    list_path: t.list_path.clone(),
                    row_key: t.row_key.clone(),
                    row_index,
                    rel_path: t.rel_path.clone(),
                    offset,
                })
            }
            EndRef::Mounted(i) => {
                let mut cur = i;
                while let Some(p) = self.nodes[cur].parent {
                    if let Some(v) = self.virtuals.get(&self.nodes[p].path) {
                        let row_key = self.nodes[cur].key.clone()?;
                        let row_index = v.index_of(&row_key)?;
                        let row_path = &self.nodes[cur].path;
                        let rel_path = if i == cur {
                            String::new()
                        } else {
                            self.nodes[i].path.strip_prefix(row_path)?.strip_prefix('/')?.to_owned()
                        };
                        return Some(VirtualEnd {
                            list_path: self.nodes[p].path.clone(),
                            row_key,
                            row_index,
                            rel_path,
                            offset,
                        });
                    }
                    cur = p;
                }
                None
            }
        }
    }

    pub(crate) fn resolve_end(&self, path: &str, v_end: Option<&VirtualEnd>) -> Option<EndRef> {
        if let Some(end) = v_end && let Some(r) = self.resolve_virtual_end(end) {
            return Some(r);
        }
        if let Some(i) = self.live(path).filter(|&i| self.is_selectable(i)) {
            return Some(EndRef::Mounted(i));
        }
        for (vi, (list_path, v)) in self.virtuals.iter().enumerate() {
            if !self.virtual_list_allowed(list_path) {
                continue;
            }
            for (ti, t) in v.selection.texts.iter().enumerate() {
                if t.path == path && v.index_of(&t.row_key).is_some_and(|r| !v.range.contains(&r)) {
                    return Some(EndRef::Offscreen(vi, ti));
                }
            }
        }
        None
    }

    fn resolve_virtual_end(&self, end: &VirtualEnd) -> Option<EndRef> {
        for (vi, (list_path, v)) in self.virtuals.iter().enumerate() {
            if list_path != &end.list_path || !self.virtual_list_allowed(list_path) {
                continue;
            }
            let row_idx = v.index_of(&end.row_key)?;
            if v.range.contains(&row_idx) {
                let full = end.full_path();
                return self.live(&full).filter(|&i| self.is_selectable(i)).map(EndRef::Mounted);
            }
            for (ti, t) in v.selection.texts.iter().enumerate() {
                if t.row_key == end.row_key && t.rel_path == end.rel_path {
                    return Some(EndRef::Offscreen(vi, ti));
                }
            }
        }
        None
    }

    pub(crate) fn selection_holder_end(&self) -> Option<EndRef> {
        let sel = &self.text_sel;
        let path = sel.path.as_deref()?;
        let h = self.resolve_end(path, sel.anchor_end.as_ref())?;
        let upto = match &sel.far {
            None => sel.anchor.max(sel.focus).max(sel.origin.1),
            Some((far, _))
                if self.resolve_end(far, sel.focus_end.as_ref())
                    .is_some_and(|f| self.tree_order(f) > self.tree_order(h)) =>
            {
                sel.content.len()
            }
            Some(_) => sel.anchor.max(sel.origin.1),
        };
        kept(&sel.content, self.end_content(h), upto).then_some(h)
    }

    pub(crate) fn focus_holder_end(&self, holder: EndRef) -> Option<EndRef> {
        let sel = &self.text_sel;
        let Some((path, content)) = &sel.far else { return Some(holder) };
        let f = self.resolve_end(path, sel.focus_end.as_ref())?;
        let upto = if self.tree_order(f) > self.tree_order(holder) { sel.focus } else { content.len() };
        (self.end_same_group(f, holder) && kept(content, self.end_content(f), upto)).then_some(f)
    }

    /// Updates `anchor_end`, `focus_end` and each `VirtualList::pinned_rows`.
    pub(crate) fn sync_selection_pins(&mut self) {
        if self.text_sel.path.is_none() {
            for v in self.virtuals.values_mut() {
                v.pinned_rows.clear();
                v.selection.texts.clear();
                v.selection.copy_providers.clear();
            }
            return;
        }
        if let Some(h) = self.selection_holder_end() {
            self.text_sel.anchor_end = self.virtual_end_of(h, self.text_sel.anchor);
            self.text_sel.focus_end = if self.text_sel.far.is_some() {
                self.focus_holder_end(h).and_then(|f| self.virtual_end_of(f, self.text_sel.focus))
            } else {
                None
            };
        }
        let a_end = self.text_sel.anchor_end.clone();
        let f_end = self.text_sel.focus_end.clone();
        for (list_path, v) in &mut self.virtuals {
            v.pinned_rows.clear();
            if let Some(a) = &a_end && &a.list_path == list_path {
                v.pinned_rows.push(a.row_key.clone());
            }
            if let Some(f) = &f_end && &f.list_path == list_path && !v.pinned_rows.contains(&f.row_key) {
                v.pinned_rows.push(f.row_key.clone());
            }
            v.selection.texts.retain(|t| v.pinned_rows.contains(&t.row_key));
        }
    }

    /// Whether `h` and `f` span across virtual rows (either between different rows of a virtual
    /// list, or between an element outside the virtual list and a row inside or past it).
    fn is_cross_row_virtual(&self, h: EndRef, f: EndRef) -> Option<(String, usize, usize)> {
        let (first, last) = if self.tree_order(h) <= self.tree_order(f) { (h, f) } else { (f, h) };
        let va = self.virtual_end_of(first, 0);
        let vb = self.virtual_end_of(last, 0);
        match (va, vb) {
            (Some(a), Some(b)) => {
                (a.list_path == b.list_path && a.row_index != b.row_index)
                    .then(|| (a.list_path, a.row_index.min(b.row_index), a.row_index.max(b.row_index)))
            }
            (Some(a), None) => {
                let v = self.virtuals.get(&a.list_path)?;
                (!v.keys.is_empty()).then(|| (a.list_path, a.row_index, v.keys.len() - 1))
            }
            (None, Some(b)) => Some((b.list_path, 0, b.row_index)),
            (None, None) => {
                let (first_ord, last_ord) = (self.tree_order(first), self.tree_order(last));
                self.virtuals.iter().find_map(|(lp, v)| {
                    if v.keys.is_empty() { return None; }
                    let ln = self.live(lp)?;
                    let list_ord = (ln * 2 + 1, 0, 0);
                    (first_ord < list_ord && last_ord > list_ord)
                        .then(|| (lp.clone(), 0, v.keys.len() - 1))
                })
            }
        }
    }

    fn mounted_row_hash(&self, list_path: &str, row_key: &str, g_path: &str) -> Option<u64> {
        let list_node = self.live(list_path)?;
        let row_path = format!("{list_path}/{row_key}");
        let row_node = (list_node + 1..self.nodes.len())
            .find(|&j| self.nodes[j].parent == Some(list_node) && self.nodes[j].path == row_path)?;
        let inside_row = |j: usize| {
            j == row_node
                || std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == row_node)
        };
        let mut hasher = rustc_hash::FxHasher::default();
        for j in (row_node..self.nodes.len()).take_while(|&j| inside_row(j)) {
            if self.is_selectable(j)
                && !self.subtree_exiting(j)
                && self.allowed(j)
                && self.group_of(j).is_some_and(|g| self.nodes[g].path == g_path)
            {
                (&self.nodes[j].path, self.content_of(j)).hash(&mut hasher);
            }
        }
        Some(hasher.finish())
    }

    pub(crate) fn between_hash_ends(&self, h: EndRef, f: EndRef) -> u64 {
        let mut hasher = rustc_hash::FxHasher::default();
        let Some(g_path) = self.end_group_path(h).filter(|_| h != f) else {
            return hasher.finish();
        };
        let (first, last) = if self.tree_order(h) < self.tree_order(f) { (h, f) } else { (f, h) };
        if let Some((list_path, r0, r1)) = self.is_cross_row_virtual(first, last) {
            if let Some(g) = self.live(g_path) && let Some(ln) = self.live(&list_path) {
                let (first_ord, last_ord) = (self.tree_order(first), self.tree_order(last));
                let inside_list = |j: usize| {
                    j == ln || std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == ln)
                };
                for j in (g + 1..self.nodes.len()).filter(|&j| !inside_list(j) && self.member(j, g)) {
                    let ord = self.tree_order(EndRef::Mounted(j));
                    if ord > first_ord && ord < last_ord {
                        (&self.nodes[j].path, self.content_of(j)).hash(&mut hasher);
                    }
                }
            }
            if let Some(v) = self.virtuals.get(&list_path) && r1 < v.keys.len() {
                v.keys[r0..=r1].hash(&mut hasher);
            }
            return hasher.finish();
        }
        match (first, last) {
            (EndRef::Mounted(i0), EndRef::Mounted(i1)) => {
                if let Some(g) = self.group_of(i0) {
                    for j in (i0 + 1..i1).filter(|&j| self.member(j, g)) {
                        (&self.nodes[j].path, self.content_of(j)).hash(&mut hasher);
                    }
                }
            }
            (EndRef::Offscreen(vi0, ti0), EndRef::Offscreen(vi1, ti1)) if vi0 == vi1 => {
                let (_, v) = self.virtuals.iter().nth(vi0).unwrap();
                let row_key = &v.selection.texts[ti0].row_key;
                let (o0, o1) = (v.selection.texts[ti0].order_in_row, v.selection.texts[ti1].order_in_row);
                for t in &v.selection.texts {
                    if &t.row_key == row_key && t.order_in_row > o0 && t.order_in_row < o1 && t.group_path.as_deref() == Some(g_path) {
                        (&t.path, t.spec.content.as_str()).hash(&mut hasher);
                    }
                }
            }
            _ => {}
        }
        hasher.finish()
    }

    pub(crate) fn seal_between(&mut self) {
        self.sync_selection_pins();
        self.text_sel.mounted_between.clear();
        let Some(h) = self.selection_holder_end() else {
            self.text_sel.between = 0;
            self.remember_pick();
            return;
        };
        let Some(f) = self.focus_holder_end(h) else {
            self.text_sel.between = 0;
            self.remember_pick();
            return;
        };
        self.text_sel.between = self.between_hash_ends(h, f);
        if let Some(g_path) = self.end_group_path(h).map(str::to_owned)
            && let Some((list_path, r0, r1)) = self.is_cross_row_virtual(h, f)
            && let Some(v) = self.virtuals.get(&list_path)
        {
            let range = v.range.clone();
            let keys: Vec<String> = ((r0 + 1)..r1)
                .filter(|r| range.contains(r))
                .filter_map(|r| v.keys.get(r).cloned())
                .collect();
            for key in keys {
                if let Some(hash) = self.mounted_row_hash(&list_path, &key, &g_path) {
                    self.text_sel.mounted_between.insert(key, hash);
                }
            }
        }
        self.remember_pick();
    }

    pub(crate) fn between_matches(&mut self, h: EndRef, f: EndRef) -> bool {
        if self.text_sel.far.is_none() {
            return true;
        }
        if self.between_hash_ends(h, f) != self.text_sel.between {
            return false;
        }
        if let Some(g_path) = self.end_group_path(h).map(str::to_owned)
            && let Some((list_path, r0, r1)) = self.is_cross_row_virtual(h, f)
            && let Some(v) = self.virtuals.get(&list_path)
        {
            let range = v.range.clone();
            let keys: Vec<String> = ((r0 + 1)..r1)
                .filter(|r| range.contains(r))
                .filter_map(|r| v.keys.get(r).cloned())
                .collect();
            for key in keys {
                if let Some(hash) = self.mounted_row_hash(&list_path, &key, &g_path) {
                    if let Some(&prev) = self.text_sel.mounted_between.get(&key) {
                        if prev != hash {
                            return false;
                        }
                    } else {
                        self.text_sel.mounted_between.insert(key, hash);
                    }
                }
            }
        }
        true
    }

    /// True when the current selection is non-empty (mounted or offscreen).
    pub(crate) fn has_nonempty_selection(&self) -> bool {
        let Some(h) = self.selection_holder_end() else { return false };
        let Some(f) = self.focus_holder_end(h) else { return false };
        if h == f {
            let (a, b) = (self.text_sel.anchor.min(self.text_sel.focus), self.text_sel.anchor.max(self.text_sel.focus));
            a < b && b <= self.end_content(h).len()
        } else {
            true
        }
    }

    /// True when the selection touches any offscreen/unbuilt virtual row.
    pub(crate) fn selection_spans_unmounted(&self) -> bool {
        let Some(h) = self.selection_holder_end() else { return false };
        let Some(f) = self.focus_holder_end(h) else { return false };
        if matches!(h, EndRef::Offscreen(..)) || matches!(f, EndRef::Offscreen(..)) {
            return true;
        }
        let (EndRef::Mounted(i0), EndRef::Mounted(i1)) = (h, f) else { return true };
        if i0 == i1 {
            return false;
        }
        let (first_ord, last_ord) = (self.tree_order(EndRef::Mounted(i0.min(i1))), self.tree_order(EndRef::Mounted(i0.max(i1))));
        self.virtuals.iter().any(|(list_path, v)| {
            if v.keys.is_empty() || (v.range.start == 0 && v.range.end >= v.keys.len()) {
                return false;
            }
            let Some(ln) = self.live(list_path) else { return false };
            let head_unmounted = v.range.start > 0
                && (ln * 2 + 1, 0, usize::MAX) >= first_ord
                && (ln * 2 + 1, 0, 0) <= last_ord;
            let last_r = v.keys.len() - 1;
            let tail_unmounted = v.range.end < v.keys.len()
                && (ln * 2 + 1, last_r, usize::MAX) >= first_ord
                && (ln * 2 + 1, last_r, 0) <= last_ord;
            head_unmounted || tail_unmounted
        })
    }

    /// A unique live selectable node at `path`'s keyed identity when an unkeyed ancestor moved.
    fn rebound(&self, path: &str, content: &str, upto: usize) -> Option<usize> {
        let mut hits = (0..self.nodes.len()).filter(|&i| {
            self.is_selectable(i) && !self.subtree_exiting(i) && self.allowed(i)
                && same_keyed_path(path, &self.nodes[i].path) && kept(content, self.content_of(i), upto)
        });
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    /// After a commit: drop a selection whose texts went away, stopped being selectable or
    /// changed what it selected, or whose texts between its ends changed.
    pub(crate) fn prune_text_selection(&mut self) {
        if self.text_sel.path.is_none() { return; }
        let mut rebind = false;
        if self.selection_holder_end().is_none()
            && self.text_sel.anchor_end.is_none()
            && let Some(old_path) = self.text_sel.path.clone()
        {
            let upto = self.text_sel.anchor.max(self.text_sel.focus).max(self.text_sel.origin.1);
            if let Some(i) = self.rebound(&old_path, &self.text_sel.content, upto) {
                let old_ancs: Vec<&str> = std::iter::successors(Some(old_path.as_str()), |p| p.rsplit_once('/').map(|(a, _)| a)).collect();
                let new_ancs: Vec<usize> = std::iter::successors(Some(i), |&p| self.nodes[p].parent).collect();
                for (&old_anc, &new_anc) in old_ancs.iter().zip(&new_ancs) {
                    if let Some(v) = self.sel_heard.by_path.remove(old_anc) {
                        self.sel_heard.by_path.insert(self.nodes[new_anc].path.clone(), v);
                    }
                }
                self.text_sel.path = Some(self.nodes[i].path.clone());
                rebind = true;
            }
        }
        if let Some(h) = self.selection_holder_end()
            && self.focus_holder_end(h).is_none()
            && self.text_sel.focus_end.is_none()
            && let Some((far, content)) = &self.text_sel.far
            && let Some(j) = self.rebound(far, content, self.text_sel.focus)
        {
            self.text_sel.far = Some((self.nodes[j].path.clone(), content.clone()));
            rebind = true;
        }
        if rebind && let Some((h, f)) = self.selection_holder_end().and_then(|h| Some((h, self.focus_holder_end(h)?))) {
            self.text_sel.between = self.between_hash_ends(h, f);
        }
        let Some((h, f)) = self.selection_holder_end().and_then(|h| Some((h, self.focus_holder_end(h)?))) else {
            self.clear_text_selection();
            return;
        };
        if !self.between_matches(h, f) {
            self.clear_text_selection();
            return;
        }
        let h_path = self.end_path(h).to_owned();
        if self.text_sel.path.as_deref() != Some(h_path.as_str()) {
            self.text_sel.path = Some(h_path);
        }
        let h_content = self.end_content(h).to_owned();
        if self.text_sel.content != h_content {
            self.text_sel.content = h_content;
        }
        if f != h {
            let (f_path, f_content) = (self.end_path(f).to_owned(), self.end_content(f).to_owned());
            if let Some((fp, fc)) = &mut self.text_sel.far {
                if *fp != f_path { *fp = f_path; }
                if *fc != f_content { *fc = f_content; }
            }
        }
        self.sync_selection_pins();
    }

    /// Selects from the start of `first` to the end of `last` (mounted or offscreen).
    pub(crate) fn select_span_ends(&mut self, first: EndRef, last: EndRef) {
        let head = self.end_path(first).to_owned();
        let content = self.end_content(first).to_owned();
        let far = (first != last).then(|| (self.end_path(last).to_owned(), self.end_content(last).to_owned()));
        let end = far.as_ref().map_or(content.len(), |f| f.1.len());
        let origin = if far.is_some() { (0, 0) } else { (0, end) };
        let anchor_end = self.virtual_end_of(first, 0);
        let focus_end = (first != last).then(|| self.virtual_end_of(last, end)).flatten();
        let sel = &mut self.text_sel;
        (sel.path, sel.content, sel.anchor, sel.focus, sel.origin, sel.unit, sel.far) =
            (Some(head), content, 0, end, origin, Unit::Char, far);
        sel.anchor_end = anchor_end;
        sel.focus_end = focus_end;
        sel.dragging = false;
        self.seal_between();
    }
}

/// Resets `Held` while preserving modifier and pick state.
pub(super) fn clear_held(sel: &mut Held) {
    sel.path = None;
    sel.far = None;
    sel.anchor_end = None;
    sel.focus_end = None;
    sel.mounted_between.clear();
    sel.dragging = false;
}
