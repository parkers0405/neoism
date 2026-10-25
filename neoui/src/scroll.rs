//! Scroll geometry and variable-height virtualization, independent of clocks,
//! window systems, and GPU state. Deltas are logical pixels, positive down/right.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::{Element, Frame, Rect, Size};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollState {
    offset: [f32; 2],
    viewport: [f32; 2],
    content: [f32; 2],
}

impl ScrollState {
    pub fn offset(&self) -> [f32; 2] { self.offset }
    pub fn viewport(&self) -> [f32; 2] { self.viewport }
    pub fn content(&self) -> [f32; 2] { self.content }
    pub fn maximum(&self) -> [f32; 2] {
        [(self.content[0] - self.viewport[0]).max(0.0), (self.content[1] - self.viewport[1]).max(0.0)]
    }
    pub fn set_metrics(&mut self, viewport: [f32; 2], content: [f32; 2]) -> bool {
        let finite = |value: f32| if value.is_finite() { value.max(0.0) } else { 0.0 };
        self.viewport = viewport.map(finite); self.content = content.map(finite);
        self.scroll_to(self.offset)
    }
    pub fn scroll_to(&mut self, offset: [f32; 2]) -> bool {
        let maximum = self.maximum();
        let next = [0, 1].map(|axis| {
            if offset[axis].is_finite() { offset[axis].clamp(0.0, maximum[axis]) }
            else { self.offset[axis].min(maximum[axis]) }
        });
        let changed = next != self.offset; self.offset = next; changed
    }
    pub fn scroll_by(&mut self, delta: [f32; 2]) -> bool {
        self.scroll_to([self.offset[0] + delta[0], self.offset[1] + delta[1]])
    }
    pub fn reveal(&mut self, bounds: Rect, margin: f32) -> bool {
        if !bounds.is_valid() { return false; }
        let margin = if margin.is_finite() { margin.max(0.0) } else { 0.0 };
        let mut next = self.offset;
        for (axis, (start, end)) in [(bounds.x, bounds.right()), (bounds.y, bounds.bottom())].into_iter().enumerate() {
            let margin = margin.min(self.viewport[axis] * 0.5);
            if start < self.offset[axis] + margin { next[axis] = start - margin; }
            else if end > self.offset[axis] + self.viewport[axis] - margin {
                next[axis] = if end - start > self.viewport[axis] - margin * 2.0 { start - margin }
                    else { end - self.viewport[axis] + margin };
            }
        }
        self.scroll_to(next)
    }
    pub fn content_origin(&self, viewport: Rect) -> [f32; 2] {
        [viewport.x - self.offset[0], viewport.y - self.offset[1]]
    }
    pub fn vertical_thumb(&self, track: Rect, minimum_height: f32) -> Option<Rect> {
        if track.is_empty() || self.content[1] <= self.viewport[1] || self.viewport[1] == 0.0 { return None; }
        let minimum = if minimum_height.is_finite() { minimum_height.max(0.0) } else { 0.0 };
        let height = (track.height * self.viewport[1] / self.content[1]).max(minimum).min(track.height);
        let travel = track.height - height;
        Some(Rect::new(track.x, track.y + travel * self.offset[1] / self.maximum()[1], track.width, height))
    }
    pub fn drag_vertical_thumb(&mut self, track: Rect, thumb_height: f32, pointer_y: f32, grab_offset: f32) -> bool {
        let travel = track.height - thumb_height;
        if travel <= 0.0 || !pointer_y.is_finite() || !grab_offset.is_finite() { return false; }
        self.scroll_to([self.offset[0], ((pointer_y - track.y - grab_offset) / travel).clamp(0.0, 1.0) * self.maximum()[1]])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VirtualListError { DuplicateKey(String), InvalidHeight, TooManyItems }

impl std::fmt::Display for VirtualListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateKey(key) => write!(f, "duplicate list item key: {key}"),
            Self::InvalidHeight => write!(f, "list heights must be finite and positive"),
            Self::TooManyItems => write!(f, "virtual list exceeds supported item count"),
        }
    }
}
impl std::error::Error for VirtualListError {}

#[derive(Clone, Debug)]
struct Item { key: String, height: f64 }

/// Prefix sums support O(log n) row-height updates and viewport lookup. Heights
/// use f64 internally so a long collection does not accumulate f32 prefix error.
#[derive(Clone, Debug, Default)]
struct Heights { tree: Vec<f64> }

impl Heights {
    fn build(items: &[Item]) -> Self {
        let mut result = Self { tree: vec![0.0; items.len() + 1] };
        for (index, item) in items.iter().enumerate() { result.add(index, item.height); }
        result
    }
    fn add(&mut self, index: usize, delta: f64) {
        let mut index = index + 1;
        while index < self.tree.len() { self.tree[index] += delta; index += index & index.wrapping_neg(); }
    }
    fn prefix(&self, count: usize) -> f64 {
        let mut index = count.min(self.tree.len().saturating_sub(1));
        let mut sum = 0.0;
        while index > 0 { sum += self.tree[index]; index &= index - 1; }
        sum
    }
    fn item_at(&self, offset: f64) -> usize {
        let count = self.tree.len().saturating_sub(1);
        if count == 0 { return 0; }
        let mut step = 1usize;
        while step <= count / 2 { step *= 2; }
        let mut index = 0;
        let mut sum = 0.0;
        while step != 0 {
            let next = index + step;
            if next <= count && sum + self.tree[next] <= offset {
                index = next; sum += self.tree[next];
            }
            step /= 2;
        }
        index.min(count)
    }
}

/// A virtualized collection owns measured heights and scroll anchoring, not the
/// application's row data. Only visible rows plus overscan become elements.
#[derive(Clone, Debug, Default)]
pub struct VirtualList {
    items: Vec<Item>,
    indices: HashMap<String, usize>,
    heights: Heights,
    scroll: ScrollState,
    overscan: f32,
}

impl VirtualList {
    pub fn new(overscan: f32) -> Self {
        Self { overscan: if overscan.is_finite() { overscan.max(0.0) } else { 0.0 }, ..Self::default() }
    }
    pub fn len(&self) -> usize { self.items.len() }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
    pub fn scroll(&self) -> &ScrollState { &self.scroll }
    pub fn scroll_mut(&mut self) -> &mut ScrollState { &mut self.scroll }
    pub fn key(&self, index: usize) -> Option<&str> { self.items.get(index).map(|item| item.key.as_str()) }
    pub fn index(&self, key: &str) -> Option<usize> { self.indices.get(key).copied() }
    pub fn total_height(&self) -> f32 { self.heights.prefix(self.items.len()) as f32 }
    pub fn item_offset(&self, index: usize) -> f32 { self.heights.prefix(index) as f32 }
    pub fn item_height(&self, index: usize) -> Option<f32> { self.items.get(index).map(|item| item.height as f32) }

    /// Keyed replacement retains existing measurements and the top visible row's
    /// visual anchor across inserts, removals, and reordering.
    pub fn set_items(&mut self, keys: impl IntoIterator<Item = String>, estimate: f32) -> Result<(), VirtualListError> {
        if !estimate.is_finite() || estimate <= 0.0 { return Err(VirtualListError::InvalidHeight); }
        let anchor_index = self.heights.item_at(self.scroll.offset()[1] as f64);
        let anchor = self.items.get(anchor_index).map(|item| (item.key.clone(), self.scroll.offset()[1] - self.item_offset(anchor_index)));
        let mut seen = HashSet::new();
        let mut next = Vec::new();
        for key in keys {
            if !seen.insert(key.clone()) { return Err(VirtualListError::DuplicateKey(key)); }
            if next.len() >= 1_000_000 { return Err(VirtualListError::TooManyItems); }
            let height = self.indices.get(&key).map_or(estimate as f64, |&index| self.items[index].height);
            next.push(Item { key, height });
        }
        let heights = Heights::build(&next);
        if heights.prefix(next.len()) > f32::MAX as f64 { return Err(VirtualListError::InvalidHeight); }
        self.indices = next.iter().enumerate().map(|(index, item)| (item.key.clone(), index)).collect();
        self.items = next; self.heights = heights;
        self.update_metrics();
        if let Some((key, within)) = anchor {
            if let Some(&index) = self.indices.get(&key) {
                self.scroll.scroll_to([self.scroll.offset()[0], self.item_offset(index) + within]);
            }
        }
        Ok(())
    }

    fn update_metrics(&mut self) {
        let viewport = self.scroll.viewport();
        self.scroll.set_metrics(viewport, [viewport[0], self.total_height()]);
    }
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.scroll.set_metrics([width, height], [width, self.total_height()]);
    }
    pub fn set_measured_height(&mut self, index: usize, height: f32) -> Result<bool, VirtualListError> {
        if !height.is_finite() || height <= 0.0 { return Err(VirtualListError::InvalidHeight); }
        let Some(item) = self.items.get(index) else { return Ok(false); };
        let delta = height as f64 - item.height;
        if delta.abs() < 0.001 { return Ok(false); }
        let total = self.heights.prefix(self.items.len()) + delta;
        if total > f32::MAX as f64 { return Err(VirtualListError::InvalidHeight); }
        let anchor = self.heights.item_at(self.scroll.offset()[1] as f64);
        self.items[index].height = height as f64; self.heights.add(index, delta);
        let target = self.scroll.offset()[1] + if index < anchor { delta as f32 } else { 0.0 };
        self.update_metrics(); self.scroll.scroll_to([self.scroll.offset()[0], target]);
        Ok(true)
    }
    pub fn visible_range(&self) -> Range<usize> {
        if self.items.is_empty() || self.scroll.viewport()[1] == 0.0 { return 0..0; }
        let start = (self.scroll.offset()[1] - self.overscan).max(0.0) as f64;
        let end = (self.scroll.offset()[1] + self.scroll.viewport()[1] + self.overscan) as f64;
        self.heights.item_at(start)..(self.heights.item_at(end) + 1).min(self.items.len())
    }
    pub fn reveal(&mut self, index: usize) -> bool {
        let Some(height) = self.item_height(index) else { return false; };
        self.scroll.reveal(Rect::new(0.0, self.item_offset(index), self.scroll.viewport()[0], height), 0.0)
    }
    pub fn element<Action>(&self, key: &str, mut render: impl FnMut(usize, &str) -> Element<Action>) -> Element<Action> {
        let range = self.visible_range();
        let mut children = Vec::with_capacity(range.len() + 2);
        children.push(Element::spacer(format!("{key}/before")).height(Size::Px(self.item_offset(range.start))));
        for index in range.clone() { children.push(render(index, &self.items[index].key)); }
        children.push(Element::spacer(format!("{key}/after"))
            .height(Size::Px((self.total_height() - self.item_offset(range.end)).max(0.0))));
        Element::column(key, children)
    }
    /// Call after layout, using the same row keys passed to element(). The
    /// returned flag asks the host to present again when estimates changed.
    pub fn observe<Action>(&mut self, frame: &Frame<Action>) -> Result<bool, VirtualListError> {
        let range = self.visible_range();
        let mut changed = false;
        for index in range {
            if let Some(bounds) = frame.bounds_for(&self.items[index].key) {
                changed |= self.set_measured_height(index, bounds.height)?;
            }
        }
        Ok(changed)
    }
}
