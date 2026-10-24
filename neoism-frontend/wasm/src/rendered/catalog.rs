use neoism_ui::Chrome;

/// Identity transitions request immediately; same-identity failures obey the
/// shared debounced/bounded retry budget, and Ready catalogs do not poll.
pub(crate) fn project_discovery_request_due(
    identity_changed: bool,
    shared_refresh_due: bool,
) -> bool {
    identity_changed || shared_refresh_due
}

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
    apply_agent_event_to_pane_inner(pane, parsed);
}

fn apply_agent_event_to_pane_inner(
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
                tool_batch_id: None,
                mcp: None,
                composition_children: Vec::new(),
                composition_child: false,
                output_kind: NeoismAgentOutputKind::Text,
                lang: String::new(),
                line_offset: None,
                todos: Vec::new(),
                detail: String::new(),
                usage: None,
                author: None,
                images: Vec::new(),
            };
            pane.ingest_live_part_message(row);
        }
        AgentServerMessage::ContentDelta {
            message_id,
            parent_message_id,
            kind,
            text,
            ..
        } => {
            let delta_kind = match kind {
                ContentKind::Text => Some("text".to_string()),
                ContentKind::Reasoning => Some("reasoning".to_string()),
                ContentKind::Compaction => Some("compaction".to_string()),
                ContentKind::Tool { name } => Some(name),
            };
            pane.ingest_live_part_delta(parent_message_id, Some(message_id), delta_kind, &text);
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
            pane.side_panel_mut().remove_session(&session_id);
            pane.clear_session_id_if(&session_id);
        }
        AgentServerMessage::SessionProjects {
            items,
            pinned_sessions,
            ..
        } => {
            // Discovery is root-global only. Keep the defensive scope gate here
            // as well as the host's endpoint/generation validation.
            if pane.side_panel().catalog_directory().is_some() {
                return;
            }
            let pins = pinned_sessions
                .iter()
                .filter_map(session_entry_from_root)
                .collect::<Vec<_>>();
            for entry in &pins {
                pane.remember_session_source(&entry.id, entry.source);
            }
            pane.side_panel_mut().set_session_projects(
                items
                    .into_iter()
                    .map(|item| (item.directory, item.updated_ms))
                    .collect(),
            );
            pane.side_panel_mut().set_pinned_sessions(pins);
        }
        AgentServerMessage::SessionProjectsFailed { message, .. } => {
            // Discovery refreshes are non-destructive. Existing grouped rows stay
            // Ready; only an initial failed catalog uses the shared retry budget.
            if !pane.side_panel().project_pagination_enabled() {
                pane.side_panel_mut().settle_session_page_error(&message);
            }
        }
        AgentServerMessage::SessionProjectsChanged => {}
        AgentServerMessage::ProjectSessionPage {
            directory,
            requested_cursor,
            threads,
            next_cursor,
            generation,
            ..
        } => {
            let entries = session_entries_from_catalog(&threads);
            for entry in &entries {
                pane.remember_session_source(&entry.id, entry.source);
            }
            pane.side_panel_mut().set_project_session_page(
                &directory,
                requested_cursor.as_deref(),
                entries,
                next_cursor,
                generation,
            );
        }
        AgentServerMessage::ProjectSessionPageFailed {
            directory,
            generation,
            message,
            ..
        } => {
            pane.side_panel_mut()
                .settle_project_session_error(&directory, generation, message);
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
            pane.set_session_options_page(
                session_options_from_catalog(&threads, current_session_id.as_deref()),
                requested_cursor.as_deref(),
                next_cursor.clone(),
            );
            let entries = session_entries_from_catalog(&threads);
            for entry in &entries {
                pane.remember_session_source(&entry.id, entry.source);
            }
            if requested_cursor == expected_cursor
                && !pane.side_panel().project_pagination_enabled()
            {
                pane.side_panel_mut().set_session_page(
                    entries,
                    requested_cursor.as_deref(),
                    next_cursor,
                );
            }
        }
        AgentServerMessage::ThreadListFailed { message } => {
            tracing::warn!(%message, "failed to refresh agent sessions");
            pane.session_picker_page_failed();
            pane.side_panel_mut()
                .settle_session_page_error("couldn't load sessions; retrying");
        }
        AgentServerMessage::HistoryChunk {
            session_id,
            messages,
            next_cursor,
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
            pane.apply_history_page(mapped, next_cursor);
            for (task_id, status, title) in seeds {
                pane.note_subagent_event(task_id, status, title, None, None, None);
            }
        }
        AgentServerMessage::SessionEvent {
            session_id,
            kind,
            properties,
        } => {
            if kind == "session.compaction.admission.failed" {
                if let Some(request_id) = properties.get("requestID").and_then(serde_json::Value::as_str) {
                    pane.compact_admission_failed(&session_id, request_id);
                }
            }
            if kind == "session.queue.updated" {
                pane.apply_queue_event(&session_id, properties.clone());
            }
            if kind == "session.compaction.queued" {
                pane.note_compaction_queued();
            }
            if kind == "session.moved"
                && pane.session_id_str() == Some(session_id.as_str())
            {
                if let Some(directory) = properties
                    .get("directory")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|directory| !directory.is_empty())
                {
                    pane.set_directory(Some(directory.to_string()));
                }
            }
        }
        AgentServerMessage::MessageUpdated {
            message, parent_message_id, ..
        } => {
            pane.ingest_live_part_message_with_parent(
                map_history(message), parent_message_id.as_deref(),
            );
        }
        AgentServerMessage::PartRemoved { part_id, .. } => {
            pane.remove_part_message(&part_id);
        }
        AgentServerMessage::SessionIdle { .. } => {
            pane.note_session_idle();
        }
        AgentServerMessage::RuntimeSnapshot {
            session_id,
            snapshot,
        } => {
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
            let accepted = if snapshot.branches_authoritative {
                pane.apply_authoritative_family_runtime_snapshot(
                    &session_id,
                    execution,
                    snapshot.root_session_id.clone(),
                    snapshot.family_revision,
                    snapshot.branches.into_iter().map(|branch| {
                        (
                            branch.session_id,
                            branch.parent_session_id,
                            branch.status,
                            branch.started_at,
                        )
                    }),
                )
            } else if pane.session_family_contains(&snapshot.root_session_id) {
                if let Some(execution) = execution {
                    pane.apply_execution_activity(execution);
                }
                true
            } else {
                false
            };
            // A stale branch revision does not make the independently
            // versioned job list stale. Still reject unrelated families.
            if !accepted && !pane.session_family_contains(&snapshot.root_session_id) {
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
            if state == neoism_protocol::agent::StreamingState::WaitingSubagents {
                pane.note_waiting_subagents_hint();
            } else {
                pane.note_remote_streaming(map_streaming(state), label);
            }
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
            tool,
            status,
            output,
            error,
        } => {
            let task_terminal = (tool == "task")
                .then(|| output.as_deref())
                .flatten()
                .and_then(|output| {
                    let task_id = neoism_ui::panels::agent_pane::stream_events::task_id_from_output(output)?;
                    let status = neoism_ui::panels::agent_pane::stream_events::task_status_from_output(output)?;
                    let status = match neoism_ui::panels::agent_pane::stream_events::normalize_subagent_status(status) {
                        "completed" => Some(BranchStatus::Completed),
                        "error" => Some(BranchStatus::Stopped),
                        _ => None,
                    }?;
                    Some((task_id, status))
                });
            pane.finalize_tool_card(
                &tool_use_id,
                tool_status_label(status),
                output,
                error,
            );
            if let Some((task_id, status)) = task_terminal {
                pane.note_subagent_event(task_id, status, None, None, None, None);
            }
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

        AgentServerMessage::CatalogReset { directory } => {
            pane.side_panel_mut().clear_catalog_families_in_scope(directory.as_deref());
            if directory.is_none() {
                pane.side_panel_mut().clear_session_activity_overrides();
            }
        }
        AgentServerMessage::CatalogFamily {
            session_id,
            snapshot,
        } => {
            if let Ok(snapshot) =
                serde_json::from_value::<neoism_agent_core::CatalogFamilySnapshot>(snapshot)
            {
                pane.side_panel_mut()
                    .apply_catalog_family_snapshot(&session_id, snapshot);
            }
        }
        AgentServerMessage::CatalogActivity {
            session_id,
            activity,
        } => {
            if let Ok(activity) =
                serde_json::from_value(serde_json::Value::String(activity))
            {
                pane.side_panel_mut()
                    .set_session_activity(&session_id, activity);
            }
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
            authoritative,
            provider_id,
            model,
            connection_id,
            agent,
            thinking,
            context_limit,
            ..
        } => {
            if authoritative {
                pane.apply_authoritative_provider_state(
                    model,
                    connection_id,
                    agent,
                    thinking,
                    context_limit,
                );
            } else {
                pane.set_connection_id(connection_id);
                pane.apply_provider_state(
                    provider_id,
                    model,
                    agent,
                    thinking,
                    context_limit,
                );
            }
        }
        AgentServerMessage::ProviderCatalog { providers, catalog_status, request_id } => {
            if pane.apply_model_catalog(model_options_from_catalog(&providers), catalog_status.as_ref(), request_id.as_deref()) && !providers.is_empty() {
                pane.set_model_context_limits(model_context_limits_from_catalog(&providers));
            }
        }
        AgentServerMessage::ConfigDefaults {
            agent,
            model,
            thinking,
            input_help_visible,
            sidebar_visible,
        } => {
            pane.apply_config_defaults_if_unset(model, agent, thinking);
            if let Some(visible) = input_help_visible {
                pane.set_input_help_visible(visible);
            }
            if let Some(visible) = sidebar_visible {
                pane.side_panel_mut().set_user_hidden(!visible);
            }
        }
        AgentServerMessage::ConfigDefaultsFailed { .. } => {
            pane.finish_config_defaults_loading();
        }
        AgentServerMessage::AgentCatalog { agents } => {
            pane.set_agent_options(agent_options_from_catalog(&agents));
        }
        AgentServerMessage::SkillCatalog { skills } => {
            pane.set_skill_options(skill_options_from_catalog(&skills));
        }
        AgentServerMessage::McpCatalog { status, directory } => {
            pane.set_mcp_status_for_directory(directory, status);
        }
        AgentServerMessage::McpOauthUrl { name, url } => {
            pane.apply_mcp_oauth_url(name, url);
        }
        AgentServerMessage::McpFailed { name, error } => {
            pane.apply_mcp_failure(name, error);
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
            ) {
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
        AgentServerMessage::SubagentMetadata {
            session_id,
            title,
            agent,
            ..
        } => {
            pane.note_subagent_metadata(&session_id, title, agent);
        }
        AgentServerMessage::Compaction {
            phase,
            text,
            reason,
            run_continues,
            ..
        } => {
            pane.note_compaction_with_continuation(
                map_compaction_phase(phase),
                text,
                reason,
                run_continues,
            );
        }

        // -- Provider connect / auth flow ------------------------
        AgentServerMessage::ConnectProviderCatalog { providers, auth, request_id } => {
            pane.apply_connect_catalog_result(request_id, providers, auth);
        }
        AgentServerMessage::ConnectProviderCatalogFailed { request_id, error } => {
            pane.apply_connect_catalog_failed(request_id, error);
        }
        AgentServerMessage::ConnectConnections {
            provider,
            connections,
        } => {
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
        AgentServerMessage::ConnectFinished {
            provider,
            connection_id,
        } => {
            pane.note_connect_finished_with_connection(provider, connection_id);
        }
        AgentServerMessage::ConnectFailed { provider, error } => {
            pane.note_connect_failed(provider, error);
        }

        // -- Maintenance -----------------------------------------
        AgentServerMessage::ThreadSearchResult { .. } => {}
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
        NeoismAgentImage, NeoismAgentMessage, NeoismAgentMessageKind,
        NeoismAgentOutputKind, NeoismAgentTodo,
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
        tool_batch_id: m.tool_batch_id,
        mcp: m.mcp.as_ref().and_then(neoism_ui::panels::agent_pane::mcp_icon::McpToolPresentation::from_wire),
        composition_children: Vec::new(),
        composition_child: false,
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
        images: m
            .attachments
            .into_iter()
            .map(|attachment| NeoismAgentImage {
                filename: attachment.filename,
                url: attachment.url.into(),
                mime: attachment.mime,
            })
            .collect(),
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
    apply_agent_event_to_cache_inner(pane, parsed)
}

fn apply_agent_event_to_cache_inner(
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
                tool_batch_id: None,
                mcp: None,
                composition_children: Vec::new(),
                composition_child: false,
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
            parent_message_id,
            kind,
            text,
        } => {
            let delta_kind = match kind {
                ContentKind::Text => "text".to_string(),
                ContentKind::Reasoning => "reasoning".to_string(),
                ContentKind::Compaction => "compaction".to_string(),
                ContentKind::Tool { name } => name,
            };
            pane.cache_apply_part_delta_with_parent(
                &session_id,
                parent_message_id.as_deref(),
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
            parent_message_id,
        } => {
            pane.cache_upsert_part_message_with_parent(&session_id, map_history(message), parent_message_id.as_deref());
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
            session_id,
            snapshot,
        } => {
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
            let accepted = if snapshot.branches_authoritative {
                pane.apply_authoritative_family_runtime_snapshot(
                    &session_id,
                    execution,
                    snapshot.root_session_id.clone(),
                    snapshot.family_revision,
                    snapshot.branches.into_iter().map(|branch| {
                        (
                            branch.session_id,
                            branch.parent_session_id,
                            branch.status,
                            branch.started_at,
                        )
                    }),
                )
            } else if pane.session_family_contains(&snapshot.root_session_id) {
                if let Some(execution) = execution {
                    pane.apply_execution_activity(execution);
                }
                true
            } else {
                false
            };
            // Branch and job revisions are independent, including empty lists.
            if !accepted && !pane.session_family_contains(&snapshot.root_session_id) {
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
            if state == neoism_protocol::agent::StreamingState::WaitingSubagents {
                pane.cache_note_waiting_subagents_hint(&session_id);
            } else {
                let active = state != neoism_protocol::agent::StreamingState::Idle;
                pane.cache_note_streaming(&session_id, map_streaming(state), label);
                pane.note_family_session_streaming(&session_id, active);
            }
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
        AgentServerMessage::ProviderState {
            session_id,
            authoritative: true,
            model,
            connection_id,
            agent,
            thinking,
            context_limit,
            ..
        } => {
            pane.cache_apply_authoritative_provider_state(
                &session_id,
                model,
                connection_id,
                agent,
                thinking,
                context_limit,
            );
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
        AgentServerMessage::SessionEvent {
            session_id, kind, properties,
        } if kind == "session.compaction.admission.failed" => {
            if let Some(request_id) = properties.get("requestID").and_then(serde_json::Value::as_str) {
                pane.compact_admission_failed(&session_id, request_id);
            }
            true
        }
        AgentServerMessage::SessionEvent {
            session_id, kind, properties,
        } if kind == "session.queue.updated" => {
            pane.apply_queue_event(&session_id, properties);
            true
        }
        AgentServerMessage::SessionEvent {
            session_id, kind, ..
        } if kind == "session.compaction.queued" => {
            pane.cache_note_compaction_queued(&session_id);
            true
        }
        AgentServerMessage::Compaction {
            session_id,
            phase,
            run_continues,
            ..
        } => {
            match phase {
                CompactionPhase::Started => pane.cache_note_streaming(
                    &session_id,
                    NeoismAgentStreamingState::Compacting,
                    None,
                ),
                CompactionPhase::Delta => {}
                CompactionPhase::Ended => {
                    pane.cache_note_compaction_ended(&session_id, run_continues)
                }

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
        AgentServerMessage::SubagentMetadata {
            session_id,
            title,
            agent,
            parent_session_id,
        } => {
            let in_family = pane.session_family_contains(&session_id)
                || parent_session_id
                    .as_deref()
                    .is_some_and(|parent| pane.session_family_contains(parent));
            if in_family {
                pane.note_subagent_metadata(&session_id, title, agent);
            }
            in_family
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
        | AgentServerMessage::SubagentMetadata { session_id, .. }
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
        AgentServerMessage::CatalogReset { .. }
        | AgentServerMessage::CatalogFamily { .. }
        | AgentServerMessage::ThreadDeleted { .. }
        | AgentServerMessage::CatalogActivity { .. }
        | AgentServerMessage::ThreadUpdated { .. }
        | AgentServerMessage::QuestionReplyFailed { .. }
        | AgentServerMessage::PermissionReplyFailed { .. }
        | AgentServerMessage::Disabled { .. }
        | AgentServerMessage::PermissionRequest { .. }
        | AgentServerMessage::Error { .. }
        | AgentServerMessage::SessionProjectsChanged
        | AgentServerMessage::SessionProjects { .. }
        | AgentServerMessage::SessionProjectsFailed { .. }
        | AgentServerMessage::ProjectSessionPage { .. }
        | AgentServerMessage::ProjectSessionPageFailed { .. }
        | AgentServerMessage::ThreadList { .. }
        | AgentServerMessage::ThreadSearchResult { .. }
        | AgentServerMessage::ThreadListFailed { .. }
        | AgentServerMessage::ProviderCatalog { .. }
        | AgentServerMessage::ConfigDefaults { .. }
        | AgentServerMessage::ConfigDefaultsFailed { .. }
        | AgentServerMessage::AgentCatalog { .. }
        | AgentServerMessage::SkillCatalog { .. }
        | AgentServerMessage::McpCatalog { .. }
        | AgentServerMessage::McpOauthUrl { .. }
        | AgentServerMessage::McpFailed { .. }
        | AgentServerMessage::McpChanged { .. }
        | AgentServerMessage::ConnectProviderCatalog { .. }
        | AgentServerMessage::ConnectProviderCatalogFailed { .. }
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
        })
        .collect()
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

fn session_entry_from_root(
    session: &neoism_protocol::agent::SessionListInfo,
) -> Option<neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry> {
    use neoism_ui::panels::agent_pane::state::side_panel::{
        ConversationSource, NeoismAgentSessionEntry,
    };
    let external = session
        .get("externalAgent")
        .or_else(|| session.pointer("/extra/externalAgent"));
    if external.is_some_and(|external| {
        external
            .get("historyState")
            .and_then(serde_json::Value::as_str)
            != Some("neoism_only")
    }) {
        return None;
    }
    let id = session.get("id")?.as_str()?;
    let title = session
        .get("title")
        .and_then(serde_json::Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled");
    Some(
        NeoismAgentSessionEntry::new(id, title, "")
            .with_directory(
                session
                    .get("directory")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            )
            .with_source(ConversationSource::from_session_json(session))
            .with_source_key(
                external
                    .and_then(|external| external.get("sourceKey"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            )
            .with_updated_ms(
                session
                    .pointer("/time/updated")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            )
            .with_pinned(
                session
                    .get("pinned")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            )
            .with_catalog_activity(
                session
                    .get("catalogActivity")
                    .and_then(|value| serde_json::from_value(value.clone()).ok()),
            ),
    )
}

pub(crate) fn session_entries_from_catalog(
    threads: &[neoism_protocol::agent::ThreadSummary],
) -> Vec<neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry> {
    use neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry;

    // Family activity is separate from branch lifecycle status. Older daemon
    // snapshots without it retain the existing busy-flag fallback.
    threads
        .iter()
        .map(|thread| {
            let mut entry = NeoismAgentSessionEntry::new(
                &thread.session_id,
                if thread.title.trim().is_empty() {
                    "untitled session"
                } else {
                    &thread.title
                },
                "",
            )
            .with_source(match thread.external_provider.as_deref() {
                Some("opencode") => neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::OpenCode,
                Some("claude") => neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::ClaudeCode,
                Some("codex") => neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::Codex,
                _ => neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::Neoism,
            })
            .with_directory(thread.directory.clone())
            .with_updated_ms(thread.updated_at)
            .with_pinned(thread.pinned)
            .with_runtime_status(thread.busy.then(|| "running".to_string()));
            entry.catalog_activity = thread.catalog_activity.as_ref().and_then(|activity| {
                serde_json::from_value(serde_json::Value::String(activity.clone())).ok()
            });
            entry
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

#[cfg(test)]
mod activity_tests {
    use super::*;
    use neoism_protocol::agent::{
        AgentServerMessage, HistoryMessage, HistoryMessageKind, Role, ToolStatus,
    };
    use neoism_ui::panels::agent_pane::state::side_panel::BranchStatus;
    use neoism_ui::panels::agent_pane::state::{
        NeoismAgentPane, NeoismAgentStreamingState,
    };
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn catalog_family_decodes_without_a_tab_or_pane_and_is_session_unscoped() {
        let message: AgentServerMessage = serde_json::from_value(serde_json::json!({
            "CatalogFamily": {
                "session_id": "unopened-root",
                "snapshot": {
                    "executionId": "execution-1",
                    "familyRevision": 7,
                    "directory": "/work/other",
                    "branches": [{
                        "sessionId": "child",
                        "parentSessionId": "unopened-root",
                        "status": "outstanding",
                        "startedAt": 123,
                        "title": "Background task",
                        "agent": "explore",
                        "depth": 1,
                        "permissionPending": true
                    }]
                }
            }
        })).unwrap();
        assert_eq!(agent_event_session_id(&message), None);
        let AgentServerMessage::CatalogFamily { session_id, snapshot } = message else {
            panic!("expected catalog family");
        };
        assert_eq!(session_id, "unopened-root");
        let snapshot: neoism_agent_core::CatalogFamilySnapshot =
            serde_json::from_value(snapshot).unwrap();
        assert_eq!(snapshot.execution_id.as_deref(), Some("execution-1"));
        assert_eq!(snapshot.family_revision, 7);
        assert_eq!(snapshot.directory, "/work/other");
        assert_eq!(snapshot.branches.len(), 1);
        assert_eq!(snapshot.branches[0].parent_session_id, "unopened-root");
        assert!(snapshot.branches[0].permission_pending);
    }

    #[wasm_bindgen_test]
    fn compact_admission_failure_event_rolls_back_only_matching_local_feedback() {
        use neoism_ui::panels::agent_pane::outbound::OutboundAgentCommand;
        let mut pane = NeoismAgentPane::default();
        pane.set_session_id(Some("root".into()));
        pane.note_streaming(NeoismAgentStreamingState::Generating, None);
        pane.insert_text("/compact");
        assert!(pane.submit());
        assert_eq!(pane.queued_prompt_count(), 1);
        assert_eq!(pane.queued_prompt_preview(), Some("Compaction queued"));
        let request_id = pane.drain_pending_outbound().into_iter().find_map(|command| {
            if let OutboundAgentCommand::CompactSessionRequest { request_id, .. } = command {
                Some(request_id)
            } else { None }
        }).unwrap();
        apply_agent_event_to_pane(&mut pane, AgentServerMessage::Notice {
            session_id: "root".into(), title: "Unrelated failure".into(), body: "error".into(),
            level: neoism_protocol::agent::NoticeLevel::Error,
        });
        assert_eq!(pane.queued_prompt_count(), 1);
        for id in ["old-request", request_id.as_str()] {
            apply_agent_event_to_pane(&mut pane, AgentServerMessage::SessionEvent {
                session_id: "root".into(), kind: "session.compaction.admission.failed".into(),
                properties: serde_json::json!({"requestID": id}),
            });
            assert_eq!(pane.queued_prompt_count(), usize::from(id == "old-request"));
        }
        assert_eq!(pane.queued_prompt_preview(), None);
        assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Generating);
    }

    #[wasm_bindgen_test]
    fn discovery_pins_preserve_old_source_identity_and_never_enter_scoped_catalogs() {
        use neoism_protocol::agent::AgentServerMessage as M;
        let pin = serde_json::json!({"id":"old-pin", "title":"Archived", "directory":"/archived",
            "time":{"updated":1}, "pinned":true, "catalogActivity":"permission",
            "externalAgent":{"provider":"codex", "sourceKey":"stable-key", "historyState":"neoism_only"}});
        let entry = session_entry_from_root(&pin).unwrap();
        assert_eq!(entry.directory.as_deref(), Some("/archived"));
        assert_eq!(entry.source_key.as_deref(), Some("stable-key"));
        assert_eq!(
            entry.source,
            neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::Codex
        );
        assert!(entry.pinned);
        let mut pane = NeoismAgentPane::default();
        pane.set_catalog_directory(Some("/scoped".into()));
        apply_agent_event_to_pane(
            &mut pane,
            M::SessionProjects {
                catalog_identity: "stale-root".into(),
                generation: 1,
                items: vec![],
                pinned_sessions: vec![pin],
            },
        );
        assert!(pane.side_panel().catalog_session("old-pin").is_none());
        assert!(!pane.side_panel().project_pagination_enabled());
    }

    #[wasm_bindgen_test]
    fn discovery_retry_policy_is_debounced_and_ready_refresh_failure_keeps_groups() {
        use super::project_discovery_request_due as due;
        use neoism_protocol::agent::{AgentServerMessage as M, SessionProjectSummary};
        assert!(due(true, false)); // endpoint/root change: initial discovery
        assert!(!due(false, false)); // in-flight/debounce or retry budget exhausted
        assert!(due(false, true)); // shared timer permits first failure retry
        let mut pane = NeoismAgentPane::default();
        pane.set_catalog_directory(None);
        pane.side_panel_mut().mark_refresh_kicked();
        apply_agent_event_to_pane(
            &mut pane,
            M::SessionProjectsFailed {
                catalog_identity: "test".into(),
                generation: 1,
                message: "503".into(),
            },
        );
        assert!(!pane.side_panel().should_refresh_sessions()); // not every frame
        apply_agent_event_to_pane(
            &mut pane,
            M::SessionProjects {
                catalog_identity: "test".into(),
                generation: 2,
                pinned_sessions: vec![],
                items: vec![SessionProjectSummary {
                    directory: "/a".into(),
                    updated_ms: 1,
                }],
            },
        );
        assert!(!pane.side_panel().should_refresh_sessions()); // Ready stops retrying
        apply_agent_event_to_pane(
            &mut pane,
            M::SessionProjectsFailed {
                catalog_identity: "test".into(),
                generation: 3,
                message: "503".into(),
            },
        );
        assert!(pane.side_panel().project_pagination_enabled());
        assert!(!pane.side_panel().should_refresh_sessions());
        assert_eq!(
            pane.side_panel_mut().begin_project_session_requests().len(),
            1
        );
    }

    #[wasm_bindgen_test]
    fn project_discovery_and_pages_use_independent_shared_requests() {
        use neoism_protocol::agent::{AgentServerMessage as M, SessionProjectSummary};
        let mut pane = NeoismAgentPane::default();
        pane.set_catalog_directory(None);
        pane.side_panel_mut().set_last_panel_height_rows(10);
        apply_agent_event_to_pane(
            &mut pane,
            M::SessionProjects {
                catalog_identity: "test".into(),
                generation: 7,
                pinned_sessions: vec![],
                items: vec![
                    SessionProjectSummary {
                        directory: "/a".into(),
                        updated_ms: 2,
                    },
                    SessionProjectSummary {
                        directory: "/b".into(),
                        updated_ms: 1,
                    },
                ],
            },
        );
        assert!(pane.side_panel().project_pagination_enabled());
        let requests = pane.side_panel_mut().begin_project_session_requests();
        assert_eq!(requests.len(), 2);
        for (directory, cursor, generation) in requests.into_iter().rev() {
            let threads = serde_json::from_value(serde_json::json!([{
                "session_id": format!("chat:{directory}"), "title": directory,
                "directory": directory, "updated_at": 2
            }]))
            .unwrap();
            apply_agent_event_to_pane(
                &mut pane,
                M::ProjectSessionPage {
                    catalog_identity: "test".into(),
                    directory: directory.clone(),
                    requested_cursor: cursor,
                    generation,
                    threads,
                    next_cursor: Some("next".into()),
                },
            );
            assert!(pane
                .side_panel()
                .catalog_session(&format!("chat:{directory}"))
                .is_some());
        }
        assert!(pane
            .side_panel_mut()
            .begin_project_session_requests()
            .is_empty());
    }

    #[wasm_bindgen_test]
    fn chat_directory_events_do_not_change_workspace_catalog_scope() {
        let mut pane = NeoismAgentPane::default();
        pane.set_catalog_directory(Some("/declared-workspace".into()));
        apply_agent_event_to_pane(
            &mut pane,
            AgentServerMessage::ThreadCreated {
                session_id: "chat-in-other-project".into(),
                title: Some("Other project".into()),
                directory: Some("/other-project".into()),
                agent: None,
                model: None,
            },
        );
        assert_eq!(
            pane.side_panel().catalog_directory(),
            Some("/declared-workspace")
        );
        pane.open_sessions_picker();
        assert!(pane.drain_pending_outbound().iter().any(|command| matches!(
            command,
            neoism_ui::panels::agent_pane::outbound::OutboundAgentCommand::RefreshSessions {
                directory: Some(directory), cursor: None,
            } if directory == "/declared-workspace"
        )));
    }

    #[wasm_bindgen_test]
    fn catalog_rows_preserve_root_and_external_preview_directories() {
        let threads = serde_json::from_value::<Vec<neoism_protocol::agent::ThreadSummary>>(
            serde_json::json!([
                { "session_id": "root", "title": "Root", "directory": "/root-project", "updated_at": 10 },
                { "session_id": "external", "title": "Preview", "directory": "/external-project", "updated_at": 9, "external_provider": "codex" },
                { "session_id": "legacy", "title": "Legacy", "updated_at": 8 }
            ]),
        ).unwrap();
        let entries = session_entries_from_catalog(&threads);
        assert_eq!(entries[0].directory.as_deref(), Some("/root-project"));
        assert_eq!(entries[1].directory.as_deref(), Some("/external-project"));
        assert_eq!(entries[2].directory, None);
        assert_eq!(
            entries[1].source,
            neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::Codex
        );
    }

    fn part(
        id: &str,
        kind: HistoryMessageKind,
        title: &str,
        tool: &str,
    ) -> HistoryMessage {
        HistoryMessage {
            id: id.to_string(),
            role: Role::Assistant,
            kind,
            author: None,
            title: title.to_string(),
            text: "chunk".to_string(),
            status: "running".to_string(),
            tool: tool.to_string(),
            tool_batch_id: None,
            mcp: None,
            lang: String::new(),
            line_offset: None,
            detail: String::new(),
            todos: Vec::new(),
            usage: None,
            created_at: 0,
            attachments: Vec::new(),
        }
    }

    #[wasm_bindgen_test]
    fn ordered_daemon_messages_match_desktop_activity_semantics() {
        let mut pane = NeoismAgentPane::default();
        pane.set_session_id(Some("root".to_string()));
        apply_agent_event_to_pane(
            &mut pane,
            AgentServerMessage::QueueUpdate {
                session_id: "root".to_string(),
                count: 1,
                preview: Some("queued".to_string()),
                started_at: Some(1),
            },
        );
        assert_eq!(
            pane.streaming_state(),
            NeoismAgentStreamingState::Generating
        );

        for (index, (kind, expected, title, tool)) in [
            (
                HistoryMessageKind::Assistant,
                NeoismAgentStreamingState::Generating,
                "",
                "",
            ),
            (
                HistoryMessageKind::Reasoning,
                NeoismAgentStreamingState::Thinking,
                "",
                "",
            ),
            (
                HistoryMessageKind::Tool,
                NeoismAgentStreamingState::Working,
                "Search",
                "grep",
            ),
            (
                HistoryMessageKind::Assistant,
                NeoismAgentStreamingState::Generating,
                "",
                "",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            apply_agent_event_to_pane(
                &mut pane,
                AgentServerMessage::MessageUpdated {
                    session_id: "root".to_string(),
                    parent_message_id: None,
                    message: part(&format!("part-{index}"), kind, title, tool),
                },
            );
            assert_eq!(pane.streaming_state(), expected);
            apply_agent_event_to_pane(
                &mut pane,
                AgentServerMessage::QueueUpdate {
                    session_id: "root".to_string(),
                    count: 1,
                    preview: Some("queued".to_string()),
                    started_at: Some(1),
                },
            );
            assert_eq!(pane.streaming_state(), expected);
        }
    }

    #[wasm_bindgen_test]
    fn history_seeds_do_not_revive_completed_catalog_children() {
        use neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry;
        let mut pane = NeoismAgentPane::default();
        pane.detail_panel_mut().set_user_hidden(true);
        pane.side_panel_mut()
            .set_sessions(vec![NeoismAgentSessionEntry::new("root", "Parent", "")]);
        let mut task = part("task-card", HistoryMessageKind::Tool, "Research", "task");
        task.detail = "task_id: historical-child\nstatus: completed\ndone".into();
        task.status = "completed".into();
        apply_agent_event_to_pane(
            &mut pane,
            AgentServerMessage::HistoryChunk {
                session_id: "root".into(),
                messages: vec![task],
                next_cursor: None,
            },
        );
        assert_eq!(
            pane.side_panel()
                .branch_activity("historical-child")
                .map(|activity| activity.status),
            Some(BranchStatus::Completed),
        );
        assert!(!pane
            .side_panel()
            .sessions()
            .iter()
            .any(|row| row.id == "historical-child"));
        assert!(pane.detail_panel().last_panel_rect().is_none());
    }

    #[wasm_bindgen_test]
    fn hidden_detail_rail_live_branch_updates_project_into_catalog() {
        use neoism_protocol::agent::SubagentStatus;
        use neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry;
        let mut pane = NeoismAgentPane::default();
        pane.set_session_id(Some("root".into()));
        pane.detail_panel_mut().set_user_hidden(true);
        pane.side_panel_mut()
            .set_sessions(vec![NeoismAgentSessionEntry::new("root", "Parent", "")]);
        for (status, expected) in [
            (SubagentStatus::Running, BranchStatus::Active),
            (SubagentStatus::Completed, BranchStatus::Completed),
        ] {
            let event = AgentServerMessage::SubagentUpdate {
                session_id: "child".into(),
                status,
                title: Some("Research".into()),
                agent: Some("explore".into()),
                current_tool: None,
                started_at: None,
                parent_session_id: Some("root".into()),
                root_session_id: Some("root".into()),
                execution_id: None,
                family_revision: None,
            };
            if status == SubagentStatus::Completed {
                // A sibling's lifecycle bypasses the viewed-session gate and
                // must still update the workspace tree via the cache path.
                assert!(apply_agent_event_to_cache(&mut pane, event));
            } else {
                apply_agent_event_to_pane(&mut pane, event);
            }
            let child = pane
                .side_panel()
                .sessions()
                .iter()
                .find(|row| row.id == "child")
                .unwrap();
            assert_eq!(child.branch_root_id.as_deref(), Some("root"));
            assert_eq!(child.branch_status, Some(expected));
        }
    }

    #[wasm_bindgen_test]
    fn terminal_task_result_clears_last_child_without_execution_event() {
        let mut pane = NeoismAgentPane::default();
        pane.set_session_id(Some("root".to_string()));
        pane.note_subagent_event(
            "child".to_string(),
            BranchStatus::Active,
            None,
            None,
            None,
            None,
        );
        assert_eq!(pane.streaming_label(), "Sub-agents working");

        apply_agent_event_to_pane(
            &mut pane,
            AgentServerMessage::ToolUseResult {
                session_id: "root".to_string(),
                tool_use_id: "task-tool".to_string(),
                tool: "task".to_string(),
                status: ToolStatus::Completed,
                output: Some("task_id: child\nstatus: completed\ndone".to_string()),
                error: None,
            },
        );

        assert_eq!(pane.streaming_label(), "");
        assert!(!pane.has_status_activity());
    }

    #[wasm_bindgen_test]
    fn late_child_metadata_does_not_recreate_finished_sidebar_activity() {
        let mut pane = NeoismAgentPane::default();
        pane.set_session_id(Some("root".to_string()));
        pane.note_subagent_event(
            "child".to_string(),
            BranchStatus::Active,
            Some("Original".to_string()),
            Some("explore".to_string()),
            None,
            None,
        );
        pane.note_subagent_event(
            "child".to_string(),
            BranchStatus::Completed,
            None,
            None,
            None,
            None,
        );

        apply_agent_event_to_pane(
            &mut pane,
            AgentServerMessage::SubagentMetadata {
                session_id: "child".to_string(),
                title: Some("Late rename".to_string()),
                agent: Some("build".to_string()),
                parent_session_id: Some("root".to_string()),
            },
        );

        assert_eq!(pane.streaming_label(), "");
        assert!(!pane.has_status_activity());
    }
}
