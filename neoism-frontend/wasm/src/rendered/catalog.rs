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
        HistoryMessage, HistoryMessageKind, NoticeLevel, Role,
        StreamingState as ProtoStreamingState, SubagentStatus, ToolStatus, Usage,
    };
    use neoism_ui::panels::agent_pane::state::side_panel::BranchStatus;
    use neoism_ui::panels::agent_pane::state::{
        CompactionPhase, NeoismAgentMessage, NeoismAgentMessageKind,
        NeoismAgentNoticeLevel, NeoismAgentOutputKind, NeoismAgentPendingPermission,
        NeoismAgentStreamingState, NeoismAgentTodo, NeoismAgentUsage,
    };

    fn map_usage(u: Usage) -> NeoismAgentUsage {
        NeoismAgentUsage {
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
    fn map_history_kind(k: HistoryMessageKind) -> NeoismAgentMessageKind {
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
    fn map_history(m: HistoryMessage) -> NeoismAgentMessage {
        let kind = map_history_kind(m.kind);
        let output_kind = match kind {
            NeoismAgentMessageKind::Tool if !m.lang.is_empty() => {
                NeoismAgentOutputKind::Code
            }
            _ => NeoismAgentOutputKind::Text,
        };
        NeoismAgentMessage {
            id: m.id,
            kind,
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
        }
    }
    fn map_streaming(s: ProtoStreamingState) -> NeoismAgentStreamingState {
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
        AgentServerMessage::ThreadList { threads } => {
            let current_session_id = pane.session_id_str().map(str::to_string);
            pane.set_session_options(session_options_from_catalog(
                &threads,
                current_session_id.as_deref(),
            ));
            pane.side_panel_mut()
                .set_sessions(session_entries_from_catalog(&threads));
        }
        AgentServerMessage::HistoryChunk {
            session_id,
            messages,
            ..
        } => {
            pane.set_session_id(Some(session_id));
            pane.apply_history(messages.into_iter().map(map_history).collect());
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
        AgentServerMessage::StreamingState { state, label, .. } => {
            pane.note_streaming(map_streaming(state), label);
        }
        AgentServerMessage::Notice {
            title, body, level, ..
        } => {
            pane.push_notice_event(title, body, map_notice_level(level));
        }
        AgentServerMessage::CommandOutput { title, body, .. } => {
            pane.system_message(title, body);
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
        AgentServerMessage::ToolUseResult {
            tool_use_id,
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
            pane.remove_pending_permission(&tool_use_id);
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
            agent,
            thinking,
            context_limit,
            ..
        } => {
            pane.apply_provider_state(provider_id, model, agent, thinking, context_limit);
        }
        AgentServerMessage::ProviderCatalog { providers } => {
            pane.set_model_options(model_options_from_catalog(&providers));
        }
        AgentServerMessage::ConfigDefaults {
            agent,
            model,
            thinking,
        } => {
            pane.apply_provider_state(None, model, agent, thinking, None);
        }
        AgentServerMessage::AgentCatalog { agents } => {
            pane.set_agent_options(agent_options_from_catalog(&agents));
        }
        AgentServerMessage::SkillCatalog { skills } => {
            pane.set_skill_options(skill_options_from_catalog(&skills));
        }
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
        } => {
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
        AgentServerMessage::ConnectOauthUrl {
            url,
            auto,
            instructions,
        } => {
            pane.apply_connect_oauth_url(url, auto, instructions);
        }
        AgentServerMessage::ConnectFinished { provider } => {
            pane.note_connect_finished(provider);
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
        | AgentServerMessage::StreamingState { session_id, .. }
        | AgentServerMessage::Notice { session_id, .. }
        | AgentServerMessage::ToolUseRequest { session_id, .. }
        | AgentServerMessage::ToolUseResult { session_id, .. }
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
        AgentServerMessage::Disabled { .. }
        | AgentServerMessage::PermissionRequest { .. }
        | AgentServerMessage::Error { .. }
        | AgentServerMessage::ThreadList { .. }
        | AgentServerMessage::ProviderCatalog { .. }
        | AgentServerMessage::ConfigDefaults { .. }
        | AgentServerMessage::AgentCatalog { .. }
        | AgentServerMessage::SkillCatalog { .. }
        | AgentServerMessage::ConnectProviderCatalog { .. }
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

pub(crate) fn agent_options_from_catalog(
    agents: &[neoism_protocol::agent::AgentInfo],
) -> Vec<neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption> {
    use neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerOption;

    let mut out = vec![NeoismAgentPickerOption::new(
        "session default",
        "Use Neoism Agent default",
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
    // headers itself), carrying the raw timestamp + pin flag.
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
        })
        .collect()
}
