use super::*;

impl NeoismAgentPane {
    pub fn maybe_request_older_timeline_page(
        &mut self,
        scroll_top: f32,
        viewport_h: f32,
    ) {
        const LOAD_OLDER_LIMIT: usize = 128;
        let threshold = (viewport_h * 0.75).max(720.0);
        if scroll_top > threshold
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

    /// Background refresh of the sub-agent / sibling-session list for
    /// the active session. Mirrors `maybe_refresh_side_panel_sessions`.
    pub fn maybe_refresh_side_panel_subagents(&mut self) {}

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn hydrate_runtime_status_for_session(&mut self, _session_id: &str) {
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
            return self.subagent_waiting_started_at.map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            });
        }
        if !self.is_streaming() && self.running_background_task_count() > 0 {
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
        self.side_panel
            .active_child_count(self.session_id.as_deref())
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
    pub(in crate::panels::agent_pane::state) fn reconcile_task_message_statuses(&mut self) {
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

    pub(in crate::panels::agent_pane::state) fn set_task_message_status_at(&mut self, index: usize, status: &str) {
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

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn suppress_streaming_after_abort(&self) -> bool {
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
    pub(in crate::panels::agent_pane::state) fn note_streaming_from_part(&mut self, kind: NeoismAgentMessageKind, title: &str) {
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
        if !self.is_streaming() && self.active_subagent_count() > 0 {
            return self.subagent_waiting_started_at.map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            });
        }
        if !self.is_streaming() && self.running_background_task_count() > 0 {
            return self.running_background_task_started_at().map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            });
        }
        if !self.has_status_activity() {
            return None;
        }
        self.streaming_state_changed_at
            .map(|t| Instant::now().saturating_duration_since(t).as_secs_f32())
    }

}
