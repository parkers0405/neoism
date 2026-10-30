use super::adapter::VirtualSurfaceBatch;
use super::backend::VirtualSurfaceBackend;
use super::gpu::{VirtualGpuFrameBackend, VirtualGpuFramePacket, VirtualGpuPacketStats};
use super::protocol::{
    SurfaceMetrics, SurfaceRevision, VirtualScrollAnchor, VirtualSurfaceCommand,
    VirtualSurfaceError,
};
use super::resource::{
    VirtualDeferredResourceStats, VirtualFrameSchedulePolicy,
    VirtualFrameTransactionStats,
};
use super::standard::{
    VirtualBatchApplyReport, VirtualSurfaceFrameRequest,
    VirtualSurfaceProtocolCapabilities,
};
use super::surface::VirtualSurface;

use serde::{Deserialize, Serialize};

/// Error from the high-level virtual-surface pipeline.
#[derive(Debug)]
pub enum VirtualSurfacePipelineError<E> {
    Surface(VirtualSurfaceError),
    Backend(E),
}

impl<E> From<VirtualSurfaceError> for VirtualSurfacePipelineError<E> {
    fn from(value: VirtualSurfaceError) -> Self {
        Self::Surface(value)
    }
}

/// Summary returned after one scheduled frame is sent through a backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualFrameRunReport {
    pub revision: SurfaceRevision,
    pub original: VirtualFrameTransactionStats,
    pub scheduled: VirtualFrameTransactionStats,
    pub deferred: VirtualDeferredResourceStats,
    pub gpu: VirtualGpuPacketStats,
    pub needs_another_frame: bool,
}

/// Combined result for a route-aware request that may apply updates and render.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualSurfaceRequestReport {
    pub batch: Option<VirtualBatchApplyReport>,
    pub frame: VirtualFrameRunReport,
}

/// Owned driver for the retained virtual-surface protocol.
///
/// It is intentionally small: adapters still produce batches/commands, the
/// backend still owns native resources, and this type coordinates update,
/// scheduling, backend execution, commit, and scroll-anchor preservation.
#[derive(Clone, Debug)]
pub struct VirtualSurfacePipeline<B> {
    surface: VirtualSurface,
    backend: B,
    schedule: VirtualFrameSchedulePolicy,
}

impl<B> VirtualSurfacePipeline<B> {
    pub fn new(
        surface: VirtualSurface,
        backend: B,
        schedule: VirtualFrameSchedulePolicy,
    ) -> Self {
        Self {
            surface,
            backend,
            schedule,
        }
    }

    pub fn surface(&self) -> &VirtualSurface {
        &self.surface
    }

    pub fn surface_mut(&mut self) -> &mut VirtualSurface {
        &mut self.surface
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn schedule(&self) -> VirtualFrameSchedulePolicy {
        self.schedule
    }

    pub fn set_schedule(&mut self, schedule: VirtualFrameSchedulePolicy) {
        self.schedule = schedule;
    }

    pub fn metrics(&self) -> SurfaceMetrics {
        self.surface.metrics()
    }

    pub fn protocol_capabilities(
        &self,
        backend_capabilities: super::backend::VirtualSurfaceBackendCapabilities,
    ) -> VirtualSurfaceProtocolCapabilities {
        VirtualSurfaceProtocolCapabilities::from_config(
            self.surface.config(),
            backend_capabilities,
        )
    }

    pub fn apply(
        &mut self,
        command: VirtualSurfaceCommand,
    ) -> Result<(), VirtualSurfaceError> {
        self.surface.apply(command)
    }

    pub fn apply_batch(
        &mut self,
        batch: VirtualSurfaceBatch,
    ) -> Result<VirtualBatchApplyReport, VirtualSurfaceError> {
        batch.apply_to(&mut self.surface)
    }

    pub fn apply_preserving_anchor(
        &mut self,
        anchor_viewport_y: f32,
        command: VirtualSurfaceCommand,
    ) -> Result<Option<VirtualScrollAnchor>, VirtualSurfaceError> {
        let anchor = self.surface.capture_scroll_anchor(anchor_viewport_y);
        self.surface.apply(command)?;
        if let Some(anchor) = anchor {
            self.surface.restore_scroll_anchor(anchor)?;
            Ok(Some(anchor))
        } else {
            Ok(None)
        }
    }
}

impl<B: VirtualSurfaceBackend> VirtualSurfacePipeline<B> {
    pub fn capabilities(&self) -> VirtualSurfaceProtocolCapabilities {
        self.protocol_capabilities(self.backend.capabilities())
    }

    pub fn run_frame(
        &mut self,
    ) -> Result<VirtualFrameRunReport, VirtualSurfacePipelineError<B::Error>> {
        let scheduled = self
            .surface
            .build_scheduled_frame_transaction(self.schedule);
        let original_stats = scheduled.original_stats;
        let scheduled_stats = scheduled.transaction.stats();
        let gpu_stats = scheduled.transaction.build_gpu_packet().stats();
        let deferred = scheduled.deferred;
        let needs_another_frame = scheduled.has_deferred_work();
        let revision = scheduled.transaction.revision;
        let commit = self
            .backend
            .execute_frame(&scheduled.transaction)
            .map_err(VirtualSurfacePipelineError::Backend)?;
        self.surface
            .commit_frame_transaction(&scheduled.transaction, &commit)
            .map_err(VirtualSurfacePipelineError::Surface)?;
        Ok(VirtualFrameRunReport {
            revision,
            original: original_stats,
            scheduled: scheduled_stats,
            deferred,
            gpu: gpu_stats,
            needs_another_frame,
        })
    }

    pub fn run_request(
        &mut self,
        request: VirtualSurfaceFrameRequest<VirtualSurfaceBatch>,
    ) -> Result<VirtualSurfaceRequestReport, VirtualSurfacePipelineError<B::Error>> {
        if let Some(schedule) = request.schedule {
            self.schedule = schedule;
        }
        let batch = if let Some(batch) = request.batch {
            Some(
                batch
                    .apply_to(&mut self.surface)
                    .map_err(VirtualSurfacePipelineError::Surface)?,
            )
        } else {
            None
        };
        if let Some(viewport) = request.viewport {
            self.surface
                .apply(VirtualSurfaceCommand::SetViewport(viewport))
                .map_err(VirtualSurfacePipelineError::Surface)?;
        }
        if let Some(scroll) = request.scroll {
            self.surface
                .apply(VirtualSurfaceCommand::SetScroll(scroll))
                .map_err(VirtualSurfacePipelineError::Surface)?;
        }
        let frame = self.run_frame()?;
        Ok(VirtualSurfaceRequestReport { batch, frame })
    }
}

impl<B: VirtualGpuFrameBackend> VirtualSurfacePipeline<B> {
    pub fn build_gpu_frame_packet(&mut self) -> VirtualGpuFramePacket {
        self.surface.build_frame_transaction().build_gpu_packet()
    }

    pub fn run_gpu_frame(
        &mut self,
    ) -> Result<VirtualFrameRunReport, VirtualSurfacePipelineError<B::Error>> {
        let scheduled = self
            .surface
            .build_scheduled_frame_transaction(self.schedule);
        let original_stats = scheduled.original_stats;
        let scheduled_stats = scheduled.transaction.stats();
        let packet = scheduled.transaction.build_gpu_packet();
        let gpu_stats = packet.stats();
        let deferred = scheduled.deferred;
        let needs_another_frame = scheduled.has_deferred_work();
        let revision = scheduled.transaction.revision;
        let commit = self
            .backend
            .execute_gpu_frame(&packet)
            .map_err(VirtualSurfacePipelineError::Backend)?;
        self.surface
            .commit_frame_transaction(&scheduled.transaction, &commit)
            .map_err(VirtualSurfacePipelineError::Surface)?;
        Ok(VirtualFrameRunReport {
            revision,
            original: original_stats,
            scheduled: scheduled_stats,
            deferred,
            gpu: gpu_stats,
            needs_another_frame,
        })
    }
}
