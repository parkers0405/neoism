//! Retained virtual surface runtime for massive structured documents.
//!
//! This module is intentionally independent of Neoism's markdown editor,
//! agent pane, and future native code editor. Those callers should submit
//! semantic node operations to this runtime instead of issuing per-frame draw
//! calls. Sugarloaf then owns layout invalidation, visible-range queries, and
//! retained GPU-ready chunks.
//!
//! The core invariant is:
//!
//! ```text
//! frame cost = visible nodes + dirty nodes
//! ```
//!
//! not total document size.

mod adapter;
mod agent;
mod backend;
mod cache;
mod code;
mod content;
mod gpu;
mod index;
mod lower;
mod markdown;
mod pipeline;
mod protocol;
mod render;
mod resource;
mod router;
mod standard;
mod surface;

pub use adapter::{VirtualSourceRevision, VirtualSurfaceAdapter, VirtualSurfaceBatch};
pub use agent::{
    VirtualAgentAdapter, VirtualAgentInput, VirtualAgentMessage,
    VirtualAgentMessageUpdate, VirtualAgentRole, VirtualAgentStats,
};
pub use backend::{
    AcceptAllVirtualSurfaceBackend, VirtualSurfaceBackend,
    VirtualSurfaceBackendCapabilities,
};
pub use cache::{CacheStats, CacheTier, GpuResidencyPolicy, RetainedChunk};
pub use code::{
    VirtualCodeAdapter, VirtualCodeAdapterConfig, VirtualCodeInput, VirtualCodeStats,
};
pub use content::{
    VirtualContentPayload, VirtualContentProvider, VirtualContentResolutionStats,
    VirtualContentStoreError, VirtualInMemoryContentStore, VirtualSourceTextEntry,
    VirtualSourceTextStore, VirtualTextAppendStats, VirtualTextEditStats,
    VirtualTextLineCheckpoint, VirtualTextLineIndex, VirtualTextLineIndexConfig,
    VirtualTextLineIndexStats,
};
pub use gpu::{
    VirtualGpuContentPrefetchPolicy, VirtualGpuContentRequest, VirtualGpuDrawBatch,
    VirtualGpuFrameBackend, VirtualGpuFrameContext, VirtualGpuFramePacket,
    VirtualGpuFramePacketError, VirtualGpuInstance, VirtualGpuLayer,
    VirtualGpuPacketStats, VirtualGpuPrimitive, VirtualGpuSourceWindow,
};
pub use index::AxisRange;
pub use lower::VirtualObjectPlan;
pub use markdown::{VirtualMarkdownAdapter, VirtualMarkdownInput, VirtualMarkdownStats};
pub use pipeline::{
    VirtualFrameRunReport, VirtualSurfacePipeline, VirtualSurfacePipelineError,
    VirtualSurfaceRequestReport,
};
pub use protocol::{
    DirtyKind, LayoutKey, NodeGeometry, NodeId, NodeRevision, NodeSource,
    NodeSourceRange, NodeStyle, SurfaceMetrics, SurfaceRevision, VirtualBounds,
    VirtualContentId, VirtualContentKind, VirtualContentRef, VirtualDamage,
    VirtualDamageKind, VirtualDiagnosticSeverity, VirtualFrameAction,
    VirtualFrameNodePlan, VirtualFramePlan, VirtualHit, VirtualHitTest, VirtualLayout,
    VirtualMeasuredLayout, VirtualNode, VirtualNodeKind, VirtualPrefetchHint,
    VirtualRevealAlign, VirtualRevealTarget, VirtualScroll, VirtualScrollAnchor,
    VirtualSourceEdit, VirtualSourceMatch, VirtualSourceQuery, VirtualSurfaceCommand,
    VirtualSurfaceConfig, VirtualSurfaceError, VirtualSurfaceSnapshot,
    VirtualTextMeasurement, VirtualTextMeasurementRequest, VirtualTextOverlay,
    VirtualTextOverlayKind, VirtualTextPlan, VirtualTextSpan, VirtualTextSpanKind,
    VirtualTextStyle, VirtualTextWrap, VirtualTileId, VirtualTileRange, VirtualViewport,
    VisibleNode, VisibleSet,
};
pub use render::VirtualDrawCommand;
pub use resource::{
    VirtualDeferredResourceStats, VirtualFrameCommit, VirtualFrameSchedulePolicy,
    VirtualFrameTransaction, VirtualFrameTransactionStats, VirtualResourceDescriptor,
    VirtualResourceFailure, VirtualResourceId, VirtualResourceKind, VirtualResourceOp,
    VirtualScheduledFrameTransaction,
};
pub use router::{
    VirtualRouteFrameReport, VirtualSurfaceRouteEntry, VirtualSurfaceRouter,
    VirtualSurfaceRouterError,
};
pub use standard::{
    VirtualBatchApplyReport, VirtualFrameIntent, VirtualSurfaceFrameRequest,
    VirtualSurfaceProtocolCapabilities, VirtualSurfaceProtocolVersion,
    VirtualSurfaceRoute, VirtualSurfaceRouteId, VirtualSurfaceRouteKind,
    VirtualSurfaceWireEnvelope,
};
pub use surface::VirtualSurface;

#[cfg(test)]
mod tests;
