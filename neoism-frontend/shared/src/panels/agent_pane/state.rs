//! Shared non-IO state for the agent pane views.

mod caches;
mod connect;
mod hit_rects;
mod ingest;
mod input_edit;
mod permissions;
pub mod picker;
mod pickers_state;
mod questions;
mod selection;
mod session;
pub mod side_panel;
mod streaming;
mod timeline;

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use web_time::Duration;

use serde_json::{json, Value};
use sugarloaf::{
    DirtyKind, NodeSourceRange, VirtualAgentAdapter, VirtualAgentMessage,
    VirtualAgentMessageUpdate, VirtualAgentRole, VirtualMeasuredLayout, VirtualScroll,
    VirtualSourceRevision, VirtualSurface, VirtualSurfaceCommand, VirtualSurfaceConfig,
    VirtualViewport,
};
use web_time::Instant;

use crate::panels::agent_pane::input_controller::{self, AgentInputBuffer, InputWrapRow};
use crate::panels::agent_pane::interaction_policy;
use crate::panels::agent_pane::outbound::OutboundAgentCommand;
use crate::panels::agent_pane::permission_policy::{self, PermissionReplyStart};
use crate::panels::agent_pane::status_policy;
use crate::panels::agent_pane::timeline_scroll_policy::ctrl_u_d_scroll_delta;
use crate::panels::agent_pane::usage_policy::{
    usage_detail_lines, usage_summary_label, UsageSnapshot,
};

use self::picker::{NeoismAgentPicker, NeoismAgentPickerKind, NeoismAgentPickerOption};
use self::side_panel::{BranchStatus, NeoismAgentSidePanel};

use crate::panels::agent_pane::question_policy::NeoismAgentPendingQuestion;

#[derive(Clone, Debug, Default)]
pub struct NeoismAgentPaneSnapshot {
    pub input: Option<String>,
    pub messages: Vec<NeoismAgentMessage>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub directory: Option<String>,
    pub session_id: Option<String>,
    pub streaming_state: Option<NeoismAgentStreamingState>,
    pub pending_permission: Option<NeoismAgentPendingPermission>,
}

const DEFAULT_AGENT: &str = "build";
const DEFAULT_MODEL: &str = "";
#[allow(dead_code)]
const FILE_MENTION_LIMIT: usize = 10;
#[allow(dead_code)]
const FILE_MENTION_SCAN_LIMIT: usize = 512;
#[allow(dead_code)]
const FILE_MENTION_VISIT_LIMIT: usize = 6000;
#[allow(dead_code)]
const FILE_MENTION_MAX_DEPTH: usize = 8;
#[allow(dead_code)]
const MAX_INLINE_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
#[allow(dead_code)]
const ABORT_STREAM_SUPPRESSION: Duration = Duration::from_secs(5);
const TOOL_EXPAND_ANIMATION: Duration = Duration::from_millis(190);
const WORDMARK_CLICK_ANIMATION: Duration = Duration::from_millis(460);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeoismAgentMode {
    Build,
    Plan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NeoismAgentMessageKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Subtask,
    Compaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NeoismAgentOutputKind {
    Text,
    Code,
    Todos,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NeoismAgentTodo {
    pub status: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeoismAgentMessage {
    pub id: String,
    pub kind: NeoismAgentMessageKind,
    pub title: String,
    pub text: String,
    pub status: String,
    pub tool: String,
    pub output_kind: NeoismAgentOutputKind,
    pub lang: String,
    pub line_offset: Option<usize>,
    pub todos: Vec<NeoismAgentTodo>,
    pub detail: String,
    pub usage: Option<NeoismAgentUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NeoismAgentUsage {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub cost_micros: u64,
    pub context_limit: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeoismAgentPendingPermission {
    pub id: String,
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub source_agent: Option<String>,
    pub source_title: Option<String>,
    pub title: String,
    pub permission: String,
    pub patterns: Vec<String>,
    pub selected: usize,
    pub responding: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeoismAgentPermissionChoice {
    Once,
    Always,
    Reject,
}

impl NeoismAgentPermissionChoice {
    pub fn reply(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Always => "always",
            Self::Reject => "reject",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct MarkdownBlocksKey {
    text_hash: u64,
    text_len: usize,
    width_bucket: i32,
    scale_bucket: i32,
}

type CachedMarkdownBlocks =
    std::rc::Rc<Vec<crate::panels::agent_pane::view::markdown::AssistantMarkdownBlock>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimelineMeasureKey {
    id: u64,
    kind: NeoismAgentMessageKind,
    output_kind: NeoismAgentOutputKind,
    width_bucket: i32,
    scale_bucket: i32,
    tool_expanded: bool,
    tool_archived: bool,
    title: u64,
    text: u64,
    status: u64,
    tool: u64,
    lang: u64,
    line_offset: Option<usize>,
    todos: u64,
    detail: u64,
    selected_tool_group_child: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct TimelineLayoutRow {
    pub source_index: usize,
    pub source_end_index: usize,
    pub top: f32,
    pub height: f32,
    pub display_text: Option<String>,
    pub display_message: Option<NeoismAgentMessage>,
    pub markdown_blocks: Option<
        std::rc::Rc<
            Vec<crate::panels::agent_pane::view::markdown::AssistantMarkdownBlock>,
        >,
    >,
    pub tool_diff_sections:
        Option<crate::panels::agent_pane::view::tool_message::CachedToolDiffSections>,
    pub is_edit_tool: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct TimelineLayoutCache {
    pub epoch: u64,
    pub source_len: usize,
    pub width_bucket: i32,
    pub scale_bucket: i32,
    pub gap_bucket: i32,
    pub content_height: f32,
    pub pages: Vec<TimelineLayoutPage>,
    pub rows: Vec<TimelineLayoutRow>,
}

#[derive(Clone, Debug)]
pub(crate) struct TimelineLayoutPage {
    pub page_index: usize,
    pub source_start: usize,
    pub source_end: usize,
    pub row_start: usize,
    pub row_end: usize,
    pub top: f32,
    pub height: f32,
    pub measured: bool,
}

#[derive(Clone, Debug)]
struct AgentTimelineHistoryState {
    oldest_loaded_cursor: Option<String>,
    has_older: bool,
    loading_older: bool,
    last_requested_session_id: Option<String>,
}

impl Default for AgentTimelineHistoryState {
    fn default() -> Self {
        Self {
            oldest_loaded_cursor: None,
            has_older: true,
            loading_older: false,
            last_requested_session_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineVirtualRowMeasurement {
    pub source_index: usize,
    pub source_end_index: usize,
    pub height: f32,
    pub visual_line_count: u32,
}

#[derive(Default)]
pub struct TimelineDirtyMarks {
    pub ids: BTreeSet<String>,
    pub indices: BTreeSet<usize>,
}

#[derive(Clone, Copy, Debug)]
struct TimelineAnchor {
    content_y: f32,
    screen_y: f32,
}

#[derive(Clone, Copy, Debug)]
struct ToolExpandAnimation {
    started_at: Instant,
    expanding: bool,
}

impl ToolExpandAnimation {
    fn is_active(self) -> bool {
        Instant::now().saturating_duration_since(self.started_at) < TOOL_EXPAND_ANIMATION
    }

    fn progress(self) -> f32 {
        let duration = TOOL_EXPAND_ANIMATION.as_secs_f32().max(0.001);
        let t = (Instant::now()
            .saturating_duration_since(self.started_at)
            .as_secs_f32()
            / duration)
            .clamp(0.0, 1.0);
        let eased = ease_out_cubic(t);
        if self.expanding {
            eased
        } else {
            1.0 - eased
        }
    }
}

impl NeoismAgentMessage {
    pub(super) fn user(text: impl Into<String>) -> Self {
        Self::new(NeoismAgentMessageKind::User, text)
    }

    pub(super) fn assistant(text: impl Into<String>) -> Self {
        Self::new(NeoismAgentMessageKind::Assistant, text)
    }

    pub(super) fn reasoning(text: impl Into<String>) -> Self {
        let mut message = Self::new(NeoismAgentMessageKind::Reasoning, text);
        message.title = "Thinking".to_string();
        message
    }

    pub(super) fn tool(
        title: impl Into<String>,
        text: impl Into<String>,
        status: impl Into<String>,
        tool: impl Into<String>,
        output_kind: NeoismAgentOutputKind,
        lang: impl Into<String>,
        todos: Vec<NeoismAgentTodo>,
    ) -> Self {
        let mut message = Self::new(NeoismAgentMessageKind::Tool, text);
        message.title = title.into();
        message.status = status.into();
        message.tool = tool.into();
        message.output_kind = output_kind;
        message.lang = lang.into();
        message.todos = todos;
        message
    }

    pub(super) fn subtask(title: impl Into<String>, text: impl Into<String>) -> Self {
        let mut message = Self::new(NeoismAgentMessageKind::Subtask, text);
        message.title = title.into();
        message
    }

    pub(super) fn system(title: impl Into<String>, text: impl Into<String>) -> Self {
        let mut message = Self::new(NeoismAgentMessageKind::System, text);
        message.title = title.into();
        message
    }

    pub(super) fn compaction(text: impl Into<String>, reason: impl Into<String>) -> Self {
        let mut message = Self::new(NeoismAgentMessageKind::Compaction, text);
        message.title = "Compaction".to_string();
        message.status = reason.into();
        message
    }

    fn new(kind: NeoismAgentMessageKind, text: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            kind,
            title: String::new(),
            text: text.into(),
            status: String::new(),
            tool: String::new(),
            output_kind: NeoismAgentOutputKind::Text,
            lang: String::new(),
            line_offset: None,
            todos: Vec::new(),
            detail: String::new(),
            usage: None,
        }
    }
}

impl From<&NeoismAgentUsage> for UsageSnapshot {
    fn from(usage: &NeoismAgentUsage) -> Self {
        Self {
            input: usage.input,
            output: usage.output,
            reasoning: usage.reasoning,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            total: usage.total,
            cost_micros: usage.cost_micros,
            context_limit: usage.context_limit,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NeoismWordmarkState {
    pub hover: [f32; 6],
    pub last_frame_at: Option<Instant>,
    pub rect: Option<[f32; 4]>,
    pub click_started: Option<Instant>,
    pub click_pos: Option<(f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeoismAgentStreamingState {
    Idle,
    Thinking,
    Working,
    Generating,
    Compacting,
    WaitingSubagents,
    BackgroundTasks,
}

impl NeoismAgentStreamingState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Thinking => "Pondering",
            Self::Working => "Tinkering",
            Self::Generating => "Crafting",
            Self::Compacting => "Compacting",
            Self::WaitingSubagents => "Sub-agents working",
            Self::BackgroundTasks => "Background",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeoismAgentNoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NeoismAgentUiEvent {
    Notice {
        message: String,
        level: NeoismAgentNoticeLevel,
    },
    Dialog {
        title: String,
        body: String,
    },
    CloseTab,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NeoismAgentInputAttachment {
    Text {
        token: String,
        text: String,
    },
    Skill {
        token: String,
        name: String,
        description: String,
    },
    File {
        token: String,
        filename: String,
        url: String,
        mime: String,
    },
}

impl NeoismAgentInputAttachment {
    fn token(&self) -> &str {
        match self {
            Self::Text { token, .. }
            | Self::Skill { token, .. }
            | Self::File { token, .. } => token,
        }
    }
}

impl Default for NeoismWordmarkState {
    fn default() -> Self {
        Self {
            hover: [0.0; 6],
            last_frame_at: None,
            rect: None,
            click_started: None,
            click_pos: None,
        }
    }
}

pub struct NeoismAgentPane {
    pub(super) input: String,
    pub(super) messages: Vec<NeoismAgentMessage>,
    pub(super) mode: NeoismAgentMode,
    pub(super) agent: Option<String>,
    pub(super) model: String,
    pub(super) thinking: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) parent_session_id: Option<String>,
    pub(super) directory: Option<String>,
    #[allow(dead_code)]
    pub(super) server: String,
    pub(super) picker: Option<NeoismAgentPicker>,
    /// In-progress `/connect` provider-auth flow (catalog + chosen
    /// provider/method), held while any connect picker stage is open.
    connect: Option<connect::ConnectFlow>,
    file_mention_anchor: Option<usize>,
    #[allow(dead_code)]
    event_stream: Option<()>,
    #[allow(dead_code)]
    background_tx: (),
    #[allow(dead_code)]
    background_rx: (),
    cursor_rect: Option<[f32; 4]>,
    cursor_byte: usize,
    /// Soft-wrapped visual rows of the input (byte spans + per-boundary
    /// x offsets), registered by the renderer each frame — the same
    /// wrap the caret is placed with. Up/Down movement walks these;
    /// `input_wrap_len` guards against a frame of staleness after the
    /// text changes.
    input_wrap_rows: Vec<InputWrapRow>,
    input_wrap_len: usize,
    /// Sticky caret x carried between consecutive Up/Down presses so a
    /// run of vertical moves keeps aiming at the column it started in
    /// (see [`AgentInputBuffer::goal_x`]). Cleared by edits and
    /// horizontal moves.
    input_goal_x: Option<f32>,
    input_attachments: Vec<NeoismAgentInputAttachment>,
    ui_events: Vec<NeoismAgentUiEvent>,
    pending_user_prompts: Vec<String>,
    queued_prompt_count: usize,
    queued_prompt_preview: Option<String>,
    sent_history: Vec<String>,
    history_index: Option<usize>,
    history_draft: String,
    last_control_c_at: Option<Instant>,
    pub(super) abort_requested_at: Option<Instant>,
    expanded_tool_ids: BTreeSet<String>,
    selected_tool_group_child: Option<(String, String)>,
    tool_expand_anims: HashMap<String, ToolExpandAnimation>,
    tool_hit_rects: Vec<(String, [f32; 4])>,
    diff_scroll_rects: Vec<(String, [f32; 4], f32)>,
    diff_scroll_offsets: HashMap<String, f32>,
    permission_choice_hit_rects: Vec<(NeoismAgentPermissionChoice, [f32; 4])>,
    question_option_hit_rects: Vec<(usize, [f32; 4])>,
    /// Rect of the prompt-picker card (permission / question) drawn last
    /// frame — folded into the same occlusion path as the "/" picker so
    /// chrome text can't bleed through it.
    prompt_picker_rect: Option<[f32; 4]>,
    link_hit_rects: Vec<(String, [f32; 4])>,
    mermaid_raw_blocks: BTreeSet<u64>,
    usage_chip_rect: Option<[f32; 4]>,
    status_chip_rects: [Option<[f32; 4]>; 3],
    background_status_rect: Option<[f32; 4]>,
    background_task_details_expanded: bool,
    hover_link_target: Option<String>,
    /// (text, screen_rect, content_y_abs). The trailing `content_y_abs`
    /// is the line's position inside the *unscrolled* timeline content,
    /// so it survives scroll passes — anchor/focus reference it instead
    /// of the per-frame `selectable_lines` index, which would otherwise
    /// drift as scroll re-registers a different window of lines.
    selectable_lines: Vec<(String, [f32; 4], f32)>,
    /// Logical count of `selectable_lines` for the current frame; the Vec
    /// retains its `String` allocations across frames so per-frame "clear"
    /// is O(1) with no alloc/free churn.
    selectable_lines_len: usize,
    selection_anchor: Option<SelectionPoint>,
    selection_focus: Option<SelectionPoint>,
    timeline_scroll_px: f32,
    timeline_content_height_px: f32,
    timeline_viewport_height_px: f32,
    timeline_viewport_rect: Option<[f32; 4]>,
    pending_timeline_anchor: Option<TimelineAnchor>,
    pending_timeline_prepend_height_px: Option<f32>,
    timeline_last_scroll_at: Option<Instant>,
    timeline_velocity_px_s: f32,
    timeline_last_tick_at: Option<Instant>,
    /// Per-gesture inertia tuning captured at injection time. Trackpads keep
    /// the long-standing glide; external mouse wheels animate each notch but
    /// settle quickly instead of drifting on after the wheel stops.
    timeline_scroll_decay_tau: f32,
    timeline_scroll_stop_px_s: f32,
    timeline_measure_cache: RefCell<HashMap<TimelineMeasureKey, f32>>,
    markdown_blocks_cache:
        RefCell<HashMap<MarkdownBlocksKey, (CachedMarkdownBlocks, u64)>>,
    markdown_blocks_tick: Cell<u64>,
    timeline_layout_epoch: u64,
    timeline_layout_cache: RefCell<Option<TimelineLayoutCache>>,
    timeline_dirty_message_ids: BTreeSet<String>,
    timeline_dirty_message_indices: BTreeSet<usize>,
    /// First source row whose trace was observed live during this visit to the
    /// session. This is intentionally not persisted: revisiting a session
    /// presents its settled answer-only history, while the current visit keeps
    /// reasoning and tools visible even after the foreground turn goes idle.
    timeline_live_trace_start: Option<usize>,
    /// Id of the user message the live-trace window is anchored after; kept
    /// alongside the index so list replacements/prepends re-derive the same
    /// turn boundary instead of drifting to the latest turn.
    timeline_live_trace_anchor: Option<String>,
    /// Bumped on every timeline message mutation, including the
    /// dirty-mark paths that deliberately do NOT bump
    /// `timeline_layout_epoch` (those patch the view's layout cache
    /// in place). The virtual timeline keys its rebuild off this so
    /// streamed appends/edits reach the virtual surface — keying off
    /// the epoch alone left it holding the pre-stream message list,
    /// and its visible-source range then culled every new row (blank
    /// timeline while heights kept growing).
    timeline_content_revision: u64,
    timeline_history: AgentTimelineHistoryState,
    virtual_timeline: NeoismAgentVirtualTimeline,
    scrollbar_thumb_rect: Option<[f32; 4]>,
    scrollbar_track_rect: Option<[f32; 4]>,
    scrollbar_drag: Option<ScrollbarDrag>,
    streaming_state: NeoismAgentStreamingState,
    streaming_started_at: Option<Instant>,
    streaming_state_changed_at: Option<Instant>,
    streaming_tool_label: Option<String>,
    subagent_waiting_started_at: Option<Instant>,
    background_tasks_started_at: Option<Instant>,
    active_subagent_ids: BTreeSet<String>,
    active_subagent_started_at: HashMap<String, u64>,
    pending_permission: Option<NeoismAgentPendingPermission>,
    pending_permission_queue: VecDeque<NeoismAgentPendingPermission>,
    /// `/yolo` — while true, every permission request auto-answers
    /// "Yes" the moment it lands (session-scoped, client-side).
    skip_permissions: bool,
    pending_question: Option<NeoismAgentPendingQuestion>,
    pending_question_queue: VecDeque<NeoismAgentPendingQuestion>,
    model_context_limit: Option<u64>,
    model_options: Vec<NeoismAgentPickerOption>,
    recent_model_options: Vec<NeoismAgentPickerOption>,
    agent_options: Vec<NeoismAgentPickerOption>,
    skill_options: Vec<NeoismAgentPickerOption>,
    session_options: Vec<NeoismAgentPickerOption>,
    subagent_options: Vec<NeoismAgentPickerOption>,
    /// Queue of "the user just asked to do X" records the host drains
    /// between event cycles to perform the actual IO. See
    /// `outbound::OutboundAgentCommand`. Append-only from the shared
    /// pane; the host calls `drain_pending_outbound` to consume.
    pending_outbound: VecDeque<OutboundAgentCommand>,
    pub wordmark: NeoismWordmarkState,
    pub(super) side_panel: NeoismAgentSidePanel,
}

#[derive(Clone, Debug)]
struct NeoismAgentVirtualTimeline {
    adapter: VirtualAgentAdapter,
    surface: VirtualSurface,
    revision: u64,
    layout_epoch: u64,
    /// Mirror of the pane's `timeline_content_revision` at last sync.
    /// Dirty-mark message mutations (streamed deltas/appends) bump the
    /// content revision without touching the layout epoch.
    content_revision: u64,
    measured_layout_epoch: u64,
    measured_content_revision: u64,
    measured_width_bucket: i32,
    measured_scale_bucket: i32,
    measured_row_count: usize,
    measured_content_height_bits: u32,
    last_session_id: Option<String>,
    message_signatures: Vec<VirtualAgentMessageSignature>,
    last_visible_nodes: usize,
    last_visible_source_range: Option<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VirtualAgentMessageSignature {
    id: String,
    role: VirtualAgentRole,
    tool_name: Option<String>,
    markdown_hash: u64,
    markdown_len: u64,
}

impl Default for NeoismAgentVirtualTimeline {
    fn default() -> Self {
        Self {
            adapter: VirtualAgentAdapter::new("neoism-agent-pane"),
            surface: VirtualSurface::new(VirtualSurfaceConfig {
                overscan_px: 1200.0,
                warm_distance_px: 18_000.0,
                cold_distance_px: 80_000.0,
                tile_height_px: 768.0,
                max_retained_chunks: 4096,
                ..VirtualSurfaceConfig::default()
            }),
            revision: 0,
            layout_epoch: u64::MAX,
            content_revision: u64::MAX,
            measured_layout_epoch: u64::MAX,
            measured_content_revision: u64::MAX,
            measured_width_bucket: i32::MIN,
            measured_scale_bucket: i32::MIN,
            measured_row_count: 0,
            measured_content_height_bits: f32::NAN.to_bits(),
            last_session_id: None,
            message_signatures: Vec::new(),
            last_visible_nodes: 0,
            last_visible_source_range: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ScrollbarDrag {
    pointer_start_y: f32,
    scroll_offset_start: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarHit {
    Thumb,
    Track,
}

#[derive(Clone, Copy, Debug)]
struct SelectionPoint {
    /// Position in the unscrolled timeline content. Stable across scroll
    /// passes — that's the whole point.
    content_y: f32,
    x: f32,
}

impl PartialEq for SelectionPoint {
    fn eq(&self, other: &Self) -> bool {
        (self.content_y - other.content_y).abs() < 0.5 && (self.x - other.x).abs() < 0.01
    }
}

fn order_endpoints(
    a: SelectionPoint,
    b: SelectionPoint,
) -> (SelectionPoint, SelectionPoint) {
    if a.content_y < b.content_y
        || ((a.content_y - b.content_y).abs() < 0.5 && a.x <= b.x)
    {
        (a, b)
    } else {
        (b, a)
    }
}

/// Extract the substring of `text` that approximately falls within the
/// horizontal range `[left_x, right_x]` over the registered `rect`. We
/// don't have per-glyph metrics here, so we treat the line as evenly
/// spaced — close enough for monospace and acceptable for proportional.
fn slice_line_by_x(text: &str, rect: &[f32; 4], left_x: f32, right_x: f32) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let line_left = rect[0];
    let line_w = rect[2].max(1.0);
    let char_w = line_w / chars.len() as f32;
    let start_idx = (((left_x - line_left) / char_w).round() as isize)
        .clamp(0, chars.len() as isize) as usize;
    let end_idx = (((right_x - line_left) / char_w).round() as isize)
        .clamp(0, chars.len() as isize) as usize;
    if end_idx <= start_idx {
        return String::new();
    }
    chars[start_idx..end_idx].iter().collect()
}

fn virtual_agent_role(kind: NeoismAgentMessageKind) -> VirtualAgentRole {
    match kind {
        NeoismAgentMessageKind::User => VirtualAgentRole::User,
        NeoismAgentMessageKind::Assistant
        | NeoismAgentMessageKind::Reasoning
        | NeoismAgentMessageKind::Subtask
        | NeoismAgentMessageKind::Compaction => VirtualAgentRole::Assistant,
        NeoismAgentMessageKind::Tool => VirtualAgentRole::Tool,
        NeoismAgentMessageKind::System => VirtualAgentRole::System,
    }
}

fn virtual_agent_markdown(message: &NeoismAgentMessage) -> String {
    let mut out = String::new();
    if !message.title.trim().is_empty() {
        out.push_str("### ");
        out.push_str(message.title.trim());
        out.push('\n');
    }
    if !message.status.trim().is_empty()
        && matches!(message.kind, NeoismAgentMessageKind::Tool)
    {
        out.push_str("status: ");
        out.push_str(message.status.trim());
        out.push('\n');
    }
    if !message.text.trim().is_empty() {
        out.push_str(message.text.trim_end());
    }
    if !message.detail.trim().is_empty() && message.detail.trim() != message.text.trim() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(message.detail.trim_end());
    }
    if out.trim().is_empty() {
        match message.kind {
            NeoismAgentMessageKind::User => "user",
            NeoismAgentMessageKind::Assistant => "assistant",
            NeoismAgentMessageKind::Reasoning => "reasoning",
            NeoismAgentMessageKind::Tool => "tool",
            NeoismAgentMessageKind::System => "system",
            NeoismAgentMessageKind::Subtask => "subtask",
            NeoismAgentMessageKind::Compaction => "compaction",
        }
        .to_string()
    } else {
        out
    }
}

fn virtual_agent_message_signature(
    message: &VirtualAgentMessage,
) -> VirtualAgentMessageSignature {
    VirtualAgentMessageSignature {
        id: message.id.clone(),
        role: message.role,
        tool_name: message.tool_name.clone(),
        markdown_hash: hash_value(&message.markdown),
        markdown_len: message.markdown.len() as u64,
    }
}

impl Default for NeoismAgentPane {
    fn default() -> Self {
        let (background_tx, background_rx) = ((), ());
        Self {
            input: String::new(),
            messages: Vec::new(),
            mode: NeoismAgentMode::Build,
            agent: Some(DEFAULT_AGENT.to_string()),
            model: DEFAULT_MODEL.to_string(),
            thinking: None,
            session_id: None,
            parent_session_id: None,
            directory: None,
            server: String::new(),
            picker: None,
            connect: None,
            file_mention_anchor: None,
            event_stream: None,
            background_tx,
            background_rx,
            cursor_rect: None,
            cursor_byte: 0,
            input_wrap_rows: Vec::new(),
            input_wrap_len: 0,
            input_goal_x: None,
            input_attachments: Vec::new(),
            ui_events: Vec::new(),
            pending_user_prompts: Vec::new(),
            queued_prompt_count: 0,
            queued_prompt_preview: None,
            sent_history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            last_control_c_at: None,
            abort_requested_at: None,
            expanded_tool_ids: BTreeSet::new(),
            selected_tool_group_child: None,
            tool_expand_anims: HashMap::new(),
            tool_hit_rects: Vec::new(),
            diff_scroll_rects: Vec::new(),
            diff_scroll_offsets: HashMap::new(),
            permission_choice_hit_rects: Vec::new(),
            question_option_hit_rects: Vec::new(),
            prompt_picker_rect: None,
            link_hit_rects: Vec::new(),
            mermaid_raw_blocks: BTreeSet::new(),
            usage_chip_rect: None,
            status_chip_rects: [None; 3],
            background_status_rect: None,
            background_task_details_expanded: false,
            hover_link_target: None,
            selectable_lines: Vec::new(),
            selectable_lines_len: 0,
            selection_anchor: None,
            selection_focus: None,
            timeline_scroll_px: 0.0,
            timeline_content_height_px: 0.0,
            timeline_viewport_height_px: 0.0,
            timeline_viewport_rect: None,
            pending_timeline_anchor: None,
            pending_timeline_prepend_height_px: None,
            timeline_last_scroll_at: None,
            timeline_velocity_px_s: 0.0,
            timeline_last_tick_at: None,
            timeline_scroll_decay_tau: Self::TIMELINE_TRACKPAD_DECAY_TAU,
            timeline_scroll_stop_px_s: Self::TIMELINE_TRACKPAD_STOP_PX_S,
            timeline_measure_cache: RefCell::new(HashMap::new()),
            markdown_blocks_cache: RefCell::new(HashMap::new()),
            markdown_blocks_tick: Cell::new(0),
            timeline_layout_epoch: 0,
            timeline_layout_cache: RefCell::new(None),
            timeline_dirty_message_ids: BTreeSet::new(),
            timeline_dirty_message_indices: BTreeSet::new(),
            timeline_live_trace_start: None,
            timeline_live_trace_anchor: None,
            timeline_content_revision: 0,
            timeline_history: AgentTimelineHistoryState::default(),
            virtual_timeline: NeoismAgentVirtualTimeline::default(),
            scrollbar_thumb_rect: None,
            scrollbar_track_rect: None,
            scrollbar_drag: None,
            streaming_state: NeoismAgentStreamingState::Idle,
            streaming_started_at: None,
            streaming_state_changed_at: None,
            streaming_tool_label: None,
            subagent_waiting_started_at: None,
            background_tasks_started_at: None,
            active_subagent_ids: BTreeSet::new(),
            active_subagent_started_at: HashMap::new(),
            pending_permission: None,
            pending_permission_queue: VecDeque::new(),
            skip_permissions: false,
            pending_question: None,
            pending_question_queue: VecDeque::new(),
            model_context_limit: None,
            model_options: Vec::new(),
            recent_model_options: Vec::new(),
            agent_options: Vec::new(),
            skill_options: Vec::new(),
            session_options: Vec::new(),
            subagent_options: Vec::new(),
            pending_outbound: VecDeque::new(),
            wordmark: NeoismWordmarkState::default(),
            side_panel: NeoismAgentSidePanel::default(),
        }
    }
}

impl NeoismAgentPane {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_snapshot(&mut self, snapshot: NeoismAgentPaneSnapshot) {
        if let Some(input) = snapshot.input {
            self.input = input;
            self.cursor_byte = self.input.len();
        }
        if !snapshot.messages.is_empty() || self.messages.is_empty() {
            self.messages = snapshot.messages;
            self.timeline_layout_epoch = self.timeline_layout_epoch.wrapping_add(1);
            self.timeline_dirty_message_ids.clear();
            self.timeline_dirty_message_indices.clear();
            self.timeline_layout_cache.borrow_mut().take();
        }
        if let Some(agent) = snapshot.agent {
            self.agent = (!agent.trim().is_empty()).then_some(agent);
        }
        if let Some(model) = snapshot.model {
            self.model = model;
        }
        if let Some(directory) = snapshot.directory {
            self.directory = (!directory.trim().is_empty()).then_some(directory);
        }
        if let Some(session_id) = snapshot.session_id {
            let session_id = (!session_id.trim().is_empty()).then_some(session_id);
            if matches!(
                (&self.session_id, &session_id),
                (Some(current), Some(next)) if current != next
            ) {
                self.timeline_live_trace_start = None;
                self.timeline_live_trace_anchor = None;
            }
            self.session_id = session_id;
        }
        if let Some(streaming_state) = snapshot.streaming_state {
            self.note_streaming(streaming_state, None);
        }
        self.pending_permission = snapshot.pending_permission;
    }

    pub fn with_directory(directory: Option<String>) -> Self {
        let mut pane = Self {
            directory,
            ..Self::default()
        };
        pane.apply_config_defaults();
        pane
    }

    // -----------------------------------------------------------------
    // Daemon-event ingress helpers.
    //
    // These mirror the desktop pane's `drain_server_updates` match arms
    // so the web/wasm bridge can dispatch `AgentServerMessage` variants
    // straight into the pane state without rewriting bookkeeping. Each
    // helper takes already-decoded primitive values — the bridge does
    // the protocol→primitive translation so this crate doesn't depend
    // on `neoism-protocol`.
    // -----------------------------------------------------------------

    /// Acknowledge a freshly-created agent-server session id. Mirrors
    /// `ThreadCreated` / `ThreadSwitched`.
    pub fn set_session_id(&mut self, session_id: Option<String>) {
        self.session_id = session_id.and_then(|id| (!id.trim().is_empty()).then_some(id));
    }

    /// Clear the active session if it matches `session_id`. Mirrors
    /// `ThreadDeleted`.
    pub fn clear_session_id_if(&mut self, session_id: &str) {
        if self.session_id.as_deref() == Some(session_id) {
            self.session_id = None;
            self.timeline_live_trace_start = None;
            self.timeline_live_trace_anchor = None;
            self.invalidate_timeline_layout();
        }
    }

    /// Replace the directory (workspace root) label. The chrome's
    /// breadcrumbs / status pull from this.
    pub fn set_directory(&mut self, directory: Option<String>) {
        self.directory = directory.and_then(|d| (!d.trim().is_empty()).then_some(d));
    }

    /// Update the model's context-window limit, used by the usage chip.
    /// Mirrors the desktop `refresh_model_context_limit` /
    /// `ProviderState` flow.
    pub fn set_model_context_limit(&mut self, context_limit: Option<u64>) {
        self.model_context_limit = context_limit;
    }

    /// Stamp a fresh usage snapshot onto the latest assistant message
    /// so `latest_usage()` / `usage_summary_label()` light up. Mirrors
    /// `UsageUpdate`.
    pub fn apply_usage(&mut self, mut usage: NeoismAgentUsage) {
        if usage.context_limit.is_none() {
            usage.context_limit = self.model_context_limit;
        }
        let target = self
            .messages
            .iter_mut()
            .rposition(|m| {
                matches!(
                    m.kind,
                    NeoismAgentMessageKind::Assistant | NeoismAgentMessageKind::Reasoning
                )
            })
            .or_else(|| self.messages.iter().rposition(|m| !m.id.is_empty()));
        if let Some(index) = target {
            self.messages[index].usage = Some(usage);
            self.mark_timeline_message_dirty_at(index);
        } else {
            // No assistant message yet — stash a synthetic system row so
            // the usage chip still has a usage value to read from.
            let mut placeholder = NeoismAgentMessage::system("usage", "");
            placeholder.id = format!("usage-{}", self.messages.len());
            placeholder.usage = Some(usage);
            self.messages.push(placeholder);
            self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
        }
    }

    /// Replace the todo-list snapshot the pane renders. Stored on a
    /// dedicated `Tool` message of kind `Todos` so the timeline renderer
    /// already knows how to paint it. Mirrors `TodoUpdate`.
    pub fn apply_todos(&mut self, todos: Vec<NeoismAgentTodo>) {
        let todos_id = "todos-snapshot";
        let existing = self.messages.iter().position(|m| m.id == todos_id);
        if todos.is_empty() {
            if let Some(index) = existing {
                self.messages.remove(index);
                self.invalidate_timeline_layout();
            }
            return;
        }
        let mut row = NeoismAgentMessage::tool(
            "Todos",
            String::new(),
            "running",
            "todowrite",
            NeoismAgentOutputKind::Todos,
            "",
            todos,
        );
        row.id = todos_id.to_string();
        if let Some(index) = existing {
            self.messages[index] = row;
            self.mark_timeline_message_dirty_at(index);
        } else {
            self.messages.push(row);
            self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
        }
    }

    /// Update the queued-prompt indicator. Mirrors `QueueUpdate`.
    pub fn apply_queue(
        &mut self,
        count: u32,
        preview: Option<String>,
        started_at: Option<u64>,
    ) {
        let decision = status_policy::queue_status_decision(
            count as usize,
            preview,
            started_at,
            self.is_streaming(),
        );
        self.queued_prompt_count = decision.count;
        self.queued_prompt_preview = decision.preview;
        if decision.should_enter_thinking {
            self.note_streaming(NeoismAgentStreamingState::Thinking, None);
        }
    }

    /// Push a tool-card row into the timeline. `tool_use_id` is the
    /// stable id the agent-server uses; we reuse it as the message id
    /// so a follow-up `ToolUseResult` can find and update the same row.
    /// Mirrors `ToolUseRequest`.
    pub fn upsert_tool_card(
        &mut self,
        tool_use_id: String,
        tool: String,
        title: String,
        status: String,
        detail: String,
        output_kind: NeoismAgentOutputKind,
        lang: String,
    ) {
        let mut row = NeoismAgentMessage::tool(
            title,
            String::new(),
            status,
            tool,
            output_kind,
            lang,
            Vec::new(),
        );
        row.id = tool_use_id.clone();
        row.detail = detail;
        self.upsert_part_message(row);
    }

    /// Finalize the matching tool-card row with the daemon's reported
    /// status / output. Mirrors `ToolUseResult`.
    pub fn finalize_tool_card(
        &mut self,
        tool_use_id: &str,
        status: &str,
        output: Option<String>,
        error: Option<String>,
    ) {
        let Some(index) = self.messages.iter().position(|m| m.id == tool_use_id) else {
            // The card didn't exist (e.g. a tool that didn't gate via
            // permission and so never hit `ToolUseRequest`). Synthesize
            // a one-shot row so the user still sees what ran.
            let mut row = NeoismAgentMessage::tool(
                String::new(),
                output.clone().unwrap_or_default(),
                status.to_string(),
                String::new(),
                NeoismAgentOutputKind::Text,
                "",
                Vec::new(),
            );
            row.id = tool_use_id.to_string();
            if let Some(err) = error {
                row.detail = err;
            }
            self.messages.push(row);
            self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
            return;
        };
        let message = &mut self.messages[index];
        message.status = status.to_string();
        if let Some(out) = output {
            if !out.trim().is_empty() {
                message.text = out;
            }
        }
        if let Some(err) = error {
            message.detail = err;
        }
        self.mark_timeline_message_and_next_dirty_at(index);
    }

    /// Record a daemon-proposed file edit as a tool-card row carrying
    /// the unified-diff patch. Mirrors `EditProposed`.
    pub fn record_edit_proposed(
        &mut self,
        edit_id: String,
        path: String,
        patch: String,
        tool: Option<String>,
    ) {
        let tool_name = tool.unwrap_or_else(|| "edit".to_string());
        let mut row = NeoismAgentMessage::tool(
            path.clone(),
            patch,
            "pending",
            tool_name,
            NeoismAgentOutputKind::Code,
            "diff",
            Vec::new(),
        );
        row.id = edit_id;
        row.detail = path;
        self.upsert_part_message(row);
    }

    /// Mark a previously-proposed edit as applied. Mirrors
    /// `EditApplied`.
    pub fn record_edit_applied(&mut self, edit_id: &str, _bytes_written: u64) {
        if let Some(index) = self.messages.iter().position(|m| m.id == edit_id) {
            self.messages[index].status = "completed".to_string();
            self.mark_timeline_message_and_next_dirty_at(index);
        }
    }

    /// Mark a previously-proposed edit as rejected. Mirrors
    /// `EditRejected`.
    pub fn record_edit_rejected(&mut self, edit_id: &str, reason: Option<String>) {
        if let Some(index) = self.messages.iter().position(|m| m.id == edit_id) {
            self.messages[index].status = "error".to_string();
            if let Some(reason) = reason {
                if !reason.trim().is_empty() {
                    self.messages[index].detail = reason;
                }
            }
            self.mark_timeline_message_and_next_dirty_at(index);
        }
    }

    /// Apply a provider-state snapshot. Mirrors `ProviderState`.
    pub fn apply_provider_state(
        &mut self,
        provider_id: Option<String>,
        model: Option<String>,
        agent: Option<String>,
        thinking: Option<String>,
        context_limit: Option<u64>,
    ) {
        let _ = provider_id; // surface-only today; reserved for richer UI
        if let Some(model) = model {
            self.set_model_local(model);
        }
        if let Some(agent) = agent {
            self.set_agent_local(agent);
        }
        if let Some(thinking) = thinking {
            self.set_thinking_local(thinking);
        }
        self.set_model_context_limit(context_limit);
    }

    /// Record a session-idle transition. Mirrors `SessionIdle`.
    pub fn note_session_idle(&mut self) {
        if self.is_streaming() {
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
        }
        self.queued_prompt_count = 0;
        self.queued_prompt_preview = None;
        self.abort_requested_at = None;
    }

    pub fn note_dequeued_prompt(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.consume_dequeued_prompt_preview(&text);
        let current_turn_start = self
            .messages
            .iter()
            .rposition(|message| message.kind != NeoismAgentMessageKind::User)
            .map(|index| index + 1)
            .unwrap_or(0);
        if self.messages[current_turn_start..]
            .iter()
            .any(|message| is_user_prompt(message, &text))
        {
            return;
        }
        self.messages.push(NeoismAgentMessage::user(text));
        self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
    }

    fn consume_dequeued_prompt_preview(&mut self, text: &str) {
        if self.queued_prompt_count > 0 {
            self.queued_prompt_count = self.queued_prompt_count.saturating_sub(1);
        }
        let preview_matches = self
            .queued_prompt_preview
            .as_deref()
            .is_some_and(|preview| preview.trim() == text.trim());
        if self.queued_prompt_count == 0 || preview_matches {
            self.queued_prompt_preview = None;
        }
    }

    /// Record a free-form notice. Mirrors `Notice`.
    pub fn push_notice_event(
        &mut self,
        title: String,
        body: String,
        level: NeoismAgentNoticeLevel,
    ) {
        // System row mirrors the desktop pane's `system_message` path
        // for inline notices; the toast/banner pipeline picks the level
        // off `ui_events`.
        self.system_message(title, body.clone());
        self.push_notice(body, level);
    }

    /// Record a subagent status/activity event. Mirrors
    /// `SubagentUpdate`.
    pub fn note_subagent_event(
        &mut self,
        session_id: String,
        status: BranchStatus,
        title: Option<String>,
        agent: Option<String>,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) {
        if session_id.is_empty() {
            return;
        }
        self.upsert_live_subagent_entry(&session_id, title, agent);
        self.side_panel.set_branch_activity_tool(
            session_id.clone(),
            status,
            current_tool,
            started_at,
        );
        self.note_subagent_runtime(session_id.clone(), status, started_at);
        if matches!(
            status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) {
            self.set_task_message_status(&session_id, "running");
        } else {
            let status_label = match status {
                BranchStatus::Completed => "completed",
                BranchStatus::Stopped => "error",
                _ => "running",
            };
            self.set_task_message_status(&session_id, status_label);
        }
        self.sync_subagent_waiting_clock();
    }

    /// Drive a compaction lifecycle event. Mirrors `Compaction`.
    pub fn note_compaction(
        &mut self,
        phase: CompactionPhase,
        text: Option<String>,
        reason: Option<String>,
    ) {
        match phase {
            CompactionPhase::Started => {
                let reason_label = reason.unwrap_or_else(|| "auto".to_string());
                self.start_compaction_message(String::new(), reason_label);
                self.note_streaming(NeoismAgentStreamingState::Compacting, None);
            }
            CompactionPhase::Delta => {
                if let Some(text) = text {
                    self.apply_compaction_delta(&text);
                }
            }
            CompactionPhase::Ended => {
                let summary = text.unwrap_or_default();
                let kind = reason.unwrap_or_else(|| "done".to_string());
                self.finish_compaction_message(&summary, &kind);
                if self.is_streaming() {
                    self.note_streaming(NeoismAgentStreamingState::Idle, None);
                }
            }
        }
    }

    /// Replace the timeline with a freshly-fetched history page.
    /// Mirrors `HistoryChunk`. Resets streaming state since history
    /// chunks are static.
    pub fn apply_history(&mut self, messages: Vec<NeoismAgentMessage>) {
        if messages.is_empty() && !self.messages.is_empty() {
            return;
        }
        self.messages = messages;
        self.rebase_current_turn_trace();
        self.invalidate_timeline_layout();
        self.clamp_timeline_scroll();
    }
}

/// Compaction lifecycle phase ingested via [`NeoismAgentPane::note_compaction`].
/// Mirrors `neoism_protocol::agent::CompactionPhase` so the wasm bridge
/// can translate one-for-one without exposing the protocol crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionPhase {
    Started,
    Delta,
    Ended,
}

#[allow(dead_code)]
fn fuzzy_score(value: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(100 - value.matches('/').count() as i64);
    }
    let value_lower = value.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if value_lower.contains(&query_lower) {
        return Some(1_000 - value_lower.find(&query_lower).unwrap_or(0) as i64);
    }
    let mut score = 0i64;
    let mut chars = query_lower.chars();
    let mut current = chars.next()?;
    for (index, ch) in value_lower.chars().enumerate() {
        if ch == current {
            score += 20 - i64::try_from(index).unwrap_or_default().min(20);
            if let Some(next) = chars.next() {
                current = next;
            } else {
                return Some(score);
            }
        }
    }
    None
}

#[allow(dead_code)]
fn file_mention_description(display: &str, kind: &str) -> String {
    display
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| format!("{kind} in {parent}"))
        .unwrap_or_else(|| kind.to_string())
}

fn compact_directory_label(path: &str) -> String {
    let mut label = path.trim().replace('\\', "/");
    if label.is_empty() {
        return "-".to_string();
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = home.trim_end_matches('/').replace('\\', "/");
        if label == home {
            label = "~".to_string();
        } else if let Some(rest) = label.strip_prefix(&format!("{home}/")) {
            label = format!("~/{rest}");
        }
    }
    if !label.ends_with('/') {
        label.push('/');
    }
    if label.chars().count() <= 44 {
        return label;
    }
    let trimmed = label.trim_end_matches('/');
    let parts = trimmed
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() <= 3 {
        return label;
    }
    let tail_start = parts.len().saturating_sub(2);
    let tail = parts[tail_start..].join("/");
    if trimmed.starts_with("~/") {
        format!("~/.../{tail}/")
    } else if trimmed.starts_with('/') {
        format!("/.../{tail}/")
    } else {
        format!("{}/.../{tail}/", parts[0])
    }
}

impl NeoismAgentMessage {
    pub(super) fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
}

fn merge_part_message(
    existing: NeoismAgentMessage,
    mut incoming: NeoismAgentMessage,
) -> NeoismAgentMessage {
    let preserve_terminal_task_status = same_task_message_id(&existing, &incoming)
        && is_terminal_task_status(&existing.status)
        && incoming.status == "running";
    let terminal_task_status = existing.status.clone();
    if incoming.usage.is_none() {
        incoming.usage = existing.usage;
    }
    if matches!(
        incoming.kind,
        NeoismAgentMessageKind::Assistant | NeoismAgentMessageKind::Reasoning
    ) && matches!(
        existing.kind,
        NeoismAgentMessageKind::Assistant | NeoismAgentMessageKind::Reasoning
    ) {
        if incoming.text.is_empty() || existing.text.starts_with(&incoming.text) {
            incoming.text = existing.text.clone();
        }
    }
    if incoming.kind == NeoismAgentMessageKind::Tool
        && existing.kind == NeoismAgentMessageKind::Tool
    {
        if incoming.text.trim().is_empty() {
            incoming.text = existing.text;
        }
        if incoming.detail.trim().is_empty() {
            incoming.detail = existing.detail;
        }
        if incoming.todos.is_empty() {
            incoming.todos = existing.todos;
        }
        if incoming.output_kind == NeoismAgentOutputKind::Text
            && existing.output_kind != NeoismAgentOutputKind::Text
        {
            incoming.output_kind = existing.output_kind;
        }
        if incoming.lang.is_empty() {
            incoming.lang = existing.lang;
        }
        if incoming.line_offset.is_none() {
            incoming.line_offset = existing.line_offset;
        }
        if preserve_terminal_task_status {
            incoming.status = terminal_task_status;
            rewrite_task_status_markers(&mut incoming.text, &incoming.status);
            rewrite_task_status_markers(&mut incoming.detail, &incoming.status);
        }
    }
    incoming
}

fn same_streamed_part_identity(a: &NeoismAgentMessage, b: &NeoismAgentMessage) -> bool {
    same_nonempty_id(a, b) || same_task_message_id(a, b)
}

fn same_task_message_id(a: &NeoismAgentMessage, b: &NeoismAgentMessage) -> bool {
    a.kind == NeoismAgentMessageKind::Tool
        && b.kind == NeoismAgentMessageKind::Tool
        && a.tool == "task"
        && b.tool == "task"
        && task_id_from_task_message(a).is_some_and(|task_id| {
            task_id_from_task_message(b).as_deref() == Some(task_id.as_str())
        })
}

fn is_terminal_task_status(status: &str) -> bool {
    matches!(status, "completed" | "error")
}

fn rewrite_task_status_markers(field: &mut String, status: &str) {
    for marker in [
        "status: running",
        "status: completed",
        "status: error",
        "status: stopped",
        "status: failed",
    ] {
        if field.contains(marker) {
            *field = field.replace(marker, &format!("status: {status}"));
            return;
        }
    }
}

#[allow(dead_code)]
fn same_message_identity(a: &NeoismAgentMessage, b: &NeoismAgentMessage) -> bool {
    // User messages are keyed by their text. The locally-pushed copy has an
    // empty id; the server's refresh assigns an id later — without this
    // special case the user prompt loses its prior_index slot and gets
    // sorted past the assistant reply on idle.
    if a.kind == NeoismAgentMessageKind::User && b.kind == NeoismAgentMessageKind::User {
        return a.text.trim() == b.text.trim();
    }
    if !a.id.is_empty() || !b.id.is_empty() {
        return !a.id.is_empty() && a.id == b.id;
    }
    a.kind == b.kind && a.title == b.title && a.text == b.text
}

#[allow(dead_code)]
fn same_nonempty_id(a: &NeoismAgentMessage, b: &NeoismAgentMessage) -> bool {
    !a.id.is_empty() && a.id == b.id
}

#[allow(dead_code)]
fn is_streamed_live_part(message: &NeoismAgentMessage) -> bool {
    matches!(
        message.kind,
        NeoismAgentMessageKind::Assistant
            | NeoismAgentMessageKind::Reasoning
            | NeoismAgentMessageKind::Tool
            | NeoismAgentMessageKind::Subtask
    )
}

fn part_delta_message_kind(kind: Option<&str>) -> NeoismAgentMessageKind {
    match kind {
        Some("reasoning" | "thinking") => NeoismAgentMessageKind::Reasoning,
        _ => NeoismAgentMessageKind::Assistant,
    }
}

#[allow(dead_code)]
fn is_user_prompt(message: &NeoismAgentMessage, prompt: &str) -> bool {
    message.kind == NeoismAgentMessageKind::User && message.text.trim() == prompt.trim()
}

pub fn branch_status_from_runtime(status: &str) -> BranchStatus {
    match status {
        "completed" | "idle" => BranchStatus::Completed,
        "blocked" | "retry" => BranchStatus::WaitingPermission,
        "error" | "failed" | "stopped" => BranchStatus::Stopped,
        _ => BranchStatus::Active,
    }
}

pub fn task_message_status_from_runtime(status: &str) -> Option<&'static str> {
    match status {
        "completed" | "idle" => Some("completed"),
        "error" | "stopped" | "failed" => Some("error"),
        "running" | "active" | "busy" | "blocked" | "retry" => Some("running"),
        _ => None,
    }
}

fn task_message_status_from_branch(status: BranchStatus) -> Option<&'static str> {
    match status {
        BranchStatus::Completed => Some("completed"),
        BranchStatus::Stopped => Some("error"),
        BranchStatus::WaitingPermission => Some("running"),
        BranchStatus::Active => None,
    }
}

fn instant_from_epoch_millis(epoch_millis: u64) -> Instant {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = epoch_millis;
        return Instant::now();
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let now_millis = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let elapsed = now_millis.saturating_sub(epoch_millis);
        Instant::now()
            .checked_sub(Duration::from_millis(elapsed))
            .unwrap_or_else(Instant::now)
    }
}

#[allow(dead_code)]
fn task_id_from_task_message(message: &NeoismAgentMessage) -> Option<String> {
    message
        .detail
        .lines()
        .chain(message.text.lines())
        .find_map(|line| {
            line.trim()
                .strip_prefix("task_id:")
                .and_then(|rest| rest.split_whitespace().next())
                .map(str::to_string)
        })
}

fn background_job_id_from_message(message: &NeoismAgentMessage) -> Option<String> {
    message
        .detail
        .lines()
        .chain(message.text.lines())
        .find_map(|line| {
            line.trim()
                .strip_prefix("job_id:")
                .or_else(|| line.trim().strip_prefix("jobId:"))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn background_completion_job_id_from_message(
    message: &NeoismAgentMessage,
) -> Option<String> {
    let text = format!("{}\n{}", message.detail, message.text);
    if !text.contains("background shell task has finished")
        && !text.contains("background task has finished")
        && message.tool != "background_task_result"
    {
        return None;
    }
    if !text.contains("status: completed") && message.status != "completed" {
        return None;
    }
    background_job_id_from_message(message)
}

fn running_background_task_count(messages: &[NeoismAgentMessage]) -> usize {
    use std::collections::BTreeSet;

    let completed = messages
        .iter()
        .filter_map(background_completion_job_id_from_message)
        .collect::<BTreeSet<_>>();

    messages
        .iter()
        .filter(|message| message.kind == NeoismAgentMessageKind::Tool)
        .filter(|message| message.tool == "background_task")
        .filter(|message| message.status == "running")
        .filter_map(background_job_id_from_message)
        .filter(|job_id| !completed.contains(job_id))
        .collect::<BTreeSet<_>>()
        .len()
}

fn active_background_task_summaries(messages: &[NeoismAgentMessage]) -> Vec<String> {
    use std::collections::BTreeSet;

    let completed = messages
        .iter()
        .filter_map(background_completion_job_id_from_message)
        .collect::<BTreeSet<_>>();

    messages
        .iter()
        .filter(|message| message.kind == NeoismAgentMessageKind::Tool)
        .filter(|message| message.tool == "background_task")
        .filter(|message| message.status == "running")
        .filter_map(|message| {
            let job_id = background_job_id_from_message(message)?;
            if completed.contains(&job_id) {
                return None;
            }
            let command = background_task_command_from_message(message)
                .unwrap_or_else(|| message.title.as_str().to_string());
            Some(format!("{} · {} · {}", job_id, message.status, command))
        })
        .collect()
}

fn background_task_command_from_message(message: &NeoismAgentMessage) -> Option<String> {
    let text = if message.detail.trim().is_empty() {
        message.text.as_str()
    } else {
        message.detail.as_str()
    };
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        if let Some(command) = value
            .get("command")
            .or_else(|| value.get("description"))
            .and_then(Value::as_str)
        {
            let command = command.trim();
            if !command.is_empty() {
                return Some(command.to_string());
            }
        }
    }
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("command:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn hash_value<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn hash_agent_message_text_for_measure(text: &str) -> u64 {
    const FULL_HASH_LIMIT: usize = 24 * 1024;
    if text.len() <= FULL_HASH_LIMIT {
        return hash_value(&text);
    }
    let bytes = text.as_bytes();
    let mut hasher = DefaultHasher::new();
    bytes.len().hash(&mut hasher);
    bytes[..bytes.len().min(4096)].hash(&mut hasher);
    bytes[bytes.len().saturating_sub(8192)..].hash(&mut hasher);
    hasher.finish()
}

fn f32_measure_bucket(value: f32) -> i32 {
    (value.max(0.0) * 4.0).round() as i32
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

#[cfg(test)]
mod tests;

fn slash_options() -> Vec<NeoismAgentPickerOption> {
    crate::panels::agent_pane::command_controller::slash_options()
}

pub mod perf {
    use web_time::Instant;
    pub fn enabled() -> bool {
        false
    }
    pub fn now() -> Option<Instant> {
        None
    }
    pub fn elapsed_us(started: Option<Instant>) -> Option<u128> {
        started.map(|s| Instant::now().saturating_duration_since(s).as_micros())
    }
}
