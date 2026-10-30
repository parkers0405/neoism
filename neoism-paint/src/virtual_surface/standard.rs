use super::backend::VirtualSurfaceBackendCapabilities;
use super::protocol::{
    NodeSource, SurfaceRevision, VirtualScroll, VirtualSurfaceConfig, VirtualViewport,
};
use super::resource::VirtualFrameSchedulePolicy;

use serde::{Deserialize, Serialize};

/// Wire-level version for Sugarloaf's virtual-surface protocol.
///
/// This is deliberately separate from crate versioning. Adapters and future
/// remote producers can negotiate this small contract without caring how the
/// renderer crate itself is packaged.
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
pub struct VirtualSurfaceProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl VirtualSurfaceProtocolVersion {
    pub const CURRENT: Self = Self { major: 1, minor: 0 };

    pub fn is_compatible_with(self, other: Self) -> bool {
        self.major == other.major && self.minor >= other.minor
    }
}

/// Stable route for a virtual surface producer.
///
/// One renderer can host many producers: markdown files, model-generated
/// markdown, agent history, diff/log streams, and future code buffers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VirtualSurfaceRoute {
    pub id: VirtualSurfaceRouteId,
    pub kind: VirtualSurfaceRouteKind,
    pub source: NodeSource,
}

impl VirtualSurfaceRoute {
    pub fn new(
        id: impl Into<String>,
        kind: VirtualSurfaceRouteKind,
        source: NodeSource,
    ) -> Self {
        Self {
            id: VirtualSurfaceRouteId(id.into()),
            kind,
            source,
        }
    }

    pub fn synthetic(id: impl Into<String>, namespace: impl Into<String>) -> Self {
        let namespace = namespace.into();
        Self::new(
            id,
            VirtualSurfaceRouteKind::Synthetic,
            NodeSource::Synthetic { namespace },
        )
    }

    pub fn markdown_file(path: impl Into<String>) -> Self {
        let path = path.into();
        Self::new(
            format!("markdown:file:{path}"),
            VirtualSurfaceRouteKind::MarkdownFile,
            NodeSource::File { path },
        )
    }

    pub fn model_markdown(
        session: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let session = session.into();
        let message = message.into();
        Self::new(
            format!("markdown:model:{session}:{message}"),
            VirtualSurfaceRouteKind::ModelMarkdown,
            NodeSource::AgentMessage { session, message },
        )
    }

    pub fn agent(session: impl Into<String>) -> Self {
        let session = session.into();
        Self::new(
            format!("agent:{session}"),
            VirtualSurfaceRouteKind::AgentTranscript,
            NodeSource::Synthetic { namespace: session },
        )
    }

    pub fn code_buffer(buffer: impl Into<String>) -> Self {
        let buffer = buffer.into();
        Self::new(
            format!("code:{buffer}"),
            VirtualSurfaceRouteKind::CodeBuffer,
            NodeSource::CodeBuffer { buffer },
        )
    }
}

impl Default for VirtualSurfaceRoute {
    fn default() -> Self {
        Self::synthetic("surface:default", "default")
    }
}

/// Stable route id, intentionally string-backed so callers can use native ids.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct VirtualSurfaceRouteId(pub String);

/// Producer family for capability and scheduling policy decisions.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualSurfaceRouteKind {
    MarkdownFile,
    ModelMarkdown,
    AgentTranscript,
    CodeBuffer,
    Terminal,
    Diff,
    Log,
    Synthetic,
    Custom(String),
}

/// Latency/quality intent for a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualFrameIntent {
    /// User is actively scrolling, typing, or selecting. Favor present latency.
    #[default]
    Interactive,
    /// Fill retained GPU/texture caches around the viewport.
    WarmCache,
    /// Max quality for export/screenshot-like surfaces.
    FullQuality,
}

/// Capability bundle that adapters can inspect before choosing node sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualSurfaceProtocolCapabilities {
    pub version: VirtualSurfaceProtocolVersion,
    pub backend: VirtualSurfaceBackendCapabilities,
    pub max_retained_chunks: usize,
    pub tile_height_px: u32,
    pub max_batch_commands: usize,
    pub anchor_preservation: bool,
    pub splice_nodes: bool,
    pub tiled_resources: bool,
    pub scheduled_gpu_uploads: bool,
    pub gpu_frame_packets: bool,
    pub source_content_refs: bool,
    pub damage_prefetch: bool,
}

impl VirtualSurfaceProtocolCapabilities {
    pub fn from_config(
        config: VirtualSurfaceConfig,
        backend: VirtualSurfaceBackendCapabilities,
    ) -> Self {
        Self {
            version: VirtualSurfaceProtocolVersion::CURRENT,
            backend,
            max_retained_chunks: config.max_retained_chunks,
            tile_height_px: config.tile_height_px.max(1.0).round() as u32,
            max_batch_commands: 65_536,
            anchor_preservation: true,
            splice_nodes: true,
            tiled_resources: true,
            scheduled_gpu_uploads: true,
            gpu_frame_packets: backend.gpu_frame_packets,
            source_content_refs: backend.source_content_requests,
            damage_prefetch: true,
        }
    }
}

/// One render/update request at the Sugarloaf boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualSurfaceFrameRequest<B> {
    pub route: VirtualSurfaceRoute,
    pub intent: VirtualFrameIntent,
    pub viewport: Option<VirtualViewport>,
    pub scroll: Option<VirtualScroll>,
    pub schedule: Option<VirtualFrameSchedulePolicy>,
    pub batch: Option<B>,
}

impl<B> VirtualSurfaceFrameRequest<B> {
    pub fn new(route: VirtualSurfaceRoute) -> Self {
        Self {
            route,
            intent: VirtualFrameIntent::Interactive,
            viewport: None,
            scroll: None,
            schedule: None,
            batch: None,
        }
    }

    pub fn with_intent(mut self, intent: VirtualFrameIntent) -> Self {
        self.intent = intent;
        self
    }

    pub fn with_viewport(mut self, viewport: VirtualViewport) -> Self {
        self.viewport = Some(viewport);
        self
    }

    pub fn with_scroll(mut self, scroll: VirtualScroll) -> Self {
        self.scroll = Some(scroll);
        self
    }

    pub fn with_schedule(mut self, schedule: VirtualFrameSchedulePolicy) -> Self {
        self.schedule = Some(schedule);
        self
    }

    pub fn with_batch(mut self, batch: B) -> Self {
        self.batch = Some(batch);
        self
    }
}

/// Route/version wrapper for protocol payloads sent across process, thread, or
/// backend boundaries. The payload can be a batch, frame transaction, compact
/// GPU packet, snapshot, or future renderer message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualSurfaceWireEnvelope<T> {
    pub version: VirtualSurfaceProtocolVersion,
    pub route: VirtualSurfaceRoute,
    pub sequence: u64,
    pub payload: T,
}

impl<T> VirtualSurfaceWireEnvelope<T> {
    pub fn new(route: VirtualSurfaceRoute, payload: T) -> Self {
        Self {
            version: VirtualSurfaceProtocolVersion::CURRENT,
            route,
            sequence: 0,
            payload,
        }
    }

    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = sequence;
        self
    }

    pub fn is_compatible(&self) -> bool {
        VirtualSurfaceProtocolVersion::CURRENT.is_compatible_with(self.version)
    }
}

/// Result of applying one producer batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualBatchApplyReport {
    pub route: VirtualSurfaceRoute,
    pub previous_revision: SurfaceRevision,
    pub current_revision: SurfaceRevision,
    pub commands: usize,
    pub anchor_preserved: bool,
}
