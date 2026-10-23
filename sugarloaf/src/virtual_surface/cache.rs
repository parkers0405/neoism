use std::collections::{BTreeSet, HashMap, HashSet};

use lru::LruCache;

use serde::{Deserialize, Serialize};

use super::protocol::{LayoutKey, NodeId, SurfaceRevision, VirtualTileRange};

/// Residency tier for a retained node/chunk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheTier {
    /// Visible or actively edited. Should have GPU-ready draw data.
    Hot,
    /// Close to the viewport. CPU display list should be ready; GPU upload may
    /// be prepared opportunistically.
    Warm,
    /// Far from viewport. Keep compact layout/hash metadata only.
    #[default]
    Cold,
    /// Stable and eligible to be baked into a texture or long-lived GPU buffer.
    Frozen,
}

/// Policy knobs for GPU/cache residency. The runtime can tune these per
/// surface: agent history wants aggressive freezing, while an active code
/// editor keeps the hot band larger and live.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpuResidencyPolicy {
    pub hot_overscan_px: f32,
    pub warm_overscan_px: f32,
    pub freeze_after_revision_age: u64,
    pub max_hot_chunks: usize,
    pub max_warm_chunks: usize,
    pub allow_texture_baking: bool,
}

impl Default for GpuResidencyPolicy {
    fn default() -> Self {
        Self {
            hot_overscan_px: 1_200.0,
            warm_overscan_px: 6_000.0,
            freeze_after_revision_age: 2,
            max_hot_chunks: 2_048,
            max_warm_chunks: 16_384,
            allow_texture_baking: true,
        }
    }
}

/// Cached render artifact for a node. This does not directly own backend GPU
/// handles yet; it is the stable protocol shape that Vulkan/Metal/WebGPU
/// backends can attach resources to without changing adapters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetainedChunk {
    pub node: NodeId,
    pub key: LayoutKey,
    pub tier: CacheTier,
    pub revision_seen: SurfaceRevision,
    pub tile_range: VirtualTileRange,
    pub draw_command_count: u32,
    pub byte_size_estimate: usize,
    pub cpu_draw_ready: bool,
    pub hit_region_ready: bool,
    pub gpu_ready: bool,
    pub texture_backed: bool,
    pub gpu_ready_tiles: BTreeSet<u32>,
    pub texture_backed_tiles: BTreeSet<u32>,
}

impl RetainedChunk {
    pub fn new(node: NodeId, key: LayoutKey, revision_seen: SurfaceRevision) -> Self {
        Self {
            node,
            key,
            tier: CacheTier::Cold,
            revision_seen,
            tile_range: VirtualTileRange::WHOLE_NODE,
            draw_command_count: 0,
            byte_size_estimate: 0,
            cpu_draw_ready: false,
            hit_region_ready: false,
            gpu_ready: false,
            texture_backed: false,
            gpu_ready_tiles: BTreeSet::new(),
            texture_backed_tiles: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheStats {
    pub chunks: usize,
    pub hot: usize,
    pub warm: usize,
    pub cold: usize,
    pub frozen: usize,
    pub gpu_ready: usize,
    pub cpu_draw_ready: usize,
    pub hit_region_ready: usize,
    pub texture_backed: usize,
    pub estimated_bytes: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct RetainedChunkCache {
    chunks: HashMap<NodeId, RetainedChunk>,
    lru: LruCache<NodeId, ()>,
    policy: GpuResidencyPolicy,
}

impl RetainedChunkCache {
    pub fn new(policy: GpuResidencyPolicy) -> Self {
        Self {
            chunks: HashMap::new(),
            lru: LruCache::unbounded(),
            policy,
        }
    }

    pub fn policy(&self) -> GpuResidencyPolicy {
        self.policy
    }

    pub fn set_policy(&mut self, policy: GpuResidencyPolicy) -> Vec<RetainedChunk> {
        self.policy = policy;
        self.evict_to_budget()
    }

    pub fn get(&self, node: NodeId) -> Option<&RetainedChunk> {
        self.chunks.get(&node)
    }

    pub fn get_or_insert(
        &mut self,
        node: NodeId,
        key: LayoutKey,
        revision: SurfaceRevision,
    ) -> &mut RetainedChunk {
        self.touch(node);
        self.chunks
            .entry(node)
            .and_modify(|chunk| {
                if chunk.key != key {
                    *chunk = RetainedChunk::new(node, key, revision);
                }
            })
            .or_insert_with(|| RetainedChunk::new(node, key, revision))
    }

    pub fn remove(&mut self, node: NodeId) -> Option<RetainedChunk> {
        let removed = self.chunks.remove(&node);
        self.lru.pop(&node);
        removed
    }

    pub fn retain_nodes(&mut self, valid: &HashSet<NodeId>) -> Vec<RetainedChunk> {
        let removed_ids = self
            .chunks
            .keys()
            .copied()
            .filter(|id| !valid.contains(id))
            .collect::<Vec<_>>();
        let mut removed = Vec::with_capacity(removed_ids.len());
        for id in removed_ids {
            if let Some(chunk) = self.chunks.remove(&id) {
                removed.push(chunk);
            }
            self.lru.pop(&id);
        }
        removed
    }

    pub fn invalidate_gpu(&mut self, node: NodeId) {
        if let Some(chunk) = self.chunks.get_mut(&node) {
            chunk.gpu_ready = false;
            chunk.texture_backed = false;
            chunk.gpu_ready_tiles.clear();
            chunk.texture_backed_tiles.clear();
        }
    }

    pub fn invalidate_draw(&mut self, node: NodeId) {
        if let Some(chunk) = self.chunks.get_mut(&node) {
            chunk.cpu_draw_ready = false;
            chunk.hit_region_ready = false;
            chunk.gpu_ready = false;
            chunk.texture_backed = false;
            chunk.gpu_ready_tiles.clear();
            chunk.texture_backed_tiles.clear();
        }
    }

    pub fn stats(&self) -> CacheStats {
        let mut stats = CacheStats {
            chunks: self.chunks.len(),
            ..CacheStats::default()
        };
        for chunk in self.chunks.values() {
            match chunk.tier {
                CacheTier::Hot => stats.hot += 1,
                CacheTier::Warm => stats.warm += 1,
                CacheTier::Cold => stats.cold += 1,
                CacheTier::Frozen => stats.frozen += 1,
            }
            if chunk.gpu_ready {
                stats.gpu_ready += 1;
            }
            if chunk.cpu_draw_ready {
                stats.cpu_draw_ready += 1;
            }
            if chunk.hit_region_ready {
                stats.hit_region_ready += 1;
            }
            if chunk.texture_backed {
                stats.texture_backed += 1;
            }
            stats.estimated_bytes = stats
                .estimated_bytes
                .saturating_add(chunk.byte_size_estimate);
        }
        stats
    }

    pub fn evict_to_budget(&mut self) -> Vec<RetainedChunk> {
        let mut evicted = Vec::new();
        let max = self.policy.max_warm_chunks.max(self.policy.max_hot_chunks);
        let mut protected = 0usize;
        while self.chunks.len() > max {
            let Some((victim, ())) = self.lru.pop_lru() else {
                break;
            };
            if self
                .chunks
                .get(&victim)
                .is_some_and(|chunk| chunk.tier == CacheTier::Hot)
            {
                protected = protected.saturating_add(1);
                self.touch(victim);
                if protected >= self.chunks.len() {
                    break;
                }
                continue;
            }
            if let Some(chunk) = self.chunks.remove(&victim) {
                evicted.push(chunk);
            }
        }
        evicted
    }

    fn touch(&mut self, node: NodeId) {
        // Exactly one entry per resident chunk; repeated animated frames must
        // not accumulate stale stamps or trigger a later cleanup cliff.
        self.lru.put(node, ());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(max: usize) -> RetainedChunkCache {
        RetainedChunkCache::new(GpuResidencyPolicy {
            max_hot_chunks: max,
            max_warm_chunks: max,
            ..GpuResidencyPolicy::default()
        })
    }

    fn touch(cache: &mut RetainedChunkCache, node: u64) -> &mut RetainedChunk {
        cache.get_or_insert(NodeId(node), LayoutKey::default(), SurfaceRevision(1))
    }

    #[test]
    fn repeated_frames_keep_lru_bookkeeping_proportional_to_residency() {
        let mut cache = cache(8);
        for _ in 0..100_000 {
            for node in 0..8 {
                touch(&mut cache, node);
            }
        }
        assert_eq!(cache.chunks.len(), 8);
        assert_eq!(cache.lru.len(), 8);
    }

    #[test]
    fn eviction_uses_actual_recency_and_preserves_hot_chunks() {
        let mut cache = cache(2);
        touch(&mut cache, 1).tier = CacheTier::Hot;
        touch(&mut cache, 2);
        touch(&mut cache, 3);
        touch(&mut cache, 2);
        let evicted = cache.evict_to_budget();
        assert_eq!(evicted.iter().map(|chunk| chunk.node).collect::<Vec<_>>(), [NodeId(3)]);
        assert!(cache.get(NodeId(1)).is_some());
        assert!(cache.get(NodeId(2)).is_some());
        assert_eq!(cache.lru.len(), 2);
    }

    #[test]
    fn removal_and_reinsertion_leave_no_old_recency_records() {
        let mut cache = cache(2);
        touch(&mut cache, 1);
        touch(&mut cache, 2);
        cache.remove(NodeId(1));
        assert_eq!(cache.lru.len(), 1);
        touch(&mut cache, 1);
        touch(&mut cache, 3);
        assert_eq!(cache.evict_to_budget()[0].node, NodeId(2));
        cache.retain_nodes(&HashSet::from([NodeId(1)]));
        assert_eq!(cache.chunks.len(), 1);
        assert_eq!(cache.lru.len(), 1);
    }

    #[test]
    fn all_hot_chunks_stop_eviction_without_growing_bookkeeping() {
        let mut cache = cache(1);
        touch(&mut cache, 1).tier = CacheTier::Hot;
        touch(&mut cache, 2).tier = CacheTier::Hot;
        for _ in 0..10 {
            assert!(cache.evict_to_budget().is_empty());
            assert_eq!(cache.lru.len(), 2);
        }
    }
}
