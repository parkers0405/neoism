//! Virtual row bookkeeping. Only geometry outside the mounted window is estimated.

use std::cell::{Cell, Ref, RefCell};
use std::ops::Range;

use rune_taffy::CompactStyle;
use taffy::style::{Display, FlexDirection, FlexWrap, LengthPercentage, LengthPercentageAuto, Position};

use crate::El;
use crate::runtime::{HashMap, HashSet};

/// What a row keeps while its key stays.
#[derive(Debug, Clone, Copy)]
struct Row {
    index: usize,
    /// Its measured height, once it has been laid out.
    height: Option<f32>,
}

#[derive(Debug, Default)]
pub(crate) struct VirtualList {
    /// Row keys in order; replace them with [`VirtualList::set_keys`].
    pub keys: Vec<String>,
    rows: HashMap<String, Row>,
    /// The commit each row in `keys` first appeared in.
    born: Vec<u64>,
    /// `keys` changed since the frame began ([`VirtualList::frame_begins`]).
    keys_fresh: bool,
    /// The last change to `keys` kept a row the reader could see.
    held: bool,
    /// Rows mounted with their insertion transition and not yet drawn; the runtime lands the
    /// ones that drew nothing (`runtime/unseen_presence.rs`).
    pub entering: HashSet<String>,
    /// Each row's height, measured or estimated, in `keys` order.
    row_heights: Vec<f32>,
    /// Running totals of `row_heights` (one longer), rebuilt when one changes. Every frame reads
    /// them, so a long list doesn't hash every key to find its window.
    sums: RefCell<Vec<f32>>,
    sums_stale: Cell<bool>,
    pub range: Range<usize>,
    pub before: f32,
    pub after: f32,
    estimate: f32,
    /// Rows are built by index (`El::virtual_rows`), so a commit costs only the rows it mounts.
    /// Such a list moves its window in small steps and spreads rows new to it over frames.
    pub indexed: bool,
    /// The rows the last applied commit mounted. Every measurement of a commit compares with
    /// these, not with the pass before it.
    pub shown: HashSet<String>,
    /// The rows drawn inside the viewport on the last frame (`Runtime::note_drawn_rows`).
    pub drawn: Vec<String>,
    /// The row holding the focused element, kept while that row is scrolled out.
    pub focused_row: Option<String>,
    /// The rows holding the text selection's anchor and focus, kept built while scrolled out.
    pub pinned_rows: Vec<String>,
    /// Offscreen row state for text selection.
    pub selection: crate::runtime::selectable::virtual_selection::PinnedSelection,
    /// The selected row on the last commit of an `El::reveal_selected(true)` stack.
    pub selected_row: Option<String>,
    pub reveal_selected: Option<String>,
    pub scroll_path: String,
    pub viewport: f32,
    pub top: f32,
    pub pad_top: f32,
    pub pad_bottom: f32,
    gap: f32,
    pub anchor: Option<(String, f32)>,
    /// The `El::virtual_rows_version` that `keys` came with.
    pub keys_version: Option<u64>,
    /// The visible row range last sent to `El::on_visible_rows`.
    pub reported_visible: Option<Range<usize>>,
}

/// The most an element can be tall before layout, so a list's first showing, which has no
/// measured viewport yet, mounts what its viewport can show rather than the window's height.
/// Its own height in points wins; otherwise `avail`, what its parent leaves it. An estimate
/// short of the laid-out height only costs a retry pass, which never presents a blank.
pub(crate) fn height_bound(style: &CompactStyle, avail: f32) -> f32 {
    let h = style.size().height.into_option().unwrap_or(avail);
    let h = length(style.max_size().height).map_or(h, |m| h.min(m));
    length(style.min_size().height).map_or(h, |m| h.max(m))
}

/// A column flex container's vertical gap in points; zero for non-column containers.
pub(crate) fn gap_y(style: &CompactStyle) -> f32 {
    let column = style.display() == Display::Flex && style.flex_wrap() == FlexWrap::NoWrap
        && matches!(style.flex_direction(), FlexDirection::Column | FlexDirection::ColumnReverse);
    if !column { return 0.0; }
    match style.gap().height.expand() {
        taffy::style::ExpandedLengthPercentage::Length(x) => x,
        _ => 0.0,
    }
}

/// The most each child of an element whose bound is `bound` can be tall: what padding leaves
/// and, in a column, what siblings of fixed height and gaps don't take. A scroll viewport's
/// content is unbounded, as is an out-of-flow child not pinned to both its parent's top and
/// bottom.
pub(crate) fn child_bounds(style: &CompactStyle, scrolls: bool, bound: f32, children: &[El]) -> ChildBounds {
    if scrolls { return ChildBounds { outer: f32::INFINITY, inner: f32::INFINITY, fixed: 0.0, column: false } }
    let points = |p: LengthPercentage| match p.expand() {
        taffy::style::ExpandedLengthPercentage::Length(x) => x,
        _ => 0.0,
    };
    let pad = style.padding();
    let inner = bound - points(pad.top) - points(pad.bottom);
    let column = style.display() == Display::Flex && style.flex_wrap() == FlexWrap::NoWrap
        && matches!(style.flex_direction(), FlexDirection::Column | FlexDirection::ColumnReverse);
    if !column || children.len() < 2 { return ChildBounds { outer: bound, inner, fixed: 0.0, column: false } }
    let gaps = points(style.gap().height) * children.iter().filter(|c| in_flow(&c.style)).count().saturating_sub(1) as f32;
    ChildBounds { outer: bound, inner: inner - gaps, fixed: children.iter().map(|c| fixed_height(&c.style)).sum(), column }
}

pub(crate) struct ChildBounds {
    /// The parent's bound: an absolute child's insets are from its padding box.
    outer: f32,
    inner: f32,
    fixed: f32,
    column: bool,
}

impl ChildBounds {
    pub fn of(&self, child: &El) -> f32 {
        let style = &child.style;
        let bound = if style.position() == Position::Absolute {
            let inset = style.inset();
            match (length(inset.top), length(inset.bottom)) {
                (Some(top), Some(bottom)) => self.outer - top - bottom,
                _ => f32::INFINITY,
            }
        } else if self.column && in_flow(style) {
            self.inner - (self.fixed - fixed_height(style))
        } else {
            self.inner
        };
        bound.max(0.0)
    }
}

fn length(d: LengthPercentageAuto) -> Option<f32> {
    match d.expand() {
        taffy::style::ExpandedLengthPercentageAuto::Length(x) => Some(x),
        _ => None,
    }
}

fn in_flow(style: &CompactStyle) -> bool {
    style.display() != Display::None && style.position() != Position::Absolute
}

/// The height a column's child takes whatever the others need: its own height when it can't
/// shrink, and its minimum.
fn fixed_height(style: &CompactStyle) -> f32 {
    if !in_flow(style) { return 0.0 }
    let min = length(style.min_size().height).unwrap_or(0.0);
    if style.flex_shrink() > 0.0 { return min }
    let h = style.size().height.into_option().unwrap_or(0.0);
    length(style.max_size().height).map_or(h, |m| h.min(m)).max(min)
}

/// How far ahead an index-built list keeps rows mounted, in viewports either side.
const OVERSCAN: f32 = 2.0;
/// An index-built list moves its window once the viewport comes this close to its edge.
const INDEXED_EDGE: f32 = 1.75;
/// How much of an index-built list's overscan, in viewports per side, may be new in one commit.
/// Rows past it mount over the next frames, so a jump or a prepended page doesn't shape it all
/// in one frame.
const INDEXED_BUDGET: f32 = 0.5;

impl VirtualList {
    pub fn height(&self, key: &str) -> f32 {
        self.rows.get(key).and_then(|r| r.height).unwrap_or(self.estimate)
    }

    pub fn has(&self, key: &str) -> bool {
        self.rows.contains_key(key)
    }

    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.rows.get(key).map(|r| r.index)
    }

    /// Whether row `i`, mounting in `commit`, skips its insertion transition. A row scrolled into
    /// view was there all along, so it does. One new to the rows enters as an insertion, and so
    /// does one that a change to the rows this frame (a filter, a sort) mounts: the rows it
    /// replaces leave with their exits. Rows mounting with their transition are noted in
    /// `entering`, and the runtime lands the ones that drew out of view, so only what shows fades.
    pub fn mounts_quietly(&mut self, i: usize, commit: u64) -> bool {
        let quiet = self.born[i] < commit && !(self.keys_fresh && !self.shown.contains(&self.keys[i]));
        if !quiet { self.entering.insert(self.keys[i].clone()); }
        quiet
    }

    /// The rows changed this frame and none the reader could see stayed (a filter): the rows
    /// that leave hold no place, so what comes in shows where they stood.
    pub fn replaced_view(&self) -> bool {
        self.keys_fresh && !self.held
    }

    /// A new frame: what changed the keys before it has been drawn. Within a frame a scroll the
    /// change asks for (`Tx::scroll_into_view`) commits again, and the rows it brings in are
    /// still the change's.
    pub fn frame_begins(&mut self) {
        self.keys_fresh = false;
    }

    /// Whether the rows are exactly `keys`, in order.
    pub fn same_keys<'a>(&self, keys: impl ExactSizeIterator<Item = Option<&'a str>>) -> bool {
        keys.len() == self.keys.len() && keys.zip(&self.keys).all(|(a, b)| a == Some(b.as_str()))
    }

    /// Replaces the rows. Keys that stay keep their measured height and birth; new ones are born
    /// at `commit`. Returns the first row visible at `offset` (when given) that stays, with its
    /// top before the change, to anchor the reader to: none when no row in view stays.
    pub fn set_keys(&mut self, keys: Vec<String>, commit: u64, offset: Option<f32>) -> Option<(String, f32)> {
        let mut rows = HashMap::with_capacity_and_hasher(keys.len(), Default::default());
        let mut born = Vec::with_capacity(keys.len());
        let mut heights = Vec::with_capacity(keys.len());
        for (index, key) in keys.iter().enumerate() {
            let old = self.rows.get(key);
            let height = old.and_then(|r| r.height);
            born.push(old.map_or(commit, |r| self.born[r.index]));
            heights.push(height.unwrap_or(self.estimate));
            assert!(rows.insert(key.clone(), Row { index, height }).is_none(), "virtual row keys must be unique");
        }
        // A change that keeps none of the rows in view (a filter) leaves nothing to hold.
        let anchor = offset.and_then(|offset| self.first_visible(offset, |k| rows.contains_key(k)))
            .filter(|(_, top)| offset.is_some_and(|offset| *top < offset + self.viewport));
        if keys != self.keys {
            self.keys_fresh = true;
            self.held = if offset.is_some() { anchor.is_some() } else { self.keys.iter().any(|k| rows.contains_key(k)) };
        }
        self.rows = rows;
        self.born = born;
        self.keys = keys;
        self.row_heights = heights;
        self.sums_stale.set(true);
        self.reported_visible = None;
        anchor
    }

    pub fn set_estimate(&mut self, estimate: f32) {
        if estimate != self.estimate {
            self.estimate = estimate;
            self.row_heights = self.keys.iter().map(|k| self.height(k)).collect();
            self.sums_stale.set(true);
        }
    }

    pub fn set_gap(&mut self, gap: f32) {
        if gap != self.gap {
            self.gap = gap;
            self.sums_stale.set(true);
        }
    }

    pub fn set_height(&mut self, key: &str, height: f32) {
        let Some(row) = self.rows.get_mut(key) else { return };
        row.height = Some(height);
        let i = row.index;
        if self.row_heights[i] != height {
            self.row_heights[i] = height;
            self.sums_stale.set(true);
        }
    }

    fn sums(&self) -> Ref<'_, Vec<f32>> {
        if self.sums_stale.replace(false) || self.sums.borrow().len() != self.row_heights.len() + 1 {
            let mut sums = self.sums.borrow_mut();
            sums.clear();
            sums.push(0.0);
            let mut y = 0.0;
            for h in &self.row_heights {
                y += h + self.gap;
                sums.push(y);
            }
        }
        self.sums.borrow()
    }

    pub fn prefix(&self, end: usize) -> f32 {
        let sums = self.sums();
        if end == 0 || end < self.keys.len() { sums[end] } else { sums[end] - self.gap }
    }

    /// The bottom of the first `end` rows (without the gap after row `end - 1`).
    pub fn row_end(&self, end: usize) -> f32 {
        if end == 0 { 0.0 } else { self.sums()[end] - self.gap }
    }

    /// The first row still visible at `offset`, among those `keep` accepts, and its top.
    pub fn first_visible(&self, offset: f32, keep: impl Fn(&str) -> bool) -> Option<(String, f32)> {
        let sums = self.sums();
        let first = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= offset);
        (first..self.keys.len())
            .find(|&i| keep(&self.keys[i]))
            .map(|i| (self.keys[i].clone(), self.pad_top + sums[i]))
    }

    /// The furthest the rows can scroll: their extent less the viewport.
    pub fn max_offset(&self) -> f32 {
        (self.pad_top + self.prefix(self.keys.len()) + self.pad_bottom - self.viewport).max(0.0)
    }

    /// The row index range intersecting the scroll viewport (`offset .. offset + viewport`).
    pub fn visible_range(&self, offset: f32, pinned: bool) -> Range<usize> {
        if self.keys.is_empty() {
            return 0..0;
        }
        let offset = if pinned { self.max_offset() } else { offset.clamp(0.0, self.max_offset()) };
        let sums = self.sums();
        let start = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= offset);
        let end = sums[..self.keys.len()].partition_point(|&s| self.pad_top + s < offset + self.viewport).max(start);
        start..end
    }

    /// Picks the rows to mount for `offset`, clamped to the rows' extent. With `budget` (the
    /// rows shown last commit), at most [`INDEXED_BUDGET`] of rows not among them mount per
    /// side beyond the viewport.
    pub fn select(&mut self, offset: f32, pinned: bool, budget: Option<&HashSet<String>>) {
        let offset = if pinned { self.max_offset() } else { offset.min(self.max_offset()) };
        let (start, end) = self.overscan(offset);
        let (mut start, mut end) = {
            let sums = self.sums();
            if let Some(shown) = budget.filter(|_| !pinned) {
                let mounted = |k: &str| shown.contains(k);
                // The viewport's own rows always mount; the overscan takes new rows up to a budget.
                let first = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= offset).max(start);
                let last = sums[..self.keys.len()].partition_point(|&s| self.pad_top + s < offset + self.viewport).max(first);
                let budget = self.viewport * INDEXED_BUDGET;
                let mut spent = 0.0;
                let mut from = first;
                while from > start {
                    let i = from - 1;
                    if !mounted(&self.keys[i]) {
                        if spent >= budget { break; }
                        spent += self.row_heights[i] + self.gap;
                    }
                    from = i;
                }
                let mut spent = 0.0;
                let mut to = last.min(end);
                while to < end {
                    if !mounted(&self.keys[to]) {
                        if spent >= budget { break; }
                        spent += self.row_heights[to] + self.gap;
                    }
                    to += 1;
                }
                (from, to.max(from))
            } else {
                (start, end)
            }
        };
        // Rows drawn in the viewport stay while motion still shows them where layout has moved
        // them out (rows below a fold gliding open), within an overscan of the window.
        let (mut lo, mut hi) = (start, end);
        for i in self.drawn.iter().filter_map(|k| self.index_of(k)) {
            if i < start && self.prefix(start) - self.prefix(i) <= self.viewport * OVERSCAN { lo = lo.min(i); }
            if i >= end && self.row_end(i + 1) - self.row_end(end) <= self.viewport * OVERSCAN { hi = hi.max(i + 1); }
        }
        (start, end) = (lo, hi);
        // A row still at its estimate grows or shrinks once measured; anchoring to it would move
        // the measured rows below, such as the reader's row under a page that just landed.
        self.anchor = if pinned { None } else { self.anchor_at(offset) };
        if pinned { start = start.min(end.saturating_sub(32)); }
        end = end.max(start);
        self.place(start..end);
    }

    /// Keeps the rows mounted now (clamped to the rows), sets the anchor for `offset` as
    /// [`select`](Self::select) does, and re-derives the space above and below them.
    pub fn hold(&mut self, offset: f32, pinned: bool) {
        let offset = if pinned { self.max_offset() } else { offset.min(self.max_offset()) };
        self.anchor = if pinned { None } else { self.anchor_at(offset) };
        let len = self.keys.len();
        self.place(self.range.start.min(len)..self.range.end.min(len));
    }

    fn place(&mut self, range: Range<usize>) {
        let total = self.prefix(self.keys.len());
        let (start, end) = (range.start, range.end.max(range.start));
        self.range = start..end;
        self.before = self.prefix(start);
        self.after = if start < end { total - self.row_end(end) } else { total - self.before };
    }

    fn anchor_at(&self, offset: f32) -> Option<(String, f32)> {
        let range = self.visible_range(offset, false);
        let sums = self.sums();
        range.into_iter().find(|&i| self.rows[&self.keys[i]].height.is_some())
            .map(|i| (self.keys[i].clone(), self.pad_top + sums[i]))
            .or_else(|| self.first_visible(offset, |_| true))
    }

    /// The rows within the overscan around `offset`: rows ending at or above its top go, rows
    /// starting above its bottom stay.
    fn overscan(&self, offset: f32) -> (usize, usize) {
        let lo = offset - self.viewport * OVERSCAN;
        let hi = offset + self.viewport * (1.0 + OVERSCAN);
        let sums = self.sums();
        let start = sums[1..].partition_point(|&s| self.pad_top + s - self.gap <= lo);
        (start, sums[..self.keys.len()].partition_point(|&s| self.pad_top + s < hi))
    }

    /// How tall the mounted rows outside the overscan around `offset` are: rows that grew since
    /// the window was picked and lay out for nothing.
    pub(crate) fn spare(&self, offset: f32, pinned: bool) -> f32 {
        if pinned || self.keys.is_empty() { return 0.0; }
        let (start, end) = self.overscan(offset.min(self.max_offset()));
        let above = self.prefix(start.min(self.range.end)) - self.prefix(self.range.start.min(start));
        let below = self.row_end(self.range.end) - self.row_end(end.max(self.range.start).min(self.range.end));
        above.max(0.0) + below.max(0.0)
    }

    pub fn anchor_delta(&self) -> f32 {
        let Some((key, old)) = &self.anchor else { return 0.0 };
        let Some(i) = self.index_of(key) else { return 0.0 };
        self.pad_top + self.prefix(i) - old
    }

    pub fn needs_window(&self, offset: f32, pinned: bool) -> bool {
        if self.keys.is_empty() { return false; }
        if pinned { return self.range.end != self.keys.len(); }
        let edge = self.viewport * if self.indexed { INDEXED_EDGE } else { 0.5 };
        let top = self.pad_top + self.before;
        let bottom = self.pad_top + self.row_end(self.range.end);
        (self.range.start > 0 && offset < top + edge)
            || (self.range.end < self.keys.len() && offset + self.viewport > bottom - edge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{col, el};

    #[test]
    fn gaps_surround_a_virtual_window_without_a_trailing_gap() {
        let mut v = VirtualList::default();
        v.set_estimate(20.0);
        v.set_gap(7.0);
        v.set_keys((0..100).map(|i| format!("r{i}")).collect(), 0, None);
        v.viewport = 40.0;
        assert_eq!(v.prefix(100), 2693.0);
        assert_eq!(v.prefix(50), 1350.0);
        assert_eq!(v.row_end(50), 1343.0);
        // In a gap the next row is the first visible row, not the one above it.
        assert_eq!(v.first_visible(21.0, |_| true), Some(("r1".into(), 27.0)));
        v.select(1350.0, false, None);
        let n = v.range.len();
        let mounted = n as f32 * 20.0 + n.saturating_sub(1) as f32 * 7.0;
        assert_eq!(v.before + mounted + v.after, 2693.0);
        assert_eq!(v.before, v.range.start as f32 * 27.0);
        v.set_height("r0", 35.0);
        assert_eq!(v.prefix(50), 1365.0, "measured heights replace only the estimate, not the gap");
        v.set_gap(3.0);
        assert_eq!(v.prefix(100), 2312.0, "a changed gap invalidates prefix sums");
        v.set_keys(Vec::new(), 1, None);
        assert_eq!(v.prefix(0), 0.0);
    }

    #[test]
    fn growth_gap_uses_the_vertical_axis_only() {
        assert_eq!(gap_y(&col().gaps(7.0, 19.0).style), 7.0);
        assert_eq!(gap_y(&crate::row().gaps(7.0, 19.0).style), 0.0);
    }

    #[test]
    fn an_absolute_child_is_bounded_only_when_pinned_top_and_bottom() {
        let children = [
            el().pin(Some(0.0), Some(10.0), Some(0.0), Some(20.0)),
            el().absolute(0.0, 10.0),
            el().h(40.0),
            el(),
        ];
        let parent = col().h(100.0).pt(5.0);
        let bounds = child_bounds(&parent.style, false, 100.0, &children);
        assert_eq!(bounds.of(&children[0]), 70.0, "between its insets in the padding box");
        assert_eq!(bounds.of(&children[1]), f32::INFINITY, "free to grow past its parent");
        assert_eq!(bounds.of(&children[3]), 55.0, "in flow, what padding and the fixed sibling leave");
    }
}
