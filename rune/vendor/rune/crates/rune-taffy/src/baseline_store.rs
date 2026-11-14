//! Upstream `taffy::TaffyTree` wrapper implementing the [`crate::LayoutStore`] commit interface
//! for `--features baseline-taffy` parity checks and A/B benchmarks.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use taffy::geometry::Size;
use taffy::style::{AvailableSpace, Style};
use taffy::tree::{DetailedLayoutInfo, Layout, LayoutInput, LayoutOutput, NodeId, TraversePartialTree};

use crate::BaselineTaffyTree;
use crate::store::{BoxLayout, SlotId, StorageStats};
use crate::style::CompactStyle;

/// Estimates retained heap bytes for a baseline `taffy::TaffyTree<NodeContext>` with `live_nodes`
/// and `peak_nodes`, accounting for `SlotMap`'s power-of-two slot buffers (`nodes`, `children`,
/// `parents`, `node_context_data`) and per-container `Vec<NodeId>` allocations.
pub fn baseline_storage_stats<NodeContext>(
    tree: &taffy::TaffyTree<NodeContext>,
    root: Option<NodeId>,
    peak_nodes: usize,
) -> StorageStats {
    let live_nodes = tree.total_node_count();
    let slot_cap = peak_nodes.max(live_nodes).max(16).next_power_of_two();
    let node_data_bytes = core::mem::size_of::<Style>()
        + core::mem::size_of::<taffy::Cache>()
        + 2 * core::mem::size_of::<Layout>()
        + core::mem::size_of::<Vec<NodeId>>()
        + core::mem::size_of::<DetailedLayoutInfo>()
        + 16;
    let children_slot_bytes = core::mem::size_of::<Vec<NodeId>>() + 8;
    let parents_slot_bytes = core::mem::size_of::<Option<NodeId>>() + 8;
    let context_slot_bytes = core::mem::size_of::<NodeContext>() + 16;
    let bytes_per_slot = node_data_bytes + children_slot_bytes + parents_slot_bytes + context_slot_bytes;
    let inline_buffer_bytes = slot_cap * bytes_per_slot;

    let mut child_heap_bytes = 0usize;
    if let Some(root_id) = root {
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            let count = tree.child_count(id);
            if count > 0 {
                let cap = count.max(4).next_power_of_two();
                child_heap_bytes += cap * core::mem::size_of::<NodeId>();
                for i in 0..count {
                    stack.push(tree.get_child_id(id, i));
                }
            }
        }
    }

    StorageStats {
        live_nodes,
        slot_capacity: slot_cap,
        sparse_capacity: slot_cap,
        bytes_per_slot,
        inline_buffer_bytes,
        child_heap_bytes,
        cold_style_bytes: 0,
        cold_cache_bytes: 0,
        cold_aux_bytes: 0,
        total_retained_bytes: inline_buffer_bytes + child_heap_bytes,
    }
}

#[derive(Debug, Clone)]
struct BaselineSlot {
    taffy_id: NodeId,
    style: CompactStyle,
    measure: Option<u64>,
    node_idx: u32,
    used: bool,
    mutated: bool,
}

/// Drop-in `LayoutStore` equivalent backed by upstream `taffy::TaffyTree<usize>`.
#[derive(Debug, Clone)]
pub struct BaselineLayoutStore {
    tree: BaselineTaffyTree<usize>,
    slots: Vec<Option<BaselineSlot>>,
    by_key: HashMap<u64, u32>,
    free: Vec<u32>,
    child_scratch: Vec<u32>,
    last_root: Option<NodeId>,
    peak_nodes: usize,
}

impl Default for BaselineLayoutStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BaselineLayoutStore {
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(32)
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let mut tree = BaselineTaffyTree::with_capacity(capacity);
        tree.disable_rounding();
        Self {
            tree,
            slots: Vec::with_capacity(capacity),
            by_key: HashMap::with_capacity(capacity),
            free: Vec::new(),
            child_scratch: Vec::with_capacity(64),
            last_root: None,
            peak_nodes: 0,
        }
    }

    #[inline]
    pub fn live_count(&self) -> usize {
        self.by_key.len()
    }

    pub fn begin_commit(&mut self) {
        self.child_scratch.clear();
        for s in self.slots.iter_mut().flatten() {
            s.used = false;
        }
    }

    #[inline(always)]
    pub fn begin_children(&self) -> usize {
        self.child_scratch.len()
    }

    #[inline(always)]
    pub fn push_child(&mut self, child: SlotId) {
        self.child_scratch.push(child.0);
    }

    #[inline]
    pub fn can_reuse(&self, key: u64, slot: SlotId) -> bool {
        self.by_key.get(&key) == Some(&slot.0)
            && self
                .slots
                .get(slot.0 as usize)
                .and_then(Option::as_ref)
                .is_some_and(|e| !e.used && !e.mutated)
    }

    #[inline]
    pub fn is_used_this_commit(&self, key: u64) -> bool {
        self.by_key.get(&key).is_some_and(|&s| {
            self.slots
                .get(s as usize)
                .and_then(Option::as_ref)
                .is_some_and(|e| e.used)
        })
    }

    #[inline]
    pub fn reuse_slot(&mut self, slot: SlotId, node_idx: u32) {
        let entry = self.slots[slot.0 as usize].as_mut().unwrap();
        entry.used = true;
        entry.node_idx = node_idx;
        if entry.measure.is_some() {
            *self.tree.get_node_context_mut(entry.taffy_id).unwrap() = node_idx as usize;
        }
    }

    pub fn rekey_and_reuse_batch(
        &mut self,
        items: impl ExactSizeIterator<Item = (u64, u64, SlotId, u32)> + Clone,
    ) {
        let mut first = true;
        for (old_key, _, slot, _) in items.clone() {
            if first {
                first = false;
                let tid = self.slots[slot.0 as usize].as_ref().unwrap().taffy_id;
                if let Some(old_p) = self.tree.parent(tid) {
                    drop(self.tree.remove_child(old_p, tid));
                }
            }
            let removed = self.by_key.remove(&old_key);
            debug_assert_eq!(removed, Some(slot.0));
        }
        for (_, new_key, slot, node_idx) in items {
            if let Some(stale) = self.by_key.remove(&new_key) {
                if let Some(old) = self.slots[stale as usize].take() {
                    drop(self.tree.set_children(old.taffy_id, &[]));
                    self.tree.remove(old.taffy_id).unwrap();
                }
                self.free.push(stale);
            }
            self.by_key.insert(new_key, slot.0);
            self.reuse_slot(slot, node_idx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sync_node(
        &mut self,
        key: u64,
        node_idx: u32,
        style: CompactStyle,
        measure: Option<u64>,
        force_measure_dirty: bool,
        child_mark: usize,
        _children_dirty: bool,
    ) -> (SlotId, bool) {
        let kids: Vec<NodeId> = self.child_scratch[child_mark..]
            .iter()
            .map(|&s| self.slots[s as usize].as_ref().unwrap().taffy_id)
            .collect();
        self.child_scratch.truncate(child_mark);
        let me = node_idx as usize;
        let measured = measure.is_some();

        if let Some(&s) = self.by_key.get(&key) {
            let slot = self.slots[s as usize].as_mut().unwrap();
            let id = slot.taffy_id;
            let mut dirty = false;
            if slot.style != style {
                self.tree.set_style(id, style.to_style()).unwrap();
                slot.style = style;
                dirty = true;
            }
            if !self.tree.child_ids(id).eq(kids.iter().copied()) {
                self.tree.set_children(id, &kids).unwrap();
                dirty = true;
            }
            if slot.measure.is_some() != measured {
                self.tree.set_node_context(id, measured.then_some(me)).unwrap();
                dirty = true;
            } else if measured {
                *self.tree.get_node_context_mut(id).unwrap() = me;
            }
            if force_measure_dirty || slot.measure != measure {
                self.tree.mark_dirty(id).unwrap();
                dirty = true;
            }
            if dirty {
                slot.mutated = true;
            }
            slot.measure = measure;
            slot.node_idx = node_idx;
            slot.used = true;
            (SlotId(s), dirty)
        } else {
            let full = style.to_style();
            let id = if !kids.is_empty() {
                self.tree.new_with_children(full, &kids).unwrap()
            } else if measured {
                self.tree.new_leaf_with_context(full, me).unwrap()
            } else {
                self.tree.new_leaf(full).unwrap()
            };
            if measured && self.tree.get_node_context(id).is_none() {
                self.tree.set_node_context(id, Some(me)).unwrap();
            }
            let entry = BaselineSlot {
                taffy_id: id,
                style,
                measure,
                node_idx,
                used: true,
                mutated: true,
            };
            let s = if let Some(free_s) = self.free.pop() {
                self.slots[free_s as usize] = Some(entry);
                free_s
            } else {
                let s = self.slots.len() as u32;
                self.slots.push(Some(entry));
                s
            };
            self.by_key.insert(key, s);
            (SlotId(s), true)
        }
    }

    pub fn finish_flatten(&mut self) {
        self.peak_nodes = self.peak_nodes.max(self.by_key.len());
        let Self {
            tree,
            slots,
            by_key,
            free,
            ..
        } = self;
        by_key.retain(|_, &mut s| {
            let keep = slots[s as usize].as_ref().is_some_and(|e| e.used);
            if !keep {
                if let Some(old) = slots[s as usize].take() {
                    tree.remove(old.taffy_id).unwrap();
                }
                free.push(s);
            }
            keep
        });
        self.peak_nodes = self.peak_nodes.max(self.by_key.len());
    }

    pub fn compact_if_sparse(&mut self, _remap: impl FnMut(u32, SlotId)) {
        for s in self.slots.iter_mut().flatten() {
            s.mutated = false;
        }
    }

    pub fn end_commit(&mut self, _root: &mut SlotId, _remap: impl FnMut(u32, SlotId)) {
        self.finish_flatten();
        for s in self.slots.iter_mut().flatten() {
            s.mutated = false;
        }
    }

    #[inline]
    pub fn style(&self, id: SlotId) -> &CompactStyle {
        &self.slots[id.0 as usize].as_ref().unwrap().style
    }

    #[inline]
    pub fn box_layout(&self, id: SlotId) -> BoxLayout {
        let tid = self.slots[id.0 as usize].as_ref().unwrap().taffy_id;
        BoxLayout::from_taffy(self.tree.layout(tid).unwrap())
    }

    #[inline]
    pub fn mounted_node_index(&self, key: u64) -> Option<usize> {
        let &s = self.by_key.get(&key)?;
        let slot = self.slots.get(s as usize)?.as_ref()?;
        slot.used.then_some(slot.node_idx as usize)
    }

    pub fn measure_hash(&self, key: u64) -> Option<Option<u64>> {
        let &s = self.by_key.get(&key)?;
        Some(self.slots[s as usize].as_ref()?.measure)
    }

    pub fn set_style_and_mark_ancestors(
        &mut self,
        id: SlotId,
        style: CompactStyle,
        _ancestors: impl IntoIterator<Item = SlotId>,
    ) {
        let slot = self.slots[id.0 as usize].as_mut().unwrap();
        self.tree.set_style(slot.taffy_id, style.to_style()).unwrap();
        slot.style = style;
        slot.mutated = true;
    }

    pub fn mark_dirty_and_ancestors(&mut self, id: SlotId, _ancestors: impl IntoIterator<Item = SlotId>) {
        let tid = self.slots[id.0 as usize].as_ref().unwrap().taffy_id;
        drop(self.tree.mark_dirty(tid));
    }

    pub fn mark_all_dirty(&mut self) {
        for s in self.slots.iter().flatten() {
            if s.measure.is_some() {
                drop(self.tree.mark_dirty(s.taffy_id));
            }
        }
    }

    pub fn compute_layout<F>(&mut self, root: SlotId, available_space: Size<AvailableSpace>, mut measure_leaf: F)
    where
        F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
    {
        let root_id = self.slots[root.0 as usize].as_ref().unwrap().taffy_id;
        self.last_root = Some(root_id);
        self.tree
            .compute_layout_with_measure(root_id, available_space, |inputs, _id, ctx, style| {
                let cs = CompactStyle::from_style(style.clone());
                measure_leaf(inputs, ctx.copied(), &cs)
            })
            .unwrap();
    }

    pub fn storage_stats(&self) -> StorageStats {
        baseline_storage_stats(&self.tree, self.last_root, self.peak_nodes.max(self.by_key.len()))
    }
}
