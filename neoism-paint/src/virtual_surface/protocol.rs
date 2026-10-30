use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable identifier for a semantic visual node inside a virtual surface.
///
/// Adapters choose the mapping:
/// - markdown: block id / line range id
/// - agent: message id / tool id / streaming tail id
/// - code editor: line tile id / fold region id
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
pub struct NodeId(pub u64);

impl NodeId {
    pub const ROOT: Self = Self(0);

    #[inline]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    #[inline]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "node:{}", self.0)
    }
}

/// Optional subdivision inside a large semantic node. A markdown block, code
/// block, table, or agent transcript chunk can remain one semantic node while
/// Sugarloaf plans GPU resources in smaller vertical tiles.
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
pub struct VirtualTileId {
    pub node: NodeId,
    pub tile: u32,
}

impl VirtualTileId {
    pub fn new(node: NodeId, tile: u32) -> Self {
        Self { node, tile }
    }
}

/// Inclusive/exclusive range of visible tiles for one node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VirtualTileRange {
    pub start: u32,
    pub end: u32,
}

impl VirtualTileRange {
    pub const WHOLE_NODE: Self = Self { start: 0, end: 1 };

    pub fn new(start: u32, end: u32) -> Self {
        Self {
            start,
            end: end.max(start),
        }
    }

    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }

    pub fn is_whole_node(self) -> bool {
        self == Self::WHOLE_NODE
    }
}

/// Monotonic node content revision. A revision change invalidates cached layout
/// and retained draw chunks for the node.
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
pub struct NodeRevision(pub u64);

impl NodeRevision {
    #[inline]
    pub fn bump(&mut self) {
        self.0 = self.0.saturating_add(1);
    }
}

/// Monotonic whole-surface revision. Useful for adapters that need to cheaply
/// know whether any protocol operation changed the surface.
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
pub struct SurfaceRevision(pub u64);

impl SurfaceRevision {
    #[inline]
    pub fn bump(&mut self) {
        self.0 = self.0.saturating_add(1);
    }
}

/// Semantic source family for a node. Sugarloaf does not parse source content;
/// this names where the node came from so callers can map hit tests and dirty
/// ranges back into their own models.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeSource {
    File { path: String },
    AgentMessage { session: String, message: String },
    CodeBuffer { buffer: String },
    Terminal { pane: String },
    Synthetic { namespace: String },
}

/// Source range associated with a node. Units are caller-defined but stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeSourceRange {
    pub start: u64,
    pub end: u64,
}

impl NodeSourceRange {
    #[inline]
    pub fn new(start: u64, end: u64) -> Self {
        Self {
            start,
            end: end.max(start),
        }
    }

    #[inline]
    pub fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    #[inline]
    pub fn intersects(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    #[inline]
    pub fn contains(self, offset: u64) -> bool {
        offset >= self.start && offset < self.end
    }

    #[inline]
    pub fn overlap(self, other: Self) -> Option<Self> {
        if self.intersects(other) {
            Some(Self::new(
                self.start.max(other.start),
                self.end.min(other.end),
            ))
        } else if self.is_empty() && other.contains(self.start) {
            Some(self)
        } else if other.is_empty() && self.contains(other.start) {
            Some(other)
        } else {
            None
        }
    }
}

/// Query shape for mapping source ranges back to virtualized visual nodes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VirtualSourceQuery {
    pub source: NodeSource,
    pub range: NodeSourceRange,
}

impl VirtualSourceQuery {
    pub fn new(source: NodeSource, range: NodeSourceRange) -> Self {
        Self { source, range }
    }
}

/// Host-facing match for a source-range query.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualSourceMatch {
    pub node: NodeId,
    pub index: usize,
    pub bounds: VirtualBounds,
    pub source_range: NodeSourceRange,
    pub overlap: NodeSourceRange,
}

/// Alignment policy for reveal/scroll-to-source operations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualRevealAlign {
    Start,
    Center,
    End,
    #[default]
    Nearest,
}

/// Protocol target for scrolling a source range into view.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VirtualRevealTarget {
    pub source: NodeSource,
    pub range: NodeSourceRange,
    pub align: VirtualRevealAlign,
}

impl VirtualRevealTarget {
    pub fn new(
        source: NodeSource,
        range: NodeSourceRange,
        align: VirtualRevealAlign,
    ) -> Self {
        Self {
            source,
            range,
            align,
        }
    }
}

/// Source edit metadata for dirtying retained nodes after text changes.
///
/// The source owner performs the actual text edit. This protocol record tells
/// the retained surface which old and new source ranges need reshaping,
/// measurement, and GPU invalidation while preserving one command shape for
/// markdown, model output, agent transcripts, and future code buffers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualSourceEdit {
    pub source: NodeSource,
    pub old_range: NodeSourceRange,
    pub new_range: NodeSourceRange,
    pub kind: DirtyKind,
}

impl VirtualSourceEdit {
    pub fn new(
        source: NodeSource,
        old_range: NodeSourceRange,
        new_range: NodeSourceRange,
        kind: DirtyKind,
    ) -> Self {
        Self {
            source,
            old_range,
            new_range,
            kind,
        }
    }

    pub fn dirty_range(&self) -> NodeSourceRange {
        NodeSourceRange::new(
            self.old_range.start.min(self.new_range.start),
            self.old_range.end.max(self.new_range.end),
        )
    }
}

/// Stable id for external content backing a virtual node.
///
/// Nodes do not need to carry raw text in every frame. They can point at a
/// source-backed content slice that a renderer, shaper, or daemon can cache by
/// id/hash and request only when needed.
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
pub struct VirtualContentId(pub u64);

/// Semantic content family for shaping and caching decisions.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualContentKind {
    PlainText,
    Markdown,
    Code { language: Option<String> },
    Table,
    Image,
    Binary,
    Custom(String),
}

/// Source-backed content descriptor. This is the cheap handoff for huge files:
/// stable identity, source range, byte/line counts, and content hash without
/// copying the actual string through every frame transaction.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VirtualContentRef {
    pub id: VirtualContentId,
    pub kind: VirtualContentKind,
    pub source: NodeSource,
    pub range: NodeSourceRange,
    pub revision: NodeRevision,
    pub hash: u64,
    pub byte_len: u64,
    pub line_start: u64,
    pub line_count: u32,
}

impl VirtualContentRef {
    pub fn new(
        id: VirtualContentId,
        kind: VirtualContentKind,
        source: NodeSource,
        range: NodeSourceRange,
        revision: NodeRevision,
        hash: u64,
        line_count: u32,
    ) -> Self {
        Self {
            id,
            kind,
            source,
            range,
            revision,
            hash,
            byte_len: range.len(),
            line_start: 0,
            line_count,
        }
    }

    pub fn with_line_start(mut self, line_start: u64) -> Self {
        self.line_start = line_start;
        self
    }

    pub fn line_end(&self) -> u64 {
        self.line_start.saturating_add(u64::from(self.line_count))
    }
}

/// Text wrapping policy for source-backed text content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualTextWrap {
    #[default]
    Word,
    NoWrap,
    Character,
}

/// Semantic span family for markdown styling, agent output, syntax tokens,
/// diagnostics, links, and future editor overlays.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualTextSpanKind {
    Plain,
    Emphasis,
    Strong,
    InlineCode,
    Link,
    Heading,
    CodeToken { token: String },
    Diagnostic { severity: VirtualDiagnosticSeverity },
    Custom(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualDiagnosticSeverity {
    Hint,
    Info,
    Warning,
    Error,
}

/// Style override for a text span. A zero/None value means inherit from the
/// node style and renderer theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualTextStyle {
    pub style_hash: u64,
    pub foreground: Option<[f32; 4]>,
    pub background: Option<[f32; 4]>,
    pub underline: Option<[f32; 4]>,
    pub flags: u16,
}

/// Byte range inside a content ref plus semantic/style metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualTextSpan {
    pub range: NodeSourceRange,
    pub kind: VirtualTextSpanKind,
    pub style: VirtualTextStyle,
}

impl VirtualTextSpan {
    pub fn new(
        range: NodeSourceRange,
        kind: VirtualTextSpanKind,
        style: VirtualTextStyle,
    ) -> Self {
        Self { range, kind, style }
    }
}

/// Visual overlay associated with text content. This covers selections,
/// cursors, search matches, IME composition, and diagnostics without making
/// those concepts editor-specific.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualTextOverlay {
    pub id: u64,
    pub range: NodeSourceRange,
    pub kind: VirtualTextOverlayKind,
    pub color: [f32; 4],
    pub priority: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualTextOverlayKind {
    Selection,
    Cursor,
    SearchMatch,
    Composition,
    Diagnostic,
    Custom(u16),
}

/// Renderer-facing text plan for a node. It references source-backed content,
/// optional semantic spans, and overlays without requiring raw text in the
/// per-frame command stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualTextPlan {
    pub content: VirtualContentRef,
    pub wrap: VirtualTextWrap,
    pub tab_width: u16,
    pub spans_hash: u64,
    pub spans: Vec<VirtualTextSpan>,
    pub overlays: Vec<VirtualTextOverlay>,
}

impl VirtualTextPlan {
    pub fn new(content: VirtualContentRef) -> Self {
        Self {
            content,
            wrap: VirtualTextWrap::default(),
            tab_width: 4,
            spans_hash: 0,
            spans: Vec::new(),
            overlays: Vec::new(),
        }
    }

    pub fn with_wrap(mut self, wrap: VirtualTextWrap) -> Self {
        self.wrap = wrap;
        self
    }

    pub fn with_spans(mut self, spans_hash: u64, spans: Vec<VirtualTextSpan>) -> Self {
        self.spans_hash = spans_hash;
        self.spans = spans;
        self
    }

    pub fn with_overlays(mut self, overlays: Vec<VirtualTextOverlay>) -> Self {
        self.overlays = overlays;
        self
    }
}

/// Top-level visual kind. It is intentionally broader than markdown so the
/// same runtime can back agent history, future code buffers, logs, diffs, and
/// tables.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualNodeKind {
    Root,
    Text,
    MarkdownBlock,
    Heading,
    CodeLine,
    CodeTile,
    CodeBlock,
    Table,
    TableTile,
    AgentMessage,
    ToolCard,
    DiffHunk,
    Image,
    Overlay,
    Custom(String),
}

impl VirtualNodeKind {
    #[inline]
    pub fn is_edit_hot(&self) -> bool {
        matches!(
            self,
            Self::Text
                | Self::MarkdownBlock
                | Self::CodeLine
                | Self::CodeTile
                | Self::TableTile
        )
    }
}

/// Coarse style key used for cache identity. Real callers can keep their rich
/// style spans outside this struct and include their own hash in `style_hash`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeStyle {
    pub style_hash: u64,
    pub font_size_bucket: u16,
    pub flags: u16,
}

/// Bounds in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl VirtualBounds {
    #[inline]
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width: width.max(0.0),
            height: height.max(0.0),
        }
    }

    #[inline]
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    #[inline]
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    #[inline]
    pub fn intersects_y(self, top: f32, bottom: f32) -> bool {
        self.bottom() >= top && self.y <= bottom
    }
}

/// Geometry hints submitted by adapters. If `estimated_height` is present, the
/// node can participate in the height index before expensive layout exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeGeometry {
    pub min_width: f32,
    pub estimated_height: Option<f32>,
    pub fixed_height: Option<f32>,
    pub can_split: bool,
}

impl NodeGeometry {
    #[inline]
    pub fn estimated(height: f32) -> Self {
        Self {
            estimated_height: Some(height.max(0.0)),
            ..Self::default()
        }
    }

    #[inline]
    pub fn fixed(height: f32) -> Self {
        let height = height.max(0.0);
        Self {
            estimated_height: Some(height),
            fixed_height: Some(height),
            can_split: false,
            ..Self::default()
        }
    }

    #[inline]
    pub fn initial_height(self, fallback: f32) -> f32 {
        self.fixed_height
            .or(self.estimated_height)
            .unwrap_or(fallback)
            .max(0.0)
    }
}

/// A semantic visual node submitted to the virtual surface protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub kind: VirtualNodeKind,
    pub source: Option<NodeSource>,
    pub source_range: Option<NodeSourceRange>,
    pub content: Option<VirtualContentRef>,
    pub text_plan: Option<VirtualTextPlan>,
    pub revision: NodeRevision,
    pub style: NodeStyle,
    pub geometry: NodeGeometry,
    pub text_hash: u64,
    pub child_count_hint: u32,
}

impl VirtualNode {
    pub fn new(id: NodeId, kind: VirtualNodeKind) -> Self {
        Self {
            id,
            parent: None,
            kind,
            source: None,
            source_range: None,
            content: None,
            text_plan: None,
            revision: NodeRevision::default(),
            style: NodeStyle::default(),
            geometry: NodeGeometry::default(),
            text_hash: 0,
            child_count_hint: 0,
        }
    }

    pub fn with_parent(mut self, parent: NodeId) -> Self {
        self.parent = Some(parent);
        self
    }

    pub fn with_geometry(mut self, geometry: NodeGeometry) -> Self {
        self.geometry = geometry;
        self
    }

    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = NodeRevision(revision);
        self
    }

    pub fn with_text_hash(mut self, hash: u64) -> Self {
        self.text_hash = hash;
        self
    }

    pub fn with_source(mut self, source: NodeSource, range: NodeSourceRange) -> Self {
        self.source = Some(source);
        self.source_range = Some(range);
        self
    }

    pub fn with_content(mut self, content: VirtualContentRef) -> Self {
        self.content = Some(content);
        self
    }

    pub fn with_text_plan(mut self, text_plan: VirtualTextPlan) -> Self {
        self.text_plan = Some(text_plan);
        self
    }
}

/// Layout identity. When this key is unchanged the retained chunk may be reused
/// even if the node moves vertically due to scroll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayoutKey {
    pub node: NodeId,
    pub revision: NodeRevision,
    pub width_bucket: i32,
    pub scale_bucket: i32,
    pub style: NodeStyle,
    pub text_hash: u64,
}

impl LayoutKey {
    pub fn new(node: &VirtualNode, width: f32, scale: f32) -> Self {
        Self {
            node: node.id,
            revision: node.revision,
            width_bucket: measure_bucket(width),
            scale_bucket: measure_bucket(scale),
            style: node.style,
            text_hash: node.text_hash,
        }
    }
}

#[inline]
pub(crate) fn measure_bucket(value: f32) -> i32 {
    (value.max(0.0) * 4.0).round() as i32
}

/// Computed layout for one node.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualLayout {
    pub key: LayoutKey,
    pub bounds: VirtualBounds,
    pub baseline: f32,
    pub visual_line_count: u32,
}

impl VirtualLayout {
    pub fn estimated(key: LayoutKey, y: f32, width: f32, height: f32) -> Self {
        Self {
            key,
            bounds: VirtualBounds::new(0.0, y, width, height),
            baseline: 0.0,
            visual_line_count: 0,
        }
    }
}

/// Renderer-measured layout feedback for one semantic node.
///
/// Adapters can submit good estimates immediately; a backend or higher-level
/// text/layout system can later commit exact wrapped height/baseline data for
/// visible or recently warmed nodes without rebuilding the full surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualMeasuredLayout {
    pub node: NodeId,
    pub revision: NodeRevision,
    pub height: f32,
    pub baseline: f32,
    pub visual_line_count: u32,
}

impl VirtualMeasuredLayout {
    pub fn new(
        node: NodeId,
        revision: NodeRevision,
        height: f32,
        baseline: f32,
        visual_line_count: u32,
    ) -> Self {
        Self {
            node,
            revision,
            height: height.max(0.0),
            baseline: baseline.max(0.0),
            visual_line_count,
        }
    }
}

/// Request emitted to a text/layout engine for exact shaping. It is stable and
/// serializable so measurement can happen in-process, in a worker, or behind a
/// future GPU/daemon boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualTextMeasurementRequest {
    pub node: NodeId,
    pub revision: NodeRevision,
    pub content: VirtualContentRef,
    pub text_plan: Option<VirtualTextPlan>,
    pub width: f32,
    pub scale: f32,
    pub tile_range: VirtualTileRange,
}

impl VirtualTextMeasurementRequest {
    pub fn new(
        node: NodeId,
        revision: NodeRevision,
        content: VirtualContentRef,
        width: f32,
        scale: f32,
        tile_range: VirtualTileRange,
    ) -> Self {
        Self {
            node,
            revision,
            content,
            text_plan: None,
            width: width.max(0.0),
            scale: scale.max(0.01),
            tile_range,
        }
    }

    pub fn with_text_plan(mut self, text_plan: VirtualTextPlan) -> Self {
        self.text_plan = Some(text_plan);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualTextMeasurement {
    pub request: VirtualTextMeasurementRequest,
    pub layout: VirtualMeasuredLayout,
}

/// Viewport in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl Default for VirtualViewport {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            scale: 1.0,
        }
    }
}

impl VirtualViewport {
    pub fn new(x: f32, y: f32, width: f32, height: f32, scale: f32) -> Self {
        Self {
            x,
            y,
            width: width.max(0.0),
            height: height.max(0.0),
            scale: scale.max(0.01),
        }
    }
}

/// Scroll state. `scroll_y` is logical pixels in surface coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualScroll {
    pub scroll_y: f32,
    pub velocity_y: f32,
}

/// Stable viewport anchor used to preserve visual position across edits.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualScrollAnchor {
    pub node: NodeId,
    pub index: usize,
    pub local_y: f32,
    pub viewport_y: f32,
}

/// Runtime configuration for a virtual surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualSurfaceConfig {
    pub fallback_node_height: f32,
    pub overscan_px: f32,
    pub cold_distance_px: f32,
    pub warm_distance_px: f32,
    pub tile_height_px: f32,
    pub max_retained_chunks: usize,
}

impl Default for VirtualSurfaceConfig {
    fn default() -> Self {
        Self {
            fallback_node_height: 24.0,
            overscan_px: 900.0,
            warm_distance_px: 4_000.0,
            cold_distance_px: 32_000.0,
            tile_height_px: 512.0,
            max_retained_chunks: 16_384,
        }
    }
}

/// Kind of invalidation. Layout invalidation implies draw invalidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirtyKind {
    Layout,
    Draw,
    Gpu,
}

/// Protocol commands. Callers can batch these and apply them without exposing
/// internal storage details.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VirtualSurfaceCommand {
    UpsertNode(VirtualNode),
    UpsertNodes(Vec<VirtualNode>),
    RemoveNode(NodeId),
    RemoveRange {
        start: usize,
        end: usize,
    },
    SpliceNodes {
        start: usize,
        delete: usize,
        insert: Vec<VirtualNode>,
    },
    RebaseSourceAfter {
        start: usize,
        byte_delta: i64,
        line_delta: i64,
    },
    ReplaceAll(Vec<VirtualNode>),
    MarkDirty {
        node: NodeId,
        kind: DirtyKind,
    },
    MarkRangeDirty {
        start: usize,
        end: usize,
        kind: DirtyKind,
    },
    MarkSourceDirty {
        source: NodeSource,
        range: NodeSourceRange,
        kind: DirtyKind,
    },
    ApplySourceEdit(VirtualSourceEdit),
    SetSourceTextOverlays {
        source: NodeSource,
        overlays: Vec<VirtualTextOverlay>,
    },
    ClearSourceTextOverlays {
        source: NodeSource,
    },
    SetViewport(VirtualViewport),
    SetScroll(VirtualScroll),
    RestoreScrollAnchor(VirtualScrollAnchor),
    RevealSource(VirtualRevealTarget),
    CommitMeasuredLayouts(Vec<VirtualMeasuredLayout>),
    SetConfig(VirtualSurfaceConfig),
}

/// One visible node returned by a viewport query.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisibleNode {
    pub node: NodeId,
    pub index: usize,
    pub bounds: VirtualBounds,
    pub screen_y: f32,
    pub cache_tier: super::cache::CacheTier,
}

/// Visible query result.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VisibleSet {
    pub nodes: Vec<VisibleNode>,
    pub query_top: f32,
    pub query_bottom: f32,
    pub content_height: f32,
}

/// What the renderer should do for one visible node this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualFrameAction {
    /// Existing retained GPU chunk is current.
    Reuse,
    /// Node has no retained chunk yet.
    Build,
    /// Layout/draw identity changed; rebuild CPU draw data.
    RebuildDraw,
    /// CPU draw data is current but GPU residency was invalidated.
    UploadGpu,
    /// Stable cold/frozen content may be baked into a texture or long-lived
    /// backend buffer.
    BakeStatic,
}

/// Per-node frame plan.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualFrameNodePlan {
    pub node: NodeId,
    pub index: usize,
    pub bounds: VirtualBounds,
    pub tile_range: VirtualTileRange,
    pub cache_tier: super::cache::CacheTier,
    pub action: VirtualFrameAction,
}

/// Damage category for a node/tile in the current frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualDamageKind {
    Layout,
    Draw,
    Gpu,
    Resource,
}

/// Backend-facing damage record. Bounds are in surface coordinates, not screen
/// coordinates; the backend combines this with viewport/scroll to clip work.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualDamage {
    pub node: NodeId,
    pub index: usize,
    pub bounds: VirtualBounds,
    pub tile_range: VirtualTileRange,
    pub kind: VirtualDamageKind,
}

/// Renderer warming hint for near-viewport content. These are not required for
/// correctness; they let a backend upload or bake nearby tiles opportunistically
/// while keeping interactive frames bounded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualPrefetchHint {
    pub node: NodeId,
    pub index: usize,
    pub bounds: VirtualBounds,
    pub tile_range: VirtualTileRange,
    pub priority: u16,
    pub content: Option<VirtualContentRef>,
    pub text_plan: Option<VirtualTextPlan>,
}

/// Retained-surface frame plan, analogous to the editor grid's row source plan
/// but generalized to variable-height nodes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualFramePlan {
    pub visible: VisibleSet,
    pub nodes: Vec<VirtualFrameNodePlan>,
    pub damage: Vec<VirtualDamage>,
    pub prefetch: Vec<VirtualPrefetchHint>,
    pub reused: usize,
    pub build: usize,
    pub rebuild_draw: usize,
    pub upload_gpu: usize,
    pub bake_static: usize,
}

impl VirtualFramePlan {
    pub fn push(&mut self, node: VirtualFrameNodePlan) {
        if let Some(kind) = damage_kind_for_action(node.action) {
            self.damage.push(VirtualDamage {
                node: node.node,
                index: node.index,
                bounds: node.bounds,
                tile_range: node.tile_range,
                kind,
            });
        }
        match node.action {
            VirtualFrameAction::Reuse => self.reused += 1,
            VirtualFrameAction::Build => self.build += 1,
            VirtualFrameAction::RebuildDraw => self.rebuild_draw += 1,
            VirtualFrameAction::UploadGpu => self.upload_gpu += 1,
            VirtualFrameAction::BakeStatic => self.bake_static += 1,
        }
        self.nodes.push(node);
    }
}

fn damage_kind_for_action(action: VirtualFrameAction) -> Option<VirtualDamageKind> {
    match action {
        VirtualFrameAction::Reuse => None,
        VirtualFrameAction::Build | VirtualFrameAction::RebuildDraw => {
            Some(VirtualDamageKind::Draw)
        }
        VirtualFrameAction::UploadGpu => Some(VirtualDamageKind::Gpu),
        VirtualFrameAction::BakeStatic => Some(VirtualDamageKind::Resource),
    }
}

/// Input for hit testing. Coordinates are logical screen-space pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualHitTest {
    pub x: f32,
    pub y: f32,
}

/// Hit-test result. The source range remains caller-defined, so markdown can
/// map to block/byte positions, agent can map to message ranges, and code can
/// map to line/column tiles.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualHit {
    pub node: NodeId,
    pub index: usize,
    pub bounds: VirtualBounds,
    pub local_x: f32,
    pub local_y: f32,
    pub source_range: Option<NodeSourceRange>,
}

/// Aggregate surface metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SurfaceMetrics {
    pub node_count: usize,
    pub content_height: f32,
    pub dirty_layout_count: usize,
    pub dirty_draw_count: usize,
    pub dirty_gpu_count: usize,
    pub revision: SurfaceRevision,
}

/// Host-facing surface snapshot. This is the cheap diagnostic/bridge shape a
/// markdown pane, agent pane, or future code editor can consume without
/// reaching into Sugarloaf internals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualSurfaceSnapshot {
    pub revision: SurfaceRevision,
    pub viewport: VirtualViewport,
    pub scroll: VirtualScroll,
    pub metrics: SurfaceMetrics,
    pub cache: super::cache::CacheStats,
    pub visible: VisibleSet,
    pub visible_start: usize,
    pub visible_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualSurfaceError {
    DuplicateNode(NodeId),
    MissingNode(NodeId),
    RevisionMismatch {
        expected: SurfaceRevision,
        actual: SurfaceRevision,
    },
    ResourceCommitFailed {
        failed: usize,
    },
    NodeRevisionMismatch {
        node: NodeId,
        expected: NodeRevision,
        actual: NodeRevision,
    },
    InvalidRange {
        start: usize,
        end: usize,
        len: usize,
    },
}

impl fmt::Display for VirtualSurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateNode(id) => write!(f, "duplicate virtual surface node {id}"),
            Self::MissingNode(id) => write!(f, "missing virtual surface node {id}"),
            Self::RevisionMismatch { expected, actual } => {
                write!(
                    f,
                    "virtual surface revision mismatch: expected {}, actual {}",
                    expected.0, actual.0
                )
            }
            Self::ResourceCommitFailed { failed } => {
                write!(
                    f,
                    "virtual surface resource commit failed for {failed} resource ops"
                )
            }
            Self::NodeRevisionMismatch {
                node,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "virtual surface node revision mismatch for {node}: expected {}, actual {}",
                    expected.0, actual.0
                )
            }
            Self::InvalidRange { start, end, len } => {
                write!(
                    f,
                    "invalid virtual surface range {start}..{end} for len {len}"
                )
            }
        }
    }
}

impl std::error::Error for VirtualSurfaceError {}
