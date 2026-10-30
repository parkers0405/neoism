use std::collections::{BTreeSet, HashMap, HashSet};

use super::cache::{CacheTier, GpuResidencyPolicy, RetainedChunk, RetainedChunkCache};
use super::index::{AxisRange, HeightIndex};
use super::protocol::{
    DirtyKind, LayoutKey, NodeGeometry, NodeId, NodeSource, NodeSourceRange,
    SurfaceMetrics, SurfaceRevision, VirtualBounds, VirtualFrameAction,
    VirtualFrameNodePlan, VirtualFramePlan, VirtualHit, VirtualHitTest, VirtualLayout,
    VirtualMeasuredLayout, VirtualNode, VirtualNodeKind, VirtualPrefetchHint,
    VirtualRevealAlign, VirtualRevealTarget, VirtualScroll, VirtualScrollAnchor,
    VirtualSourceEdit, VirtualSourceMatch, VirtualSourceQuery, VirtualSurfaceCommand,
    VirtualSurfaceConfig, VirtualSurfaceError, VirtualSurfaceSnapshot,
    VirtualTextOverlay, VirtualTextPlan, VirtualTileId, VirtualTileRange,
    VirtualViewport, VisibleNode, VisibleSet,
};
use super::render::VirtualDrawCommand;
use super::resource::{
    VirtualFrameCommit, VirtualFrameSchedulePolicy, VirtualFrameTransaction,
    VirtualResourceDescriptor, VirtualResourceId, VirtualResourceKind, VirtualResourceOp,
    VirtualScheduledFrameTransaction,
};

/// Standalone retained runtime for massive virtualized content.
///
/// This is deliberately not markdown-specific. It is the shared engine that
/// markdown, agent history, diffs, logs, and future code buffers can adapt to.
#[derive(Clone, Debug)]
pub struct VirtualSurface {
    config: VirtualSurfaceConfig,
    viewport: VirtualViewport,
    scroll: VirtualScroll,
    revision: SurfaceRevision,
    nodes: Vec<VirtualNode>,
    index_by_id: HashMap<NodeId, usize>,
    source_index: HashMap<NodeSource, Vec<SourceIndexEntry>>,
    layouts: Vec<VirtualLayout>,
    height_index: HeightIndex,
    dirty_layout: HashSet<NodeId>,
    dirty_draw: HashSet<NodeId>,
    dirty_gpu: HashSet<NodeId>,
    cache: RetainedChunkCache,
    pending_resource_drops: Vec<VirtualResourceOp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceIndexEntry {
    start: u64,
    end: u64,
    index: usize,
    node: NodeId,
}

impl Default for VirtualSurface {
    fn default() -> Self {
        Self::new(VirtualSurfaceConfig::default())
    }
}

impl VirtualSurface {
    pub fn new(config: VirtualSurfaceConfig) -> Self {
        Self {
            config,
            viewport: VirtualViewport::default(),
            scroll: VirtualScroll::default(),
            revision: SurfaceRevision::default(),
            nodes: Vec::new(),
            index_by_id: HashMap::new(),
            source_index: HashMap::new(),
            layouts: Vec::new(),
            height_index: HeightIndex::default(),
            dirty_layout: HashSet::new(),
            dirty_draw: HashSet::new(),
            dirty_gpu: HashSet::new(),
            cache: RetainedChunkCache::new(GpuResidencyPolicy {
                max_warm_chunks: config.max_retained_chunks,
                ..GpuResidencyPolicy::default()
            }),
            pending_resource_drops: Vec::new(),
        }
    }

    pub fn config(&self) -> VirtualSurfaceConfig {
        self.config
    }

    pub fn viewport(&self) -> VirtualViewport {
        self.viewport
    }

    pub fn scroll(&self) -> VirtualScroll {
        self.scroll
    }

    pub fn revision(&self) -> SurfaceRevision {
        self.revision
    }

    pub fn nodes(&self) -> &[VirtualNode] {
        &self.nodes
    }

    /// Current layout coordinates, resolved lazily from the height index.
    /// Editing an earlier node does not require rewriting every later node.
    pub fn layout_at(&self, index: usize) -> Option<VirtualLayout> {
        self.layouts.get(index).map(|_| self.current_layout(index))
    }

    /// Cached layout metadata. Use `layout_at` for current document coordinates.
    pub fn layouts(&self) -> &[VirtualLayout] {
        &self.layouts
    }

    pub fn content_height(&self) -> f32 {
        self.height_index.total_height()
    }

    pub fn cache_stats(&self) -> super::cache::CacheStats {
        self.cache.stats()
    }

    pub fn cache_policy(&self) -> GpuResidencyPolicy {
        self.cache.policy()
    }

    pub fn set_cache_policy(&mut self, policy: GpuResidencyPolicy) {
        let evicted = self.cache.set_policy(policy);
        self.record_evicted_chunks(evicted);
    }

    pub fn metrics(&self) -> SurfaceMetrics {
        SurfaceMetrics {
            node_count: self.nodes.len(),
            content_height: self.content_height(),
            dirty_layout_count: self.dirty_layout.len(),
            dirty_draw_count: self.dirty_draw.len(),
            dirty_gpu_count: self.dirty_gpu.len(),
            revision: self.revision,
        }
    }

    pub fn snapshot(&mut self) -> VirtualSurfaceSnapshot {
        let visible = self.visible_set();
        let visible_start = visible.nodes.first().map(|node| node.index).unwrap_or(0);
        let visible_end = visible
            .nodes
            .last()
            .map(|node| node.index.saturating_add(1))
            .unwrap_or(visible_start);
        VirtualSurfaceSnapshot {
            revision: self.revision,
            viewport: self.viewport,
            scroll: self.scroll,
            metrics: self.metrics(),
            cache: self.cache_stats(),
            visible,
            visible_start,
            visible_end,
        }
    }

    pub fn apply(
        &mut self,
        command: VirtualSurfaceCommand,
    ) -> Result<(), VirtualSurfaceError> {
        match command {
            VirtualSurfaceCommand::UpsertNode(node) => self.upsert_node(node),
            VirtualSurfaceCommand::UpsertNodes(nodes) => self.upsert_nodes(nodes),
            VirtualSurfaceCommand::RemoveNode(id) => self.remove_node(id),
            VirtualSurfaceCommand::RemoveRange { start, end } => {
                self.remove_range(start, end)
            }
            VirtualSurfaceCommand::SpliceNodes {
                start,
                delete,
                insert,
            } => self.splice_nodes(start, delete, insert),
            VirtualSurfaceCommand::RebaseSourceAfter {
                start,
                byte_delta,
                line_delta,
            } => self.rebase_source_after(start, byte_delta, line_delta),
            VirtualSurfaceCommand::ReplaceAll(nodes) => self.replace_all(nodes),
            VirtualSurfaceCommand::MarkDirty { node, kind } => {
                self.mark_dirty(node, kind)?;
                Ok(())
            }
            VirtualSurfaceCommand::MarkRangeDirty { start, end, kind } => {
                self.mark_range_dirty(start, end, kind)
            }
            VirtualSurfaceCommand::MarkSourceDirty {
                source,
                range,
                kind,
            } => self.mark_source_dirty(&source, range, kind),
            VirtualSurfaceCommand::ApplySourceEdit(edit) => self.apply_source_edit(&edit),
            VirtualSurfaceCommand::SetSourceTextOverlays { source, overlays } => {
                self.set_source_text_overlays(&source, overlays)
            }
            VirtualSurfaceCommand::ClearSourceTextOverlays { source } => {
                self.clear_source_text_overlays(&source)
            }
            VirtualSurfaceCommand::SetViewport(viewport) => {
                let relayout = self.viewport.width != viewport.width
                    || self.viewport.scale != viewport.scale;
                self.viewport = viewport;
                self.revision.bump();
                if relayout {
                    self.rebuild_layouts_from_estimates();
                    self.mark_all_layout_dirty();
                }
                Ok(())
            }
            VirtualSurfaceCommand::SetScroll(scroll) => {
                self.scroll = scroll;
                self.revision.bump();
                Ok(())
            }
            VirtualSurfaceCommand::RestoreScrollAnchor(anchor) => {
                self.restore_scroll_anchor(anchor)?;
                Ok(())
            }
            VirtualSurfaceCommand::RevealSource(target) => self.reveal_source(target),
            VirtualSurfaceCommand::CommitMeasuredLayouts(measurements) => {
                self.commit_measured_layouts(&measurements)
            }
            VirtualSurfaceCommand::SetConfig(config) => {
                self.config = config;
                self.revision.bump();
                let evicted = self.cache.set_policy(GpuResidencyPolicy {
                    max_warm_chunks: config.max_retained_chunks,
                    ..self.cache.policy()
                });
                self.record_evicted_chunks(evicted);
                self.mark_all_layout_dirty();
                Ok(())
            }
        }
    }

    pub fn replace_all(
        &mut self,
        nodes: Vec<VirtualNode>,
    ) -> Result<(), VirtualSurfaceError> {
        validate_unique_ids(&nodes)?;
        let previous_layouts = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let layout = self.layouts.get(index).copied().unwrap_or_else(|| {
                    VirtualLayout::estimated(
                        LayoutKey::new(node, self.viewport.width, self.viewport.scale),
                        0.0,
                        self.viewport.width,
                        node.geometry
                            .initial_height(self.config.fallback_node_height),
                    )
                });
                (node.id, layout)
            })
            .collect::<HashMap<_, _>>();
        let valid_ids = nodes.iter().map(|node| node.id).collect::<HashSet<_>>();
        self.nodes = nodes;
        self.rebuild_id_index();
        self.rebuild_source_index();
        self.rebuild_layouts_preserving(&previous_layouts);
        self.dirty_layout.retain(|id| valid_ids.contains(id));
        self.dirty_draw.retain(|id| valid_ids.contains(id));
        self.dirty_gpu.retain(|id| valid_ids.contains(id));
        let removed = self.cache.retain_nodes(&valid_ids);
        self.record_evicted_chunks(removed);
        let dirty_ids = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                let next_key = self.layouts[index].key;
                match previous_layouts.get(&node.id) {
                    Some(previous_layout) if previous_layout.key == next_key => None,
                    _ => Some(node.id),
                }
            })
            .collect::<Vec<_>>();
        for id in dirty_ids {
            self.mark_dirty_existing(id, DirtyKind::Layout);
        }
        self.revision.bump();
        Ok(())
    }

    pub fn upsert_node(&mut self, node: VirtualNode) -> Result<(), VirtualSurfaceError> {
        if let Some(index) = self.index_by_id.get(&node.id).copied() {
            let old = std::mem::replace(&mut self.nodes[index], node);
            let id = self.nodes[index].id;
            if old.id != id {
                self.rebuild_id_index();
            }
            self.rebuild_source_index();
            self.mark_dirty_existing(id, DirtyKind::Layout);
            self.revision.bump();
            return Ok(());
        }

        let id = node.id;
        let height = node
            .geometry
            .initial_height(self.config.fallback_node_height);
        let key = LayoutKey::new(&node, self.viewport.width, self.viewport.scale);
        let y = self.height_index.total_height();
        self.nodes.push(node);
        let index = self.nodes.len() - 1;
        self.index_by_id.insert(id, index);
        self.append_source_index_entry(index);
        self.layouts.push(VirtualLayout::estimated(
            key,
            y,
            self.viewport.width,
            height,
        ));
        self.height_index
            .splice(self.layouts.len().saturating_sub(1), 0, vec![height]);
        self.mark_dirty_existing(id, DirtyKind::Draw);
        self.revision.bump();
        Ok(())
    }

    pub fn upsert_nodes(
        &mut self,
        nodes: Vec<VirtualNode>,
    ) -> Result<(), VirtualSurfaceError> {
        if nodes.is_empty() {
            return Ok(());
        }
        validate_unique_ids(&nodes)?;

        let mut appended_heights = Vec::new();
        let append_start = self.nodes.len();
        let mut source_index_rebuild_from = None;
        let mut next_y = self.content_height();
        for node in nodes {
            if let Some(index) = self.index_by_id.get(&node.id).copied() {
                let id = node.id;
                self.nodes[index] = node;
                source_index_rebuild_from = Some(
                    source_index_rebuild_from
                        .map_or(index, |existing: usize| existing.min(index)),
                );
                self.mark_dirty_existing(id, DirtyKind::Layout);
                continue;
            }

            let id = node.id;
            let height = node
                .geometry
                .initial_height(self.config.fallback_node_height);
            let key = LayoutKey::new(&node, self.viewport.width, self.viewport.scale);
            self.nodes.push(node);
            self.index_by_id.insert(id, self.nodes.len() - 1);
            self.layouts.push(VirtualLayout::estimated(
                key,
                next_y,
                self.viewport.width,
                height,
            ));
            next_y += height;
            self.mark_dirty_existing(id, DirtyKind::Draw);
            appended_heights.push(height);
        }

        if !appended_heights.is_empty() {
            let start = self.layouts.len().saturating_sub(appended_heights.len());
            self.height_index.splice(start, 0, appended_heights);
        }
        if let Some(start) = source_index_rebuild_from {
            self.rebuild_source_index_from(start);
        } else {
            self.append_source_index_from(append_start);
        }
        self.revision.bump();
        Ok(())
    }

    pub fn remove_node(&mut self, id: NodeId) -> Result<(), VirtualSurfaceError> {
        let Some(index) = self.index_by_id.get(&id).copied() else {
            return Err(VirtualSurfaceError::MissingNode(id));
        };
        self.nodes.remove(index);
        self.layouts.remove(index);
        self.height_index.splice(index, 1, Vec::new());
        if let Some(chunk) = self.cache.remove(id) {
            self.record_evicted_chunk(chunk);
        }
        self.dirty_layout.remove(&id);
        self.dirty_draw.remove(&id);
        self.dirty_gpu.remove(&id);
        self.rebuild_id_index();
        self.rebuild_source_index();
        self.revision.bump();
        Ok(())
    }

    pub fn remove_range(
        &mut self,
        start: usize,
        end: usize,
    ) -> Result<(), VirtualSurfaceError> {
        if start > end || end > self.nodes.len() {
            return Err(VirtualSurfaceError::InvalidRange {
                start,
                end,
                len: self.nodes.len(),
            });
        }
        let ids = self.nodes[start..end]
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(chunk) = self.cache.remove(id) {
                self.record_evicted_chunk(chunk);
            }
            self.dirty_layout.remove(&id);
            self.dirty_draw.remove(&id);
            self.dirty_gpu.remove(&id);
        }
        self.nodes.drain(start..end);
        self.layouts.drain(start..end);
        self.height_index.splice(start, end - start, Vec::new());
        self.rebuild_id_index();
        self.rebuild_source_index();
        self.revision.bump();
        Ok(())
    }

    pub fn splice_nodes(
        &mut self,
        start: usize,
        delete: usize,
        insert: Vec<VirtualNode>,
    ) -> Result<(), VirtualSurfaceError> {
        let end = start.saturating_add(delete);
        if start > self.nodes.len() || end > self.nodes.len() {
            return Err(VirtualSurfaceError::InvalidRange {
                start,
                end,
                len: self.nodes.len(),
            });
        }
        validate_unique_ids(&insert)?;
        validate_splice_ids(&self.nodes, start, end, &insert)?;

        let removed_ids = self.nodes[start..end]
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        for id in removed_ids {
            if let Some(chunk) = self.cache.remove(id) {
                self.record_evicted_chunk(chunk);
            }
            self.dirty_layout.remove(&id);
            self.dirty_draw.remove(&id);
            self.dirty_gpu.remove(&id);
        }

        let mut insert_layouts = Vec::with_capacity(insert.len());
        let mut insert_heights = Vec::with_capacity(insert.len());
        let mut y = self.height_index.prefix_sum(start);
        for node in &insert {
            let height = node
                .geometry
                .initial_height(self.config.fallback_node_height);
            let key = LayoutKey::new(node, self.viewport.width, self.viewport.scale);
            insert_layouts.push(VirtualLayout::estimated(
                key,
                y,
                self.viewport.width,
                height,
            ));
            insert_heights.push(height);
            y += height;
        }

        let inserted_len = insert_layouts.len();
        self.nodes.splice(start..end, insert);
        self.layouts.splice(start..end, insert_layouts);
        self.height_index.splice(start, delete, insert_heights);
        self.rebuild_id_index();
        self.rebuild_source_index_from(start);

        let dirty_end = (start + inserted_len.max(1)).min(self.nodes.len());
        let ids = self.nodes[start..dirty_end]
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.mark_dirty_existing(id, DirtyKind::Draw);
        }
        self.revision.bump();
        Ok(())
    }

    pub fn rebase_source_after(
        &mut self,
        start: usize,
        byte_delta: i64,
        line_delta: i64,
    ) -> Result<(), VirtualSurfaceError> {
        if start > self.nodes.len() {
            return Err(VirtualSurfaceError::InvalidRange {
                start,
                end: start,
                len: self.nodes.len(),
            });
        }
        if byte_delta == 0 && line_delta == 0 {
            return Ok(());
        }

        for node in &mut self.nodes[start..] {
            rebase_node_source(node, byte_delta, line_delta);
        }
        self.rebuild_source_index_from(start);
        self.revision.bump();
        Ok(())
    }

    pub fn mark_dirty(
        &mut self,
        id: NodeId,
        kind: DirtyKind,
    ) -> Result<(), VirtualSurfaceError> {
        if !self.index_by_id.contains_key(&id) {
            return Err(VirtualSurfaceError::MissingNode(id));
        }
        self.mark_dirty_existing(id, kind);
        self.revision.bump();
        Ok(())
    }

    pub fn mark_range_dirty(
        &mut self,
        start: usize,
        end: usize,
        kind: DirtyKind,
    ) -> Result<(), VirtualSurfaceError> {
        if start > end || end > self.nodes.len() {
            return Err(VirtualSurfaceError::InvalidRange {
                start,
                end,
                len: self.nodes.len(),
            });
        }
        let ids = self.nodes[start..end]
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.mark_dirty_existing(id, kind);
        }
        self.revision.bump();
        Ok(())
    }

    pub fn mark_source_dirty(
        &mut self,
        source: &NodeSource,
        range: NodeSourceRange,
        kind: DirtyKind,
    ) -> Result<(), VirtualSurfaceError> {
        let ids = self.source_range_ids(source, range);
        for id in &ids {
            self.mark_dirty_existing(*id, kind);
        }
        if !ids.is_empty() {
            self.revision.bump();
        }
        Ok(())
    }

    pub fn apply_source_edit(
        &mut self,
        edit: &VirtualSourceEdit,
    ) -> Result<(), VirtualSurfaceError> {
        let mut ids = self.source_range_ids(&edit.source, edit.old_range);
        ids.extend(self.source_range_ids(&edit.source, edit.new_range));
        ids.extend(self.source_range_ids(&edit.source, edit.dirty_range()));
        ids.sort_unstable();
        ids.dedup();
        for id in &ids {
            self.mark_dirty_existing(*id, edit.kind);
        }
        if !ids.is_empty() {
            self.revision.bump();
        }
        Ok(())
    }

    pub fn set_source_text_overlays(
        &mut self,
        source: &NodeSource,
        overlays: Vec<VirtualTextOverlay>,
    ) -> Result<(), VirtualSurfaceError> {
        let mut touched = false;
        let indices = self
            .source_index
            .get(source)
            .map(|entries| entries.iter().map(|entry| entry.index).collect::<Vec<_>>())
            .unwrap_or_default();

        for index in indices {
            let Some(node) = self.nodes.get_mut(index) else {
                continue;
            };
            let Some(node_range) = node.source_range else {
                continue;
            };
            let node_overlays = overlays
                .iter()
                .filter(|overlay| overlay.range.intersects(node_range))
                .cloned()
                .collect::<Vec<_>>();
            if node_overlays.is_empty() {
                continue;
            }
            if node.text_plan.is_none() {
                if let Some(content) = node.content.clone() {
                    node.text_plan = Some(VirtualTextPlan::new(content));
                }
            }
            if let Some(text_plan) = node.text_plan.as_mut() {
                text_plan.overlays = node_overlays;
                touched = true;
            }
        }

        if touched {
            let ids = self.source_range_ids(source, NodeSourceRange::new(0, u64::MAX));
            for id in ids {
                self.mark_dirty_existing(id, DirtyKind::Draw);
            }
            self.revision.bump();
        }
        Ok(())
    }

    pub fn clear_source_text_overlays(
        &mut self,
        source: &NodeSource,
    ) -> Result<(), VirtualSurfaceError> {
        let mut touched = false;
        let indices = self
            .source_index
            .get(source)
            .map(|entries| entries.iter().map(|entry| entry.index).collect::<Vec<_>>())
            .unwrap_or_default();
        for index in indices {
            let Some(node) = self.nodes.get_mut(index) else {
                continue;
            };
            let Some(text_plan) = node.text_plan.as_mut() else {
                continue;
            };
            if !text_plan.overlays.is_empty() {
                text_plan.overlays.clear();
                touched = true;
            }
        }
        if touched {
            let ids = self.source_range_ids(source, NodeSourceRange::new(0, u64::MAX));
            for id in ids {
                self.mark_dirty_existing(id, DirtyKind::Draw);
            }
            self.revision.bump();
        }
        Ok(())
    }

    pub fn commit_measured_layouts(
        &mut self,
        measurements: &[VirtualMeasuredLayout],
    ) -> Result<(), VirtualSurfaceError> {
        if measurements.is_empty() {
            return Ok(());
        }

        let mut changed = false;
        for measurement in measurements {
            let Some(index) = self.index_by_id.get(&measurement.node).copied() else {
                return Err(VirtualSurfaceError::MissingNode(measurement.node));
            };
            let node_revision = self.nodes[index].revision;
            if node_revision != measurement.revision {
                return Err(VirtualSurfaceError::NodeRevisionMismatch {
                    node: measurement.node,
                    expected: node_revision,
                    actual: measurement.revision,
                });
            }

            let previous = self.current_layout(index);
            let mut next = previous;
            next.bounds.height = measurement.height.max(0.0);
            next.baseline = measurement.baseline.max(0.0);
            next.visual_line_count = measurement.visual_line_count;
            if next != previous {
                self.layouts[index] = next;
                self.height_index.set_height(index, next.bounds.height);
                self.mark_dirty_existing(measurement.node, DirtyKind::Draw);
                changed = true;
            }
        }

        if changed {
            self.clamp_scroll_to_content();
            self.revision.bump();
        }
        Ok(())
    }

    fn clamp_scroll_to_content(&mut self) {
        let max_scroll = (self.content_height() - self.viewport.height.max(0.0)).max(0.0);
        self.scroll.scroll_y = self.scroll.scroll_y.clamp(0.0, max_scroll);
    }

    /// Ensure all dirty node layouts are current. The current implementation
    /// uses deterministic estimate-based layout; later adapters can plug in
    /// real text measurement while keeping this invalidation surface.
    pub fn resolve_dirty_layout(&mut self) {
        if self.dirty_layout.is_empty() {
            return;
        }

        let dirty = self.dirty_layout.drain().collect::<Vec<_>>();
        let mut first_changed = self.nodes.len();
        for id in dirty {
            let Some(index) = self.index_by_id.get(&id).copied() else {
                continue;
            };
            let node = &self.nodes[index];
            let previous = self.current_layout(index);
            let height = measured_node_height(
                node.kind.clone(),
                node.geometry,
                self.viewport.width,
                self.viewport.scale,
                self.config.fallback_node_height,
            );
            let key = LayoutKey::new(node, self.viewport.width, self.viewport.scale);
            let next = VirtualLayout::estimated(
                key,
                previous.bounds.y,
                self.viewport.width,
                height,
            );
            if next != previous {
                self.layouts[index] = next;
                self.height_index.set_height(index, height);
                first_changed = first_changed.min(index);
            }
            self.dirty_draw.insert(id);
        }

        let _ = first_changed;
    }

    /// Query visible nodes in O(log n + visible). Calling this updates cache
    /// tiers for visible and near-visible chunks.
    pub fn visible_set(&mut self) -> VisibleSet {
        self.resolve_dirty_layout();
        let top = self.scroll.scroll_y.max(0.0);
        let bottom = top + self.viewport.height.max(0.0);
        let query_top = (top - self.config.overscan_px).max(0.0);
        let query_bottom = bottom + self.config.overscan_px;
        let range = self.height_index.visible_range(query_top, query_bottom);
        let content_height = self.content_height();
        let mut nodes = Vec::with_capacity(range.len());
        for index in range.start..range.end {
            let node = &self.nodes[index];
            let layout = self.current_layout(index);
            let tier = tier_for_distance(
                layout.bounds,
                top,
                bottom,
                self.config.warm_distance_px,
                self.config.cold_distance_px,
            );
            let chunk = self.cache.get_or_insert(node.id, layout.key, self.revision);
            chunk.tier = tier;
            chunk.draw_command_count = estimate_draw_commands(node.kind.clone());
            chunk.byte_size_estimate = estimate_chunk_bytes(node.kind.clone(), layout);
            nodes.push(VisibleNode {
                node: node.id,
                index,
                bounds: layout.bounds,
                screen_y: self.viewport.y + layout.bounds.y - top,
                cache_tier: tier,
            });
        }
        let evicted = self.cache.evict_to_budget();
        self.record_evicted_chunks(evicted);
        VisibleSet {
            nodes,
            query_top,
            query_bottom,
            content_height,
        }
    }

    /// Build a retained frame plan without emitting draw commands. This is the
    /// generalized equivalent of the current grid renderer's row plan: reuse
    /// current chunks, build missing chunks, rebuild dirty draw data, upload
    /// invalidated GPU data, and optionally bake stable static nodes.
    pub fn frame_plan(&mut self) -> VirtualFramePlan {
        self.resolve_dirty_layout();
        let top = self.scroll.scroll_y.max(0.0);
        let bottom = top + self.viewport.height.max(0.0);
        let query_top = (top - self.config.overscan_px).max(0.0);
        let query_bottom = bottom + self.config.overscan_px;
        let range = self.height_index.visible_range(query_top, query_bottom);
        let mut plan = VirtualFramePlan {
            visible: VisibleSet {
                query_top,
                query_bottom,
                content_height: self.content_height(),
                ..VisibleSet::default()
            },
            ..VirtualFramePlan::default()
        };
        let allow_texture_baking = self.cache.policy().allow_texture_baking;

        for index in range.start..range.end {
            let node = &self.nodes[index];
            let layout = self.current_layout(index);
            let tier = tier_for_distance(
                layout.bounds,
                top,
                bottom,
                self.config.warm_distance_px,
                self.config.cold_distance_px,
            );
            let previous = self.cache.get(node.id).cloned();
            let tile_range = visible_tile_range(
                node,
                layout,
                query_top,
                query_bottom,
                self.config.tile_height_px,
            );
            let action = match previous {
                None => VirtualFrameAction::Build,
                Some(chunk) if chunk.key != layout.key => VirtualFrameAction::RebuildDraw,
                Some(chunk) if chunk.tile_range != tile_range => {
                    VirtualFrameAction::Build
                }
                Some(chunk) if !chunk.cpu_draw_ready => VirtualFrameAction::RebuildDraw,
                Some(_) if self.dirty_draw.contains(&node.id) => {
                    VirtualFrameAction::RebuildDraw
                }
                Some(chunk) if !tile_range_ready(&chunk.gpu_ready_tiles, tile_range) => {
                    VirtualFrameAction::UploadGpu
                }
                Some(_) if self.dirty_gpu.contains(&node.id) => {
                    VirtualFrameAction::UploadGpu
                }
                Some(chunk)
                    if tier == CacheTier::Frozen
                        && allow_texture_baking
                        && !tile_range_ready(&chunk.texture_backed_tiles, tile_range) =>
                {
                    VirtualFrameAction::BakeStatic
                }
                Some(_) => VirtualFrameAction::Reuse,
            };

            let chunk = self.cache.get_or_insert(node.id, layout.key, self.revision);
            chunk.tier = tier;
            chunk.tile_range = tile_range;
            match action {
                VirtualFrameAction::Reuse => {}
                VirtualFrameAction::Build
                | VirtualFrameAction::RebuildDraw
                | VirtualFrameAction::UploadGpu
                | VirtualFrameAction::BakeStatic => {
                    chunk.gpu_ready = false;
                }
            }
            chunk.texture_backed =
                tile_range_ready(&chunk.texture_backed_tiles, tile_range);

            let visible = VisibleNode {
                node: node.id,
                index,
                bounds: layout.bounds,
                screen_y: self.viewport.y + layout.bounds.y - top,
                cache_tier: tier,
            };
            plan.visible.nodes.push(visible);
            plan.push(VirtualFrameNodePlan {
                node: node.id,
                index,
                bounds: layout.bounds,
                tile_range,
                cache_tier: tier,
                action,
            });
        }

        let evicted = self.cache.evict_to_budget();
        self.record_evicted_chunks(evicted);
        self.append_prefetch_hints(&mut plan, range, query_top, query_bottom);
        plan
    }

    /// Emit backend-neutral commands for currently visible nodes. This is a
    /// protocol bridge, not the final backend GPU implementation.
    pub fn build_draw_commands(&mut self) -> Vec<VirtualDrawCommand> {
        let visible = self.visible_set();
        let clip = VirtualBounds::new(
            self.viewport.x,
            self.viewport.y,
            self.viewport.width,
            self.viewport.height,
        );
        let mut commands = Vec::with_capacity(visible.nodes.len() * 4);
        for visible_node in visible.nodes {
            let node = &self.nodes[visible_node.index];
            let mut bounds = visible_node.bounds;
            bounds.x += self.viewport.x;
            bounds.y = visible_node.screen_y;
            commands.push(VirtualDrawCommand::BeginNode {
                node: node.id,
                kind: node.kind.clone(),
                bounds,
                clip,
            });
            if node.kind != VirtualNodeKind::Overlay {
                commands.push(VirtualDrawCommand::Rect {
                    node: node.id,
                    bounds,
                    color: color_for_kind(&node.kind),
                });
            }
            if node.text_hash != 0 {
                commands.push(VirtualDrawCommand::TextRun {
                    node: node.id,
                    x: bounds.x,
                    y: bounds.y,
                    content: node.content.clone(),
                    text_plan: node.text_plan.clone(),
                    text_hash: node.text_hash,
                    byte_len: estimated_text_len(node),
                });
            }
            commands.push(VirtualDrawCommand::EndNode { node: node.id });
        }
        self.update_chunk_draw_stats(&commands);
        commands
    }

    /// Build a complete backend-neutral frame transaction.
    ///
    /// This is the API a future Sugarloaf backend adapter should consume. It
    /// keeps planning, retained resource operations, and visible draw commands
    /// in one stable packet while leaving actual Vulkan/Metal/WebGPU handle
    /// management behind the backend boundary.
    pub fn build_frame_transaction(&mut self) -> VirtualFrameTransaction {
        let plan = self.frame_plan();
        let mut transaction = VirtualFrameTransaction::new(self.revision, plan)
            .with_viewport_scroll(self.viewport, self.scroll);
        transaction
            .resource_ops
            .extend(self.pending_resource_drops.drain(..));
        let clip = VirtualBounds::new(
            self.viewport.x,
            self.viewport.y,
            self.viewport.width,
            self.viewport.height,
        );
        let top = self.scroll.scroll_y.max(0.0);

        let planned_nodes = transaction.plan.nodes.clone();
        for planned in planned_nodes {
            let node = &self.nodes[planned.index];
            let layout = self.current_layout(planned.index);
            let retained = self.cache.get(node.id).cloned();
            let planned_tile_list = planned_tiles(node.id, planned.tile_range);
            let resource_tiles = match planned.action {
                VirtualFrameAction::UploadGpu => planned_tile_list
                    .into_iter()
                    .filter(|tile| {
                        retained.as_ref().is_none_or(|chunk| {
                            !chunk.gpu_ready_tiles.contains(&tile_index(*tile))
                        })
                    })
                    .collect::<Vec<_>>(),
                VirtualFrameAction::BakeStatic => planned_tile_list
                    .into_iter()
                    .filter(|tile| {
                        retained.as_ref().is_none_or(|chunk| {
                            !chunk.texture_backed_tiles.contains(&tile_index(*tile))
                        })
                    })
                    .collect::<Vec<_>>(),
                VirtualFrameAction::Reuse
                | VirtualFrameAction::Build
                | VirtualFrameAction::RebuildDraw => planned_tile_list,
            };

            for tile in resource_tiles {
                let bounds = tile_bounds(layout.bounds, tile, self.config.tile_height_px);
                let estimated_bytes = estimate_bounds_bytes(node.kind.clone(), bounds);
                let gpu_descriptor = resource_descriptor(
                    node.id,
                    layout.key,
                    tile,
                    VirtualResourceKind::GpuDrawBuffer,
                    planned.cache_tier,
                    bounds,
                    estimated_bytes,
                    self.revision,
                );

                match planned.action {
                    VirtualFrameAction::Reuse => {
                        transaction
                            .resource_ops
                            .push(VirtualResourceOp::Retain(gpu_descriptor));
                    }
                    VirtualFrameAction::Build | VirtualFrameAction::RebuildDraw => {
                        let cpu_descriptor = resource_descriptor(
                            node.id,
                            layout.key,
                            tile,
                            VirtualResourceKind::CpuDrawList,
                            planned.cache_tier,
                            bounds,
                            estimated_bytes,
                            self.revision,
                        );
                        transaction
                            .resource_ops
                            .push(VirtualResourceOp::BuildDrawList(cpu_descriptor));
                        let hit_descriptor = resource_descriptor(
                            node.id,
                            layout.key,
                            tile,
                            VirtualResourceKind::HitRegion,
                            planned.cache_tier,
                            bounds,
                            std::mem::size_of::<VirtualBounds>(),
                            self.revision,
                        );
                        transaction
                            .resource_ops
                            .push(VirtualResourceOp::BuildHitRegion(hit_descriptor));
                        transaction
                            .resource_ops
                            .push(VirtualResourceOp::UploadGpuBuffer(gpu_descriptor));
                    }
                    VirtualFrameAction::UploadGpu => {
                        transaction
                            .resource_ops
                            .push(VirtualResourceOp::UploadGpuBuffer(gpu_descriptor));
                    }
                    VirtualFrameAction::BakeStatic => {
                        let texture_descriptor = resource_descriptor(
                            node.id,
                            layout.key,
                            tile,
                            VirtualResourceKind::TextureTile,
                            planned.cache_tier,
                            bounds,
                            estimated_bytes,
                            self.revision,
                        );
                        transaction
                            .resource_ops
                            .push(VirtualResourceOp::BakeTexture(texture_descriptor));
                    }
                }
            }

            let mut bounds = layout.bounds;
            bounds.x += self.viewport.x;
            bounds.y = self.viewport.y + layout.bounds.y - top;
            transaction.push_command(VirtualDrawCommand::BeginNode {
                node: node.id,
                kind: node.kind.clone(),
                bounds,
                clip,
            });
            if node.kind != VirtualNodeKind::Overlay {
                transaction.push_command(VirtualDrawCommand::Rect {
                    node: node.id,
                    bounds,
                    color: color_for_kind(&node.kind),
                });
            }
            if node.text_hash != 0 {
                transaction.push_command(VirtualDrawCommand::TextRun {
                    node: node.id,
                    x: bounds.x,
                    y: bounds.y,
                    content: node.content.clone(),
                    text_plan: node.text_plan.clone(),
                    text_hash: node.text_hash,
                    byte_len: estimated_text_len(node),
                });
            }
            transaction.push_command(VirtualDrawCommand::EndNode { node: node.id });
        }

        transaction
    }

    pub fn build_scheduled_frame_transaction(
        &mut self,
        policy: VirtualFrameSchedulePolicy,
    ) -> VirtualScheduledFrameTransaction {
        self.build_frame_transaction().scheduled(policy)
    }

    /// Commit backend results for a previously built frame transaction.
    ///
    /// This is the point where CPU draw stats and GPU readiness become true.
    /// Building a transaction alone is just a plan; a backend, probe, or test
    /// must acknowledge the resource ids it successfully retained/uploaded.
    pub fn commit_frame_transaction(
        &mut self,
        transaction: &VirtualFrameTransaction,
        commit: &VirtualFrameCommit,
    ) -> Result<(), VirtualSurfaceError> {
        if commit.revision != transaction.revision {
            return Err(VirtualSurfaceError::RevisionMismatch {
                expected: transaction.revision,
                actual: commit.revision,
            });
        }
        if !commit.is_success() {
            return Err(VirtualSurfaceError::ResourceCommitFailed {
                failed: commit.failed.len(),
            });
        }

        let mut ready = HashSet::with_capacity(commit.ready.len());
        ready.extend(commit.ready.iter().copied());
        let mut draw_ready_nodes = HashSet::new();
        let mut hit_ready_nodes = HashSet::new();
        let mut gpu_ready_tiles: HashMap<NodeId, BTreeSet<u32>> = HashMap::new();
        let mut texture_ready_tiles: HashMap<NodeId, BTreeSet<u32>> = HashMap::new();
        for op in &transaction.resource_ops {
            let Some(descriptor) = op.descriptor() else {
                continue;
            };
            if !ready.contains(&descriptor.id) {
                continue;
            }
            match descriptor.kind {
                VirtualResourceKind::CpuDrawList => {
                    draw_ready_nodes.insert(descriptor.node);
                }
                VirtualResourceKind::HitRegion => {
                    hit_ready_nodes.insert(descriptor.node);
                }
                VirtualResourceKind::GpuDrawBuffer => {
                    gpu_ready_tiles
                        .entry(descriptor.node)
                        .or_default()
                        .insert(tile_index(descriptor.tile));
                }
                VirtualResourceKind::TextureTile => {
                    texture_ready_tiles
                        .entry(descriptor.node)
                        .or_default()
                        .insert(tile_index(descriptor.tile));
                }
            }
        }

        self.update_chunk_resource_stats_for_nodes(
            &transaction.commands,
            &draw_ready_nodes,
            &hit_ready_nodes,
            &gpu_ready_tiles,
            &texture_ready_tiles,
        );
        Ok(())
    }

    pub fn visible_range_for_current_viewport(&mut self) -> AxisRange {
        self.resolve_dirty_layout();
        let top = (self.scroll.scroll_y - self.config.overscan_px).max(0.0);
        let bottom =
            self.scroll.scroll_y + self.viewport.height + self.config.overscan_px;
        self.height_index.visible_range(top, bottom)
    }

    pub fn capture_scroll_anchor(
        &mut self,
        viewport_y: f32,
    ) -> Option<VirtualScrollAnchor> {
        self.resolve_dirty_layout();
        let viewport_y = viewport_y.clamp(0.0, self.viewport.height.max(0.0));
        let content_y = self.scroll.scroll_y + viewport_y;
        let index = self.height_index.lower_bound_y(content_y);
        let node = self.nodes.get(index)?;
        let bounds = self.current_layout(index).bounds;
        Some(VirtualScrollAnchor {
            node: node.id,
            index,
            local_y: (content_y - bounds.y).clamp(0.0, bounds.height),
            viewport_y,
        })
    }

    pub fn restore_scroll_anchor(
        &mut self,
        anchor: VirtualScrollAnchor,
    ) -> Result<(), VirtualSurfaceError> {
        self.resolve_dirty_layout();
        let index = self
            .index_by_id
            .get(&anchor.node)
            .copied()
            .unwrap_or(anchor.index.min(self.nodes.len().saturating_sub(1)));
        if self.nodes.is_empty() {
            self.scroll.scroll_y = 0.0;
            self.scroll.velocity_y = 0.0;
            self.revision.bump();
            return Ok(());
        }
        let bounds = self.current_layout(index).bounds;
        let max_scroll = (self.content_height() - self.viewport.height).max(0.0);
        self.scroll.scroll_y =
            (bounds.y + anchor.local_y - anchor.viewport_y).clamp(0.0, max_scroll);
        self.scroll.velocity_y = 0.0;
        self.revision.bump();
        Ok(())
    }

    pub fn source_matches(
        &mut self,
        query: VirtualSourceQuery,
    ) -> Vec<VirtualSourceMatch> {
        self.resolve_dirty_layout();
        self.source_range_entries(&query.source, query.range)
            .into_iter()
            .filter_map(|entry| {
                let node = self.nodes.get(entry.index)?;
                let source_range = node.source_range?;
                let overlap = source_range.overlap(query.range)?;
                Some(VirtualSourceMatch {
                    node: node.id,
                    index: entry.index,
                    bounds: self.current_layout(entry.index).bounds,
                    source_range,
                    overlap,
                })
            })
            .collect()
    }

    pub fn reveal_source(
        &mut self,
        target: VirtualRevealTarget,
    ) -> Result<(), VirtualSurfaceError> {
        let Some(first) = self
            .source_matches(VirtualSourceQuery::new(target.source, target.range))
            .into_iter()
            .next()
        else {
            return Ok(());
        };

        let before = self.scroll.scroll_y;
        let max_scroll = (self.content_height() - self.viewport.height).max(0.0);
        let top = first.bounds.y;
        let bottom = first.bounds.bottom();
        let viewport_top = self.scroll.scroll_y;
        let viewport_bottom = viewport_top + self.viewport.height;
        let next = match target.align {
            VirtualRevealAlign::Start => top,
            VirtualRevealAlign::Center => {
                top - (self.viewport.height - first.bounds.height).max(0.0) * 0.5
            }
            VirtualRevealAlign::End => bottom - self.viewport.height,
            VirtualRevealAlign::Nearest => {
                if top >= viewport_top && bottom <= viewport_bottom {
                    before
                } else if top < viewport_top {
                    top
                } else {
                    bottom - self.viewport.height
                }
            }
        }
        .clamp(0.0, max_scroll);

        if (next - before).abs() > f32::EPSILON {
            self.scroll.scroll_y = next;
            self.scroll.velocity_y = 0.0;
            self.revision.bump();
        }
        Ok(())
    }

    /// Hit test a logical screen-space point against the retained height index.
    /// This does not walk the whole surface; it maps the y coordinate into
    /// content space and uses the prefix-sum index to find the candidate node.
    pub fn hit_test(&mut self, input: VirtualHitTest) -> Option<VirtualHit> {
        self.resolve_dirty_layout();
        if input.x < self.viewport.x
            || input.x > self.viewport.x + self.viewport.width
            || input.y < self.viewport.y
            || input.y > self.viewport.y + self.viewport.height
        {
            return None;
        }
        let content_y = self.scroll.scroll_y + (input.y - self.viewport.y);
        let index = self.height_index.lower_bound_y(content_y);
        let node = self.nodes.get(index)?;
        let bounds = self.current_layout(index).bounds;
        if !bounds.intersects_y(content_y, content_y) {
            return None;
        }
        Some(VirtualHit {
            node: node.id,
            index,
            bounds,
            local_x: input.x - self.viewport.x - bounds.x,
            local_y: content_y - bounds.y,
            source_range: node.source_range,
        })
    }

    fn mark_dirty_existing(&mut self, id: NodeId, kind: DirtyKind) {
        match kind {
            DirtyKind::Layout => {
                self.dirty_layout.insert(id);
                self.dirty_draw.insert(id);
                self.dirty_gpu.insert(id);
                self.cache.invalidate_draw(id);
            }
            DirtyKind::Draw => {
                self.dirty_draw.insert(id);
                self.dirty_gpu.insert(id);
                self.cache.invalidate_draw(id);
            }
            DirtyKind::Gpu => {
                self.dirty_gpu.insert(id);
                self.cache.invalidate_gpu(id);
            }
        }
    }

    fn mark_all_layout_dirty(&mut self) {
        let ids = self.nodes.iter().map(|node| node.id).collect::<Vec<_>>();
        for id in ids {
            self.mark_dirty_existing(id, DirtyKind::Layout);
        }
    }

    fn rebuild_id_index(&mut self) {
        self.index_by_id.clear();
        for (index, node) in self.nodes.iter().enumerate() {
            self.index_by_id.insert(node.id, index);
        }
    }

    fn rebuild_source_index(&mut self) {
        self.source_index.clear();
        for (index, node) in self.nodes.iter().enumerate() {
            let Some((source, entry)) = source_index_entry(index, node) else {
                continue;
            };
            self.source_index.entry(source).or_default().push(entry);
        }
        for entries in self.source_index.values_mut() {
            entries.sort_by_key(|entry| (entry.start, entry.end, entry.index));
        }
    }

    fn rebuild_source_index_from(&mut self, start: usize) {
        for entries in self.source_index.values_mut() {
            let keep = entries.partition_point(|entry| entry.index < start);
            entries.truncate(keep);
        }
        self.source_index.retain(|_, entries| !entries.is_empty());
        self.append_source_index_from(start);
    }

    fn append_source_index_from(&mut self, start: usize) {
        for index in start..self.nodes.len() {
            self.append_source_index_entry(index);
        }
    }

    fn append_source_index_entry(&mut self, index: usize) {
        let Some(node) = self.nodes.get(index) else {
            return;
        };
        let Some((source, entry)) = source_index_entry(index, node) else {
            return;
        };
        let entries = self.source_index.entry(source).or_default();
        let needs_sort = entries.last().is_some_and(|last| {
            (entry.start, entry.end, entry.index) < (last.start, last.end, last.index)
        });
        entries.push(entry);
        if needs_sort {
            entries.sort_by_key(|entry| (entry.start, entry.end, entry.index));
        }
    }

    fn source_range_ids(
        &self,
        source: &NodeSource,
        range: NodeSourceRange,
    ) -> Vec<NodeId> {
        self.source_range_entries(source, range)
            .into_iter()
            .map(|entry| entry.node)
            .collect()
    }

    fn source_range_entries(
        &self,
        source: &NodeSource,
        range: NodeSourceRange,
    ) -> Vec<SourceIndexEntry> {
        let Some(entries) = self.source_index.get(source) else {
            return Vec::new();
        };
        if entries.is_empty() {
            return Vec::new();
        }

        let mut lo = 0usize;
        let mut hi = entries.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if entries[mid].end <= range.start {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        let mut out = Vec::new();
        for entry in entries.iter().skip(lo) {
            if !range.is_empty() && entry.start >= range.end {
                break;
            }
            let hit = if range.is_empty() {
                entry.start <= range.start && range.start <= entry.end
            } else {
                entry.start < range.end && range.start < entry.end
            };
            if hit {
                out.push(*entry);
            }
        }
        out
    }

    fn append_prefetch_hints(
        &self,
        plan: &mut VirtualFramePlan,
        visible_range: AxisRange,
        query_top: f32,
        query_bottom: f32,
    ) {
        const MAX_PREFETCH_PER_SIDE: usize = 24;

        let mut before = Vec::new();
        let mut index = visible_range.start;
        while index > 0 && before.len() < MAX_PREFETCH_PER_SIDE {
            index -= 1;
            let layout = self.current_layout(index);
            if layout.bounds.bottom() + self.config.warm_distance_px < query_top {
                break;
            }
            let node = &self.nodes[index];
            let tile_range = visible_tile_range(
                node,
                layout,
                query_top - self.config.warm_distance_px,
                query_top,
                self.config.tile_height_px,
            );
            if !tile_range.is_empty() {
                before.push(VirtualPrefetchHint {
                    node: node.id,
                    index,
                    bounds: layout.bounds,
                    tile_range,
                    priority: (MAX_PREFETCH_PER_SIDE - before.len()) as u16,
                    content: node.content.clone(),
                    text_plan: node.text_plan.clone(),
                });
            }
        }
        before.reverse();
        plan.prefetch.extend(before);

        let mut after_count = 0usize;
        let mut index = visible_range.end;
        while index < self.nodes.len() && after_count < MAX_PREFETCH_PER_SIDE {
            let layout = self.current_layout(index);
            if layout.bounds.y > query_bottom + self.config.warm_distance_px {
                break;
            }
            let node = &self.nodes[index];
            let tile_range = visible_tile_range(
                node,
                layout,
                query_bottom,
                query_bottom + self.config.warm_distance_px,
                self.config.tile_height_px,
            );
            if !tile_range.is_empty() {
                plan.prefetch.push(VirtualPrefetchHint {
                    node: node.id,
                    index,
                    bounds: layout.bounds,
                    tile_range,
                    priority: (MAX_PREFETCH_PER_SIDE - after_count) as u16,
                    content: node.content.clone(),
                    text_plan: node.text_plan.clone(),
                });
            }
            after_count += 1;
            index += 1;
        }
    }

    fn current_layout(&self, index: usize) -> VirtualLayout {
        let mut layout = self.layouts[index];
        layout.bounds = self.height_index.bounds_for(index, self.viewport.width);
        layout
    }

    fn rebuild_layouts_from_estimates(&mut self) {
        let mut y = 0.0;
        self.layouts.clear();
        self.layouts.reserve(self.nodes.len());
        let mut heights = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let height = node
                .geometry
                .initial_height(self.config.fallback_node_height);
            let key = LayoutKey::new(node, self.viewport.width, self.viewport.scale);
            self.layouts.push(VirtualLayout::estimated(
                key,
                y,
                self.viewport.width,
                height,
            ));
            heights.push(height);
            y += height;
        }
        self.height_index = HeightIndex::from_heights(heights);
    }

    fn rebuild_layouts_preserving(
        &mut self,
        previous_layouts: &HashMap<NodeId, VirtualLayout>,
    ) {
        let mut y = 0.0;
        self.layouts.clear();
        self.layouts.reserve(self.nodes.len());
        let mut heights = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let key = LayoutKey::new(node, self.viewport.width, self.viewport.scale);
            let mut layout = previous_layouts
                .get(&node.id)
                .copied()
                .filter(|previous| previous.key == key)
                .unwrap_or_else(|| {
                    let height = node
                        .geometry
                        .initial_height(self.config.fallback_node_height);
                    VirtualLayout::estimated(key, y, self.viewport.width, height)
                });
            layout.key = key;
            layout.bounds.x = 0.0;
            layout.bounds.y = y;
            layout.bounds.width = self.viewport.width;
            layout.bounds.height = layout.bounds.height.max(0.0);
            heights.push(layout.bounds.height);
            y += layout.bounds.height;
            self.layouts.push(layout);
        }
        self.height_index = HeightIndex::from_heights(heights);
    }

    fn update_chunk_draw_stats(&mut self, commands: &[VirtualDrawCommand]) {
        let all_nodes = commands.iter().map(command_node).collect::<HashSet<_>>();
        let all_tiles = all_nodes
            .iter()
            .copied()
            .map(|node| (node, BTreeSet::from([0])))
            .collect::<HashMap<_, _>>();
        self.update_chunk_resource_stats_for_nodes(
            commands,
            &all_nodes,
            &all_nodes,
            &all_tiles,
            &HashMap::new(),
        );
    }

    fn update_chunk_resource_stats_for_nodes(
        &mut self,
        commands: &[VirtualDrawCommand],
        draw_ready_nodes: &HashSet<NodeId>,
        hit_ready_nodes: &HashSet<NodeId>,
        gpu_ready_tiles: &HashMap<NodeId, BTreeSet<u32>>,
        texture_ready_tiles: &HashMap<NodeId, BTreeSet<u32>>,
    ) {
        let mut counts: HashMap<NodeId, (u32, usize)> = HashMap::new();
        for command in commands {
            let node = command_node(command);
            if !draw_ready_nodes.contains(&node) {
                continue;
            }
            let entry = counts.entry(node).or_default();
            entry.0 = entry.0.saturating_add(1);
            entry.1 = entry.1.saturating_add(command.byte_size_estimate());
        }
        for (node, (count, bytes)) in counts {
            if let Some(chunk) = self.cache.get(node).cloned() {
                let retained = self.cache.get_or_insert(node, chunk.key, self.revision);
                retained.draw_command_count = count;
                retained.byte_size_estimate = bytes;
                retained.cpu_draw_ready = true;
            }
        }
        for node in hit_ready_nodes.iter().copied() {
            if let Some(chunk) = self.cache.get(node).cloned() {
                let retained = self.cache.get_or_insert(node, chunk.key, self.revision);
                retained.hit_region_ready = true;
            }
        }
        for (node, tiles) in gpu_ready_tiles {
            if let Some(chunk) = self.cache.get(*node).cloned() {
                let retained = self.cache.get_or_insert(*node, chunk.key, self.revision);
                retained.gpu_ready_tiles.extend(tiles.iter().copied());
                retained.gpu_ready =
                    tile_range_ready(&retained.gpu_ready_tiles, retained.tile_range);
            }
        }
        for (node, tiles) in texture_ready_tiles {
            if let Some(chunk) = self.cache.get(*node).cloned() {
                let retained = self.cache.get_or_insert(*node, chunk.key, self.revision);
                retained.texture_backed_tiles.extend(tiles.iter().copied());
                retained.gpu_ready_tiles.extend(tiles.iter().copied());
                retained.texture_backed =
                    tile_range_ready(&retained.texture_backed_tiles, retained.tile_range);
                retained.gpu_ready =
                    tile_range_ready(&retained.gpu_ready_tiles, retained.tile_range);
            }
        }
        for node in draw_ready_nodes {
            self.dirty_draw.remove(node);
        }
        for node in gpu_ready_tiles.keys().chain(texture_ready_tiles.keys()) {
            if self
                .cache
                .get(*node)
                .is_some_and(|chunk| chunk.gpu_ready || chunk.texture_backed)
            {
                self.dirty_gpu.remove(node);
            }
        }
    }

    fn record_evicted_chunks(&mut self, chunks: Vec<RetainedChunk>) {
        for chunk in chunks {
            self.record_evicted_chunk(chunk);
        }
    }

    fn record_evicted_chunk(&mut self, chunk: RetainedChunk) {
        for tile in planned_tiles(chunk.node, chunk.tile_range) {
            self.pending_resource_drops.push(VirtualResourceOp::Drop(
                VirtualResourceId::from_layout_tile(
                    chunk.key,
                    tile,
                    resource_salt(VirtualResourceKind::GpuDrawBuffer),
                ),
            ));
            if chunk.texture_backed {
                self.pending_resource_drops.push(VirtualResourceOp::Drop(
                    VirtualResourceId::from_layout_tile(
                        chunk.key,
                        tile,
                        resource_salt(VirtualResourceKind::TextureTile),
                    ),
                ));
            }
            self.pending_resource_drops.push(VirtualResourceOp::Drop(
                VirtualResourceId::from_layout_tile(
                    chunk.key,
                    tile,
                    resource_salt(VirtualResourceKind::HitRegion),
                ),
            ));
        }
    }
}

fn validate_unique_ids(nodes: &[VirtualNode]) -> Result<(), VirtualSurfaceError> {
    let mut seen = HashSet::with_capacity(nodes.len());
    for node in nodes {
        if !seen.insert(node.id) {
            return Err(VirtualSurfaceError::DuplicateNode(node.id));
        }
    }
    Ok(())
}

fn validate_splice_ids(
    existing: &[VirtualNode],
    start: usize,
    end: usize,
    insert: &[VirtualNode],
) -> Result<(), VirtualSurfaceError> {
    let inserted = insert.iter().map(|node| node.id).collect::<HashSet<_>>();
    for (index, node) in existing.iter().enumerate() {
        if (start..end).contains(&index) {
            continue;
        }
        if inserted.contains(&node.id) {
            return Err(VirtualSurfaceError::DuplicateNode(node.id));
        }
    }
    Ok(())
}

fn rebase_node_source(node: &mut VirtualNode, byte_delta: i64, line_delta: i64) {
    if let Some(range) = node.source_range.as_mut() {
        rebase_source_range(range, byte_delta);
    }

    let rebased_content = if let Some(content) = node.content.as_mut() {
        rebase_source_range(&mut content.range, byte_delta);
        content.byte_len = content.range.len();
        content.line_start = shift_u64(content.line_start, line_delta);
        Some(content.clone())
    } else {
        None
    };

    if let Some(text_plan) = node.text_plan.as_mut() {
        if let Some(content) = rebased_content {
            text_plan.content = content;
        } else {
            rebase_source_range(&mut text_plan.content.range, byte_delta);
            text_plan.content.byte_len = text_plan.content.range.len();
            text_plan.content.line_start =
                shift_u64(text_plan.content.line_start, line_delta);
        }
    }
}

fn rebase_source_range(range: &mut NodeSourceRange, delta: i64) {
    range.start = shift_u64(range.start, delta);
    range.end = shift_u64(range.end, delta).max(range.start);
}

fn shift_u64(value: u64, delta: i64) -> u64 {
    if delta >= 0 {
        value.saturating_add(delta as u64)
    } else {
        value.saturating_sub(delta.unsigned_abs())
    }
}

fn command_node(command: &VirtualDrawCommand) -> NodeId {
    match command {
        VirtualDrawCommand::BeginNode { node, .. }
        | VirtualDrawCommand::Rect { node, .. }
        | VirtualDrawCommand::TextRun { node, .. }
        | VirtualDrawCommand::EndNode { node } => *node,
    }
}

fn source_index_entry(
    index: usize,
    node: &VirtualNode,
) -> Option<(NodeSource, SourceIndexEntry)> {
    let (Some(source), Some(range)) = (&node.source, node.source_range) else {
        return None;
    };
    Some((
        source.clone(),
        SourceIndexEntry {
            start: range.start,
            end: range.end,
            index,
            node: node.id,
        },
    ))
}

fn measured_node_height(
    kind: VirtualNodeKind,
    geometry: NodeGeometry,
    width: f32,
    scale: f32,
    fallback: f32,
) -> f32 {
    if let Some(height) = geometry.fixed_height {
        return height.max(0.0);
    }
    let base = geometry.estimated_height.unwrap_or_else(|| match kind {
        VirtualNodeKind::Heading => 34.0,
        VirtualNodeKind::CodeLine => 20.0,
        VirtualNodeKind::CodeTile => 320.0,
        VirtualNodeKind::CodeBlock => 180.0,
        VirtualNodeKind::Table => 220.0,
        VirtualNodeKind::TableTile => 360.0,
        VirtualNodeKind::AgentMessage => 96.0,
        VirtualNodeKind::ToolCard => 140.0,
        VirtualNodeKind::DiffHunk => 180.0,
        VirtualNodeKind::Image => 240.0,
        VirtualNodeKind::Overlay => 0.0,
        VirtualNodeKind::Root
        | VirtualNodeKind::Text
        | VirtualNodeKind::MarkdownBlock
        | VirtualNodeKind::Custom(_) => fallback,
    });
    let width_factor = if geometry.can_split {
        (900.0 / width.max(220.0)).clamp(0.85, 1.65)
    } else {
        1.0
    };
    (base * width_factor * scale.clamp(0.5, 3.0)).max(0.0)
}

fn tier_for_distance(
    bounds: VirtualBounds,
    viewport_top: f32,
    viewport_bottom: f32,
    warm_distance: f32,
    cold_distance: f32,
) -> CacheTier {
    if bounds.intersects_y(viewport_top, viewport_bottom) {
        return CacheTier::Hot;
    }
    let distance = if bounds.bottom() < viewport_top {
        viewport_top - bounds.bottom()
    } else {
        bounds.y - viewport_bottom
    };
    if distance <= warm_distance {
        CacheTier::Warm
    } else if distance <= cold_distance {
        CacheTier::Cold
    } else {
        CacheTier::Frozen
    }
}

fn estimate_draw_commands(kind: VirtualNodeKind) -> u32 {
    match kind {
        VirtualNodeKind::Table | VirtualNodeKind::TableTile => 12,
        VirtualNodeKind::CodeBlock | VirtualNodeKind::CodeTile => 8,
        VirtualNodeKind::AgentMessage | VirtualNodeKind::ToolCard => 6,
        VirtualNodeKind::Overlay => 2,
        _ => 4,
    }
}

fn estimate_chunk_bytes(kind: VirtualNodeKind, layout: VirtualLayout) -> usize {
    estimate_bounds_bytes(kind, layout.bounds)
}

fn estimate_bounds_bytes(kind: VirtualNodeKind, bounds: VirtualBounds) -> usize {
    let area = (bounds.width * bounds.height).max(0.0) as usize;
    let base = match kind {
        VirtualNodeKind::Table | VirtualNodeKind::TableTile => 4096,
        VirtualNodeKind::CodeBlock | VirtualNodeKind::CodeTile => 3072,
        VirtualNodeKind::AgentMessage | VirtualNodeKind::ToolCard => 2048,
        _ => 1024,
    };
    base + area / 16
}

fn visible_tile_range(
    node: &VirtualNode,
    layout: VirtualLayout,
    query_top: f32,
    query_bottom: f32,
    tile_height: f32,
) -> VirtualTileRange {
    if !node.geometry.can_split || layout.bounds.height <= tile_height.max(1.0) {
        return VirtualTileRange::WHOLE_NODE;
    }

    let tile_height = tile_height.max(1.0);
    let total_tiles = (layout.bounds.height / tile_height).ceil().max(1.0) as u32;
    let local_top = (query_top - layout.bounds.y).clamp(0.0, layout.bounds.height);
    let local_bottom = (query_bottom - layout.bounds.y).clamp(0.0, layout.bounds.height);
    let start =
        ((local_top / tile_height).floor() as u32).min(total_tiles.saturating_sub(1));
    let end = ((local_bottom / tile_height).ceil() as u32)
        .max(start + 1)
        .min(total_tiles);
    VirtualTileRange::new(start, end)
}

fn planned_tiles(node: NodeId, range: VirtualTileRange) -> Vec<Option<VirtualTileId>> {
    if range.is_whole_node() {
        return vec![None];
    }

    (range.start..range.end)
        .map(|tile| Some(VirtualTileId::new(node, tile)))
        .collect()
}

fn tile_index(tile: Option<VirtualTileId>) -> u32 {
    tile.map(|tile| tile.tile).unwrap_or(0)
}

fn tile_range_ready(ready: &BTreeSet<u32>, range: VirtualTileRange) -> bool {
    if range.is_whole_node() {
        return ready.contains(&0);
    }
    (range.start..range.end).all(|tile| ready.contains(&tile))
}

fn tile_bounds(
    bounds: VirtualBounds,
    tile: Option<VirtualTileId>,
    tile_height: f32,
) -> VirtualBounds {
    let Some(tile) = tile else {
        return bounds;
    };
    let tile_height = tile_height.max(1.0);
    let y = bounds.y + tile.tile as f32 * tile_height;
    let bottom = (y + tile_height).min(bounds.bottom());
    VirtualBounds::new(bounds.x, y, bounds.width, bottom - y)
}

fn resource_descriptor(
    node: NodeId,
    layout: LayoutKey,
    tile: Option<VirtualTileId>,
    kind: VirtualResourceKind,
    tier: CacheTier,
    bounds: VirtualBounds,
    estimated_bytes: usize,
    revision: SurfaceRevision,
) -> VirtualResourceDescriptor {
    VirtualResourceDescriptor {
        id: VirtualResourceId::from_layout_tile(layout, tile, resource_salt(kind)),
        node,
        tile,
        layout,
        kind,
        tier,
        bounds,
        estimated_bytes,
        revision_seen: revision,
    }
}

fn estimated_text_len(node: &VirtualNode) -> u32 {
    node.source_range
        .map(|range| range.len().min(u32::MAX as u64) as u32)
        .unwrap_or(64)
}

fn color_for_kind(kind: &VirtualNodeKind) -> [f32; 4] {
    match kind {
        VirtualNodeKind::Heading => [0.18, 0.22, 0.28, 1.0],
        VirtualNodeKind::CodeLine
        | VirtualNodeKind::CodeTile
        | VirtualNodeKind::CodeBlock => [0.09, 0.10, 0.12, 1.0],
        VirtualNodeKind::Table | VirtualNodeKind::TableTile => [0.12, 0.12, 0.10, 1.0],
        VirtualNodeKind::AgentMessage => [0.10, 0.13, 0.15, 1.0],
        VirtualNodeKind::ToolCard => [0.13, 0.11, 0.15, 1.0],
        VirtualNodeKind::DiffHunk => [0.10, 0.12, 0.10, 1.0],
        VirtualNodeKind::Image => [0.08, 0.08, 0.08, 1.0],
        VirtualNodeKind::Root
        | VirtualNodeKind::Text
        | VirtualNodeKind::MarkdownBlock
        | VirtualNodeKind::Overlay
        | VirtualNodeKind::Custom(_) => [0.11, 0.11, 0.12, 1.0],
    }
}

fn resource_salt(kind: VirtualResourceKind) -> u64 {
    match kind {
        VirtualResourceKind::CpuDrawList => 0x10,
        VirtualResourceKind::GpuDrawBuffer => 0x20,
        VirtualResourceKind::TextureTile => 0x30,
        VirtualResourceKind::HitRegion => 0x40,
    }
}
