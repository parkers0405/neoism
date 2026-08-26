use super::*;

impl NeoismAgentPane {
    pub fn maybe_request_older_timeline_page(
        &mut self,
        scroll_top: f32,
        viewport_h: f32,
    ) {
        // Keep history materialization frame-sized. Long agent turns are
        // allowed to span multiple pages instead of arriving as one huge UI
        // prepend merely to reach their user-message boundary.
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

    /// Kick off (debounced) a background refresh of the previous-session
    /// list shown in the side panel's home mode. Mirrors the file_tree
    /// git-status worker pattern: never blocks the frame; the worker
    /// pushes its result through `background_tx` and the next frame's
    /// `drain_background_updates` lifts it into `side_panel`.
    pub fn maybe_refresh_side_panel_sessions(&mut self) {}

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

    /// Whether the side-panel's selected home-mode session is the one
    /// already open. Used so picking the current (green-dotted) session
    /// from the "← Back" recent list returns to the live conversation.
    pub fn selected_side_panel_session_is_current(&self) -> bool {
        self.side_panel
            .selected_session()
            .map(|entry| Some(entry.id.as_str()) == self.session_id.as_deref())
            .unwrap_or(false)
    }

    /// Background refresh of the sub-agent / sibling-session list for
    /// the active session. Mirrors `maybe_refresh_side_panel_sessions`.
    pub fn maybe_refresh_side_panel_subagents(&mut self) {}

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn hydrate_runtime_status_for_session(
        &mut self,
        _session_id: &str,
    ) {
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
            || self.execution_activity.is_some()
            || self.active_subagent_count() > 0
            || self.running_background_task_count() > 0
            // The grace hold counts as activity so the status row's
            // reserved height (see `view/timeline/render.rs`) doesn't
            // collapse-and-return around a transient idle reading —
            // that one-frame reflow was the visible "bounce".
            || self.side_panel.held_status_display().is_some()
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
        if self.active_subagent_count() > 0 {
            return NeoismAgentStreamingState::WaitingSubagents;
        }
        if self.running_background_task_count() > 0 {
            return NeoismAgentStreamingState::BackgroundTasks;
        }
        self.streaming_state
    }

    /// Display status with hysteresis: a raw Idle reading only clears
    /// the label after idle has persisted for
    /// [`side_panel::STATUS_LABEL_GRACE`] — transient gaps between
    /// events (MessageEnd → next MessageStart, tool-phase handoffs, a
    /// child's inter-message idle edge) keep the last shown status so
    /// the label never blinks out mid-run. Recording happens here, on
    /// the render path, so the hold tracks what is actually displayed.
    pub fn streaming_state(&self) -> NeoismAgentStreamingState {
        let raw = self.raw_streaming_status();
        if raw != NeoismAgentStreamingState::Idle {
            self.side_panel
                .note_status_display(raw, self.raw_streaming_elapsed_seconds());
            return raw;
        }
        if let Some(held) = self.side_panel.held_status_display() {
            return held;
        }
        NeoismAgentStreamingState::Idle
    }

    pub fn streaming_label(&self) -> String {
        let state = self.streaming_state();
        if state == NeoismAgentStreamingState::Idle
            && self
                .execution_activity
                .as_ref()
                .is_some_and(|activity| activity.finished)
        {
            return "Completed".to_string();
        }
        if state == NeoismAgentStreamingState::Retrying {
            if let Some(reason) = self
                .streaming_tool_label
                .as_deref()
                .and_then(crate::panels::agent_pane::status_policy::compact_retry_reason)
            {
                return format!("Retrying · {reason}");
            }
        }
        // Other states stay intentionally terse; elapsed time is appended by
        // the renderer.
        state.label().to_string()
    }

    pub fn streaming_elapsed_seconds(&self) -> Option<f32> {
        if let Some(activity) = &self.execution_activity {
            let elapsed = self.execution_timer_anchor.as_ref().map_or_else(
                || activity.elapsed_ms_at(unix_millis()),
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

    pub fn apply_execution_activity(&mut self, mut incoming: ExecutionActivityState) -> bool {
        let now = Instant::now();
        let current_floor = self
            .execution_timer_anchor
            .as_ref()
            .and_then(|anchor| {
                anchor
                    .matches(&incoming.execution_id)
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
            self.execution_timer_anchor = Some(ExecutionTimerAnchor::from_snapshot(
                activity,
                unix_millis(),
                now,
                current_floor,
            ));
        }
        if replaced {
            self.active_subagent_ids.clear();
            self.active_subagent_started_at.clear();
            self.side_panel.reset_branch_lifecycle();
        }
        true
    }

    pub fn execution_activity_matches(&self, execution_id: &str, revision: u64) -> bool {
        self.execution_activity.as_ref().is_some_and(|current| {
            current.execution_id == execution_id && current.revision == revision
        })
    }

    pub fn apply_branch_lifecycle_snapshot(
        &mut self,
        root_session_id: String,
        revision: u64,
        branches: impl IntoIterator<Item = (String, String, Option<u64>)>,
    ) -> bool {
        if self.runtime_snapshot_root.as_deref() == Some(root_session_id.as_str())
            && revision <= self.runtime_snapshot_revision
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
        let branches = branches.into_iter().collect::<Vec<_>>();
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
            let status = match status.as_str() {
                "outstanding" => BranchStatus::Active,
                "completed" => BranchStatus::Completed,
                _ => BranchStatus::Stopped,
            };
            self.upsert_live_subagent_entry(&session_id, None, None);
            let status = self
                .side_panel
                .reconcile_branch_lifecycle_snapshot(session_id.clone(), status);
            self.side_panel
                .set_branch_activity_started_at(session_id.clone(), started_at);
            if matches!(status, BranchStatus::Active | BranchStatus::WaitingPermission) {
                self.active_subagent_ids.insert(session_id.clone());
                if let Some(started_at) = started_at {
                    self.active_subagent_started_at.insert(session_id, started_at);
                }
            } else {
                self.active_subagent_ids.remove(&session_id);
                self.active_subagent_started_at.remove(&session_id);
            }
        }
        self.sync_subagent_waiting_clock();
        true
    }

    fn raw_streaming_elapsed_seconds(&self) -> Option<f32> {
        // Clock precedence mirrors `raw_streaming_status`: the viewed
        // session's own run clock while it streams, then the aggregate
        // sub-agents / background-tasks clocks once it has stopped.
        if self.is_streaming() {
            return self.streaming_started_at.map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            });
        }
        if self.active_subagent_count() > 0 {
            return self.subagent_waiting_started_at.map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            });
        }
        if self.running_background_task_count() > 0 {
            return self.running_background_task_started_at().map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            });
        }
        self.streaming_started_at.map(|started| {
            Instant::now()
                .saturating_duration_since(started)
                .as_secs_f32()
        })
    }

    pub(in crate::panels::agent_pane::state) fn active_subagent_count(&self) -> usize {
        if self.is_subagent_session() {
            return 0;
        }
        let mut active_ids = self
            .side_panel
            .active_child_ids(self.session_id.as_deref());
        active_ids.extend(self.active_subagent_ids.iter().filter(|session_id| {
                Some(session_id.as_str()) != self.session_id.as_deref()
                    && !self.side_panel.branch_terminal_locked(session_id)
            }).cloned());
        active_ids.len()
    }

    pub fn note_subagent_runtime(
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

    pub fn settle_tracked_subagents(&mut self, status: BranchStatus) {
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

    /// Part-level activity update for a child (raw text/reasoning/tool
    /// delta). Subordinate to authoritative lifecycle status: if the
    /// branch already latched a terminal state it stays finished, and
    /// `active_subagent_ids` is *not* re-populated by a straggler delta.
    /// This is what stops a finished sub-agent from being dragged back to
    /// "responding"/"working". Returns whether the activity was applied.
    pub fn note_subagent_part_activity(
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
            // Branch already finished authoritatively — make sure our
            // live bookkeeping agrees so the waiting clock and child
            // counts don't keep treating it as in-flight.
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
        } else {
            self.active_subagent_ids.remove(&session_id);
            self.active_subagent_started_at.remove(&session_id);
        }
        true
    }

    /// Mirror a live streaming/idle edge from a NON-viewed session into
    /// the parent-keyed subagent roster. While a child transcript is
    /// open, the host's per-session event gate diverts sibling events
    /// into the background session cache; this keeps the sidebar's
    /// names / running dots / active count live anyway. Only sessions
    /// already tracked by the roster are touched, so unrelated
    /// background sessions can't pollute the family view. Returns
    /// whether the roster consumed the edge.
    pub fn note_family_session_streaming(
        &mut self,
        session_id: &str,
        active: bool,
    ) -> bool {
        if session_id.is_empty() || Some(session_id) == self.session_id.as_deref() {
            return false;
        }
        let tracked = self
            .side_panel
            .subagents()
            .iter()
            .any(|entry| entry.id == session_id);
        if !tracked {
            return false;
        }
        if active && self.side_panel.branch_terminal_locked(session_id) {
            // A straggler delta from a child that already finished
            // authoritatively must not resurrect its row.
            return false;
        }
        let status = if active {
            BranchStatus::Active
        } else {
            BranchStatus::Completed
        };
        self.note_subagent_runtime(session_id.to_string(), status, None);
        self.sync_subagent_waiting_clock();
        true
    }

    pub fn upsert_live_subagent_entry(
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

    pub fn set_task_message_status(&mut self, task_id: &str, status: &str) {
        let Some(index) = self.messages.iter().rposition(|message| {
            message.kind == NeoismAgentMessageKind::Tool
                && message.tool == "task"
                && (message.text.contains(task_id) || message.detail.contains(task_id))
        }) else {
            return;
        };
        self.set_task_message_status_at(index, status);
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn reconcile_task_message_statuses(
        &mut self,
    ) {
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
        let latched_statuses = active_task_ids
            .iter()
            .filter_map(|task_id| {
                self.side_panel
                    .branch_activity(task_id)
                    .map(|activity| (task_id.clone(), activity.status))
            })
            .collect::<Vec<_>>();
        for (task_id, status) in latched_statuses {
            self.note_subagent_runtime(task_id, status, None);
        }
        for (task_id, status) in &explicit_statuses {
            // The poll is authoritative even when the child was previously
            // active. Always mirror it into the runtime set so a completed
            // child is removed instead of lingering until another live event.
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

    pub(in crate::panels::agent_pane::state) fn set_task_message_status_at(
        &mut self,
        index: usize,
        status: &str,
    ) {
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

    pub(in crate::panels::agent_pane::state) fn sync_subagent_waiting_clock(&mut self) {
        if self.active_subagent_count() > 0 {
            // Keep one clock for one continuous "sub-agents working" state.
            // Child part/tool updates have their own newer `started_at`
            // values; adopting each one made the aggregate timer and label
            // animation visibly restart even though no state transition had
            // occurred.
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

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn suppress_streaming_after_abort(
        &self,
    ) -> bool {
        self.abort_requested_at.is_some_and(|requested| {
            Instant::now().saturating_duration_since(requested)
                <= ABORT_STREAM_SUPPRESSION
        })
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn refresh_streaming_from_tail(&mut self) {
        let Some(tail) = self.messages.last() else {
            return;
        };
        let kind = tail.kind;
        let title = tail.title.clone();
        self.note_streaming_from_part(kind, &title);
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn note_streaming_from_part(
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

    pub fn note_streaming(
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
        // Mirrors `streaming_state` precedence so label scramble
        // animations restart on the same edges the displayed word does.
        if self.is_streaming() {
            return self
                .streaming_state_changed_at
                .map(|t| Instant::now().saturating_duration_since(t).as_secs_f32());
        }
        if self.active_subagent_count() > 0 {
            return self.subagent_waiting_started_at.map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            });
        }
        if self.running_background_task_count() > 0 {
            return self.running_background_task_started_at().map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            });
        }
        None
    }
}
