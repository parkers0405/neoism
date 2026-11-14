//! Retained no-second-tree layout store (`LayoutStore`) driving Taffy 0.15's solver directly.
//!
//! Instead of maintaining a general-purpose `TaffyTree` (`SlotMap`s / `SparseSet`, parent links,
//! per-node child `Vec`s, dual 92-byte `Layout`s, and `Option<NodeContext>`), `LayoutStore` holds:
//! - Dense 216-byte `RetainedSlot`s indexed directly by `SlotId(u32)` (`NodeId`).
//! - Double-buffered flat `Vec<u32>` child arenas (`prev_children` / `curr_children`) plus a
//!   reusable DFS `child_scratch` stack so `flatten` allocates zero child `Vec`s.
//! - Single-pass post-order dirty propagation during `flatten`, with an `O(1)` warm unmount check
//!   when the mounted key count is unchanged.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use taffy::DetailedGridInfo;
use taffy::compute::compute_root_layout;
use taffy::geometry::Size;
use taffy::style::AvailableSpace;
use taffy::tree::{ClearState, DetailedLayoutInfo, Layout, LayoutInput, LayoutOutput, NodeId};

use crate::cache::CompactCache;
use crate::style::CompactStyle;
use crate::view::LayoutStoreView;

/// Memory breakdown for a layout store instance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageStats {
    pub live_nodes: usize,
    pub slot_capacity: usize,
    pub sparse_capacity: usize,
    pub bytes_per_slot: usize,
    pub inline_buffer_bytes: usize,
    pub child_heap_bytes: usize,
    pub cold_style_bytes: usize,
    pub cold_cache_bytes: usize,
    pub cold_aux_bytes: usize,
    pub total_retained_bytes: usize,
}

const UNMOUNTED: u32 = u32::MAX;

#[derive(Default)]
struct FxU64(u64);

impl Hasher for FxU64 {
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline(always)]
    fn write(&mut self, bytes: &[u8]) {
        let mut h = self.0;
        for &b in bytes {
            h = (h.rotate_left(5) ^ (b as u64)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
        self.0 = h;
    }

    #[inline(always)]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

type KeyMap<V> = HashMap<u64, V, BuildHasherDefault<FxU64>>;

/// Direct index into [`LayoutStore`]'s dense slot array.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SlotId(pub u32);

impl SlotId {
    #[inline(always)]
    pub const fn to_taffy(self) -> NodeId {
        NodeId::new(self.0 as u64)
    }

    #[inline(always)]
    pub fn from_taffy(id: NodeId) -> Self {
        Self(u64::from(id) as u32)
    }
}

/// Unrounded box geometry and combined `padding + border` insets (32 bytes vs 92 bytes for `taffy::Layout`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BoxLayout {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// `[left, top, right, bottom]` = `padding + border`.
    pub inset: [f32; 4],
}

const _: () = assert!(core::mem::size_of::<BoxLayout>() == 32);

impl BoxLayout {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
        inset: [0.0; 4],
    };

    #[inline(always)]
    pub fn from_taffy(l: &Layout) -> Self {
        Self {
            x: l.location.x,
            y: l.location.y,
            w: l.size.width,
            h: l.size.height,
            inset: [
                l.padding.left + l.border.left,
                l.padding.top + l.border.top,
                l.padding.right + l.border.right,
                l.padding.bottom + l.border.bottom,
            ],
        }
    }

    #[inline(always)]
    pub const fn size(&self) -> Size<f32> {
        Size {
            width: self.w,
            height: self.h,
        }
    }
}

const HAS_CTX_BIT: u32 = 1 << 31;
const MUTATED_BIT: u32 = 1 << 30;
const CHILD_LEN_MASK: u32 = !(HAS_CTX_BIT | MUTATED_BIT);

#[derive(Debug, Clone)]
pub(crate) struct RetainedSlot {
    pub(crate) style: CompactStyle,
    pub(crate) cache: CompactCache,
    pub(crate) box_layout: BoxLayout,
    pub(crate) measure: u64,
    pub(crate) grid_info: Option<Box<DetailedLayoutInfo>>,
    pub(crate) child_start: u32,
    pub(crate) child_len_and_flags: u32,
}

const _: () = assert!(core::mem::size_of::<RetainedSlot>() <= 216);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<RetainedSlot>() == 216);

impl RetainedSlot {
    #[inline(always)]
    fn empty() -> Self {
        Self {
            style: CompactStyle::default(),
            cache: CompactCache::new(),
            box_layout: BoxLayout::ZERO,
            measure: 0,
            grid_info: None,
            child_start: 0,
            child_len_and_flags: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn child_len(&self) -> u32 {
        self.child_len_and_flags & CHILD_LEN_MASK
    }

    #[inline(always)]
    pub(crate) fn has_context(&self) -> bool {
        (self.child_len_and_flags & HAS_CTX_BIT) != 0
    }

    #[inline(always)]
    fn has_mutated(&self) -> bool {
        (self.child_len_and_flags & MUTATED_BIT) != 0
    }
}

/// Contiguous slice iterator over a node's child [`SlotId`]s for the current commit.
#[derive(Debug, Clone)]
pub struct SlotChildIter<'a>(pub(crate) core::slice::Iter<'a, u32>);

impl Iterator for SlotChildIter<'_> {
    type Item = NodeId;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|&s| NodeId::new(s as u64))
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for SlotChildIter<'_> {
    #[inline(always)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Retained layout store integrated directly with Rune's commit pass.
#[derive(Debug, Clone, Default)]
pub struct LayoutStore {
    pub(crate) slots: Vec<RetainedSlot>,
    pub(crate) node_idx: Vec<u32>,
    by_key: KeyMap<u32>,
    free: Vec<u32>,
    prev_children: Vec<u32>,
    pub(crate) curr_children: Vec<u32>,
    child_scratch: Vec<u32>,
    mutated_slots: Vec<u32>,
    prev_live: usize,
    visited_existing: usize,
}

impl LayoutStore {
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(32)
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            node_idx: Vec::with_capacity(capacity),
            by_key: KeyMap::with_capacity_and_hasher(capacity, Default::default()),
            free: Vec::new(),
            prev_children: Vec::with_capacity(capacity),
            curr_children: Vec::with_capacity(capacity),
            child_scratch: Vec::with_capacity(64),
            mutated_slots: Vec::new(),
            prev_live: 0,
            visited_existing: 0,
        }
    }

    #[inline]
    pub fn live_count(&self) -> usize {
        self.by_key.len()
    }

    /// Starts a new flatten pass, swapping the flat child arena without deallocating.
    #[inline]
    pub fn begin_commit(&mut self) {
        core::mem::swap(&mut self.prev_children, &mut self.curr_children);
        self.curr_children.clear();
        self.child_scratch.clear();
        self.node_idx.fill(UNMOUNTED);
        self.prev_live = self.by_key.len();
        self.visited_existing = 0;
    }

    #[inline(always)]
    pub fn begin_children(&self) -> usize {
        self.child_scratch.len()
    }

    #[inline(always)]
    pub fn push_child(&mut self, child: SlotId) {
        self.child_scratch.push(child.0);
    }

    /// Whether `key` still owns `slot` and has not been claimed or mutated earlier in this commit.
    #[inline]
    pub fn can_reuse(&self, key: u64, slot: SlotId) -> bool {
        let idx = slot.0 as usize;
        self.by_key.get(&key) == Some(&slot.0)
            && self.node_idx.get(idx).copied() == Some(UNMOUNTED)
            && self.slots.get(idx).is_some_and(|s| !s.has_mutated())
    }

    /// Whether `key` has already been claimed earlier in this commit.
    #[inline]
    pub fn is_used_this_commit(&self, key: u64) -> bool {
        self.by_key
            .get(&key)
            .is_some_and(|&s| self.node_idx.get(s as usize).copied() != Some(UNMOUNTED))
    }

    /// Reuses an unchanged slot (`memo_reuse`) by copying its previous child slice into
    /// `curr_children` and updating its `node_idx` without re-checking style or cache.
    #[inline]
    pub fn reuse_slot(&mut self, slot: SlotId, node_idx: u32) {
        let idx = slot.0 as usize;
        debug_assert!(idx < self.slots.len());
        if self.node_idx[idx] == UNMOUNTED {
            self.visited_existing += 1;
        }
        self.node_idx[idx] = node_idx;
        let old_start = self.slots[idx].child_start as usize;
        let len = self.slots[idx].child_len() as usize;
        let new_start = self.curr_children.len() as u32;
        if len > 0 {
            self.curr_children
                .extend_from_slice(&self.prev_children[old_start..old_start + len]);
        }
        self.slots[idx].child_start = new_start;
    }

    /// Rebases a slice of `(old_key, new_key, slot, node_idx)` entries when a memoized subtree
    /// moves to a new parent path, preserving each slot's warm cache and children.
    pub fn rekey_and_reuse_batch(
        &mut self,
        items: impl ExactSizeIterator<Item = (u64, u64, SlotId, u32)> + Clone,
    ) {
        for (old_key, _, slot, _) in items.clone() {
            let removed = self.by_key.remove(&old_key);
            debug_assert_eq!(removed, Some(slot.0));
        }
        for (_, new_key, slot, node_idx) in items {
            if let Some(stale) = self.by_key.remove(&new_key) {
                self.slots[stale as usize] = RetainedSlot::empty();
                self.free.push(stale);
                self.prev_live = self.prev_live.saturating_sub(1);
            }
            self.by_key.insert(new_key, slot.0);
            self.reuse_slot(slot, node_idx);
        }
    }

    /// Reconciles one node in post-order after its children have pushed onto `child_scratch[child_mark..]`.
    /// Returns `(slot_id, subtree_dirty)` and clears this slot's cache immediately if dirty.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn sync_node(
        &mut self,
        key: u64,
        node_idx: u32,
        style: CompactStyle,
        measure: Option<u64>,
        force_measure_dirty: bool,
        child_mark: usize,
        children_dirty: bool,
    ) -> (SlotId, bool) {
        let new_kids = &self.child_scratch[child_mark..];
        let child_start = self.curr_children.len() as u32;
        let child_len = new_kids.len() as u32;
        self.curr_children.extend_from_slice(new_kids);

        if let Some(&s) = self.by_key.get(&key) {
            let idx = s as usize;
            debug_assert!(idx < self.slots.len());
            // SAFETY: every slot value stored in `by_key` is a valid index into `node_idx` and `slots`.
            let prev_idx = unsafe { self.node_idx.get_unchecked_mut(idx) };
            let first_visit = *prev_idx == UNMOUNTED;
            if first_visit {
                self.visited_existing += 1;
            }
            *prev_idx = node_idx;

            // SAFETY: `idx < self.slots.len()` holds for all entries in `by_key`.
            let slot = unsafe { self.slots.get_unchecked_mut(idx) };
            let old_start = slot.child_start as usize;
            let old_end = old_start + slot.child_len() as usize;
            let kids_changed = if first_visit {
                &self.prev_children[old_start..old_end] != new_kids
            } else {
                self.curr_children[old_start..old_end] != self.curr_children[child_start as usize..]
            };

            let style_changed = slot.style != style;
            if style_changed {
                slot.style = style;
            }
            slot.child_start = child_start;

            let had_measure = slot.has_context();
            let has_measure = measure.is_some();
            let new_m = measure.unwrap_or(0);
            let measure_changed =
                had_measure != has_measure || (has_measure && (force_measure_dirty || slot.measure != new_m));
            let was_mutated = slot.has_mutated();
            let now_mutated = was_mutated || style_changed || kids_changed || measure_changed;
            slot.child_len_and_flags = child_len
                | if has_measure { HAS_CTX_BIT } else { 0 }
                | if now_mutated { MUTATED_BIT } else { 0 };
            slot.measure = new_m;

            let dirty = style_changed || kids_changed || measure_changed || children_dirty;
            if dirty {
                slot.cache.clear();
            }
            if now_mutated && !was_mutated {
                self.mutated_slots.push(s);
            }
            self.child_scratch.truncate(child_mark);
            (SlotId(s), dirty)
        } else {
            self.child_scratch.truncate(child_mark);
            let new_slot = RetainedSlot {
                style,
                cache: CompactCache::new(),
                box_layout: BoxLayout::ZERO,
                measure: measure.unwrap_or(0),
                grid_info: None,
                child_start,
                child_len_and_flags: child_len
                    | if measure.is_some() { HAS_CTX_BIT } else { 0 }
                    | MUTATED_BIT,
            };
            let s = if let Some(free_s) = self.free.pop() {
                let idx = free_s as usize;
                self.slots[idx] = new_slot;
                self.node_idx[idx] = node_idx;
                free_s
            } else {
                let s = self.slots.len() as u32;
                self.slots.push(new_slot);
                self.node_idx.push(node_idx);
                s
            };
            self.mutated_slots.push(s);
            self.by_key.insert(key, s);
            (SlotId(s), true)
        }
    }

    /// Frees any slots not visited during `flatten` without renumbering kept `SlotId`s, so
    /// measurement retries still see stable `SlotId`s on `old_nodes`.
    #[inline]
    pub fn finish_flatten(&mut self) {
        if self.visited_existing == self.prev_live {
            return;
        }
        let Self {
            slots,
            node_idx,
            by_key,
            free,
            ..
        } = self;
        by_key.retain(|_, &mut s| {
            let idx = s as usize;
            if node_idx[idx] == UNMOUNTED {
                slots[idx] = RetainedSlot::empty();
                free.push(s);
                false
            } else {
                true
            }
        });
    }

    /// Shrinks storage when live occupancy drops below 25% of capacity.
    #[inline]
    pub fn compact_if_sparse(&mut self, remap: impl FnMut(u32, SlotId)) {
        let mut dummy = SlotId(UNMOUNTED);
        self.compact_with_root(&mut dummy, remap);
    }

    fn compact_with_root(&mut self, root: &mut SlotId, mut remap: impl FnMut(u32, SlotId)) {
        for idx in self.mutated_slots.drain(..) {
            if let Some(s) = self.slots.get_mut(idx as usize) {
                s.child_len_and_flags &= !MUTATED_BIT;
            }
        }
        let live = self.by_key.len();
        if self.slots.capacity() <= 64 || live.saturating_mul(4) > self.slots.capacity() {
            return;
        }
        let Self {
            slots,
            node_idx,
            by_key,
            free,
            curr_children,
            prev_children,
            child_scratch,
            ..
        } = self;
        let cap = (live * 2).max(32);
        let mut remap_table = vec![UNMOUNTED; slots.len()];
        let mut new_slots = Vec::with_capacity(cap);
        let mut new_node_idx = Vec::with_capacity(cap);
        for s_mut in by_key.values_mut() {
            let old_s = *s_mut as usize;
            let new_s = new_slots.len() as u32;
            remap_table[old_s] = new_s;
            let mut slot = core::mem::replace(&mut slots[old_s], RetainedSlot::empty());
            slot.cache.clear();
            let me = node_idx[old_s];
            remap(me, SlotId(new_s));
            *s_mut = new_s;
            new_slots.push(slot);
            new_node_idx.push(me);
        }
        if let Some(&new_root) = remap_table.get(root.0 as usize)
            && new_root != UNMOUNTED
        {
            root.0 = new_root;
        }
        for c in curr_children.iter_mut() {
            *c = remap_table[*c as usize];
        }
        *slots = new_slots;
        *node_idx = new_node_idx;
        *free = Vec::new();
        by_key.shrink_to(cap);
        curr_children.shrink_to(cap);
        *prev_children = Vec::with_capacity(cap);
        child_scratch.shrink_to(64);
    }

    /// Finishes a flatten pass, recycling unvisited slots and compacting when sparse.
    #[inline]
    pub fn end_commit(&mut self, root: &mut SlotId, remap: impl FnMut(u32, SlotId)) {
        if self.visited_existing != self.prev_live {
            self.finish_flatten();
        }
        self.compact_with_root(root, remap);
    }

    #[inline(always)]
    pub fn style(&self, id: SlotId) -> &CompactStyle {
        debug_assert!((id.0 as usize) < self.slots.len());
        // SAFETY: `id` is a live `SlotId` index into `slots`.
        unsafe { &self.slots.get_unchecked(id.0 as usize).style }
    }

    #[inline(always)]
    pub fn box_layout(&self, id: SlotId) -> BoxLayout {
        debug_assert!((id.0 as usize) < self.slots.len());
        // SAFETY: `id` is a live `SlotId` index into `slots`.
        unsafe { self.slots.get_unchecked(id.0 as usize).box_layout }
    }

    /// Returns the index in the current commit's `nodes` slice for `key`, if mounted this commit.
    #[inline]
    pub fn mounted_node_index(&self, key: u64) -> Option<usize> {
        let &s = self.by_key.get(&key)?;
        let idx = *self.node_idx.get(s as usize)?;
        (idx != UNMOUNTED).then_some(idx as usize)
    }

    pub fn measure_hash(&self, key: u64) -> Option<Option<u64>> {
        let &s = self.by_key.get(&key)?;
        let slot = &self.slots[s as usize];
        Some(slot.has_context().then_some(slot.measure))
    }

    /// Updates `id`'s style (for intra-commit two-pass refits like `aspect_fit` and `anchored_fit`)
    /// and clears `id`'s and its ancestors' layout caches.
    #[inline]
    pub fn set_style_and_mark_ancestors(
        &mut self,
        id: SlotId,
        style: CompactStyle,
        ancestors: impl IntoIterator<Item = SlotId>,
    ) {
        let slot = &mut self.slots[id.0 as usize];
        if !slot.has_mutated() {
            slot.child_len_and_flags |= MUTATED_BIT;
            self.mutated_slots.push(id.0);
        }
        slot.style = style;
        slot.cache.clear();
        for anc in ancestors {
            if matches!(self.slots[anc.0 as usize].cache.clear(), ClearState::AlreadyEmpty) {
                break;
            }
        }
    }

    /// Clears `id`'s and its ancestors' layout caches without modifying style.
    #[inline]
    pub fn mark_dirty_and_ancestors(&mut self, id: SlotId, ancestors: impl IntoIterator<Item = SlotId>) {
        self.slots[id.0 as usize].cache.clear();
        for anc in ancestors {
            if matches!(self.slots[anc.0 as usize].cache.clear(), ClearState::AlreadyEmpty) {
                break;
            }
        }
    }

    /// Clears every live slot's layout cache (e.g. when switching font families).
    pub fn mark_all_dirty(&mut self) {
        for &s in self.by_key.values() {
            self.slots[s as usize].cache.clear();
        }
    }

    pub fn compute_layout<F>(&mut self, root: SlotId, available_space: Size<AvailableSpace>, measure_leaf: F)
    where
        F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
    {
        let mut view = LayoutStoreView {
            store: self,
            measure_leaf,
        };
        compute_root_layout(&mut view, root.to_taffy(), available_space);
    }

    pub fn storage_stats(&self) -> StorageStats {
        let slot_capacity = self.slots.capacity();
        let bytes_per_slot = core::mem::size_of::<RetainedSlot>() + core::mem::size_of::<u32>();
        let inline_buffer_bytes = slot_capacity * bytes_per_slot
            + (self.prev_children.capacity() + self.curr_children.capacity() + self.child_scratch.capacity())
                * core::mem::size_of::<u32>()
            + self.free.capacity() * core::mem::size_of::<u32>();
        let mut cold_style_bytes = 0usize;
        let mut cold_cache_bytes = 0usize;
        let mut cold_aux_bytes = 0usize;
        for &s in self.by_key.values() {
            let slot = &self.slots[s as usize];
            cold_style_bytes += slot.style.heap_bytes();
            cold_cache_bytes += slot.cache.heap_bytes();
            if slot.grid_info.is_some() {
                cold_aux_bytes += core::mem::size_of::<DetailedLayoutInfo>() + core::mem::size_of::<DetailedGridInfo>();
            }
        }
        StorageStats {
            live_nodes: self.by_key.len(),
            slot_capacity,
            sparse_capacity: self.by_key.capacity(),
            bytes_per_slot,
            inline_buffer_bytes,
            child_heap_bytes: 0,
            cold_style_bytes,
            cold_cache_bytes,
            cold_aux_bytes,
            total_retained_bytes: inline_buffer_bytes + cold_style_bytes + cold_cache_bytes + cold_aux_bytes,
        }
    }
}
