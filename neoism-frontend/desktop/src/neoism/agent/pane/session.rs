use super::*;

impl NeoismAgentPane {
    pub fn switch_server(&mut self, server: String) {
        let server = server.trim_end_matches('/').to_string();
        if self.server == server {
            return;
        }
        let directory = self.directory.clone();
        *self = Self::default();
        self.server = server;
        self.directory = directory;
        // Construction queued defaults for the original (usually local)
        // server. Retargeting replaces that channel and state, so hydrate the
        // selected host as well; joined panes otherwise stay on
        // "server default" / "none" while the host uses its configured model.
        self.apply_config_defaults();
        // Keep server switching off the UI thread. The next visible sidebar
        // render calls `maybe_refresh_side_panel_sessions`, which paints its
        // unloaded skeleton immediately and hydrates the host's chats through
        // the existing background channel.
    }

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
        if self.code_copy_feedback_is_animating() {
            return Some("code_copy_feedback");
        }
        if self.agent_label_changed_elapsed_ms().is_some() {
            return Some("agent_label_transition");
        }
        if !self.has_conversation() {
            return Some("agent_home_wordmark");
        }
        if self.fx_active() {
            return Some("easter_fx");
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
        if self.timeline_interaction_settle_active() {
            return Some("timeline_interaction_settle");
        }
        // Provider time, status dots, and streamed deltas advance even across
        // transient run-idle edges. Keep draining and painting until the
        // durable execution/branch activity settles.
        if self.streaming_state != NeoismAgentStreamingState::Idle
            || self
                .execution_activity
                .as_ref()
                .is_some_and(|activity| self.execution_status_live(activity))
            || self.active_subagent_count() > 0
            || self.viewed_subagent_outstanding()
        {
            return Some("streaming");
        }
        // A transient idle gap keeps the last status label on screen
        // through the display grace hold; keep frames coming so it can
        // expire (and erase) without waiting for the next input event.
        if self.side_panel.held_status_display().is_some() {
            return Some("streaming_status_hold");
        }
        // Only an on-screen side panel drives the render loop. A pane
        // that has never been laid out (fresh/backgrounded) must not
        // spin redraws off its still-unloaded sessions skeleton — the
        // shimmer only needs to animate once the panel is actually
        // painted (`last_panel_rect` is stamped during render).
        if self.side_panel.last_panel_rect().is_some() && self.side_panel.is_animating() {
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
        // Bound each UI insertion independently of conversational turn size.
        // A single tool-heavy turn can contain hundreds of stored messages;
        // loading until its user boundary causes a large synchronous prepend
        // and Markdown layout hitch when the background fetch completes.
        const LOAD_OLDER_LIMIT: usize = 64;
        let threshold = (viewport_h * 0.75).max(720.0);
        if self.timeline_follow_bottom
            || scroll_top > threshold
            || !self.timeline_history.has_older
            || self.timeline_history.loading_older
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
        self.request_side_panel_session_page(None);
    }

    fn request_side_panel_session_page(&mut self, cursor: Option<String>) {
        let generation = self.side_panel.next_session_request_generation();
        let server = self.server.clone();
        let current = self.session_id.clone();
        let directory = self.directory.clone();
        let tx = self.background_tx.clone();
        if std::thread::Builder::new()
            .name("neoism-agent-sessions".into())
            .spawn(move || {
                let entries = crate::neoism::agent::api::fetch_session_entries_page(
                    &server,
                    current.as_deref(),
                    directory.as_deref(),
                    cursor.as_deref(),
                );
                let _ =
                    tx.send(NeoismAgentBackgroundUpdate::SidePanelSessionsRefreshed {
                        generation,
                        requested_cursor: cursor,
                        result: entries,
                    });
            })
            .is_err()
        {
            if self.side_panel.session_request_is_current(generation) {
                self.side_panel
                    .settle_session_page_error("couldn't start sessions request");
            }
        }
    }

    pub fn scroll_side_panel_pixels(&mut self, delta_pixels: f32, rows: usize) {
        self.side_panel.scroll_pixels(delta_pixels, rows);
        self.maybe_request_side_panel_session_page();
    }

    pub fn maybe_request_side_panel_session_page(&mut self) {
        if let Some(cursor) = self.side_panel.begin_session_page_near_end() {
            self.request_side_panel_session_page(Some(cursor));
        }
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
            return self.pending_session_switch.take().is_some();
        }
        self.switch_session(entry.id);
        true
    }

    /// Whether the side-panel's selected home-mode session is the one
    /// already open. Used so that picking the current (green-dotted)
    /// session from the "← Back" recent list returns to the live chat.
    pub fn selected_side_panel_session_is_current(&self) -> bool {
        self.side_panel
            .selected_session()
            .map(|entry| Some(entry.id.as_str()) == self.session_id.as_deref())
            .unwrap_or(false)
    }

    /// Take one sub-agent tree snapshot after bootstrap, a lifecycle edge, or
    /// an event-stream reconnect. Live status is driven by SSE events; this
    /// method does not establish a periodic polling cadence.
    pub fn maybe_refresh_side_panel_subagents(&mut self) {
        // The goal lives on the same per-session refresh cadence as the
        // branch list (both render in chat mode); piggyback here so the
        // Goal section updates live without a separate frame hook.
        self.maybe_refresh_session_goal();
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        let Some(generation) = self.side_panel.begin_subagent_refresh() else {
            return;
        };
        let server = self.server.clone();
        let tx = self.background_tx.clone();
        let requested_session_id = session_id.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("neoism-agent-subagents".into())
            .spawn(move || {
                let result = fetch_subagent_entries(&server, &requested_session_id);
                let _ =
                    tx.send(NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed {
                        session_id: requested_session_id,
                        generation,
                        result,
                    });
            })
        {
            self.side_panel.complete_subagent_refresh(generation);
            self.side_panel.settle_failed_subagent_refresh();
            tracing::warn!(%error, "failed to start subagent refresh worker");
        }
    }

    /// Reconcile the durable family projection whenever this pane becomes
    /// visible again. Live-tail SSE intentionally has no replay window, so
    /// activation is the correctness barrier for lifecycle edges missed while
    /// another workspace owned the frame loop.
    pub fn reconcile_after_activation(&mut self) {
        self.side_panel.mark_subagent_tree_dirty();
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        self.hydrate_runtime_status_for_session(&session_id);
        let stream_session_id = self.session_tree_root_id.clone().unwrap_or(session_id);
        self.start_session_updates(&stream_session_id);
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
        let session_id = session_id.to_string();
        self.runtime_status_request_generation =
            self.runtime_status_request_generation.wrapping_add(1);
        let request_generation = self.runtime_status_request_generation;
        let runtime_revision = self.session_runtime_revision(&session_id);
        self.runtime_status_requests
            .insert(session_id.clone(), request_generation);
        let server = self.server.clone();
        let tx = self.background_sender();
        let worker_session_id = session_id.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("neoism-agent-runtime-{session_id}"))
            .spawn(move || {
                let result = fetch_session_statuses(&server);
                let runtime = fetch_family_runtime(&server, &worker_session_id);
                let permissions = fetch_pending_permissions(&server);
                let questions = fetch_pending_questions(&server);
                let _ =
                    tx.send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
                        session_id: worker_session_id,
                        request_generation,
                        runtime_revision,
                        result,
                        runtime,
                        permissions,
                        questions,
                    });
            })
        {
            self.runtime_status_requests.remove(&session_id);
            tracing::warn!(%error, "failed to start agent runtime refresh worker");
        }
    }

    pub(crate) fn apply_runtime_status_for_session(
        &mut self,
        session_id: &str,
        statuses: &HashMap<String, super::super::api::SessionStatusSnapshot>,
    ) {
        // Background jobs are owned by the separately versioned family
        // runtime snapshot. A failed runtime refresh must not erase warm state.
        let active_status = statuses
            .get(session_id)
            .filter(|status| matches!(status.kind.as_str(), "busy" | "retry"));
        if let Some(status) = active_status {
            self.terminal_idle_sessions.remove(session_id);
            self.queued_prompt_count = self
                .status_timing
                .queue_count(status.queue_count, status.started_at);
            self.queued_prompt_preview = status.preview.clone();
            self.refresh_streaming_from_tail();
            if !self.is_streaming() {
                self.note_streaming(NeoismAgentStreamingState::Thinking, None);
            }
            if let Some(started_at) = status.started_at {
                let started = self
                    .status_timing
                    .started_at(self.streaming_started_at, started_at);
                self.streaming_started_at = Some(started);
                if !self.status_timing.enabled()
                    || self.streaming_state_changed_at.is_none()
                {
                    self.streaming_state_changed_at = Some(started);
                }
            }
            // A cold child activation paints its fetched transcript before
            // this runtime snapshot arrives. Open the existing trace as soon
            // as the snapshot confirms the child is ongoing.
            self.reveal_ongoing_session_trace();
        } else {
            self.terminal_idle_sessions.insert(session_id.to_string());
            // Runtime status is authoritative. Idle sessions are omitted from
            // the map, so explicitly settle any optimistic activity retained
            // across a dropped event stream or history refresh.
            self.queued_prompt_count = 0;
            self.queued_prompt_preview = None;
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
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
                self.note_subagent_observed_runtime(
                    entry.id.clone(),
                    branch_status,
                    None,
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
            let applied = self.note_subagent_observed_runtime(
                child_id.clone(),
                branch_status,
                None,
                status.started_at,
            );
            if applied {
                self.set_task_message_status(child_id, "running");
            }
        }
        self.reconcile_task_message_statuses();
        self.sync_subagent_waiting_clock();
        self.runtime_hydrated_sessions
            .insert(session_id.to_string());
    }

    pub(crate) fn session_runtime_revision(&self, session_id: &str) -> u64 {
        self.session_runtime_revisions
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn note_session_runtime_event(&mut self, session_id: &str) {
        let revision = self
            .session_runtime_revisions
            .entry(session_id.to_string())
            .or_default();
        *revision = revision.wrapping_add(1);
        self.runtime_hydrated_sessions
            .insert(session_id.to_string());
    }

    pub(crate) fn note_session_branch_runtime(
        &mut self,
        session_id: &str,
        status: BranchStatus,
        started_at: Option<u64>,
    ) {
        self.note_session_runtime_event(session_id);
        let active = self.session_id.as_deref() == Some(session_id);
        if active {
            if matches!(status, BranchStatus::Completed | BranchStatus::Stopped) {
                self.note_streaming(NeoismAgentStreamingState::Idle, None);
                self.abort_requested_at = None;
            } else if !self.is_streaming() {
                self.note_streaming(NeoismAgentStreamingState::Thinking, None);
                if let Some(started_at) = started_at {
                    let started = self
                        .status_timing
                        .started_at(self.streaming_started_at, started_at);
                    self.streaming_started_at = Some(started);
                    self.streaming_state_changed_at = Some(started);
                }
            }
            return;
        }
        let runtime = &mut self
            .session_cache
            .entry(session_id.to_string())
            .or_insert_with(CachedAgentSession::live_only)
            .runtime;
        if matches!(status, BranchStatus::Completed | BranchStatus::Stopped) {
            runtime.note_streaming(NeoismAgentStreamingState::Idle, None);
        } else if !runtime.is_streaming() {
            runtime.note_streaming(NeoismAgentStreamingState::Thinking, None);
            if let Some(started_at) = started_at {
                let started = runtime
                    .status_timing
                    .started_at(runtime.streaming_started_at, started_at);
                runtime.streaming_started_at = Some(started);
                runtime.streaming_state_changed_at = Some(started);
            }
        }
    }

    /// Settle only the child's current run chrome. This must not mutate the
    /// parent-owned branch lifecycle/count.
    pub(crate) fn note_session_run_idle(&mut self, session_id: &str) {
        self.note_session_runtime_event(session_id);
        if self.session_id.as_deref() == Some(session_id) {
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
        } else if let Some(cached) = self.session_cache.get_mut(session_id) {
            cached
                .runtime
                .note_streaming(NeoismAgentStreamingState::Idle, None);
        }
    }

    /// Switch to the side-panel-highlighted sub-agent (or back to the
    /// parent). Called from the click / Enter path when chat mode is
    /// showing the Sub Agents list.
    pub fn activate_side_panel_subagent(&mut self) -> bool {
        let Some(entry) = self.side_panel.selected_row().cloned() else {
            return false;
        };
        if Some(entry.id.as_str()) == self.session_id.as_deref() {
            return self.pending_session_switch.take().is_some();
        }
        self.switch_session(entry.id);
        true
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming_state != NeoismAgentStreamingState::Idle
            && self.streaming_started_at.is_some()
    }

    pub fn interruptible_run_active(&self) -> bool {
        self.is_streaming()
            || self.viewed_subagent_outstanding()
            || self.active_subagent_count() > 0
    }

    pub fn has_status_activity(&self) -> bool {
        self.is_streaming()
            || self.viewed_subagent_outstanding()
            || self.active_subagent_count() > 0
            || self.running_background_task_count() > 0
            // The grace hold counts as activity so the status row's
            // reserved height (see shared `view/timeline/render.rs`)
            // doesn't collapse-and-return around a transient idle
            // reading — that one-frame reflow was the visible "bounce".
            || self.side_panel.held_status_display().is_some()
    }

    fn viewed_subagent_outstanding(&self) -> bool {
        self.is_subagent_session()
            && self.session_id.as_deref().is_some_and(|session_id| {
                self.side_panel
                    .branch_activity(session_id)
                    .is_some_and(|activity| {
                        matches!(
                            activity.status,
                            BranchStatus::Active | BranchStatus::WaitingPermission
                        )
                    })
            })
    }

    fn execution_status_live(
        &self,
        activity: &neoism_ui::panels::agent_pane::state::ExecutionActivityState,
    ) -> bool {
        if activity.finished {
            return false;
        }
        let Some(session_id) = self.session_id.as_deref() else {
            return true;
        };
        session_id == activity.root_session_id
            || self.viewed_subagent_outstanding()
            || activity
                .session_activities
                .get(session_id)
                .is_some_and(|session| !session.active_segments.is_empty())
    }

    pub fn running_background_task_count(&self) -> usize {
        self.running_background_task_count
    }

    pub(crate) fn apply_running_background_tasks(
        &mut self,
        epoch: &str,
        revision: u64,
        tasks: &[(String, u64)],
    ) -> bool {
        if self.background_jobs_epoch.as_deref() == Some(epoch)
            && revision <= self.background_jobs_revision
        {
            return false;
        }
        let count = tasks.len();
        let started_at = tasks
            .iter()
            .map(|(_, started_at)| *started_at)
            .min()
            .map(instant_from_epoch_millis);
        let changed = self.running_background_task_count != count
            || self.background_tasks_started_at != started_at;
        self.running_background_task_count = count;
        self.background_tasks_started_at = started_at;
        self.background_jobs_epoch = Some(epoch.to_string());
        self.background_jobs_revision = revision;
        if count == 0 {
            self.background_task_details_expanded = false;
        }
        changed
    }

    pub(crate) fn ensure_background_task_activity_clock(&mut self) {
        self.running_background_task_count =
            running_background_task_count(&self.messages);
        if self.running_background_task_count > 0 {
            if self.background_tasks_started_at.is_none() {
                self.background_tasks_started_at = Some(Instant::now());
            }
        } else {
            self.background_tasks_started_at = None;
        }
    }

    /// Raw, hold-free status derivation. The viewed session's own live
    /// run always wins: while the main agent is Pondering/Crafting (or
    /// the user just sent a prompt), the footer shows that verb even
    /// though children are still running in the background.
    /// "Sub-agents working" is the *aggregate idle* state — the viewed
    /// session has stopped streaming while ≥1 child keeps working.
    /// Viewing a child makes that child's own streaming state the
    /// "main" one for the label (`active_subagent_count` is already 0
    /// for subagent sessions).
    fn raw_streaming_status(&self) -> NeoismAgentStreamingState {
        if self.is_streaming() {
            return self.streaming_state;
        }
        if self.viewed_subagent_outstanding() {
            return NeoismAgentStreamingState::Working;
        }
        if self.active_subagent_count() > 0 {
            return NeoismAgentStreamingState::WaitingSubagents;
        }
        if self.running_background_task_count() > 0 {
            return NeoismAgentStreamingState::BackgroundTasks;
        }
        self.streaming_state
    }

    /// Display status with hysteresis — mirrors the shared pane's
    /// `streaming_state`: a raw Idle reading only clears the label
    /// after idle has persisted for the side panel's
    /// `STATUS_LABEL_GRACE`; transient gaps between events keep the
    /// last shown status so the label never blinks out mid-run.
    pub fn streaming_state(&self) -> NeoismAgentStreamingState {
        let raw = self.raw_streaming_status();
        if raw != NeoismAgentStreamingState::Idle {
            self.side_panel.note_status_display(
                shared_streaming_state(raw),
                self.raw_streaming_elapsed_seconds(),
            );
            return raw;
        }
        if let Some(held) = self.side_panel.held_status_display() {
            return desktop_streaming_state(held);
        }
        NeoismAgentStreamingState::Idle
    }

    pub fn streaming_label(&self) -> String {
        let state = self.streaming_state();
        if state == NeoismAgentStreamingState::Retrying {
            if let Some(reason) = self
                .streaming_tool_label
                .as_deref()
                .and_then(status_policy::compact_retry_reason)
            {
                return format!("Retrying · {reason}");
            }
        }
        // Other states stay intentionally terse; elapsed time is appended by
        // the renderer.
        state.label().to_string()
    }

    pub fn streaming_elapsed_seconds(&self) -> Option<f32> {
        if let Some(activity) = self.execution_activity.as_ref().filter(|activity| {
            self.status_timing.accepts_execution(&activity.execution_id)
        }) {
            let viewed = self
                .session_id
                .as_deref()
                .unwrap_or(&activity.root_session_id);
            let elapsed = self
                .execution_timer_anchor
                .as_ref()
                .filter(|anchor| anchor.matches(&activity.execution_id, viewed))
                .map_or_else(
                    || activity.elapsed_ms_for_session(Some(viewed), unix_millis()),
                    |anchor| anchor.elapsed_ms_at(Instant::now()),
                );
            return Some(elapsed as f32 / 1000.0);
        }
        // Display clock: raw clocks while active, then the held clock so
        // the timer/animation phase stays continuous through a transient
        // idle gap instead of snapping to zero.
        self.raw_streaming_elapsed_seconds()
            .or_else(|| self.side_panel.held_status_elapsed_seconds())
    }

    pub(crate) fn apply_execution_activity(
        &mut self,
        mut incoming: neoism_ui::panels::agent_pane::state::ExecutionActivityState,
    ) -> bool {
        if !self.status_timing.accepts_execution(&incoming.execution_id) {
            return false;
        }
        let now = Instant::now();
        let viewed = self
            .session_id
            .as_deref()
            .unwrap_or(&incoming.root_session_id);
        let current_floor = self
            .execution_timer_anchor
            .as_ref()
            .and_then(|anchor| {
                anchor
                    .matches(&incoming.execution_id, viewed)
                    .then(|| anchor.elapsed_ms_at(now))
            })
            .unwrap_or(0);
        if let Some(current) = self.execution_activity.as_ref() {
            if current.execution_id == incoming.execution_id
                && current.revision > incoming.revision
            {
                return false;
            }
            if current.execution_id != incoming.execution_id
                && incoming.execution_id <= current.execution_id
            {
                return false;
            }
            if current.execution_id == incoming.execution_id {
                incoming.completed_ms = incoming.completed_ms.max(current.completed_ms);
            }
        }
        let replaced = self
            .execution_activity
            .as_ref()
            .is_some_and(|current| current.execution_id != incoming.execution_id);
        self.execution_activity = Some(incoming);
        if let Some(activity) = self.execution_activity.as_ref() {
            self.execution_timer_anchor = Some(
                neoism_ui::panels::agent_pane::state::ExecutionTimerAnchor::from_snapshot(
                    activity,
                    self.session_id.as_deref(),
                    self.status_timing.execution_wall_ms(
                        &activity.execution_id,
                        now,
                        unix_millis(),
                    ),
                    now,
                    current_floor,
                ),
            );
        }
        if self
            .execution_activity
            .as_ref()
            .is_some_and(|activity| activity.finished)
        {
            self.side_panel.clear_status_display_hold();
        }
        if replaced {
            self.active_subagent_ids.clear();
            self.active_subagent_started_at.clear();
            self.side_panel.reset_branch_lifecycle();
            if let Some(activity) = self.execution_activity.as_ref() {
                self.terminal_subagent_revisions
                    .retain(|_, (execution_id, _)| {
                        execution_id == &activity.execution_id
                    });
            }
        }
        true
    }

    pub(crate) fn execution_activity_matches(
        &self,
        execution_id: &str,
        revision: u64,
    ) -> bool {
        self.execution_activity.as_ref().is_some_and(|current| {
            current.execution_id == execution_id && current.revision == revision
        })
    }

    pub(crate) fn apply_runtime_lifecycle_snapshot<I>(
        &mut self,
        execution: Option<neoism_ui::panels::agent_pane::state::ExecutionActivityState>,
        root_session_id: String,
        family_revision: u64,
        branches: I,
    ) -> bool
    where
        I: IntoIterator<Item = (String, String, Option<u64>)>,
    {
        // Execution and branches are one server snapshot. Reject both when
        // its execution revision is stale so branches cannot travel alone.
        let (execution_current, execution_changed) = execution
            .map(|activity| {
                let execution_id = activity.execution_id.clone();
                let revision = activity.revision;
                let changed = self.apply_execution_activity(activity);
                (
                    changed || self.execution_activity_matches(&execution_id, revision),
                    changed,
                )
            })
            .unwrap_or((true, false));
        if !execution_current {
            return false;
        }
        self.apply_branch_lifecycle_snapshot(root_session_id, family_revision, branches)
            || execution_changed
    }

    pub(crate) fn apply_branch_lifecycle_snapshot(
        &mut self,
        root_session_id: String,
        revision: u64,
        branches: impl IntoIterator<Item = (String, String, Option<u64>)>,
    ) -> bool {
        if self.runtime_snapshot_root.as_deref() == Some(root_session_id.as_str())
            && revision < self.runtime_snapshot_revision
        {
            return false;
        }
        if self.runtime_snapshot_root.as_deref() != Some(root_session_id.as_str()) {
            self.runtime_snapshot_root = Some(root_session_id);
            self.runtime_snapshot_revision = 0;
            self.active_subagent_ids.clear();
            self.active_subagent_started_at.clear();
            self.side_panel.reset_branch_lifecycle();
        }
        self.runtime_snapshot_revision = revision;
        let execution_id = self
            .execution_activity
            .as_ref()
            .map(|activity| activity.execution_id.clone());
        let branches = branches.into_iter().collect::<Vec<_>>();
        let mut viewed_terminal = false;
        let branch_ids = branches
            .iter()
            .map(|(session_id, _, _)| session_id.clone())
            .collect::<std::collections::HashSet<_>>();
        self.active_subagent_ids
            .retain(|session_id| branch_ids.contains(session_id));
        self.active_subagent_started_at
            .retain(|session_id, _| branch_ids.contains(session_id));
        self.side_panel.retain_authoritative_branches(&branch_ids);
        for (session_id, status, started_at) in branches {
            let mut status = match status.as_str() {
                "outstanding" => BranchStatus::Active,
                "completed" => BranchStatus::Completed,
                _ => BranchStatus::Stopped,
            };
            if matches!(
                status,
                BranchStatus::Active | BranchStatus::WaitingPermission
            ) {
                if let (
                    Some(current_execution),
                    Some((terminal_execution, terminal_revision)),
                ) = (
                    execution_id.as_deref(),
                    self.terminal_subagent_revisions.get(&session_id),
                ) {
                    if terminal_execution == current_execution
                        && revision <= *terminal_revision
                    {
                        status = self
                            .side_panel
                            .branch_activity(&session_id)
                            .map(|activity| activity.status)
                            .filter(|status| {
                                matches!(
                                    status,
                                    BranchStatus::Completed | BranchStatus::Stopped
                                )
                            })
                            .unwrap_or(BranchStatus::Completed);
                    } else if terminal_execution == current_execution {
                        self.terminal_subagent_revisions.remove(&session_id);
                    }
                }
            }
            if matches!(
                status,
                BranchStatus::Active | BranchStatus::WaitingPermission
            ) {
                self.upsert_live_subagent_entry(&session_id, None, None);
            }
            // A newer family revision is authoritative proof of a genuine
            // continuation and may reopen the same child session ID.
            self.side_panel
                .set_branch_activity_status_from_recovery(session_id.clone(), status);
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
                viewed_terminal |=
                    self.reconcile_viewed_subagent_runtime(&session_id, status);
            }
        }
        if viewed_terminal {
            self.side_panel.clear_status_display_hold();
        }
        // Rewrite the parent task card while terminal branch evidence still
        // exists; recovery pruning removes that evidence immediately.
        self.reconcile_task_message_statuses();
        self.side_panel.prune_expired_completed_subagents();
        self.sync_subagent_waiting_clock();
        true
    }

    pub(crate) fn note_subagent_terminal_revision(
        &mut self,
        session_id: &str,
        root_session_id: Option<&str>,
        execution_id: Option<&str>,
        family_revision: Option<u64>,
    ) -> bool {
        let (Some(root), Some(execution), Some(revision)) =
            (root_session_id, execution_id, family_revision)
        else {
            return true;
        };
        if revision == 0
            || self
                .runtime_snapshot_root
                .as_deref()
                .is_some_and(|current| current != root)
        {
            return false;
        }
        if let Some(current) = self.execution_activity.as_ref() {
            if execution < current.execution_id.as_str()
                || (current.execution_id == execution
                    && self.runtime_snapshot_revision > revision)
            {
                return false;
            }
        }
        self.terminal_subagent_revisions
            .entry(session_id.to_string())
            .and_modify(|(current_execution, current_revision)| {
                if current_execution != execution || revision > *current_revision {
                    *current_execution = execution.to_string();
                    *current_revision = revision;
                }
            })
            .or_insert_with(|| (execution.to_string(), revision));
        true
    }

    fn raw_streaming_elapsed_seconds(&self) -> Option<f32> {
        // Clock precedence mirrors `raw_streaming_status`: the viewed
        // session's own run clock while it streams, then the aggregate
        // sub-agents / background-tasks clocks once it has stopped.
        if self.is_streaming() {
            return self
                .streaming_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        if self.active_subagent_count() > 0 {
            return self
                .subagent_waiting_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        if self.running_background_task_count() > 0 {
            return self
                .background_tasks_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        None
    }

    /// Whether `session_id` belongs to the conversation family the side
    /// panel currently tracks: the viewed session, its parent, or any
    /// row of the subagent roster (whose first entry is the family
    /// root). Session switches within one family keep the parent-keyed
    /// roster alive — a subagent transcript is an *extension* of the
    /// main chat, so entering a child must not clear the sidebar's
    /// names/statuses of its siblings. Mirrors the shared pane's
    /// `session_family_contains`.
    pub(crate) fn session_family_contains(&self, session_id: &str) -> bool {
        if session_id.is_empty() {
            return false;
        }
        self.session_id.as_deref() == Some(session_id)
            || self.parent_session_id.as_deref() == Some(session_id)
            || self
                .side_panel
                .subagents()
                .iter()
                .any(|entry| entry.id == session_id)
    }

    pub(crate) fn active_subagent_count(&self) -> usize {
        if self.is_subagent_session() {
            return 0;
        }
        let mut active_ids = self.side_panel.active_child_ids(self.session_id.as_deref());
        active_ids.extend(
            self.active_subagent_ids
                .iter()
                .filter(|session_id| {
                    Some(session_id.as_str()) != self.session_id.as_deref()
                })
                .cloned(),
        );
        active_ids.len()
    }

    pub(crate) fn clear_family_activity(&mut self) {
        self.active_subagent_ids.clear();
        self.active_subagent_started_at.clear();
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

    pub(crate) fn note_subagent_observed_runtime(
        &mut self,
        session_id: String,
        status: BranchStatus,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) -> bool {
        if matches!(status, BranchStatus::Completed | BranchStatus::Stopped) {
            self.side_panel.set_branch_activity_tool(
                session_id.clone(),
                status,
                current_tool,
                started_at,
            );
            self.note_subagent_runtime(session_id, status, started_at);
            return true;
        }
        self.note_subagent_part_activity(session_id, status, current_tool, started_at)
    }

    pub(crate) fn settle_tracked_subagents(&mut self, status: BranchStatus) {
        let child_ids = self
            .side_panel
            .subagents()
            .iter()
            .skip(1)
            .map(|entry| entry.id.clone())
            .chain(self.active_subagent_ids.iter().cloned())
            .filter(|id| Some(id.as_str()) != self.session_id.as_deref())
            .collect::<BTreeSet<_>>();
        for child_id in child_ids {
            self.note_subagent_runtime(child_id, status, None);
        }
        self.sync_subagent_waiting_clock();
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
            // A child part delta can arrive before the parent task/status
            // event. Create its row from the accepted live signal so both
            // the sidebar and aggregate composer status remain continuous.
            self.upsert_live_subagent_entry(&session_id, None, None);
            self.active_subagent_ids.insert(session_id.clone());
            if let Some(started_at) = started_at {
                self.active_subagent_started_at
                    .insert(session_id, started_at);
            }
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

    pub(crate) fn note_subagent_metadata(
        &mut self,
        session_id: &str,
        title: Option<String>,
        agent: Option<String>,
    ) -> bool {
        let Some((existing_title, existing_agent)) = self
            .side_panel
            .subagents()
            .iter()
            .find(|entry| entry.id == session_id)
            .map(|entry| (entry.title.clone(), entry.time_label.clone()))
        else {
            return false;
        };
        self.side_panel.upsert_subagent(
            session_id.to_string(),
            title.unwrap_or(existing_title),
            agent.unwrap_or(existing_agent),
        );
        true
    }

    pub(crate) fn set_task_message_status(&mut self, task_id: &str, status: &str) {
        if let Some(index) = task_message_index(&self.messages, task_id) {
            self.set_task_message_status_at(index, status);
        }
        for cached in self.session_cache.values_mut() {
            let Some(index) = task_message_index(&cached.messages, task_id) else {
                continue;
            };
            set_task_message_status_at(&mut cached.messages, index, status);
            cached.invalidate_timeline_layout();
        }
    }

    pub(crate) fn set_task_message_status_at(&mut self, index: usize, status: &str) {
        set_task_message_status_at(&mut self.messages, index, status);
        self.mark_timeline_message_and_next_dirty_at(index);
    }

    pub(crate) fn reconcile_parent_after_subagent_terminal(&mut self, child_id: &str) {
        let root_id = self
            .session_tree_root_id
            .clone()
            .or_else(|| {
                self.side_panel
                    .subagents()
                    .first()
                    .map(|entry| entry.id.clone())
            })
            .or_else(|| self.parent_session_id.clone());
        if let Some(root_id) = root_id.filter(|root_id| root_id != child_id) {
            if self.session_preloads_in_flight.contains(&root_id)
                || self
                    .session_preload_queue
                    .iter()
                    .any(|(session_id, _)| session_id == &root_id)
            {
                return;
            }
            self.ensure_session_preloaded(root_id, true);
        }
    }

    pub(crate) fn reconcile_task_message_statuses(&mut self) {
        let active_task_ids = self.active_subagent_ids.clone();
        // `set_subagents` has already reconciled the recovery snapshot with
        // newer live lifecycle edges and terminal locks. Mirror that
        // effective branch state into task cards/runtime bookkeeping instead
        // of replaying the raw snapshot and resurrecting a completed child.
        let mut reconciled_statuses = self
            .side_panel
            .subagents()
            .iter()
            .filter_map(|entry| {
                let status = self
                    .side_panel
                    .branch_activity(&entry.id)
                    .and_then(|activity| {
                        task_message_status_from_branch(activity.status).or_else(|| {
                            matches!(activity.status, BranchStatus::Active)
                                .then_some("running")
                        })
                    })
                    .or_else(|| {
                        entry
                            .runtime_status
                            .as_deref()
                            .and_then(task_message_status_from_runtime)
                    })?;
                Some((entry.id.clone(), status))
            })
            .collect::<HashMap<_, _>>();
        for task_id in &active_task_ids {
            reconciled_statuses
                .entry(task_id.clone())
                .or_insert_with(|| {
                    self.side_panel
                        .branch_activity(task_id)
                        .and_then(|activity| {
                            task_message_status_from_branch(activity.status).or_else(
                                || {
                                    matches!(activity.status, BranchStatus::Active)
                                        .then_some("running")
                                },
                            )
                        })
                        .unwrap_or("running")
                });
        }
        for (task_id, status) in &reconciled_statuses {
            self.note_subagent_runtime(
                task_id.clone(),
                branch_status_from_runtime(status),
                None,
            );
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
                let status = reconciled_statuses
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
            // This clock represents the *displayed waiting state*, not the
            // latest child part/tool. Part updates carry newer `started_at`
            // values, so replacing the clock on every refresh restarts both
            // the elapsed timer and the label animation while the same child
            // is still running. Latch it until the active-child count reaches
            // zero; a genuinely new waiting period will then start fresh.
            if self.subagent_waiting_started_at.is_none() {
                self.subagent_waiting_started_at = Some(
                    self.side_panel
                        .active_child_started_at(self.session_id.as_deref())
                        .map(instant_from_epoch_millis)
                        .unwrap_or_else(Instant::now),
                );
            }
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

    /// Settle the local activity label when the conversation currently being
    /// viewed is the child whose authoritative lifecycle just terminated.
    /// Parent sessions finish through `SessionIdle`; viewed children finish
    /// through `SubagentStatus` / `SubagentCompleted` on the root stream.
    pub(crate) fn reconcile_viewed_subagent_runtime(
        &mut self,
        session_id: &str,
        status: BranchStatus,
    ) -> bool {
        if self.session_id.as_deref() != Some(session_id)
            || !matches!(status, BranchStatus::Completed | BranchStatus::Stopped)
        {
            return false;
        }
        let had_activity = self.streaming_state != NeoismAgentStreamingState::Idle
            || self.streaming_started_at.is_some()
            || self.streaming_state_changed_at.is_some()
            || self.streaming_tool_label.is_some();
        self.note_streaming(NeoismAgentStreamingState::Idle, None);
        self.abort_requested_at = None;
        had_activity
    }

    /// A terminal child lifecycle is latched in the side panel. Continue to
    /// ingest any late transcript part, but do not let it resurrect the viewed
    /// child's Crafting/Tinkering label after completion.
    pub(crate) fn child_part_can_drive_streaming(&self, session_id: &str) -> bool {
        !self
            .side_panel
            .branch_activity(session_id)
            .is_some_and(|activity| activity.terminal_locked)
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
            self.status_timing.settle();
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
        // Mirrors `streaming_state` precedence so label scramble
        // animations restart on the same edges the displayed word does.
        if self.is_streaming() {
            return self
                .streaming_state_changed_at
                .map(|t| t.elapsed().as_secs_f32());
        }
        if self.active_subagent_count() > 0 {
            return self
                .subagent_waiting_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        if self.running_background_task_count() > 0 {
            return self
                .background_tasks_started_at
                .map(|started| started.elapsed().as_secs_f32());
        }
        self.streaming_state_changed_at
            .map(|t| t.elapsed().as_secs_f32())
    }
}

fn task_message_index(messages: &[NeoismAgentMessage], task_id: &str) -> Option<usize> {
    messages.iter().rposition(|message| {
        message.kind == NeoismAgentMessageKind::Tool
            && message.tool == "task"
            && (message.text.contains(task_id) || message.detail.contains(task_id))
    })
}

fn set_task_message_status_at(
    messages: &mut [NeoismAgentMessage],
    index: usize,
    status: &str,
) {
    let normalized = match status {
        "completed" | "error" | "running" => status,
        "stopped" => "error",
        _ => "running",
    };
    let Some(message) = messages.get_mut(index) else {
        return;
    };
    message.status = normalized.to_string();
    for field in [&mut message.text, &mut message.detail] {
        rewrite_task_status_markers(field, normalized);
    }
}

/// Desktop ↔ shared streaming-state conversion for the side panel's
/// status display hold (the hold stores the shared enum so one grace
/// mechanism serves both hosts). The two enums are variant-for-variant
/// identical.
fn shared_streaming_state(
    state: NeoismAgentStreamingState,
) -> neoism_ui::panels::agent_pane::state::NeoismAgentStreamingState {
    use neoism_ui::panels::agent_pane::state::NeoismAgentStreamingState as Shared;
    match state {
        NeoismAgentStreamingState::Idle => Shared::Idle,
        NeoismAgentStreamingState::Thinking => Shared::Thinking,
        NeoismAgentStreamingState::Working => Shared::Working,
        NeoismAgentStreamingState::Generating => Shared::Generating,
        NeoismAgentStreamingState::Compacting => Shared::Compacting,
        NeoismAgentStreamingState::WaitingSubagents => Shared::WaitingSubagents,
        NeoismAgentStreamingState::BackgroundTasks => Shared::BackgroundTasks,
        NeoismAgentStreamingState::Retrying => Shared::Retrying,
    }
}

fn desktop_streaming_state(
    state: neoism_ui::panels::agent_pane::state::NeoismAgentStreamingState,
) -> NeoismAgentStreamingState {
    use neoism_ui::panels::agent_pane::state::NeoismAgentStreamingState as Shared;
    match state {
        Shared::Idle => NeoismAgentStreamingState::Idle,
        Shared::Thinking => NeoismAgentStreamingState::Thinking,
        Shared::Working => NeoismAgentStreamingState::Working,
        Shared::Generating => NeoismAgentStreamingState::Generating,
        Shared::Compacting => NeoismAgentStreamingState::Compacting,
        Shared::WaitingSubagents => NeoismAgentStreamingState::WaitingSubagents,
        Shared::BackgroundTasks => NeoismAgentStreamingState::BackgroundTasks,
        Shared::Retrying => NeoismAgentStreamingState::Retrying,
    }
}
