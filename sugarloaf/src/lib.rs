#![cfg_attr(
    target_arch = "wasm32",
    allow(
        dead_code,
        irrefutable_let_patterns,
        unreachable_patterns,
        unused_imports,
        unused_macros
    )
)]

pub mod components;
pub mod context;
pub mod font;
mod font_cache;
pub mod grid;
pub mod layout;
mod occlusion;
pub mod renderer;
mod sugarloaf;
pub mod text;
pub mod virtual_surface;

// Re-export upstream swash so call sites can use `sugarloaf::swash::*`.
// This path was used by the in-tree fork; preserve it for stability.
pub use swash;

// Expose WGPU when the `wgpu` feature is enabled. Downstream code
// that needs `wgpu::Color` etc. picks it up via `sugarloaf::wgpu::…`.
#[cfg(feature = "wgpu")]
pub use wgpu;

pub use swash::{Attributes, Stretch, Style, Weight};

pub use crate::font_cache::ResolvedGlyph;
pub use crate::sugarloaf::{
    graphics::{
        ColorType, Graphic, GraphicData, GraphicDataEntry, GraphicId, GraphicOverlay,
        Graphics, ResizeCommand, ResizeParameter, MAX_GRAPHIC_DIMENSIONS,
    },
    primitives::{
        contains_braille_dot, drawable_character, is_private_user_area,
        BackgroundImageFit, Corners, CursorKind, DrawableChar, ImageProperties, Object,
        Quad, Rect, RichText, RichTextLinesRange, RichTextRenderData, SugarCursor,
    },
    Color, Colorspace, Sugarloaf, SugarloafBackend, SugarloafErrors, SugarloafRenderer,
    SugarloafWindow, SugarloafWindowSize, SugarloafWithErrors,
};
// `Filter` is the librashader CRT/scanline-filter wrapper — wgpu-only,
// and also unavailable on wasm32 (see `components/mod.rs`).
#[cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
pub use components::filters::Filter;
pub use components::shader_overlay::{
    ShaderOverlayConfig, ShaderOverlayError, BUILTIN_CTV_ROUND, BUILTIN_HYPNO_CRT,
    BUILTIN_SHADER_OVERLAYS,
};
pub use layout::{
    Content, RichTextConfig, SpanStyle, SpanStyleDecoration, TextDimensions,
    UnderlineInfo, UnderlineShape,
};
pub use virtual_surface::{
    AcceptAllVirtualSurfaceBackend, AxisRange, CacheStats, CacheTier, DirtyKind,
    GpuResidencyPolicy, LayoutKey, NodeGeometry, NodeId, NodeRevision, NodeSource,
    NodeSourceRange, NodeStyle, RetainedChunk, SurfaceMetrics, SurfaceRevision,
    VirtualAgentAdapter, VirtualAgentInput, VirtualAgentMessage,
    VirtualAgentMessageUpdate, VirtualAgentRole, VirtualAgentStats,
    VirtualBatchApplyReport, VirtualBounds, VirtualCodeAdapter, VirtualCodeAdapterConfig,
    VirtualCodeInput, VirtualCodeStats, VirtualContentId, VirtualContentKind,
    VirtualContentPayload, VirtualContentProvider, VirtualContentRef,
    VirtualContentResolutionStats, VirtualContentStoreError, VirtualDamage,
    VirtualDamageKind, VirtualDeferredResourceStats, VirtualDiagnosticSeverity,
    VirtualDrawCommand, VirtualFrameAction, VirtualFrameCommit, VirtualFrameIntent,
    VirtualFrameNodePlan, VirtualFramePlan, VirtualFrameRunReport,
    VirtualFrameSchedulePolicy, VirtualFrameTransaction, VirtualFrameTransactionStats,
    VirtualGpuContentPrefetchPolicy, VirtualGpuContentRequest, VirtualGpuDrawBatch,
    VirtualGpuFrameBackend, VirtualGpuFrameContext, VirtualGpuFramePacket,
    VirtualGpuFramePacketError, VirtualGpuInstance, VirtualGpuLayer,
    VirtualGpuPacketStats, VirtualGpuPrimitive, VirtualGpuSourceWindow, VirtualHit,
    VirtualHitTest, VirtualInMemoryContentStore, VirtualLayout, VirtualMarkdownAdapter,
    VirtualMarkdownInput, VirtualMarkdownStats, VirtualMeasuredLayout, VirtualNode,
    VirtualNodeKind, VirtualPrefetchHint, VirtualResourceDescriptor,
    VirtualResourceFailure, VirtualResourceId, VirtualResourceKind, VirtualResourceOp,
    VirtualRevealAlign, VirtualRevealTarget, VirtualRouteFrameReport,
    VirtualScheduledFrameTransaction, VirtualScroll, VirtualScrollAnchor,
    VirtualSourceEdit, VirtualSourceMatch, VirtualSourceQuery, VirtualSourceRevision,
    VirtualSourceTextEntry, VirtualSourceTextStore, VirtualSugarloafObjectPlan,
    VirtualSurface, VirtualSurfaceAdapter, VirtualSurfaceBackend,
    VirtualSurfaceBackendCapabilities, VirtualSurfaceBatch, VirtualSurfaceCommand,
    VirtualSurfaceConfig, VirtualSurfaceError, VirtualSurfaceFrameRequest,
    VirtualSurfacePipeline, VirtualSurfacePipelineError,
    VirtualSurfaceProtocolCapabilities, VirtualSurfaceProtocolVersion,
    VirtualSurfaceRequestReport, VirtualSurfaceRoute, VirtualSurfaceRouteEntry,
    VirtualSurfaceRouteId, VirtualSurfaceRouteKind, VirtualSurfaceRouter,
    VirtualSurfaceRouterError, VirtualSurfaceSnapshot, VirtualSurfaceWireEnvelope,
    VirtualTextAppendStats, VirtualTextEditStats, VirtualTextLineCheckpoint,
    VirtualTextLineIndex, VirtualTextLineIndexConfig, VirtualTextLineIndexStats,
    VirtualTextMeasurement, VirtualTextMeasurementRequest, VirtualTextOverlay,
    VirtualTextOverlayKind, VirtualTextPlan, VirtualTextSpan, VirtualTextSpanKind,
    VirtualTextStyle, VirtualTextWrap, VirtualTileId, VirtualTileRange, VirtualViewport,
    VisibleNode, VisibleSet,
};
