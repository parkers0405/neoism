use super::protocol::VirtualBounds;

/// Inclusive/exclusive axis range used by the height index.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisRange {
    pub start: usize,
    pub end: usize,
}

impl AxisRange {
    #[inline]
    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

/// Prefix-sum height index backed by a Fenwick tree.
///
/// The index answers "which node is at scroll y?" in O(log n), then callers
/// walk only the visible band. This is the generalized version of the editor
/// grid's source-row offset, but for variable-height nodes.
#[derive(Clone, Debug, Default)]
pub(crate) struct HeightIndex {
    heights: Vec<f32>,
    tree: Vec<f32>,
}

impl HeightIndex {
    pub fn from_heights(heights: Vec<f32>) -> Self {
        let mut index = Self {
            heights,
            tree: Vec::new(),
        };
        index.rebuild_tree();
        index
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    pub fn total_height(&self) -> f32 {
        self.prefix_sum(self.heights.len())
    }

    pub fn height(&self, index: usize) -> f32 {
        self.heights.get(index).copied().unwrap_or(0.0)
    }

    pub fn set_height(&mut self, index: usize, height: f32) {
        if index >= self.heights.len() {
            return;
        }
        let height = height.max(0.0);
        let delta = height - self.heights[index];
        if delta.abs() <= f32::EPSILON {
            return;
        }
        self.heights[index] = height;
        self.add(index, delta);
    }

    pub fn splice(&mut self, start: usize, delete: usize, insert: Vec<f32>) {
        let start = start.min(self.heights.len());
        let end = start.saturating_add(delete).min(self.heights.len());
        self.heights
            .splice(start..end, insert.into_iter().map(|height| height.max(0.0)));
        self.rebuild_tree();
    }

    pub fn prefix_sum(&self, count: usize) -> f32 {
        let mut i = count.min(self.heights.len());
        let mut sum = 0.0;
        while i > 0 {
            sum += self.tree[i];
            i &= i - 1;
        }
        sum
    }

    pub fn bounds_for(&self, index: usize, width: f32) -> VirtualBounds {
        let y = self.prefix_sum(index);
        VirtualBounds::new(0.0, y, width, self.height(index))
    }

    /// Return the node index containing `y`, or `len` when `y` is beyond the
    /// content. Uses Fenwick lower-bound on prefix sums.
    pub fn lower_bound_y(&self, y: f32) -> usize {
        if self.heights.is_empty() {
            return 0;
        }
        let target = y.max(0.0);
        if target <= 0.0 {
            return 0;
        }
        if target >= self.total_height() {
            return self.heights.len();
        }

        let mut idx = 0usize;
        let mut bit = highest_power_of_two_at_least(self.tree.len());
        let mut sum = 0.0;
        while bit != 0 {
            let next = idx + bit;
            if next < self.tree.len() && sum + self.tree[next] <= target {
                idx = next;
                sum += self.tree[next];
            }
            bit >>= 1;
        }
        idx.min(self.heights.len())
    }

    pub fn visible_range(&self, top: f32, bottom: f32) -> AxisRange {
        if self.is_empty() || bottom < top {
            return AxisRange::default();
        }
        let start = self.lower_bound_y(top);
        let mut end = start;
        let mut y = self.prefix_sum(start);
        let len = self.len();
        while end < len {
            if y > bottom {
                break;
            }
            y += self.height(end);
            end += 1;
        }
        AxisRange { start, end }
    }

    fn rebuild_tree(&mut self) {
        self.tree.clear();
        self.tree.resize(self.heights.len() + 1, 0.0);
        for i in 0..self.heights.len() {
            self.add(i, self.heights[i].max(0.0));
        }
    }

    fn add(&mut self, index: usize, delta: f32) {
        let mut i = index + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += i & (!i + 1);
        }
    }
}

fn highest_power_of_two_at_least(value: usize) -> usize {
    if value <= 1 {
        return 1;
    }
    let mut bit = 1usize;
    while bit < value {
        bit <<= 1;
    }
    bit
}
