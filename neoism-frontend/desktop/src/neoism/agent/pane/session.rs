use super::*;

impl NeoismAgentPane {
    pub fn pop_wordmark_click(&mut self, x: f32, y: f32) -> bool {
        let Some([rx, ry, rw, rh]) = self.wordmark.rect else {
            return false;
        };
        if x < rx || x > rx + rw || y < ry || y > ry + rh {
            return false;
        }
        self.wordmark.click_started = Some(Instant::now());
        self.wordmark.click_pos = Some((x, y));
        true
    }

    pub fn is_animating(&self) -> bool {
        self.animation_reason().is_some()
    }

    pub fn animation_reason(&self) -> Option<&'static str> {
        if self.wordmark_click_is_animating() {
            return Some("wordmark");
        }
        if self
            .picker
            .as_ref()
            .is_some_and(NeoismAgentPicker::is_animating)
        {
            return Some("picker");
        }
        if self.tool_expansion_is_animating() {
            return Some("tool_expansion");
        }
        if self.timeline_is_inertial() {
            return Some("timeline_inertia");
        }
        if self.is_streaming() {
            return Some("streaming");
        }
        if self.active_subagent_count() > 0 {
            return Some("subagents");
        }
        if self.running_background_task_count() > 0 {
            return Some("background_tasks");
        }
        if self.side_panel.is_animating() {
            return Some("side_panel");
        }
        None
    }

    pub(crate) fn wordmark_click_is_animating(&self) -> bool {
        self.wordmark.click_started.is_some_and(|started| {
            Instant::now().saturating_duration_since(started) <= WORDMARK_CLICK_ANIMATION
        })
    }

    pub fn side_panel(&self) -> &NeoismAgentSidePanel {
        &self.side_panel
    }

    pub fn side_panel_mut(&mut self) -> &mut NeoismAgentSidePanel {
        &mut self.side_panel
    }

    pub fn session_id_str(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn maybe_request_older_timeline_page(
        &mut self,
        scroll_top: f32,
        viewport_h: f32,
    ) {
        const LOAD_OLDER_LIMIT: usize = 128;
        // Minimum spacing between page requests. Without it, a page that adds
        // little height (collapsed tool groups keep you near the top) re-arms
        // the trigger every round-trip and drags the whole transcript in at
        // once — each fold/measure piling on, so it "gets more slow".
        const LOAD_OLDER_COOLDOWN: Duration = Duration::from_millis(180);
        let threshold = (viewport_h * 0.75).max(720.0);
        if scroll_top > threshold
            || !self.timeline_history.has_older
            || self.timeline_history.loading_older
        {
            return;
        }
        // Only paginate while the reader is actually moving toward the top
        // (manual scroll or inertial glide). Parked at the top we do nothing,
        // so reaching the top pulls exactly one page per scroll gesture.
        let now = Instant::now();
        let recently_scrolled = self
            .timeline_last_scroll_at
            .is_some_and(|at| now.duration_since(at) < Duration::from_millis(250))
            || self.timeline_is_inertial();
        if !recently_scrolled {
            return;
        }
        if self
            .timeline_last_older_request_at
            .is_some_and(|at| now.duration_since(at) < LOAD_OLDER_COOLDOWN)
        {
            return;
        }
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        if self.timeline_history.last_requested_session_id.as_deref()
            == Some(session_id.as_str())
        {
            return;
        }
        self.timeline_last_older_request_at = Some(now);
        self.timeline_history.loading_older = true;
        self.timeline_history.last_requested_session_id = Some(session_id.clone());
        self.push_outbound(OutboundAgentCommand::LoadOlderTimeline {
            session_id,
            before: self.timeline_history.oldest_loaded_cursor.clone(),
            limit: LOAD_OLDER_LIMIT,
        });
    }

    pub(crate) fn mark_timeline_prepend_pending_at_current_height(&mut self) {
        self.pending_timeline_prepend_height_px = Some(self.timeline_content_height_px);
    }

    /// Publish a human-readable title for this pane's agent session at
    /// the daemon level (right-click → Rename on an agent tab). No-ops
    /// when the pane has no live session yet. Queues an
    /// [`OutboundAgentCommand::SetTitle`] so the desktop runtime PATCHes
    /// `/session/{id}` (and the web bridge ships `SetTitle` over the
    /// daemon WS) on the next drain.
    pub fn publish_session_title(&mut self, title: impl Into<String>) -> bool {
        let Some(session_id) = self.session_id.clone() else {
            return false;
        };
        self.push_outbound(OutboundAgentCommand::SetTitle {
            session_id,
            title: title.into(),
        });
        true
    }

    /// Kick off (debounced) a background refresh of the previous-session
    /// list shown in the side panel's home mode. Mirrors the file_tree
    /// git-status worker pattern: never blocks the frame; the worker
    /// pushes its result through `background_tx` and the next frame's
    /// `drain_background_updates` lifts it into `side_panel`.
    pub fn maybe_refresh_side_panel_sessions(&mut self) {
        if !self.side_panel.should_refresh_sessions() {
            return;
        }
        self.side_panel.mark_refresh_kicked();
        let server = self.server.clone();
        let current = self.session_id.clone();
        let directory = self.directory.clone();
        let tx = self.background_tx.clone();
        std::thread::Builder::new()
            .name("neoism-agent-sessions".into())
            .spawn(move || {
                let entries = fetch_session_entries(
                    &server,
                    current.as_deref(),
                    directory.as_deref(),
                )
                .unwrap_or_default();
                let _ = tx.send(NeoismAgentBackgroundUpdate::SidePanelSessionsRefreshed(
                    entries,
                ));
            })
            .ok();
    }

    /// Kick a background semantic transcript search for `query`, coalesced
    /// to one fetch in flight (the newest query typed meanwhile replaces any
    /// waiting one). Results land as `SemanticSessionHits` in
    /// `drain_background_updates` and feed both the side-panel session list
    /// and an open `/sessions` picker. No-ops once the server has reported
    /// semantic search unavailable.
    pub(crate) fn kick_semantic_session_search(&mut self, query: String) {
        let query = query.trim().to_string();
        if query.is_empty() || self.semantic_unavailable {
            self.set_semantic_loading_indicators(false);
            return;
        }
        self.set_semantic_loading_indicators(true);
        if self.semantic_in_flight {
            self.semantic_pending_query = Some(query);
            return;
        }
        self.semantic_in_flight = true;
        let server = self.server.clone();
        let current = self.session_id.clone();
        let directory = self.directory.clone();
        let tx = self.background_tx.clone();
        std::thread::Builder::new()
            .name("neoism-agent-semantic".into())
            .spawn(move || {
                let hits = match crate::neoism::agent::api::fetch_semantic_session_hits(
                    &server,
                    &query,
                    current.as_deref(),
                    directory.as_deref(),
                ) {
                    Ok(hits) => hits,
                    // Network hiccups yield "no results" rather than
                    // latching the feature off.
                    Err(_) => Some(Vec::new()),
                };
                let _ = tx.send(NeoismAgentBackgroundUpdate::SemanticSessionHits {
                    query,
                    hits,
                });
            })
            .ok();
    }

    /// Toggle the searching state that drives the skeleton shimmer in the
    /// side panel and an open `/sessions` picker while semantic results are
    /// still in flight.
    pub(crate) fn set_semantic_loading_indicators(&mut self, loading: bool) {
        self.side_panel.set_semantic_searching(loading);
        if let Some(picker) = self.picker.as_mut().filter(|picker| {
            picker.kind == crate::neoism::agent::picker::NeoismAgentPickerKind::Session
        }) {
            picker.set_loading(loading);
        }
    }

    /// Resume the side-panel's currently selected previous session, if
    /// any. Exposed for the click/Enter handler in `screen::bridges::agent`.
    pub fn activate_side_panel_selection(&mut self) -> bool {
        let Some(entry) = self.side_panel.selected_session().cloned() else {
            return false;
        };
        if Some(entry.id.as_str()) == self.session_id.as_deref() {
            return false;
        }
        self.switch_session(entry.id);
        true
    }

    /// Background refresh of the sub-agent / sibling-session list for
    /// the active session. Mirrors `maybe_refresh_side_panel_sessions`.
    pub fn maybe_refresh_side_panel_subagents(&mut self) {
        // The goal lives on the same per-session refresh cadence as the
        // branch list (both render in chat mode); piggyback here so the
        // Goal section updates live without a separate frame hook.
        self.maybe_refresh_session_goal();
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        if !self.side_panel.should_refresh_subagents() {
            return;
        }
        self.side_panel.mark_subagent_refresh_kicked();
        let server = self.server.clone();
        let tx = self.background_tx.clone();
        std::thread::Builder::new()
            .name("neoism-agent-subagents".into())
            .spawn(move || {
                let entries = match fetch_subagent_entries(&server, &session_id) {
                    Ok(entries) => entries,
                    Err(_) => Vec::new(),
                };
                let _ = tx.send(
                    NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed(entries),
                );
            })
            .ok();
    }

    /// Debounced background refetch of the session's persistent goal.
    /// Fires on session change / `SESSION_UPDATED` (via
    /// `invalidate_goal_refresh`) and on a slow steady cadence otherwise.
    pub(crate) fn maybe_refresh_session_goal(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        if !self.side_panel.should_refresh_goal() {
            return;
        }
        self.side_panel.mark_goal_refresh_kicked();
        let server = self.server.clone();
        let tx = self.background_tx.clone();
        std::thread::Builder::new()
            .name("neoism-agent-goal".into())
            .spawn(move || {
                let goal = fetch_session_goal(&server, &session_id).unwrap_or(None);
                let _ = tx.send(NeoismAgentBackgroundUpdate::SessionGoalRefreshed {
                    session_id,
                    goal,
                });
            })
            .ok();
    }

    pub(crate) fn hydrate_runtime_status_for_session(&mut self, session_id: &str) {
        let Ok(statuses) = fetch_session_statuses(&self.server) else {
            self.sync_subagent_waiting_clock();
            return;
        };
        self.active_subagent_ids.clear();
        self.active_subagent_started_at.clear();
        if let Some(status) = statuses.get(session_id) {
            self.queued_prompt_count = status.queue_count;
            self.queued_prompt_preview = status.preview.clone();
            if matches!(status.kind.as_str(), "busy" | "retry") {
                self.refresh_streaming_from_tail();
                if !self.is_streaming() {
                    self.note_streaming(NeoismAgentStreamingState::Thinking, None);
                }
                if let Some(started_at) = status.started_at {
                    let started = instant_from_epoch_millis(started_at);
                    self.streaming_started_at = Some(started);
                    self.streaming_state_changed_at = Some(started);
                }
            }
        }

        for entry in self.side_panel.subagents().to_vec() {
            if let Some(status) = statuses.get(&entry.id) {
                // Server status is authoritative here. The previous
                // fallback to `side_panel.branch_activity` could reuse
                // a stale `Active` status from before the pane's event
                // stream was redirected to the subagent — leaving the
                // parent's "Sub-agents working" status row stuck on
                // after the subagent had already completed.
                let branch_status = branch_status_from_runtime(&status.kind);
                self.note_subagent_runtime(
                    entry.id.clone(),
                    branch_status,
                    status.started_at,
                );
            }
        }
        for (child_id, status) in statuses.iter().filter(|(_, status)| {
            status.parent_session_id.as_deref() == Some(session_id)
                && matches!(status.kind.as_str(), "busy" | "retry")
        }) {
            let branch_status = if status.kind == "retry" {
                BranchStatus::WaitingPermission
            } else {
                BranchStatus::Active
            };
            self.note_subagent_runtime(
                child_id.clone(),
                branch_status,
                status.started_at,
            );
            self.set_task_message_status(child_id, "running");
        }
        self.reconcile_task_message_statuses();
        self.sync_subagent_waiting_clock();
    }

    /// Switch to the side-panel-highlighted sub-agent (or back to the
    /// parent). Called from the click / Enter path when chat mode is
    /// showing the Sub Agents list.
    pub fn activate_side_panel_subagent(&mut self) -> bool {
        let Some(entry) = self.side_panel.selected_row().cloned() else {
            return false;
        };
        if Some(entry.id.as_str()) == self.session_id.as_deref() {
            return false;
        }
        self.switch_session(entry.id);
        true
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming_state != NeoismAgentStreamingState::Idle
            && self.streaming_started_at.is_some()
    }

    pub fn has_status_activity(&self) -> bool {
        self.is_streaming()
            || self.active_subagent_count() > 0
            || self.running_background_task_count() > 0
    }

    pub fn running_background_task_count(&self) -> usize {
        running_background_task_count(&self.messages)
    }

    pub(crate) fn ensure_background_task_activity_clock(&mut self) {
        if self.running_background_task_count() > 0 {
            if self.background_tasks_started_at.is_none() {
                self.background_tasks_started_at = Some(Instant::now());
            }
        } else {
            self.background_tasks_started_at = None;
        }
    }

    pub fn streaming_state(&self) -> NeoismAgentStreamingState {
        if !self.is_streaming() && self.active_subagent_count() > 0 {
            return NeoismAgentStreamingState::WaitingSubagents;
        }
        if !self.is_streaming() && self.running_background_task_count() > 0 {
            return NeoismAgentStreamingState::BackgroundTasks;
        }
        self.streaming_state
    }

    pub fn streaming_label(&self) -> String {
        // Always render just the state label — no tool name, no slash
        // command, no surplus chrome. Elapsed time is appended by the
        // renderer.
        self.streaming_state().label().to_string()
    }

    pub fn streaming_elapsed_seconds(&self) -> Option<f32> {
        if !self.is_streaming() && self.active_subagent_count() > 0 {
            return self
                .subagent_waiting_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        if !self.is_streaming() && self.running_background_task_count() > 0 {
            return self
                .background_tasks_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        if !self.has_status_activity() {
            return None;
        }
        self.streaming_started_at
            .map(|started| started.elapsed().as_secs_f32())
    }

    pub(crate) fn active_subagent_count(&self) -> usize {
        if self.is_subagent_session() {
            return 0;
        }
        self.side_panel
            .active_child_count(self.session_id.as_deref())
    }

    pub(crate) fn note_subagent_runtime(
        &mut self,
        session_id: String,
        status: BranchStatus,
        started_at: Option<u64>,
    ) {
        self.side_panel
            .set_branch_activity_status(session_id.clone(), status);
        self.side_panel
            .set_branch_activity_started_at(session_id.clone(), started_at);
        if matches!(
            status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) {
            self.active_subagent_ids.insert(session_id.clone());
            if let Some(started_at) = started_at {
                self.active_subagent_started_at
                    .insert(session_id, started_at);
            }
        } else {
            self.active_subagent_ids.remove(&session_id);
            self.active_subagent_started_at.remove(&session_id);
        }
    }

    /// Part-level activity for a child (raw text/reasoning/tool delta),
    /// subordinate to its authoritative lifecycle. Once the branch has
    /// latched a terminal state, late "responding"/"thinking" deltas are
    /// dropped instead of resurrecting the row — the fix for sub-agents
    /// that stayed stuck on "responding"/"working" after finishing.
    /// Returns whether the update was applied.
    pub(crate) fn note_subagent_part_activity(
        &mut self,
        session_id: String,
        status: BranchStatus,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) -> bool {
        let applied = self.side_panel.note_subagent_part_activity(
            &session_id,
            status,
            current_tool,
            started_at,
        );
        if !applied {
            self.active_subagent_ids.remove(&session_id);
            self.active_subagent_started_at.remove(&session_id);
            return false;
        }
        if matches!(
            status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) {
            self.active_subagent_ids.insert(session_id.clone());
            if let Some(started_at) = started_at {
                self.active_subagent_started_at
                    .insert(session_id, started_at);
            }
        } else {
            self.active_subagent_ids.remove(&session_id);
            self.active_subagent_started_at.remove(&session_id);
        }
        true
    }

    pub(crate) fn upsert_live_subagent_entry(
        &mut self,
        session_id: &str,
        title: Option<String>,
        agent: Option<String>,
    ) {
        if session_id.is_empty() || Some(session_id) == self.session_id.as_deref() {
            return;
        }
        if let Some(parent_id) = self
            .parent_session_id
            .as_deref()
            .or(self.session_id.as_deref())
            .filter(|id| !id.is_empty())
        {
            self.side_panel
                .ensure_subagent_main_entry(parent_id.to_string());
        }
        let inserted = self.side_panel.upsert_subagent(
            session_id.to_string(),
            title.unwrap_or_else(|| "subagent".to_string()),
            agent.unwrap_or_else(|| "subagent".to_string()),
        );
        if inserted {
            self.side_panel.mark_subagent_tree_dirty();
        }
    }

    pub(crate) fn set_task_message_status(&mut self, task_id: &str, status: &str) {
        let Some(index) = self.messages.iter().rposition(|message| {
            message.kind == NeoismAgentMessageKind::Tool
                && message.tool == "task"
                && (message.text.contains(task_id) || message.detail.contains(task_id))
        }) else {
            return;
        };
        self.set_task_message_status_at(index, status);
    }

    pub(crate) fn set_task_message_status_at(&mut self, index: usize, status: &str) {
        let normalized = match status {
            "completed" | "error" | "running" => status,
            "stopped" => "error",
            _ => "running",
        };
        let Some(message) = self.messages.get_mut(index) else {
            return;
        };
        message.status = normalized.to_string();
        for field in [&mut message.text, &mut message.detail] {
            rewrite_task_status_markers(field, normalized);
        }
        self.mark_timeline_message_and_next_dirty_at(index);
    }

    pub(crate) fn reconcile_task_message_statuses(&mut self) {
        let active_task_ids = self.active_subagent_ids.clone();
        let explicit_statuses = self
            .side_panel
            .subagents()
            .iter()
            .filter_map(|entry| {
                task_message_status_from_runtime(entry.runtime_status.as_deref()?)
                    .map(|status| (entry.id.clone(), status))
            })
            .collect::<HashMap<_, _>>();
        for (task_id, status) in &explicit_statuses {
            if !active_task_ids.contains(task_id) {
                self.note_subagent_runtime(
                    task_id.clone(),
                    branch_status_from_runtime(status),
                    None,
                );
            }
        }
        let task_updates = self
            .messages
            .iter()
            .enumerate()
            .filter(|message| {
                message.1.kind == NeoismAgentMessageKind::Tool && message.1.tool == "task"
            })
            .filter_map(|(index, message)| {
                let task_id = task_id_from_task_message(message)?;
                let status = explicit_statuses
                    .get(&task_id)
                    .copied()
                    .or_else(|| {
                        self.side_panel
                            .branch_activity(&task_id)
                            .and_then(|activity| {
                                task_message_status_from_branch(activity.status)
                            })
                    })
                    .or_else(|| {
                        active_task_ids.contains(&task_id).then_some("running")
                    })?;
                Some((index, status))
            })
            .collect::<Vec<_>>();
        for (index, status) in task_updates {
            self.set_task_message_status_at(index, status);
        }
    }

    pub(crate) fn sync_subagent_waiting_clock(&mut self) {
        if self.active_subagent_count() > 0 {
            self.subagent_waiting_started_at = self
                .side_panel
                .active_child_started_at(self.session_id.as_deref())
                .map(instant_from_epoch_millis)
                .or(self.subagent_waiting_started_at)
                .or_else(|| Some(Instant::now()));
        } else {
            self.subagent_waiting_started_at = None;
        }
    }

    pub fn queued_prompt_count(&self) -> usize {
        self.queued_prompt_count
    }

    pub(crate) fn suppress_streaming_after_abort(&self) -> bool {
        self.abort_requested_at
            .is_some_and(|requested| requested.elapsed() <= ABORT_STREAM_SUPPRESSION)
    }

    pub(crate) fn refresh_streaming_from_tail(&mut self) {
        let Some(tail) = self.messages.last() else {
            return;
        };
        let kind = tail.kind;
        let title = tail.title.clone();
        self.note_streaming_from_part(kind, &title);
    }

    pub(crate) fn note_streaming_from_part(
        &mut self,
        kind: NeoismAgentMessageKind,
        title: &str,
    ) {
        if matches!(
            kind,
            NeoismAgentMessageKind::Reasoning
                | NeoismAgentMessageKind::Tool
                | NeoismAgentMessageKind::Subtask
        ) {
            self.retain_current_turn_trace();
        }
        match kind {
            NeoismAgentMessageKind::Reasoning => {
                self.note_streaming(NeoismAgentStreamingState::Thinking, None);
            }
            NeoismAgentMessageKind::Tool | NeoismAgentMessageKind::Subtask => {
                let tool = (!title.is_empty()).then(|| title.to_string());
                self.note_streaming(NeoismAgentStreamingState::Working, tool);
            }
            NeoismAgentMessageKind::Assistant => {
                self.note_streaming(NeoismAgentStreamingState::Generating, None);
            }
            // User / System messages don't move us into a streaming state.
            NeoismAgentMessageKind::User
            | NeoismAgentMessageKind::System
            | NeoismAgentMessageKind::Compaction => {}
        }
    }

    pub(crate) fn note_streaming(
        &mut self,
        state: NeoismAgentStreamingState,
        tool: Option<String>,
    ) {
        if state == NeoismAgentStreamingState::Compacting {
            self.retain_current_turn_trace();
        }
        if state == NeoismAgentStreamingState::Idle {
            self.streaming_state = state;
            self.streaming_started_at = None;
            self.streaming_state_changed_at = None;
            self.streaming_tool_label = None;
            return;
        }
        if self.streaming_started_at.is_none() {
            self.streaming_started_at = Some(Instant::now());
        }
        // Stamp the transition so the renderer can drive a per-letter
        // scramble animation when the label word swaps.
        if self.streaming_state != state {
            self.streaming_state_changed_at = Some(Instant::now());
        } else if self.streaming_state_changed_at.is_none() {
            self.streaming_state_changed_at = Some(Instant::now());
        }
        self.streaming_state = state;
        self.streaming_tool_label = tool;
    }

    pub fn streaming_state_changed_elapsed(&self) -> Option<f32> {
        if !self.is_streaming() && self.active_subagent_count() > 0 {
            return self
                .subagent_waiting_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        if !self.is_streaming() && self.running_background_task_count() > 0 {
            return self
                .background_tasks_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        self.streaming_state_changed_at
            .map(|t| t.elapsed().as_secs_f32())
    }
}
