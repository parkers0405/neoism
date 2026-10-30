use std::{collections::HashMap, fmt};

use super::adapter::VirtualSurfaceBatch;
use super::backend::VirtualSurfaceBackend;
use super::gpu::{VirtualGpuFrameBackend, VirtualGpuFramePacket, VirtualGpuPacketStats};
use super::protocol::{
    VirtualSurfaceCommand, VirtualSurfaceConfig, VirtualSurfaceError,
    VirtualSurfaceSnapshot,
};
use super::resource::{
    VirtualDeferredResourceStats, VirtualFrameSchedulePolicy, VirtualFrameTransaction,
    VirtualFrameTransactionStats,
};
use super::standard::{
    VirtualBatchApplyReport, VirtualSurfaceRoute, VirtualSurfaceRouteId,
    VirtualSurfaceWireEnvelope,
};
use super::surface::VirtualSurface;

use serde::{Deserialize, Serialize};

/// Multi-surface host for the virtual-surface protocol.
///
/// This keeps producer routing out of markdown/agent/code consumers. A future
/// desktop or wasm frontend can keep one router and let each panel/file/session
/// submit route-tagged batches into the same Sugarloaf standard.
#[derive(Clone, Debug)]
pub struct VirtualSurfaceRouter {
    config: VirtualSurfaceConfig,
    routes: HashMap<VirtualSurfaceRouteId, VirtualSurfaceRouteEntry>,
}

impl Default for VirtualSurfaceRouter {
    fn default() -> Self {
        Self::new(VirtualSurfaceConfig::default())
    }
}

impl VirtualSurfaceRouter {
    pub fn new(config: VirtualSurfaceConfig) -> Self {
        Self {
            config,
            routes: HashMap::new(),
        }
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn contains_route(&self, id: &VirtualSurfaceRouteId) -> bool {
        self.routes.contains_key(id)
    }

    pub fn ensure_route(&mut self, route: VirtualSurfaceRoute) -> &mut VirtualSurface {
        &mut self
            .routes
            .entry(route.id.clone())
            .or_insert_with(|| VirtualSurfaceRouteEntry {
                route,
                surface: VirtualSurface::new(self.config),
            })
            .surface
    }

    pub fn surface(&self, id: &VirtualSurfaceRouteId) -> Option<&VirtualSurface> {
        self.routes.get(id).map(|entry| &entry.surface)
    }

    pub fn surface_mut(
        &mut self,
        id: &VirtualSurfaceRouteId,
    ) -> Option<&mut VirtualSurface> {
        self.routes.get_mut(id).map(|entry| &mut entry.surface)
    }

    pub fn route(&self, id: &VirtualSurfaceRouteId) -> Option<&VirtualSurfaceRoute> {
        self.routes.get(id).map(|entry| &entry.route)
    }

    pub fn remove_route(
        &mut self,
        id: &VirtualSurfaceRouteId,
    ) -> Option<VirtualSurfaceRouteEntry> {
        self.routes.remove(id)
    }

    pub fn apply(
        &mut self,
        route: VirtualSurfaceRoute,
        command: VirtualSurfaceCommand,
    ) -> Result<(), VirtualSurfaceError> {
        self.ensure_route(route).apply(command)
    }

    pub fn apply_batch(
        &mut self,
        batch: VirtualSurfaceBatch,
    ) -> Result<VirtualBatchApplyReport, VirtualSurfaceError> {
        let surface = self.ensure_route(batch.route.clone());
        batch.apply_to(surface)
    }

    pub fn build_frame_transaction(
        &mut self,
        id: &VirtualSurfaceRouteId,
    ) -> Result<VirtualFrameTransaction, VirtualSurfaceRouterError> {
        let surface = self
            .surface_mut(id)
            .ok_or_else(|| VirtualSurfaceRouterError::MissingRoute(id.clone()))?;
        Ok(surface.build_frame_transaction())
    }

    pub fn build_gpu_frame_packet(
        &mut self,
        id: &VirtualSurfaceRouteId,
    ) -> Result<VirtualGpuFramePacket, VirtualSurfaceRouterError> {
        Ok(self.build_frame_transaction(id)?.build_gpu_packet())
    }

    pub fn build_gpu_frame_envelope(
        &mut self,
        id: &VirtualSurfaceRouteId,
        sequence: u64,
    ) -> Result<
        VirtualSurfaceWireEnvelope<VirtualGpuFramePacket>,
        VirtualSurfaceRouterError,
    > {
        let route = self
            .route(id)
            .cloned()
            .ok_or_else(|| VirtualSurfaceRouterError::MissingRoute(id.clone()))?;
        let packet = self.build_gpu_frame_packet(id)?;
        Ok(VirtualSurfaceWireEnvelope::new(route, packet).with_sequence(sequence))
    }

    pub fn build_all_gpu_frame_envelopes(
        &mut self,
        sequence_start: u64,
    ) -> Result<
        Vec<VirtualSurfaceWireEnvelope<VirtualGpuFramePacket>>,
        VirtualSurfaceRouterError,
    > {
        let mut ids = self.routes.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        let mut envelopes = Vec::with_capacity(ids.len());
        for (offset, id) in ids.iter().enumerate() {
            envelopes.push(self.build_gpu_frame_envelope(
                id,
                sequence_start.saturating_add(offset as u64),
            )?);
        }
        Ok(envelopes)
    }

    pub fn snapshot(
        &mut self,
        id: &VirtualSurfaceRouteId,
    ) -> Result<VirtualSurfaceSnapshot, VirtualSurfaceRouterError> {
        let surface = self
            .surface_mut(id)
            .ok_or_else(|| VirtualSurfaceRouterError::MissingRoute(id.clone()))?;
        Ok(surface.snapshot())
    }

    pub fn run_route_frame<B: VirtualSurfaceBackend>(
        &mut self,
        id: &VirtualSurfaceRouteId,
        backend: &mut B,
        schedule: VirtualFrameSchedulePolicy,
    ) -> Result<VirtualRouteFrameReport, VirtualSurfaceRouterError>
    where
        B::Error: fmt::Debug,
    {
        let surface = self
            .surface_mut(id)
            .ok_or_else(|| VirtualSurfaceRouterError::MissingRoute(id.clone()))?;
        let scheduled = surface.build_scheduled_frame_transaction(schedule);
        let original = scheduled.original_stats;
        let scheduled_stats = scheduled.transaction.stats();
        let gpu_stats = scheduled.transaction.build_gpu_packet().stats();
        let deferred = scheduled.deferred;
        let needs_another_frame = scheduled.has_deferred_work();
        let commit = backend
            .execute_frame(&scheduled.transaction)
            .map_err(|err| VirtualSurfaceRouterError::Backend(format!("{err:?}")))?;
        surface.commit_frame_transaction(&scheduled.transaction, &commit)?;
        Ok(VirtualRouteFrameReport {
            route: id.clone(),
            original,
            scheduled: scheduled_stats,
            deferred,
            gpu: gpu_stats,
            needs_another_frame,
        })
    }

    pub fn run_route_gpu_frame<B: VirtualGpuFrameBackend>(
        &mut self,
        id: &VirtualSurfaceRouteId,
        backend: &mut B,
        schedule: VirtualFrameSchedulePolicy,
    ) -> Result<VirtualRouteFrameReport, VirtualSurfaceRouterError>
    where
        B::Error: fmt::Debug,
    {
        let surface = self
            .surface_mut(id)
            .ok_or_else(|| VirtualSurfaceRouterError::MissingRoute(id.clone()))?;
        let scheduled = surface.build_scheduled_frame_transaction(schedule);
        let original = scheduled.original_stats;
        let scheduled_stats = scheduled.transaction.stats();
        let packet = scheduled.transaction.build_gpu_packet();
        let gpu_stats = packet.stats();
        let deferred = scheduled.deferred;
        let needs_another_frame = scheduled.has_deferred_work();
        let commit = backend
            .execute_gpu_frame(&packet)
            .map_err(|err| VirtualSurfaceRouterError::Backend(format!("{err:?}")))?;
        surface.commit_frame_transaction(&scheduled.transaction, &commit)?;
        Ok(VirtualRouteFrameReport {
            route: id.clone(),
            original,
            scheduled: scheduled_stats,
            deferred,
            gpu: gpu_stats,
            needs_another_frame,
        })
    }
}

/// State for one registered route.
#[derive(Clone, Debug)]
pub struct VirtualSurfaceRouteEntry {
    pub route: VirtualSurfaceRoute,
    pub surface: VirtualSurface,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VirtualSurfaceRouterError {
    MissingRoute(VirtualSurfaceRouteId),
    Surface(VirtualSurfaceError),
    Backend(String),
}

impl From<VirtualSurfaceError> for VirtualSurfaceRouterError {
    fn from(value: VirtualSurfaceError) -> Self {
        Self::Surface(value)
    }
}

impl fmt::Display for VirtualSurfaceRouterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRoute(route) => {
                write!(f, "missing virtual surface route {}", route.0)
            }
            Self::Surface(err) => write!(f, "{err}"),
            Self::Backend(err) => write!(f, "virtual surface backend error: {err}"),
        }
    }
}

impl std::error::Error for VirtualSurfaceRouterError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualRouteFrameReport {
    pub route: VirtualSurfaceRouteId,
    pub original: VirtualFrameTransactionStats,
    pub scheduled: VirtualFrameTransactionStats,
    pub deferred: VirtualDeferredResourceStats,
    pub gpu: VirtualGpuPacketStats,
    pub needs_another_frame: bool,
}
