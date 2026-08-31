use neoism_ui::Chrome;

pub(crate) fn is_neoism_agent_shortcut(event: &neoism_ui::event::UiEvent) -> bool {
    use neoism_ui::event::{KeyState, LogicalKey, Modifiers};

    let neoism_ui::event::UiEvent::Key(key) = event else {
        return false;
    };
    key.state == KeyState::Pressed
        && key.modifiers.contains(Modifiers::META)
        && !key
            .modifiers
            .intersects(Modifiers::SHIFT | Modifiers::CTRL | Modifiers::ALT)
        && matches!(&key.logical, LogicalKey::Character(ch) if ch.eq_ignore_ascii_case("a"))
}

pub(crate) fn is_enter_press(event: &neoism_ui::event::UiEvent) -> bool {
    use neoism_ui::event::{KeyState, LogicalKey, NamedKey};
    let neoism_ui::event::UiEvent::Key(key) = event else {
        return false;
    };
    key.state == KeyState::Pressed
        && matches!(&key.logical, LogicalKey::Named(NamedKey::Enter))
}

pub(crate) fn palette_enter_action(
    chrome: &Chrome<()>,
    event: &neoism_ui::event::UiEvent,
) -> Option<neoism_ui::panels::command_palette::PaletteAction> {
    use neoism_ui::event::{KeyState, LogicalKey, NamedKey};

    let neoism_ui::event::UiEvent::Key(key) = event else {
        return None;
    };
    if !chrome.command_palette.is_enabled()
        || key.state != KeyState::Pressed
        || !matches!(&key.logical, LogicalKey::Named(NamedKey::Enter))
    {
        return None;
    }
    chrome.command_palette.get_selected_action()
}

// ---------- base64 helper for `flush_pty_outbox` ----------------
//
// PTY response bytes round-trip through base64 so the JS side can
// stuff them straight into the WebSocket envelope. We keep the
// helper inline (zero new deps) — the alphabet is RFC 4648
// standard with `=` padding.

pub(crate) const B64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(B64_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(B64_ALPHABET[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_ALPHABET[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_ALPHABET[(b2 & 0b11_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub(crate) fn agent_bridge_key_from_web(
    key: &str,
) -> neoism_ui::panels::agent_pane::bridge_policy::AgentBridgeKey {
    use neoism_ui::panels::agent_pane::bridge_policy::{
        AgentBridgeKey, AgentBridgeNamedKey,
    };
    match key {
        "ArrowDown" => AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowDown),
        "ArrowLeft" => AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowLeft),
        "ArrowRight" => AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowRight),
        "ArrowUp" => AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowUp),
        "Backspace" => AgentBridgeKey::Named(AgentBridgeNamedKey::Backspace),
        "End" => AgentBridgeKey::Named(AgentBridgeNamedKey::End),
        "Enter" => AgentBridgeKey::Named(AgentBridgeNamedKey::Enter),
        "Escape" => AgentBridgeKey::Named(AgentBridgeNamedKey::Escape),
        "Home" => AgentBridgeKey::Named(AgentBridgeNamedKey::Home),
        "Insert" => AgentBridgeKey::Named(AgentBridgeNamedKey::Insert),
        "Paste" => AgentBridgeKey::Named(AgentBridgeNamedKey::Paste),
        "Tab" => AgentBridgeKey::Named(AgentBridgeNamedKey::Tab),
        "" => AgentBridgeKey::Other,
        value if value.chars().count() == 1 => {
            AgentBridgeKey::Character(value.to_string())
        }
        _ => AgentBridgeKey::Other,
    }
}

pub(crate) fn agent_bridge_physical_key_from_web(
    code: &str,
) -> Option<neoism_ui::panels::agent_pane::bridge_policy::AgentBridgePhysicalKey> {
    use neoism_ui::panels::agent_pane::bridge_policy::AgentBridgePhysicalKey;
    match code {
        "Insert" => Some(AgentBridgePhysicalKey::Insert),
        "KeyD" => Some(AgentBridgePhysicalKey::KeyD),
        "KeyU" => Some(AgentBridgePhysicalKey::KeyU),
        "KeyV" => Some(AgentBridgePhysicalKey::KeyV),
        _ => None,
    }
}

/// Dispatch a parsed `AgentServerMessage` into the right
/// `NeoismAgentPane` method. Mirrors the desktop pane's
/// `drain_server_updates` arm-by-arm so the web pane has parity.
pub(crate) fn apply_agent_event_to_pane(
    pane: &mut neoism_ui::panels::agent_pane::state::NeoismAgentPane,
    parsed: neoism_protocol::agent::AgentServerMessage,
) {
    use neoism_protocol::agent::{
        AgentServerMessage, CompactionPhase as ProtoCompactionPhase, ContentKind,
        NoticeLevel, Role, SubagentStatus, ToolStatus,
    };
    use neoism_ui::panels::agent_pane::state::side_panel::BranchStatus;
    use neoism_ui::panels::agent_pane::state::{
        CompactionPhase, NeoismAgentMessage, NeoismAgentMessageKind,
        NeoismAgentNoticeLevel, NeoismAgentOutputKind, NeoismAgentPendingPermission,
        NeoismAgentStreamingState, NeoismAgentTodo,
    };

    fn map_notice_level(l: NoticeLevel) -> NeoismAgentNoticeLevel {
        match l {
            NoticeLevel::Info => NeoismAgentNoticeLevel::Info,
            NoticeLevel::Warn => NeoismAgentNoticeLevel::Warn,
            NoticeLevel::Error => NeoismAgentNoticeLevel::Error,
        }
    }
    fn map_subagent_status(s: SubagentStatus) -> BranchStatus {
        match s {
            SubagentStatus::Running => BranchStatus::Active,
            SubagentStatus::Blocked => BranchStatus::WaitingPermission,
            SubagentStatus::Completed => BranchStatus::Completed,
            SubagentStatus::Failed => BranchStatus::Stopped,
        }
    }
    fn map_compaction_phase(p: ProtoCompactionPhase) -> CompactionPhase {
        match p {
            ProtoCompactionPhase::Started => CompactionPhase::Started,
            ProtoCompactionPhase::Delta => CompactionPhase::Delta,
            ProtoCompactionPhase::Ended => CompactionPhase::Ended,
        }
    }
    fn tool_status_label(s: ToolStatus) -> &'static str {
        match s {
            ToolStatus::Pending => "pending",
            ToolStatus::Running => "running",
            ToolStatus::Completed => "completed",
            ToolStatus::Failed => "error",
            ToolStatus::Cancelled => "stopped",
        }
    }

    match parsed {
        // -- Original direct-proxy surface ------------------------
        AgentServerMessage::Disabled { reason: _ } => {
            pane.note_streaming(NeoismAgentStreamingState::Idle, None);
        }
        AgentServerMessage::MessageStart {
            role, message_id, ..
        } => {
            pane.note_streaming(NeoismAgentStreamingState::Generating, None);
            let kind = match role {
                Role::User => NeoismAgentMessageKind::User,
                Role::System => NeoismAgentMessageKind::System,
                Role::Assistant => NeoismAgentMessageKind::Assistant,
            };
            let row = NeoismAgentMessage {
                id: message_id,
                kind,
                title: String::new(),
                text: String::new(),
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
            };
            pane.upsert_part_message(row);
        }
        AgentServerMessage::ContentDelta {
            message_id,
            kind,
            text,
            ..
        } => {
            let delta_kind = match kind {
                ContentKind::Text => Some("text".to_string()),
                ContentKind::Reasoning => Some("reasoning".to_string()),
                ContentKind::Tool { name } => Some(name),
            };
            pane.apply_part_delta(None, Some(message_id), delta_kind, &text);
        }
        AgentServerMessage::MessageEnd { .. } => {
            // SessionIdle is the authoritative idle signal; this
            // arm intentionally doesn't flip streaming off.
        }
        AgentServerMessage::PermissionRequest {
            request_id, tool, ..
        } => {
            let permission = NeoismAgentPendingPermission {
                id: format!("legacy-{request_id}"),
                session_id: pane
                    .session_id_str()
                    .map(str::to_string)
                    .unwrap_or_default(),
                parent_session_id: None,
                source_agent: None,
                source_title: None,
                title: format!("Permission requested: {tool}"),
                permission: tool,
                patterns: Vec::new(),
                selected: 0,
                responding: false,
            };
            pane.enqueue_pending_permission(permission);
        }
        AgentServerMessage::Error { message } => {
            pane.system_message("Agent error", message);
        }

        // -- Session lifecycle ------------------------------------
        AgentServerMessage::ThreadCreated {
            session_id,
            title,
            directory,
            agent,
            model,
        } => {
            pane.set_session_id(Some(session_id));
            if let Some(directory) = directory {
                pane.set_directory(Some(directory));
            }
            if let Some(agent) = agent {
                pane.apply_agent(agent);
            }
            if let Some(model) = model {
                pane.apply_model(model);
            }
            if let Some(title) = title {
                pane.system_message("Session", title);
            }
        }
        AgentServerMessage::ThreadSwitched { session_id } => {
            pane.set_session_id(Some(session_id));
        }
        AgentServerMessage::ThreadDeleted { session_id } => {
            pane.clear_session_id_if(&session_id);
        }
        AgentServerMessage::ThreadList {
            threads,
            requested_cursor,
            next_cursor,
        } => {
            let current_session_id = pane.session_id_str().map(str::to_string);
            let expected_cursor = pane
                .side_panel()
                .session_requested_cursor()
                .map(str::to_string);
            if requested_cursor.is_none() {
                pane.set_session_options(session_options_from_catalog(
                    &threads,
                    current_session_id.as_deref(),
                ));
            }
            if requested_cursor == expected_cursor {
                pane.side_panel_mut().set_session_page(
                    session_entries_from_catalog(&threads),
                    requested_cursor.as_deref(),
                    next_cursor,
                );
            }
        }
        AgentServerMessage::ThreadListFailed { message } => {
            tracing::warn!(%message, "failed to refresh agent sessions");
            pane.side_panel_mut()
                .settle_session_page_error("couldn't load sessions; retrying");
        }
        AgentServerMessage::HistoryChunk {
            session_id,
            messages,
            ..
        } => {
            pane.set_session_id(Some(session_id));
            let mapped: Vec<NeoismAgentMessage> =
                messages.into_iter().map(map_history).collect();
            // Hydrate the side panel's Subagents section from the
            // timeline's historical `task` tool cards BEFORE any live
            // `SubagentUpdate` arrives. Desktop refetches
            // `/session/:id/children` over HTTP on resume
            // (`maybe_refresh_side_panel_subagents`); the ws protocol has
            // no children-listing message, so the persisted task cards
            // are the web's recovery source. Live events keep overriding
            // these seeds (`preserve_specific_subagent_metadata`).
            let seeds = subagent_seeds_from_history(&mapped);
            pane.apply_history(mapped);
            for (task_id, status, title) in seeds {
                pane.note_subagent_event(task_id, status, title, None, None, None);
            }
        }
        AgentServerMessage::SessionEvent { .. } => {
            // Typed variants below cover the chrome's needs; the
            // raw envelope is reserved for forward-compatible events
            // the daemon proxies through without a typed match.
        }
        AgentServerMessage::MessageUpdated { message, .. } => {
            pane.upsert_part_message(map_history(message));
        }
        AgentServerMessage::PartRemoved { part_id, .. } => {
            pane.remove_part_message(&part_id);
        }
        AgentServerMessage::SessionIdle { .. } => {
            pane.note_session_idle();
        }
        AgentServerMessage::RuntimeSnapshot { snapshot, .. } => {
            if !pane.session_family_contains(&snapshot.root_session_id) {
                return;
            }
            if let (Some(epoch), Some(revision), Some(tasks)) = (
                snapshot.background_jobs_epoch.as_deref(),
                snapshot.background_jobs_revision,
                snapshot.running_background_tasks.as_ref(),
            ) {
                let tasks = tasks
                    .iter()
                    .map(|task| (task.job_id.clone(), task.started_at))
                    .collect::<Vec<_>>();
                pane.apply_running_background_tasks(epoch, revision, &tasks);
            }
            let execution = snapshot.execution.map(|activity| {
                let activity = neoism_ui::panels::agent_pane::state::ExecutionActivityState {
                        execution_id: activity.execution_id,
                        root_session_id: activity.root_session_id,
                        completed_ms: activity.completed_ms,
                        active_segments: activity.active_segments,
                        session_activities: activity.session_activities.into_iter().map(|(session_id, activity)| {
                            (session_id, neoism_ui::panels::agent_pane::state::ProviderActivityState {
                                completed_ms: activity.completed_ms,
                                active_segments: activity.active_segments,
                            })
                        }).collect(),
                        revision: activity.revision,
                        finished: activity.finished,
                    };
                activity
            });
            if snapshot.branches_authoritative {
                pane.apply_runtime_lifecycle_snapshot(
                    execution,
                    snapshot.root_session_id.clone(),
                    snapshot.family_revision,
                    snapshot.branches.into_iter().map(|branch| {
                        (branch.session_id, branch.status, branch.started_at)
                    }),
                );
            } else if let Some(execution) = execution {
                pane.apply_execution_activity(execution);
            }
        }
        AgentServerMessage::BackgroundTasksUpdated {
            epoch,
            revision,
            tasks,
            ..
        } => {
            let tasks = tasks
                .into_iter()
                .map(|task| (task.job_id, task.started_at))
                .collect::<Vec<_>>();
            pane.apply_running_background_tasks(&epoch, revision, &tasks);
        }
        AgentServerMessage::StreamingState { state, label, .. } => {
            pane.note_streaming(map_streaming(state), label);
        }
        AgentServerMessage::Notice {
            title, body, level, ..
        } => {
            pane.push_notice_event(title, body, map_notice_level(level));
        }
        AgentServerMessage::CommandOutput { title, body, .. } => {
            if title == "Directory" {
                if let Some(directory) = body.strip_prefix("Switched location to ") {
                    pane.set_directory(Some(directory.to_string()));
                    pane.location_message(body);
                } else {
                    pane.system_message(title, body);
                }
            } else {
                pane.system_message(title, body);
            }
        }

        // -- Tool / permission gating ----------------------------
        AgentServerMessage::ToolUseRequest {
            request_id,
            session_id,
            tool,
            title,
            patterns,
            args,
            source_agent,
        } => {
            let detail = serde_json::to_string(&args).unwrap_or_default();
            pane.upsert_tool_card(
                request_id.clone(),
                tool.clone(),
                title.clone(),
                "pending".to_string(),
                detail,
                NeoismAgentOutputKind::Code,
                String::new(),
            );
            let permission = NeoismAgentPendingPermission {
                id: request_id,
                session_id,
                parent_session_id: None,
                source_agent,
                source_title: Some(title.clone()),
                title,
                permission: tool,
                patterns,
                selected: 0,
                responding: false,
            };
            pane.enqueue_pending_permission(permission);
        }
        AgentServerMessage::PermissionRemoved {
            request_id,
            session_id,
        } => {
            pane.note_permission_replied(&request_id, Some(&session_id));
        }
        AgentServerMessage::PermissionReplyFailed { request_id, error } => {
            pane.permission_reply_failed(&request_id, error);
        }
        AgentServerMessage::ToolUseResult {
            tool_use_id,
            session_id: _,
            status,
            output,
            error,
            ..
        } => {
            pane.finalize_tool_card(
                &tool_use_id,
                tool_status_label(status),
                output,
                error,
            );
        }

        // -- Structured questions (the `question` tool) ----------
        AgentServerMessage::QuestionAsked {
            session_id,
            request,
        } => {
            enqueue_question_request(pane, &session_id, &request);
        }
        AgentServerMessage::QuestionsUpdated {
            session_id,
            requests,
        } => {
            // Recovery snapshot (stream re-attach / `/questions`):
            // enqueue-only — `enqueue_pending_question` dedupes by id,
            // and an empty snapshot must not clear a question that was
            // asked a beat ago (the removal path is `QuestionRemoved`).
            for request in &requests {
                enqueue_question_request(pane, &session_id, request);
            }
        }
        AgentServerMessage::QuestionRemoved { request_id, .. } => {
            // Same handling desktop gives `question.replied` /
            // `question.rejected`: clear it whether it's the current
            // prompt (our own reply's ack) or queued (answered from
            // another device).
            pane.question_reply_succeeded(&request_id);
        }
        AgentServerMessage::QuestionReplyFailed { request_id, error } => {
            pane.question_reply_failed(&request_id, error);
        }

        // -- Session metadata acks -------------------------------
        AgentServerMessage::ThreadUpdated { .. } => {
            // Rename / pin ack. The bridge already fired a
            // `ListThreads` refresh from
            // `note_agent_catalog_side_effects`; the resulting
            // `ThreadList` updates the picker + side panel, so there
            // is no pane state to touch here.
        }

        // -- Edit proposals --------------------------------------
        AgentServerMessage::EditProposed {
            edit_id,
            path,
            patch,
            tool,
            ..
        } => {
            pane.record_edit_proposed(edit_id, path, patch, tool);
        }
        AgentServerMessage::EditApplied {
            edit_id,
            bytes_written,
            ..
        } => {
            pane.record_edit_applied(&edit_id, bytes_written);
        }
        AgentServerMessage::EditRejected {
            edit_id, reason, ..
        } => {
            pane.record_edit_rejected(&edit_id, reason);
        }

        // -- Provider / model / agent state ----------------------
        AgentServerMessage::ProviderState {
            provider_id,
            model,
            connection_id,
            agent,
            thinking,
            context_limit,
            ..
        } => {
            pane.set_connection_id(connection_id);
            pane.apply_provider_state(provider_id, model, agent, thinking, context_limit);
        }
        AgentServerMessage::ProviderCatalog { providers } => {
            pane.set_model_context_limits(model_context_limits_from_catalog(&providers));
            pane.set_model_options(model_options_from_catalog(&providers));
        }
        AgentServerMessage::ConfigDefaults {
            agent,
            model,
            thinking,
            input_help_visible,
            sidebar_visible,
        } => {
            pane.apply_provider_state(None, model, agent, thinking, None);
            if let Some(visible) = input_help_visible {
                pane.set_input_help_visible(visible);
            }
            if let Some(visible) = sidebar_visible {
                pane.side_panel_mut().set_user_hidden(!visible);
            }
        }
        AgentServerMessage::AgentCatalog { agents } => {
            pane.set_agent_options(agent_options_from_catalog(&agents));
        }
        AgentServerMessage::SkillCatalog { skills } => {
            pane.set_skill_options(skill_options_from_catalog(&skills));
        }
        AgentServerMessage::McpCatalog { status } => {
            pane.set_mcp_status(status);
        }
        AgentServerMessage::McpOauthUrl { name, url } => {
            pane.apply_mcp_oauth_url(name, url);
        }
        AgentServerMessage::McpFailed { name, error } => {
            pane.system_message(name.unwrap_or_else(|| "MCP".to_string()), error);
        }
        AgentServerMessage::McpChanged { .. } => pane.refresh_mcp_if_visible(),
        AgentServerMessage::UsageUpdate { usage, .. } => {
            pane.apply_usage(map_usage(usage));
        }
        AgentServerMessage::TodoUpdate { todos, .. } => {
            pane.apply_todos(
                todos
                    .into_iter()
                    .map(|t| NeoismAgentTodo {
                        status: t.status,
                        content: t.content,
                    })
                    .collect(),
            );
        }
        AgentServerMessage::QueueUpdate {
            count,
            preview,
            started_at,
            ..
        } => {
            pane.apply_queue(count, preview, started_at);
        }
        AgentServerMessage::SubagentUpdate {
            session_id,
            status,
            title,
            agent,
            current_tool,
            started_at,
            root_session_id,
            execution_id,
            family_revision,
            ..
        } => {
            if matches!(
                status,
                neoism_protocol::agent::SubagentStatus::Completed
                    | neoism_protocol::agent::SubagentStatus::Failed
            ) && !pane.note_subagent_terminal_revision(
                    &session_id,
                    root_session_id.as_deref(),
                    execution_id.as_deref(),
                    family_revision,
                )
            {
                return;
            }
            pane.note_subagent_event(
                session_id,
                map_subagent_status(status),
                title,
                agent,
                current_tool,
                started_at,
            );
        }
        AgentServerMessage::Compaction {
            phase,
            text,
            reason,
            ..
        } => {
            pane.note_compaction(map_compaction_phase(phase), text, reason);
        }

        // -- Provider connect / auth flow ------------------------
        AgentServerMessage::ConnectProviderCatalog { providers, auth } => {
            pane.apply_connect_catalog(providers, auth);
        }
        AgentServerMessage::ConnectConnections { provider, connections } => {
            pane.apply_provider_connections(provider, connections);
        }
        AgentServerMessage::ConnectOauthUrl {
            url,
            auto,
            instructions,
            attempt_id,
        } => {
            pane.apply_connect_oauth_url(url, auto, instructions, attempt_id);
        }
        AgentServerMessage::ConnectFinished { provider, connection_id } => {
            pane.note_connect_finished_with_connection(provider, connection_id);
        }
        AgentServerMessage::ConnectFailed { provider, error } => {
            pane.note_connect_failed(provider, error);
        }

        // -- Maintenance -----------------------------------------
        AgentServerMessage::Pong => {
            // Connection-health probe — no UI mutation needed.
        }
    }
}

fn map_usage(
    u: neoism_protocol::agent::Usage,
) -> neoism_ui::panels::agent_pane::state::NeoismAgentUsage {
    neoism_ui::panels::agent_pane::state::NeoismAgentUsage {
        input: u.input,
        output: u.output,
        reasoning: u.reasoning,
        cache_read: u.cache_read,
        cache_write: u.cache_write,
        total: u.total,
        cost_micros: u.cost_micros,
        context_limit: u.context_limit,
    }
}

fn map_history_kind(
    k: neoism_protocol::agent::HistoryMessageKind,
) -> neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind {
    use neoism_protocol::agent::HistoryMessageKind;
    use neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind;
    match k {
        HistoryMessageKind::User => NeoismAgentMessageKind::User,
        HistoryMessageKind::Assistant => NeoismAgentMessageKind::Assistant,
        HistoryMessageKind::Reasoning => NeoismAgentMessageKind::Reasoning,
        HistoryMessageKind::Tool => NeoismAgentMessageKind::Tool,
        HistoryMessageKind::System => NeoismAgentMessageKind::System,
        HistoryMessageKind::Subtask => NeoismAgentMessageKind::Subtask,
        HistoryMessageKind::Compaction => NeoismAgentMessageKind::Compaction,
    }
}

fn map_history(
    m: neoism_protocol::agent::HistoryMessage,
) -> neoism_ui::panels::agent_pane::state::NeoismAgentMessage {
    use neoism_ui::panels::agent_pane::state::{
        NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentOutputKind,
        NeoismAgentTodo,
    };
    let kind = map_history_kind(m.kind);
    let output_kind = match kind {
        NeoismAgentMessageKind::Tool if !m.lang.is_empty() => NeoismAgentOutputKind::Code,
        _ => NeoismAgentOutputKind::Text,
    };
    NeoismAgentMessage {
        id: m.id,
        kind,
        author: m.author,
        title: m.title,
        text: m.text,
        status: m.status,
        tool: m.tool,
        output_kind,
        lang: m.lang,
        line_offset: m.line_offset.map(|l| l as usize),
        todos: m
            .todos
            .into_iter()
            .map(|t| NeoismAgentTodo {
                status: t.status,
                content: t.content,
            })
            .collect(),
        detail: m.detail,
        usage: m.usage.map(map_usage),
        // The daemon's HistoryMessage carries no attachment list;
        // image parts arrive through the live part stream instead.
        images: Vec::new(),
    }
}

fn map_streaming(
    s: neoism_protocol::agent::StreamingState,
) -> neoism_ui::panels::agent_pane::state::NeoismAgentStreamingState {
    use neoism_protocol::agent::StreamingState as ProtoStreamingState;
    use neoism_ui::panels::agent_pane::state::NeoismAgentStreamingState;
    match s {
        ProtoStreamingState::Idle => NeoismAgentStreamingState::Idle,
        ProtoStreamingState::Thinking => NeoismAgentStreamingState::Thinking,
        ProtoStreamingState::Working => NeoismAgentStreamingState::Working,
        ProtoStreamingState::Generating => NeoismAgentStreamingState::Generating,
        ProtoStreamingState::Compacting => NeoismAgentStreamingState::Compacting,
        ProtoStreamingState::WaitingSubagents => {
            NeoismAgentStreamingState::WaitingSubagents
        }
    }
}

/// Route a session-scoped event that FAILED the live gate into the
/// shared pane's background session cache instead of dropping it —
/// the wasm analogue of desktop's `!stream_is_active` arms in
/// `drain_server_updates`. The active session keeps rendering live;
/// cache-eligible events update their session's parked entry so
/// switching to it later restores the fully-caught-up conversation.
/// Returns `true` when the event was cache-routed; non-eligible
/// variants return `false` and are dropped exactly as before.
pub(crate) fn apply_agent_event_to_cache(
    pane: &mut neoism_ui::panels::agent_pane::state::NeoismAgentPane,
    parsed: neoism_protocol::agent::AgentServerMessage,
) -> bool {
    use neoism_protocol::agent::{
        AgentServerMessage, CompactionPhase, ContentKind, Role,
    };
    use neoism_ui::panels::agent_pane::state::{
        NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentStreamingState,
    };

    match parsed {
        // Background history snapshot: desktop's cached `Messages` arm —
        // reconcile pending prompts + merge with live-streamed parts.
        AgentServerMessage::HistoryChunk {
            session_id,
            messages,
            next_cursor,
        } => {
            let mapped: Vec<NeoismAgentMessage> =
                messages.into_iter().map(map_history).collect();
            pane.apply_history_to_cache(&session_id, mapped, next_cursor);
            true
        }
        AgentServerMessage::MessageStart {
            session_id,
            role,
            message_id,
        } => {
            let kind = match role {
                Role::User => NeoismAgentMessageKind::User,
                Role::System => NeoismAgentMessageKind::System,
                Role::Assistant => NeoismAgentMessageKind::Assistant,
            };
            let row = NeoismAgentMessage {
                id: message_id,
                kind,
                title: String::new(),
                text: String::new(),
                status: String::new(),
                tool: String::new(),
                output_kind:
                    neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind::Text,
                lang: String::new(),
                line_offset: None,
                todos: Vec::new(),
                detail: String::new(),
                usage: None,
                author: None,
                images: Vec::new(),
            };
            pane.cache_upsert_part_message(&session_id, row);
            pane.cache_note_streaming(
                &session_id,
                NeoismAgentStreamingState::Generating,
                None,
            );
            // A family member (sibling child / the parent) started
            // talking while another family session is on screen — keep
            // the parent-keyed sidebar roster's running state live.
            pane.note_family_session_streaming(&session_id, true);
            true
        }
        AgentServerMessage::ContentDelta {
            session_id,
            message_id,
            kind,
            text,
        } => {
            let delta_kind = match kind {
                ContentKind::Text => "text".to_string(),
                ContentKind::Reasoning => "reasoning".to_string(),
                ContentKind::Tool { name } => name,
            };
            pane.cache_apply_part_delta(
                &session_id,
                Some(&message_id),
                Some(delta_kind.as_str()),
                &text,
            );
            true
        }
        // SessionIdle is the authoritative idle edge (the live arm also
        // ignores MessageEnd), but consuming it here keeps a stray
        // MessageEnd from being logged as dropped.
        AgentServerMessage::MessageEnd { .. } => true,
        AgentServerMessage::MessageUpdated {
            session_id,
            message,
        } => {
            pane.cache_upsert_part_message(&session_id, map_history(message));
            true
        }
        AgentServerMessage::PartRemoved {
            session_id,
            part_id,
        } => {
            pane.cache_remove_part_message(&session_id, &part_id);
            true
        }
        AgentServerMessage::SessionIdle { session_id } => {
            pane.cache_note_session_idle(&session_id);
            // SessionRun idle is not a terminal parent-task lifecycle edge.
            true
        }
        AgentServerMessage::RuntimeSnapshot {
            session_id: _,
            snapshot,
        } => {
            if !pane.session_family_contains(&snapshot.root_session_id) {
                return true;
            }
            if let (Some(epoch), Some(revision), Some(tasks)) = (
                snapshot.background_jobs_epoch.as_deref(),
                snapshot.background_jobs_revision,
                snapshot.running_background_tasks.as_ref(),
            ) {
                let tasks = tasks
                    .iter()
                    .map(|task| (task.job_id.clone(), task.started_at))
                    .collect::<Vec<_>>();
                pane.apply_running_background_tasks(epoch, revision, &tasks);
            }
            let execution = snapshot.execution.map(|activity| {
                let activity = neoism_ui::panels::agent_pane::state::ExecutionActivityState {
                        execution_id: activity.execution_id,
                        root_session_id: activity.root_session_id,
                        completed_ms: activity.completed_ms,
                        active_segments: activity.active_segments,
                        session_activities: activity.session_activities.into_iter().map(|(session_id, activity)| {
                            (session_id, neoism_ui::panels::agent_pane::state::ProviderActivityState {
                                completed_ms: activity.completed_ms,
                                active_segments: activity.active_segments,
                            })
                        }).collect(),
                        revision: activity.revision,
                        finished: activity.finished,
                    };
                activity
            });
            if snapshot.branches_authoritative {
                pane.apply_runtime_lifecycle_snapshot(
                    execution,
                    snapshot.root_session_id.clone(),
                    snapshot.family_revision,
                    snapshot.branches.into_iter().map(|branch| {
                        (branch.session_id, branch.status, branch.started_at)
                    }),
                );
            } else if let Some(execution) = execution {
                pane.apply_execution_activity(execution);
            }
            true
        }
        AgentServerMessage::BackgroundTasksUpdated {
            epoch,
            revision,
            tasks,
            ..
        } => {
            let tasks = tasks
                .into_iter()
                .map(|task| (task.job_id, task.started_at))
                .collect::<Vec<_>>();
            pane.apply_running_background_tasks(&epoch, revision, &tasks);
            true
        }
        AgentServerMessage::StreamingState {
            session_id,
            state,
            label,
        } => {
            pane.cache_note_streaming(&session_id, map_streaming(state), label);
            pane.note_family_session_streaming(&session_id, true);
            true
        }
        AgentServerMessage::QueueUpdate {
            session_id,
            count,
            preview,
            started_at,
        } => {
            pane.cache_apply_queue(&session_id, count, preview, started_at);
            true
        }
        AgentServerMessage::UsageUpdate { session_id, usage } => {
            pane.cache_apply_usage(&session_id, map_usage(usage));
            true
        }
        AgentServerMessage::Notice {
            session_id,
            title,
            body,
            ..
        } => {
            pane.cache_push_system_message(&session_id, title, body);
            true
        }
        AgentServerMessage::Compaction {
            session_id, phase, ..
        } => {
            match phase {
                CompactionPhase::Started => pane.cache_note_streaming(
                    &session_id,
                    NeoismAgentStreamingState::Compacting,
                    None,
                ),
                CompactionPhase::Delta => {}
                CompactionPhase::Ended => pane.cache_note_session_idle(&session_id),
            }
            true
        }
        // Roster-level, parent-keyed: while a child transcript is open
        // the viewed-session gate diverts sibling subagent updates
        // here. Apply them to the live roster when they belong to the
        // tracked family so the sidebar keeps its names, statuses and
        // running count (a subagent conversation is an extension of the
        // main chat, not a separate context).
        AgentServerMessage::SubagentUpdate {
            session_id,
            status,
            title,
            agent,
            current_tool,
            started_at,
            parent_session_id,
            root_session_id,
            execution_id,
            family_revision,
        } => {
            use neoism_protocol::agent::SubagentStatus;
            use neoism_ui::panels::agent_pane::state::side_panel::BranchStatus;
            // A BRAND-NEW child (daemon-synthesized from the server's
            // `session.created`) isn't in the roster yet, so its own id
            // fails the family check — admit it through its parent link
            // instead: if the announced parent is the tracked family
            // (the viewed session, the viewed child's parent, or any
            // roster row), the newcomer belongs to this conversation.
            let in_family = pane.session_family_contains(&session_id)
                || parent_session_id
                    .as_deref()
                    .is_some_and(|parent| pane.session_family_contains(parent));
            if in_family {
                let status = match status {
                    SubagentStatus::Running => BranchStatus::Active,
                    SubagentStatus::Blocked => BranchStatus::WaitingPermission,
                    SubagentStatus::Completed => BranchStatus::Completed,
                    SubagentStatus::Failed => BranchStatus::Stopped,
                };
                if matches!(status, BranchStatus::Completed | BranchStatus::Stopped)
                    && !pane.note_subagent_terminal_revision(
                        &session_id,
                        root_session_id.as_deref(),
                        execution_id.as_deref(),
                        family_revision,
                    )
                {
                    return true;
                }
                pane.note_subagent_event(
                    session_id,
                    status,
                    title,
                    agent,
                    current_tool,
                    started_at,
                );
                true
            } else {
                false
            }
        }
        // Everything else (tool gating, questions, edits, catalogs,
        // thread lifecycle) stays gated out exactly as before — those
        // either target the live view or are handled by the pre-gate
        // catalog side effects.
        _ => false,
    }
}

/// Parse one raw `question.asked`-shaped request payload and enqueue it
/// as a pending question. Runs the payload through the SAME
/// `question_policy::question_request_from_event` parser the desktop
/// SSE path uses; requests without an id or questions are dropped
/// (nothing could be answered), and a payload missing its `sessionID`
/// inherits the envelope's session id.
pub(crate) fn enqueue_question_request(
    pane: &mut neoism_ui::panels::agent_pane::state::NeoismAgentPane,
    session_id: &str,
    request: &serde_json::Value,
) {
    let mut pending =
        neoism_ui::panels::agent_pane::question_policy::question_request_from_event(
            request,
        );
    if pending.id.is_empty() || pending.questions.is_empty() {
        return;
    }
    if pending.session_id.is_empty() {
        pending.session_id = session_id.to_string();
    }
    pane.enqueue_pending_question(pending);
}

pub(crate) fn agent_event_session_id(
    parsed: &neoism_protocol::agent::AgentServerMessage,
) -> Option<&str> {
    use neoism_protocol::agent::AgentServerMessage;
    match parsed {
        AgentServerMessage::ThreadCreated { session_id, .. }
        | AgentServerMessage::ThreadSwitched { session_id, .. }
        | AgentServerMessage::ThreadDeleted { session_id, .. }
        | AgentServerMessage::HistoryChunk { session_id, .. }
        | AgentServerMessage::SessionEvent { session_id, .. }
        | AgentServerMessage::MessageUpdated { session_id, .. }
        | AgentServerMessage::PartRemoved { session_id, .. }
        | AgentServerMessage::SessionIdle { session_id, .. }
        | AgentServerMessage::RuntimeSnapshot { session_id, .. }
        | AgentServerMessage::BackgroundTasksUpdated { session_id, .. }
        | AgentServerMessage::StreamingState { session_id, .. }
        | AgentServerMessage::Notice { session_id, .. }
        | AgentServerMessage::ToolUseRequest { session_id, .. }
        | AgentServerMessage::PermissionRemoved { session_id, .. }
        | AgentServerMessage::ToolUseResult { session_id, .. }
        | AgentServerMessage::QuestionAsked { session_id, .. }
        | AgentServerMessage::QuestionsUpdated { session_id, .. }
        | AgentServerMessage::QuestionRemoved { session_id, .. }
        | AgentServerMessage::EditProposed { session_id, .. }
        | AgentServerMessage::EditApplied { session_id, .. }
        | AgentServerMessage::EditRejected { session_id, .. }
        | AgentServerMessage::TodoUpdate { session_id, .. }
        | AgentServerMessage::SubagentUpdate { session_id, .. }
        | AgentServerMessage::Compaction { session_id, .. }
        | AgentServerMessage::ProviderState { session_id, .. }
        | AgentServerMessage::QueueUpdate { session_id, .. }
        | AgentServerMessage::UsageUpdate { session_id, .. } => Some(session_id),
        AgentServerMessage::MessageStart { session_id, .. }
        | AgentServerMessage::ContentDelta { session_id, .. }
        | AgentServerMessage::MessageEnd { session_id, .. } => Some(session_id),
        AgentServerMessage::CommandOutput { session_id, .. } => session_id.as_deref(),
        // ThreadUpdated is deliberately session-UNSCOPED for gating:
        // renames / pins can target any row of the /sessions picker,
        // and their ack must reach the catalog-refresh trigger even
        // when the target isn't the active session. QuestionReplyFailed
        // and PermissionReplyFailed carry no session id at all.
        AgentServerMessage::ThreadUpdated { .. }
        | AgentServerMessage::QuestionReplyFailed { .. }
        | AgentServerMessage::PermissionReplyFailed { .. }
        | AgentServerMessage::Disabled { .. }
        | AgentServerMessage::PermissionRequest { .. }
        | AgentServerMessage::Error { .. }
        | AgentServerMessage::ThreadList { .. }
        | AgentServerMessage::ThreadListFailed { .. }
        | AgentServerMessage::ProviderCatalog { .. }
        | AgentServerMessage::ConfigDefaults { .. }
        | AgentServerMessage::AgentCatalog { .. }
        | AgentServerMessage::SkillCatalog { .. }
        | AgentServerMessage::McpCatalog { .. }
        | AgentServerMessage::McpOauthUrl { .. }
        | AgentServerMessage::McpFailed { .. }
        | AgentServerMessage::McpChanged { .. }
        | AgentServerMessage::ConnectProviderCatalog { .. }
        | AgentServerMessage::ConnectConnections { .. }
        | AgentServerMessage::ConnectOauthUrl { .. }
        | AgentServerMessage::ConnectFinished { .. }
        | AgentServerMessage::ConnectFailed { .. }
        | AgentServerMessage::Pong => None,
    }
}

pub(crate) fn model_options_from_catalog(
    providers: &[neoism_protocol::agent::ProviderInfo],
) -> Vec<neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption> {
    use neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption;

    let mut out = Vec::new();
    for provider in providers {
        if provider.models.is_empty() {
            continue;
        }
        out.push(NeoismAgentPickerOption::header(&provider.name));
        for model in &provider.models {
            let footer = model
                .context_limit
                .map(|limit| format!("{}k ctx", (limit as f32 / 1000.0).round() as u64))
                .unwrap_or_default();
            out.push(NeoismAgentPickerOption::model(
                &model.name,
                &provider.name,
                &footer,
                &format!("{}/{}", provider.id, model.id),
            ));
        }
    }
    out
}

pub(crate) fn model_context_limits_from_catalog(
    providers: &[neoism_protocol::agent::ProviderInfo],
) -> std::collections::HashMap<String, u64> {
    providers
        .iter()
        .flat_map(|provider| {
            provider.models.iter().filter_map(move |model| {
                model
                    .context_limit
                    .map(|limit| (format!("{}/{}", provider.id, model.id), limit))
            })
        })
        .collect()
}

pub(crate) fn agent_options_from_catalog(
    agents: &[neoism_protocol::agent::AgentInfo],
) -> Vec<neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption> {
    use neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption;

    let mut out = vec![NeoismAgentPickerOption::new(
        "session default",
        "Use Neoism default",
        "default",
        "",
    )];
    // Subagent-only definitions (mode == "subagent", e.g.
    // explore/general) are Task-tool targets, not top-level
    // agents — the picker shows primaries (build/plan) plus
    // whatever the user's config adds.
    out.extend(
        agents
            .iter()
            .filter(|agent| agent.mode.as_deref() != Some("subagent"))
            .map(|agent| {
                NeoismAgentPickerOption::new(
                    &agent.name,
                    &agent.description,
                    agent.mode.as_deref().unwrap_or("agent"),
                    &agent.name,
                )
            }),
    );
    out
}

pub(crate) fn skill_options_from_catalog(
    skills: &[neoism_protocol::agent::SkillInfo],
) -> Vec<neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption> {
    use neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption;

    skills
        .iter()
        .map(|skill| {
            NeoismAgentPickerOption::new(
                &skill.name,
                &skill.description,
                skill.path.as_deref().unwrap_or("skill"),
                &skill.name,
            )
        })
        .collect()
}

pub(crate) fn session_options_from_catalog(
    threads: &[neoism_protocol::agent::ThreadSummary],
    current_session_id: Option<&str>,
) -> Vec<neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption> {
    use neoism_ui::panels::agent_pane::session_group::{
        group_session_options, SessionOptionInput,
    };
    use neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption;

    // Title-only rows grouped under cyan date headers ("Pinned" first,
    // then newest day first) — the date header replaces the old
    // per-row relative-time footer.
    let inputs = threads
        .iter()
        .map(|thread| {
            let mut option = NeoismAgentPickerOption::new(
                if thread.title.trim().is_empty() {
                    "Untitled"
                } else {
                    &thread.title
                },
                "",
                "",
                &thread.session_id,
            );
            option.is_current = Some(thread.session_id.as_str()) == current_session_id;
            option.pinned = thread.pinned;
            SessionOptionInput {
                option,
                updated_ms: thread.updated_at,
            }
        })
        .collect::<Vec<_>>();
    group_session_options(inputs)
}

pub(crate) fn session_entries_from_catalog(
    threads: &[neoism_protocol::agent::ThreadSummary],
) -> Vec<neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry> {
    use neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry;

    // Flat entries (no header rows — the side panel injects date-group
    // headers itself), carrying the raw timestamp + pin flag. A busy
    // session surfaces `running` so the home list paints the live
    // status dot — the web analogue of desktop's
    // `session_running_status` (api.rs), which only ever reports
    // "running" for home rows and never terminalizes idle sessions.
    threads
        .iter()
        .map(|thread| {
            NeoismAgentSessionEntry::new(
                &thread.session_id,
                if thread.title.trim().is_empty() {
                    "untitled session"
                } else {
                    &thread.title
                },
                "",
            )
            .with_updated_ms(thread.updated_at)
            .with_pinned(thread.pinned)
            .with_runtime_status(thread.busy.then(|| "running".to_string()))
        })
        .collect()
}

/// Derive Subagents-section seeds from a freshly-fetched history page:
/// every `task` tool card carries its child session id in a
/// `task_id: <id>` line (see `message_policy::task_id_from_text`), and
/// the card's status field records the branch's settled state. Returns
/// `(child_session_id, status, title)` triples, one per unique child,
/// newest occurrence winning.
pub(crate) fn subagent_seeds_from_history(
    messages: &[neoism_ui::panels::agent_pane::state::NeoismAgentMessage],
) -> Vec<(
    String,
    neoism_ui::panels::agent_pane::state::side_panel::BranchStatus,
    Option<String>,
)> {
    use neoism_ui::panels::agent_pane::message_policy::task_id_from_text;
    use neoism_ui::panels::agent_pane::state::side_panel::BranchStatus;
    use neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind;

    let mut seeds: Vec<(String, BranchStatus, Option<String>)> = Vec::new();
    for message in messages {
        let is_task = matches!(
            message.kind,
            NeoismAgentMessageKind::Tool | NeoismAgentMessageKind::Subtask
        ) && message.tool == "task";
        if !is_task {
            continue;
        }
        let Some(task_id) = task_id_from_text(&message.detail, &message.text) else {
            continue;
        };
        // Historical cards default to Completed: a card whose child was
        // still live when the client disconnected shows a stale
        // "running" marker, and resurrecting it as Active would blink a
        // white working dot forever. A genuinely-live child re-announces
        // itself through `SubagentUpdate` right after `ResumeStream`.
        let status = match message.status.trim().to_ascii_lowercase().as_str() {
            "error" | "failed" | "stopped" | "aborted" => BranchStatus::Stopped,
            _ => BranchStatus::Completed,
        };
        let title = {
            let trimmed = message.title.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        };
        if let Some(existing) = seeds.iter_mut().find(|(id, _, _)| *id == task_id) {
            existing.1 = status;
            if title.is_some() {
                existing.2 = title;
            }
        } else {
            seeds.push((task_id, status, title));
        }
    }
    seeds
}
