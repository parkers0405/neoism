//! Low-level Taffy 0.15 solver trait implementations over [`LayoutStore`].

use taffy::compute::{
    compute_block_layout, compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_hidden_layout,
    compute_oof_layout,
};
use taffy::style::{CoreStyle, Display, Style};
use taffy::tree::{
    DetailedLayoutInfo, Layout, LayoutBlockContainer, LayoutContainingBlock, LayoutFlexboxContainer,
    LayoutGridContainer, LayoutInput, LayoutOutput, LayoutPartialTree, NodeId, RunMode, TraversePartialTree,
    TraverseTree,
};
use taffy::{BlockContext, CacheTree, DetailedGridInfo};

use crate::store::{BoxLayout, LayoutStore, RetainedSlot, SlotChildIter};
use crate::style::CompactStyle;

const NONE_DETAIL: DetailedLayoutInfo = DetailedLayoutInfo::None;

pub(crate) struct LayoutStoreView<'s, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    pub(crate) store: &'s mut LayoutStore,
    pub(crate) measure_leaf: F,
}

impl<F> LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    #[inline(always)]
    fn slot(&self, node_id: NodeId) -> &RetainedSlot {
        let idx = u64::from(node_id) as usize;
        debug_assert!(idx < self.store.slots.len());
        // SAFETY: all `NodeId`s in `curr_children` and `root` are allocated indices into `slots`.
        unsafe { self.store.slots.get_unchecked(idx) }
    }

    #[inline(always)]
    fn slot_mut(&mut self, node_id: NodeId) -> &mut RetainedSlot {
        let idx = u64::from(node_id) as usize;
        debug_assert!(idx < self.store.slots.len());
        // SAFETY: all `NodeId`s in `curr_children` and `root` are allocated indices into `slots`.
        unsafe { self.store.slots.get_unchecked_mut(idx) }
    }

    #[inline(always)]
    fn compute_child_layout_inner(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            self.slot_mut(node_id).box_layout = BoxLayout::ZERO;
            return compute_hidden_layout(self, node_id);
        }

        compute_cached_layout(self, node_id, inputs, |view, node_id, inputs| {
            let idx = u64::from(node_id) as usize;
            // SAFETY: `node_id` is a live `SlotId` index into `slots`.
            let (display_mode, has_children) = unsafe {
                let s = view.store.slots.get_unchecked(idx);
                (s.style.display(), s.child_len() != 0)
            };

            let mut output = match (display_mode, has_children) {
                (Display::None, _) => compute_hidden_layout(view, node_id),
                (Display::Block, true) => compute_block_layout(view, node_id, inputs, block_ctx),
                (Display::FlowRoot, true) => compute_block_layout(view, node_id, inputs, None),
                (Display::Flex, true) => compute_flexbox_layout(view, node_id, inputs),
                (Display::Grid, true) => compute_grid_layout(view, node_id, inputs),
                (_, false) => {
                    // SAFETY: `idx` is in bounds of both `slots` and `node_idx`.
                    let (ctx, style) = unsafe {
                        let s = view.store.slots.get_unchecked(idx);
                        let ctx = s
                            .has_context()
                            .then(|| *view.store.node_idx.get_unchecked(idx) as usize);
                        (ctx, &s.style)
                    };
                    (view.measure_leaf)(inputs, ctx, style)
                }
            };

            if inputs.run_mode == RunMode::PerformLayout {
                compute_oof_layout(view, node_id, &mut output);
            }

            output
        })
    }
}

impl<F> TraversePartialTree for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type ChildIter<'a>
        = SlotChildIter<'a>
    where
        Self: 'a;

    #[inline(always)]
    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_> {
        let s = self.slot(parent_node_id);
        let start = s.child_start as usize;
        let end = start + s.child_len() as usize;
        // SAFETY: `child_start..child_start + child_len` was appended to `curr_children` this commit.
        SlotChildIter(unsafe { self.store.curr_children.get_unchecked(start..end) }.iter())
    }

    #[inline(always)]
    fn child_count(&self, parent_node_id: NodeId) -> usize {
        self.slot(parent_node_id).child_len() as usize
    }

    #[inline(always)]
    fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId {
        let s = self.slot(parent_node_id);
        debug_assert!(child_index < s.child_len() as usize);
        // SAFETY: Taffy calls `get_child_id` with `child_index < child_count(parent_node_id)`.
        let child_slot = unsafe { *self.store.curr_children.get_unchecked(s.child_start as usize + child_index) };
        NodeId::new(child_slot as u64)
    }
}

impl<F> TraverseTree for LayoutStoreView<'_, F> where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput
{
}

impl<F> LayoutPartialTree for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type CoreContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    type CustomIdent = <Style as CoreStyle>::CustomIdent;

    #[inline(always)]
    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout) {
        self.slot_mut(node_id).box_layout = BoxLayout::from_taffy(layout);
    }

    #[inline(always)]
    fn resolve_calc_value(&self, _val: *const (), _basis: f32) -> f32 {
        0.0
    }

    #[inline(always)]
    fn compute_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute_child_layout_inner(node_id, inputs, None)
    }
}

impl<F> LayoutContainingBlock for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type OofItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_oof_item_style(&self, node_id: NodeId) -> Self::OofItemStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn clear_hoisted_children(&mut self, _node_id: NodeId) {}

    #[inline(always)]
    fn add_hoisted_children(&mut self, _node_id: NodeId, _hoisted: &[NodeId]) {}

    #[inline(always)]
    fn get_detailed_layout_info(&self, node_id: NodeId) -> &DetailedLayoutInfo {
        self.slot(node_id).grid_info.as_deref().unwrap_or(&NONE_DETAIL)
    }
}

impl<F> CacheTree for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    #[inline(always)]
    fn cache_get(&mut self, node_id: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.slot_mut(node_id).cache.get(input)
    }

    #[inline(always)]
    fn cache_store(&mut self, node_id: NodeId, input: &LayoutInput, layout_output: LayoutOutput) {
        self.slot_mut(node_id).cache.store(input, layout_output);
    }

    #[inline(always)]
    fn cache_clear(&mut self, node_id: NodeId) {
        self.slot_mut(node_id).cache.clear();
    }
}

impl<F> LayoutBlockContainer for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type BlockContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;
    type BlockItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_block_container_style(&self, node_id: NodeId) -> Self::BlockContainerStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn get_block_child_style(&self, child_node_id: NodeId) -> Self::BlockItemStyle<'_> {
        &self.slot(child_node_id).style
    }

    #[inline(always)]
    fn compute_block_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        self.compute_child_layout_inner(node_id, inputs, block_ctx)
    }
}

impl<F> LayoutFlexboxContainer for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type FlexboxContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_> {
        &self.slot(child_node_id).style
    }
}

impl<F> LayoutGridContainer for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type GridContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;
    type GridItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_grid_container_style(&self, node_id: NodeId) -> Self::GridContainerStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_> {
        &self.slot(child_node_id).style
    }

    #[inline(always)]
    fn set_detailed_grid_info(&mut self, node_id: NodeId, detailed_grid_info: DetailedGridInfo) {
        self.slot_mut(node_id).grid_info = Some(Box::new(DetailedLayoutInfo::Grid(Box::new(detailed_grid_info))));
    }
}
