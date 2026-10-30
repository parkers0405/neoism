use super::cache::CacheTier;
use super::protocol::{
    LayoutKey, NodeId, SurfaceRevision, VirtualBounds, VirtualFramePlan, VirtualScroll,
    VirtualTileId, VirtualViewport,
};
use super::render::VirtualDrawCommand;

use serde::{Deserialize, Serialize};

/// Stable backend resource id derived from a node + layout key.
///
/// Backends are free to map this to Vulkan buffers, Metal buffers, WebGPU
/// buffers, texture tiles, or CPU fallback allocations.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct VirtualResourceId(pub u64);

impl VirtualResourceId {
    pub fn from_layout(key: LayoutKey, salt: u64) -> Self {
        Self::from_layout_tile(key, None, salt)
    }

    pub fn from_layout_tile(
        key: LayoutKey,
        tile: Option<VirtualTileId>,
        salt: u64,
    ) -> Self {
        let mut hash = 0xcbf29ce484222325u64 ^ salt;
        hash = fnv_mix(hash, key.node.0);
        hash = fnv_mix(hash, key.revision.0);
        hash = fnv_mix(hash, key.width_bucket as u32 as u64);
        hash = fnv_mix(hash, key.scale_bucket as u32 as u64);
        hash = fnv_mix(hash, key.style.style_hash);
        hash = fnv_mix(hash, key.style.font_size_bucket as u64);
        hash = fnv_mix(hash, key.style.flags as u64);
        hash = fnv_mix(hash, key.text_hash);
        if let Some(tile) = tile {
            hash = fnv_mix(hash, tile.node.0);
            hash = fnv_mix(hash, tile.tile as u64);
        }
        Self(hash)
    }
}

fn fnv_mix(mut hash: u64, value: u64) -> u64 {
    hash ^= value;
    hash = hash.wrapping_mul(0x100000001b3);
    hash
}

/// Backend-neutral resource class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualResourceKind {
    CpuDrawList,
    GpuDrawBuffer,
    TextureTile,
    HitRegion,
}

/// Metadata about a retained backend resource.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualResourceDescriptor {
    pub id: VirtualResourceId,
    pub node: NodeId,
    pub tile: Option<VirtualTileId>,
    pub layout: LayoutKey,
    pub kind: VirtualResourceKind,
    pub tier: CacheTier,
    pub bounds: VirtualBounds,
    pub estimated_bytes: usize,
    pub revision_seen: SurfaceRevision,
}

/// Operation a backend should apply after a virtual frame is planned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VirtualResourceOp {
    Retain(VirtualResourceDescriptor),
    BuildDrawList(VirtualResourceDescriptor),
    BuildHitRegion(VirtualResourceDescriptor),
    UploadGpuBuffer(VirtualResourceDescriptor),
    BakeTexture(VirtualResourceDescriptor),
    Drop(VirtualResourceId),
}

impl VirtualResourceOp {
    pub fn id(&self) -> VirtualResourceId {
        match self {
            Self::Retain(descriptor)
            | Self::BuildDrawList(descriptor)
            | Self::BuildHitRegion(descriptor)
            | Self::UploadGpuBuffer(descriptor)
            | Self::BakeTexture(descriptor) => descriptor.id,
            Self::Drop(id) => *id,
        }
    }

    pub fn descriptor(&self) -> Option<VirtualResourceDescriptor> {
        match self {
            Self::Retain(descriptor)
            | Self::BuildDrawList(descriptor)
            | Self::BuildHitRegion(descriptor)
            | Self::UploadGpuBuffer(descriptor)
            | Self::BakeTexture(descriptor) => Some(*descriptor),
            Self::Drop(_) => None,
        }
    }
}

/// Backend-reported failure for an individual retained resource operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualResourceFailure {
    pub id: VirtualResourceId,
    pub message: String,
}

/// Backend acknowledgement for a frame transaction. A native backend should
/// return this after executing resource operations; test/probe paths can use
/// `VirtualFrameTransaction::successful_commit` to simulate a perfect backend.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualFrameCommit {
    pub revision: SurfaceRevision,
    pub ready: Vec<VirtualResourceId>,
    pub dropped: Vec<VirtualResourceId>,
    pub failed: Vec<VirtualResourceFailure>,
}

impl VirtualFrameCommit {
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Complete retained-surface frame product. The frontend can inspect this for
/// metrics, and backends can execute `resource_ops` before drawing `commands`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualFrameTransaction {
    pub revision: SurfaceRevision,
    pub viewport: VirtualViewport,
    pub scroll: VirtualScroll,
    pub plan: VirtualFramePlan,
    pub resource_ops: Vec<VirtualResourceOp>,
    pub commands: Vec<VirtualDrawCommand>,
    pub command_bytes: usize,
}

impl VirtualFrameTransaction {
    pub fn new(revision: SurfaceRevision, plan: VirtualFramePlan) -> Self {
        Self {
            revision,
            viewport: VirtualViewport::default(),
            scroll: VirtualScroll::default(),
            plan,
            resource_ops: Vec::new(),
            commands: Vec::new(),
            command_bytes: 0,
        }
    }

    pub fn with_viewport_scroll(
        mut self,
        viewport: VirtualViewport,
        scroll: VirtualScroll,
    ) -> Self {
        self.viewport = viewport;
        self.scroll = scroll;
        self
    }

    pub fn push_command(&mut self, command: VirtualDrawCommand) {
        self.command_bytes = self
            .command_bytes
            .saturating_add(command.byte_size_estimate());
        self.commands.push(command);
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

    pub fn stats(&self) -> VirtualFrameTransactionStats {
        let mut stats = VirtualFrameTransactionStats {
            visible_nodes: self.plan.visible.nodes.len(),
            damage_regions: self.plan.damage.len(),
            prefetch_hints: self.plan.prefetch.len(),
            draw_commands: self.commands.len(),
            command_bytes: self.command_bytes,
            ..VirtualFrameTransactionStats::default()
        };
        for op in &self.resource_ops {
            match op {
                VirtualResourceOp::Retain(_) => stats.retain += 1,
                VirtualResourceOp::BuildDrawList(_) => stats.build_draw_list += 1,
                VirtualResourceOp::BuildHitRegion(_) => stats.build_hit_region += 1,
                VirtualResourceOp::UploadGpuBuffer(_) => stats.upload_gpu_buffer += 1,
                VirtualResourceOp::BakeTexture(_) => stats.bake_texture += 1,
                VirtualResourceOp::Drop(_) => stats.drop += 1,
            }
        }
        stats
    }

    /// Return a transaction whose expensive backend work is bounded for this
    /// frame. Retain/drop/CPU draw-list/hit-region ops stay immediate; GPU
    /// uploads and texture bakes are prioritized by cache tier and budgeted.
    pub fn scheduled(
        &self,
        policy: VirtualFrameSchedulePolicy,
    ) -> VirtualScheduledFrameTransaction {
        let mut scheduled = VirtualFrameTransaction {
            revision: self.revision,
            viewport: self.viewport,
            scroll: self.scroll,
            plan: self.plan.clone(),
            commands: self.commands.clone(),
            command_bytes: self.command_bytes,
            resource_ops: Vec::with_capacity(self.resource_ops.len()),
        };
        let mut deferred = VirtualDeferredResourceStats::default();
        let mut deferrable = Vec::new();

        for op in &self.resource_ops {
            match op {
                VirtualResourceOp::Retain(_)
                | VirtualResourceOp::BuildDrawList(_)
                | VirtualResourceOp::BuildHitRegion(_)
                | VirtualResourceOp::Drop(_) => scheduled.resource_ops.push(op.clone()),
                VirtualResourceOp::UploadGpuBuffer(_)
                | VirtualResourceOp::BakeTexture(_) => deferrable.push(op.clone()),
            }
        }

        deferrable.sort_by_key(|op| {
            let descriptor = op.descriptor();
            (
                descriptor
                    .map(|descriptor| tier_priority(descriptor.tier))
                    .unwrap_or(usize::MAX),
                op_priority(op),
                descriptor
                    .map(|descriptor| descriptor.estimated_bytes)
                    .unwrap_or(usize::MAX),
            )
        });

        let mut upload_ops = 0usize;
        let mut upload_bytes = 0usize;
        let mut bake_ops = 0usize;
        let mut bake_bytes = 0usize;

        for op in deferrable {
            match op {
                VirtualResourceOp::UploadGpuBuffer(descriptor) => {
                    let next_bytes =
                        upload_bytes.saturating_add(descriptor.estimated_bytes);
                    if upload_ops < policy.max_upload_ops
                        && next_bytes <= policy.max_upload_bytes
                    {
                        upload_ops += 1;
                        upload_bytes = next_bytes;
                        scheduled
                            .resource_ops
                            .push(VirtualResourceOp::UploadGpuBuffer(descriptor));
                    } else {
                        deferred.upload_gpu_buffer += 1;
                        deferred.upload_bytes = deferred
                            .upload_bytes
                            .saturating_add(descriptor.estimated_bytes);
                    }
                }
                VirtualResourceOp::BakeTexture(descriptor) => {
                    let next_bytes =
                        bake_bytes.saturating_add(descriptor.estimated_bytes);
                    if policy.allow_texture_baking
                        && bake_ops < policy.max_bake_ops
                        && next_bytes <= policy.max_bake_bytes
                    {
                        bake_ops += 1;
                        bake_bytes = next_bytes;
                        scheduled
                            .resource_ops
                            .push(VirtualResourceOp::BakeTexture(descriptor));
                    } else {
                        deferred.bake_texture += 1;
                        deferred.bake_bytes = deferred
                            .bake_bytes
                            .saturating_add(descriptor.estimated_bytes);
                    }
                }
                VirtualResourceOp::Retain(_)
                | VirtualResourceOp::BuildDrawList(_)
                | VirtualResourceOp::BuildHitRegion(_)
                | VirtualResourceOp::Drop(_) => unreachable!(),
            }
        }

        VirtualScheduledFrameTransaction {
            transaction: scheduled,
            deferred,
            original_stats: self.stats(),
        }
    }
}

/// Compact counters for inspecting transaction shape without walking all ops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualFrameTransactionStats {
    pub visible_nodes: usize,
    pub damage_regions: usize,
    pub prefetch_hints: usize,
    pub draw_commands: usize,
    pub command_bytes: usize,
    pub retain: usize,
    pub build_draw_list: usize,
    pub build_hit_region: usize,
    pub upload_gpu_buffer: usize,
    pub bake_texture: usize,
    pub drop: usize,
}

/// Per-frame scheduling budget for expensive backend work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualFrameSchedulePolicy {
    pub max_upload_ops: usize,
    pub max_upload_bytes: usize,
    pub max_bake_ops: usize,
    pub max_bake_bytes: usize,
    pub allow_texture_baking: bool,
}

impl Default for VirtualFrameSchedulePolicy {
    fn default() -> Self {
        Self {
            max_upload_ops: 512,
            max_upload_bytes: 32 * 1024 * 1024,
            max_bake_ops: 64,
            max_bake_bytes: 64 * 1024 * 1024,
            allow_texture_baking: true,
        }
    }
}

/// Resource work intentionally delayed by frame scheduling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualDeferredResourceStats {
    pub upload_gpu_buffer: usize,
    pub upload_bytes: usize,
    pub bake_texture: usize,
    pub bake_bytes: usize,
}

/// A scheduled transaction plus the work left for later frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualScheduledFrameTransaction {
    pub transaction: VirtualFrameTransaction,
    pub deferred: VirtualDeferredResourceStats,
    pub original_stats: VirtualFrameTransactionStats,
}

impl VirtualScheduledFrameTransaction {
    pub fn has_deferred_work(&self) -> bool {
        self.deferred.upload_gpu_buffer > 0 || self.deferred.bake_texture > 0
    }
}

fn tier_priority(tier: CacheTier) -> usize {
    match tier {
        CacheTier::Hot => 0,
        CacheTier::Warm => 1,
        CacheTier::Cold => 2,
        CacheTier::Frozen => 3,
    }
}

fn op_priority(op: &VirtualResourceOp) -> usize {
    match op {
        VirtualResourceOp::UploadGpuBuffer(_) => 0,
        VirtualResourceOp::BakeTexture(_) => 1,
        VirtualResourceOp::Retain(_)
        | VirtualResourceOp::BuildDrawList(_)
        | VirtualResourceOp::BuildHitRegion(_)
        | VirtualResourceOp::Drop(_) => 2,
    }
}
