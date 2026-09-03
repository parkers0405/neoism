//! Right-attached side panel for the Neoism agent pane.
//!
//! Mirrors the visual shape of `editor/file_tree` (frame + scroll spring
//! + per-row cursor spring + pixel-accurate wheel handling) but lives
//! on the agent pane itself rather than on the renderer. That keeps it
//! scoped to the neoism tab — it only paints while a neoism agent pane
//! is rendering, and it splits with the agent's own rect instead of the
//! window-wide left column the file tree owns.
//!
//! Two display modes, toggled by whether the agent is mid-conversation:
//!
//! - **Home mode** (no messages yet — wordmark + centered input
//!   visible): show the most recent previous sessions, click to resume.
//! - **Chat mode** (a conversation has started): show live session info
//!   — agent / model / thinking, streaming state, usage chip, queued
//!   prompts, pending permissions.
//!
//! State is per-pane; the rendering side reads from this module via
//! [`NeoismAgentPane::side_panel`].
//!
//! See [`crate::editor::file_tree::state::FileTree`] for the structural
//! analogue — same scroll/cursor primitives, same clamp/scrolloff
//! pattern.

use std::collections::{BTreeSet, HashMap, HashSet};
use web_time::Duration;
use web_time::Instant;

use crate::animation::CriticallyDampedSpring;

use crate::panels::agent_pane::icon::AgentKind;
use crate::widgets::scroll::Scroll;

/// Default width when the panel is shown. Smaller than the file tree
/// because the agent pane is usually narrower than the full window.
pub const SIDE_PANEL_WIDTH: f32 = 260.0;

/// Minimum agent-pane width below which the panel hides itself —
/// otherwise a narrow split would shove the chat content into nothing.
pub const SIDE_PANEL_MIN_PANE_WIDTH: f32 = 640.0;

/// Row height matches the file tree at 1.0 scale so the two side panels
/// read as the same family.
pub const ROW_HEIGHT: f32 = 26.0;
pub const FONT_SIZE: f32 = 13.0;
pub const ROW_PADDING_X: f32 = 12.0;
pub const FRAME_RADIUS: f32 = 14.0;
pub const FRAME_STROKE: f32 = 2.25;

// Snappy home-list scroll: short critically-damped catch-up so wheel /
// trackpad / arrow scrolling tracks the gesture tightly instead of
// trailing behind (the old 0.30s spring read as "laggy").
pub const SCROLL_ANIMATION_LENGTH: f32 = 0.12;
pub const CURSOR_ANIMATION_LENGTH: f32 = 0.12;
pub const SCROLL_OFF_ROWS: usize = 3;

/// One entry in the sessions / sub-agents list. `time_label` doubles
/// as the right-aligned footer text — "5 minutes ago" for sessions,
/// agent name ("build" / "plan" / "main session") for sub-agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeoismAgentSessionEntry {
    pub id: String,
    pub title: String,
    pub time_label: String,
    pub depth: usize,
    pub agent_kind: Option<AgentKind>,
    pub runtime_status: Option<String>,
    /// Raw `time.updated` unix-ms — buckets the entry under a date-group
    /// header in home mode. `0` for untracked / non-session rows.
    pub updated_ms: u64,
    /// Whether the session is pinned to the top of the list.
    pub pinned: bool,
    /// True for an injected cyan date-group / "Pinned" header row (not a
    /// selectable session). `title` holds the header label.
    pub is_header: bool,
    /// True for an injected semantic-match excerpt row shown under its
    /// session while a transcript search is active. `id` carries the parent
    /// session id (so activating it resumes that session) and `title` holds
    /// the matched excerpt text.
    pub is_excerpt: bool,
    /// Byte ranges into `title` that matched the active search terms —
    /// rendered highlighted on excerpt rows. Sorted and non-overlapping.
    pub highlights: Vec<(usize, usize)>,
}

/// One semantic transcript-search hit, per session (best chunk kept).
#[derive(Clone, Debug, PartialEq)]
pub struct NeoismAgentSemanticMatch {
    pub session_id: String,
    /// The matched transcript chunk, as returned by the server.
    pub excerpt: String,
    /// Cosine distance — lower is closer.
    pub distance: f64,
}

impl NeoismAgentSessionEntry {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        time_label: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            time_label: time_label.into(),
            depth: 0,
            agent_kind: None,
            runtime_status: None,
            updated_ms: 0,
            pinned: false,
            is_header: false,
            is_excerpt: false,
            highlights: Vec::new(),
        }
    }

    /// A non-selectable date-group / "Pinned" header row.
    pub fn header(label: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            title: label.into(),
            time_label: String::new(),
            depth: 0,
            agent_kind: None,
            runtime_status: None,
            updated_ms: 0,
            pinned: false,
            is_header: true,
            is_excerpt: false,
            highlights: Vec::new(),
        }
    }

    /// A semantic-match excerpt row under `session_id`'s entry. Selectable:
    /// activating it resumes the parent session.
    pub fn excerpt(session_id: impl Into<String>, excerpt: impl Into<String>) -> Self {
        Self {
            id: session_id.into(),
            title: excerpt.into(),
            time_label: String::new(),
            depth: 0,
            agent_kind: None,
            runtime_status: None,
            updated_ms: 0,
            pinned: false,
            is_header: false,
            is_excerpt: true,
            highlights: Vec::new(),
        }
    }

    pub fn with_updated_ms(mut self, updated_ms: u64) -> Self {
        self.updated_ms = updated_ms;
        self
    }

    pub fn with_pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    pub fn with_depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    pub fn with_agent_kind(mut self, agent_kind: Option<AgentKind>) -> Self {
        self.agent_kind = agent_kind;
        self
    }

    pub fn with_runtime_status(mut self, runtime_status: Option<String>) -> Self {
        self.runtime_status = runtime_status;
        self
    }
}

fn generic_subagent_metadata(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "agent" | "subagent" | "sub-agent" | "sub agent" | "untitled" | "unknown"
    )
}

/// Collapse a transcript excerpt to one displayable line: whitespace runs
/// (including newlines) become single spaces, and long chunks truncate on a
/// char boundary with an ellipsis.
fn compact_excerpt(excerpt: &str) -> String {
    let mut compact = String::with_capacity(excerpt.len().min(160));
    let mut last_space = true;
    for ch in excerpt.chars() {
        if ch.is_whitespace() {
            if !last_space {
                compact.push(' ');
                last_space = true;
            }
        } else {
            compact.push(ch);
            last_space = false;
        }
    }
    let compact = compact.trim().to_string();
    // Wrapping handles presentation; this is just a sanity ceiling so one
    // enormous chunk can't bloat the display list.
    const MAX: usize = 480;
    if compact.chars().count() <= MAX {
        return compact;
    }
    compact.chars().take(MAX).collect()
}

/// Byte ranges of every ASCII-case-insensitive occurrence of any search
/// term in `line`, merged where they touch or overlap. Terms are expected
/// pre-lowercased; ASCII lowering preserves byte offsets so the ranges
/// index the original string safely.
fn term_highlight_ranges(line: &str, terms: &[String]) -> Vec<(usize, usize)> {
    if terms.is_empty() {
        return Vec::new();
    }
    let haystack = line.to_ascii_lowercase();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let mut from = 0;
        while let Some(found) = haystack[from..].find(term.as_str()) {
            let start = from + found;
            ranges.push((start, start + term.len()));
            from = start + term.len().max(1);
        }
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Word-wrap an excerpt into at most `max_lines` lines of `columns`
/// monospace characters. Overlong words hard-split; a truncated tail gets an
/// ellipsis on the final line.
fn wrap_excerpt_lines(excerpt: &str, columns: usize, max_lines: usize) -> Vec<String> {
    let columns = columns.max(8);
    let mut lines: Vec<String> = Vec::with_capacity(max_lines);
    let mut current = String::new();
    let mut current_len = 0usize;
    let mut truncated = false;
    'words: for word in excerpt.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let sep = usize::from(current_len > 0);
            if current_len + sep + word.len() <= columns {
                if sep == 1 {
                    current.push(' ');
                }
                current.extend(word.iter());
                current_len += sep + word.len();
                continue 'words;
            }
            // The word doesn't fit on this line.
            if word.len() > columns {
                // Hard-split an overlong token across lines.
                let take = columns - current_len - sep;
                if take > 0 {
                    if sep == 1 {
                        current.push(' ');
                    }
                    current.extend(word.iter().take(take));
                    word.drain(..take);
                }
            }
            if lines.len() + 1 == max_lines {
                truncated = true;
                break 'words;
            }
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    if truncated {
        if let Some(last) = lines.last_mut() {
            while last.chars().count() >= columns {
                last.pop();
            }
            last.push('…');
        }
    }
    lines.retain(|line| !line.is_empty());
    lines
}

fn preserve_specific_subagent_metadata(
    incoming: &mut NeoismAgentSessionEntry,
    previous: &NeoismAgentSessionEntry,
) {
    if generic_subagent_metadata(&incoming.title)
        && !generic_subagent_metadata(&previous.title)
    {
        incoming.title = previous.title.clone();
    }
    if generic_subagent_metadata(&incoming.time_label)
        && !generic_subagent_metadata(&previous.time_label)
    {
        incoming.time_label = previous.time_label.clone();
    }
    if incoming.agent_kind.is_none() {
        incoming.agent_kind = previous.agent_kind;
    }
    // `/session/status` is only the live SessionRun set. Omission cannot
    // terminalize the parent-owned subtask lifecycle.
    if incoming.runtime_status.is_none() {
        incoming.runtime_status = previous.runtime_status.clone();
    }
}

/// Which list the side panel is currently steering selection/scroll
/// over. Set every frame by the renderer based on `pane.has_conversation()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidePanelMode {
    /// Home view — previous sessions.
    Sessions,
    /// In-session view — sibling / child sub-agent sessions.
    Subagents,
}

/// State of a branch (main session or child) inferred from its
/// latest messages. Drives the colored dot rendered next to each row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchStatus {
    /// Working — tool in flight or waiting for assistant reply.
    /// Rendered as a *blinking* white dot.
    Active,
    /// Blocked on a tool permission prompt — blinking yellow dot.
    WaitingPermission,
    /// Latest message is the assistant's reply — green dot.
    Completed,
    /// Latest signal looks like an error/abort — red dot.
    Stopped,
}

/// How much authority a branch-status transition has over a latched terminal
/// state. Run lifecycle edges may represent a real continuation of the same
/// child session; ancillary events may arrive late while cleaning up a run
/// that has already finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchStatusTransition {
    /// A real session/subtask lifecycle edge. Active states reopen a finished
    /// child, while terminal states latch it closed.
    AuthoritativeRun,
    /// Permission cleanup or other non-run activity. It may refine a live
    /// branch, but can never reopen a terminal-locked child.
    AncillaryActivity,
}

impl BranchStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "working",
            Self::WaitingPermission => "needs approval",
            Self::Completed => "done",
            Self::Stopped => "stopped",
        }
    }

    pub fn from_runtime_status(status: &str) -> Option<Self> {
        match status.trim().to_ascii_lowercase().as_str() {
            "active" | "busy" | "created" | "running" => Some(Self::Active),
            "blocked" | "retry" | "waiting_permission" | "waiting-permission" => {
                Some(Self::WaitingPermission)
            }
            "completed" | "complete" | "idle" | "done" => Some(Self::Completed),
            "failed" | "error" | "errored" | "stopped" | "aborted" => Some(Self::Stopped),
            _ => None,
        }
    }

    fn runtime_status(self) -> &'static str {
        match self {
            Self::Active => "running",
            Self::WaitingPermission => "blocked",
            Self::Completed => "completed",
            Self::Stopped => "error",
        }
    }
}

/// Per-branch (per-session) snapshot the side panel renders under each
/// row: the most recent tool the branch was using (truncated to fit)
/// and the derived run status.
#[derive(Clone, Debug)]
pub struct BranchActivity {
    pub status: BranchStatus,
    pub current_tool: Option<String>,
    pub started_at: Option<u64>,
    /// When the branch last transitioned into a terminal state
    /// (`Completed` / `Stopped`). Drives the timed auto-hide of
    /// finished sub-agents from the sidebar — see
    /// [`NeoismAgentSidePanel::visible_subagents`]. Reset to `None`
    /// the moment the branch goes back to `Active` /
    /// `WaitingPermission` so a respawned sub-agent reappears.
    pub completed_at: Option<Instant>,
    /// Set when the branch reached a terminal state via an
    /// *authoritative* lifecycle signal — the parent's `task` tool
    /// part finishing, the child session going `idle`, or an explicit
    /// `session.subtask.completed`. Once latched, noisy part-level
    /// activity (a straggler text/reasoning delta from the child that
    /// still claims "responding"/"thinking") can no longer resurrect
    /// the row to "active". This is the fix for sub-agents that stay
    /// stuck on "responding"/"working" after they've actually
    /// finished: those late part deltas are not lifecycle events, so
    /// they must not un-terminate the branch. Only another
    /// authoritative signal (a genuine respawn) clears the latch.
    pub terminal_locked: bool,
}

impl BranchActivity {
    /// Build a fresh activity for `status`. A branch that is *first
    /// observed* already terminal (e.g. a sub-agent that finished before
    /// the panel opened, or one re-listed by a recovery snapshot) gets NO
    /// visibility window — `completed_at` stays `None`, which
    /// [`NeoismAgentSidePanel::subagent_hidden`] treats as "hide
    /// immediately". The 7s show-then-hide window is only started by a
    /// live `active → terminal` transition in [`Self::transition_status`].
    fn new(
        status: BranchStatus,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) -> Self {
        let terminal = matches!(status, BranchStatus::Completed | BranchStatus::Stopped);
        Self {
            status,
            current_tool: if terminal { None } else { current_tool },
            started_at: if terminal { None } else { started_at },
            completed_at: None,
            terminal_locked: false,
        }
    }

    /// Transition an existing activity to `status`, maintaining the
    /// `completed_at` auto-hide clock: stamp it on the first edge into
    /// a terminal state, clear it the moment the branch becomes active
    /// again (a respawn) so the row reappears. Returns `true` when the
    /// new status is terminal (caller can skip tool/started-at edits).
    fn transition_status(&mut self, status: BranchStatus) -> bool {
        let terminal = matches!(status, BranchStatus::Completed | BranchStatus::Stopped);
        let was_terminal =
            matches!(self.status, BranchStatus::Completed | BranchStatus::Stopped);
        if terminal {
            // Stamp the show-then-hide window ONLY on the edge into a
            // terminal state (a genuine live completion the user is
            // watching). A terminal→terminal re-apply (e.g. a recovery
            // snapshot re-reporting an already-finished child) must NOT restart the
            // window — otherwise old completions keep flashing back.
            if !was_terminal {
                self.completed_at = Some(Instant::now());
            }
            self.current_tool = None;
            self.started_at = None;
        } else {
            self.completed_at = None;
        }
        self.status = status;
        terminal
    }
}

/// How long a finished (`Completed` / `Stopped`) sub-agent lingers in
/// the sidebar before it's auto-hidden, mirroring Claude's behaviour.
/// A respawn (status back to active) clears the timer and the row
/// returns immediately.
pub const SUBAGENT_HIDE_AFTER: Duration = Duration::from_secs(7);

/// How long the composer's status label survives a *transient* idle
/// reading. Between events the raw derivation can pass through Idle for
/// a tick (MessageEnd → next MessageStart, tool-phase handoffs, a
/// child's inter-message idle edge zeroing the active-subagent count);
/// blanking the label for that frame also collapsed the status row's
/// reserved height and made the whole composer bounce. The display
/// derivation therefore keeps showing the last non-idle status until
/// idle has persisted for this window; genuine completion still clears
/// once the window elapses.
pub const STATUS_LABEL_GRACE: Duration = Duration::from_millis(400);

/// Memory behind [`STATUS_LABEL_GRACE`]: the status the composer row
/// last displayed, refreshed every frame the raw derivation is
/// non-idle. Stored on the side panel (shared by the desktop and web
/// panes) behind a `RefCell` so the `&self` display getters can record
/// it — same interior-mutability precedent as the pane's timeline
/// layout cache.
#[derive(Clone)]
struct StatusDisplayHold {
    state: crate::panels::agent_pane::state::NeoismAgentStreamingState,
    /// Anchor whose `elapsed()` continues the displayed timer through
    /// the gap, so the label's wave animation / elapsed suffix don't
    /// jump to zero while the hold bridges two events.
    elapsed_anchor: Instant,
    /// Last frame the raw derivation was non-idle. The grace window
    /// counts from here and is deliberately NOT refreshed while the
    /// hold itself is what's showing.
    last_seen: Instant,
    /// Session the label belonged to — a session switch must never show
    /// the previous conversation's held label.
    session: Option<String>,
}

/// Lifecycle of the session's persistent goal, mirroring the backend
/// `goal.status` field. Drives the status shown alongside "Goal" in the
/// side-panel section heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GoalStatus {
    /// The agent is actively pursuing the goal.
    #[default]
    Active,
    /// The goal has been accomplished.
    Complete,
    /// The agent is blocked and can't make progress unaided.
    Blocked,
}

impl GoalStatus {
    pub fn from_str(status: &str) -> Self {
        match status.trim().to_ascii_lowercase().as_str() {
            "complete" | "completed" | "done" => Self::Complete,
            "blocked" | "stuck" => Self::Blocked,
            _ => Self::Active,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Complete => "Complete",
            Self::Blocked => "Blocked",
        }
    }
}

/// The session's persistent goal, surfaced live above the task list.
/// Mirrors the backend goal model: `text` + `status` + an optional
/// `summary` (what the agent accomplished, or why it's blocked) and
/// the `paused` flag. Only rendered when `text` is non-empty.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SessionGoal {
    pub text: String,
    pub status: GoalStatus,
    pub summary: String,
    pub paused: bool,
    /// Backend `updated` millis — the monotonic version used to drop a
    /// stale `GET /goal` poll that raced (and would clobber) a newer
    /// live `SESSION_UPDATED`. See `SidePanel::set_session_goal`.
    pub updated: u64,
}

impl SessionGoal {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Parse the backend goal object (`{ text, status, summary,
    /// paused, ... }`). Returns `None` for a null/absent goal or one
    /// with empty text so the section stays hidden.
    pub fn from_json(goal: &serde_json::Value) -> Option<Self> {
        if goal.is_null() {
            return None;
        }
        let text = goal
            .get("text")
            .and_then(|text| text.as_str())
            .unwrap_or_default()
            .to_string();
        if text.trim().is_empty() {
            return None;
        }
        Some(Self {
            text,
            status: goal
                .get("status")
                .and_then(|status| status.as_str())
                .map(GoalStatus::from_str)
                .unwrap_or_default(),
            summary: goal
                .get("summary")
                .and_then(|summary| summary.as_str())
                .unwrap_or_default()
                .to_string(),
            paused: goal
                .get("paused")
                .and_then(|paused| paused.as_bool())
                .unwrap_or(false),
            updated: goal
                .get("updated")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        })
    }
}

/// Per-pane side panel state. Holds animation springs, scroll cursor,
/// and the cached session list. Refreshing the list is the pane's
/// responsibility — see [`NeoismAgentPane::request_side_panel_refresh`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionCatalogState {
    Initial,
    Loading,
    Ready,
    Error(String),
}

pub struct NeoismAgentSidePanel {
    width: f32,
    focused: bool,
    /// User-driven open/close — set by the bottom-right toggle button.
    /// When `true`, the renderer skips drawing the side panel column
    /// entirely (the carve helper returns `None`) so the chat area
    /// reclaims the full pane width.
    user_hidden: bool,
    /// Bottom-right toggle button rect, cached every frame so the
    /// click handler can hit-test it without recomputing the layout.
    toggle_button_rect: Option<[f32; 4]>,
    mode: SidePanelMode,
    scroll: Scroll,
    cursor_spring: CriticallyDampedSpring,
    scroll_top: usize,
    /// Continuous pixel scroll position for the home (sessions) list —
    /// the target `self.scroll` animates toward. Drives pixel-precise
    /// wheel / trackpad scrolling (no whole-row quantization dead zone);
    /// `scroll_top` is derived as `floor(scroll_px / row_h)`. Subagent
    /// mode uses `content_scroll_px` instead and leaves this at 0.
    scroll_px: f32,
    selected: usize,
    last_scroll_frame: Instant,
    last_cursor_frame: Instant,
    last_panel_height_rows: usize,
    /// Last screen rect the panel was rendered into. Click / wheel
    /// handlers in `screen::bridges::agent` read this to decide whether
    /// pointer events belong to the panel without having to recompute
    /// the carve.
    last_panel_rect: Option<[f32; 4]>,
    /// Screen rect for the actual selectable row list. In chat mode the
    /// branch rows sit below session/usage headers, so hit-testing the
    /// whole panel would map clicks to the wrong row. The rect is
    /// clamped to the visible viewport (so a click in a lower section
    /// is rejected), while `last_row_origin_y` keeps the *full* list's
    /// top — which can sit above the viewport once scrolled — so the
    /// row math stays anchored correctly.
    last_row_hit_rect: Option<[f32; 4]>,
    last_row_hit_height: f32,
    last_row_origin_y: f32,
    /// Where the selected row's cursor caret should sit. The screen
    /// render polls this and routes the global trail cursor to it when
    /// the panel owns focus — same pattern file_tree uses so Alt+Right
    /// animates the actual block cursor into the panel.
    selected_cursor_rect: Option<[f32; 4]>,
    /// Flat, unfiltered source list of home-mode sessions (no header
    /// rows). `sessions` is derived from this by
    /// [`rebuild_session_display`](Self::rebuild_session_display) — sorted
    /// pinned-first / newest-day-first, filtered by `session_query`, with
    /// cyan date-group header rows injected.
    all_sessions: Vec<NeoismAgentSessionEntry>,
    /// Live search filter for the home-mode session list. Typed while the
    /// panel owns focus; matches session titles case-insensitively.
    session_query: String,
    /// Query the current `semantic_results` were fetched for (trimmed).
    /// Results only apply while it still matches the live filter, so a
    /// stale fetch that raced further typing never mislabels the list.
    semantic_query: String,
    /// Sessions matched by server-side semantic transcript search,
    /// best-first. Unioned into the display list with title matches.
    semantic_results: Vec<NeoismAgentSemanticMatch>,
    /// True while a semantic transcript search is in flight — the empty
    /// list state renders a skeleton shimmer instead of "No results".
    semantic_searching: bool,
    semantic_search_started: Option<Instant>,
    /// Monospace column budget for wrapping semantic excerpt chunks into
    /// list rows, fed back from the renderer's measured panel width.
    result_wrap_columns: usize,
    /// True when the selection cursor sits on the search row (reached by
    /// arrow-up past the first session, or by typing). While set, the
    /// trail cursor renders on the search field and no session row is
    /// highlighted.
    search_focused: bool,
    sessions: Vec<NeoismAgentSessionEntry>,
    session_catalog_state: SessionCatalogState,
    /// Opaque keyset cursor returned by the session catalogue. `None` means
    /// the successful page was final.
    session_next_cursor: Option<String>,
    /// Prevents repeated wheel/touch events at the bottom from launching the
    /// same continuation request more than once.
    session_page_loading: bool,
    session_requested_cursor: Option<String>,
    session_request_generation: u64,
    session_refresh_attempts: u8,
    /// Phase origin for the first-load session skeleton. The panel starts
    /// unloaded, so this clock can begin with the panel state and stops
    /// participating in redraws as soon as `set_sessions` lands.
    sessions_loading_started: Instant,
    /// Last time we kicked off a refresh of the sessions list.
    /// Used to debounce refreshes when the home view re-renders every
    /// frame.
    last_sessions_refresh: Option<Instant>,
    /// Sub-agent / sibling session list shown in chat mode. First entry
    /// is always "main session" (the parent), followed by children — the
    /// shape `fetch_subagent_options` returns. We render the section
    /// only when this list has at least one *non-main* entry.
    subagents: Vec<NeoismAgentSessionEntry>,
    /// Session whose transcript is currently open in the pane. Completed
    /// children are retained only when we observed them running while viewed.
    viewed_session_id: Option<String>,
    /// Ephemeral hold for a viewed child observed live before it finished.
    /// Never populate this from completed recovery data: doing so would
    /// resurrect dead sessions after startup or reconnect.
    retained_viewed_subagent_id: Option<String>,
    subagents_loaded: bool,
    /// Only one branch-tree snapshot may be in flight at a time. The
    /// generation invalidates an older worker when the viewed session or
    /// live branch tree changes, preventing late snapshots from replacing
    /// newer event-driven state.
    subagent_refresh_in_flight: bool,
    subagent_refresh_generation: u64,
    /// Per-session activity snapshot used to paint the indented
    /// connector + tool + status dot under each branch row. Keyed by
    /// session id so it survives sub-agent list reorderings.
    branch_activities: HashMap<String, BranchActivity>,
    /// The session's persistent goal, refreshed from
    /// `GET /session/:id/goal` on session change / `SESSION_UPDATED`.
    /// Rendered above the task list when its `text` is non-empty.
    session_goal: Option<SessionGoal>,
    /// Last time a goal refetch was kicked. `None` forces the next
    /// `should_refresh_goal` to fire (used on session change /
    /// `SESSION_UPDATED`).
    last_goal_refresh: Option<Instant>,
    /// Set true once the goal has been fetched at least once for the
    /// current session, so we don't refetch every frame.
    goal_loaded: bool,
    /// Monotonic version (backend `updated` millis) of the most recent
    /// goal state we applied. A slow `GET /goal` poll that returns an
    /// OLDER snapshot than a live event already applied is dropped, so
    /// the Goal section never flickers active → stale → active.
    goal_version: u64,
    /// Pixel scroll offset for the *whole* chat-mode content column
    /// (Directory → Session → Usage → Goal → Branches → Tasks). Unlike
    /// `scroll_top` (the home-view row cursor), this lets every section
    /// scroll as one viewport so nothing falls off the bottom out of
    /// reach. Clamped against `content_scroll_max` each frame.
    content_scroll_px: f32,
    content_scroll_max: f32,
    /// When a live conversation is open, the "← Back" affordance flips the
    /// panel to the home/recent-sessions view *without* ending the chat.
    /// The renderer shows the sessions list whenever this is set (or there
    /// is no conversation at all); activating a session clears it.
    show_home_override: bool,
    /// True when the selection cursor sits on the "← Back" affordance at the
    /// top of the panel (reached by arrow-up past the first row / search).
    /// Mirrors `search_focused`: while set, no row is highlighted and the
    /// trail cursor parks on the Back button.
    back_focused: bool,
    /// Screen rect of the "← Back" button, cached each frame it is drawn so
    /// the click handler can hit-test it and the focus model knows it is
    /// present. `None` when no Back button was drawn (genuine home view).
    back_button_rect: Option<[f32; 4]>,
    hovered_session: Option<usize>,
    session_hover_target: bool,
    session_hover_scale: f32,
    last_hover_frame: Instant,
    back_scramble_started: Option<Instant>,
    /// Hit target spanning the compact Usage meter and numeric label.
    usage_rect: Option<[f32; 4]>,
    /// Last values actually presented by the compact Usage surface.
    usage_snapshot: Option<(u64, Option<u64>)>,
    usage_scramble_started: Option<Instant>,
    usage_scramble_revision: u64,
    /// See [`StatusDisplayHold`] / [`STATUS_LABEL_GRACE`].
    status_hold: std::cell::RefCell<Option<StatusDisplayHold>>,
}

impl Default for NeoismAgentSidePanel {
    fn default() -> Self {
        Self {
            width: SIDE_PANEL_WIDTH,
            focused: false,
            // Default visible; the renderer still suppresses the panel
            // on narrow panes, and the user can toggle it with Alt+H or
            // the top-bar action.
            user_hidden: false,
            toggle_button_rect: None,
            mode: SidePanelMode::Sessions,
            scroll: Scroll::new().with_animation_length(SCROLL_ANIMATION_LENGTH),
            cursor_spring: CriticallyDampedSpring::new(),
            scroll_top: 0,
            scroll_px: 0.0,
            selected: 0,
            last_scroll_frame: Instant::now(),
            last_cursor_frame: Instant::now(),
            last_panel_height_rows: 1,
            last_panel_rect: None,
            last_row_hit_rect: None,
            last_row_hit_height: ROW_HEIGHT,
            last_row_origin_y: 0.0,
            selected_cursor_rect: None,
            all_sessions: Vec::new(),
            session_query: String::new(),
            semantic_query: String::new(),
            semantic_results: Vec::new(),
            semantic_searching: false,
            semantic_search_started: None,
            result_wrap_columns: 42,
            search_focused: false,
            session_next_cursor: None,
            session_page_loading: false,
            session_requested_cursor: None,
            session_request_generation: 0,
            session_refresh_attempts: 0,
            sessions: Vec::new(),
            session_catalog_state: SessionCatalogState::Initial,
            sessions_loading_started: Instant::now(),
            last_sessions_refresh: None,
            subagents: Vec::new(),
            viewed_session_id: None,
            retained_viewed_subagent_id: None,
            subagents_loaded: false,
            subagent_refresh_in_flight: false,
            subagent_refresh_generation: 0,
            branch_activities: HashMap::new(),
            session_goal: None,
            last_goal_refresh: None,
            goal_loaded: false,
            goal_version: 0,
            content_scroll_px: 0.0,
            content_scroll_max: 0.0,
            show_home_override: false,
            back_focused: false,
            back_button_rect: None,
            hovered_session: None,
            session_hover_target: false,
            session_hover_scale: 0.0,
            last_hover_frame: Instant::now(),
            back_scramble_started: None,
            usage_rect: None,
            usage_snapshot: None,
            usage_scramble_started: None,
            usage_scramble_revision: 0,
            status_hold: std::cell::RefCell::new(None),
        }
    }
}

impl NeoismAgentSidePanel {
    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.search_focused = false;
            self.back_focused = false;
        }
    }

    pub fn show_home_override(&self) -> bool {
        self.show_home_override
    }

    pub fn set_show_home_override(&mut self, value: bool) {
        self.show_home_override = value;
    }

    /// Flip the home-while-chatting view on/off. Non-destructive — the live
    /// conversation stays open underneath; only which list the panel shows
    /// changes.
    pub fn toggle_home_override(&mut self) {
        self.show_home_override = !self.show_home_override;
    }

    pub fn back_button_rect(&self) -> Option<[f32; 4]> {
        self.back_button_rect
    }

    pub fn set_back_button_rect(&mut self, rect: [f32; 4]) {
        self.back_button_rect = Some(rect);
    }

    /// Called when the panel renders a view that carries no Back button
    /// (the genuine no-conversation home). Drops the cached rect and any
    /// focus that was on it so the focus model doesn't strand the cursor.
    pub fn clear_back_button_rect(&mut self) {
        self.back_button_rect = None;
        self.back_focused = false;
    }

    pub fn back_button_contains(&self, x: f32, y: f32) -> bool {
        let Some([bx, by, bw, bh]) = self.back_button_rect else {
            return false;
        };
        x >= bx && x <= bx + bw && y >= by && y <= by + bh
    }

    pub fn tick_pointer_animations(&mut self, hovered_session: Option<usize>) {
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_hover_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_hover_frame = now;

        if let Some(hovered) = hovered_session {
            if Some(hovered) != self.hovered_session {
                self.hovered_session = Some(hovered);
                self.session_hover_scale = 0.0;
            }
        }
        self.session_hover_target = hovered_session.is_some();
        if self.hovered_session.is_none() {
            self.session_hover_scale = 0.0;
        }
        let target = if self.session_hover_target { 1.0 } else { 0.0 };
        let blend = 1.0 - (-dt * 18.0).exp();
        self.session_hover_scale += (target - self.session_hover_scale) * blend;
        if (self.session_hover_scale - target).abs() < 0.002 {
            self.session_hover_scale = target;
            if target == 0.0 {
                self.hovered_session = None;
            }
        }
    }

    /// Start the Back-label scramble after an actual activation. Hovering and
    /// view-label changes deliberately do not trigger this effect.
    pub fn trigger_back_scramble(&mut self) {
        self.back_scramble_started = Some(Instant::now());
    }

    pub fn hovered_session(&self) -> Option<usize> {
        self.hovered_session
    }

    pub fn session_hover_scale(&self) -> f32 {
        self.session_hover_scale
    }

    pub fn back_scramble_elapsed_ms(&self) -> Option<f32> {
        let elapsed = self.back_scramble_started?.elapsed().as_secs_f32() * 1000.0;
        (elapsed < 320.0).then_some(elapsed)
    }

    /// Update the compact Usage surface and rearm its scramble only when the
    /// displayed token tuple actually changes.
    pub fn update_usage_meter(&mut self, used: u64, context_limit: Option<u64>) {
        let snapshot = (used, context_limit);
        if self.usage_snapshot == Some(snapshot) {
            return;
        }
        self.usage_snapshot = Some(snapshot);
        self.usage_scramble_started = Some(Instant::now());
        self.usage_scramble_revision = self.usage_scramble_revision.wrapping_add(1);
    }

    pub fn usage_scramble_elapsed_ms(&self) -> Option<f32> {
        let elapsed = self.usage_scramble_started?.elapsed().as_secs_f32() * 1000.0;
        (elapsed < 320.0).then_some(elapsed)
    }

    pub fn usage_scramble_revision(&self) -> u64 {
        self.usage_scramble_revision
    }

    pub fn set_usage_rect(&mut self, rect: [f32; 4]) {
        self.usage_rect = Some(rect);
    }

    pub fn clear_usage_rect(&mut self) {
        self.usage_rect = None;
    }

    pub fn usage_contains(&self, x: f32, y: f32) -> bool {
        self.usage_rect.is_some_and(|[rx, ry, rw, rh]| {
            x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
        })
    }

    /// Whether the selection cursor is on the "← Back" affordance. Only
    /// meaningful while a Back button is actually being drawn.
    pub fn back_focused(&self) -> bool {
        self.back_focused && self.back_button_rect.is_some()
    }

    /// Move the selection cursor onto the "← Back" affordance.
    pub fn focus_back(&mut self) {
        self.back_focused = true;
        self.search_focused = false;
        self.cursor_spring.reset();
    }

    fn clear_back_focus(&mut self) {
        self.back_focused = false;
    }

    /// True when the Back affordance is the *only* thing worth landing a
    /// cursor on (chat with no branch rows, or an empty session list). The
    /// focus-entry drops the cursor straight onto Back in that case.
    pub fn only_back_focusable(&self) -> bool {
        self.back_button_rect.is_some()
            && match self.mode {
                SidePanelMode::Sessions => self.sessions.is_empty(),
                SidePanelMode::Subagents => self.subagents.len() <= 1,
            }
    }

    pub fn user_hidden(&self) -> bool {
        self.user_hidden
    }

    pub fn set_user_hidden(&mut self, hidden: bool) {
        self.user_hidden = hidden;
        if hidden {
            self.focused = false;
            self.selected_cursor_rect = None;
        }
    }

    /// Toggle the panel's visibility. When hiding, also drops keyboard
    /// focus so the cursor returns to the agent input.
    pub fn toggle_visibility(&mut self) {
        self.user_hidden = !self.user_hidden;
        if self.user_hidden {
            self.focused = false;
            self.selected_cursor_rect = None;
        }
    }

    #[allow(dead_code)]
    pub fn toggle_button_rect(&self) -> Option<[f32; 4]> {
        self.toggle_button_rect
    }

    pub fn set_toggle_button_rect(&mut self, rect: [f32; 4]) {
        self.toggle_button_rect = Some(rect);
    }

    pub fn clear_toggle_button_rect(&mut self) {
        self.toggle_button_rect = None;
    }

    pub fn toggle_button_contains(&self, x: f32, y: f32) -> bool {
        let Some([bx, by, bw, bh]) = self.toggle_button_rect else {
            return false;
        };
        x >= bx && x <= bx + bw && y >= by && y <= by + bh
    }

    #[allow(dead_code)]
    pub fn mode(&self) -> SidePanelMode {
        self.mode
    }

    /// Set by the renderer each frame so navigation / scroll / click
    /// operate on the correct list. Resets selection cursor if the
    /// active list shrinks below the previous index.
    pub fn set_mode(&mut self, mode: SidePanelMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.selected = 0;
        self.scroll_top = 0;
        self.scroll_px = 0.0;
        self.scroll.reset();
        self.cursor_spring.reset();
        self.content_scroll_px = 0.0;
    }

    /// The session's persistent goal, if any has been fetched.
    pub fn session_goal(&self) -> Option<&SessionGoal> {
        self.session_goal.as_ref()
    }

    /// Apply a goal update from a live event or a `GET /goal` poll,
    /// guarded by a monotonic `version` (backend `updated` millis).
    ///
    /// Two things this gets right that a bare assignment did not:
    ///  - **No flicker.** A slow poll can land *after* a newer live
    ///    event already advanced the goal; applying its stale snapshot
    ///    made the section blink active → old → active. We drop any
    ///    update that is not strictly newer than what we've shown.
    ///  - **Completed goals retire cleanly.** A `Complete` update hides the
    ///    finished goal while still advancing `goal_version`, so a stale
    ///    active snapshot cannot resurrect it and a genuinely newer goal can
    ///    replace it normally.
    ///
    /// `version == 0` means "unversioned" — a poll that found no goal.
    /// It must never clear a goal a live event just set (authoritative
    /// clears arrive as a versioned event), so a version-0 `None` is
    /// ignored.
    pub fn set_session_goal(&mut self, goal: Option<SessionGoal>, version: u64) {
        self.goal_loaded = true;
        if version != 0 && version <= self.goal_version {
            return;
        }
        if version == 0 && goal.is_none() {
            return;
        }
        if version != 0 {
            self.goal_version = version;
        }
        self.session_goal =
            goal.filter(|goal| !goal.is_empty() && goal.status != GoalStatus::Complete);
    }

    /// Reset the goal section for a session switch: drop the cached goal
    /// AND its version so the next session's goal (whatever its version)
    /// applies cleanly. Pair with `invalidate_goal_refresh` to force the
    /// refetch.
    pub fn reset_session_goal(&mut self) {
        self.session_goal = None;
        self.goal_version = 0;
        self.goal_loaded = false;
    }

    /// Optimistically clear the displayed goal after a user-driven
    /// `/goal clear` (or an empty `/goal`). Unlike [`reset_session_goal`]
    /// this keeps `goal_version`, so a slow in-flight `GET /goal` poll that
    /// still carries the just-removed goal (same version) is dropped by the
    /// monotonic guard instead of resurrecting the section — while the
    /// authoritative clear (a versioned `SESSION_UPDATED`) still applies.
    pub fn clear_session_goal_local(&mut self) {
        self.session_goal = None;
        self.goal_loaded = true;
    }

    /// True when the goal is stale enough to justify a refetch. Loaded
    /// goals refresh slowly (every few seconds) since live edits also
    /// arrive via `SESSION_UPDATED` → `invalidate_goal_refresh`.
    pub fn should_refresh_goal(&self) -> bool {
        if !self.goal_loaded {
            return self
                .last_goal_refresh
                .map(|last| {
                    Instant::now().saturating_duration_since(last).as_millis() >= 400
                })
                .unwrap_or(true);
        }
        self.last_goal_refresh
            .map(|last| {
                Instant::now().saturating_duration_since(last).as_secs_f32() >= 6.0
            })
            .unwrap_or(true)
    }

    pub fn mark_goal_refresh_kicked(&mut self) {
        self.last_goal_refresh = Some(Instant::now());
    }

    /// Force the next `should_refresh_goal` to fire — used on session
    /// change and when a `SESSION_UPDATED` event reports the goal may
    /// have changed, so the side panel reflects it promptly.
    pub fn invalidate_goal_refresh(&mut self) {
        self.last_goal_refresh = None;
        self.goal_loaded = false;
    }

    /// Current pixel scroll offset of the whole chat-mode content
    /// column. The renderer subtracts this from every section's `y`.
    pub fn content_scroll_px(&self) -> f32 {
        self.content_scroll_px
    }

    /// Record the total scrollable overflow for the chat-mode content
    /// column (content height minus viewport height, clamped at 0) and
    /// re-clamp the current offset against it. Called by the renderer
    /// once it knows the laid-out content height.
    pub fn set_content_scroll_max(&mut self, max: f32) {
        self.content_scroll_max = max.max(0.0);
        self.content_scroll_px =
            self.content_scroll_px.clamp(0.0, self.content_scroll_max);
    }

    /// Scroll the whole chat-mode content column by `delta_pixels`
    /// (positive scrolls the content up / reveals lower sections),
    /// clamped to the known overflow. Returns whether the offset moved.
    pub fn scroll_content_pixels(&mut self, delta_pixels: f32) -> bool {
        let before = self.content_scroll_px;
        self.content_scroll_px =
            (self.content_scroll_px + delta_pixels).clamp(0.0, self.content_scroll_max);
        self.content_scroll_px != before
    }

    pub fn sessions(&self) -> &[NeoismAgentSessionEntry] {
        &self.sessions
    }

    pub fn invalidate_session_catalog(&mut self) {
        self.all_sessions.clear();
        self.sessions.clear();
        self.session_next_cursor = None;
        self.session_requested_cursor = None;
        self.session_page_loading = false;
        self.session_request_generation = self.session_request_generation.saturating_add(1);
        self.session_refresh_attempts = 0;
        self.session_catalog_state = SessionCatalogState::Initial;
        self.sessions_loading_started = Instant::now();
        self.last_sessions_refresh = None;
        self.scroll_top = 0;
        self.scroll_px = 0.0;
        self.scroll.reset();
    }

    pub fn sessions_loaded(&self) -> bool {
        matches!(
            self.session_catalog_state,
            SessionCatalogState::Ready | SessionCatalogState::Error(_)
        )
    }

    pub fn session_catalog_state(&self) -> &SessionCatalogState {
        &self.session_catalog_state
    }

    /// Seconds since the initial session-list skeleton began.
    pub fn sessions_loading_elapsed(&self) -> f32 {
        Instant::now()
            .saturating_duration_since(self.sessions_loading_started)
            .as_secs_f32()
    }

    pub fn subagents(&self) -> &[NeoismAgentSessionEntry] {
        &self.subagents
    }

    pub fn set_viewed_session_id(&mut self, session_id: Option<String>) {
        let session_id = session_id.filter(|id| !id.is_empty());
        if self.retained_viewed_subagent_id.as_deref() != session_id.as_deref() {
            self.retained_viewed_subagent_id = None;
        }
        self.viewed_session_id = session_id;
        let Some(viewed) = self.viewed_session_id.as_deref() else {
            return;
        };
        let active = self
            .branch_activities
            .get(viewed)
            .is_some_and(|activity| {
                matches!(
                    activity.status,
                    BranchStatus::Active | BranchStatus::WaitingPermission
                )
            })
            || self
                .subagents
                .iter()
                .find(|entry| entry.id == viewed)
                .and_then(|entry| entry.runtime_status.as_deref())
                .and_then(BranchStatus::from_runtime_status)
                .is_some_and(|status| {
                    matches!(
                        status,
                        BranchStatus::Active | BranchStatus::WaitingPermission
                    )
                });
        if active {
            self.retained_viewed_subagent_id = Some(viewed.to_string());
        }
    }

    /// Record the non-idle status the composer row is displaying this
    /// frame. `elapsed_seconds` is the timer currently shown with it,
    /// carried into the hold so the animation phase stays continuous if
    /// the raw derivation dips through Idle next frame. Idle is never
    /// recorded — the hold only ever bridges *away* from activity.
    pub fn note_status_display(
        &self,
        state: crate::panels::agent_pane::state::NeoismAgentStreamingState,
        elapsed_seconds: Option<f32>,
    ) {
        if state == crate::panels::agent_pane::state::NeoismAgentStreamingState::Idle {
            return;
        }
        let now = Instant::now();
        let elapsed_anchor = elapsed_seconds
            .and_then(|seconds| {
                now.checked_sub(Duration::from_secs_f32(seconds.max(0.0)))
            })
            .unwrap_or(now);
        *self.status_hold.borrow_mut() = Some(StatusDisplayHold {
            state,
            elapsed_anchor,
            last_seen: now,
            session: self.viewed_session_id.clone(),
        });
    }

    fn active_status_hold(&self) -> Option<StatusDisplayHold> {
        let hold = self.status_hold.borrow();
        let hold = hold.as_ref()?;
        if hold.session != self.viewed_session_id {
            return None;
        }
        (Instant::now().saturating_duration_since(hold.last_seen) < STATUS_LABEL_GRACE)
            .then(|| hold.clone())
    }

    /// The recently-displayed status, if the raw derivation went idle
    /// less than [`STATUS_LABEL_GRACE`] ago and the viewed session
    /// hasn't changed since it was recorded. `None` once idle has
    /// persisted for the whole window — the label may then clear (and
    /// the status row collapse) as a genuine completion.
    pub fn held_status_display(
        &self,
    ) -> Option<crate::panels::agent_pane::state::NeoismAgentStreamingState> {
        self.active_status_hold().map(|hold| hold.state)
    }

    /// Continuation of the held status's elapsed timer, so the display
    /// clock doesn't jump to zero while the hold bridges two events.
    pub fn held_status_elapsed_seconds(&self) -> Option<f32> {
        self.active_status_hold().map(|hold| {
            Instant::now()
                .saturating_duration_since(hold.elapsed_anchor)
                .as_secs_f32()
        })
    }

    /// Drop the hold immediately — used by hard stops (user abort, new
    /// conversation) where a lingering label would read as lag.
    pub fn clear_status_display_hold(&self) {
        *self.status_hold.borrow_mut() = None;
    }

    /// Test support: age the hold past [`STATUS_LABEL_GRACE`] without
    /// sleeping. Falls back to dropping the hold when the platform
    /// clock can't be rewound that far.
    pub fn rewind_status_display_hold(&self, by: Duration) {
        let mut hold = self.status_hold.borrow_mut();
        if let Some(inner) = hold.as_mut() {
            match inner.last_seen.checked_sub(by) {
                Some(last_seen) => inner.last_seen = last_seen,
                None => *hold = None,
            }
        }
    }

    /// Whether `entry` (a non-main branch) should be hidden from the
    /// sidebar because it finished more than [`SUBAGENT_HIDE_AFTER`]
    /// ago and isn't currently running again. The main session (index
    /// 0) is never hidden — it's the user's anchor back to the parent.
    /// A respawned sub-agent reports `Active`/`WaitingPermission`
    /// (which clears `completed_at`) so it stays visible.
    fn subagent_hidden(&self, entry: &NeoismAgentSessionEntry) -> bool {
        // Keep only a child observed live while its transcript was open.
        // Merely viewing a historical completed child must not revive it.
        if self.retained_viewed_subagent_id.as_deref() == Some(entry.id.as_str()) {
            return false;
        }
        // Runtime status reported straight on the entry takes
        // precedence — a live respawn keeps the row regardless of any
        // stale activity snapshot.
        if let Some(status) = entry
            .runtime_status
            .as_deref()
            .and_then(BranchStatus::from_runtime_status)
        {
            if matches!(
                status,
                BranchStatus::Active | BranchStatus::WaitingPermission
            ) {
                return false;
            }
        }
        let Some(activity) = self.branch_activities.get(&entry.id) else {
            return false;
        };
        if matches!(
            activity.status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) {
            return false;
        }
        match activity.completed_at {
            // A live completion stamped the window — show it until it
            // elapses, then hide.
            Some(at) => {
                Instant::now().saturating_duration_since(at) >= SUBAGENT_HIDE_AFTER
            }
            // Terminal but no window: finished before we were watching (or
            // a re-listed old completion). Hide it immediately.
            None => true,
        }
    }

    /// Drop finished sub-agents whose auto-hide window has elapsed.
    /// Called by the renderer each frame (cheap — usually a no-op) so
    /// the hide fires ~[`SUBAGENT_HIDE_AFTER`] after completion even
    /// without a server refresh, and keeps `self.subagents` aligned
    /// with what's drawn so selection/scroll indices stay valid. The
    /// main session at index 0 is always retained. Returns `true` if
    /// anything was removed (so the caller can mark the frame dirty).
    pub fn prune_expired_completed_subagents(&mut self) -> bool {
        if self.subagents.len() <= 1 {
            return false;
        }
        let selected_id = self
            .subagents
            .get(self.selected)
            .map(|entry| entry.id.clone());
        let hidden: Vec<bool> = self
            .subagents
            .iter()
            .enumerate()
            .map(|(ix, entry)| ix != 0 && self.subagent_hidden(entry))
            .collect();
        if !hidden.iter().any(|&h| h) {
            return false;
        }
        let mut ix = 0;
        self.subagents.retain(|_| {
            let keep = !hidden[ix];
            ix += 1;
            keep
        });
        // Keep the cursor on the same logical branch; clamp if it was
        // the row that just got hidden.
        if let Some(index) = selected_id
            .as_deref()
            .and_then(|id| self.subagents.iter().position(|entry| entry.id == id))
        {
            self.selected = index;
        } else if self.selected >= self.subagents.len() {
            self.selected = self.subagents.len().saturating_sub(1);
        }
        if self.scroll_top >= self.subagents.len() {
            self.scroll_top = self.subagents.len().saturating_sub(1);
        }
        true
    }

    #[allow(dead_code)]
    pub fn subagents_loaded(&self) -> bool {
        self.subagents_loaded
    }

    /// Current list — sessions or subagents — keyed off `mode`.
    pub fn active_rows(&self) -> &[NeoismAgentSessionEntry] {
        match self.mode {
            SidePanelMode::Sessions => &self.sessions,
            SidePanelMode::Subagents => &self.subagents,
        }
    }

    /// Whether Alt+Right should claim focus on this panel. Home mode
    /// is focusable whenever there's any session row. Chat mode is
    /// focusable only when the Branches section actually renders —
    /// i.e. there's at least one real child below the implicit "main
    /// session" entry the picker prepends.
    pub fn focusable(&self) -> bool {
        // A hidden panel is never focusable, whatever rows it last held.
        if self.user_hidden {
            return false;
        }
        // The "← Back" affordance (drawn whenever a live conversation is
        // open) is always reachable, so the panel is focusable even in a
        // chat with no sub-agent branches to land a row cursor on.
        if self.back_button_rect.is_some() {
            return true;
        }
        match self.mode {
            SidePanelMode::Sessions => !self.sessions.is_empty(),
            SidePanelMode::Subagents => self.subagents.len() > 1,
        }
    }

    pub fn selected_row(&self) -> Option<&NeoismAgentSessionEntry> {
        self.active_rows().get(self.selected)
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    pub fn last_panel_height_rows(&self) -> usize {
        self.last_panel_height_rows
    }

    pub fn set_last_panel_height_rows(&mut self, rows: usize) {
        self.last_panel_height_rows = rows.max(1);
    }

    pub fn last_panel_rect(&self) -> Option<[f32; 4]> {
        self.last_panel_rect
    }

    pub fn set_last_panel_rect(&mut self, rect: [f32; 4]) {
        self.last_panel_rect = Some(rect);
    }

    pub fn set_row_hit_rect(&mut self, rect: [f32; 4], row_height: f32) {
        self.last_row_hit_rect = Some(rect);
        self.last_row_hit_height = row_height.max(1.0);
        self.last_row_origin_y = rect[1];
    }

    /// Like [`set_row_hit_rect`] but for the chat-mode branch list,
    /// where the visible `rect` is clamped to the viewport while
    /// `origin_y` is the (possibly off-screen) top of the full list —
    /// the anchor the row math counts from.
    pub fn set_row_hit_rect_with_origin(
        &mut self,
        rect: [f32; 4],
        origin_y: f32,
        row_height: f32,
    ) {
        self.last_row_hit_rect = Some(rect);
        self.last_row_hit_height = row_height.max(1.0);
        self.last_row_origin_y = origin_y;
    }

    pub fn clear_row_hit_rect(&mut self) {
        self.last_row_hit_rect = None;
    }

    /// Clear the cached rect when the pane has gone too narrow to host
    /// the panel. Also drops focus — a panel that isn't on screen
    /// shouldn't keep keyboard focus.
    pub fn clear_last_panel_rect(&mut self) {
        self.last_panel_rect = None;
        self.last_row_hit_rect = None;
        self.back_button_rect = None;
        self.back_focused = false;
        self.focused = false;
        self.selected_cursor_rect = None;
        self.usage_rect = None;
    }

    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        self.selected_cursor_rect
    }

    pub fn set_selected_cursor_rect(&mut self, rect: [f32; 4]) {
        self.selected_cursor_rect = Some(rect);
    }

    pub fn clear_selected_cursor_rect(&mut self) {
        self.selected_cursor_rect = None;
    }

    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        let Some([px, py, pw, ph]) = self.last_panel_rect else {
            return false;
        };
        x >= px && x <= px + pw && y >= py && y <= py + ph
    }

    /// Kept for the home-mode click path that always wants a session
    /// (not a subagent). Subagent click uses `selected_row()` instead.
    pub fn selected_session(&self) -> Option<&NeoismAgentSessionEntry> {
        if self.search_focused() {
            return None;
        }
        self.sessions.get(self.selected).filter(|e| !e.is_header)
    }

    /// Replace the cached sessions list. The incoming list is a *flat*
    /// set of session rows (no headers); the displayed `sessions` list is
    /// derived from it — filtered by `session_query`, sorted pinned-first
    /// / newest-day-first, with date-group header rows injected. Resets
    /// selection / scroll on the sessions axis only when home mode is
    /// active — flipping modes already resets these.
    pub fn set_sessions(&mut self, sessions: Vec<NeoismAgentSessionEntry>) {
        self.set_session_page(sessions, None, None);
    }

    /// Apply a catalogue page. First pages replace and reset the home list;
    /// continuation pages append by id while preserving the viewport.
    pub fn set_session_page(
        &mut self,
        sessions: Vec<NeoismAgentSessionEntry>,
        requested_cursor: Option<&str>,
        next_cursor: Option<String>,
    ) {
        let was_home = matches!(self.mode, SidePanelMode::Sessions);
        if requested_cursor.is_some() {
            let mut known = self
                .all_sessions
                .iter()
                .map(|session| session.id.clone())
                .collect::<HashSet<_>>();
            self.all_sessions.extend(
                sessions
                    .into_iter()
                    .filter(|session| known.insert(session.id.clone())),
            );
        } else {
            self.all_sessions = sessions;
        }
        self.session_next_cursor = next_cursor;
        self.session_page_loading = false;
        self.session_requested_cursor = None;
        self.session_refresh_attempts = 0;
        self.session_catalog_state = SessionCatalogState::Ready;
        self.rebuild_session_display();
        if was_home && requested_cursor.is_none() {
            self.scroll_px = 0.0;
            self.scroll.reset();
            self.cursor_spring.reset();
        }
    }

    pub fn session_next_cursor(&self) -> Option<&str> {
        self.session_next_cursor.as_deref()
    }

    pub fn session_requested_cursor(&self) -> Option<&str> {
        self.session_requested_cursor.as_deref()
    }

    pub fn session_page_loading(&self) -> bool {
        self.session_page_loading
    }

    pub fn next_session_request_generation(&mut self) -> u64 {
        self.session_request_generation = self.session_request_generation.saturating_add(1);
        self.session_request_generation
    }

    pub fn session_request_is_current(&self, generation: u64) -> bool {
        self.session_request_generation == generation
    }

    /// Claim the continuation cursor once the viewport is within four rows
    /// of the loaded end. The claim is single-flight until a page or failure
    /// settles it.
    pub fn begin_session_page_near_end(&mut self) -> Option<String> {
        if self.session_page_loading
            || !matches!(self.mode, SidePanelMode::Sessions)
            || !self.session_query.trim().is_empty()
            || self.scroll_top.saturating_add(self.last_panel_height_rows + 4)
                < self.active_len()
        {
            return None;
        }
        let cursor = self.session_next_cursor.clone()?;
        self.session_page_loading = true;
        self.session_requested_cursor = Some(cursor.clone());
        self.last_sessions_refresh = Some(Instant::now());
        self.sessions_loading_started = Instant::now();
        Some(cursor)
    }

    pub fn settle_session_page_error(&mut self, message: impl Into<String>) {
        self.session_page_loading = false;
        self.session_requested_cursor = None;
        if self.all_sessions.is_empty() {
            self.session_catalog_state = SessionCatalogState::Error(message.into());
        }
    }

    /// The live home-mode search filter.
    pub fn session_query(&self) -> &str {
        &self.session_query
    }

    /// Replace the home-mode search filter and rebuild the display list.
    pub fn set_session_query(&mut self, query: String) {
        if self.session_query == query {
            return;
        }
        self.session_query = query;
        self.rebuild_session_display();
        self.selected = self.nearest_selectable(0).unwrap_or(0);
        self.scroll_top = 0;
        self.scroll_px = 0.0;
        self.scroll.reset();
        self.cursor_spring.reset();
    }

    /// Append typed text to the home-mode search filter. Typing moves the
    /// cursor onto the search row.
    pub fn push_session_query(&mut self, text: &str) {
        self.search_focused = true;
        let mut query = self.session_query.clone();
        query.push_str(text);
        self.set_session_query(query);
    }

    /// Delete the last character of the home-mode search filter.
    pub fn backspace_session_query(&mut self) {
        let mut query = self.session_query.clone();
        if query.pop().is_some() {
            self.set_session_query(query);
        }
    }

    /// Clear the home-mode search filter.
    pub fn clear_session_query(&mut self) {
        if !self.session_query.is_empty() {
            self.set_session_query(String::new());
        }
    }

    /// Install semantic transcript-search results (best chunk per session,
    /// closest-first) fetched for `query`, and fold them into the display
    /// list: matched sessions join the title filter and each shows its
    /// matched excerpt chunk beneath the row. A result set for a query the
    /// user has already typed past is stored but simply won't apply until
    /// queries line up again.
    pub fn set_semantic_results(
        &mut self,
        query: String,
        results: Vec<NeoismAgentSemanticMatch>,
    ) {
        self.semantic_query = query.trim().to_string();
        self.semantic_results = results;
        self.rebuild_session_display();
    }

    /// Mark a semantic transcript search as started/finished so the
    /// empty state can shimmer instead of declaring "No results" while
    /// hits may still arrive.
    pub fn set_semantic_searching(&mut self, searching: bool) {
        if searching && !self.semantic_searching {
            self.semantic_search_started = Some(Instant::now());
        }
        if !searching {
            self.semantic_search_started = None;
        }
        self.semantic_searching = searching;
    }

    pub fn semantic_searching(&self) -> bool {
        self.semantic_searching
    }

    /// Feed the renderer's measured monospace column budget back so excerpt
    /// chunks wrap to the actual panel width. Rebuilds the display list only
    /// when the budget actually changes (a resize), so calling this every
    /// frame is free.
    pub fn set_result_wrap_columns(&mut self, columns: usize) {
        let columns = columns.clamp(16, 160);
        if columns != self.result_wrap_columns {
            self.result_wrap_columns = columns;
            self.rebuild_session_display();
        }
    }

    /// Seconds since the in-flight semantic search began.
    pub fn semantic_search_elapsed(&self) -> f32 {
        self.semantic_search_started
            .map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            })
            .unwrap_or(0.0)
    }

    /// Rebuild the displayed `sessions` list from `all_sessions`: filter
    /// by `session_query`, sort pinned-first / newest-day-first, and
    /// inject "Pinned" + date-group header rows. Semantic matches are
    /// unioned in with title matches while their query is current.
    fn rebuild_session_display(&mut self) {
        use crate::panels::agent_pane::session_group::section_label_at;

        let needle = self.session_query.trim().to_lowercase();
        // Search terms for excerpt highlighting: whitespace-split words of
        // two characters or more, already lowercased with the needle.
        let terms: Vec<String> = needle
            .split_whitespace()
            .filter(|word| word.len() >= 2)
            .map(str::to_string)
            .collect();
        let semantic_active =
            !needle.is_empty() && self.semantic_query == self.session_query.trim();
        let semantic: std::collections::HashMap<&str, &NeoismAgentSemanticMatch> =
            if semantic_active {
                self.semantic_results
                    .iter()
                    .map(|hit| (hit.session_id.as_str(), hit))
                    .collect()
            } else {
                std::collections::HashMap::new()
            };
        let mut visible: Vec<NeoismAgentSessionEntry> = self
            .all_sessions
            .iter()
            .filter(|entry| {
                needle.is_empty()
                    || entry.title.to_lowercase().contains(&needle)
                    || semantic.contains_key(entry.id.as_str())
            })
            .cloned()
            .collect();
        // Pinned first, then newest updated time. Stable so equal keys
        // keep the source order.
        visible.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_ms.cmp(&a.updated_ms))
        });

        let mut out = Vec::with_capacity(visible.len() + 8);
        for i in 0..visible.len() {
            if let Some(label) =
                section_label_at(i, |k| visible[k].pinned, |k| visible[k].updated_ms)
            {
                out.push(NeoismAgentSessionEntry::header(label));
            }
            out.push(visible[i].clone());
            // The matched transcript chunk renders as its own rows under the
            // session — word-wrapped to the measured panel width, one list
            // row per line, so the fixed-height virtualized list stays
            // uniform while the chunk reads like real text.
            if let Some(hit) = semantic.get(visible[i].id.as_str()) {
                let excerpt = compact_excerpt(&hit.excerpt);
                for line in wrap_excerpt_lines(&excerpt, self.result_wrap_columns, 3) {
                    let mut row = NeoismAgentSessionEntry::excerpt(
                        visible[i].id.clone(),
                        line,
                    );
                    row.highlights = term_highlight_ranges(&row.title, &terms);
                    out.push(row);
                }
            }
        }
        self.sessions = out;

        // Selection / scroll indices only refer to the sessions list in
        // home mode; in chat (subagent) mode they belong to `subagents`,
        // so leave them untouched there.
        if matches!(self.mode, SidePanelMode::Sessions) {
            if self.selected >= self.sessions.len() {
                self.selected = self.sessions.len().saturating_sub(1);
            }
            self.snap_selection_to_selectable();
            if self.scroll_top >= self.sessions.len() {
                self.scroll_top = self.sessions.len().saturating_sub(1);
            }
        }
    }

    pub fn set_subagents(&mut self, mut subagents: Vec<NeoismAgentSessionEntry>) {
        let was_subagents = matches!(self.mode, SidePanelMode::Subagents);
        // Recovery snapshots can briefly know only that a child is a generic
        // "subagent", while live task/status events already supplied its real
        // title and agent. Refresh runtime/depth from the snapshot, but never let
        // placeholder metadata downgrade a row the user has already seen.
        for incoming in &mut subagents {
            if let Some(previous) = self
                .subagents
                .iter()
                .find(|previous| previous.id == incoming.id)
            {
                preserve_specific_subagent_metadata(incoming, previous);
            } else if incoming.runtime_status.is_none() {
                // On first bootstrap, children absent from the active-run map
                // are historical/idle. Subsequent recovery snapshots retain
                // the event-driven state above instead of repeating this
                // inference and flickering an active child off.
                incoming.runtime_status = Some("completed".to_string());
            }
        }
        // A branch discovered by the live event stream is newer than a
        // recovery snapshot that started before that event. If such a
        // snapshot omits an agent that is still authoritatively active, retain
        // its last known row. This
        // keeps both the sidebar and the footer's active count stable until a
        // real terminal lifecycle event arrives.
        let incoming_ids = subagents
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<HashSet<_>>();
        let missing_active = self
            .subagents
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                *index > 0
                    && !incoming_ids.contains(&entry.id)
                    && (self
                        .branch_activities
                        .get(&entry.id)
                        .is_some_and(|activity| {
                            matches!(
                                activity.status,
                                BranchStatus::Active | BranchStatus::WaitingPermission
                            )
                        })
                        || self.retained_viewed_subagent_id.as_deref()
                            == Some(entry.id.as_str()))
            })
            .map(|(_, entry)| entry.clone())
            .collect::<Vec<_>>();
        subagents.extend(missing_active);
        // Capture the id of the row the cursor was on so it survives a
        // re-fetch. The subagent list is rebuilt wholesale on every
        // refresh (and after a session switch invalidates it), so a raw
        // index would silently re-point the highlight at a *different*
        // branch whenever the list reorders or grows — the "switching
        // between sub agents seems odd" report. Preserve by id instead,
        // mirroring how the picker keeps its selection across filters.
        let previously_selected_id = self
            .subagents
            .get(self.selected)
            .map(|entry| entry.id.clone());
        // Whether the set of rows actually changed. A status-only recovery
        // snapshot (same ids) must not reset scroll/selection springs.
        let ids_changed = self.subagents.len() != subagents.len()
            || self
                .subagents
                .iter()
                .zip(subagents.iter())
                .any(|(old, new)| old.id != new.id);
        self.subagents = subagents;
        self.subagents_loaded = true;
        // Reconcile explicit statuses from a bootstrap/reconnect snapshot.
        // Collect first to avoid borrowing `self.subagents` while mutating
        // `self.branch_activities`.
        let reconciled = self
            .subagents
            .iter()
            .filter_map(|entry| {
                entry
                    .runtime_status
                    .as_deref()
                    .and_then(BranchStatus::from_runtime_status)
                    .map(|status| (entry.id.clone(), status))
            })
            .collect::<Vec<_>>();
        for (id, status) in reconciled {
            self.reconcile_snapshot_branch_activity_status(id, status);
        }
        if was_subagents {
            // Restore the cursor onto the same logical branch it was on
            // before the refresh, falling back to a clamped index when
            // that branch is gone (e.g. a completed subagent dropped out
            // of the list).
            if let Some(index) = previously_selected_id
                .as_deref()
                .and_then(|id| self.subagents.iter().position(|entry| entry.id == id))
            {
                self.selected = index;
            } else if self.selected >= self.subagents.len() {
                self.selected = self.subagents.len().saturating_sub(1);
            }
            if self.scroll_top >= self.subagents.len() {
                self.scroll_top = self.subagents.len().saturating_sub(1);
            }
            if ids_changed {
                self.scroll.reset();
                self.cursor_spring.reset();
            }
        }
        // Drop finished sub-agents the backend re-listed (the /children
        // endpoint always returns completed children) before they can paint
        // for even one frame — this is what stopped already-hidden rows from
        // flashing back when a new sub-agent completes.
        self.prune_expired_completed_subagents();
    }

    pub fn ensure_subagent_main_entry(&mut self, id: impl Into<String>) {
        let id = id.into();
        if id.is_empty() || self.subagents.first().is_some_and(|entry| entry.id == id) {
            return;
        }
        self.subagents.insert(
            0,
            NeoismAgentSessionEntry::new(id, "main session", "return"),
        );
        self.subagents_loaded = true;
    }

    pub fn upsert_subagent(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        time_label: impl Into<String>,
    ) -> bool {
        let id = id.into();
        if id.is_empty() {
            return false;
        }
        let title = title.into();
        let time_label = time_label.into();
        if let Some(entry) = self.subagents.iter_mut().find(|entry| entry.id == id) {
            if !title.trim().is_empty()
                && (!generic_subagent_metadata(&title)
                    || generic_subagent_metadata(&entry.title))
            {
                entry.title = title;
            }
            if !time_label.trim().is_empty()
                && (!generic_subagent_metadata(&time_label)
                    || generic_subagent_metadata(&entry.time_label))
            {
                entry.time_label = time_label;
                entry.agent_kind =
                    AgentKind::from_label(&entry.time_label).or(entry.agent_kind);
            }
            return false;
        }
        let time_label = if time_label.trim().is_empty() {
            "subagent".to_string()
        } else {
            time_label
        };
        let entry = NeoismAgentSessionEntry::new(
            id,
            if title.trim().is_empty() {
                "subagent".to_string()
            } else {
                title
            },
            time_label.clone(),
        )
        .with_depth(1)
        .with_agent_kind(AgentKind::from_label(&time_label));
        self.subagents.push(entry);
        self.subagents_loaded = true;
        true
    }

    /// True when the sessions list is stale enough to justify a refetch.
    /// Used by the pane to debounce the refresh worker.
    pub fn should_refresh_sessions(&self) -> bool {
        if matches!(self.session_catalog_state, SessionCatalogState::Ready) {
            // Periodic first-page replacement races continuation pages and
            // resets a scrolled catalogue. Ready lists advance only through
            // their cursor; directory changes explicitly invalidate them.
            return false;
        }
        let Some(last) = self.last_sessions_refresh else {
            return true;
        };
        let retry_after = if matches!(self.session_catalog_state, SessionCatalogState::Error(_)) {
            if self.session_refresh_attempts >= 2 {
                return false;
            }
            0.75
        } else {
            8.0
        };
        Instant::now().saturating_duration_since(last).as_secs_f32() >= retry_after
    }

    pub fn mark_refresh_kicked(&mut self) {
        self.session_refresh_attempts = self.session_refresh_attempts.saturating_add(1);
        self.last_sessions_refresh = Some(Instant::now());
        if self.all_sessions.is_empty() {
            self.session_catalog_state = SessionCatalogState::Loading;
            self.sessions_loading_started = Instant::now();
        }
    }

    pub fn set_sessions_error(&mut self, message: impl Into<String>) {
        self.settle_session_page_error(message);
    }

    pub fn should_refresh_subagents(&self) -> bool {
        !self.subagent_refresh_in_flight && !self.subagents_loaded
    }

    /// Whether any tracked sub-agent is still active.
    fn has_active_subagents(&self) -> bool {
        let branch_active = self.branch_activities.values().any(|activity| {
            matches!(
                activity.status,
                BranchStatus::Active | BranchStatus::WaitingPermission
            )
        });
        branch_active
            || self.subagents.iter().any(|entry| {
                entry
                    .runtime_status
                    .as_deref()
                    .and_then(BranchStatus::from_runtime_status)
                    .is_some_and(|status| {
                        matches!(
                            status,
                            BranchStatus::Active | BranchStatus::WaitingPermission
                        )
                    })
            })
    }

    /// Claim the next branch-tree refresh. Returns a generation token that
    /// must accompany the result; `None` means another worker already owns
    /// the refresh or the debounce has not elapsed.
    pub fn begin_subagent_refresh(&mut self) -> Option<u64> {
        if !self.should_refresh_subagents() {
            return None;
        }
        self.subagent_refresh_generation =
            self.subagent_refresh_generation.wrapping_add(1);
        self.subagent_refresh_in_flight = true;
        Some(self.subagent_refresh_generation)
    }

    /// Release a refresh only when it is still the current generation.
    /// Stale workers must not clear the in-flight flag of a newer request.
    pub fn complete_subagent_refresh(&mut self, generation: u64) -> bool {
        if generation != self.subagent_refresh_generation {
            return false;
        }
        self.subagent_refresh_in_flight = false;
        true
    }

    /// Stop retrying a failed event-triggered snapshot every render frame.
    /// The last good rows remain authoritative; a later lifecycle event,
    /// session switch, or event-stream reconnect will invalidate the tree and
    /// request one new recovery snapshot.
    pub fn settle_failed_subagent_refresh(&mut self) {
        self.subagents_loaded = true;
    }

    /// Discard an in-flight snapshot after a newer live lifecycle/activity
    /// event arrives. The refresh worker cannot be cancelled, but its
    /// generation can be invalidated so an older snapshot never overwrites
    /// the event-driven branch truth and flickers the footer/sidebar.
    pub fn invalidate_inflight_subagent_refresh(&mut self) {
        if !self.subagent_refresh_in_flight {
            return;
        }
        self.subagent_refresh_generation =
            self.subagent_refresh_generation.wrapping_add(1);
        self.subagent_refresh_in_flight = false;
    }

    /// Force the next `should_refresh_*` to fire — used when the
    /// session changes (e.g. user picks a different sub-agent) so the
    /// list reflects the new parent immediately rather than waiting
    /// for the debounce to tick down.
    pub fn invalidate_subagent_refresh(&mut self) {
        self.subagent_refresh_generation =
            self.subagent_refresh_generation.wrapping_add(1);
        self.subagent_refresh_in_flight = false;
        self.subagents_loaded = false;
        self.subagents.clear();
    }

    pub fn mark_subagent_tree_dirty(&mut self) {
        self.subagent_refresh_generation =
            self.subagent_refresh_generation.wrapping_add(1);
        self.subagent_refresh_in_flight = false;
        self.subagents_loaded = false;
    }

    pub fn branch_activity(&self, session_id: &str) -> Option<&BranchActivity> {
        self.branch_activities.get(session_id)
    }

    /// Apply an *authoritative* lifecycle status to a branch (parent
    /// `task` tool part, child `session.status idle`,
    /// `session.subtask.completed`). A terminal status here latches
    /// `terminal_locked` so subsequent straggler part-level activity
    /// can't flip the row back to "active"; an active/respawn status
    /// clears the latch so a genuinely re-spawned child reappears.
    pub fn set_branch_activity_status(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
    ) {
        self.transition_branch_activity_status(
            session_id,
            status,
            BranchStatusTransition::AuthoritativeRun,
        );
    }

    /// Apply a reconnect/history lifecycle snapshot. Terminal branches are
    /// hidden immediately instead of receiving the live completion linger
    /// window; active branches may still reopen on a newer family revision.
    pub fn set_branch_activity_status_from_recovery(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
    ) {
        let session_id = session_id.into();
        if matches!(status, BranchStatus::Active | BranchStatus::WaitingPermission) {
            self.set_branch_activity_status(session_id, status);
            return;
        }
        let mut activity = BranchActivity::new(status, None, None);
        activity.terminal_locked = true;
        self.branch_activities.insert(session_id.clone(), activity);
        if let Some(entry) = self
            .subagents
            .iter_mut()
            .find(|entry| entry.id == session_id)
        {
            entry.runtime_status = Some(status.runtime_status().to_string());
        }
    }

    /// Apply a branch status with explicit lifecycle semantics. Returns
    /// `false` only when ancillary activity was rejected by a terminal lock.
    pub fn transition_branch_activity_status(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
        transition: BranchStatusTransition,
    ) -> bool {
        let session_id = session_id.into();
        if matches!(status, BranchStatus::Active | BranchStatus::WaitingPermission)
            && self.viewed_session_id.as_deref() == Some(session_id.as_str())
        {
            self.retained_viewed_subagent_id = Some(session_id.clone());
        }
        if transition == BranchStatusTransition::AncillaryActivity
            && matches!(
                status,
                BranchStatus::Active | BranchStatus::WaitingPermission
            )
            && self.branch_terminal_locked(&session_id)
        {
            return false;
        }
        let terminal = matches!(status, BranchStatus::Completed | BranchStatus::Stopped);
        self.branch_activities
            .entry(session_id.clone())
            .and_modify(|activity| {
                activity.transition_status(status);
                activity.terminal_locked = terminal;
            })
            .or_insert_with(|| {
                let mut activity = BranchActivity::new(status, None, None);
                activity.terminal_locked = terminal;
                activity
            });

        // The sidebar row and its BranchActivity used to be two independent
        // lifecycle stores. A live completion updated BranchActivity (and the
        // parent Task card), while the already-rendered row could retain its
        // snapshot-era `runtime_status: running` until opening the child
        // forced another snapshot. Keep the visible row on the same
        // authoritative edge so completion paints immediately in the parent.
        if let Some(entry) = self
            .subagents
            .iter_mut()
            .find(|entry| entry.id == session_id)
        {
            entry.runtime_status = Some(status.runtime_status().to_string());
        }
        true
    }

    /// Reconcile a bootstrap/reconnect status snapshot without allowing an
    /// older snapshot to undo a terminal lifecycle event that arrived live. A genuinely
    /// restarted child emits a fresh live active event, which uses
    /// `set_branch_activity_status` and intentionally clears the latch.
    fn reconcile_snapshot_branch_activity_status(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
    ) {
        let session_id = session_id.into();
        if matches!(
            status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) && self.branch_terminal_locked(&session_id)
        {
            // `self.subagents` was rebuilt from this snapshot before status
            // reconciliation. Restore the live terminal value onto the row as
            // well as retaining it in BranchActivity; otherwise rendering
            // still sees the stale `running` snapshot even though the active
            // count correctly says the child is finished.
            if let Some(locked_status) = self
                .branch_activities
                .get(&session_id)
                .map(|activity| activity.status)
            {
                if let Some(entry) = self
                    .subagents
                    .iter_mut()
                    .find(|entry| entry.id == session_id)
                {
                    entry.runtime_status =
                        Some(locked_status.runtime_status().to_string());
                }
            }
            return;
        }
        self.set_branch_activity_status(session_id, status);
    }

    pub fn reconcile_branch_lifecycle_snapshot(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
    ) -> BranchStatus {
        let session_id = session_id.into();
        self.reconcile_snapshot_branch_activity_status(session_id.clone(), status);
        self.branch_activities
            .get(&session_id)
            .map(|activity| activity.status)
            .unwrap_or(status)
    }

    pub fn reset_branch_lifecycle(&mut self) {
        self.branch_activities.clear();
        for entry in self.subagents.iter_mut().skip(1) {
            entry.runtime_status = None;
        }
    }

    pub fn retain_authoritative_branches(
        &mut self,
        branch_ids: &std::collections::HashSet<String>,
    ) {
        let retained = self.retained_viewed_subagent_id.as_deref();
        self.branch_activities.retain(|session_id, _| {
            branch_ids.contains(session_id) || retained == Some(session_id.as_str())
        });
        let root_id = self.subagents.first().map(|entry| entry.id.clone());
        self.subagents.retain(|entry| {
            root_id.as_deref() == Some(entry.id.as_str())
                || branch_ids.contains(&entry.id)
                || retained == Some(entry.id.as_str())
        });
    }

    pub fn set_branch_activity_started_at(
        &mut self,
        session_id: impl Into<String>,
        started_at: Option<u64>,
    ) {
        self.branch_activities
            .entry(session_id.into())
            .and_modify(|activity| activity.started_at = started_at)
            .or_insert_with(|| {
                BranchActivity::new(BranchStatus::Active, None, started_at)
            });
    }

    pub fn set_branch_activity_tool(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) {
        self.branch_activities
            .entry(session_id.into())
            .and_modify(|activity| {
                let was_terminal = activity.transition_status(status);
                if was_terminal {
                    return;
                }
                match current_tool.as_ref().map(|tool| tool.trim()) {
                    Some(tool) if !tool.is_empty() => {
                        activity.current_tool = Some(tool.to_string());
                    }
                    Some(_) => {
                        activity.current_tool = None;
                    }
                    None => {}
                }
                if started_at.is_some() {
                    activity.started_at = started_at;
                }
            })
            .or_insert_with(|| BranchActivity::new(status, current_tool, started_at));
    }

    /// Apply *part-level* activity (a raw text/reasoning/tool delta
    /// from the child) to a branch. Unlike an authoritative lifecycle
    /// status, this is noisy: the child keeps emitting "active"
    /// part-updates even as it winds down, and a straggler can arrive
    /// after the run has already finished. Once a branch is
    /// `terminal_locked` (it finished via an authoritative signal),
    /// these updates are dropped entirely so a finished sub-agent
    /// never resurrects to "responding"/"thinking". Returns `true`
    /// when the update was applied (so the caller can mirror the
    /// `active_subagent_ids` set only for genuinely-live branches).
    pub fn note_subagent_part_activity(
        &mut self,
        session_id: &str,
        status: BranchStatus,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) -> bool {
        if let Some(activity) = self.branch_activities.get(session_id) {
            if activity.terminal_locked {
                // The branch already finished for real — ignore the
                // late part delta that still claims it's working.
                return false;
            }
        }
        self.set_branch_activity_tool(
            session_id.to_string(),
            status,
            current_tool,
            started_at,
        );
        true
    }

    /// Whether `session_id` has latched an authoritative terminal
    /// state. Callers use this to keep their own "live" bookkeeping
    /// (`active_subagent_ids`) from re-adding a finished branch on a
    /// straggler part delta.
    pub fn branch_terminal_locked(&self, session_id: &str) -> bool {
        self.branch_activities
            .get(session_id)
            .is_some_and(|activity| activity.terminal_locked)
    }

    pub fn active_child_count(&self, current_session_id: Option<&str>) -> usize {
        self.active_child_ids(current_session_id).len()
    }

    pub fn active_child_ids(&self, current_session_id: Option<&str>) -> BTreeSet<String> {
        self.branch_activities
            .iter()
            .filter(|(session_id, activity)| {
                Some(session_id.as_str()) != current_session_id
                    && self
                        .subagents
                        .iter()
                        .any(|entry| entry.id.as_str() == session_id.as_str())
                    && matches!(
                        activity.status,
                        BranchStatus::Active | BranchStatus::WaitingPermission
                    )
            })
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }

    pub fn active_child_started_at(
        &self,
        current_session_id: Option<&str>,
    ) -> Option<u64> {
        self.branch_activities
            .iter()
            .filter(|(session_id, activity)| {
                Some(session_id.as_str()) != current_session_id
                    && self
                        .subagents
                        .iter()
                        .any(|entry| entry.id.as_str() == session_id.as_str())
                    && matches!(
                        activity.status,
                        BranchStatus::Active | BranchStatus::WaitingPermission
                    )
            })
            .filter_map(|(_, activity)| activity.started_at)
            .min()
    }

    /// Effective per-row height. Chat mode doubles up so each branch
    /// has space for its indented activity sub-row (connector + tool +
    /// status dot).
    pub fn row_height(&self) -> f32 {
        match self.mode {
            SidePanelMode::Sessions => ROW_HEIGHT,
            SidePanelMode::Subagents => ROW_HEIGHT * 2.0,
        }
    }

    /// Number of rows that fit in `panel_height` logical pixels.
    /// Kept available for the keyboard-nav path queued behind the
    /// initial wire-up.
    #[allow(dead_code)]
    pub fn rows_per_panel(&self, panel_height: f32) -> usize {
        let row_h = self.row_height();
        if row_h <= 0.0 {
            return 0;
        }
        (panel_height / row_h).floor().max(0.0) as usize
    }

    fn active_len(&self) -> usize {
        self.active_rows().len()
    }

    /// Next selectable (non-header) row in `active_rows` from `from`,
    /// scanning forward or backward. `None` when there is none in that
    /// direction — the caller then keeps its current selection.
    fn step_selectable(&self, from: usize, forward: bool) -> Option<usize> {
        let rows = self.active_rows();
        if forward {
            ((from + 1)..rows.len()).find(|&i| !rows[i].is_header)
        } else {
            (0..from).rev().find(|&i| !rows[i].is_header)
        }
    }

    /// Nearest selectable (non-header) row at or around `index`.
    fn nearest_selectable(&self, index: usize) -> Option<usize> {
        let rows = self.active_rows();
        if rows.is_empty() {
            return None;
        }
        let index = index.min(rows.len() - 1);
        if !rows[index].is_header {
            return Some(index);
        }
        self.step_selectable(index, true)
            .or_else(|| self.step_selectable(index, false))
    }

    /// Move selection off a header row (used after the display list is
    /// rebuilt so the cursor never lands on a group caption).
    fn snap_selection_to_selectable(&mut self) {
        if let Some(index) = self.nearest_selectable(self.selected) {
            self.selected = index;
        }
    }

    /// Whether the selection cursor is currently on the home-mode search
    /// row (only meaningful in [`SidePanelMode::Sessions`]).
    pub fn search_focused(&self) -> bool {
        self.search_focused && matches!(self.mode, SidePanelMode::Sessions)
    }

    /// Move the selection cursor onto the search row.
    pub fn focus_search(&mut self) {
        self.search_focused = true;
        self.back_focused = false;
        self.cursor_spring.reset();
    }

    fn clear_search_focus(&mut self) {
        self.search_focused = false;
    }

    pub fn select_next(&mut self) {
        // Leaving the Back affordance drops onto the element directly below
        // it: the search row in home mode, the first branch in chat mode.
        if self.back_focused() {
            self.clear_back_focus();
            if matches!(self.mode, SidePanelMode::Sessions) {
                self.focus_search();
            } else if let Some(first) = self.nearest_selectable(0) {
                self.selected = first;
                self.scroll_top = 0;
                self.scroll_px = 0.0;
                self.scroll.set_target(0.0);
                self.cursor_spring.reset();
            }
            return;
        }
        let len = self.active_len();
        if len == 0 {
            return;
        }
        // Leaving the search row lands on the first session.
        if self.search_focused() {
            self.clear_search_focus();
            if let Some(first) = self.nearest_selectable(0) {
                self.selected = first;
                self.scroll_top = 0;
                self.scroll_px = 0.0;
                self.scroll.set_target(0.0);
            }
            return;
        }
        if let Some(next) = self.step_selectable(self.selected, true) {
            self.move_selection_to(next);
        }
    }

    pub fn select_prev(&mut self) {
        if self.back_focused() {
            return; // already at the top of the panel
        }
        // Arrow-up from the search row lands on the Back affordance above it.
        if self.search_focused() {
            if self.back_button_rect.is_some() {
                self.focus_back();
            }
            return;
        }
        if self.active_len() == 0 {
            // No selectable rows, but a Back affordance may still be reachable
            // (e.g. a chat with no sub-agent branches).
            if self.back_button_rect.is_some() {
                self.focus_back();
            }
            return;
        }
        // Arrow-up past the first row lands on the search row (home mode) or
        // the Back affordance (chat mode).
        match self.step_selectable(self.selected, false) {
            Some(prev) => self.move_selection_to(prev),
            None if matches!(self.mode, SidePanelMode::Sessions) => {
                self.focus_search();
            }
            None => {
                if self.back_button_rect.is_some() {
                    self.focus_back();
                }
            }
        }
    }

    pub fn set_selected(&mut self, row: usize) {
        let len = self.active_len();
        if len == 0 {
            return;
        }
        // A click selects a real row, so it also leaves the search field and
        // the Back affordance.
        self.search_focused = false;
        self.back_focused = false;
        // A click may land on a header row; snap to the nearest session.
        let row = self.nearest_selectable(row.min(len - 1)).unwrap_or(0);
        self.move_selection_to(row);
    }

    fn move_selection_to(&mut self, new_selected: usize) {
        let len = self.active_len();
        if len == 0 {
            return;
        }
        let new_selected = new_selected.min(len - 1);
        if new_selected == self.selected {
            return;
        }
        let was_idle = self.cursor_spring.position == 0.0;
        let rows = self.selected as i32 - new_selected as i32;
        self.cursor_spring.position += rows as f32 * self.row_height();
        if was_idle {
            self.last_cursor_frame = Instant::now();
        }
        self.selected = new_selected;
        self.clamp_scroll(self.last_panel_height_rows);
    }

    fn scrolloff_for(panel_height_rows: usize) -> usize {
        if panel_height_rows <= 2 {
            return 0;
        }
        SCROLL_OFF_ROWS.min(panel_height_rows.saturating_sub(1) / 2)
    }

    pub fn clamp_scroll(&mut self, panel_height_rows: usize) {
        if self.active_len() == 0 {
            self.scroll_top = 0;
            return;
        }
        if panel_height_rows == 0 {
            return;
        }
        let scrolloff = Self::scrolloff_for(panel_height_rows);
        if self.selected < self.scroll_top.saturating_add(scrolloff) {
            self.set_scroll_top(self.selected.saturating_sub(scrolloff));
        } else if self.selected.saturating_add(scrolloff)
            >= self.scroll_top.saturating_add(panel_height_rows)
        {
            self.set_scroll_top(self.selected + scrolloff + 1 - panel_height_rows);
        }
        let max_top = self.max_scroll_top(panel_height_rows);
        if self.scroll_top > max_top {
            self.set_scroll_top(max_top);
        }
    }

    pub fn clamp_scroll_bounds(&mut self, panel_height_rows: usize) {
        if self.active_len() == 0 {
            self.scroll_top = 0;
            return;
        }
        let max_top = self.max_scroll_top(panel_height_rows);
        if self.scroll_top > max_top {
            self.set_scroll_top(max_top);
        }
    }

    pub fn max_scroll_top(&self, panel_height_rows: usize) -> usize {
        let visible = panel_height_rows.max(1);
        self.active_len().saturating_sub(visible)
    }

    /// Set the committed top row and drive the pixel spring toward its
    /// row boundary (used by arrow-key navigation / clamping — rows align
    /// flush, so no sub-row remainder).
    fn set_scroll_top(&mut self, new_top: usize) {
        self.scroll_top = new_top;
        self.scroll_px = new_top as f32 * self.row_height();
        self.scroll.set_target(self.scroll_px);
    }

    /// Maximum continuous pixel scroll for the home list at the given
    /// viewport height (rows), leaving the last row flush at the bottom.
    fn max_scroll_px(&self, panel_height_rows: usize) -> f32 {
        let row_h = self.row_height();
        let content = self.active_len() as f32 * row_h;
        let viewport = panel_height_rows.max(1) as f32 * row_h;
        (content - viewport).max(0.0)
    }

    pub fn scroll_by(&mut self, delta: i32, panel_height_rows: usize) {
        let max_top = self.max_scroll_top(panel_height_rows);
        let new_top = if delta < 0 {
            self.scroll_top
                .saturating_sub(delta.unsigned_abs() as usize)
        } else {
            self.scroll_top.saturating_add(delta as usize).min(max_top)
        };
        if new_top != self.scroll_top {
            self.set_scroll_top(new_top);
        }
    }

    pub fn scroll_pixels(&mut self, delta_pixels: f32, panel_height_rows: usize) {
        if delta_pixels == 0.0 {
            return;
        }
        // Chat mode scrolls the *whole* content column as one pixel
        // viewport so every section (goal, branches, tasks) is
        // reachable — not just the branch sub-list. A positive
        // `delta_pixels` is a scroll-up gesture, which should reveal
        // earlier (top) content, i.e. reduce the offset.
        if matches!(self.mode, SidePanelMode::Subagents) {
            self.scroll_content_pixels(-delta_pixels);
            return;
        }
        let row_h = self.row_height();
        if row_h <= 0.0 {
            return;
        }
        // Pixel-precise home scroll: move the continuous position by the
        // exact gesture delta (positive = scroll toward the top) and let
        // the short spring animate to it. No whole-row quantization, so
        // trackpad / wheel track the finger tightly. `scroll_top` is the
        // derived integer top row for the render window + selection math.
        let max_px = self.max_scroll_px(panel_height_rows);
        let next = (self.scroll_px - delta_pixels).clamp(0.0, max_px);
        if next == self.scroll_px {
            return;
        }
        self.scroll_px = next;
        self.scroll.set_target(next);
        self.scroll_top = (next / row_h).floor() as usize;
    }

    /// Advance the home-list scroll spring and return the *absolute*
    /// animated scroll position in **rows** (0 = top). The renderer
    /// multiplies by its own (chrome-scaled) row height and derives the
    /// top row + sub-row offset from it, so scrolling is pixel-smooth and
    /// scale-independent.
    pub fn tick_scroll(&mut self) -> f32 {
        if matches!(self.mode, SidePanelMode::Subagents) {
            self.scroll.reset();
            self.last_scroll_frame = Instant::now();
            return 0.0;
        }
        let row_h = self.row_height().max(1.0);
        if !self.scroll.is_animating() {
            self.last_scroll_frame = Instant::now();
            return self.scroll.current().max(0.0) / row_h;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_scroll_frame = now;
        self.scroll.tick(dt);
        self.scroll.current().max(0.0) / row_h
    }

    pub fn tick_cursor(&mut self) -> f32 {
        if self.cursor_spring.position == 0.0 {
            self.last_cursor_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_cursor_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_cursor_frame = now;
        self.cursor_spring.update(dt, CURSOR_ANIMATION_LENGTH);
        self.cursor_spring.position
    }

    pub fn is_animating(&self) -> bool {
        let sessions_loading = matches!(self.mode, SidePanelMode::Sessions)
            && !self.user_hidden
            && self.sessions_loading_elapsed() < 1.5
            && (matches!(self.session_catalog_state, SessionCatalogState::Loading)
                || self.session_page_loading);
        self.scroll.is_animating()
                || self.cursor_spring.position != 0.0
                || sessions_loading
                || (self.semantic_searching && self.semantic_search_elapsed() < 1.5)
                // A running sub-agent paints the rainbow loader spinner (and
                // the blinking status dot), both of which need the host to
                // keep redrawing — otherwise the spinner freezes on whatever
                // frame the last event happened to land on.
                || self.has_active_subagents()
                || (self.session_hover_target && self.session_hover_scale < 0.998)
                || (!self.session_hover_target && self.session_hover_scale > 0.002)
                || self.back_scramble_elapsed_ms().is_some()
                || self.usage_scramble_elapsed_ms().is_some()
    }

    /// Map a window-space click to a row index. Returns `None` when
    /// outside the panel content area or past the last visible row.
    pub fn hit_test_row(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        _panel_rect: [f32; 4],
    ) -> Option<usize> {
        let [content_x, content_y, content_w, content_h] = self.last_row_hit_rect?;
        if content_w <= 0.0 || content_h <= 0.0 {
            return None;
        }
        let row_h = self.last_row_hit_height;
        if mouse_x < content_x || mouse_x > content_x + content_w {
            return None;
        }
        // Bounds-check against the visible (clamped) rect, but anchor
        // the row math to the full list's origin so a scrolled list
        // still maps clicks to the right index.
        if mouse_y < content_y || mouse_y > content_y + content_h {
            return None;
        }
        // Home mode scrolls by continuous rows: convert the spring's
        // pixel position to rows and add the visual row under the cursor.
        // Chat (subagent) mode keeps the row-index + lag-offset anchor.
        let row = if matches!(self.mode, SidePanelMode::Sessions) {
            let scroll_rows = self.scroll.current().max(0.0) / self.row_height();
            ((mouse_y - self.last_row_origin_y) / row_h + scroll_rows).floor() as isize
        } else {
            let local_y = mouse_y - self.last_row_origin_y - self.scroll.current();
            (local_y / row_h).floor() as isize + self.scroll_top as isize
        };
        if row < 0 {
            return None;
        }
        let row = row as usize;
        if row >= self.active_len() {
            return None;
        }
        Some(row)
    }
}
