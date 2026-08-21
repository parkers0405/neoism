//! Multi-session background cache — the shared port of the desktop
//! pane's `CachedAgentSession` machinery
//! (`desktop/src/neoism/agent/pane.rs` + `pane/ingest.rs` +
//! `agent/commands.rs`). Switching away from a session PARKS its full
//! conversation state under its id; events that arrive for a
//! non-active session stream into that session's cache instead of
//! being dropped; switching back restores instantly (no refetch) and
//! the background history refresh reconciles through the same
//! snapshot-merge pipeline desktop runs.

use super::*;

/// Hard cap on parked sessions — mirrors desktop's
/// `trim_session_cache` bound exactly. The wasm pane is long-lived in
/// one instance, so this is the memory ceiling for background
/// conversations.
const MAX_CACHED_SESSIONS: usize = 40;

/// Per-session runtime UI state (streaming indicator, queue chip,
/// subagent clocks) parked alongside the transcript. Mirrors desktop's
/// `CachedAgentRuntime`.
pub(in crate::panels::agent_pane::state) struct CachedAgentRuntime {
    pub queued_prompt_count: usize,
    pub queued_prompt_preview: Option<String>,
    pub streaming_state: NeoismAgentStreamingState,
    pub streaming_started_at: Option<Instant>,
    pub streaming_state_changed_at: Option<Instant>,
    pub streaming_tool_label: Option<String>,
    pub subagent_waiting_started_at: Option<Instant>,
    pub background_tasks_started_at: Option<Instant>,
    pub running_background_task_count: usize,
    pub active_subagent_ids: BTreeSet<String>,
    pub active_subagent_started_at: HashMap<String, u64>,
    pub abort_requested_at: Option<Instant>,
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
            active_subagent_ids: BTreeSet::new(),
            active_subagent_started_at: HashMap::new(),
            abort_requested_at: None,
        }
    }
}

impl CachedAgentRuntime {
    pub(in crate::panels::agent_pane::state) fn is_streaming(&self) -> bool {
        self.streaming_state != NeoismAgentStreamingState::Idle
            && self.streaming_started_at.is_some()
    }

    pub(in crate::panels::agent_pane::state) fn note_streaming(
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

    pub(in crate::panels::agent_pane::state) fn refresh_streaming_from_tail(
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

    pub(in crate::panels::agent_pane::state) fn apply_queue_status(
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
}

/// One parked conversation, keyed by session id. Field-for-field
/// mirror of desktop's `CachedAgentSession` (minus the desktop-only
/// preload bookkeeping).
pub(in crate::panels::agent_pane::state) struct CachedAgentSession {
    pub state: crate::panels::agent_pane::api_mapping::SessionState,
    pub messages: Vec<NeoismAgentMessage>,
    pub pending_user_prompts: Vec<String>,
    pub prompt_echo_aliases: Vec<(String, String)>,
    pub timeline_history: AgentTimelineHistoryState,
    pub timeline_scroll_px: f32,
    pub timeline_follow_bottom: bool,
    pub timeline_content_height_px: f32,
    pub timeline_layout_epoch: u64,
    pub timeline_layout_cache: Option<TimelineLayoutCache>,
    pub timeline_dirty_message_ids: BTreeSet<String>,
    pub timeline_dirty_message_indices: BTreeSet<usize>,
    pub runtime: CachedAgentRuntime,
    pub model_context_limit: Option<u64>,
    /// A full history snapshot has landed for this entry — switching to
    /// it can restore instantly. `false` = live-only (created by
    /// background events); switching to it clears + fetches like a
    /// cold open, seeded with whatever streamed in.
    pub hydrated: bool,
    pub last_access: Instant,
}

impl CachedAgentSession {
    pub(in crate::panels::agent_pane::state) fn live_only() -> Self {
        Self {
            state: crate::panels::agent_pane::api_mapping::SessionState::default(),
            messages: Vec::new(),
            pending_user_prompts: Vec::new(),
            prompt_echo_aliases: Vec::new(),
            timeline_history: AgentTimelineHistoryState::default(),
            timeline_scroll_px: 0.0,
            timeline_follow_bottom: true,
            timeline_content_height_px: 0.0,
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

    pub(in crate::panels::agent_pane::state) fn invalidate_timeline_layout(&mut self) {
        self.last_access = Instant::now();
        self.timeline_layout_epoch = self.timeline_layout_epoch.wrapping_add(1);
        self.timeline_layout_cache = None;
        self.timeline_dirty_message_ids.clear();
        self.timeline_dirty_message_indices.clear();
    }
}

/// Merge a stored transcript snapshot with parts that arrived live
/// while the snapshot request was in flight. Live text wins when the
/// stored part is empty or an older prefix. Desktop:
/// `pane.rs::merge_session_snapshot`.
pub(in crate::panels::agent_pane::state) fn merge_session_snapshot(
    snapshot: Vec<NeoismAgentMessage>,
    live: Vec<NeoismAgentMessage>,
) -> Vec<NeoismAgentMessage> {
    use std::collections::HashSet;

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

/// Cache-keyed variant of the active pipeline's echo/pending
/// reconciliation: canonicalize expanded prompt echoes back to the
/// composer form, and retire optimistic user echoes the snapshot has
/// caught up to (dropping their live duplicates). Desktop:
/// `pane.rs::reconcile_cached_pending_user_prompts`.
pub(in crate::panels::agent_pane::state) fn reconcile_cached_pending_user_prompts(
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
    pending.retain(|prompt| {
        let resolved = snapshot.iter().any(|message| {
            message.kind == NeoismAgentMessageKind::User
                && message.text.trim() == prompt.trim()
        });
        if resolved {
            live.retain(|message| {
                !(message.kind == NeoismAgentMessageKind::User
                    && message.id.is_empty()
                    && message.text.trim() == prompt.trim())
            });
        }
        !resolved
    });
}

fn session_message_identity(message: &NeoismAgentMessage) -> Option<String> {
    if !message.id.is_empty() {
        return Some(format!("id:{}", message.id));
    }
    task_id_from_task_message(message).map(|task_id| format!("task:{task_id}"))
}

/// Upsert one streamed part into a cached (non-active) transcript.
/// Desktop: `pane.rs::upsert_cached_part_message`.
pub(in crate::panels::agent_pane::state) fn upsert_cached_part_message(
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

/// Append one streamed text delta into a cached (non-active)
/// transcript. Desktop: `pane.rs::apply_cached_part_delta`.
pub(in crate::panels::agent_pane::state) fn apply_cached_part_delta(
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
    let message_kind = part_delta_message_kind(kind);
    if let Some(index) = messages
        .iter()
        .rposition(|message| message.kind == message_kind)
    {
        messages[index].text.push_str(delta);
        return;
    }
    messages.push(match message_kind {
        NeoismAgentMessageKind::Reasoning => NeoismAgentMessage::reasoning(delta),
        _ => NeoismAgentMessage::assistant(delta),
    });
}

impl NeoismAgentPane {
    // -----------------------------------------------------------------
    // Park / restore (desktop `commands.rs::cache_current_session` /
    // `activate_cached_session`).
    // -----------------------------------------------------------------

    /// Park the active conversation's full state under its session id.
    /// No-op without an active session.
    pub(in crate::panels::agent_pane::state) fn cache_current_session(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        let state = crate::panels::agent_pane::api_mapping::SessionState {
            agent: self.agent.clone(),
            model: (!self.model.is_empty()).then(|| self.model.clone()),
            thinking: self.thinking.clone(),
            parent_id: self.parent_session_id.clone(),
            directory: self.directory.clone(),
        };
        // Merge with whatever streamed into this session's cache slot
        // while a stale entry lingered (defensive — normally empty).
        let cached_live = self
            .session_cache
            .remove(&session_id)
            .map(|cached| cached.messages)
            .unwrap_or_default();
        let messages = std::mem::take(&mut self.messages);
        let messages = if cached_live.is_empty() {
            messages
        } else {
            merge_session_snapshot(messages, cached_live)
        };
        let timeline_history = std::mem::take(&mut self.timeline_history);
        let timeline_layout_cache = self.timeline_layout_cache.replace(None);
        let runtime = self.take_session_runtime_ui();
        self.session_cache.insert(
            session_id,
            CachedAgentSession {
                state,
                messages,
                pending_user_prompts: std::mem::take(&mut self.pending_user_prompts),
                prompt_echo_aliases: std::mem::take(&mut self.prompt_echo_aliases),
                timeline_history,
                timeline_scroll_px: self.timeline_scroll_px,
                timeline_follow_bottom: self.timeline_follow_bottom,
                timeline_content_height_px: self.timeline_content_height_px,
                timeline_layout_epoch: self.timeline_layout_epoch,
                timeline_layout_cache,
                timeline_dirty_message_ids: std::mem::take(
                    &mut self.timeline_dirty_message_ids,
                ),
                timeline_dirty_message_indices: std::mem::take(
                    &mut self.timeline_dirty_message_indices,
                ),
                runtime,
                model_context_limit: self.model_context_limit,
                hydrated: true,
                last_access: Instant::now(),
            },
        );
        self.trim_session_cache();
    }

    /// Instant switch to a parked session: park the current one, then
    /// restore `session_id`'s transcript, scroll, pending prompts and
    /// runtime UI without waiting for a refetch. Returns `false` (and
    /// mutates nothing) when the target has no hydrated cache entry.
    pub(in crate::panels::agent_pane::state) fn activate_cached_session(
        &mut self,
        session_id: &str,
    ) -> bool {
        if self.session_id.as_deref() == Some(session_id) {
            return true;
        }
        if !self
            .session_cache
            .get(session_id)
            .is_some_and(|cached| cached.hydrated)
        {
            return false;
        }
        let Some(cached) = self.session_cache.remove(session_id) else {
            return false;
        };
        // Decide BEFORE the ids change hands: a switch within the same
        // conversation family (parent ↔ child ↔ sibling) keeps the
        // parent-keyed subagent roster alive — the sidebar continues to
        // show the family's sub-agent names/statuses while a child
        // transcript is open, and sibling lifecycle updates keep
        // landing in it. Only leaving the family rebuilds the roster.
        let stays_in_family = self.session_family_contains(session_id)
            || cached
                .state
                .parent_id
                .as_deref()
                .is_some_and(|parent| self.session_family_contains(parent));
        // A live-only cache slot carries no session metadata. When the
        // roster tracks the target as a child row, carry the family root
        // as its parent so the restored view opens as a view-only
        // subagent transcript instead of a detached main session.
        let roster_parent = self
            .side_panel
            .subagents()
            .first()
            .map(|entry| entry.id.clone())
            .filter(|root| {
                root != session_id
                    && self
                        .side_panel
                        .subagents()
                        .iter()
                        .skip(1)
                        .any(|entry| entry.id == session_id)
            });
        self.cache_current_session();
        let state = cached.state;
        self.session_id = Some(session_id.to_string());
        self.parent_session_id = state.parent_id.clone().or(roster_parent);
        self.side_panel
            .set_viewed_session_id(Some(session_id.to_string()));
        self.input.clear();
        self.close_picker();
        self.reset_timeline_navigation_for_session_switch();
        // The restored timeline must render fresh — no raised/expanded
        // card artifacts from the click that navigated away. Leave-and-return
        // also collapses the live-trace window, so a parked layout that still
        // contains tool rows would paint leftover titles until the next click.
        self.reset_transient_timeline_interactions();
        self.timeline_history = cached.timeline_history;
        self.timeline_scroll_px = cached.timeline_scroll_px;
        self.timeline_follow_bottom = cached.timeline_follow_bottom;
        self.timeline_content_height_px = cached.timeline_content_height_px;
        self.side_panel.set_show_home_override(false);
        if !stays_in_family {
            self.side_panel.invalidate_subagent_refresh();
        }
        self.side_panel.reset_session_goal();
        self.side_panel.invalidate_goal_refresh();
        if let Some(directory) = state.directory {
            self.set_directory(Some(directory));
        }
        if let Some(agent) = state.agent {
            self.agent = Some(agent);
        }
        if let Some(model) = state.model {
            self.model = model;
        }
        self.thinking = state.thinking;
        self.model_context_limit = cached.model_context_limit;
        if self.model_context_limit.is_none() && !self.model.is_empty() {
            self.refresh_model_context_limit();
        }
        if self.is_subagent_session() {
            self.clear_composer();
            self.set_cursor_rect(None);
            self.close_picker();
        }
        self.messages = cached.messages;
        self.pending_user_prompts = cached.pending_user_prompts;
        self.prompt_echo_aliases = cached.prompt_echo_aliases;
        self.restore_session_runtime_ui(cached.runtime);
        // Live-trace was cleared by the switch. Drop the parked layout so
        // settled tool/reasoning rows are re-masked instead of flashing as
        // leftover titles from the previous visit.
        self.invalidate_timeline_layout();
        // The virtual timeline rebuilds off the content revision, which
        // is pane-global and monotonic (never parked/restored) — bump
        // it so the restored transcript replaces the parked one on the
        // next sync even if the restored layout epoch coincides.
        self.timeline_content_revision = self.timeline_content_revision.wrapping_add(1);
        self.sync_subagent_waiting_clock();
        true
    }

    /// Take the cached live-only remnants of `session_id` (messages,
    /// pending prompts, echo aliases, runtime) so a cold switch can
    /// seed itself with whatever streamed in the background. Returns
    /// `None` when nothing is cached.
    pub(in crate::panels::agent_pane::state) fn take_live_only_cache(
        &mut self,
        session_id: &str,
    ) -> Option<CachedAgentSession> {
        self.session_cache.remove(session_id)
    }

    fn cached_session_entry(&mut self, session_id: &str) -> &mut CachedAgentSession {
        self.session_cache
            .entry(session_id.to_string())
            .or_insert_with(CachedAgentSession::live_only)
    }

    /// LRU eviction, desktop bounds verbatim: cap 40 entries; the
    /// active session, its parent and live subagents are pinned.
    pub(in crate::panels::agent_pane::state) fn trim_session_cache(&mut self) {
        if self.session_cache.len() <= MAX_CACHED_SESSIONS {
            return;
        }
        let mut pinned = self.active_subagent_ids.clone();
        pinned.extend(self.session_id.iter().cloned());
        pinned.extend(self.parent_session_id.iter().cloned());
        while self.session_cache.len() > MAX_CACHED_SESSIONS {
            let Some(candidate) = self
                .session_cache
                .iter()
                .filter(|(session_id, _)| !pinned.contains(*session_id))
                .min_by_key(|(_, cached)| cached.last_access)
                .map(|(session_id, _)| session_id.clone())
            else {
                break;
            };
            self.session_cache.remove(&candidate);
        }
    }

    pub(in crate::panels::agent_pane::state) fn take_session_runtime_ui(
        &mut self,
    ) -> CachedAgentRuntime {
        CachedAgentRuntime {
            queued_prompt_count: std::mem::take(&mut self.queued_prompt_count),
            queued_prompt_preview: self.queued_prompt_preview.take(),
            streaming_state: std::mem::replace(
                &mut self.streaming_state,
                NeoismAgentStreamingState::Idle,
            ),
            streaming_started_at: self.streaming_started_at.take(),
            streaming_state_changed_at: self.streaming_state_changed_at.take(),
            streaming_tool_label: self.streaming_tool_label.take(),
            subagent_waiting_started_at: self.subagent_waiting_started_at.take(),
            background_tasks_started_at: self.background_tasks_started_at.take(),
            running_background_task_count: std::mem::take(
                &mut self.running_background_task_count,
            ),
            active_subagent_ids: std::mem::take(&mut self.active_subagent_ids),
            active_subagent_started_at: std::mem::take(
                &mut self.active_subagent_started_at,
            ),
            abort_requested_at: self.abort_requested_at.take(),
        }
    }

    pub(in crate::panels::agent_pane::state) fn restore_session_runtime_ui(
        &mut self,
        runtime: CachedAgentRuntime,
    ) {
        self.queued_prompt_count = runtime.queued_prompt_count;
        self.queued_prompt_preview = runtime.queued_prompt_preview;
        self.streaming_state = runtime.streaming_state;
        self.streaming_started_at = runtime.streaming_started_at;
        self.streaming_state_changed_at = runtime.streaming_state_changed_at;
        self.streaming_tool_label = runtime.streaming_tool_label;
        self.subagent_waiting_started_at = runtime.subagent_waiting_started_at;
        self.background_tasks_started_at = runtime.background_tasks_started_at;
        self.running_background_task_count = runtime.running_background_task_count;
        self.active_subagent_ids = runtime.active_subagent_ids;
        self.active_subagent_started_at = runtime.active_subagent_started_at;
        self.abort_requested_at = runtime.abort_requested_at;
        self.permission_choice_hit_rects.clear();
        self.question_option_hit_rects.clear();
    }

    /// Reset scroll physics / anchors / selection so the restored (or
    /// fresh) session doesn't inherit the previous one's in-flight
    /// gesture. Desktop:
    /// `ingest.rs::reset_timeline_navigation_for_session_switch`.
    pub(in crate::panels::agent_pane::state) fn reset_timeline_navigation_for_session_switch(
        &mut self,
    ) {
        self.timeline_velocity_px_s = 0.0;
        self.timeline_last_tick_at = None;
        self.timeline_wheel_target_px = None;
        self.timeline_last_scroll_at = None;
        self.pending_timeline_anchor = None;
        self.timeline_view_anchor = None;
        self.pending_timeline_prepend_height_px = None;
        self.pending_timeline_prepend_delta_px = None;
        self.scrollbar_drag = None;
        self.selection_anchor = None;
        self.selection_focus = None;
        self.timeline_live_trace_start = None;
        self.timeline_live_trace_anchor = None;
    }

    // -----------------------------------------------------------------
    // Background ingestion — events for NON-active sessions land here
    // (the wasm bridge routes them through `apply_agent_event_to_cache`
    // instead of dropping them). Each mirrors the `!stream_is_active`
    // arm of desktop's `drain_server_updates`.
    // -----------------------------------------------------------------

    /// A history snapshot arrived for a cached (non-active) session:
    /// reconcile optimistic prompts, merge with live-streamed parts and
    /// mark the entry hydrated. Desktop: the `Messages` /
    /// `ChildMessages` cached arms.
    pub fn apply_history_to_cache(
        &mut self,
        session_id: &str,
        mut messages: Vec<NeoismAgentMessage>,
        oldest_cursor: Option<String>,
    ) {
        let cached = self.cached_session_entry(session_id);
        let mut live = std::mem::take(&mut cached.messages);
        reconcile_cached_pending_user_prompts(
            &mut messages,
            &mut live,
            &mut cached.pending_user_prompts,
            &cached.prompt_echo_aliases,
        );
        cached.messages = merge_session_snapshot(messages, live);
        cached.timeline_history.oldest_loaded_cursor = oldest_cursor;
        cached.hydrated = true;
        cached.invalidate_timeline_layout();
        self.trim_session_cache();
    }

    /// Whether `session_id` has a hydrated cache entry (instant-switch
    /// eligible).
    pub fn cached_session_is_hydrated(&self, session_id: &str) -> bool {
        self.session_cache
            .get(session_id)
            .is_some_and(|cached| cached.hydrated)
    }

    /// Streamed whole-part upsert for a non-active session. Desktop:
    /// `PartUpdated` cached arm.
    pub fn cache_upsert_part_message(
        &mut self,
        session_id: &str,
        message: NeoismAgentMessage,
    ) {
        let cached = self.cached_session_entry(session_id);
        upsert_cached_part_message(&mut cached.messages, message);
        cached.runtime.refresh_streaming_from_tail(&cached.messages);
        cached.invalidate_timeline_layout();
        self.trim_session_cache();
    }

    /// Streamed text delta for a non-active session. Desktop:
    /// `PartDelta` cached arm.
    pub fn cache_apply_part_delta(
        &mut self,
        session_id: &str,
        part_id: Option<&str>,
        kind: Option<&str>,
        delta: &str,
    ) {
        if delta.is_empty() {
            return;
        }
        let cached = self.cached_session_entry(session_id);
        apply_cached_part_delta(&mut cached.messages, part_id, kind, delta);
        cached.runtime.refresh_streaming_from_tail(&cached.messages);
        cached.invalidate_timeline_layout();
        self.trim_session_cache();
    }

    /// Part removal for a non-active session. Desktop: `PartRemoved`
    /// cached arm. Never creates an entry.
    pub fn cache_remove_part_message(&mut self, session_id: &str, part_id: &str) {
        if part_id.is_empty() {
            return;
        }
        if let Some(cached) = self.session_cache.get_mut(session_id) {
            cached.messages.retain(|message| message.id != part_id);
            cached.invalidate_timeline_layout();
        }
    }

    /// Streaming-state edge for a non-active session (`MessageStart`,
    /// `StreamingState`, compaction started/ended). Desktop: the
    /// runtime `note_streaming` cached arms.
    pub fn cache_note_streaming(
        &mut self,
        session_id: &str,
        state: NeoismAgentStreamingState,
        tool: Option<String>,
    ) {
        let cached = self.cached_session_entry(session_id);
        cached.runtime.note_streaming(state, tool);
        cached.last_access = Instant::now();
        self.trim_session_cache();
    }

    /// Idle edge for a non-active session. Desktop: `SessionIdle`
    /// cached arm (never creates an entry).
    pub fn cache_note_session_idle(&mut self, session_id: &str) {
        if let Some(cached) = self.session_cache.get_mut(session_id) {
            cached
                .runtime
                .note_streaming(NeoismAgentStreamingState::Idle, None);
            cached.last_access = Instant::now();
        }
    }

    /// Queue-status update for a non-active session. Desktop:
    /// `QueueStatus` cached arm.
    pub fn cache_apply_queue(
        &mut self,
        session_id: &str,
        count: u32,
        preview: Option<String>,
        started_at: Option<u64>,
    ) {
        let cached = self.cached_session_entry(session_id);
        cached
            .runtime
            .apply_queue_status(count as usize, preview, started_at);
        cached.last_access = Instant::now();
        self.trim_session_cache();
    }

    /// Usage snapshot for a non-active session — stamped onto the
    /// latest assistant/reasoning message like the live `apply_usage`.
    pub fn cache_apply_usage(&mut self, session_id: &str, mut usage: NeoismAgentUsage) {
        let Some(cached) = self.session_cache.get_mut(session_id) else {
            return;
        };
        if usage.context_limit.is_none() {
            usage.context_limit = cached.model_context_limit;
        }
        let target = cached
            .messages
            .iter()
            .rposition(|m| {
                matches!(
                    m.kind,
                    NeoismAgentMessageKind::Assistant | NeoismAgentMessageKind::Reasoning
                )
            })
            .or_else(|| cached.messages.iter().rposition(|m| !m.id.is_empty()));
        if let Some(index) = target {
            cached.messages[index].usage = Some(usage);
            cached.invalidate_timeline_layout();
        }
    }

    /// Inline system row for a non-active session (session-scoped
    /// notices). Desktop: `PromptDispatchFailed` cached arm's shape.
    pub fn cache_push_system_message(
        &mut self,
        session_id: &str,
        title: impl Into<String>,
        text: impl Into<String>,
    ) {
        let cached = self.cached_session_entry(session_id);
        cached
            .messages
            .push(NeoismAgentMessage::system(title, text));
        cached.invalidate_timeline_layout();
        self.trim_session_cache();
    }
}
