use super::*;

impl NeoismAgentPane {
    pub fn drain_server_updates(&mut self) -> bool {
        self.drain_server_updates_inner(true)
    }

    pub fn drain_live_session_updates(&mut self) -> bool {
        self.drain_server_updates_inner(false)
    }

    fn drain_server_updates_inner(&mut self, include_outbound: bool) -> bool {
        const MAX_STREAM_UPDATES_PER_FRAME: usize = 512;
        let started = crate::neoism::agent::perf::now();
        let messages_before = self.messages.len();
        let mut drained_updates = 0usize;
        let mut delta_updates = 0usize;
        let mut delta_bytes = 0usize;
        let mut changed = include_outbound && self.drain_outbound_commands();
        changed |= self.drain_background_updates();
        self.tick_stream_liveness();
        let Some(event_stream) = self.event_stream.as_mut() else {
            return changed;
        };
        let stream_session_id = event_stream.session_id().to_string();
        let (updates, stream_has_more) = event_stream.drain(MAX_STREAM_UPDATES_PER_FRAME);
        if !updates.is_empty() {
            self.last_stream_update_at = Some(Instant::now());
        }
        let stream_is_active =
            self.session_id.as_deref() == Some(stream_session_id.as_str());
        for update in updates {
            drained_updates += 1;
            match update {
                AgentSessionUpdate::Messages {
                    mut messages,
                    oldest_cursor,
                } => {
                    if !stream_is_active {
                        let cached = self
                            .session_cache
                            .entry(stream_session_id.clone())
                            .or_insert_with(CachedAgentSession::live_only);
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
                        changed = true;
                        continue;
                    }
                    let messages = self.compact_inbound_user_texts(messages);
                    let messages = self.merge_pending_user_prompts(messages);
                    let mut messages = self.preserve_streamed_response_text(messages);
                    if self.timeline_history.oldest_loaded_cursor.is_some() {
                        messages =
                            merge_session_snapshot(messages, self.messages.clone());
                    }
                    // Landed background-task completion cards survive the
                    // snapshot replacement (runs last so the dedupe check
                    // sees the final candidate list).
                    let messages = self.preserve_background_completion_cards(messages);
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
                    let previous_len = self.messages.len();
                    let structural = previous_len != messages.len();
                    let stable_prefix = previous_len <= messages.len()
                        && stable_timeline_source_prefix(&self.messages, &messages);
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
                        self.rebase_current_turn_trace();
                        if structural
                            && !stable_prefix
                            && self.pending_timeline_prepend_count.is_none()
                        {
                            self.invalidate_timeline_layout();
                        } else {
                            if structural {
                                self.timeline_dirty_message_indices
                                    .insert(previous_len.saturating_sub(1));
                            }
                            for index in dirty_indices {
                                self.mark_timeline_message_dirty_at(index);
                            }
                        }
                        self.clamp_timeline_scroll();
                        changed = true;
                    }
                    if self.is_streaming() || self.background_tasks_started_at.is_some() {
                        self.ensure_background_task_activity_clock();
                    }
                    if self.timeline_history.oldest_loaded_cursor.is_none() {
                        self.timeline_history.oldest_loaded_cursor = oldest_cursor;
                    }
                    // Do not clear the streaming status from a message
                    // refresh alone. The event stream sends a separate
                    // SessionIdle update after the final idle status, which
                    // lets us finish the turn without treating ordinary
                    // history refreshes as completion.
                }
                AgentSessionUpdate::ChildMessages {
                    session_id,
                    mut messages,
                    oldest_cursor,
                } => {
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        messages = self.compact_inbound_user_texts(messages);
                        messages = self.merge_pending_user_prompts(messages);
                        messages = self.preserve_streamed_response_text(messages);
                        self.messages = merge_session_snapshot(
                            messages,
                            std::mem::take(&mut self.messages),
                        );
                        if self.timeline_history.oldest_loaded_cursor.is_none() {
                            self.timeline_history.oldest_loaded_cursor = oldest_cursor;
                        }
                        self.rebase_current_turn_trace();
                        self.invalidate_timeline_layout();
                    } else {
                        let cached = self
                            .session_cache
                            .entry(session_id)
                            .or_insert_with(CachedAgentSession::live_only);
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
                    }
                    changed = true;
                }
                AgentSessionUpdate::EventStreamReconnected => {
                    // Event-driven recovery: lifecycle is inferred from
                    // during a healthy stream. A reconnect edge requests one
                    // tree/status snapshot to cover anything missed while the
                    // transport was down; it does not start a timer loop.
                    self.side_panel.mark_subagent_tree_dirty();
                    changed = true;
                }
                AgentSessionUpdate::SessionIdle => {
                    self.note_session_runtime_event(&stream_session_id);
                    self.terminal_idle_sessions
                        .insert(stream_session_id.clone());
                    self.retry_reset_pending = false;
                    if stream_is_active {
                        let had_root_activity = self.streaming_state
                            != NeoismAgentStreamingState::Idle
                            || self.streaming_started_at.is_some()
                            || self.streaming_state_changed_at.is_some()
                            || self.streaming_tool_label.is_some();
                        self.note_streaming(NeoismAgentStreamingState::Idle, None);
                        self.abort_requested_at = None;
                        changed |= had_root_activity;
                    } else if let Some(cached) =
                        self.session_cache.get_mut(&stream_session_id)
                    {
                        cached
                            .runtime
                            .note_streaming(NeoismAgentStreamingState::Idle, None);
                    }
                }
                AgentSessionUpdate::System { title, body } => {
                    self.system_message(title, body);
                    changed = true;
                }
                AgentSessionUpdate::Retrying { attempt, message } => {
                    self.note_session_runtime_event(&stream_session_id);
                    self.terminal_idle_sessions.remove(&stream_session_id);
                    if !stream_is_active {
                        let cached = self
                            .session_cache
                            .entry(stream_session_id.clone())
                            .or_insert_with(CachedAgentSession::live_only);
                        let _ = attempt;
                        cached.runtime.note_streaming(
                            NeoismAgentStreamingState::Retrying,
                            message.filter(|message| !message.is_empty()),
                        );
                        changed = true;
                        continue;
                    }
                    // Recoverable provider error: the server is backing off and
                    // retrying the in-flight run. Show it inline where the
                    // "Thinking…" indicator lives so the run doesn't look
                    // stalled; the next part/idle event moves us back out.
                    let _ = attempt;
                    let reason = message.filter(|m| !m.is_empty());
                    self.retry_reset_pending = true;
                    self.note_streaming(NeoismAgentStreamingState::Retrying, reason);
                    changed = true;
                }
                AgentSessionUpdate::QueueStatus {
                    count,
                    preview,
                    started_at,
                } => {
                    self.note_session_runtime_event(&stream_session_id);
                    self.terminal_idle_sessions.remove(&stream_session_id);
                    if !stream_is_active {
                        self.session_cache
                            .entry(stream_session_id.clone())
                            .or_insert_with(CachedAgentSession::live_only)
                            .runtime
                            .apply_queue_status(count, preview, started_at);
                        changed = true;
                        continue;
                    }
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
                    self.note_session_runtime_event(&stream_session_id);
                    self.terminal_idle_sessions.remove(&stream_session_id);
                    if stream_is_active {
                        changed |= self.insert_dequeued_user_prompt(text);
                    } else {
                        let cached = self
                            .session_cache
                            .entry(stream_session_id.clone())
                            .or_insert_with(CachedAgentSession::live_only);
                        cached.runtime.consume_dequeued_prompt(&text);
                        if !text.trim().is_empty()
                            && !cached.messages.iter().any(|message| {
                                message.kind == NeoismAgentMessageKind::User
                                    && message.text.trim() == text.trim()
                            })
                        {
                            cached.messages.push(NeoismAgentMessage::user(text));
                            cached.invalidate_timeline_layout();
                        }
                        changed = true;
                    }
                }
                AgentSessionUpdate::ChildRunIdle { session_id } => {
                    self.note_session_run_idle(&session_id);
                    changed = true;
                }
                AgentSessionUpdate::SubagentStatus {
                    session_id,
                    status,
                    started_at,
                    title,
                    agent,
                } => {
                    // This lifecycle event is newer than any tree/status
                    // snapshot already in flight. Do not let that older snapshot
                    // overwrite the live child and make the sidebar/footer
                    // disappear for a frame.
                    self.side_panel.invalidate_inflight_subagent_refresh();
                    let refresh_completed = matches!(
                        branch_status_from_runtime(&status),
                        BranchStatus::Completed | BranchStatus::Stopped
                    );
                    self.ensure_session_preloaded(session_id.clone(), refresh_completed);
                    self.upsert_live_subagent_entry(&session_id, title, agent);
                    let branch_status = branch_status_from_runtime(&status);
                    self.note_session_branch_runtime(
                        &session_id,
                        branch_status,
                        started_at,
                    );
                    self.note_subagent_runtime(
                        session_id.clone(),
                        branch_status,
                        started_at,
                    );
                    self.reconcile_viewed_subagent_runtime(&session_id, branch_status);
                    if matches!(
                        branch_status,
                        BranchStatus::Active | BranchStatus::WaitingPermission
                    ) {
                        self.set_task_message_status(&session_id, "running");
                    } else {
                        self.set_task_message_status(&session_id, status.as_str());
                        self.reconcile_parent_after_subagent_terminal(&session_id);
                    }
                    self.sync_subagent_waiting_clock();
                    changed = true;
                }
                AgentSessionUpdate::SubagentMetadata {
                    session_id,
                    title,
                    agent,
                } => {
                    // Metadata is not a lifecycle edge. Update a row that is
                    // already tracked, but never recreate a pruned completed
                    // child or alter the aggregate working state.
                    let tracked = self
                        .side_panel
                        .subagents()
                        .iter()
                        .any(|entry| entry.id == session_id);
                    if tracked {
                        self.upsert_live_subagent_entry(&session_id, title, agent);
                        changed = true;
                    }
                }
                AgentSessionUpdate::SubagentActivity {
                    session_id,
                    status,
                    current_tool,
                    started_at,
                } => {
                    self.side_panel.invalidate_inflight_subagent_refresh();
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
                    if applied {
                        self.note_session_branch_runtime(
                            &session_id,
                            branch_status,
                            started_at,
                        );
                    }
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
                AgentSessionUpdate::BackgroundTaskCompleted {
                    session_id,
                    job_id,
                    status,
                } => {
                    self.note_session_runtime_event(&session_id);
                    let text = format!(
                        "job_id: {job_id}\nstatus: {status}\nbackground shell task finished"
                    );
                    let message = NeoismAgentMessage::tool(
                        "background_task_result",
                        text,
                        status,
                        "background_task_result",
                        NeoismAgentOutputKind::Text,
                        "text",
                        Vec::new(),
                    )
                    .with_id(format!("background-task-{job_id}"));
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        self.upsert_part_message(message);
                        self.ensure_background_task_activity_clock();
                    } else {
                        let cached = self
                            .session_cache
                            .entry(session_id)
                            .or_insert_with(CachedAgentSession::live_only);
                        upsert_cached_part_message(&mut cached.messages, message);
                        cached.runtime.running_background_task_count =
                            running_background_task_count(&cached.messages);
                        if cached.runtime.running_background_task_count == 0 {
                            cached.runtime.background_tasks_started_at = None;
                        }
                        cached.invalidate_timeline_layout();
                    }
                    changed = true;
                }
                AgentSessionUpdate::SubagentCompleted {
                    task_id,
                    status,
                    title,
                    agent,
                } => {
                    self.side_panel.invalidate_inflight_subagent_refresh();
                    if !task_id.is_empty() {
                        self.ensure_session_preloaded(task_id.clone(), true);
                        self.upsert_live_subagent_entry(&task_id, title, agent);
                        let branch_status = branch_status_from_runtime(&status);
                        self.note_session_branch_runtime(&task_id, branch_status, None);
                        self.note_subagent_runtime(task_id.clone(), branch_status, None);
                        self.reconcile_viewed_subagent_runtime(&task_id, branch_status);
                        self.set_task_message_status(&task_id, status.as_str());
                        self.reconcile_parent_after_subagent_terminal(&task_id);
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
                    if self.note_permission_replied(&request_id, session_id.as_deref()) {
                        changed = true;
                    }
                }
                AgentSessionUpdate::QuestionAsked(question) => {
                    self.enqueue_pending_question(question);
                    changed = true;
                }
                AgentSessionUpdate::QuestionRemoved { request_id } => {
                    if self.remove_pending_question(&request_id) {
                        changed = true;
                    }
                }
                AgentSessionUpdate::GoalUpdated { goal, version } => {
                    self.session_goal_cache
                        .insert(stream_session_id.clone(), (goal.clone(), version));
                    if !stream_is_active {
                        continue;
                    }
                    // SESSION_UPDATED carries the authoritative goal, so apply
                    // it live whether it was set, changed, paused, completed,
                    // blocked, or CLEARED (goal = None). The `version` lets the
                    // setter drop a slow `GET /goal` poll that raced this live
                    // event — without it the section flickered active → stale
                    // → active when a goal was set over a finished one.
                    self.side_panel.set_session_goal(goal, version);
                    changed = true;
                }
                AgentSessionUpdate::SessionMetadataUpdated {
                    agent,
                    model,
                    thinking,
                } => {
                    if !stream_is_active {
                        let cached = self
                            .session_cache
                            .entry(stream_session_id.clone())
                            .or_insert_with(CachedAgentSession::live_only);
                        if let Some(agent) = agent {
                            cached.state.agent = Some(agent);
                        }
                        if let Some(model) = model {
                            cached.state.model = Some(model);
                        }
                        if let Some(thinking) = thinking {
                            cached.state.thinking = thinking;
                        }
                        continue;
                    }
                    if let Some(agent) = agent {
                        self.agent = Some(agent);
                    }
                    if let Some(model) = model {
                        self.model = model;
                        self.execute_refresh_model_context_limit_command();
                    }
                    if let Some(thinking) = thinking {
                        self.thinking = thinking;
                    }
                    changed = true;
                }
                AgentSessionUpdate::ExecutionUpdated(snapshot) => {
                    if let Some(activity) = super::super::api::execution_activity_from_json(&snapshot)
                    {
                        if !stream_is_active
                            && !self.session_family_contains(&activity.root_session_id)
                        {
                            continue;
                        }
                        changed |= self.apply_execution_activity(activity);
                    }
                }
                AgentSessionUpdate::RuntimeUpdated(snapshot) => {
                    if let Ok(runtime) = super::super::api::family_runtime_from_json(
                        &snapshot,
                        &stream_session_id,
                    ) {
                        if !stream_is_active
                            && !self.session_family_contains(&runtime.root_session_id)
                        {
                            continue;
                        }
                        if let Some(activity) = runtime.execution {
                            self.apply_execution_activity(activity);
                        }
                        self.apply_branch_lifecycle_snapshot(
                            runtime.root_session_id,
                            runtime.family_revision,
                            runtime.branches,
                        );
                        changed = true;
                    }
                }
                AgentSessionUpdate::PartDelta {
                    message_id,
                    part_id,
                    kind,
                    delta,
                } => {
                    delta_updates += 1;
                    delta_bytes += delta.len();
                    self.note_session_runtime_event(&stream_session_id);
                    self.remember_live_part_parent(
                        part_id.as_deref().unwrap_or_default(),
                        message_id.as_deref(),
                    );
                    let reasoning_part_id =
                        matches!(kind.as_deref(), Some("reasoning" | "thinking"))
                            .then(|| part_id.clone())
                            .flatten();
                    if stream_is_active {
                        self.apply_part_delta(message_id, part_id, kind, &delta);
                        if !self.suppress_streaming_after_abort()
                            && !self.terminal_idle_sessions.contains(&stream_session_id)
                        {
                            self.refresh_streaming_from_tail();
                        }
                    } else {
                        let cached = self
                            .session_cache
                            .entry(stream_session_id.clone())
                            .or_insert_with(CachedAgentSession::live_only);
                        apply_cached_part_delta(
                            &mut cached.messages,
                            part_id.as_deref(),
                            kind.as_deref(),
                            &delta,
                        );
                        if let Some(reasoning_part_id) = reasoning_part_id.as_deref() {
                            normalize_cached_live_reasoning_order(
                                &mut cached.messages,
                                &self.live_part_parent_ids,
                                reasoning_part_id,
                            );
                        }
                        if !self.terminal_idle_sessions.contains(&stream_session_id) {
                            cached.runtime.refresh_streaming_from_tail(&cached.messages);
                        }
                        cached.invalidate_timeline_layout();
                    }
                    changed = true;
                }
                AgentSessionUpdate::PartUpdated {
                    message,
                    parent_message_id,
                } => {
                    self.note_session_runtime_event(&stream_session_id);
                    let part_id = message.id.clone();
                    let is_reasoning = message.kind == NeoismAgentMessageKind::Reasoning;
                    if stream_is_active {
                        let kind = message.kind;
                        let title = message.title.clone();
                        self.remember_live_part_parent(
                            &message.id,
                            parent_message_id.as_deref(),
                        );
                        self.upsert_part_message(message);
                        if !self.suppress_streaming_after_abort()
                            && !self.terminal_idle_sessions.contains(&stream_session_id)
                        {
                            self.note_streaming_from_part(kind, &title);
                        }
                    } else {
                        self.remember_live_part_parent(
                            &message.id,
                            parent_message_id.as_deref(),
                        );
                        let cached = self
                            .session_cache
                            .entry(stream_session_id.clone())
                            .or_insert_with(CachedAgentSession::live_only);
                        upsert_cached_part_message(&mut cached.messages, message);
                        if is_reasoning {
                            normalize_cached_live_reasoning_order(
                                &mut cached.messages,
                                &self.live_part_parent_ids,
                                &part_id,
                            );
                        }
                        if !self.terminal_idle_sessions.contains(&stream_session_id) {
                            cached.runtime.refresh_streaming_from_tail(&cached.messages);
                        }
                        cached.invalidate_timeline_layout();
                    }
                    changed = true;
                }
                AgentSessionUpdate::PartRemoved(part_id) => {
                    if stream_is_active {
                        self.remove_part_message(&part_id);
                    } else if let Some(cached) =
                        self.session_cache.get_mut(&stream_session_id)
                    {
                        cached.messages.retain(|message| message.id != part_id);
                        cached.invalidate_timeline_layout();
                    }
                    self.live_part_parent_ids.remove(&part_id);
                    changed = true;
                }
                AgentSessionUpdate::ChildPartDelta {
                    session_id,
                    message_id,
                    part_id,
                    kind,
                    delta,
                } => {
                    delta_updates += 1;
                    delta_bytes += delta.len();
                    self.note_session_runtime_event(&session_id);
                    let can_drive_streaming =
                        self.child_part_can_drive_streaming(&session_id);
                    self.remember_live_part_parent(
                        part_id.as_deref().unwrap_or_default(),
                        message_id.as_deref(),
                    );
                    let reasoning_part_id =
                        matches!(kind.as_deref(), Some("reasoning" | "thinking"))
                            .then(|| part_id.clone())
                            .flatten();
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        self.apply_part_delta(message_id, part_id, kind, &delta);
                        if can_drive_streaming && !self.suppress_streaming_after_abort() {
                            self.refresh_streaming_from_tail();
                        }
                    } else {
                        let cached = self
                            .session_cache
                            .entry(session_id)
                            .or_insert_with(CachedAgentSession::live_only);
                        apply_cached_part_delta(
                            &mut cached.messages,
                            part_id.as_deref(),
                            kind.as_deref(),
                            &delta,
                        );
                        if let Some(reasoning_part_id) = reasoning_part_id.as_deref() {
                            normalize_cached_live_reasoning_order(
                                &mut cached.messages,
                                &self.live_part_parent_ids,
                                reasoning_part_id,
                            );
                        }
                        if can_drive_streaming {
                            cached.runtime.refresh_streaming_from_tail(&cached.messages);
                        }
                        cached.invalidate_timeline_layout();
                    }
                    changed = true;
                }
                AgentSessionUpdate::ChildPartUpdated {
                    session_id,
                    message,
                    parent_message_id,
                } => {
                    self.note_session_runtime_event(&session_id);
                    let can_drive_streaming =
                        self.child_part_can_drive_streaming(&session_id);
                    let part_id = message.id.clone();
                    let is_reasoning = message.kind == NeoismAgentMessageKind::Reasoning;
                    self.remember_live_part_parent(
                        &message.id,
                        parent_message_id.as_deref(),
                    );
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        let kind = message.kind;
                        let title = message.title.clone();
                        self.upsert_part_message(message);
                        if can_drive_streaming && !self.suppress_streaming_after_abort() {
                            self.note_streaming_from_part(kind, &title);
                        }
                    } else {
                        let cached = self
                            .session_cache
                            .entry(session_id)
                            .or_insert_with(CachedAgentSession::live_only);
                        upsert_cached_part_message(&mut cached.messages, message);
                        if is_reasoning {
                            normalize_cached_live_reasoning_order(
                                &mut cached.messages,
                                &self.live_part_parent_ids,
                                &part_id,
                            );
                        }
                        if can_drive_streaming {
                            cached.runtime.refresh_streaming_from_tail(&cached.messages);
                        }
                        cached.invalidate_timeline_layout();
                    }
                    changed = true;
                }
                AgentSessionUpdate::ChildPartRemoved {
                    session_id,
                    part_id,
                } => {
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        self.remove_part_message(&part_id);
                    } else if let Some(cached) = self.session_cache.get_mut(&session_id) {
                        cached.messages.retain(|message| message.id != part_id);
                        cached.invalidate_timeline_layout();
                    }
                    self.live_part_parent_ids.remove(&part_id);
                    changed = true;
                }
                AgentSessionUpdate::CompactionStarted {
                    session_id,
                    id,
                    reason,
                } => {
                    self.note_session_runtime_event(&session_id);
                    if self.session_id.as_deref() != Some(session_id.as_str()) {
                        let _ = (id, reason);
                        self.session_cache
                            .entry(session_id)
                            .or_insert_with(CachedAgentSession::live_only)
                            .runtime
                            .note_streaming(NeoismAgentStreamingState::Compacting, None);
                        changed = true;
                        continue;
                    }
                    self.start_compaction_message(id, reason);
                    self.note_streaming(NeoismAgentStreamingState::Compacting, None);
                    changed = true;
                }
                AgentSessionUpdate::CompactionDelta { session_id, delta } => {
                    if self.session_id.as_deref() != Some(session_id.as_str()) {
                        let _ = delta;
                        continue;
                    }
                    self.apply_compaction_delta(&delta);
                    changed = true;
                }
                AgentSessionUpdate::CompactionEnded {
                    session_id,
                    summary,
                    kind,
                } => {
                    self.note_session_runtime_event(&session_id);
                    if self.session_id.as_deref() != Some(session_id.as_str()) {
                        let _ = (summary, kind);
                        self.session_cache
                            .entry(session_id)
                            .or_insert_with(CachedAgentSession::live_only)
                            .runtime
                            .note_streaming(NeoismAgentStreamingState::Idle, None);
                        changed = true;
                        continue;
                    }
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
        self.trim_session_cache();
        changed || stream_has_more
    }

    pub(crate) fn drain_outbound_commands(&mut self) -> bool {
        let mut changed = false;
        for command in self.drain_pending_outbound() {
            match command {
                OutboundAgentCommand::AbortSession => {
                    self.execute_abort_session_command();
                    changed = true;
                }
                OutboundAgentCommand::StopBackgroundTask { session_id, job_id } => {
                    self.execute_stop_background_task_command(session_id, job_id);
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
                    message_id,
                    text,
                    parts,
                    system,
                    agent,
                    model,
                    thinking,
                    delivery,
                    transcript_echo,
                } => {
                    self.queue_send_prompt_command(
                        message_id,
                        text,
                        parts,
                        system,
                        agent,
                        model,
                        thinking,
                        delivery,
                        transcript_echo,
                    );
                    changed = true;
                }
                OutboundAgentCommand::ApplyConfigDefaults => {
                    self.execute_apply_config_defaults_command();
                    changed = true;
                }
                OutboundAgentCommand::PersistConfigChoice { model, thinking } => {
                    // A joined workspace reads the host's config through the
                    // reverse proxy, but this desktop process can only write
                    // its own config file. Leave host-side persistence to the
                    // daemon protocol instead of writing the choice locally.
                    if self.server.trim_end_matches('/')
                        != neoism_agent_server().trim_end_matches('/')
                    {
                        continue;
                    }
                    let Ok(defaults) =
                        fetch_config_defaults(&self.server, self.directory.as_deref())
                    else {
                        continue;
                    };
                    for (key, value) in
                        [("agent.model", model), ("agent.variant", thinking)]
                    {
                        let Some(value) = value.filter(|value| !value.trim().is_empty())
                        else {
                            continue;
                        };
                        let already_set = match key {
                            "agent.model" => defaults.model.is_some(),
                            _ => defaults.thinking.is_some(),
                        };
                        if already_set {
                            continue;
                        }
                        if let Err(error) =
                            neoism_backend::config::write_setting_if_absent(
                                key,
                                Value::String(value),
                            )
                        {
                            tracing::warn!(
                                target: "neoism::config",
                                %error,
                                key,
                                "first-run agent preference write failed"
                            );
                        }
                    }
                    changed = true;
                }
                OutboundAgentCommand::SetInputHelpVisible { visible } => {
                    if let Err(error) = neoism_backend::config::write_setting(
                        "agent.input-hints",
                        Value::Bool(visible),
                    ) {
                        tracing::warn!(
                            target: "neoism::config",
                            %error,
                            "agent input hints preference write failed"
                        );
                    }
                    changed = true;
                }
                OutboundAgentCommand::SetSidebarVisible { visible } => {
                    if let Err(error) = neoism_backend::config::write_setting(
                        "agent.sidebar",
                        Value::Bool(visible),
                    ) {
                        tracing::warn!(
                            target: "neoism::config",
                            %error,
                            "agent sidebar preference write failed"
                        );
                    }
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
                OutboundAgentCommand::ReplyQuestion { id, answers } => {
                    self.execute_reply_question_command(id, answers);
                    changed = true;
                }
                OutboundAgentCommand::RejectQuestion { id } => {
                    self.execute_reject_question_command(id);
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
                OutboundAgentCommand::RefreshMcp { .. }
                | OutboundAgentCommand::McpOauthAuthorize { .. }
                | OutboundAgentCommand::McpSetEnabled { .. }
                | OutboundAgentCommand::McpConnect { .. }
                | OutboundAgentCommand::McpDisconnect { .. }
                | OutboundAgentCommand::McpRemoveAuth { .. } => {}
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
                // The desktop key bridge drives pin/delete through the
                // pane's synchronous methods (`toggle_selected_session_pin`
                // / `delete_selected_session` in pane/input.rs), which call
                // the agent-server directly — but the shared pane records
                // these commands, so honour them if they ever land here.
                OutboundAgentCommand::DeleteSession { session_id } => {
                    if let Err(error) = delete_session(&self.server, &session_id) {
                        self.system_message("Sessions", error);
                    } else {
                        if self.session_id.as_deref() == Some(session_id.as_str()) {
                            self.create_new_session();
                        }
                        self.refresh_sessions_after_mutation();
                    }
                    changed = true;
                }
                OutboundAgentCommand::SetSessionPinned { session_id, pinned } => {
                    if let Err(error) =
                        set_session_pinned(&self.server, &session_id, pinned)
                    {
                        self.system_message("Sessions", error);
                    } else {
                        self.refresh_sessions_after_mutation();
                    }
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
            &format!("/v2/interactions/permissions/{id}/reply"),
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

    pub(crate) fn execute_reply_question_command(
        &mut self,
        id: String,
        answers: Vec<Vec<String>>,
    ) {
        let body = json!({ "answers": answers });
        match api_request_json(
            &self.server,
            "POST",
            &format!("/v2/interactions/questions/{id}/reply"),
            Some(&body),
        ) {
            Ok(_) => {
                self.question_reply_succeeded(&id);
            }
            Err(error) => {
                self.question_reply_failed(&id, error);
            }
        }
    }

    pub(crate) fn execute_reject_question_command(&mut self, id: String) {
        match api_request_json(
            &self.server,
            "POST",
            &format!("/v2/interactions/questions/{id}/reject"),
            None,
        ) {
            Ok(_) => {
                self.question_reply_succeeded(&id);
            }
            Err(error) => {
                self.question_reply_failed(&id, error);
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
            // /yolo — the queue promotion made the next request
            // current; keep auto-answering until the queue drains.
            self.maybe_auto_respond_permission();
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
        const MAX_BACKGROUND_UPDATES_PER_FRAME: usize = 64;
        let mut changed = false;
        let mut remaining = MAX_BACKGROUND_UPDATES_PER_FRAME;
        loop {
            if remaining == 0 {
                return true;
            }
            remaining -= 1;
            match self.background_rx.try_recv() {
                Ok(NeoismAgentBackgroundUpdate::PromptDispatched {
                    origin_session_id,
                    origin_draft_id,
                    session_id,
                    transcript_echo,
                    event_stream,
                }) => {
                    self.prompt_dispatch_in_flight = false;
                    let is_active_origin = self.session_id == origin_session_id
                        && (origin_session_id.is_some()
                            || self.prompt_draft_id == origin_draft_id);
                    if origin_session_id.is_none() {
                        for pending in &mut self.pending_prompt_dispatches {
                            if pending.origin_session_id.is_none()
                                && pending.origin_draft_id == origin_draft_id
                            {
                                pending.origin_session_id = Some(session_id.clone());
                            }
                        }
                    }
                    if is_active_origin {
                        if origin_session_id.is_none() {
                            self.session_id = Some(session_id.clone());
                            self.parent_session_id = None;
                            self.session_tree_root_id = Some(session_id.clone());
                            self.side_panel
                                .set_viewed_session_id(Some(session_id.clone()));
                        }
                        if let Some(event_stream) = event_stream {
                            let mut event_stream = event_stream;
                            if let Some(wake) = self.event_wake.clone() {
                                event_stream.set_wake(wake);
                            }
                            self.event_stream = Some(event_stream);
                        } else {
                            self.start_session_updates(&session_id);
                        }
                        if let Some(echo) = transcript_echo {
                            self.remember_pending_user_prompt(&echo);
                        }
                    } else {
                        let cache_session_id = origin_session_id
                            .clone()
                            .unwrap_or_else(|| session_id.clone());
                        let cached = self
                            .session_cache
                            .entry(cache_session_id)
                            .or_insert_with(CachedAgentSession::live_only);
                        if origin_session_id.is_none() {
                            cached.state.parent_id = None;
                        }
                        if let Some(echo) = transcript_echo {
                            cached.pending_user_prompts.push(echo);
                        }
                        cached
                            .runtime
                            .note_streaming(NeoismAgentStreamingState::Generating, None);
                    }
                    self.start_next_prompt_dispatch();
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::PromptDispatchFailed {
                    origin_session_id,
                    origin_draft_id,
                    error,
                }) => {
                    self.prompt_dispatch_in_flight = false;
                    let is_active_origin = self.session_id == origin_session_id
                        && (origin_session_id.is_some()
                            || self.prompt_draft_id == origin_draft_id);
                    if is_active_origin {
                        self.system_message("Prompt failed", error);
                    } else if let Some(origin_session_id) = origin_session_id.as_ref() {
                        let cached = self
                            .session_cache
                            .entry(origin_session_id.clone())
                            .or_insert_with(CachedAgentSession::live_only);
                        cached
                            .runtime
                            .note_streaming(NeoismAgentStreamingState::Idle, None);
                        cached
                            .messages
                            .push(NeoismAgentMessage::system("Prompt failed", error));
                        cached.invalidate_timeline_layout();
                    }
                    self.start_next_prompt_dispatch();
                    if is_active_origin && !self.prompt_dispatch_in_flight {
                        self.note_streaming(NeoismAgentStreamingState::Idle, None);
                    }
                    changed = true;
                }
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
                Ok(NeoismAgentBackgroundUpdate::ConfigDefaultsLoaded(defaults)) => {
                    if let Some(agent) = defaults.agent {
                        match agent.as_str() {
                            "build" => self.mode = NeoismAgentMode::Build,
                            "plan" => self.mode = NeoismAgentMode::Plan,
                            _ => {}
                        }
                        self.agent = Some(agent);
                    }
                    if self.model.trim().is_empty() {
                        if let Some(model) = defaults.model {
                            self.model = model;
                        }
                    }
                    if self.thinking.is_none() {
                        self.thinking = defaults.thinking;
                    }
                    if let Some(visible) = defaults.input_help_visible {
                        self.set_input_help_visible(visible);
                    }
                    if let Some(visible) = defaults.sidebar_visible {
                        self.side_panel.set_user_hidden(!visible);
                    }
                    self.execute_refresh_model_context_limit_command();
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::ModelContextLimitRefreshed {
                    model,
                    limit,
                }) => {
                    if self.model == model {
                        self.model_context_limit = limit;
                        changed = true;
                    }
                }
                Ok(NeoismAgentBackgroundUpdate::SidePanelSessionsRefreshed(sessions)) => {
                    self.side_panel.set_sessions(sessions);
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed {
                    session_id,
                    generation,
                    result,
                }) => {
                    if !self.side_panel.complete_subagent_refresh(generation)
                        || !self.session_family_contains(&session_id)
                    {
                        continue;
                    }
                    let Ok(subagents) = result else {
                        // Preserve the last good sidebar snapshot and active
                        // count. Avoid a request on every render frame; the
                        // next lifecycle/session/reconnect edge will retry.
                        self.side_panel.settle_failed_subagent_refresh();
                        continue;
                    };
                    let root_id = subagents.first().map(|root| root.id.clone());
                    if let Some(root_id) = root_id.as_ref() {
                        self.session_tree_root_id = Some(root_id.clone());
                    }
                    let preload_ids = subagents
                        .iter()
                        .skip(1)
                        .map(|entry| entry.id.clone())
                        .collect::<Vec<_>>();
                    self.side_panel.set_subagents(subagents);
                    if let Some(stream) = self.event_stream.as_ref() {
                        stream.track_child_sessions(preload_ids.iter().cloned());
                    }
                    // A restored/nested child may initially be subscribed to
                    // itself or its immediate parent. Once the tree lookup
                    // resolves the actual root, move the one global stream to
                    // that root so every sibling/descendant keeps hydrating.
                    if self.is_subagent_session() {
                        if let Some(root_id) = root_id {
                            self.start_session_updates(&root_id);
                        }
                    }
                    for session_id in preload_ids {
                        self.ensure_session_preloaded(session_id, false);
                    }
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
                                hits.iter()
                                    .map(|hit| {
                                        neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSemanticMatch {
                                            session_id: hit.session_id.clone(),
                                            excerpt: hit.excerpt.clone(),
                                            distance: hit.distance,
                                        }
                                    })
                                    .collect(),
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
                        self.session_goal_cache
                            .insert(session_id.clone(), (goal.clone(), version));
                        self.side_panel.set_session_goal(goal, version);
                        changed = true;
                    }
                }
                Ok(NeoismAgentBackgroundUpdate::SessionPreloaded {
                    session_id,
                    state,
                    mut messages,
                    oldest_cursor,
                }) => {
                    self.session_preloads_in_flight.remove(&session_id);
                    let force_again =
                        self.session_preloads_force_pending.remove(&session_id);
                    let active = self.session_id.as_deref() == Some(session_id.as_str());
                    let mut cached = self
                        .session_cache
                        .remove(&session_id)
                        .unwrap_or_else(CachedAgentSession::live_only);
                    let mut cached_live = std::mem::take(&mut cached.messages);
                    reconcile_cached_pending_user_prompts(
                        &mut messages,
                        &mut cached_live,
                        &mut cached.pending_user_prompts,
                        &cached.prompt_echo_aliases,
                    );
                    if active {
                        messages = self.compact_inbound_user_texts(messages);
                        messages = self.merge_pending_user_prompts(messages);
                        messages = self.preserve_streamed_response_text(messages);
                    }
                    let live = if active {
                        let active_messages = std::mem::take(&mut self.messages);
                        if cached_live.is_empty() {
                            active_messages
                        } else {
                            merge_session_snapshot(active_messages, cached_live)
                        }
                    } else {
                        cached_live
                    };
                    let merged = merge_session_snapshot(messages, live);
                    let mut timeline_history =
                        std::mem::take(&mut cached.timeline_history);
                    timeline_history.oldest_loaded_cursor = oldest_cursor;
                    if active {
                        self.messages = merged;
                        self.timeline_history = timeline_history;
                        self.rebase_current_turn_trace();
                        self.invalidate_timeline_layout();
                    } else {
                        cached.state = state;
                        cached.messages = merged;
                        cached.timeline_history = timeline_history;
                        cached.hydrated = true;
                        cached.invalidate_timeline_layout();
                        self.session_cache.insert(session_id.clone(), cached);
                        self.trim_session_cache();
                    }
                    if self.pending_session_switch.as_deref() == Some(session_id.as_str())
                    {
                        self.activate_cached_session(&session_id);
                    }
                    if force_again {
                        self.ensure_session_preloaded(session_id, true);
                    }
                    self.start_queued_session_preloads();
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::SessionPreloadFailed {
                    session_id,
                    error,
                }) => {
                    self.session_preloads_in_flight.remove(&session_id);
                    self.session_preloads_force_pending.remove(&session_id);
                    if self.pending_session_switch.as_deref() == Some(session_id.as_str())
                    {
                        self.pending_session_switch = None;
                        self.system_message("Session", error);
                        changed = true;
                    }
                    self.start_queued_session_preloads();
                }
                Ok(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
                    session_id,
                    request_generation,
                    runtime_revision,
                    result,
                    runtime,
                }) => {
                    let is_latest = self
                        .runtime_status_requests
                        .get(&session_id)
                        .is_some_and(|generation| *generation == request_generation);
                    if is_latest {
                        self.runtime_status_requests.remove(&session_id);
                    }
                    if !is_latest
                        || self.session_id.as_deref() != Some(session_id.as_str())
                        || self.session_runtime_revision(&session_id) != runtime_revision
                    {
                        continue;
                    }
                    if let Ok(statuses) = result {
                        self.apply_runtime_status_for_session(&session_id, &statuses);
                        changed = true;
                    }
                    if let Ok(runtime) = runtime {
                        if let Some(activity) = runtime.execution {
                            self.apply_execution_activity(activity);
                        }
                        self.apply_branch_lifecycle_snapshot(
                            runtime.root_session_id,
                            runtime.family_revision,
                            runtime.branches,
                        );
                        changed = true;
                    }
                }
                Ok(NeoismAgentBackgroundUpdate::OlderTimelineLoaded {
                    session_id,
                    messages,
                    raw_count,
                    requested_limit,
                    oldest_cursor,
                    reached_start,
                }) => {
                    self.apply_older_timeline_page(
                        session_id,
                        messages,
                        raw_count,
                        requested_limit,
                        oldest_cursor,
                        reached_start,
                    );
                    changed = true;
                }
                Ok(NeoismAgentBackgroundUpdate::OlderTimelineFailed {
                    session_id,
                    error,
                }) => {
                    if self.session_id.as_deref() == Some(session_id.as_str()) {
                        self.timeline_history.loading_older = false;
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
        self.pending_timeline_prepend_height_px = None;
        self.pending_timeline_prepend_delta_px = None;
        *self.timeline_layout_cache.borrow_mut() = None;
    }

    pub(crate) fn retain_current_turn_trace(&mut self) {
        if self.timeline_live_trace_start.is_some() {
            return;
        }
        let last_user = self
            .messages
            .iter()
            .rposition(|message| message.kind == NeoismAgentMessageKind::User);
        // Anchor by the user message's id, not its index: the marker must
        // stay on this SAME turn for the whole visit even as the list is
        // replaced or older pages are prepended. Trace collapses only when
        // the session is left and re-entered, never because
        // a newer prompt was sent.
        self.timeline_live_trace_anchor =
            last_user.map(|index| self.messages[index].id.clone());
        self.timeline_live_trace_start = Some(last_user.map_or(0, |index| index + 1));
        self.invalidate_timeline_layout();
    }

    pub(crate) fn reveal_ongoing_session_trace(&mut self) {
        if !self.is_subagent_session() {
            self.retain_current_turn_trace();
            return;
        }
        // The running-child inspector retains every accumulated frame.
        // Do the same for an ongoing sub-agent, including tools from earlier
        // resumed turns. Completion leaves this stable for the current visit;
        // leaving and reopening the settled child restores the clean mask.
        if self.timeline_live_trace_start == Some(0) {
            return;
        }
        self.timeline_live_trace_anchor = None;
        self.timeline_live_trace_start = Some(0);
        self.invalidate_timeline_layout();
    }

    /// Re-derive the live-trace start index from the visit anchor after the
    /// message list was replaced or prepended. Does NOT move the boundary to
    /// the latest turn — earlier turns of this visit keep their trace.
    pub(crate) fn rebase_current_turn_trace(&mut self) {
        if self.timeline_live_trace_start.is_none() {
            return;
        }
        let derived = match self.timeline_live_trace_anchor.as_deref() {
            None => Some(0),
            // Optimistic prompts carry empty ids until the server echo lands;
            // an empty anchor is unfindable by design and falls through to
            // the re-anchor branch below, which picks up the durable id.
            Some("") => None,
            Some(anchor) => self
                .messages
                .iter()
                .position(|message| message.id == anchor)
                .map(|index| index + 1),
        };
        let index = match derived {
            Some(index) => index,
            // An OPTIMISTIC anchor (empty id) is unfindable by design: the
            // prompt has no durable id until the server echo lands. Re-anchor
            // at the latest turn to pick that id up.
            None if self.timeline_live_trace_anchor.as_deref() == Some("") => {
                let last_user = self
                    .messages
                    .iter()
                    .rposition(|message| message.kind == NeoismAgentMessageKind::User);
                self.timeline_live_trace_anchor =
                    last_user.map(|index| self.messages[index].id.clone());
                last_user.map_or(0, |index| index + 1)
            }
            // A DURABLE anchor that isn't in the list means the turn it
            // marked is older than everything currently loaded - the idle
            // refresh replaces the transcript with only the last page of
            // messages. Every row in view therefore belongs to that turn or
            // a later one, so the window opens at 0.
            //
            // This used to fall into the re-anchor branch above and jump the
            // boundary to the LAST user message, re-hiding trace rows that
            // were on screen a frame earlier - the "it goes away while I'm
            // looking at it" collapse. That also contradicted this method's
            // own contract (and `retain_current_turn_trace`'s): the trace
            // collapses when the session is left and re-entered, never
            // underneath a visit.
            None => 0,
        };
        self.timeline_live_trace_start = Some(index);
    }

    pub(crate) fn mark_timeline_message_dirty_at(&mut self, index: usize) {
        self.timeline_dirty_message_indices.insert(index);
    }

    pub(crate) fn mark_timeline_message_and_next_dirty_at(&mut self, index: usize) {
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

    /// Liveness watchdog, called every frame the pane drains. When the pane
    /// believes a run is active (the status pill is painted from a state we
    /// reached via events or polling) but the event stream has delivered
    /// NOTHING for the stall window, the stream is wedged in a way its own
    /// reader cannot see — force a resubscribe whose first connect performs
    /// the full reconnect reconciliation. This is the automated equivalent
    /// of closing and reopening the chat.
    pub(crate) fn tick_stream_liveness(&mut self) {
        const STREAM_STALL_AFTER: std::time::Duration =
            std::time::Duration::from_secs(40);
        const RESUBSCRIBE_MIN_INTERVAL: std::time::Duration =
            std::time::Duration::from_secs(60);
        if self.streaming_state == NeoismAgentStreamingState::Idle {
            return;
        }
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        if self.event_stream.is_none() {
            return;
        }
        // Nothing drained yet at all: measure from when streaming began.
        let last_signal = self
            .last_stream_update_at
            .or(self.streaming_state_changed_at)
            .or(self.streaming_started_at);
        let Some(last_signal) = last_signal else {
            return;
        };
        if last_signal.elapsed() < STREAM_STALL_AFTER {
            return;
        }
        if self
            .last_stream_resubscribe_at
            .is_some_and(|at| at.elapsed() < RESUBSCRIBE_MIN_INTERVAL)
        {
            return;
        }
        self.last_stream_resubscribe_at = Some(Instant::now());
        tracing::warn!(
            session_id = %session_id,
            "agent event stream stalled while a run is active; forcing resubscribe"
        );
        self.force_resubscribe_session_updates(&session_id);
    }

    /// Tear down the current event stream and start a fresh subscription
    /// whose first connect reconciles status + transcript like a reconnect.
    pub(crate) fn force_resubscribe_session_updates(&mut self, session_id: &str) {
        let known_child_session_ids = self.tracked_sessions_for_stream(session_id);
        self.event_stream = None;
        let mut event_stream =
            crate::neoism::agent::updates::start_session_event_stream_with_reconcile(
                self.server.clone(),
                session_id.to_string(),
                true,
            );
        if let Some(wake) = self.event_wake.clone() {
            event_stream.set_wake(wake);
        }
        event_stream.track_child_sessions(known_child_session_ids);
        self.event_stream = Some(event_stream);
    }

    pub(crate) fn start_session_updates(&mut self, session_id: &str) {
        let known_child_session_ids = self.tracked_sessions_for_stream(session_id);
        let previous_session_id = self
            .event_stream
            .as_ref()
            .map(|stream| stream.session_id().to_string());
        if self.event_stream.as_ref().is_some_and(|stream| {
            stream.session_id() == session_id && !stream.is_disconnected()
        }) {
            if let Some(stream) = self.event_stream.as_ref() {
                stream.track_child_sessions(known_child_session_ids);
            }
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
        let mut event_stream =
            start_session_event_stream(self.server.clone(), session_id.to_string());
        if let Some(wake) = self.event_wake.clone() {
            event_stream.set_wake(wake);
        }
        event_stream.track_child_sessions(known_child_session_ids);
        self.event_stream = Some(event_stream);
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

    /// Sessions whose events arrive over a family-root subscription. The
    /// roster is hydrated asynchronously, so the session currently on screen
    /// must be seeded explicitly when it differs from the subscribed root.
    /// Otherwise early child deltas remain in the decoder's unknown-session
    /// queue while root status events continue to animate the activity pill.
    pub(super) fn tracked_sessions_for_stream(
        &self,
        stream_session_id: &str,
    ) -> Vec<String> {
        let mut session_ids = self
            .side_panel
            .subagents()
            .iter()
            .map(|entry| entry.id.clone())
            .filter(|child_id| child_id != stream_session_id)
            .collect::<Vec<_>>();
        if let Some(viewed_session_id) = self
            .session_id
            .as_ref()
            .filter(|viewed_session_id| viewed_session_id.as_str() != stream_session_id)
        {
            if !session_ids.contains(viewed_session_id) {
                session_ids.push(viewed_session_id.clone());
            }
        }
        session_ids
    }

    pub(crate) fn set_event_wake(&mut self, wake: AgentEventWake) {
        self.event_wake = Some(wake.clone());
        if let Some(stream) = self.event_stream.as_mut() {
            stream.set_wake(wake);
        }
    }

    pub(crate) fn event_wake(&self) -> Option<AgentEventWake> {
        self.event_wake.clone()
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

    /// Keep the hidden background-completion sentinel through the short gap
    /// between the live completion event and the durable runtime prompt.
    /// Once the server snapshot contains its own copy, the server's position
    /// is authoritative. The completion event has no transcript anchor, so
    /// preserving its speculative live position can cross assistant/user
    /// turn boundaries and reorder the visible timeline.
    pub(crate) fn preserve_background_completion_cards(
        &self,
        mut server_messages: Vec<NeoismAgentMessage>,
    ) -> Vec<NeoismAgentMessage> {
        for (index, existing) in self.messages.iter().enumerate() {
            if !is_background_completion_card(existing) {
                continue;
            }
            let job_id = background_job_id_from_message(existing);
            let already_present = server_messages.iter().any(|incoming| {
                incoming.id == existing.id
                    || (job_id.is_some()
                        && background_completion_job_id_from_message(incoming) == job_id)
            });
            if already_present {
                continue;
            }
            let insert_at = self.messages[..index]
                .iter()
                .rev()
                .filter(|message| !message.id.is_empty())
                .find_map(|prior| {
                    server_messages
                        .iter()
                        .position(|incoming| incoming.id == prior.id)
                })
                .map(|position| position + 1)
                .unwrap_or(server_messages.len());
            server_messages.insert(insert_at, existing.clone());
        }
        server_messages
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
        self.restore_session_runtime_ui(CachedAgentRuntime::default());
        self.pending_permission = None;
        self.pending_permission_queue.clear();
        self.pending_question = None;
        self.pending_question_queue.clear();
        self.permission_choice_hit_rects.clear();
        self.timeline_live_trace_start = None;
        self.timeline_live_trace_anchor = None;
    }

    pub(crate) fn reset_timeline_navigation_for_session_switch(&mut self) {
        self.timeline_velocity_px_s = 0.0;
        self.timeline_last_tick_at = None;
        self.timeline_wheel_target_px = None;
        self.timeline_last_scroll_at = None;
        self.pending_timeline_anchor = None;
        self.timeline_view_anchor = None;
        self.pending_timeline_prepend_height_px = None;
        self.pending_timeline_prepend_delta_px = None;
        self.pending_timeline_prepend_count = None;
        self.scrollbar_drag = None;
        self.selection_anchor = None;
        self.selection_focus = None;
        self.timeline_live_trace_start = None;
        self.timeline_live_trace_anchor = None;
    }

    pub(crate) fn live_trace_for_cache(
        &self,
        preserve: bool,
    ) -> (Option<usize>, Option<String>) {
        if preserve {
            (
                self.timeline_live_trace_start,
                self.timeline_live_trace_anchor.clone(),
            )
        } else {
            (None, None)
        }
    }

    pub(crate) fn restore_cached_live_trace(
        &mut self,
        start: Option<usize>,
        anchor: Option<String>,
    ) {
        self.timeline_live_trace_start = start;
        self.timeline_live_trace_anchor = anchor;
    }

    pub(crate) fn trim_session_cache(&mut self) {
        const MAX_CACHED_SESSIONS: usize = 40;
        if self.session_cache.len() <= MAX_CACHED_SESSIONS {
            return;
        }
        let mut pinned = self.active_subagent_ids.clone();
        pinned.extend(self.session_preloads_in_flight.iter().cloned());
        pinned.extend(
            self.session_preload_queue
                .iter()
                .map(|(session_id, _)| session_id.clone()),
        );
        pinned.extend(self.pending_session_switch.iter().cloned());
        pinned.extend(self.session_tree_root_id.iter().cloned());
        pinned.extend(self.session_id.iter().cloned());
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
            self.runtime_hydrated_sessions.remove(&candidate);
            self.terminal_idle_sessions.remove(&candidate);
            self.session_runtime_revisions.remove(&candidate);
            self.runtime_status_requests.remove(&candidate);
            self.session_goal_cache.remove(&candidate);
        }
    }

    pub(crate) fn take_session_runtime_ui(&mut self) -> CachedAgentRuntime {
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
            abort_requested_at: self.abort_requested_at.take(),
        }
    }

    pub(crate) fn restore_session_runtime_ui(&mut self, runtime: CachedAgentRuntime) {
        self.queued_prompt_count = runtime.queued_prompt_count;
        self.queued_prompt_preview = runtime.queued_prompt_preview;
        self.streaming_state = runtime.streaming_state;
        self.streaming_started_at = runtime.streaming_started_at;
        self.streaming_state_changed_at = runtime.streaming_state_changed_at;
        self.streaming_tool_label = runtime.streaming_tool_label;
        self.subagent_waiting_started_at = runtime.subagent_waiting_started_at;
        self.background_tasks_started_at = runtime.background_tasks_started_at;
        self.running_background_task_count = runtime.running_background_task_count;
        self.abort_requested_at = runtime.abort_requested_at;
        self.permission_choice_hit_rects.clear();
        self.question_option_hit_rects.clear();
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
        let mut consumed_server = vec![false; server_messages.len()];
        let mut consumed_previous = vec![false; previous_messages.len()];

        for prompt in pending {
            if let Some(server_index) =
                server_messages
                    .iter()
                    .enumerate()
                    .position(|(index, message)| {
                        !consumed_server[index] && is_user_prompt(message, &prompt)
                    })
            {
                consumed_server[server_index] = true;
                if let Some(previous_index) = previous_messages
                    .iter()
                    .enumerate()
                    .position(|(index, message)| {
                        !consumed_previous[index] && is_user_prompt(message, &prompt)
                    })
                {
                    consumed_previous[previous_index] = true;
                }
                continue;
            }
            let previous_index =
                previous_messages
                    .iter()
                    .enumerate()
                    .position(|(index, message)| {
                        !consumed_previous[index] && is_user_prompt(message, &prompt)
                    });
            let message = previous_index
                .map(|index| {
                    consumed_previous[index] = true;
                    previous_messages[index].clone()
                })
                .unwrap_or_else(|| NeoismAgentMessage::user(prompt.clone()));
            inserts.push((previous_index.unwrap_or(server_messages.len()), message));
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
        if let (Some(part_id), Some(message_id)) = (
            part_id.as_deref().filter(|id| !id.is_empty()),
            message_id.as_deref().filter(|id| !id.is_empty()),
        ) {
            self.live_part_parent_ids
                .insert(part_id.to_string(), message_id.to_string());
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
                if self.messages[index].kind == NeoismAgentMessageKind::Reasoning {
                    self.move_previous_assistant_after_reasoning(index);
                }
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
                if self.messages[index].kind == NeoismAgentMessageKind::Reasoning {
                    self.move_previous_assistant_after_reasoning(index);
                }
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
        ) && !is_background_completion_card(&message)
        {
            // Open the live-trace window for real turn output, but NOT for
            // the background-task completion card. That card lands in an
            // already-settled session and is mask-exempt (it shows either
            // way), so revealing for it only un-hid the whole previous
            // turn's trace, which the next idle refresh then re-masked -
            // rows appearing and vanishing while the user watched.
            //
            // This must NOT be gated on `is_streaming()`: the web host's
            // `MessageUpdated` path calls `upsert_part_message` and nothing
            // else, so this is web's ONLY opener of the window. Desktop
            // additionally calls `note_streaming_from_part` right after.
            self.retain_current_turn_trace();
        }
        if message.kind == NeoismAgentMessageKind::User {
            if let Some(echo) = self.compact_user_prompt_text(&message.text) {
                message.text = echo;
            }
            // Text and image fragments are broadcast independently. Fold
            // both into the optimistic local card, including the image-first
            // ordering where a server-id row already exists by the time text
            // arrives.
            if !message.id.is_empty() {
                let optimistic_index = (!message.text.trim().is_empty())
                    .then(|| {
                        self.messages.iter().rposition(|existing| {
                            existing.kind == NeoismAgentMessageKind::User
                                && existing.id.is_empty()
                                && existing.text.trim() == message.text.trim()
                        })
                    })
                    .flatten();
                let server_index = self
                    .messages
                    .iter()
                    .position(|existing| existing.id == message.id);
                match (optimistic_index, server_index) {
                    (Some(optimistic_index), Some(server_index))
                        if optimistic_index != server_index =>
                    {
                        let server_fragment = self.messages.remove(server_index);
                        let optimistic_index = optimistic_index
                            .saturating_sub(usize::from(server_index < optimistic_index));
                        let merged = merge_part_message(
                            self.messages[optimistic_index].clone(),
                            server_fragment,
                        );
                        self.messages[optimistic_index] =
                            merge_part_message(merged, message);
                        self.rebase_current_turn_trace();
                        self.invalidate_timeline_layout();
                        return;
                    }
                    (Some(index), _) => {
                        self.messages[index] =
                            merge_part_message(self.messages[index].clone(), message);
                        self.mark_timeline_message_and_next_dirty_at(index);
                        return;
                    }
                    (None, Some(index)) => {
                        self.messages[index] =
                            merge_part_message(self.messages[index].clone(), message);
                        self.mark_timeline_message_and_next_dirty_at(index);
                        return;
                    }
                    (None, None) => {}
                }
            }
        }
        if message.kind == NeoismAgentMessageKind::Assistant
            && message.text.is_empty()
            && !message.id.is_empty()
        {
            if let Some(index) = self
                .messages
                .iter()
                .position(|existing| existing.id == message.id)
            {
                // A retry re-seeds this same part with empty text to wipe
                // the partial reply before re-streaming; honor the wipe so
                // the retried tokens don't append onto the partial. Outside
                // a retry, a late empty snapshot must never regress text
                // that already streamed.
                if self.retry_reset_pending {
                    self.retry_reset_pending = false;
                    self.messages[index].text.clear();
                    self.mark_timeline_message_dirty_at(index);
                }
                return;
            }
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
        // A runtime notification can be persisted and broadcast after the
        // provider has already started streaming the assistant response it
        // triggered. Both carry ascending canonical message ids, so put a
        // delayed row before any later message group instead of appending it
        // through the middle of that live assistant turn.
        if let Some(index) = chronological_live_insert_index(
            &self.messages,
            &self.live_part_parent_ids,
            &message,
        ) {
            let is_reasoning = message.kind == NeoismAgentMessageKind::Reasoning;
            self.messages.insert(index, message);
            if is_reasoning {
                self.move_previous_assistant_after_reasoning(index);
            } else {
                self.invalidate_timeline_layout();
            }
            return;
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

    /// Keep live order consistent with hydrated history. When SSE supplied
    /// the parent assistant-message ids, a delayed reasoning-end event moves
    /// only that same response's answer; unrelated/older answers remain
    /// chronological. Older event shapes without grouping retain the narrow
    /// empty-placeholder fallback.
    pub(crate) fn move_previous_assistant_after_reasoning(&mut self, index: usize) {
        let reasoning_id = self
            .messages
            .get(index)
            .map(|message| message.id.clone())
            .unwrap_or_default();
        if self.live_part_parent_ids.contains_key(&reasoning_id) {
            if move_grouped_assistant_after_reasoning(
                &mut self.messages,
                &self.live_part_parent_ids,
                &reasoning_id,
            ) {
                self.invalidate_timeline_layout();
            } else {
                self.mark_timeline_message_and_next_dirty_at(index);
            }
            return;
        }
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
        self.live_part_parent_ids.remove(part_id);
        if self.messages.len() != before {
            self.invalidate_timeline_layout();
        }
    }

    pub(crate) fn remember_live_part_parent(
        &mut self,
        part_id: &str,
        parent_id: Option<&str>,
    ) {
        if !part_id.is_empty() {
            if let Some(parent_id) = parent_id.filter(|id| !id.is_empty()) {
                self.live_part_parent_ids
                    .insert(part_id.to_string(), parent_id.to_string());
                if normalize_grouped_assistant_reasoning_order(
                    &mut self.messages,
                    &self.live_part_parent_ids,
                    parent_id,
                ) {
                    self.invalidate_timeline_layout();
                }
            }
        }
    }
}

pub(super) fn stable_timeline_source_prefix(
    previous: &[NeoismAgentMessage],
    incoming: &[NeoismAgentMessage],
) -> bool {
    previous.iter().zip(incoming).all(|(existing, next)| {
        if existing.id != next.id {
            return false;
        }
        if existing.id.is_empty() {
            return existing == next;
        }
        let previous_duplicates = previous
            .iter()
            .filter(|message| message.id == existing.id)
            .count();
        let incoming_duplicates = incoming
            .iter()
            .filter(|message| message.id == existing.id)
            .count();
        (previous_duplicates == 1 && incoming_duplicates == 1) || existing == next
    })
}

fn move_grouped_assistant_after_reasoning(
    messages: &mut Vec<NeoismAgentMessage>,
    parent_ids: &HashMap<String, String>,
    reasoning_id: &str,
) -> bool {
    let Some(parent_id) = parent_ids.get(reasoning_id) else {
        return false;
    };
    let Some(reasoning_index) = messages.iter().position(|message| {
        message.id == reasoning_id && message.kind == NeoismAgentMessageKind::Reasoning
    }) else {
        return false;
    };
    let turn_start = messages[..reasoning_index]
        .iter()
        .rposition(|message| message.kind == NeoismAgentMessageKind::User)
        .map(|index| index + 1)
        .unwrap_or(0);
    let Some(assistant_index) = messages[turn_start..reasoning_index]
        .iter()
        .rposition(|message| {
            message.kind == NeoismAgentMessageKind::Assistant
                && parent_ids.get(&message.id) == Some(parent_id)
        })
        .map(|index| turn_start + index)
    else {
        return false;
    };
    let assistant = messages.remove(assistant_index);
    let reasoning_index = messages
        .iter()
        .position(|message| message.id == reasoning_id)
        .unwrap_or_else(|| messages.len().saturating_sub(1));
    messages.insert(reasoning_index + 1, assistant);
    true
}

fn normalize_cached_live_reasoning_order(
    messages: &mut Vec<NeoismAgentMessage>,
    parent_ids: &HashMap<String, String>,
    reasoning_id: &str,
) {
    let _ = move_grouped_assistant_after_reasoning(messages, parent_ids, reasoning_id);
}

fn normalize_grouped_assistant_reasoning_order(
    messages: &mut Vec<NeoismAgentMessage>,
    parent_ids: &HashMap<String, String>,
    parent_id: &str,
) -> bool {
    let reasoning_ids = messages
        .iter()
        .filter(|message| {
            message.kind == NeoismAgentMessageKind::Reasoning
                && parent_ids.get(&message.id).is_some_and(|id| id == parent_id)
        })
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let mut changed = false;
    for reasoning_id in reasoning_ids {
        while move_grouped_assistant_after_reasoning(messages, parent_ids, &reasoning_id) {
            changed = true;
        }
    }
    changed
}

/// The durable background-task completion card (`api_mapping`'s
/// `background_completion_card`). It reports work that finished while the
/// user was elsewhere, is exempt from the timeline visibility mask, and
/// must not drag the whole settled turn back into view with it.
fn is_background_completion_card(message: &NeoismAgentMessage) -> bool {
    message.tool == "background_task_result" && message.id.starts_with("background-task-")
}
