use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use neoism_backend::clipboard::ClipboardImage;
use neoism_ui::panels::agent_pane::api_mapping::SessionState;
use neoism_ui::panels::agent_pane::input_controller::{self, AgentInputBuffer};
use neoism_ui::panels::agent_pane::interaction_policy;
use neoism_ui::panels::agent_pane::outbound::OutboundAgentCommand;
use neoism_ui::panels::agent_pane::permission_policy::{self, PermissionReplyStart};
use neoism_ui::panels::agent_pane::question_policy::NeoismAgentPendingQuestion;
use neoism_ui::panels::agent_pane::selection_model::SelectableLine;
use neoism_ui::panels::agent_pane::state::{
    branch_status_from_runtime, task_message_status_from_runtime,
};
use neoism_ui::panels::agent_pane::status_policy;
use neoism_ui::panels::agent_pane::timeline_scroll_policy::ctrl_u_d_scroll_delta;
use neoism_ui::panels::agent_pane::usage_policy::{self, UsageSnapshot};
use neoism_ui::panels::agent_pane::view::timeline::TimelineViewAnchorKey;
use serde_json::{json, Value};

use super::api::{
    api_request_json, delete_session, fetch_agent_options, fetch_config_defaults,
    fetch_family_runtime, fetch_model_context_limit, fetch_model_options,
    fetch_pending_permissions, fetch_pending_questions, fetch_session_entries, fetch_session_goal,
    fetch_session_options, fetch_session_statuses, fetch_skill_options, fetch_subagent_entries,
    fetch_subagent_options, neoism_agent_server, rename_session, set_session_pinned,
    SessionStatusSnapshot,
};
use super::commands::slash_options;
use super::picker::{NeoismAgentPicker, NeoismAgentPickerKind, NeoismAgentPickerOption};
use super::side_panel::{
    BranchStatus, BranchStatusTransition, NeoismAgentSessionEntry, NeoismAgentSidePanel,
    SessionGoal,
};
use super::updates::{
    start_session_event_stream, AgentEventWake, AgentSessionEventStream, AgentSessionUpdate,
};

const DEFAULT_AGENT: &str = "build";
const DEFAULT_MODEL: &str = "";
const FILE_MENTION_LIMIT: usize = 10;
const MAX_INLINE_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
const ABORT_STREAM_SUPPRESSION: Duration = Duration::from_secs(5);
const TOOL_EXPAND_ANIMATION: Duration = Duration::from_millis(190);
const WORDMARK_CLICK_ANIMATION: Duration = Duration::from_millis(460);
const CODE_COPY_FEEDBACK_ANIMATION: Duration = Duration::from_millis(1_400);

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
    /// Display name of who sent this (user) message — seeds the presence
    /// orb + hover tooltip. `None` = the local user, resolved at render
    /// time to the local presence name (see
    /// `NeoismAgentPane::local_presence_name`). See the `TODO(shared-author)`
    /// seam in the shared `api_mapping::part_block` for the multiplayer path.
    pub author: Option<String>,
    pub images: Vec<neoism_ui::panels::agent_pane::state::NeoismAgentImage>,
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
    tool_archived: bool,
    title: u64,
    text: u64,
    status: u64,
    tool: u64,
    lang: u64,
    line_offset: Option<usize>,
    todos: u64,
    detail: u64,
    images: u64,
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

#[derive(Clone, Debug)]
struct TimelineViewAnchor {
    key: TimelineViewAnchorKey,
    screen_offset: f32,
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

pub(super) struct CachedAgentSession {
    pub state: SessionState,
    pub messages: Vec<NeoismAgentMessage>,
    pub pending_user_prompts: Vec<String>,
    pub prompt_echo_aliases: Vec<(String, String)>,
    pub timeline_history: AgentTimelineHistoryState,
    pub timeline_scroll_px: f32,
    pub timeline_follow_bottom: bool,
    pub timeline_content_height_px: f32,
    pub timeline_live_trace_start: Option<usize>,
    pub timeline_live_trace_anchor: Option<String>,
    pub timeline_layout_epoch: u64,
    pub timeline_layout_cache: Option<TimelineLayoutCache>,
    pub timeline_dirty_message_ids: BTreeSet<String>,
    pub timeline_dirty_message_indices: BTreeSet<usize>,
    pub runtime: CachedAgentRuntime,
    pub model_context_limit: Option<u64>,
    pub hydrated: bool,
    pub last_access: Instant,
}

impl CachedAgentSession {
    pub(super) fn live_only() -> Self {
        Self {
            state: SessionState::default(),
            messages: Vec::new(),
            pending_user_prompts: Vec::new(),
            prompt_echo_aliases: Vec::new(),
            timeline_history: AgentTimelineHistoryState::default(),
            timeline_scroll_px: 0.0,
            timeline_follow_bottom: true,
            timeline_content_height_px: 0.0,
            timeline_live_trace_start: None,
            timeline_live_trace_anchor: None,
            timeline_layout_epoch: 0,
            timeline_layout_cache: None,
            timeline_dirty_message_ids: BTreeSet::new(),
            timeline_dirty_message_indices: BTreeSet::new(),
            runtime: CachedAgentRuntime::default(),
            model_context_limit: None,
            hydrated: false,
            last_access: Instant::now(),
        }
    }

    pub(super) fn invalidate_timeline_layout(&mut self) {
        self.last_access = Instant::now();
        self.timeline_layout_epoch = self.timeline_layout_epoch.wrapping_add(1);
        self.timeline_layout_cache = None;
        self.timeline_dirty_message_ids.clear();
        self.timeline_dirty_message_indices.clear();
    }
}

pub(crate) struct CachedAgentRuntime {
    queued_prompt_count: usize,
    queued_prompt_preview: Option<String>,
    streaming_state: NeoismAgentStreamingState,
    streaming_started_at: Option<Instant>,
    streaming_state_changed_at: Option<Instant>,
    streaming_tool_label: Option<String>,
    subagent_waiting_started_at: Option<Instant>,
    background_tasks_started_at: Option<Instant>,
    running_background_task_count: usize,
    background_jobs_epoch: Option<String>,
    background_jobs_revision: u64,
    abort_requested_at: Option<Instant>,
}

impl Default for CachedAgentRuntime {
    fn default() -> Self {
        Self {
            queued_prompt_count: 0,
            queued_prompt_preview: None,
            streaming_state: NeoismAgentStreamingState::Idle,
            streaming_started_at: None,
            streaming_state_changed_at: None,
            streaming_tool_label: None,
            subagent_waiting_started_at: None,
            background_tasks_started_at: None,
            running_background_task_count: 0,
            background_jobs_epoch: None,
            background_jobs_revision: 0,
            abort_requested_at: None,
        }
    }
}

impl CachedAgentRuntime {
    pub(super) fn is_streaming(&self) -> bool {
        self.streaming_state != NeoismAgentStreamingState::Idle
            && self.streaming_started_at.is_some()
    }

    pub(super) fn note_streaming(
        &mut self,
        state: NeoismAgentStreamingState,
        tool: Option<String>,
    ) {
        if state == NeoismAgentStreamingState::Idle {
            self.streaming_state = state;
            self.streaming_started_at = None;
            self.streaming_state_changed_at = None;
            self.streaming_tool_label = None;
            self.abort_requested_at = None;
            return;
        }
        if self.streaming_started_at.is_none() {
            self.streaming_started_at = Some(Instant::now());
        }
        if self.streaming_state != state || self.streaming_state_changed_at.is_none() {
            self.streaming_state_changed_at = Some(Instant::now());
        }
        self.streaming_state = state;
        self.streaming_tool_label = tool;
    }

    pub(super) fn refresh_streaming_from_tail(
        &mut self,
        messages: &[NeoismAgentMessage],
    ) {
        let Some(tail) = messages.last() else {
            return;
        };
        let (state, tool) = match tail.kind {
            NeoismAgentMessageKind::Reasoning => {
                (NeoismAgentStreamingState::Thinking, None)
            }
            NeoismAgentMessageKind::Tool | NeoismAgentMessageKind::Subtask => (
                NeoismAgentStreamingState::Working,
                (!tail.title.is_empty()).then(|| tail.title.clone()),
            ),
            NeoismAgentMessageKind::Assistant => {
                (NeoismAgentStreamingState::Generating, None)
            }
            NeoismAgentMessageKind::User
            | NeoismAgentMessageKind::System
            | NeoismAgentMessageKind::Compaction => return,
        };
        self.note_streaming(state, tool);
    }

    pub(super) fn apply_queue_status(
        &mut self,
        count: usize,
        preview: Option<String>,
        started_at: Option<u64>,
    ) {
        let decision = status_policy::queue_status_decision(
            count,
            preview,
            started_at,
            self.is_streaming(),
        );
        self.queued_prompt_count = decision.count;
        self.queued_prompt_preview = decision.preview;
        if decision.should_enter_thinking {
            self.note_streaming(NeoismAgentStreamingState::Thinking, None);
        }
        if let Some(started_at) = decision.started_at {
            let started = instant_from_epoch_millis(started_at);
            self.streaming_started_at = Some(started);
            self.streaming_state_changed_at.get_or_insert(started);
        }
    }

    pub(super) fn consume_dequeued_prompt(&mut self, text: &str) {
        if self.queued_prompt_count > 0 {
            self.queued_prompt_count -= 1;
        }
        if self
            .queued_prompt_preview
            .as_deref()
            .is_some_and(|preview| {
                self.queued_prompt_count == 0 || preview.trim() == text.trim()
            })
        {
            self.queued_prompt_preview = None;
        }
    }
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
            author: None,
            images: Vec::new(),
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
    /// Transient backoff after a recoverable provider error: the server is
    /// retrying the in-flight run. Clears back to a normal streaming/idle
    /// state as soon as the run resumes or finishes.
    Retrying,
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
            Self::Retrying => "Retrying",
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
    PromptDispatched {
        origin_session_id: Option<String>,
        origin_draft_id: u64,
        session_id: String,
        transcript_echo: Option<String>,
        event_stream: Option<AgentSessionEventStream>,
    },
    PromptDispatchFailed {
        origin_session_id: Option<String>,
        origin_draft_id: u64,
        error: String,
    },
    CompactFinished,
    CompactFailed(String),
    ConfigDefaultsLoaded(neoism_ui::panels::agent_pane::api_mapping::ConfigDefaults),
    ModelContextLimitRefreshed {
        model: String,
        limit: Option<u64>,
    },
    ModelOptionsRefreshed(Result<Vec<NeoismAgentPickerOption>, String>),
    AgentOptionsRefreshed(Result<Vec<NeoismAgentPickerOption>, String>),
    SessionOptionsRefreshed(Result<Vec<NeoismAgentPickerOption>, String>),
    SkillOptionsRefreshed {
        directory: Option<String>,
        result: Result<Vec<NeoismAgentPickerOption>, String>,
    },
    SidePanelSessionsRefreshed {
        generation: u64,
        requested_cursor: Option<String>,
        result: Result<(Vec<NeoismAgentSessionEntry>, Option<String>), String>,
    },
    SidePanelSubagentsRefreshed {
        session_id: String,
        generation: u64,
        result: Result<Vec<NeoismAgentSessionEntry>, String>,
    },
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
    /// Background hydration for parent/child navigation. Live
    /// events continue landing in the per-session cache while this request is
    /// in flight; the applier merges instead of replacing them.
    SessionPreloaded {
        session_id: String,
        state: SessionState,
        messages: Vec<NeoismAgentMessage>,
        oldest_cursor: Option<String>,
    },
    SessionPreloadFailed {
        session_id: String,
        error: String,
    },
    SessionRuntimeStatusRefreshed {
        session_id: String,
        request_generation: u64,
        runtime_revision: u64,
        result: Result<HashMap<String, SessionStatusSnapshot>, String>,
        runtime: Result<super::api::FamilyRuntimeSnapshot, String>,
        permissions: Result<Vec<NeoismAgentPendingPermission>, String>,
        questions: Result<Vec<NeoismAgentPendingQuestion>, String>,
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
        oldest_cursor: Option<String>,
        reached_start: bool,
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
        connection_id: Option<String>,
    },
    /// An auto-completing OAuth `/connect` flow failed (timed out, cancelled in
    /// the browser, or the exchange errored).
    ConnectOauthFailed {
        provider_name: String,
        error: String,
    },
}

/// Sender for work completed off the UI thread. A plain `mpsc::Sender`
/// leaves a finished result parked until some unrelated input happens to
/// schedule another frame. Keep the channel cheap, but explicitly wake the
/// owning window after enqueueing so cold session catalogues, restored branch
/// metadata, OAuth results, and other background work paint immediately.
#[derive(Clone)]
pub(crate) struct AgentBackgroundSender {
    tx: mpsc::Sender<NeoismAgentBackgroundUpdate>,
    wake: Arc<Mutex<Option<AgentEventWake>>>,
}

impl AgentBackgroundSender {
    fn new(
        tx: mpsc::Sender<NeoismAgentBackgroundUpdate>,
        wake: Arc<Mutex<Option<AgentEventWake>>>,
    ) -> Self {
        Self { tx, wake }
    }

    pub(crate) fn send(
        &self,
        update: NeoismAgentBackgroundUpdate,
    ) -> Result<(), mpsc::SendError<NeoismAgentBackgroundUpdate>> {
        self.tx.send(update)?;
        if let Ok(wake) = self.wake.lock() {
            if let Some(wake) = wake.as_ref() {
                wake.wake();
            }
        }
        Ok(())
    }

    fn set_wake(&self, wake: AgentEventWake) {
        if let Ok(mut current) = self.wake.lock() {
            *current = Some(wake);
        }
    }

    fn begin_drain(&self) {
        if let Ok(wake) = self.wake.lock() {
            if let Some(wake) = wake.as_ref() {
                wake.begin_drain();
            }
        }
    }
}

pub(crate) struct PendingPromptDispatch {
    pub(crate) origin_session_id: Option<String>,
    pub(crate) origin_draft_id: u64,
    pub(crate) server: String,
    pub(crate) directory: Option<String>,
    pub(crate) message_id: String,
    pub(crate) parts: Vec<Value>,
    pub(crate) system: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) model: String,
    pub(crate) connection_id: Option<String>,
    pub(crate) thinking: Option<String>,
    pub(crate) delivery: neoism_protocol::agent::PromptDelivery,
    pub(crate) author: Option<String>,
    pub(crate) transcript_echo: Option<String>,
    pub(crate) event_wake: Option<AgentEventWake>,
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
    /// Whether the keyboard/help strip below the composer is
    /// visible. `/hints` toggles this pane-local presentation preference.
    input_help_visible: bool,
    pub(super) messages: Vec<NeoismAgentMessage>,
    pub(super) mode: NeoismAgentMode,
    pub(super) agent: Option<String>,
    /// Rearms the footer chip's rainbow/scramble transition when the user
    /// switches agents (including Build/Plan via Tab).
    pub(super) agent_label_changed_at: Option<Instant>,
    pub(super) model: String,
    pub(super) connection_id: Option<String>,
    pub(super) pending_account_model: Option<String>,
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
    file_mention_search: std::sync::Arc<dyn neoism_agent_service_api::WorkspaceSearchService>,
    file_mention_root_pin: Mutex<Option<std::sync::Arc<dyn neoism_agent_service_api::WorkspaceSearchRootPin>>>,
    event_stream: Option<AgentSessionEventStream>,
    event_wake: Option<AgentEventWake>,
    /// When the most recent update was drained from the event stream.
    /// Feeds the liveness watchdog: a session that claims active work but
    /// whose stream has delivered nothing for too long gets a forced
    /// resubscribe + transcript reconcile (the automated version of the
    /// user closing and reopening the chat).
    pub(super) last_stream_update_at: Option<Instant>,
    /// Rate-limits forced resubscribes so a genuinely quiet-but-healthy
    /// stream is not torn down repeatedly.
    pub(super) last_stream_resubscribe_at: Option<Instant>,
    /// Set when the server reported a recoverable provider error and is
    /// retrying the in-flight run. The retry re-seeds the SAME text part
    /// with empty text to wipe the partial reply; the normal empty-snapshot
    /// guard would ignore that wipe and the re-streamed tokens would append
    /// onto the partial, doubling the text. Consumed by the first empty
    /// assistant snapshot (or cleared at idle).
    pub(super) retry_reset_pending: bool,
    /// Transcript/state caches keyed by real session id. Parent and child
    /// sessions remain resident while navigation only changes `session_id`,
    /// matching the route-over-global-store model.
    pub(super) session_cache: HashMap<String, CachedAgentSession>,
    pub(super) session_preloads_in_flight: BTreeSet<String>,
    pub(super) session_preload_queue: VecDeque<(String, bool)>,
    pub(super) session_preloads_force_pending: BTreeSet<String>,
    pub(super) pending_session_switch: Option<String>,
    pub(super) session_tree_root_id: Option<String>,
    pub(super) runtime_status_request_generation: u64,
    pub(super) runtime_status_requests: HashMap<String, u64>,
    pub(super) session_runtime_revisions: HashMap<String, u64>,
    pub(super) runtime_hydrated_sessions: BTreeSet<String>,
    /// Sessions with an authoritative idle edge. Late part reconciliation may
    /// update their transcript, but cannot resurrect activity chrome until a
    /// real busy/retry/new-prompt edge starts another run.
    pub(super) terminal_idle_sessions: BTreeSet<String>,
    pub(super) session_goal_cache: HashMap<String, (Option<SessionGoal>, u64)>,
    background_tx: AgentBackgroundSender,
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
    /// Easter-egg skit (`/piss`, `/cuss`): request consumed by the
    /// next render (which stamps `fx_started` on its animation
    /// clock); `fx_pending_prompt` is submitted once the skit's
    /// prompt moment passes.
    fx_requested: Option<neoism_ui::panels::agent_pane::view::fx::AgentFxKind>,
    fx_started: Option<(neoism_ui::panels::agent_pane::view::fx::AgentFxKind, f32)>,
    fx_pending_prompt: Option<String>,
    cursor_byte: usize,
    /// Soft-wrapped visual rows of the input (byte spans + per-boundary
    /// x offsets), registered by the renderer each frame — the same
    /// wrap the caret is placed with. Up/Down movement walks these;
    /// `input_wrap_len` guards against a frame of staleness after the
    /// text changes.
    input_wrap_rows: Vec<neoism_ui::panels::agent_pane::input_controller::InputWrapRow>,
    input_wrap_len: usize,
    /// Sticky caret x carried between consecutive Up/Down presses so a
    /// run of vertical moves keeps aiming at the column it started in.
    /// Cleared by edits and horizontal moves.
    input_goal_x: Option<f32>,
    input_attachments: Vec<NeoismAgentInputAttachment>,
    ui_events: Vec<NeoismAgentUiEvent>,
    pub(super) pending_user_prompts: Vec<String>,
    /// `(expanded, composer echo)` pairs for prompts sent with paste
    /// attachments: the server echoes the expanded text back, the
    /// transcript shows the compact `[pasted N lines]` form.
    pub(super) prompt_echo_aliases: Vec<(String, String)>,
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
    /// Rendered Markdown code/table horizontal viewports. Hit geometry is
    /// frame-local while offsets persist by stable message/block key.
    markdown_horizontal_scroll_rects: Vec<(String, [f32; 4], f32)>,
    markdown_horizontal_scroll_offsets: HashMap<String, f32>,
    markdown_horizontal_scrollbars: Vec<interaction_policy::MarkdownHorizontalScrollbar>,
    markdown_horizontal_scrollbar_drag:
        Option<interaction_policy::MarkdownHorizontalScrollbarDrag>,
    markdown_horizontal_scroll_hover_key: Option<String>,
    copied_code_feedback: Option<(String, Instant)>,
    permission_choice_hit_rects: Vec<(NeoismAgentPermissionChoice, [f32; 4])>,
    question_option_hit_rects: Vec<(usize, [f32; 4])>,
    /// Rect of the prompt-picker card (permission / question) drawn last
    /// frame — folded into `picker_card_rect()` so chrome text occludes
    /// under it exactly like the "/" picker modal.
    prompt_picker_rect: Option<[f32; 4]>,
    link_hit_rects: Vec<(String, [f32; 4])>,
    mermaid_raw_blocks: BTreeSet<u64>,
    usage_chip_rect: Option<[f32; 4]>,
    status_chip_rects: [Option<[f32; 4]>; 3],
    background_status_rect: Option<[f32; 4]>,
    background_task_details_expanded: bool,
    hover_link_target: Option<String>,
    selectable_lines: Vec<SelectableLine>,
    /// Logical count of `selectable_lines` valid for the current frame. The
    /// Vec retains its `String` allocations across frames (reused in place)
    /// so the per-frame "clear" is just resetting this to 0 — no per-line
    /// alloc/free churn, which in debug was costing ~1.5ms/frame.
    selectable_lines_len: usize,
    selection_anchor: Option<SelectionPoint>,
    selection_focus: Option<SelectionPoint>,
    pub(super) timeline_scroll_px: f32,
    pub(super) timeline_follow_bottom: bool,
    pub(super) timeline_content_height_px: f32,
    timeline_viewport_height_px: f32,
    timeline_viewport_rect: Option<[f32; 4]>,
    pending_timeline_anchor: Option<TimelineAnchor>,
    timeline_view_anchor: Option<TimelineViewAnchor>,
    pub(super) pending_timeline_prepend_height_px: Option<f32>,
    pub(super) pending_timeline_prepend_delta_px: Option<f32>,
    /// Count of messages just prepended by history pagination, awaiting an
    /// incremental layout fold (consumed by `take_timeline_prepend`).
    pub(super) pending_timeline_prepend_count: Option<usize>,
    timeline_last_scroll_at: Option<Instant>,
    timeline_velocity_px_s: f32,
    timeline_last_tick_at: Option<Instant>,
    /// Fixed destination for discrete mouse-wheel notches. Precision trackpad
    /// input leaves this unset and keeps the existing kinetic path.
    timeline_wheel_target_px: Option<f32>,
    /// Per-gesture inertia tuning for precision trackpad input.
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
    markdown_blocks_source_bytes: std::cell::Cell<usize>,
    pub(super) timeline_layout_epoch: u64,
    pub(super) timeline_layout_cache: RefCell<Option<TimelineLayoutCache>>,
    pub(super) timeline_dirty_message_ids: BTreeSet<String>,
    pub(super) timeline_dirty_message_indices: BTreeSet<usize>,
    /// Live SSE parts are flattened into timeline rows, but the provider
    /// groups them under an assistant message. Preserve that relationship so
    /// a delayed reasoning-end event can use the same order as final history.
    live_part_parent_ids: HashMap<String, String>,
    /// First source row whose trace was observed live during this visit to the
    /// session. Family-local navigation parks it with the transcript; leaving
    /// the full conversation clears it and settles the trace.
    timeline_live_trace_start: Option<usize>,
    /// Id of the user message the live-trace window is anchored after; kept
    /// alongside the index so list replacements/prepends re-derive the same
    /// turn boundary instead of drifting to the latest turn.
    timeline_live_trace_anchor: Option<String>,
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
    running_background_task_count: usize,
    background_jobs_epoch: Option<String>,
    background_jobs_revision: u64,
    active_subagent_ids: BTreeSet<String>,
    active_subagent_started_at: HashMap<String, u64>,
    pub(super) execution_activity:
        Option<neoism_ui::panels::agent_pane::state::ExecutionActivityState>,
    pub(super) execution_timer_anchor:
        Option<neoism_ui::panels::agent_pane::state::ExecutionTimerAnchor>,
    pub(super) runtime_snapshot_root: Option<String>,
    pub(super) runtime_snapshot_revision: u64,
    pub(super) terminal_subagent_revisions: HashMap<String, (String, u64)>,
    pending_permission: Option<NeoismAgentPendingPermission>,
    pending_permission_queue: VecDeque<NeoismAgentPendingPermission>,
    /// `/yolo` — while true, every permission request auto-answers
    /// "Yes" the moment it lands (session-scoped, client-side; the
    /// config-level `dangerouslySkipPermissions` stops the server
    /// asking at all).
    skip_permissions: bool,
    pending_question: Option<NeoismAgentPendingQuestion>,
    pending_question_queue: VecDeque<NeoismAgentPendingQuestion>,
    pending_outbound: VecDeque<OutboundAgentCommand>,
    /// Prompt admission performs socket I/O and may create a session. Keep it
    /// serialized off the render thread so rapid sends retain their order and
    /// a fresh draft cannot race itself into multiple backend sessions.
    pub(super) pending_prompt_dispatches: VecDeque<PendingPromptDispatch>,
    pub(super) prompt_dispatch_in_flight: bool,
    /// Identity for the session-less composer. It changes on `/new` so a
    /// delayed session-creation result cannot attach an older prompt to the
    /// replacement draft merely because both have `session_id == None`.
    pub(super) prompt_draft_id: u64,
    pub(super) model_context_limit: Option<u64>,
    pub wordmark: NeoismWordmarkState,
    pub(super) side_panel: NeoismAgentSidePanel,
    perf_frame: AgentPanePerfFrame,
    /// The local peer's presence display name (the same seed the editor
    /// caret / top-chrome orb use). Pushed by the screen each frame; the
    /// fallback author for user messages with no explicit `author`, so
    /// the local user's own messages render their own presence orb.
    local_presence_name: Option<String>,
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
    row_x: f32,
    byte_offset: usize,
    x: f32,
}

impl PartialEq for SelectionPoint {
    fn eq(&self, other: &Self) -> bool {
        (self.content_y - other.content_y).abs() < 0.5
            && (self.row_x - other.row_x).abs() < 0.5
            && self.byte_offset == other.byte_offset
    }
}

fn order_endpoints(
    a: SelectionPoint,
    b: SelectionPoint,
) -> (SelectionPoint, SelectionPoint) {
    if a.content_y < b.content_y
        || ((a.content_y - b.content_y).abs() < 0.5
            && (a.row_x < b.row_x
                || ((a.row_x - b.row_x).abs() < 0.5 && a.byte_offset <= b.byte_offset)))
    {
        (a, b)
    } else {
        (b, a)
    }
}

impl Default for NeoismAgentPane {
    fn default() -> Self {
        let (background_raw_tx, background_rx) = mpsc::channel();
        let background_tx =
            AgentBackgroundSender::new(background_raw_tx, Arc::new(Mutex::new(None)));
        Self {
            input: String::new(),
            input_help_visible: true,
            messages: Vec::new(),
            mode: NeoismAgentMode::Build,
            agent: Some(DEFAULT_AGENT.to_string()),
            agent_label_changed_at: None,
            model: DEFAULT_MODEL.to_string(),
            connection_id: None,
            pending_account_model: None,
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
            file_mention_search: Arc::new(
                neoism_agent_workspace_search_fff::FffWorkspaceSearchService::new(),
            ),
            file_mention_root_pin: Mutex::new(None),
            event_stream: None,
            event_wake: None,
            last_stream_update_at: None,
            last_stream_resubscribe_at: None,
            retry_reset_pending: false,
            session_cache: HashMap::new(),
            session_preloads_in_flight: BTreeSet::new(),
            session_preload_queue: VecDeque::new(),
            session_preloads_force_pending: BTreeSet::new(),
            pending_session_switch: None,
            session_tree_root_id: None,
            runtime_status_request_generation: 0,
            runtime_status_requests: HashMap::new(),
            session_runtime_revisions: HashMap::new(),
            runtime_hydrated_sessions: BTreeSet::new(),
            terminal_idle_sessions: BTreeSet::new(),
            session_goal_cache: HashMap::new(),
            background_tx,
            background_rx,
            semantic_in_flight: false,
            semantic_pending_query: None,
            semantic_unavailable: false,
            session_picker_base: Vec::new(),
            cursor_rect: None,
            fx_requested: None,
            fx_started: None,
            fx_pending_prompt: None,
            cursor_byte: 0,
            input_wrap_rows: Vec::new(),
            input_wrap_len: 0,
            input_goal_x: None,
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
            markdown_horizontal_scroll_rects: Vec::new(),
            markdown_horizontal_scroll_offsets: HashMap::new(),
            markdown_horizontal_scrollbars: Vec::new(),
            markdown_horizontal_scrollbar_drag: None,
            markdown_horizontal_scroll_hover_key: None,
            copied_code_feedback: None,
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
            timeline_follow_bottom: true,
            timeline_content_height_px: 0.0,
            timeline_viewport_height_px: 0.0,
            timeline_viewport_rect: None,
            pending_timeline_anchor: None,
            timeline_view_anchor: None,
            pending_timeline_prepend_height_px: None,
            pending_timeline_prepend_delta_px: None,
            pending_timeline_prepend_count: None,
            timeline_last_scroll_at: None,
            timeline_velocity_px_s: 0.0,
            timeline_last_tick_at: None,
            timeline_wheel_target_px: None,
            timeline_scroll_decay_tau: Self::TIMELINE_TRACKPAD_DECAY_TAU,
            timeline_scroll_stop_px_s: Self::TIMELINE_TRACKPAD_STOP_PX_S,
            timeline_measure_cache: RefCell::new(HashMap::new()),
            markdown_blocks_cache: RefCell::new(HashMap::new()),
            markdown_blocks_tick: std::cell::Cell::new(0),
            markdown_blocks_source_bytes: std::cell::Cell::new(0),
            timeline_layout_epoch: 0,
            timeline_layout_cache: RefCell::new(None),
            timeline_dirty_message_ids: BTreeSet::new(),
            timeline_dirty_message_indices: BTreeSet::new(),
            live_part_parent_ids: HashMap::new(),
            timeline_live_trace_start: None,
            timeline_live_trace_anchor: None,
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
            running_background_task_count: 0,
            background_jobs_epoch: None,
            background_jobs_revision: 0,
            active_subagent_ids: BTreeSet::new(),
            active_subagent_started_at: HashMap::new(),
            execution_activity: None,
            execution_timer_anchor: None,
            runtime_snapshot_root: None,
            runtime_snapshot_revision: 0,
            terminal_subagent_revisions: HashMap::new(),
            pending_permission: None,
            pending_permission_queue: VecDeque::new(),
            skip_permissions: false,
            pending_question: None,
            pending_question_queue: VecDeque::new(),
            pending_outbound: VecDeque::new(),
            pending_prompt_dispatches: VecDeque::new(),
            prompt_dispatch_in_flight: false,
            prompt_draft_id: 0,
            model_context_limit: None,
            wordmark: NeoismWordmarkState::default(),
            side_panel: NeoismAgentSidePanel::default(),
            perf_frame: AgentPanePerfFrame::default(),
            local_presence_name: None,
        }
    }
}

pub(super) mod connect;
mod ingest;
mod input;
mod permissions;
mod questions;
mod render_state;
mod selection;
mod session;
mod submit;
mod timeline;

/// Options for the agent input bar's `@` file-mention picker. Candidates
/// come from the workspace search service rooted at `root`, ranked best-first
/// and capped to `limit`.
fn file_mention_options(
    search: &dyn neoism_agent_service_api::WorkspaceSearchService,
    active_root: &Mutex<Option<std::sync::Arc<dyn neoism_agent_service_api::WorkspaceSearchRootPin>>>,
    root: &Path,
    query: &str,
    limit: usize,
) -> Vec<NeoismAgentPickerOption> {
    if limit == 0 {
        return Vec::new();
    }
    // Over-fetch so the ignored-path post-filter (below) can drop a stray
    // `target/` / `build/` leak without starving the final `limit` rows.
    let fetch = limit.saturating_mul(4).max(64);
    let relatives = file_mention_search(search, active_root, root, query, fetch);

    relatives
        .into_iter()
        .filter(|relative| !file_mention_path_ignored(relative))
        .take(limit)
        .map(|relative| {
            let description = file_mention_description(&relative, "file");
            NeoismAgentPickerOption::new(
                &format!("@{relative}"),
                &description,
                "file",
                &relative,
            )
        })
        .collect()
}

fn file_mention_search(
    search: &dyn neoism_agent_service_api::WorkspaceSearchService,
    active_root: &Mutex<Option<std::sync::Arc<dyn neoism_agent_service_api::WorkspaceSearchRootPin>>>,
    root: &Path,
    query: &str,
    limit: usize,
) -> Vec<String> {
    // Only the currently used mention root is pinned. Switching workspaces
    // releases the previous pin so the bounded registry can retire it.
    if let Ok(mut pin) = active_root.lock() {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let already_pinned = pin.as_ref().is_some_and(|pin| pin.root() == canonical);
        if !already_pinned {
            *pin = search.pin_root(root).ok();
        }
    }
    search.find_files(&neoism_agent_service_api::FindFilesRequest {
        root: root.to_path_buf(),
        query: query.to_string(),
        include_hidden: false,
        offset: 0,
        limit,
        control: neoism_agent_service_api::WorkspaceSearchRequestControl::default(),
    })
        .inspect_err(|error| {
            tracing::warn!(
                target: "neoism::agent_mentions",
                root = %root.display(),
                %error,
                "workspace file-mention search failed"
            );
        })
        .map(|result| result.items.into_iter().map(|item| item.path.replace('\\', "/")).collect())
        .unwrap_or_default()
}

/// True when any component of a relative path falls in the historic
/// `@`-mention exclude set (`target`, `build`, `node_modules`, …). fff
/// already skips hidden dirs and gitignored paths, but a bare `target/`
/// on a non-git root can still leak, so this preserves the old walk's
/// explicit excludes.
fn file_mention_path_ignored(relative: &str) -> bool {
    Path::new(relative).components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(part) if file_mention_ignored_component(part)
        )
    })
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
    if incoming.kind == NeoismAgentMessageKind::User
        && existing.kind == NeoismAgentMessageKind::User
    {
        if incoming.text.trim().is_empty() {
            incoming.text = existing.text.clone();
        }
        for image in existing.images {
            if !incoming.images.contains(&image) {
                incoming.images.push(image);
            }
        }
    } else if incoming.images.is_empty() {
        incoming.images = existing.images;
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

/// Merge a stored transcript snapshot with parts that arrived live while the
/// snapshot request was in flight. Live text wins when the stored part is
/// empty or an older prefix, preserving monotonic hydration.
pub(super) fn merge_session_snapshot(
    snapshot: Vec<NeoismAgentMessage>,
    live: Vec<NeoismAgentMessage>,
) -> Vec<NeoismAgentMessage> {
    if snapshot.is_empty() || live.is_empty() {
        return if snapshot.is_empty() { live } else { snapshot };
    }

    let mut snapshot = snapshot.into_iter().map(Some).collect::<Vec<_>>();
    let snapshot_indices = snapshot
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            session_message_identity(message.as_ref()?).map(|identity| (identity, index))
        })
        .collect::<HashMap<_, _>>();
    let mut live = live.into_iter().map(Some).collect::<Vec<_>>();

    // Shared identities are chronological anchors. Unmatched cached rows are
    // kept in the interval where they originally appeared instead of all being
    // appended after a partial newest-page snapshot.
    let mut anchors = Vec::new();
    let mut next_snapshot_index = 0usize;
    for (live_index, message) in live.iter().enumerate() {
        let Some(message) = message.as_ref() else {
            continue;
        };
        let Some(snapshot_index) = session_message_identity(message)
            .and_then(|identity| snapshot_indices.get(&identity).copied())
        else {
            continue;
        };
        if snapshot_index < next_snapshot_index {
            continue;
        }
        anchors.push((live_index, snapshot_index));
        next_snapshot_index = snapshot_index + 1;
    }

    let mut live_slots = vec![Vec::new(); snapshot.len() + 1];
    if anchors.is_empty() {
        live_slots[snapshot.len()].extend(live.iter_mut().filter_map(Option::take));
    } else {
        let first_live_index = anchors[0].0;
        for message in live[..first_live_index].iter_mut().filter_map(Option::take) {
            if session_message_identity(&message)
                .is_none_or(|identity| !snapshot_indices.contains_key(&identity))
            {
                live_slots[0].push(message);
            }
        }
        for (anchor_index, &(live_index, snapshot_index)) in anchors.iter().enumerate() {
            let existing = live[live_index].take().expect("live anchor");
            let incoming = snapshot[snapshot_index].take().expect("snapshot anchor");
            snapshot[snapshot_index] = Some(merge_part_message(existing, incoming));

            let range_start = live_index + 1;
            let (range_end, slot) = anchors
                .get(anchor_index + 1)
                .map(|(next_live_index, _)| (*next_live_index, snapshot_index + 1))
                .unwrap_or((live.len(), snapshot.len()));
            for message in live[range_start..range_end]
                .iter_mut()
                .filter_map(Option::take)
            {
                if session_message_identity(&message)
                    .is_none_or(|identity| !snapshot_indices.contains_key(&identity))
                {
                    live_slots[slot].push(message);
                }
            }
        }
    }

    let mut seen = HashSet::with_capacity(snapshot.len().saturating_add(live.len()));
    let mut merged = Vec::with_capacity(snapshot.len().saturating_add(live.len()));
    for (index, incoming) in snapshot.into_iter().enumerate() {
        for message in std::mem::take(&mut live_slots[index]) {
            if session_message_identity(&message)
                .is_none_or(|identity| seen.insert(identity))
            {
                merged.push(message);
            }
        }
        if let Some(incoming) = incoming {
            if session_message_identity(&incoming)
                .is_none_or(|identity| seen.insert(identity))
            {
                merged.push(incoming);
            }
        }
    }
    for message in live_slots.pop().unwrap_or_default() {
        if session_message_identity(&message).is_none_or(|identity| seen.insert(identity))
        {
            merged.push(message);
        }
    }
    merged
}

pub(super) fn reconcile_cached_pending_user_prompts(
    snapshot: &mut [NeoismAgentMessage],
    live: &mut Vec<NeoismAgentMessage>,
    pending: &mut Vec<String>,
    aliases: &[(String, String)],
) {
    for message in snapshot
        .iter_mut()
        .filter(|message| message.kind == NeoismAgentMessageKind::User)
    {
        if let Some((_, echo)) = aliases
            .iter()
            .rev()
            .find(|(expanded, _)| expanded.trim() == message.text.trim())
        {
            message.text = echo.clone();
        }
    }
    let mut consumed_snapshot = vec![false; snapshot.len()];
    let mut unresolved = Vec::new();
    for prompt in std::mem::take(pending) {
        let resolved = snapshot.iter().enumerate().position(|(index, message)| {
            !consumed_snapshot[index]
                && message.kind == NeoismAgentMessageKind::User
                && message.text.trim() == prompt.trim()
        });
        let Some(snapshot_index) = resolved else {
            unresolved.push(prompt);
            continue;
        };
        consumed_snapshot[snapshot_index] = true;
        if let Some(live_index) = live.iter().position(|message| {
            message.kind == NeoismAgentMessageKind::User
                && message.text.trim() == prompt.trim()
        }) {
            live.remove(live_index);
        }
    }
    *pending = unresolved;
}

fn session_message_identity(message: &NeoismAgentMessage) -> Option<String> {
    if !message.id.is_empty() {
        return Some(format!("id:{}", message.id));
    }
    task_id_from_task_message(message).map(|task_id| format!("task:{task_id}"))
}

fn chronological_live_insert_index(
    messages: &[NeoismAgentMessage],
    parent_ids: &HashMap<String, String>,
    incoming: &NeoismAgentMessage,
) -> Option<usize> {
    let incoming_id = canonical_live_message_id(incoming, parent_ids)?;
    // An optimistic/queued human prompt is a hard turn boundary even before
    // its durable id arrives. Never move later output above what the user can
    // already see at the bottom of the timeline.
    let turn_start = messages
        .iter()
        .rposition(|message| message.kind == NeoismAgentMessageKind::User)
        .map_or(0, |index| index + 1);
    messages[turn_start..]
        .iter()
        .position(|existing| {
            canonical_live_message_id(existing, parent_ids)
                .is_some_and(|existing_id| existing_id > incoming_id)
        })
        .map(|index| turn_start + index)
}

fn canonical_live_message_id<'a>(
    message: &'a NeoismAgentMessage,
    parent_ids: &'a HashMap<String, String>,
) -> Option<&'a str> {
    // Runtime completion rows are lifecycle/context sentinels, not real
    // ascending transcript messages. Their reserved ids sort after every
    // canonical `msg_0...` id and previously acted as a false barrier: the
    // 30-second reply and then the reply to an optimistic user prompt were
    // inserted above the 15-second reply. Subagent/runtime notices likewise
    // must never participate in transcript ordering.
    if message.id.starts_with("background-task-") {
        return None;
    }
    let id = if message.id.starts_with("msg_") {
        message.id.as_str()
    } else {
        parent_ids.get(&message.id)?.as_str()
    };
    (id.starts_with("msg_")
        && !id.starts_with("msg_background_completion_")
        && !id.starts_with("msg_subtask_completion_"))
    .then_some(id)
}

pub(super) fn upsert_cached_part_message(
    messages: &mut Vec<NeoismAgentMessage>,
    message: NeoismAgentMessage,
) {
    if !message.id.is_empty() {
        if let Some(index) = messages
            .iter()
            .position(|existing| same_streamed_part_identity(existing, &message))
        {
            messages[index] = merge_part_message(messages[index].clone(), message);
            return;
        }
    }
    messages.push(message);
}

pub(super) fn apply_cached_part_delta(
    messages: &mut Vec<NeoismAgentMessage>,
    part_id: Option<&str>,
    kind: Option<&str>,
    delta: &str,
) {
    if delta.is_empty() {
        return;
    }
    if let Some(part_id) = part_id.filter(|id| !id.is_empty()) {
        if let Some(message) = messages.iter_mut().find(|message| message.id == part_id) {
            message.text.push_str(delta);
            return;
        }
        let message = match kind {
            Some("reasoning" | "thinking") => {
                NeoismAgentMessage::reasoning(delta).with_id(part_id.to_string())
            }
            _ => NeoismAgentMessage::assistant(delta).with_id(part_id.to_string()),
        };
        messages.push(message);
        return;
    }
    let message_kind = match kind {
        Some("reasoning" | "thinking") => NeoismAgentMessageKind::Reasoning,
        _ => NeoismAgentMessageKind::Assistant,
    };
    if let Some(message) = messages
        .iter_mut()
        .rfind(|message| message.kind == message_kind)
    {
        message.text.push_str(delta);
    } else {
        messages.push(match message_kind {
            NeoismAgentMessageKind::Reasoning => NeoismAgentMessage::reasoning(delta),
            _ => NeoismAgentMessage::assistant(delta),
        });
    }
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
        "status: queued",
        "status: working",
        "status: busy",
        "status: completed",
        "status: error",
        "status: stopped",
        "status: failed",
    ] {
        if field.contains(marker) {
            *field = field.replace(marker, &format!("status: {status}"));
            break;
        }
    }
    rewrite_stale_task_running_explanation(field, status);
}

fn rewrite_stale_task_running_explanation(field: &mut String, status: &str) {
    if !matches!(status, "completed" | "error") {
        return;
    }

    let Some(start) = field
        .lines()
        .scan(0usize, |offset, line| {
            let line_start = *offset;
            *offset += line.len() + 1;
            Some((line_start, line))
        })
        .find_map(|(line_start, line)| {
            let lower = line.to_ascii_lowercase();
            (lower.contains("subagent is running")
                || lower.contains("subagent is still running"))
            .then_some(line_start)
        })
    else {
        return;
    };

    field.truncate(start);
    *field = field.trim_end().to_string();
    field.push_str("\n\nThe subagent is no longer running.");
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

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
                .and_then(|value| value.split_whitespace().next())
                .map(ToOwned::to_owned)
        })
}

fn background_job_status_from_message(message: &NeoismAgentMessage) -> Option<&str> {
    message
        .detail
        .lines()
        .chain(message.text.lines())
        .find_map(|line| {
            line.trim()
                .strip_prefix("status:")
                .map(str::trim)
                .and_then(|value| value.split_whitespace().next())
        })
}

fn background_task_message_is_running(message: &NeoismAgentMessage) -> bool {
    message.kind == NeoismAgentMessageKind::Tool
        && message.tool == "background_task"
        && background_job_status_from_message(message) == Some("running")
}

fn background_completion_job_id_from_message(
    message: &NeoismAgentMessage,
) -> Option<String> {
    let text = format!("{}\n{}", message.detail, message.text).to_ascii_lowercase();
    if !text.contains("background shell task has finished")
        && !text.contains("background shell task finished")
        && !text.contains("background task has finished")
        && message.tool != "background_task_result"
    {
        return None;
    }
    match background_job_status_from_message(message) {
        Some("completed" | "cancelled" | "error" | "timed_out") => {}
        Some(_) => return None,
        None if message.status == "completed" => {}
        None => return None,
    }
    background_job_id_from_message(message)
}

fn background_task_empty_snapshot(message: &NeoismAgentMessage) -> bool {
    if message.kind != NeoismAgentMessageKind::Tool
        || message.tool != "background_task_result"
    {
        return false;
    }
    let text = format!("{}\n{}", message.detail, message.text).to_ascii_lowercase();
    text.contains("no background tasks exist")
        || text.contains("no background tasks are running")
}

fn running_background_task_count(messages: &[NeoismAgentMessage]) -> usize {
    use std::collections::BTreeSet;

    // Fast path: this runs every frame via `has_status_activity()` (see
    // `render_timeline_with`). The overwhelmingly common case — no running
    // background_task tool message — must not allocate. A single cheap scan
    // decides, and only when a task is actually running do we do the
    // dedup-with-completions work below.
    let has_running = messages.iter().any(background_task_message_is_running);
    if !has_running {
        return 0;
    }

    let messages = messages
        .iter()
        .rposition(background_task_empty_snapshot)
        .map_or(messages, |index| &messages[index + 1..]);
    let completed = messages
        .iter()
        .filter_map(background_completion_job_id_from_message)
        .collect::<BTreeSet<_>>();

    messages
        .iter()
        .filter(|message| background_task_message_is_running(message))
        .filter_map(background_job_id_from_message)
        .filter(|job_id| !completed.contains(job_id))
        .collect::<BTreeSet<_>>()
        .len()
}

fn active_background_task_summaries(messages: &[NeoismAgentMessage]) -> Vec<String> {
    use std::collections::BTreeSet;

    let messages = messages
        .iter()
        .rposition(background_task_empty_snapshot)
        .map_or(messages, |index| &messages[index + 1..]);
    let completed = messages
        .iter()
        .filter_map(background_completion_job_id_from_message)
        .collect::<BTreeSet<_>>();

    messages
        .iter()
        .filter(|message| background_task_message_is_running(message))
        .filter_map(|message| {
            let job_id = background_job_id_from_message(message)?;
            if completed.contains(&job_id) {
                return None;
            }
            let command = background_task_command_from_message(message)
                .unwrap_or_else(|| message.title.as_str().to_string());
            Some(format!("{} · running · {}", job_id, command))
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

fn is_unsettled_edit_tool(tool: &str, status: &str) -> bool {
    let normalized = tool
        .chars()
        .filter(|ch| *ch != '_' && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(normalized.as_str(), "applypatch" | "patch")
        && matches!(
            status.trim().to_ascii_lowercase().as_str(),
            "pending" | "running" | "streaming"
        )
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
