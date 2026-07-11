use super::*;

impl NeoismAgentPane {
    pub fn drain_server_updates(&mut self) -> bool {
        let started = crate::neoism::agent::perf::now();
        let messages_before = self.messages.len();
        let mut drained_updates = 0usize;
        let mut delta_updates = 0usize;
        let mut delta_bytes = 0usize;
        let mut changed = self.drain_outbound_commands();
        changed |= self.drain_background_updates();
        let Some(event_stream) = self.event_stream.as_mut() else {
            return changed;
        };
        for update in event_stream.drain() {
            drained_updates += 1;
            match update {
                AgentSessionUpdate::Messages(messages) => {
                    let messages = self.compact_inbound_user_texts(messages);
                    let messages = self.merge_pending_user_prompts(messages);
                    let messages = self.preserve_streamed_response_text(messages);
                    // These full-transcript snapshots arrive repeatedly around
                    // each turn. `invalidate_timeline_layout()` here dropped the
                    // WHOLE layout cache, so the next frame re-measured and
                    // re-hashed every message in the transcript — an
                    // O(total history) rebuild that grew with every question and
                    // made scrolling heavy after a few Q&As. Instead, diff the
                    // snapshot: when the structure (message count) is unchanged,
                    // mark only the rows that actually differ dirty so the layout
                    // patches just those; fall back to a full invalidation only
                    // on a structural change (count differs / reorder).
                    let structural = self.messages.len() != messages.len();
                    let dirty_indices: Vec<usize> = if structural {
                        Vec::new()
                    } else {
                        self.messages
                            .iter()
                            .zip(messages.iter())
                            .enumerate()
                            .filter(|(_, (old, new))| old != new)
                            .map(|(index, _)| index)
                            .collect()
                    };
                    if structural || !dirty_indices.is_empty() {
                        self.messages = messages;
                        if structural {
                            self.invalidate_timeline_layout();
                        } else {
                            for index in dirty_indices {
                                self.mark_timeline_message_dirty_at(index);
                            }
                        }
                        self.clamp_timeline_scroll();
                        changed = true;
                    }
                    // Do not clear the streaming status from a message
                    // refresh alone. The event stream sends a separate
                    // SessionIdle update after the final idle status, which
                    // lets us finish the turn without treating ordinary
                    // history refreshes as completion.
                }
                AgentSessionUpdate::SessionIdle => {
                    if self.is_streaming() {
                        self.note_streaming(NeoismAgentStreamingState::Idle, None);
                        changed = true;
                    }
                    if self.queued_prompt_count != 0
                        || self.queued_prompt_preview.is_some()
                    {
                        self.queued_prompt_count = 0;
                        self.queued_prompt_preview = None;
                        changed = true;
                    }
                    self.abort_requested_at = None;
                }
                AgentSessionUpdate::System { title, body } => {
                    self.system_message(title, body);
                    changed = true;
                }
                AgentSessionUpdate::QueueStatus {
                    count,
                    preview,
                    started_at,
                } => {
                    let decision = status_policy::queue_status_decision(
                        count,
                        preview,
                        started_at,
                        self.is_streaming(),
                    );
                    if self.queued_prompt_count != decision.count
                        || self.queued_prompt_preview != decision.preview
                    {
                        self.queued_prompt_count = decision.count;
                        self.queued_prompt_preview = decision.preview;
                        changed = true;
                    }
                    if decision.should_enter_thinking {
                        self.note_streaming(NeoismAgentStreamingState::Thinking, None);
                        changed = true;
                    }
                    if let Some(started_at) = decision.started_at {
                        let started = instant_from_epoch_millis(started_at);
                        self.streaming_started_at = Some(started);
                        if self.streaming_state_changed_at.is_none() {
                            self.streaming_state_changed_at = Some(started);
                        }
                        changed = true;
                    }
                }
                AgentSessionUpdate::DequeuedPrompt { text } => {
                    changed |= self.insert_dequeued_user_prompt(text);
                }
                AgentSessionUpdate::SubagentStatus {
                    session_id,
                    status,
                    started_at,
                    title,
                    agent,
                } => {
                    self.upsert_live_subagent_entry(&session_id, title, agent);
                    let branch_status = branch_status_from_runtime(&status);
                    self.note_subagent_runtime(
                        session_id.clone(),
                        branch_status,
                        started_at,
                    );
                    if matches!(
                        branch_status,
                        BranchStatus::Active | BranchStatus::WaitingPermission
                    ) {
                        self.set_task_message_status(&session_id, "running");
                    } else {
                        self.set_task_message_status(&session_id, status.as_str());
                    }
                    self.sync_subagent_waiting_clock();
                    changed = true;
                }
                AgentSessionUpdate::SubagentActivity {
                    session_id,
                    status,
                    current_tool,
                    started_at,
                } => {
                    let branch_status = branch_status_from_runtime(&status);
                    // Part-level activity is subordinate to the child's
                    // authoritative lifecycle: a straggler "responding"
                    // delta that lands after the sub-agent has already
                    // finished must NOT resurrect the row. The guarded
                    // path drops the update when the branch is terminal.
                    let applied = self.note_subagent_part_activity(
                        session_id.clone(),
                        branch_status,
                        current_tool,
                        started_at,
                    );
                    if applied
                        && matches!(
                            branch_status,
                            BranchStatus::Active | BranchStatus::WaitingPermission
                        )
                    {
                        self.set_task_message_status(&session_id, "running");
                    }
                    self.sync_subagent_waiting_clock();
                    changed = true;
                }
                AgentSessionUpdate::SubagentCompleted {
                    task_id,
                    status,
                    title,
                    agent,
                } => {
                    if !task_id.is_empty() {
                        self.upsert_live_subagent_entry(&task_id, title, agent);
                        let branch_status = branch_status_from_runtime(&status);
                        self.note_subagent_runtime(task_id.clone(), branch_status, None);
                        self.set_task_message_status(&task_id, status.as_str());
                    }
                    self.sync_subagent_waiting_clock();
                    changed = true;
                }
                AgentSessionUpdate::PermissionAsked(permission) => {
                    self.enqueue_pending_permission(permission);
                    changed = true;
                }
                AgentSessionUpdate::PermissionReplied {
                    request_id,
                    session_id,
                } => {
                    if let Some(session_id) = session_id {
                        if Some(session_id.as_str()) != self.session_id.as_deref() {
                            self.side_panel.set_branch_activity_status(
                                session_id,
                                BranchStatus::Active,
                            );
                        }
                    }
                    if self.remove_pending_permission(&request_id) {
                        self.sync_subagent_waiting_clock();
                        changed = true;
                    }
                }
                AgentSessionUpdate::GoalUpdated { goal, version } => {
                    // SESSION_UPDATED carries the authoritative goal, so apply
                    // it live whether it was set, changed, paused, completed,
                    // blocked, or CLEARED (goal = None). The `version` lets the
                    // setter drop a slow `GET /goal` poll that raced this live
                    // event — without it the section flickered active → stale
                    // → active when a goal was set over a finished one.
                    self.side_panel.set_session_goal(goal, version);
                    changed = true;
                }
                AgentSessionUpdate::PartDelta {
                    message_id,
                    part_id,
                    kind,
                    delta,
                } => {
                    delta_updates += 1;
                    delta_bytes += delta.len();
                    self.apply_part_delta(message_id, part_id, kind, &delta);
                    if !self.suppress_streaming_after_abort() {
                        self.refresh_streaming_from_tail();
                    }
                    changed = true;
                }
                AgentSessionUpdate::PartUpdated(message) => {
                    let kind = message.kind;
                    let title = message.title.clone();
                    self.upsert_part_message(message);
                    if !self.suppress_streaming_after_abort() {
                        self.note_streaming_from_part(kind, &title);
                    }
                    changed = true;
                }
                AgentSessionUpdate::PartRemoved(part_id) => {
                    self.remove_part_message(&part_id);
                    changed = true;
                }
                AgentSessionUpdate::CompactionStarted { id, reason } => {
                    self.start_compaction_message(id, reason);
                    self.note_streaming(NeoismAgentStreamingState::Compacting, None);
                    changed = true;
                }
                AgentSessionUpdate::CompactionDelta { delta } => {
                    self.apply_compaction_delta(&delta);
                    changed = true;
                }
                AgentSessionUpdate::CompactionEnded { summary, kind } => {
                    self.finish_compaction_message(&summary, &kind);
                    if self.is_streaming() {
                        self.note_streaming(NeoismAgentStreamingState::Idle, None);
                    }
                    changed = true;
                }
            }
        }
        if self
            .event_stream
            .as_ref()
            .is_some_and(AgentSessionEventStream::is_disconnected)
        {
            self.event_stream = None;
        }
        if crate::neoism::agent::perf::enabled() && drained_updates > 0 {
            tracing::info!(
                target: "neoism::agent_ui_perf",
                drained_updates,
                delta_updates,
                delta_bytes,
                messages_before,
                messages_after = self.messages.len(),
                changed,
                elapsed_us = crate::neoism::agent::perf::elapsed_us(started),
                "agent event stream drained"
            );
        }
        changed
    }

    pub(crate) fn drain_outbound_commands(&mut self) -> bool {
        let mut changed = false;
        for command in self.drain_pending_outbound() {
            match command {
                OutboundAgentCommand::AbortSession => {
                    self.execute_abort_session_command();
                    changed = true;
                }
                OutboundAgentCommand::SwitchSession { session_id } => {
                    self.execute_switch_session_command(session_id);
                    changed = true;
                }
                OutboundAgentCommand::CompactSession => {
                    self.execute_compact_session_command();
                    changed = true;
                }
                OutboundAgentCommand::UndoSession => {
                    self.execute_undo_session_command();
                    changed = true;
                }
                OutboundAgentCommand::RedoSession => {
                    self.execute_redo_session_command();
                    changed = true;
                }
                OutboundAgentCommand::EnsureSession => {
                    if let Err(error) = self.execute_ensure_session_command() {
                        self.system_message("Session failed", error);
                    }
                    changed = true;
                }
                OutboundAgentCommand::SendPrompt {
                    text,
                    parts,
                    system,
                    agent,
                    model,
                    thinking,
                    transcript_echo,
                } => {
                    if let Err(error) = self.execute_send_prompt_command(
                        text,
                        parts,
                        system,
                        agent,
                        model,
                        thinking,
                        transcript_echo,
                    ) {
                        self.system_message("Prompt failed", error);
                        self.note_streaming(NeoismAgentStreamingState::Idle, None);
                    }
                    changed = true;
                }
                OutboundAgentCommand::ApplyConfigDefaults => {
                    self.execute_apply_config_defaults_command();
                    changed = true;
                }
                OutboundAgentCommand::RefreshModelContextLimit => {
                    self.execute_refresh_model_context_limit_command();
                    changed = true;
                }
                OutboundAgentCommand::RefreshSessions { .. } => {
                    self.open_sessions_picker();
                    changed = true;
                }
                OutboundAgentCommand::LoadOlderTimeline {
                    session_id,
                    before,
                    limit,
                } => {
                    self.execute_load_older_timeline_command(session_id, before, limit);
                    changed = true;
                }
                OutboundAgentCommand::RefreshModels => {
                    self.open_model_picker();
                    changed = true;
                }
                OutboundAgentCommand::RefreshAgents { .. } => {
                    self.open_agent_picker();
                    changed = true;
                }
                OutboundAgentCommand::RefreshSkills { .. } => {
                    self.open_skill_picker();
                    changed = true;
                }
                OutboundAgentCommand::ReplyPermission { id, reply } => {
                    self.execute_reply_permission_command(id, reply);
                    changed = true;
                }
                OutboundAgentCommand::SlashCommand { name, args } => {
                    self.execute_server_command(name, args);
                    changed = true;
                }
                OutboundAgentCommand::ApplyAgent { session_id, agent } => {
                    self.execute_apply_agent_command(session_id, agent);
                    changed = true;
                }
                OutboundAgentCommand::ApplyModel { session_id, model } => {
                    self.execute_apply_model_command(session_id, model);
                    changed = true;
                }
                OutboundAgentCommand::ApplyThinking {
                    session_id,
                    model,
                    thinking,
                } => {
                    self.execute_apply_thinking_command(session_id, model, thinking);
                    changed = true;
                }
                OutboundAgentCommand::ShowSkills { directory } => {
                    self.execute_show_skills_command(directory);
                    changed = true;
                }
                OutboundAgentCommand::ShowMcp { directory } => {
                    self.execute_show_mcp_command(directory);
                    changed = true;
                }
                OutboundAgentCommand::ShowPermissions { session_id } => {
                    self.execute_show_permissions_command(session_id);
                    changed = true;
                }
                OutboundAgentCommand::ShowQuestions { session_id } => {
                    self.execute_show_questions_command(session_id);
                    changed = true;
                }
                OutboundAgentCommand::HandleQueue { session_id, action } => {
                    self.execute_handle_queue_command(session_id, action);
                    changed = true;
                }
                OutboundAgentCommand::HandlePermit {
                    session_id,
                    reply,
                    id,
                } => {
                    self.execute_handle_permit_command(session_id, reply, id);
                    changed = true;
                }
                OutboundAgentCommand::HandleAnswer { session_id, answer } => {
                    self.execute_handle_answer_command(session_id, answer);
                    changed = true;
                }
                OutboundAgentCommand::HandleReject { session_id, id } => {
                    self.execute_handle_reject_command(session_id, id);
                    changed = true;
                }
                OutboundAgentCommand::SetTitle { session_id, title } => {
                    self.execute_set_title_command(session_id, title);
                    changed = true;
                }
                // The desktop pane drives `/connect` through its own
                // synchronous `connect.rs` methods (blocking HTTP +
                // background threads) and never enqueues these shared
                // connect commands; they exist for the web/wasm host. Kept
                // as no-ops so the exhaustive match stays complete.
                OutboundAgentCommand::RefreshConnectProviders { .. }
                | OutboundAgentCommand::ConnectStoreApiKey { .. }
                | OutboundAgentCommand::ConnectDisconnect { .. }
                | OutboundAgentCommand::ConnectOauthAuthorize { .. }
                | OutboundAgentCommand::ConnectOauthCallback { .. } => {}
            }
        }
        changed
    }

    pub(crate) fn execute_reply_permission_command(&mut self, id: String, reply: String) {
        let body = json!({ "reply": reply });
        match api_request_json(
            &self.server,
            "POST",
            &format!("/permission/{id}/reply"),
            Some(&body),
        ) {
            Ok(_) => {
                let reply = body["reply"].as_str().unwrap_or("");
                self.permission_reply_succeeded(&id, reply);
            }
            Err(error) => {
                self.permission_reply_failed(&id, error);
            }
        }
    }

    pub(crate) fn permission_reply_succeeded(&mut self, id: &str, reply: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        if self
            .pending_permission
            .as_ref()
            .is_some_and(|permission| permission.id == id)
        {
            self.clear_pending_permission_current();
            self.push_notice(
                format!("Permission: {id}: {reply}"),
                NeoismAgentNoticeLevel::Info,
            );
            return true;
        }
        self.remove_pending_permission(id)
    }

    pub(crate) fn permission_reply_failed(
        &mut self,
        id: &str,
        error: impl Into<String>,
    ) -> bool {
        let error = error.into();
        let changed = permission_policy::fail_reply(
            &mut self.pending_permission,
            id,
            |permission| permission.id.as_str(),
            |permission, responding| permission.responding = responding,
        );
        if changed {
            self.system_message("Permission", error);
        }
        changed
    }

    pub(crate) fn drain_background_updates(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.background_rx.try_recv() {
                Ok(NeoismAgentBackgroundUpdate::CompactFinished) => {
                    if self.is_streaming() {
                        self.note_streaming(NeoismAgentStreamingState::Idle, None);
                    }
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::CompactFailed(error)) => {
                    self.fail_compaction_message(error);
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::SidePanelSessionsRefreshed(sessions)) => {
                    self.side_panel.set_sessions(sessions);
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed(
                    subagents,
                )) => {
                    self.side_panel.set_subagents(subagents);
                    self.reconcile_task_message_statuses();
                    self.sync_subagent_waiting_clock();
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::SemanticSessionHits { query, hits }) => {
                    self.semantic_in_flight = false;
                    self.set_semantic_loading_indicators(false);
                    match hits {
                        None => self.semantic_unavailable = true,
                        Some(hits) => {
                            self.side_panel.set_semantic_results(
                                query.clone(),
                                hits.iter().map(|hit| hit.session_id.clone()).collect(),
                            );
                            self.apply_semantic_hits_to_session_picker(&query, &hits);
                        }
                    }
                    // A query typed while this fetch was in flight runs now.
                    if let Some(pending) = self.semantic_pending_query.take() {
                        if pending != query {
                            self.kick_semantic_session_search(pending);
                        }
                    }
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::SessionGoalRefreshed {
                    session_id,
                    goal,
                }) => {
                    // Drop a stale result that raced a session switch.
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        // Version a poll result by the goal's own `updated`
                        // millis; `None` (no goal found) is unversioned (0) so
                        // it can't clear a goal a live event just set.
                        let version = goal.as_ref().map(|goal| goal.updated).unwrap_or(0);
                        self.side_panel.set_session_goal(goal, version);
                        changed = true;
                    }
                }
                Ok(NeoismAgentBackgroundUpdate::OlderTimelineLoaded {
                    session_id,
                    messages,
                    raw_count,
                    requested_limit,
                }) => {
                    self.apply_older_timeline_page(
                        session_id,
                        messages,
                        raw_count,
                        requested_limit,
                    );
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::OlderTimelineFailed {
                    session_id,
                    error,
                }) => {
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        self.timeline_history.loading_older = false;
                        self.timeline_history.last_requested_session_id = None;
                        self.system_message("History", error);
                    }
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::SessionHistoryApplied {
                    session_id,
                    title,
                    messages,
                }) => {
                    // Ignore a revert that finished after the user switched away.
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        self.messages = messages;
                        self.invalidate_timeline_layout();
                        self.hydrate_runtime_status_for_session(&session_id);
                        self.start_session_updates(&session_id);
                        self.system_message(&title, "session history updated");
                    }
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::SessionHistoryFailed {
                    session_id,
                    title,
                    error,
                }) => {
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        self.system_message(&title, error);
                    }
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::ConnectOauthFinished {
                    provider_name,
                }) => {
                    self.system_message(
                        "Connected",
                        format!(
                            "{provider_name} connected. Open /model to pick one of its models."
                        ),
                    );
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::ConnectOauthFailed {
                    provider_name,
                    error,
                }) => {
                    self.system_message(
                        &provider_name,
                        format!("sign-in didn't complete: {error}"),
                    );
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        changed
    }

    pub(crate) fn push_notice(
        &mut self,
        message: impl Into<String>,
        level: NeoismAgentNoticeLevel,
    ) {
        let message = message.into();
        if message.trim().is_empty() {
            return;
        }
        self.ui_events
            .push(NeoismAgentUiEvent::Notice { message, level });
    }

    /// Surface a Neoism-style "Copied" notification — fires after a
    /// drag-to-select copy lands in the clipboard.
    pub fn push_copied_notice(&mut self, char_count: usize) {
        let message = if char_count == 1 {
            "Copied 1 char to clipboard".to_string()
        } else {
            format!("Copied {char_count} chars to clipboard")
        };
        self.push_notice(message, NeoismAgentNoticeLevel::Info);
    }

    pub(crate) fn push_dialog(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
    ) {
        let title = title.into();
        let body = body.into();
        if body.trim().is_empty() {
            return;
        }
        self.ui_events
            .push(NeoismAgentUiEvent::Dialog { title, body });
    }

    pub(crate) fn request_close_tab(&mut self) {
        self.ui_events.push(NeoismAgentUiEvent::CloseTab);
    }

    pub(crate) fn max_timeline_scroll(&self) -> f32 {
        (self.timeline_content_height_px - self.timeline_viewport_height_px).max(0.0)
    }

    pub(crate) fn clamp_timeline_scroll(&mut self) {
        self.timeline_scroll_px = self
            .timeline_scroll_px
            .clamp(0.0, self.max_timeline_scroll());
    }

    pub(crate) fn invalidate_timeline_layout(&mut self) {
        self.timeline_layout_epoch = self.timeline_layout_epoch.wrapping_add(1);
        self.timeline_dirty_message_ids.clear();
        self.timeline_dirty_message_indices.clear();
        // A full invalidation rebuilds every row, so any pending incremental
        // prepend fold is moot — drop it so it can't mis-target the new cache.
        self.pending_timeline_prepend_count = None;
        *self.timeline_layout_cache.borrow_mut() = None;
    }

    pub(crate) fn retain_current_turn_trace(&mut self) {
        if self.timeline_live_trace_start.is_some() {
            return;
        }
        self.timeline_live_trace_start = Some(
            self.messages
                .iter()
                .rposition(|message| message.kind == NeoismAgentMessageKind::User)
                .map_or(0, |index| index + 1),
        );
        self.invalidate_timeline_layout();
    }

    pub(crate) fn mark_timeline_message_dirty_at(&mut self, index: usize) {
        self.ensure_background_task_activity_clock();
        self.timeline_dirty_message_indices.insert(index);
    }

    pub(crate) fn mark_timeline_message_and_next_dirty_at(&mut self, index: usize) {
        self.ensure_background_task_activity_clock();
        self.timeline_dirty_message_indices.insert(index);
        self.timeline_dirty_message_indices
            .insert(index.saturating_add(1));
    }

    pub(crate) fn tool_expansion_is_animating(&self) -> bool {
        self.tool_expand_anims.values().any(|anim| anim.is_active())
    }

    pub(crate) fn apply_timeline_anchor(&mut self, anchor: TimelineAnchor) {
        let max_scroll = self.max_timeline_scroll();
        if max_scroll <= 0.0 {
            self.timeline_scroll_px = 0.0;
            self.timeline_velocity_px_s = 0.0;
            self.timeline_last_tick_at = None;
            return;
        }
        let viewport_y = self
            .timeline_viewport_rect
            .map(|rect| rect[1])
            .unwrap_or(0.0);
        let scroll_top =
            (anchor.content_y - (anchor.screen_y - viewport_y)).clamp(0.0, max_scroll);
        self.timeline_scroll_px = (max_scroll - scroll_top).clamp(0.0, max_scroll);
        self.timeline_velocity_px_s = 0.0;
        self.timeline_last_tick_at = None;
        self.timeline_last_scroll_at = Some(Instant::now());
    }

    pub(crate) fn start_session_updates(&mut self, session_id: &str) {
        let previous_session_id = self
            .event_stream
            .as_ref()
            .map(|stream| stream.session_id().to_string());
        if self.event_stream.as_ref().is_some_and(|stream| {
            stream.session_id() == session_id && !stream.is_disconnected()
        }) {
            if crate::neoism::agent::perf::enabled() {
                tracing::info!(
                    target: "neoism::agent_ui_perf",
                    session_id,
                    reused = true,
                    "agent event stream start"
                );
            }
            return;
        }
        self.event_stream = Some(start_session_event_stream(
            self.server.clone(),
            session_id.to_string(),
        ));
        if crate::neoism::agent::perf::enabled() {
            tracing::info!(
                target: "neoism::agent_ui_perf",
                previous_session_id = previous_session_id.as_deref(),
                session_id,
                reused = false,
                "agent event stream start"
            );
        }
    }

    pub(crate) fn fail_compaction_message(&mut self, error: impl Into<String>) {
        self.finish_compaction_message("", "failed");
        self.note_streaming(NeoismAgentStreamingState::Idle, None);
        self.system_message("Compaction failed", error.into());
    }

    pub(crate) fn remember_pending_user_prompt(&mut self, text: &str) {
        if !text.trim().is_empty() {
            self.pending_user_prompts.push(text.to_string());
        }
    }

    /// The compact composer form (`… [pasted 2 lines #3]`) for a prompt
    /// the server echoes back expanded. Canonicalizing every inbound
    /// user text through this keeps ONE transcript bubble instead of a
    /// token + expanded duplicate pair.
    pub(crate) fn compact_user_prompt_text(&self, text: &str) -> Option<String> {
        let trimmed = text.trim();
        self.prompt_echo_aliases
            .iter()
            .rev()
            .find(|(expanded, _)| expanded == trimmed)
            .map(|(_, echo)| echo.clone())
    }

    pub(crate) fn compact_inbound_user_texts(
        &self,
        mut messages: Vec<NeoismAgentMessage>,
    ) -> Vec<NeoismAgentMessage> {
        if self.prompt_echo_aliases.is_empty() {
            return messages;
        }
        for message in &mut messages {
            if message.kind == NeoismAgentMessageKind::User {
                if let Some(echo) = self.compact_user_prompt_text(&message.text) {
                    message.text = echo;
                }
            }
        }
        messages
    }

    pub(crate) fn clear_pending_user_prompts(&mut self) {
        self.pending_user_prompts.clear();
    }

    pub(crate) fn insert_dequeued_user_prompt(&mut self, text: String) -> bool {
        let text = self.compact_user_prompt_text(&text).unwrap_or(text);
        let text = text.trim().to_string();
        if text.is_empty() {
            return false;
        }
        let mut changed = self.consume_dequeued_prompt_preview(&text);
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
            return changed;
        }
        self.messages.push(NeoismAgentMessage::user(text));
        self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
        changed = true;
        changed
    }

    pub(crate) fn consume_dequeued_prompt_preview(&mut self, text: &str) -> bool {
        let mut changed = false;
        if self.queued_prompt_count > 0 {
            self.queued_prompt_count = self.queued_prompt_count.saturating_sub(1);
            changed = true;
        }
        let preview_matches = self
            .queued_prompt_preview
            .as_deref()
            .is_some_and(|preview| preview.trim() == text.trim());
        if self.queued_prompt_preview.is_some()
            && (self.queued_prompt_count == 0 || preview_matches)
        {
            self.queued_prompt_preview = None;
            changed = true;
        }
        changed
    }

    pub(crate) fn clear_composer(&mut self) {
        self.input.clear();
        self.cursor_byte = 0;
        self.input_attachments.clear();
        self.history_index = None;
        self.file_mention_anchor = None;
    }

    pub(crate) fn reset_session_runtime_ui(&mut self) {
        self.queued_prompt_count = 0;
        self.queued_prompt_preview = None;
        self.streaming_state = NeoismAgentStreamingState::Idle;
        self.streaming_started_at = None;
        self.streaming_state_changed_at = None;
        self.streaming_tool_label = None;
        self.subagent_waiting_started_at = None;
        self.active_subagent_ids.clear();
        self.active_subagent_started_at.clear();
        self.pending_permission = None;
        self.pending_permission_queue.clear();
        self.permission_choice_hit_rects.clear();
        self.timeline_live_trace_start = None;
    }

    pub(crate) fn merge_pending_user_prompts(
        &mut self,
        mut server_messages: Vec<NeoismAgentMessage>,
    ) -> Vec<NeoismAgentMessage> {
        if self.pending_user_prompts.is_empty() {
            return server_messages;
        }

        let previous_messages = self.messages.clone();
        let pending = std::mem::take(&mut self.pending_user_prompts);
        let mut unresolved = Vec::new();
        let mut inserts = Vec::new();

        for prompt in pending {
            if let Some(server_index) = server_messages
                .iter()
                .position(|message| is_user_prompt(message, &prompt))
            {
                if let Some(previous_index) = previous_messages
                    .iter()
                    .rposition(|message| is_user_prompt(message, &prompt))
                    .filter(|previous_index| *previous_index < server_index)
                {
                    let message = server_messages.remove(server_index);
                    server_messages
                        .insert(previous_index.min(server_messages.len()), message);
                }
                continue;
            }
            let previous_index = previous_messages
                .iter()
                .rposition(|message| is_user_prompt(message, &prompt))
                .unwrap_or(server_messages.len());
            inserts.push((previous_index, NeoismAgentMessage::user(prompt.clone())));
            unresolved.push(prompt);
        }

        inserts.sort_by_key(|(index, _)| *index);
        for (offset, (index, message)) in inserts.into_iter().enumerate() {
            server_messages.insert((index + offset).min(server_messages.len()), message);
        }
        self.pending_user_prompts = unresolved;
        server_messages
    }

    pub(crate) fn preserve_streamed_response_text(
        &self,
        mut server_messages: Vec<NeoismAgentMessage>,
    ) -> Vec<NeoismAgentMessage> {
        for incoming in &mut server_messages {
            if !is_streamed_live_part(incoming) {
                continue;
            }
            let Some(existing) = self
                .messages
                .iter()
                .find(|existing| same_streamed_part_identity(existing, incoming))
            else {
                continue;
            };
            *incoming = merge_part_message(existing.clone(), incoming.clone());
        }

        server_messages
    }

    pub(crate) fn apply_part_delta(
        &mut self,
        message_id: Option<String>,
        part_id: Option<String>,
        kind: Option<String>,
        delta: &str,
    ) {
        if delta.is_empty() {
            return;
        }
        if matches!(kind.as_deref(), Some("reasoning" | "thinking")) {
            self.retain_current_turn_trace();
        }
        if let Some(message_id) = message_id.as_deref().filter(|id| !id.is_empty()) {
            if let Some(index) = self
                .messages
                .iter()
                .position(|message| message.id == message_id)
            {
                self.messages[index].text.push_str(delta);
                self.mark_timeline_message_dirty_at(index);
                return;
            }
        }
        if let Some(part_id) = part_id.as_deref().filter(|id| !id.is_empty()) {
            if let Some(index) = self
                .messages
                .iter()
                .position(|message| message.id == part_id)
            {
                self.messages[index].text.push_str(delta);
                self.mark_timeline_message_dirty_at(index);
                return;
            }
            let message = match kind.as_deref() {
                Some("reasoning" | "thinking") => {
                    NeoismAgentMessage::reasoning(delta).with_id(part_id.to_string())
                }
                _ => NeoismAgentMessage::assistant(delta).with_id(part_id.to_string()),
            };
            self.upsert_part_message(message);
            return;
        }

        let message_kind = part_delta_message_kind(kind.as_deref());
        if let Some(index) = self
            .messages
            .iter()
            .rposition(|message| message.kind == message_kind)
        {
            self.messages[index].text.push_str(delta);
            self.mark_timeline_message_dirty_at(index);
            return;
        }

        self.messages.push(match message_kind {
            NeoismAgentMessageKind::Reasoning => NeoismAgentMessage::reasoning(delta),
            _ => NeoismAgentMessage::assistant(delta),
        });
        self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
    }

    pub(crate) fn start_compaction_message(&mut self, _id: String, reason: String) {
        let _ = reason;
    }

    pub(crate) fn apply_compaction_delta(&mut self, delta: &str) {
        let _ = delta;
    }

    pub(crate) fn finish_compaction_message(&mut self, summary: &str, kind: &str) {
        let _ = (summary, kind);
    }

    pub(crate) fn upsert_part_message(&mut self, mut message: NeoismAgentMessage) {
        if matches!(
            message.kind,
            NeoismAgentMessageKind::Reasoning
                | NeoismAgentMessageKind::Tool
                | NeoismAgentMessageKind::Subtask
                | NeoismAgentMessageKind::Compaction
        ) {
            self.retain_current_turn_trace();
        }
        if message.kind == NeoismAgentMessageKind::User {
            if let Some(echo) = self.compact_user_prompt_text(&message.text) {
                message.text = echo;
            }
            // Adopt the server's user part into the locally-echoed
            // bubble instead of appending a duplicate: the local echo
            // has no id yet, so match by (canonicalized) text.
            if !message.id.is_empty()
                && !self
                    .messages
                    .iter()
                    .any(|existing| existing.id == message.id)
            {
                if let Some(index) = self.messages.iter().rposition(|existing| {
                    existing.kind == NeoismAgentMessageKind::User
                        && existing.id.is_empty()
                        && existing.text.trim() == message.text.trim()
                }) {
                    let merged =
                        merge_part_message(self.messages[index].clone(), message);
                    self.messages[index] = merged;
                    self.mark_timeline_message_and_next_dirty_at(index);
                    return;
                }
            }
        }
        if message.kind == NeoismAgentMessageKind::Assistant
            && message.text.is_empty()
            && !message.id.is_empty()
            && self
                .messages
                .iter()
                .any(|existing| existing.id == message.id)
        {
            return;
        }
        if !message.id.is_empty() {
            if let Some(index) = self
                .messages
                .iter()
                .position(|existing| existing.id == message.id)
            {
                let merged = merge_part_message(self.messages[index].clone(), message);
                self.messages[index] = merged;
                if self.messages[index].kind == NeoismAgentMessageKind::Reasoning {
                    self.move_previous_assistant_after_reasoning(index);
                } else {
                    self.mark_timeline_message_and_next_dirty_at(index);
                }
                return;
            }
        }
        if message.kind == NeoismAgentMessageKind::Reasoning {
            if self
                .messages
                .iter()
                .any(|existing| existing.kind == NeoismAgentMessageKind::Assistant)
            {
                self.messages.push(message);
                self.move_previous_assistant_after_reasoning(
                    self.messages.len().saturating_sub(1),
                );
                self.invalidate_timeline_layout();
                return;
            }
        }
        self.messages.push(message);
        self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
    }

    /// See the shared `state.rs` twin for the full rationale. A reasoning
    /// part only pulls back an *empty* assistant placeholder (a provider
    /// that opens the turn with a blank text part before streaming its
    /// thinking). A non-empty assistant part is a completed/streaming
    /// answer; the stream is chronological, so it keeps its slot — moving
    /// it here is the "finished answer drops below a later thinking block"
    /// bug. Insertion order wins for finished text.
    pub(crate) fn move_previous_assistant_after_reasoning(&mut self, index: usize) {
        let turn_start = self.messages[..index]
            .iter()
            .rposition(|message| message.kind == NeoismAgentMessageKind::User)
            .map(|user_index| user_index + 1)
            .unwrap_or(0);
        let Some(assistant_index) = self.messages[turn_start..index]
            .iter()
            .rposition(|message| {
                message.kind == NeoismAgentMessageKind::Assistant
                    && message.text.is_empty()
            })
            .map(|relative_index| turn_start + relative_index)
        else {
            self.mark_timeline_message_and_next_dirty_at(index);
            return;
        };
        let assistant = self.messages.remove(assistant_index);
        let reasoning_index = index.saturating_sub(1);
        self.messages.insert(reasoning_index + 1, assistant);
        self.invalidate_timeline_layout();
    }

    pub(crate) fn remove_part_message(&mut self, part_id: &str) {
        if part_id.is_empty() {
            return;
        }
        let before = self.messages.len();
        self.messages.retain(|message| message.id != part_id);
        if self.messages.len() != before {
            self.invalidate_timeline_layout();
        }
    }
}
