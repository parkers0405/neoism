//! Read-only text people can select and copy (`El::selectable`), as SwiftUI's
//! `.textSelection(.enabled)` does on the Mac. It shares the text fields' machinery: Parley's
//! layout hit-tests the pointer into byte offsets, the highlight is the fields' own selection
//! boxes (`push_selection`), and copying goes through `Tx::copy`. The selection lives here, not in
//! app state, so selecting never re-runs the view or re-lays out the text.
//!
//! One text holds the selection at a time, or one selection group (`El::selection_group`): the
//! anchor is in one of its texts, the focus in another, and every text of the group between them
//! in tree order is selected whole. A press elsewhere clears it, as does focus moving to another
//! element. The selection is byte offsets into the texts' content; a view that changes the
//! selected part of a text drops it, while text appended after it (a message streaming in)
//! keeps it.
//!
//! Code reads the selection with `Tx::text_selection` and sets it with `Tx::select_text`,
//! `Tx::select_all_text` and `Tx::clear_text_selection`; an `El::selection_copy` element
//! decides what copying a selection inside it takes.
use super::*;
use std::ops::Range;

/// A secondary press outside the selection selects the word under it first: AppKit text does,
/// Chromium on Windows and Linux doesn't.
const SECONDARY_SELECTS_WORD: bool = cfg!(target_os = "macos");

#[path = "copy_join.rs"]
mod copy_join;
#[path = "toolbar.rs"]
mod selection_toolbar;
#[path = "text_selection.rs"]
mod text_selection;
pub(crate) use copy_join::CopyJoin;
pub(crate) use selection_toolbar::OnPress;
pub use text_selection::{SelectedText, SelectionChange, TextSelection};

/// The commit, the selection's ends and whether a press is making it: what a selection read
/// from them depends on.
type Basis = (u64, Option<String>, usize, usize, Option<String>, bool);

/// `Tx::text_selection`'s snapshot and what it was taken from.
pub(super) struct Snapshot {
    from: Basis,
    selection: Option<Rc<TextSelection>>,
}

/// What each `El::on_selection_change` element last heard, by path, and what that was checked
/// against.
#[derive(Default)]
pub(super) struct Heard {
    from: Option<Basis>,
    by_path: HashMap<String, Rc<TextSelection>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Unit {
    #[default]
    Char,
    Word,
    Paragraph,
}

#[derive(Default)]
pub(super) struct Held {
    /// The text holding the anchor, and its content when last seen.
    pub(super) path: Option<String>,
    pub(super) content: String,
    pub(super) anchor: usize,
    /// An offset into the anchor's text, or into `far`'s when that's set.
    pub(super) focus: usize,
    /// The text holding the focus when it's another text of the anchor's selection group: its
    /// path and content when last seen.
    pub(super) far: Option<(String, String)>,
    /// Virtual-list row identity of the anchor and focus when inside a virtual list.
    pub(super) anchor_end: Option<virtual_selection::VirtualEnd>,
    pub(super) focus_end: Option<virtual_selection::VirtualEnd>,
    /// Hashes of mounted rows strictly between anchor and focus in a virtual list.
    pub(super) mounted_between: HashMap<String, u64>,
    /// A hash of the texts strictly between the anchor's and the focus's (their paths and
    /// content), taken when the selection changes, so a commit that rewrites, adds or removes
    /// one drops it.
    pub(super) between: u64,
    /// What the press that made it selects by, and the range of that unit it began on, which a
    /// drag keeps whole.
    pub(super) unit: Unit,
    pub(super) origin: (usize, usize),
    /// The held press is extending this selection.
    pub(super) dragging: bool,
    /// The last press on selectable text: its path, time, point and click count.
    pub(super) last: Option<(String, f64, f32, f32, u8)>,
    /// Shift is held (from the host's modifier events), so a press extends the selection.
    pub(super) shift: bool,
    pub(super) pick: super::select_all::Pick, // where Select All goes with no selection
}

/// The char boundary at or before byte `i` of `text`.
pub(super) fn floor_boundary(text: &str, i: usize) -> usize {
    text.floor_char_boundary(i)
}

/// The char boundary before byte `i` (a boundary) of `text`.
fn prev_boundary(text: &str, i: usize) -> usize { floor_boundary(text, i.saturating_sub(1)) }

/// The range of `unit` at byte `i` of `text`: a word (or run of spaces or punctuation) by
/// Unicode word boundaries, or a paragraph with its line break. Past the end of a line (on its
/// break, or past the end of the text) it takes the line's last word or paragraph, as AppKit does.
pub(crate) fn unit_at(text: &str, i: usize, unit: Unit) -> (usize, usize) {
    use unicode_segmentation::UnicodeSegmentation;
    let mut i = floor_boundary(text, i);
    if i > 0 && i == text.len() && unit != Unit::Char { i = prev_boundary(text, i); }
    match unit {
        Unit::Char => (i, i),
        Unit::Word => {
            if i > 0 && text[i..].starts_with(['\n', '\r']) && !text[..i].ends_with('\n') {
                i = prev_boundary(text, i);
                if text[i..].starts_with('\r') && i > 0 && !text[..i].ends_with('\n') { i = prev_boundary(text, i); }
            }
            let mut last = (i, i);
            for (start, word) in text.split_word_bound_indices() {
                let end = start + word.len();
                last = (start, end);
                if i < end { return last; }
            }
            last
        }
        Unit::Paragraph => {
            let start = text[..i].rfind('\n').map_or(0, |n| n + 1);
            let end = text[i..].find('\n').map_or(text.len(), |n| i + n + 1);
            (start, end)
        }
    }
}

/// The selection a drag from unit range `origin` to unit range `at` makes: (anchor, focus).
pub(crate) fn extend(origin: (usize, usize), at: (usize, usize)) -> (usize, usize) {
    if at.0 < origin.0 { (origin.1, at.0) } else { (origin.0, at.1.max(origin.1)) }
}

/// Pushes the selection boxes of bytes `a..b` of `layout`, whose top-left draws at `origin`
/// (device px) `k` times its size. Text fields and selectable text both draw through here, so
/// their highlights match.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_selection(out: &mut Vec<Instance>, layout: &Layout<Ink>, a: usize, b: usize, origin: [f32; 2], k: f32,
    fill: [f32; 4], s: f32, clip: [f32; 4], fade: [f32; 4]) {
    for (bb, _) in crate::text::range_boxes(layout, a, b, true) {
        let rect = [origin[0] + bb.x0 as f32 * k, origin[1] + bb.y0 as f32 * k, (bb.x1 - bb.x0) as f32 * k, (bb.y1 - bb.y0) as f32 * k];
        let radius = 2.0 * s * k;
        extend_run(out, rect, fill, radius, clip, fade, 40.0 * s * k);
    }
}

/// A text field's selection: one layout, so no runs to join, and a field's own fill behind it
/// is the field, not a chip the highlight should stretch to.
pub(super) fn push_field_selection(out: &mut Vec<Instance>, layout: &Layout<Ink>, a: usize, b: usize, origin: [f32; 2], k: f32,
    fill: [f32; 4], s: f32, clip: [f32; 4], fade: [f32; 4]) {
    for (bb, _) in crate::text::range_boxes(layout, a, b, true) {
        let rect = [origin[0] + bb.x0 as f32 * k, origin[1] + bb.y0 as f32 * k, (bb.x1 - bb.x0) as f32 * k, (bb.y1 - bb.y0) as f32 * k];
        out.push(rect_inst(rect, fill, 2.0 * s * k, clip, fade));
    }
}

/// Words flowed as separate texts each draw their own highlight. One that starts where a recent
/// one on the same line ended (or after the little gap a chip's icon and padding leave) joins
/// it, so a run reads as one rect with no seams, as tall as the line's tallest text. When
/// something filled was drawn in between (a chip's background), a rect of its own is drawn on
/// top, reaching back to where the last one ended.
fn extend_run(out: &mut Vec<Instance>, mut rect: [f32; 4], fill: [f32; 4], radius: f32, clip: [f32; 4], fade: [f32; 4], reach: f32) {
    let from = out.len().saturating_sub(256);
    let found = out[from..].iter().rposition(|i| {
        let gap = rect[0] - (i.rect[0] + i.rect[2]);
        let overlap = (i.rect[1] + i.rect[3]).min(rect[1] + rect[3]) - i.rect[1].max(rect[1]);
        i.fill == fill && i.clip == clip && i.radii == [radius; 4] && i.params[1] == crate::render::KIND_RECT
            && (-1.0..=reach).contains(&gap) && overlap > 0.6 * i.rect[3].min(rect[3])
    }).map(|k| from + k);
    let Some(at) = found else {
        // First on its line, inside a chip's background: start the highlight at the chip's edge.
        let chip = out[from..].iter().rev().find(|i| {
            let overlap = (i.rect[1] + i.rect[3]).min(rect[1] + rect[3]) - i.rect[1].max(rect[1]);
            i.params[1] == crate::render::KIND_RECT && i.fill != fill && i.fill[3] > 0.0 && i.rect[0] >= rect[0] - reach && i.rect[0] < rect[0]
                && i.rect[0] + i.rect[2] > rect[0] && i.rect[3] <= 1.8 * rect[3] && overlap > 0.6 * rect[3]
        }).map(|i| i.rect);
        if let Some(c) = chip {
            let (top, bottom) = (c[1].min(rect[1]), (c[1] + c[3]).max(rect[1] + rect[3]));
            rect = [c[0], top, rect[0] + rect[2] - c[0], bottom - top];
        }
        return out.push(rect_inst(rect, fill, radius, clip, fade));
    };
    let covered = out[at + 1..].iter().any(|i| i.params[1] == crate::render::KIND_RECT && i.fill != fill && i.fill[3] > 0.0);
    let prev = &mut out[at];
    let (top, bottom) = (prev.rect[1].min(rect[1]), (prev.rect[1] + prev.rect[3]).max(rect[1] + rect[3]));
    if covered {
        let left = (prev.rect[0] + prev.rect[2]).min(rect[0]);
        rect = [left, top, rect[0] + rect[2] - left, bottom - top];
        return out.push(rect_inst(rect, fill, radius, clip, fade));
    }
    prev.rect[2] = prev.rect[0].max(rect[0] + rect[2]) - prev.rect[0];
    prev.rect[1] = top;
    prev.rect[3] = bottom - top;
}

/// `now` still reads as `then` did up to byte `upto`: text added after it (streaming in) keeps
/// a selection, a change inside it doesn't. With nothing selected in it (`upto` 0) only text
/// appended keeps it.
pub(super) fn kept(then: &str, now: &str, upto: usize) -> bool {
    if upto == 0 { return now.starts_with(then); }
    then.get(..upto).is_some_and(|a| now.get(..upto) == Some(a))
}

/// `r`, a word, without the [`El::uncopied`] bytes at its ends, unless that leaves nothing.
pub(super) fn trim_uncopied(uncopied: &[(u32, u32)], (mut a, mut b): (usize, usize)) -> (usize, usize) {
    let r0 = (a, b);
    while let Some(&(_, e)) = uncopied.iter().find(|r| (r.0 as usize) <= a && a < r.1 as usize && a < b) { a = (e as usize).min(b); }
    while let Some(&(st, _)) = uncopied.iter().find(|r| (r.0 as usize) < b && b <= r.1 as usize && a < b) { b = (st as usize).max(a); }
    if a < b { (a, b) } else { r0 }
}

/// Node `i`'s selected bytes among `ranges` (from `selection_ranges`).
pub(super) fn range_in(ranges: &[(usize, usize, usize)], i: usize) -> Option<(usize, usize)> {
    ranges.iter().find(|r| r.0 == i).map(|r| (r.1, r.2))
}

/// How far `x` is outside box `r` (x0, y0, x1, y1) horizontally.
fn dist_x(r: [f32; 4], x: f32) -> f32 { (r[0] - x).max(x - r[2]).max(0.0) }

/// Whether `a` and `b` have the same keyed segments and the same unkeyed tail after their last
/// key, so a keyed text keeps its selection when a sibling shifts an unkeyed ancestor's index.
pub(super) fn same_keyed_path(a: &str, b: &str) -> bool {
    fn split(p: &str) -> Option<(&str, &str)> {
        let last_key = p.rmatch_indices('/').find(|&(i, _)| !p[i + 1..].starts_with('#'))
            .map_or_else(|| (!p.starts_with('#')).then_some(0), |(i, _)| Some(i + 1))?;
        let end = last_key + p[last_key..].find('/').unwrap_or(p.len() - last_key);
        Some((&p[..end], &p[end..]))
    }
    let (Some((ka, ta)), Some((kb, tb))) = (split(a), split(b)) else { return false };
    ta == tb && ka.split('/').filter(|s| !s.starts_with('#')).eq(kb.split('/').filter(|s| !s.starts_with('#')))
}

impl<S: 'static> Runtime<S> {
    pub(super) fn is_selectable(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        n.input.is_none() && n.text.as_ref().is_some_and(|t| t.selectable && !t.redacted)
    }

    pub(super) fn live(&self, path: &str) -> Option<usize> {
        (0..self.nodes.len()).find(|&i| self.nodes[i].path == path && !self.subtree_exiting(i) && self.allowed(i))
    }

    pub(super) fn content_of(&self, i: usize) -> &str {
        self.nodes[i].text.as_ref().map_or("", |t| t.content.as_str())
    }

    /// The selection group node `i` is in: its nearest `El::selection_group` ancestor.
    pub(super) fn group_of(&self, i: usize) -> Option<usize> {
        std::iter::successors(self.nodes[i].parent, |&p| self.nodes[p].parent).find(|&p| self.nodes[p].selection_group)
    }

    /// Node `j` is a selectable text of group `g` (not of a group inside it).
    pub(super) fn member(&self, j: usize, g: usize) -> bool {
        self.is_selectable(j) && self.group_of(j) == Some(g) && !self.subtree_exiting(j) && self.allowed(j)
    }

    /// Group `g`'s selectable texts, in tree order.
    pub(crate) fn group_texts(&self, g: usize) -> Vec<usize> {
        // Nodes are in tree order, so a subtree is the run after its root.
        let inside = |j: usize| std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == g);
        (g + 1..self.nodes.len()).take_while(|&j| inside(j)).filter(|&j| self.member(j, g)).collect()
    }

    /// The range of `unit` at byte `at` of node `i`'s text, a word without uncopied ends.
    pub(super) fn unit_in(&self, i: usize, at: usize, unit: Unit) -> (usize, usize) {
        let r = unit_at(self.content_of(i), at, unit);
        match (unit, &self.nodes[i].text) {
            (Unit::Word, Some(t)) if !t.uncopied.is_empty() => trim_uncopied(&t.uncopied, r),
            _ => r,
        }
    }

    /// The node holding the anchor, while it's still there, still selectable and still reads as
    /// it did up to the selection.
    pub(super) fn selection_holder(&self) -> Option<usize> {
        use virtual_selection::EndRef;
        let h = self.selection_holder_end()?;
        match h {
            EndRef::Mounted(i) => Some(i),
            EndRef::Offscreen(vi, ti) => match self.focus_holder_end(h) {
                Some(EndRef::Mounted(j)) => Some(j),
                _ => self.end_group_path(h).and_then(|g| self.live(g)).or_else(|| self.live(&self.offscreen_text(vi, ti).list_path)),
            },
        }
    }

    /// Every text the selection covers with its selected bytes, in tree order, none empty.
    pub(super) fn selection_ranges(&self) -> Vec<(usize, usize, usize)> {
        use virtual_selection::EndRef;
        let Some(h) = self.selection_holder_end() else { return Vec::new() };
        let Some(f) = self.focus_holder_end(h) else { return Vec::new() };
        let sel = &self.text_sel;
        if f == h {
            let EndRef::Mounted(i) = h else { return Vec::new() };
            let (a, b) = (sel.anchor.min(sel.focus), sel.anchor.max(sel.focus));
            return if a < b && b <= self.content_of(i).len() { vec![(i, a, b)] } else { Vec::new() };
        }
        let ((first, a), (last, b)) = if self.tree_order(h) < self.tree_order(f) {
            ((h, sel.anchor), (f, sel.focus))
        } else {
            ((f, sel.focus), (h, sel.anchor))
        };
        let Some(g_path) = self.end_group_path(h) else { return Vec::new() };
        let Some(g) = self.live(g_path) else { return Vec::new() };
        if let (EndRef::Mounted(i0), EndRef::Mounted(i1)) = (first, last) {
            return (i0..=i1).filter(|&j| self.member(j, g)).filter_map(|j| {
                let len = self.content_of(j).len();
                let (a, b) = (if j == i0 { a.min(len) } else { 0 }, if j == i1 { b.min(len) } else { len });
                (a < b).then_some((j, a, b))
            }).collect();
        }
        let (first_ord, last_ord) = (self.tree_order(first), self.tree_order(last));
        self.group_texts(g).into_iter().filter_map(|j| {
            let end_j = EndRef::Mounted(j);
            let ord = self.tree_order(end_j);
            if ord < first_ord || ord > last_ord { return None; }
            let len = self.content_of(j).len();
            let (a, b) = (if end_j == first { a.min(len) } else { 0 }, if end_j == last { b.min(len) } else { len });
            (a < b).then_some((j, a, b))
        }).collect()
    }

    /// The text holding the selection's anchor and its selected byte range there (empty after a
    /// single click), for tests and tools.
    pub fn text_selection(&self) -> Option<(&str, std::ops::Range<usize>)> {
        let h = self.selection_holder_end()?;
        let sel = &self.text_sel;
        let range = match &sel.far {
            None => sel.anchor.min(sel.focus)..sel.anchor.max(sel.focus),
            Some(_) => {
                let f = self.focus_holder_end(h)?;
                if f == h {
                    sel.anchor.min(sel.focus)..sel.anchor.max(sel.focus)
                } else {
                    let len = self.end_content(h).len();
                    let (a, b) = if self.tree_order(h) < self.tree_order(f) {
                        (sel.anchor.min(len), len)
                    } else {
                        (0, sel.anchor.min(len))
                    };
                    if a < b { a..b } else { sel.anchor..sel.anchor }
                }
            }
        };
        Some((sel.path.as_deref()?, range))
    }

    /// The selected text as drawn, as copying puts it on the clipboard without an
    /// [`El::selection_copy`] (as authored, before `El::text_case`). Across a selection group's
    /// texts: a newline between texts, a tab between texts side by side (table cells).
    ///
    /// When the selection spans unbuilt rows of an [`El::virtual_rows`] or [`El::virtualize`]
    /// list, those rows are built on demand (view only: no layout, mount or GPU) to collect their
    /// selectable text in tree order. On-demand collection is bounded to 10,000 unbuilt rows and
    /// 4 MiB of UTF-8 text; past either cap, collection stops building further unbuilt rows and
    /// truncates the returned string on a character boundary.
    pub fn selected_text(&self) -> Option<String> {
        if self.selection_spans_unmounted() {
            return self.collect_virtual_selection(None).map(|s| s.text);
        }
        let out = self.joined(&self.selection_ranges());
        (!out.is_empty()).then_some(out)
    }

    /// The selection as [`Tx::text_selection`] reads it, `None` with nothing selected.
    pub fn selection(&self) -> Option<TextSelection> {
        if self.selection_spans_unmounted() {
            return self.collect_virtual_selection(None);
        }
        let ranges = self.selection_ranges();
        if ranges.is_empty() { return None; }
        let texts = ranges.iter().map(|&(i, a, b)| self.selected_piece(i, a..b, None)).collect();
        Some(TextSelection { texts, text: self.joined(&ranges), backward: self.selection_backward() })
    }

    /// [`Runtime::selection`] for a handler's `Tx`, kept until a commit or the selection changes.
    pub(super) fn selection_snapshot(&self) -> Option<Rc<TextSelection>> {
        self.text_sel.path.as_ref()?;
        {
            let cache = self.sel_snapshot.borrow();
            if let Some(s) = cache.as_ref().filter(|s| self.same_from(&s.from)) { return s.selection.clone(); }
        }
        let from = self.selection_from();
        let selection = self.selection().map(Rc::new);
        *self.sel_snapshot.borrow_mut() = Some(Snapshot { from, selection: selection.clone() });
        selection
    }

    fn same_from(&self, from: &Basis) -> bool {
        let sel = &self.text_sel;
        from.0 == self.commits
            && from.1.as_deref() == sel.path.as_deref()
            && from.2 == sel.anchor
            && from.3 == sel.focus
            && from.4.as_deref() == sel.far.as_ref().map(|f| f.0.as_str())
            && from.5 == sel.dragging
    }

    fn selection_from(&self) -> Basis {
        let sel = &self.text_sel;
        (self.commits, sel.path.clone(), sel.anchor, sel.focus, sel.far.as_ref().map(|f| f.0.clone()), sel.dragging)
    }

    /// The `El::on_selection_change` calls owed since the last check: each element whose part
    /// of the selection changed, started or cleared.
    pub(super) fn selection_changes(&mut self) -> Vec<Action> {
        if self.sel_heard.from.as_ref().is_some_and(|f| self.same_from(f)) { return Vec::new(); }
        // A release is heard by whoever holds the selection, though it didn't change.
        let released = self.sel_heard.from.as_ref().is_some_and(|f| f.5 && !self.text_sel.dragging);
        self.sel_heard.from = Some(self.selection_from());
        if self.sel_heard.by_path.is_empty() && !self.nodes.iter().any(|n| n.handlers.get().is_some_and(|h| h.on_selection_change.is_some())) {
            return Vec::new();
        }
        let watchers: Vec<usize> = (0..self.nodes.len())
            .filter(|&i| self.nodes[i].handlers.get().is_some_and(|h| h.on_selection_change.is_some()) && !self.subtree_exiting(i) && self.allowed(i))
            .collect();
        if watchers.is_empty() && self.sel_heard.by_path.is_empty() { return Vec::new(); }
        let mut selected: Vec<usize> = self.selection_ranges().into_iter().filter(|r| r.1 < r.2).map(|r| r.0).collect();
        if self.has_nonempty_selection()
            && let Some(h) = self.selection_holder_end()
        {
            for e in [Some(h), self.focus_holder_end(h)].into_iter().flatten() {
                if let virtual_selection::EndRef::Offscreen(vi, ti) = e
                    && let Some(ln) = self.live(&self.offscreen_text(vi, ti).list_path)
                    && !selected.contains(&ln)
                {
                    selected.push(ln);
                }
            }
        }
        let inside = |w: usize, j: usize| std::iter::successors(Some(j), |&p| self.nodes[p].parent).any(|p| p == w);
        if self.sel_heard.by_path.is_empty() && !watchers.iter().any(|&w| selected.iter().any(|&j| inside(w, j))) {
            return Vec::new();
        }
        let sel = (!selected.is_empty()).then(|| self.selection_snapshot()).flatten();
        let (mut out, mut heard) = (Vec::new(), HashMap::default());
        for w in watchers {
            let path = self.nodes[w].path.clone();
            let now = sel.clone().filter(|_| selected.iter().any(|&j| inside(w, j)));
            let before = self.sel_heard.by_path.remove(&path).or_else(|| {
                let k = self.sel_heard.by_path.keys().find(|k| same_keyed_path(k, &path))?.clone();
                self.sel_heard.by_path.remove(&k)
            });
            let same = match (&before, &now) {
                (Some(a), Some(b)) => {
                    Rc::ptr_eq(a, b)
                        || (a.text == b.text
                            && a.backward == b.backward
                            && a.texts.len() == b.texts.len()
                            && a.texts.iter().zip(&b.texts).all(|(x, y)| {
                                x.key == y.key
                                    && x.range == y.range
                                    && x.len == y.len
                                    && x.whole == y.whole
                                    && x.text == y.text
                                    && (x.path == y.path || same_keyed_path(&x.path, &y.path))
                            }))
                }
                (a, b) => a.is_none() && b.is_none(),
            };
            if !same || (released && now.is_some()) {
                let change = SelectionChange { selection: now.clone(), was: before.is_some(), dragging: self.text_sel.dragging };
                out.push(Action::SelectionChange(path.clone(), change));
            }
            if let Some(now) = now { heard.insert(path, now); }
        }
        self.sel_heard.by_path = heard;
        out
    }

    /// Node `i` as a [`SelectedText`] with `range` selected, its path relative to `base`'s.
    pub(super) fn selected_piece(&self, i: usize, range: Range<usize>, base: Option<usize>) -> SelectedText {
        let n = &self.nodes[i];
        let path = match base {
            Some(b) if b == i => String::new(),
            Some(b) => n.path.strip_prefix(&self.nodes[b].path).and_then(|p| p.strip_prefix('/')).unwrap_or(&n.path).to_owned(),
            None => n.path.clone(),
        };
        let len = self.content_of(i).len();
        let (full_a, full_b) = n.text.as_ref().filter(|t| !t.uncopied.is_empty())
            .map_or((0, len), |t| trim_uncopied(&t.uncopied, (0, len)));
        let whole = !range.is_empty() && len > 0 && range.start <= full_a && range.end >= full_b;
        let text = n.text.as_ref().and_then(|t| t.copy_slice(range.start as u32, range.end as u32)).unwrap_or_default();
        SelectedText { path, key: n.key.clone(), range, len, whole, text }
    }

    /// What copying the selection puts on the clipboard: the nearest [`El::selection_copy`]
    /// around it says, else the text as drawn.
    pub(crate) fn copy_text(&self) -> Option<String> {
        if self.selection_spans_unmounted() {
            return self.copy_virtual_text();
        }
        let ranges = self.selection_ranges();
        let (&(first, ..), &(last, ..)) = (ranges.first()?, ranges.last()?);
        let drawn = self.joined(&ranges);
        let around = |a: usize, j: usize| std::iter::successors(Some(j), |&p| self.nodes[p].parent).any(|p| p == a);
        let provider = std::iter::successors(Some(first), |&p| self.nodes[p].parent)
            .find(|&p| self.nodes[p].handlers.selection_copy.is_some() && around(p, last));
        if let Some(p) = provider && let Some(f) = self.nodes[p].handlers.selection_copy.clone() {
            let g = self.group_of(first);
            let texts = (p..self.nodes.len())
                .take_while(|&j| around(p, j))
                .filter(|&j| self.is_selectable(j) && !self.subtree_exiting(j) && self.allowed(j))
                .filter(|&j| match g { Some(g) => self.group_of(j) == Some(g), None => j == first })
                .map(|j| self.selected_piece(j, range_in(&ranges, j).map_or(0..0, |(a, b)| a..b), Some(p)))
                .collect();
            if let Some(text) = f(&TextSelection { texts, text: drawn.clone(), backward: self.selection_backward() }) { return Some(text); }
        }
        (!drawn.is_empty()).then_some(drawn)
    }

    /// The texts of `ranges` joined as copying joins them.
    fn joined(&self, ranges: &[(usize, usize, usize)]) -> String {
        let mut out = CopyJoin::default();
        let mut prev: Option<usize> = None;
        for &(i, a, b) in ranges {
            let Some(spec) = self.nodes[i].text.as_ref() else { continue };
            let Some(piece) = spec.copy_slice(a as u32, b as u32) else { continue };
            out.push(spec, a..b, &piece, prev.is_some_and(|p| self.beside(p, i)), usize::MAX);
            prev = Some(i);
        }
        out.finish()
    }

    /// Text `i` was drawn beside text `p`, to its right on the same line (a table row's cells).
    pub(crate) fn beside(&self, p: usize, i: usize) -> bool {
        let (pb, nb) = (self.drawn_box(p), self.drawn_box(i));
        nb[1] < pb[3] - 0.5 && nb[0] >= pb[2] - 0.5
    }

    /// Selects from the start of text `first` to the end of text `last`, a later text of its
    /// selection group or `first` itself.
    pub(crate) fn select_span(&mut self, first: usize, last: usize) {
        use virtual_selection::EndRef;
        self.select_span_ends(EndRef::Mounted(first), EndRef::Mounted(last));
    }

    /// The held press is selecting text by dragging, and has selected some.
    pub(super) fn selected_by_drag(&self) -> bool {
        self.text_sel.dragging && self.press.held().is_some_and(|h| h.moved) && (!self.selection_ranges().is_empty() || self.has_nonempty_selection())
    }

    pub(super) fn clear_text_selection(&mut self) {
        virtual_selection::clear_held(&mut self.text_sel);
        self.sync_selection_pins();
    }

    /// Byte offset of node `i`'s text under window point (x, y), where it was drawn this frame.
    fn text_index_at(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        let n = &self.nodes[i];
        let (spec, layout) = (n.text.as_ref()?, n.text_layout.as_ref()?);
        let s = self.env.scale;
        let z = self.drawn_scale.get(i).copied().unwrap_or(1.0).max(1e-4);
        let (rect, x, y) = self.unturned_box_and_point(i, x, y);
        let [ox, oy] = links::origin(n, spec, layout, rect, s, z);
        let px = s / z;
        let at = crate::text::cursor_from_point(layout, (x - ox) * px, (y - oy) * px).index();
        Some(n.elided.map_or(at, |e| e.content(at)).min(spec.content.len()))
    }

    /// The text of group `g` a drag from `anchor` reaches at (x, y), and the byte there. Texts
    /// whose boxes overlap vertically (a table row's cells) form a band; in a band the point
    /// takes the text nearest it across. Between bands a drag down stops at the end of the band
    /// above, and a drag up at the start of the band below, so it reaches no further than the
    /// pointer has.
    fn group_point(&self, g: usize, anchor: virtual_selection::EndRef, x: f32, y: f32) -> Option<(usize, usize)> {
        use virtual_selection::EndRef;
        let mut boxes: Vec<_> = self.group_texts(g).into_iter().filter_map(|j| Some((j, self.shown_box(j)?))).collect();
        boxes.sort_by(|a, b| a.1[1].total_cmp(&b.1[1]));
        // Bands as (top, bottom, first text, last text), with their texts in `boxes[from..to]`.
        let mut bands: Vec<(f32, f32, usize, usize)> = Vec::new();
        for (k, &(_, r)) in boxes.iter().enumerate() {
            match bands.last_mut() {
                Some(b) if r[1] < b.1 => { b.1 = b.1.max(r[3]); b.3 = k + 1; }
                _ => bands.push((r[1], r[3], k, k + 1)),
            }
        }
        let texts = |b: &(f32, f32, usize, usize)| boxes[b.2..b.3].iter().map(|t| t.0);
        if let Some(b) = bands.iter().find(|b| y >= b.0 && y < b.1) {
            let &(j, _) = boxes[b.2..b.3].iter().min_by(|p, q| dist_x(p.1, x).total_cmp(&dist_x(q.1, x)))?;
            return Some((j, self.text_index_at(j, x, y)?));
        }
        let above = bands.iter().rev().find(|b| b.1 <= y);
        let below = bands.iter().find(|b| b.0 > y);
        match (above, below) {
            (a, Some(b)) if a.is_none() || self.tree_order(EndRef::Mounted(texts(b).min()?)) <= self.tree_order(anchor) => {
                Some((texts(b).min()?, 0))
            }
            (Some(a), _) => texts(a).max().map(|j| (j, self.content_of(j).len())),
            _ => None,
        }
    }

    /// The part of text `j` (x0, y0, x1, y1) that shows inside the clips around it, None when
    /// none does. Text clipped out of sight can lie under the words that show (a folded part
    /// above them, or the hidden lines of a long folded prompt), and a drag that reached it
    /// selected everything between it and the press.
    fn shown_box(&self, j: usize) -> Option<[f32; 4]> {
        let r = self.drawn_box(j);
        if self.inh_scratch.len() != self.nodes.len() || self.drawn.get(j).is_none_or(|d| d[2] <= 0.0 && d[3] <= 0.0) {
            return Some(r);
        }
        let c = self.screen_clip_of_parent(Some(j)).map(|v| v / self.env.scale);
        let v = [r[0].max(c[0]), r[1].max(c[1]), r[2].min(c[2]), r[3].min(c[3])];
        (v[0] < v[2] && v[1] < v[3]).then_some(v)
    }

    /// A primary press on `target` (None: on nothing). On selectable text away from its links it
    /// starts a selection: one click places it, two select a word, three a paragraph, and Shift
    /// extends the current one, into another text of its selection group too. Anywhere else it
    /// clears the selection.
    pub(super) fn selection_down(&mut self, target: Option<usize>, x: f32, y: f32, word: bool) {
        use virtual_selection::EndRef;
        let Some(i) = target.filter(|&i| self.is_selectable(i) && self.link_at(i, x, y).is_none()) else {
            self.clear_text_selection();
            self.text_sel.last = None;
            return;
        };
        let Some(at) = self.text_index_at(i, x, y) else { return };
        let path = self.nodes[i].path.clone();
        let content = self.content_of(i).to_owned();
        let now = self.now_clock();
        let clicks = if word { 2 } else { match &self.text_sel.last {
            Some((p, t, px, py, n)) if *p == path && now - t <= self.double_click_interval
                && (x - px).abs() <= CLICK_SLOP && (y - py).abs() <= CLICK_SLOP => (n + 1).min(3),
            _ => 1,
        } };
        self.text_sel.last = Some((path.clone(), now, x, y, clicks));
        let i_end = EndRef::Mounted(i);
        let holder = self.selection_holder_end().filter(|&h| h == i_end || self.end_same_group(h, i_end));
        match holder {
            Some(h) if clicks == 1 && self.text_sel.shift => {
                // Keep the anchor; the focus follows the pointer from here.
                let sel = &mut self.text_sel;
                sel.unit = Unit::Char;
                sel.origin = (sel.anchor, sel.anchor);
                sel.focus = at;
                sel.far = (h != i_end).then_some((path, content));
                sel.focus_end = None;
            }
            _ => {
                let unit = match clicks { 1 => Unit::Char, 2 => Unit::Word, _ => Unit::Paragraph };
                let origin = self.unit_in(i, at, unit);
                let sel = &mut self.text_sel;
                (sel.unit, sel.origin) = (unit, origin);
                (sel.anchor, sel.focus) = sel.origin;
                sel.path = Some(path);
                sel.content = content;
                (sel.far, sel.anchor_end, sel.focus_end) = (None, None, None);
            }
        }
        self.text_sel.dragging = true;
        self.seal_between();
    }

    /// The held press moved: extend the selection it started to the pointer, by its unit, into
    /// the other texts of its selection group. The press itself may belong to an ancestor (a
    /// clickable row), so this follows the selection's own text.
    pub(super) fn selection_drag(&mut self, x: f32, y: f32) {
        use virtual_selection::EndRef;
        if !self.text_sel.dragging { return; }
        let Some(h) = self.selection_holder_end() else { self.clear_text_selection(); return };
        let g = self.end_group_path(h).and_then(|gp| self.live(gp));
        let Some((j, at)) = (match (g, h) {
            (Some(g), _) => self.group_point(g, h, x, y),
            (None, EndRef::Mounted(hi)) => self.text_index_at(hi, x, y).map(|at| (hi, at)),
            (None, EndRef::Offscreen(..)) => None,
        }) else { return };
        let j_end = EndRef::Mounted(j);
        let at = self.unit_in(j, at, self.text_sel.unit);
        let (anchor, focus) = if j_end == h {
            extend(self.text_sel.origin, at)
        } else if self.tree_order(j_end) > self.tree_order(h) {
            (self.text_sel.origin.0, at.1)
        } else {
            (self.text_sel.origin.1, at.0)
        };
        let same_far = if j_end == h {
            self.text_sel.far.is_none()
        } else {
            self.text_sel.far.as_ref().is_some_and(|(p, _)| *p == self.nodes[j].path)
        };
        if (self.text_sel.anchor, self.text_sel.focus) == (anchor, focus) && same_far {
            return;
        }
        self.text_sel.anchor = anchor;
        self.text_sel.focus = focus;
        if j_end == h {
            self.text_sel.far = None;
            self.text_sel.focus_end = None;
        } else if !same_far {
            self.text_sel.far = Some((self.nodes[j].path.clone(), self.content_of(j).to_owned()));
            self.text_sel.focus_end = None;
        }
        if !same_far {
            self.seal_between();
        } else {
            self.sync_selection_pins();
        }
    }

    /// Cmd+C (Ctrl+C elsewhere) copies a non-empty selection and Cmd+A selects all of the text
    /// holding it, or all of its selection group (a click on it makes it the holder, as it makes
    /// an AppKit text view first responder), unless a text field has the keyboard. Only the plain
    /// chord: with Shift, Option (Alt) or Control added it's the app's shortcut.
    pub(super) fn selection_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let crate::Key::Command(c @ ('c' | 'a')) = ev.key else { return false };
        let m = ev.mods;
        let plain = m.cmd && !m.shift && !m.alt && !(m.ctrl && m.logo);
        if !plain || self.focused_input().is_some() || (c == 'c' && self.text_sel.path.is_none()) { return false; }
        let holder = self.selection_holder_end();
        if holder.is_none() { self.clear_text_selection(); }
        if c == 'a' { return self.select_all_key(); }
        let Some(h) = holder else { return false };
        let path = self.end_path(h).to_owned();
        let Some(text) = self.copy_text() else { return false };
        log_event!(self, "copy {path}");
        tx.copy(text);
        true
    }

    /// A secondary click on selectable text `path` without its own `on_secondary_press` opens the
    /// Copy menu. On macOS it selects the word under the pointer first when the click is outside
    /// the selection, as AppKit text does; elsewhere the selection stays as it is, as Chromium's
    /// does on Windows and Linux. False when the app's handler should run instead.
    pub(super) fn selection_menu(&mut self, path: &str, x: f32, y: f32) -> bool {
        let Some(i) = self.live(path) else { return false };
        if !self.is_selectable(i) || self.nodes[i].handlers.on_secondary_press.is_some() { return false; }
        if SECONDARY_SELECTS_WORD
            && let Some(at) = self.text_index_at(i, x, y)
            && range_in(&self.selection_ranges(), i).is_none_or(|(a, b)| at < a || at >= b)
        {
            let content = self.content_of(i).to_owned();
            let (a, b) = self.unit_in(i, at, Unit::Word);
            self.text_sel = Held { path: Some(path.to_owned()), content, anchor: a, focus: b, far: None,
                anchor_end: None, focus_end: None, mounted_between: HashMap::default(), between: 0,
                unit: Unit::Word, origin: (a, b), dragging: false, last: None, shift: self.text_sel.shift,
                pick: std::mem::take(&mut self.text_sel.pick) };
            self.seal_between();
        }
        let text = self.copy_text();
        let mut copy = crate::PopupItem::new("copy", crate::labels::label(crate::Label::Copy)).enabled(text.is_some());
        // The primary modifier, as the copy key handler takes it: ⌘C on macOS, Ctrl+C elsewhere.
        if let Ok(shortcut) = "mod+c".parse::<crate::Shortcut>() { copy = copy.key_equivalent(shortcut); }
        log_event!(self, "selection menu {path}");
        self.popups.push_back(crate::popup_menu::Request {
            menu: crate::PopupMenu::new().item(copy),
            anchor: crate::PopupAnchor::Point(x, y),
            on_pick: Box::new(move |_, picked, tx| {
                if picked == Some("copy") && let Some(text) = text { tx.copy(text); }
            }),
            field: None,
        });
        true
    }
}

#[path = "../virtual/selection.rs"]
pub(crate) mod virtual_selection;
#[cfg(test)]
#[path = "../virtual_selection_tests.rs"]
mod virtual_selection_tests;

#[cfg(test)]
mod tests {
    use super::{Unit, extend, unit_at};

    #[test]
    fn units_cover_words_spaces_and_paragraphs() {
        let s = "opencode --port 4096\nnext line";
        assert_eq!(unit_at(s, 3, Unit::Word), (0, 8));
        assert_eq!(unit_at(s, 8, Unit::Word), (8, 9), "a space run is its own word");
        assert_eq!(unit_at(s, 12, Unit::Word), (11, 15));
        assert_eq!(unit_at(s, s.len(), Unit::Word), (26, 30), "past the end takes the last word");
        assert_eq!(unit_at(s, 5, Unit::Paragraph), (0, 21), "a paragraph takes its line break");
        assert_eq!(unit_at(s, 23, Unit::Paragraph), (21, s.len()));
        assert_eq!(unit_at("", 0, Unit::Word), (0, 0));
    }

    #[test]
    fn line_ends_take_the_lines_last_word_and_paragraph() {
        let s = "first line\nsecond";
        assert_eq!(unit_at(s, 10, Unit::Word), (6, 10), "on the break: the line's last word");
        assert_eq!(unit_at("a\r\nb", 1, Unit::Word), (0, 1));
        assert_eq!(unit_at("first line\n", 11, Unit::Paragraph), (0, 11), "past the end: the last paragraph");
        assert_eq!(unit_at("first line\n", 11, Unit::Word), (6, 10));
        assert_eq!(unit_at("a\n\nb", 2, Unit::Word), (2, 3), "an empty line's break is its own word");
    }

    #[test]
    fn offsets_inside_a_character_stay_on_its_boundary() {
        let s = "a —";
        for i in 0..=s.len() + 2 {
            for unit in [Unit::Char, Unit::Word, Unit::Paragraph] {
                let (a, b) = unit_at(s, i, unit);
                assert!(s.is_char_boundary(a) && s.is_char_boundary(b), "{i} {unit:?}");
            }
        }
    }

    #[test]
    fn a_drag_keeps_the_unit_it_began_on_whole() {
        assert_eq!(extend((8, 12), (20, 24)), (8, 24));
        assert_eq!(extend((8, 12), (0, 4)), (12, 0));
        assert_eq!(extend((8, 12), (9, 10)), (8, 12));
        assert_eq!(extend((5, 5), (2, 2)), (5, 2));
    }
}
