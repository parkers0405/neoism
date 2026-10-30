use std::collections::{BTreeMap, BTreeSet};

use super::backend::AcceptAllVirtualSurfaceBackend;
use super::protocol::{
    NodeId, NodeSource, NodeSourceRange, SurfaceRevision, VirtualBounds,
    VirtualContentId, VirtualContentRef, VirtualDamage, VirtualNodeKind,
    VirtualPrefetchHint, VirtualScroll, VirtualTextMeasurementRequest, VirtualTextPlan,
    VirtualTileId, VirtualTileRange, VirtualViewport,
};
use super::render::VirtualDrawCommand;
use super::resource::{VirtualFrameCommit, VirtualFrameTransaction, VirtualResourceOp};

use serde::{Deserialize, Serialize};

/// Stable layer buckets for the virtual GPU packet.
///
/// A backend can map these directly to render passes, indirect draw buckets, or
/// WebGPU/Metal/Vulkan pipelines without knowing whether the producer was a
/// markdown file, model response, agent transcript, or code buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualGpuLayer {
    Background,
    #[default]
    Content,
    Code,
    Table,
    Agent,
    Overlay,
}

/// Primitive family inside the GPU packet. This is intentionally compact:
/// every richer thing starts as one of these instance streams, then the backend
/// can specialize text shaping, atlas lookup, texture baking, and hit regions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualGpuPrimitive {
    #[default]
    SolidQuad,
    TextRun,
    TextureTile,
}

/// One backend-neutral GPU instance. It is small enough to become a storage
/// buffer row and rich enough to keep source-backed content out of per-frame
/// text copies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualGpuInstance {
    pub node: NodeId,
    pub tile: Option<VirtualTileId>,
    pub layer: VirtualGpuLayer,
    pub primitive: VirtualGpuPrimitive,
    pub bounds: VirtualBounds,
    pub clip: VirtualBounds,
    pub origin: [f32; 2],
    pub color: [f32; 4],
    pub text_hash: u64,
    pub content: Option<VirtualContentId>,
}

/// Consecutive instance range that can be submitted as one indirect draw call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualGpuDrawBatch {
    pub layer: VirtualGpuLayer,
    pub primitive: VirtualGpuPrimitive,
    pub first_instance: u32,
    pub instance_count: u32,
}

/// Content the backend should make available for shaping/rasterization.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualGpuContentRequest {
    pub content: VirtualContentRef,
    pub text_plan: Option<VirtualTextPlan>,
}

/// Coalesced source window represented by visible or warm-nearby content.
///
/// This lets native editor/markdown/model integrations map a GPU packet back to
/// source bytes and lines without walking every retained node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualGpuSourceWindow {
    pub source: NodeSource,
    pub range: NodeSourceRange,
    pub line_start: u64,
    pub line_end: u64,
    pub content_count: u32,
    pub prefetch: bool,
}

/// Immutable context for a GPU packet. A backend should be able to schedule,
/// clip, and prefetch from this packet without reaching back into `VirtualSurface`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualGpuFrameContext {
    pub revision: SurfaceRevision,
    pub viewport: VirtualViewport,
    pub scroll: VirtualScroll,
    pub visible_start: usize,
    pub visible_end: usize,
    pub query_top: f32,
    pub query_bottom: f32,
    pub content_height: f32,
}

/// Backend-neutral frame packet for the eventual native Sugarloaf renderer.
///
/// This is the compact handoff that keeps huge documents fast: visible draw
/// instances, retained resource operations, and source-backed content refs are
/// all separated from the full markdown/code/agent text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualGpuFramePacket {
    pub revision: SurfaceRevision,
    pub context: VirtualGpuFrameContext,
    pub visible_nodes: usize,
    pub damage: Vec<VirtualDamage>,
    pub prefetch: Vec<VirtualPrefetchHint>,
    pub resource_ops: Vec<VirtualResourceOp>,
    pub instances: Vec<VirtualGpuInstance>,
    pub draw_batches: Vec<VirtualGpuDrawBatch>,
    pub content_requests: Vec<VirtualGpuContentRequest>,
    pub prefetch_content_requests: Vec<VirtualGpuContentRequest>,
    pub source_windows: Vec<VirtualGpuSourceWindow>,
    pub measurement_requests: Vec<VirtualTextMeasurementRequest>,
    pub command_bytes: usize,
    pub instance_bytes: usize,
}

impl VirtualGpuFramePacket {
    pub fn from_transaction(transaction: &VirtualFrameTransaction) -> Self {
        Self::from_transaction_with_content_prefetch_policy(
            transaction,
            VirtualGpuContentPrefetchPolicy::default(),
        )
    }

    pub fn from_transaction_with_content_prefetch_policy(
        transaction: &VirtualFrameTransaction,
        content_prefetch_policy: VirtualGpuContentPrefetchPolicy,
    ) -> Self {
        let visible_start = transaction
            .plan
            .visible
            .nodes
            .first()
            .map(|node| node.index)
            .unwrap_or(0);
        let visible_end = transaction
            .plan
            .visible
            .nodes
            .last()
            .map(|node| node.index.saturating_add(1))
            .unwrap_or(visible_start);
        let mut packet = Self {
            revision: transaction.revision,
            context: VirtualGpuFrameContext {
                revision: transaction.revision,
                viewport: transaction.viewport,
                scroll: transaction.scroll,
                visible_start,
                visible_end,
                query_top: transaction.plan.visible.query_top,
                query_bottom: transaction.plan.visible.query_bottom,
                content_height: transaction.plan.visible.content_height,
            },
            visible_nodes: transaction.plan.visible.nodes.len(),
            damage: transaction.plan.damage.clone(),
            prefetch: transaction.plan.prefetch.clone(),
            resource_ops: transaction.resource_ops.clone(),
            command_bytes: transaction.command_bytes,
            ..Self::default()
        };
        let mut content_requests = BTreeMap::<VirtualContentId, VirtualContentRef>::new();
        let mut text_plans = BTreeMap::<VirtualContentId, VirtualTextPlan>::new();
        let mut measurement_requests =
            BTreeMap::<VirtualContentId, VirtualTextMeasurementRequest>::new();
        let mut current_node = NodeId::ROOT;
        let mut current_kind = VirtualNodeKind::Root;
        let mut current_bounds = VirtualBounds::default();
        let mut current_clip = VirtualBounds::default();

        for command in &transaction.commands {
            match command {
                VirtualDrawCommand::BeginNode {
                    node,
                    kind,
                    bounds,
                    clip,
                } => {
                    current_node = *node;
                    current_kind = kind.clone();
                    current_bounds = *bounds;
                    current_clip = *clip;
                }
                VirtualDrawCommand::Rect {
                    node,
                    bounds,
                    color,
                } => {
                    packet.push_instance(VirtualGpuInstance {
                        node: *node,
                        tile: None,
                        layer: layer_for_kind(&current_kind),
                        primitive: VirtualGpuPrimitive::SolidQuad,
                        bounds: *bounds,
                        clip: current_clip,
                        origin: [bounds.x, bounds.y],
                        color: *color,
                        text_hash: 0,
                        content: None,
                    });
                }
                VirtualDrawCommand::TextRun {
                    node,
                    x,
                    y,
                    content,
                    text_plan,
                    text_hash,
                    ..
                } => {
                    if let Some(content) = content {
                        content_requests
                            .entry(content.id)
                            .or_insert_with(|| content.clone());
                        if let Some(text_plan) = text_plan {
                            text_plans
                                .entry(content.id)
                                .or_insert_with(|| text_plan.clone());
                        }
                        let request = VirtualTextMeasurementRequest::new(
                            *node,
                            content.revision,
                            content.clone(),
                            current_bounds.width,
                            transaction.viewport.scale,
                            VirtualTileRange::WHOLE_NODE,
                        );
                        measurement_requests.entry(content.id).or_insert_with(|| {
                            text_plan
                                .clone()
                                .map(|plan| request.clone().with_text_plan(plan))
                                .unwrap_or(request)
                        });
                    }
                    packet.push_instance(VirtualGpuInstance {
                        node: *node,
                        tile: None,
                        layer: layer_for_kind(&current_kind),
                        primitive: VirtualGpuPrimitive::TextRun,
                        bounds: current_bounds,
                        clip: current_clip,
                        origin: [*x, *y],
                        color: text_color_for_kind(&current_kind),
                        text_hash: *text_hash,
                        content: content.as_ref().map(|content| content.id),
                    });
                }
                VirtualDrawCommand::EndNode { node } => {
                    if *node == current_node {
                        current_node = NodeId::ROOT;
                        current_kind = VirtualNodeKind::Root;
                        current_bounds = VirtualBounds::default();
                        current_clip = VirtualBounds::default();
                    }
                }
            }
        }

        packet.content_requests = content_requests
            .into_iter()
            .map(|(id, content)| VirtualGpuContentRequest {
                content,
                text_plan: text_plans.remove(&id),
            })
            .collect();
        let visible_content_ids = packet
            .content_requests
            .iter()
            .map(|request| request.content.id)
            .collect::<BTreeSet<_>>();
        let mut prefetch_candidates = Vec::new();
        for hint in &packet.prefetch {
            let Some(content) = hint.content.as_ref() else {
                continue;
            };
            if visible_content_ids.contains(&content.id) {
                continue;
            }
            prefetch_candidates.push((
                hint.priority,
                hint.index,
                VirtualGpuContentRequest {
                    content: content.clone(),
                    text_plan: hint.text_plan.clone(),
                },
            ));
        }
        prefetch_candidates.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.content.id.cmp(&right.2.content.id))
        });
        let mut seen_prefetch_content = BTreeSet::new();
        let mut prefetch_bytes = 0u64;
        for (_, _, request) in prefetch_candidates {
            if packet.prefetch_content_requests.len()
                >= content_prefetch_policy.max_requests
            {
                break;
            }
            if !seen_prefetch_content.insert(request.content.id) {
                continue;
            }
            let next_bytes = prefetch_bytes.saturating_add(request.content.byte_len);
            if !packet.prefetch_content_requests.is_empty()
                && next_bytes > content_prefetch_policy.max_bytes
            {
                continue;
            }
            prefetch_bytes = next_bytes;
            packet.prefetch_content_requests.push(request);
        }
        packet.source_windows =
            source_windows_for_requests(&packet.content_requests, false);
        packet.source_windows.extend(source_windows_for_requests(
            &packet.prefetch_content_requests,
            true,
        ));
        packet.measurement_requests = measurement_requests.into_values().collect();
        packet
    }

    pub fn stats(&self) -> VirtualGpuPacketStats {
        let mut stats = VirtualGpuPacketStats {
            visible_nodes: self.visible_nodes,
            resource_ops: self.resource_ops.len(),
            draw_batches: self.draw_batches.len(),
            content_requests: self.content_requests.len(),
            prefetch_content_requests: self.prefetch_content_requests.len(),
            prefetch_content_bytes: self
                .prefetch_content_requests
                .iter()
                .map(|request| request.content.byte_len)
                .sum(),
            source_windows: self
                .source_windows
                .iter()
                .filter(|window| !window.prefetch)
                .count(),
            prefetch_source_windows: self
                .source_windows
                .iter()
                .filter(|window| window.prefetch)
                .count(),
            source_window_bytes: self
                .source_windows
                .iter()
                .map(|window| window.range.len())
                .sum(),
            measurement_requests: self.measurement_requests.len(),
            text_spans: self
                .content_requests
                .iter()
                .filter_map(|request| request.text_plan.as_ref())
                .map(|plan| plan.spans.len())
                .sum(),
            text_overlays: self
                .content_requests
                .iter()
                .filter_map(|request| request.text_plan.as_ref())
                .map(|plan| plan.overlays.len())
                .sum(),
            damage_regions: self.damage.len(),
            prefetch_hints: self.prefetch.len(),
            command_bytes: self.command_bytes,
            instance_bytes: self.instance_bytes,
            ..VirtualGpuPacketStats::default()
        };
        for instance in &self.instances {
            match instance.primitive {
                VirtualGpuPrimitive::SolidQuad => stats.solid_quads += 1,
                VirtualGpuPrimitive::TextRun => stats.text_runs += 1,
                VirtualGpuPrimitive::TextureTile => stats.texture_tiles += 1,
            }
        }
        stats.instances = self.instances.len();
        stats
    }

    pub fn validate(&self) -> Result<(), VirtualGpuFramePacketError> {
        if self.context.revision != self.revision {
            return Err(VirtualGpuFramePacketError::ContextRevisionMismatch {
                packet: self.revision,
                context: self.context.revision,
            });
        }

        let mut covered_instances = 0usize;
        for batch in &self.draw_batches {
            if batch.instance_count == 0 {
                return Err(VirtualGpuFramePacketError::EmptyDrawBatch);
            }
            let start = batch.first_instance as usize;
            let end = start.saturating_add(batch.instance_count as usize);
            if end > self.instances.len() {
                return Err(VirtualGpuFramePacketError::DrawBatchOutOfBounds {
                    first_instance: batch.first_instance,
                    instance_count: batch.instance_count,
                    instances: self.instances.len(),
                });
            }
            for instance in &self.instances[start..end] {
                if instance.layer != batch.layer || instance.primitive != batch.primitive
                {
                    return Err(VirtualGpuFramePacketError::DrawBatchKindMismatch {
                        first_instance: batch.first_instance,
                    });
                }
            }
            covered_instances =
                covered_instances.saturating_add(batch.instance_count as usize);
        }
        if covered_instances != self.instances.len() {
            return Err(VirtualGpuFramePacketError::DrawBatchCoverageMismatch {
                covered_instances,
                instances: self.instances.len(),
            });
        }

        let mut requested_content = BTreeSet::new();
        for request in &self.content_requests {
            if !requested_content.insert(request.content.id) {
                return Err(VirtualGpuFramePacketError::DuplicateContentRequest {
                    content: request.content.id,
                });
            }
        }
        let mut requested_prefetch_content = BTreeSet::new();
        for request in &self.prefetch_content_requests {
            if requested_content.contains(&request.content.id)
                || !requested_prefetch_content.insert(request.content.id)
            {
                return Err(
                    VirtualGpuFramePacketError::DuplicatePrefetchContentRequest {
                        content: request.content.id,
                    },
                );
            }
        }
        for instance in &self.instances {
            if instance.primitive == VirtualGpuPrimitive::TextRun {
                let Some(content) = instance.content else {
                    continue;
                };
                if !requested_content.contains(&content) {
                    return Err(VirtualGpuFramePacketError::MissingContentRequest {
                        content,
                    });
                }
            }
        }
        for window in &self.source_windows {
            if window.content_count == 0 || window.line_end < window.line_start {
                return Err(VirtualGpuFramePacketError::InvalidSourceWindow);
            }
        }

        Ok(())
    }

    pub fn successful_commit(&self) -> VirtualFrameCommit {
        let mut commit = VirtualFrameCommit {
            revision: self.revision,
            ..VirtualFrameCommit::default()
        };
        for op in &self.resource_ops {
            match op {
                VirtualResourceOp::Drop(id) => commit.dropped.push(*id),
                VirtualResourceOp::Retain(descriptor)
                | VirtualResourceOp::BuildDrawList(descriptor)
                | VirtualResourceOp::BuildHitRegion(descriptor)
                | VirtualResourceOp::UploadGpuBuffer(descriptor)
                | VirtualResourceOp::BakeTexture(descriptor) => {
                    commit.ready.push(descriptor.id);
                }
            }
        }
        commit
    }

    fn push_instance(&mut self, instance: VirtualGpuInstance) {
        self.instance_bytes = self
            .instance_bytes
            .saturating_add(std::mem::size_of::<VirtualGpuInstance>());
        if let Some(batch) = self.draw_batches.last_mut() {
            if batch.layer == instance.layer && batch.primitive == instance.primitive {
                batch.instance_count = batch.instance_count.saturating_add(1);
                self.instances.push(instance);
                return;
            }
        }
        self.draw_batches.push(VirtualGpuDrawBatch {
            layer: instance.layer,
            primitive: instance.primitive,
            first_instance: self.instances.len().min(u32::MAX as usize) as u32,
            instance_count: 1,
        });
        self.instances.push(instance);
    }
}

impl VirtualFrameTransaction {
    pub fn build_gpu_packet(&self) -> VirtualGpuFramePacket {
        VirtualGpuFramePacket::from_transaction(self)
    }
}

/// Backend contract for consumers that want the compact GPU packet instead of
/// the fuller planning transaction.
pub trait VirtualGpuFrameBackend {
    type Error;

    fn execute_gpu_frame(
        &mut self,
        packet: &VirtualGpuFramePacket,
    ) -> Result<VirtualFrameCommit, Self::Error>;
}

impl VirtualGpuFrameBackend for AcceptAllVirtualSurfaceBackend {
    type Error = std::convert::Infallible;

    fn execute_gpu_frame(
        &mut self,
        packet: &VirtualGpuFramePacket,
    ) -> Result<VirtualFrameCommit, Self::Error> {
        Ok(packet.successful_commit())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualGpuPacketStats {
    pub visible_nodes: usize,
    pub resource_ops: usize,
    pub instances: usize,
    pub solid_quads: usize,
    pub text_runs: usize,
    pub texture_tiles: usize,
    pub draw_batches: usize,
    pub content_requests: usize,
    pub prefetch_content_requests: usize,
    pub prefetch_content_bytes: u64,
    pub source_windows: usize,
    pub prefetch_source_windows: usize,
    pub source_window_bytes: u64,
    pub measurement_requests: usize,
    pub text_spans: usize,
    pub text_overlays: usize,
    pub damage_regions: usize,
    pub prefetch_hints: usize,
    pub command_bytes: usize,
    pub instance_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualGpuContentPrefetchPolicy {
    pub max_requests: usize,
    pub max_bytes: u64,
}

impl Default for VirtualGpuContentPrefetchPolicy {
    fn default() -> Self {
        Self {
            max_requests: 8,
            max_bytes: 256 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualGpuFramePacketError {
    ContextRevisionMismatch {
        packet: SurfaceRevision,
        context: SurfaceRevision,
    },
    EmptyDrawBatch,
    DrawBatchOutOfBounds {
        first_instance: u32,
        instance_count: u32,
        instances: usize,
    },
    DrawBatchKindMismatch {
        first_instance: u32,
    },
    DrawBatchCoverageMismatch {
        covered_instances: usize,
        instances: usize,
    },
    DuplicateContentRequest {
        content: VirtualContentId,
    },
    DuplicatePrefetchContentRequest {
        content: VirtualContentId,
    },
    MissingContentRequest {
        content: VirtualContentId,
    },
    InvalidSourceWindow,
}

fn source_windows_for_requests(
    requests: &[VirtualGpuContentRequest],
    prefetch: bool,
) -> Vec<VirtualGpuSourceWindow> {
    let mut windows = Vec::<VirtualGpuSourceWindow>::new();
    for request in requests {
        push_source_window(&mut windows, &request.content, prefetch);
    }
    windows
}

fn push_source_window(
    windows: &mut Vec<VirtualGpuSourceWindow>,
    content: &VirtualContentRef,
    prefetch: bool,
) {
    for window in windows.iter_mut() {
        if window.prefetch == prefetch
            && window.source == content.source
            && ranges_touch(window.range, content.range)
        {
            window.range = NodeSourceRange::new(
                window.range.start.min(content.range.start),
                window.range.end.max(content.range.end),
            );
            window.line_start = window.line_start.min(content.line_start);
            window.line_end = window.line_end.max(content.line_end());
            window.content_count = window.content_count.saturating_add(1);
            return;
        }
    }
    windows.push(VirtualGpuSourceWindow {
        source: content.source.clone(),
        range: content.range,
        line_start: content.line_start,
        line_end: content.line_end(),
        content_count: 1,
        prefetch,
    });
}

fn ranges_touch(left: NodeSourceRange, right: NodeSourceRange) -> bool {
    left.start <= right.end && right.start <= left.end
}

fn layer_for_kind(kind: &VirtualNodeKind) -> VirtualGpuLayer {
    match kind {
        VirtualNodeKind::CodeLine
        | VirtualNodeKind::CodeTile
        | VirtualNodeKind::CodeBlock => VirtualGpuLayer::Code,
        VirtualNodeKind::Table | VirtualNodeKind::TableTile => VirtualGpuLayer::Table,
        VirtualNodeKind::AgentMessage | VirtualNodeKind::ToolCard => {
            VirtualGpuLayer::Agent
        }
        VirtualNodeKind::Overlay => VirtualGpuLayer::Overlay,
        VirtualNodeKind::Root => VirtualGpuLayer::Background,
        _ => VirtualGpuLayer::Content,
    }
}

fn text_color_for_kind(kind: &VirtualNodeKind) -> [f32; 4] {
    match kind {
        VirtualNodeKind::CodeLine
        | VirtualNodeKind::CodeTile
        | VirtualNodeKind::CodeBlock => [0.82, 0.88, 0.92, 1.0],
        VirtualNodeKind::Heading => [0.96, 0.98, 1.0, 1.0],
        VirtualNodeKind::ToolCard => [0.78, 0.86, 0.95, 1.0],
        _ => [0.88, 0.91, 0.94, 1.0],
    }
}
