use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use neoism_backend::clipboard::ClipboardImage;
use neoism_ui::panels::agent_pane::input_controller::{self, AgentInputBuffer};
use neoism_ui::panels::agent_pane::interaction_policy;
use neoism_ui::panels::agent_pane::outbound::OutboundAgentCommand;
use neoism_ui::panels::agent_pane::permission_policy::{self, PermissionReplyStart};
use neoism_ui::panels::agent_pane::state::{
    branch_status_from_runtime, task_message_status_from_runtime,
};
use neoism_ui::panels::agent_pane::status_policy;
use neoism_ui::panels::agent_pane::timeline_scroll_policy::ctrl_u_d_scroll_delta;
use neoism_ui::panels::agent_pane::usage_policy::{self, UsageSnapshot};
use serde_json::{json, Value};

use super::api::{
    api_request_json, delete_session, fetch_agent_options, fetch_config_defaults,
    fetch_model_context_limit, fetch_model_options, fetch_session_entries,
    fetch_session_goal, fetch_session_options, fetch_session_statuses,
    fetch_skill_options, fetch_subagent_entries, fetch_subagent_options,
    neoism_agent_server, rename_session, set_session_pinned,
};
use super::commands::slash_options;
use super::picker::{NeoismAgentPicker, NeoismAgentPickerKind, NeoismAgentPickerOption};
use super::side_panel::{
    BranchStatus, NeoismAgentSessionEntry, NeoismAgentSidePanel, SessionGoal,
};
use super::updates::{
    start_session_event_stream, AgentSessionEventStream, AgentSessionUpdate,
};

const DEFAULT_AGENT: &str = "build";
const DEFAULT_MODEL: &str = "";
const FILE_MENTION_LIMIT: usize = 10;
const FILE_MENTION_SCAN_LIMIT: usize = 512;
const FILE_MENTION_VISIT_LIMIT: usize = 6000;
const FILE_MENTION_MAX_DEPTH: usize = 8;
const MAX_INLINE_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
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
    #[allow(dead_code)]
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

fn usage_snapshot(usage: &NeoismAgentUsage) -> UsageSnapshot {
    UsageSnapshot {
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
    fn reply(self) -> &'static str {
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
    std::rc::Rc<Vec<crate::neoism::view::markdown::AssistantMarkdownBlock>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimelineMeasureKey {
    id: u64,
    kind: NeoismAgentMessageKind,
    output_kind: NeoismAgentOutputKind,
    width_bucket: i32,
    scale_bucket: i32,
    tool_expanded: bool,
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

pub(crate) type TimelineLayoutCache =
    neoism_ui::panels::agent_pane::view::timeline::TimelineLayoutCache<
        NeoismAgentMessage,
    >;

#[derive(Default)]
pub(crate) struct TimelineDirtyMarks {
    pub ids: BTreeSet<String>,
    pub indices: BTreeSet<usize>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TimelineAnchor {
    content_y: f32,
    screen_y: f32,
}

#[derive(Clone, Copy, Debug)]
struct ToolExpandAnimation {
    started_at: Instant,
    expanding: bool,
}

#[derive(Clone, Debug)]
pub(super) struct AgentTimelineHistoryState {
    pub oldest_loaded_cursor: Option<String>,
    pub has_older: bool,
    pub loading_older: bool,
    pub last_requested_session_id: Option<String>,
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

impl ToolExpandAnimation {
    fn is_active(self) -> bool {
        self.started_at.elapsed() < TOOL_EXPAND_ANIMATION
    }

    fn progress(self) -> f32 {
        let duration = TOOL_EXPAND_ANIMATION.as_secs_f32().max(0.001);
        let t = (self.started_at.elapsed().as_secs_f32() / duration).clamp(0.0, 1.0);
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

pub(crate) enum NeoismAgentBackgroundUpdate {
    CompactFinished,
    CompactFailed(String),
    SidePanelSessionsRefreshed(Vec<NeoismAgentSessionEntry>),
    SidePanelSubagentsRefreshed(Vec<NeoismAgentSessionEntry>),
    /// Semantic session-search results for `query`, fetched off-thread.
    /// `hits: None` means the server reports semantic search unavailable
    /// (no vector backend / no embeddings key) — stop asking this run.
    SemanticSessionHits {
        query: String,
        hits: Option<Vec<super::api::NeoismAgentSemanticSessionHit>>,
    },
    /// The session's persistent goal, refetched in the background. The
    /// `session_id` it was fetched for is carried so a stale result that
    /// landed after a session switch is dropped instead of mislabelling
    /// the new session.
    SessionGoalRefreshed {
        session_id: String,
        goal: Option<SessionGoal>,
    },
    /// An older history page, fetched off the UI thread. `messages` is in
    /// ascending (oldest-first) order, ready to prepend. `raw_count` is the
    /// number of stored messages the server returned (vs expanded blocks),
    /// and together with `requested_limit` tells the applier whether more
    /// history remains (a short page means we hit the start of the transcript).
    OlderTimelineLoaded {
        session_id: String,
        messages: Vec<NeoismAgentMessage>,
        raw_count: usize,
        requested_limit: usize,
    },
    /// The older-history fetch failed; carries the session it was for so a
    /// stale failure that raced a session switch is ignored.
    OlderTimelineFailed {
        session_id: String,
        error: String,
    },
    /// An `/undo` or `/redo` completed off the UI thread. Runs on a background
    /// thread (the revert POST plus a full message re-fetch can be slow on a
    /// large session, and doing it inline froze the UI so ESC couldn't be
    /// processed). `title` is "Undo"/"Redo" for the confirmation line.
    SessionHistoryApplied {
        session_id: String,
        title: String,
        messages: Vec<NeoismAgentMessage>,
    },
    /// An `/undo` or `/redo` failed off the UI thread.
    SessionHistoryFailed {
        session_id: String,
        title: String,
        error: String,
    },
    /// An auto-completing OAuth `/connect` flow (e.g. OpenAI, GitHub Copilot)
    /// finished on a background thread — the browser callback was captured and
    /// the token exchanged/stored.
    ConnectOauthFinished {
        provider_name: String,
    },
    /// An auto-completing OAuth `/connect` flow failed (timed out, cancelled in
    /// the browser, or the exchange errored).
    ConnectOauthFailed {
        provider_name: String,
        error: String,
    },
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
    pub(super) server: String,
    pub(super) picker: Option<NeoismAgentPicker>,
    /// Active `/connect` provider-auth flow. `Some` while any of the connect
    /// pickers (provider list / auth method / secret entry) is open, carrying
    /// the fetched catalog and the in-progress provider/method selection.
    pub(super) connect: Option<connect::ConnectFlow>,
    /// Active inline rename of a `/sessions` picker row: `(session_id,
    /// buffer)`. `Some` diverts typed keys into the buffer until the user
    /// commits (Enter) or cancels (Esc).
    pub(super) session_rename: Option<(String, String)>,
    recent_model_options: Vec<NeoismAgentPickerOption>,
    skill_options: Vec<NeoismAgentPickerOption>,
    skill_options_directory: Option<Option<String>>,
    file_mention_anchor: Option<usize>,
    event_stream: Option<AgentSessionEventStream>,
    background_tx: Sender<NeoismAgentBackgroundUpdate>,
    background_rx: Receiver<NeoismAgentBackgroundUpdate>,
    /// Semantic session-search coalescing: at most one fetch in flight; a
    /// query typed meanwhile waits in `semantic_pending_query` and is kicked
    /// when the current fetch lands. `semantic_unavailable` latches once the
    /// server says the feature is off so we stop asking.
    pub(crate) semantic_in_flight: bool,
    pub(crate) semantic_pending_query: Option<String>,
    pub(crate) semantic_unavailable: bool,
    /// Ungrouped-into-`picker` copy of the last-fetched `/sessions` picker
    /// options, kept so semantic hits can be merged in without refetching.
    pub(crate) session_picker_base: Vec<NeoismAgentPickerOption>,
    cursor_rect: Option<[f32; 4]>,
    cursor_byte: usize,
    /// Byte spans of the input's soft-wrapped visual rows, registered
    /// by the renderer each frame (same wrap the caret is placed
    /// with). Up/Down movement walks these; `input_wrap_len` guards
    /// against a frame of staleness after the text changes.
    input_wrap_ranges: Vec<(usize, usize)>,
    input_wrap_len: usize,
    input_attachments: Vec<NeoismAgentInputAttachment>,
    ui_events: Vec<NeoismAgentUiEvent>,
    pending_user_prompts: Vec<String>,
    /// `(expanded, composer echo)` pairs for prompts sent with paste
    /// attachments: the server echoes the expanded text back, the
    /// transcript shows the compact `[pasted N lines]` form.
    prompt_echo_aliases: Vec<(String, String)>,
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
    /// Logical count of `selectable_lines` valid for the current frame. The
    /// Vec retains its `String` allocations across frames (reused in place)
    /// so the per-frame "clear" is just resetting this to 0 — no per-line
    /// alloc/free churn, which in debug was costing ~1.5ms/frame.
    selectable_lines_len: usize,
    selection_anchor: Option<SelectionPoint>,
    selection_focus: Option<SelectionPoint>,
    timeline_scroll_px: f32,
    timeline_content_height_px: f32,
    timeline_viewport_height_px: f32,
    timeline_viewport_rect: Option<[f32; 4]>,
    pending_timeline_anchor: Option<TimelineAnchor>,
    pub(super) pending_timeline_prepend_height_px: Option<f32>,
    /// Count of messages just prepended by history pagination, awaiting an
    /// incremental layout fold (consumed by `take_timeline_prepend`).
    pub(super) pending_timeline_prepend_count: Option<usize>,
    /// When the last older-history request fired. Rate-limits pagination so a
    /// page that adds little height (e.g. collapsed tool groups) can't trigger
    /// a back-to-back load cascade that drags the whole transcript in at once.
    pub(super) timeline_last_older_request_at: Option<Instant>,
    timeline_last_scroll_at: Option<Instant>,
    timeline_velocity_px_s: f32,
    timeline_last_tick_at: Option<Instant>,
    /// Per-gesture inertia tuning captured at injection time. Trackpads keep
    /// the long-standing glide; external mouse wheels animate each notch but
    /// settle quickly instead of drifting on after the wheel stops.
    timeline_scroll_decay_tau: f32,
    timeline_scroll_stop_px_s: f32,
    timeline_measure_cache: RefCell<HashMap<TimelineMeasureKey, f32>>,
    // Value is `(blocks, last_used_tick)`. The tick drives true LRU eviction:
    // every cache *hit* (per visible card, per frame) bumps the entry's tick,
    // so the actively-scrolled working set stays resident regardless of how
    // long the transcript is. Without this the cache was FIFO — stale
    // partial-text keys minted while streaming flooded the cap and evicted the
    // oldest *finalized* cards, so scrolling up through a long history
    // re-parsed + re-shaped them on the UI thread inside the draw loop.
    markdown_blocks_cache:
        RefCell<HashMap<MarkdownBlocksKey, (CachedMarkdownBlocks, u64)>>,
    markdown_blocks_tick: std::cell::Cell<u64>,
    timeline_layout_epoch: u64,
    timeline_layout_cache: RefCell<Option<TimelineLayoutCache>>,
    timeline_dirty_message_ids: BTreeSet<String>,
    timeline_dirty_message_indices: BTreeSet<usize>,
    /// First source row whose trace was observed live during this visit to the
    /// session. It is cleared on session navigation, never persisted.
    timeline_live_trace_start: Option<usize>,
    pub(super) timeline_history: AgentTimelineHistoryState,
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
    pending_outbound: VecDeque<OutboundAgentCommand>,
    model_context_limit: Option<u64>,
    pub wordmark: NeoismWordmarkState,
    pub(super) side_panel: NeoismAgentSidePanel,
    perf_frame: AgentPanePerfFrame,
}

#[derive(Default)]
struct AgentPanePerfFrame {
    last_render_at: Option<Instant>,
    frames: u64,
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
pub(crate) struct SelectionPoint {
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

impl Default for NeoismAgentPane {
    fn default() -> Self {
        let (background_tx, background_rx) = mpsc::channel();
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
            server: neoism_agent_server(),
            picker: None,
            connect: None,
            session_rename: None,
            recent_model_options: Vec::new(),
            skill_options: Vec::new(),
            skill_options_directory: None,
            file_mention_anchor: None,
            event_stream: None,
            background_tx,
            background_rx,
            semantic_in_flight: false,
            semantic_pending_query: None,
            semantic_unavailable: false,
            session_picker_base: Vec::new(),
            cursor_rect: None,
            cursor_byte: 0,
            input_wrap_ranges: Vec::new(),
            input_wrap_len: 0,
            input_attachments: Vec::new(),
            ui_events: Vec::new(),
            pending_user_prompts: Vec::new(),
            prompt_echo_aliases: Vec::new(),
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
            pending_timeline_prepend_count: None,
            timeline_last_older_request_at: None,
            timeline_last_scroll_at: None,
            timeline_velocity_px_s: 0.0,
            timeline_last_tick_at: None,
            timeline_scroll_decay_tau: Self::TIMELINE_TRACKPAD_DECAY_TAU,
            timeline_scroll_stop_px_s: Self::TIMELINE_TRACKPAD_STOP_PX_S,
            timeline_measure_cache: RefCell::new(HashMap::new()),
            markdown_blocks_cache: RefCell::new(HashMap::new()),
            markdown_blocks_tick: std::cell::Cell::new(0),
            timeline_layout_epoch: 0,
            timeline_layout_cache: RefCell::new(None),
            timeline_dirty_message_ids: BTreeSet::new(),
            timeline_dirty_message_indices: BTreeSet::new(),
            timeline_live_trace_start: None,
            timeline_history: AgentTimelineHistoryState::default(),
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
            pending_outbound: VecDeque::new(),
            model_context_limit: None,
            wordmark: NeoismWordmarkState::default(),
            side_panel: NeoismAgentSidePanel::default(),
            perf_frame: AgentPanePerfFrame::default(),
        }
    }
}

pub(super) mod connect;
mod ingest;
mod input;
mod permissions;
mod render_state;
mod selection;
mod session;
mod submit;
mod timeline;

fn file_mention_options(
    root: &Path,
    query: &str,
    limit: usize,
) -> Vec<NeoismAgentPickerOption> {
    if limit == 0 {
        return Vec::new();
    }
    let mut scored = Vec::new();
    let mut visited = 0usize;
    collect_file_mention_candidates(root, root, query, 0, &mut visited, &mut scored);
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, display, path)| {
            let kind = if path.is_dir() { "directory" } else { "file" };
            let description = file_mention_description(&display, kind);
            NeoismAgentPickerOption::new(
                &format!("@{display}"),
                &description,
                kind,
                &display,
            )
        })
        .collect()
}

fn collect_file_mention_candidates(
    root: &Path,
    dir: &Path,
    query: &str,
    depth: usize,
    visited: &mut usize,
    output: &mut Vec<(i64, String, PathBuf)>,
) {
    if output.len() >= FILE_MENTION_SCAN_LIMIT
        || *visited >= FILE_MENTION_VISIT_LIMIT
        || depth > FILE_MENTION_MAX_DEPTH
    {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if output.len() >= FILE_MENTION_SCAN_LIMIT || *visited >= FILE_MENTION_VISIT_LIMIT
        {
            return;
        }
        *visited += 1;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_mention_ignored_component(std::ffi::OsStr::new(name)) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() && !file_type.is_dir() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(score) = fuzzy_score(&relative, query) {
            output.push((
                score - i64::try_from(depth).unwrap_or_default(),
                if file_type.is_dir() {
                    format!("{relative}/")
                } else {
                    relative.clone()
                },
                path.clone(),
            ));
        }
        if file_type.is_dir() {
            collect_file_mention_candidates(
                root,
                &path,
                query,
                depth + 1,
                visited,
                output,
            );
        }
    }
}

fn file_mention_ignored_component(part: &std::ffi::OsStr) -> bool {
    matches!(
        part.to_str(),
        Some(
            ".git"
                | ".claude"
                | ".cache"
                | ".direnv"
                | ".neoism"
                | ".next"
                | "build"
                | "dist"
                | "node_modules"
                | "target"
        )
    )
}

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

fn file_mention_description(display: &str, kind: &str) -> String {
    display
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| format!("{kind} in {parent}"))
        .unwrap_or_else(|| kind.to_string())
}

fn attachment_url_for_path(path: &Path, mime: &str) -> String {
    if input_controller::attachment_mime_can_inline(mime) {
        if let Ok(metadata) = fs::metadata(path) {
            if metadata.len() <= MAX_INLINE_ATTACHMENT_BYTES {
                if let Ok(bytes) = fs::read(path) {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                    return format!("data:{mime};base64,{encoded}");
                }
            }
        }
    }
    input_controller::file_url(path)
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

fn same_nonempty_id(a: &NeoismAgentMessage, b: &NeoismAgentMessage) -> bool {
    !a.id.is_empty() && a.id == b.id
}

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

fn task_message_status_from_branch(status: BranchStatus) -> Option<&'static str> {
    match status {
        BranchStatus::Completed => Some("completed"),
        BranchStatus::Stopped => Some("error"),
        BranchStatus::WaitingPermission => Some("running"),
        BranchStatus::Active => None,
    }
}

fn is_user_prompt(message: &NeoismAgentMessage, prompt: &str) -> bool {
    message.kind == NeoismAgentMessageKind::User && message.text.trim() == prompt.trim()
}

fn instant_from_epoch_millis(epoch_millis: u64) -> Instant {
    let now_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let elapsed = now_millis.saturating_sub(epoch_millis);
    Instant::now()
        .checked_sub(Duration::from_millis(elapsed))
        .unwrap_or_else(Instant::now)
}

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

    // Fast path: this runs every frame via `has_status_activity()` (see
    // `render_timeline_with`). The overwhelmingly common case — no running
    // background_task tool message — must not allocate. A single cheap scan
    // decides, and only when a task is actually running do we do the
    // dedup-with-completions work below.
    let has_running = messages.iter().any(|message| {
        message.kind == NeoismAgentMessageKind::Tool
            && message.tool == "background_task"
            && message.status == "running"
    });
    if !has_running {
        return 0;
    }

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
