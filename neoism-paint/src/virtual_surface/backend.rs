use super::resource::{VirtualFrameCommit, VirtualFrameTransaction};

use serde::{Deserialize, Serialize};

/// Backend capabilities advertised to source adapters and scheduling code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualSurfaceBackendCapabilities {
    pub cpu_draw_lists: bool,
    pub gpu_draw_buffers: bool,
    pub gpu_frame_packets: bool,
    pub indirect_draw_batches: bool,
    pub source_content_requests: bool,
    pub texture_tiles: bool,
    pub hit_regions: bool,
    pub max_texture_dimension: u32,
    pub max_upload_bytes_per_frame: usize,
}

impl Default for VirtualSurfaceBackendCapabilities {
    fn default() -> Self {
        Self {
            cpu_draw_lists: true,
            gpu_draw_buffers: true,
            gpu_frame_packets: true,
            indirect_draw_batches: true,
            source_content_requests: true,
            texture_tiles: true,
            hit_regions: true,
            max_texture_dimension: 16_384,
            max_upload_bytes_per_frame: 64 * 1024 * 1024,
        }
    }
}

/// Backend contract for the retained virtual-surface protocol.
///
/// Implementations should consume resource operations in order, upload or retain
/// backend resources, then return a commit containing the resource ids that are
/// actually ready. Sugarloaf only marks chunks reusable after that commit.
pub trait VirtualSurfaceBackend {
    type Error;

    fn capabilities(&self) -> VirtualSurfaceBackendCapabilities {
        VirtualSurfaceBackendCapabilities::default()
    }

    fn execute_frame(
        &mut self,
        transaction: &VirtualFrameTransaction,
    ) -> Result<VirtualFrameCommit, Self::Error>;
}

/// Probe/test backend that accepts every resource operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct AcceptAllVirtualSurfaceBackend;

impl VirtualSurfaceBackend for AcceptAllVirtualSurfaceBackend {
    type Error = std::convert::Infallible;

    fn execute_frame(
        &mut self,
        transaction: &VirtualFrameTransaction,
    ) -> Result<VirtualFrameCommit, Self::Error> {
        Ok(transaction.successful_commit())
    }
}
