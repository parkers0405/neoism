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

/// Agent event dispatch and catalog projections live in the shared crate so
/// the GPUIX host runs the identical pane updates.
pub(crate) use neoism_ui::panels::agent_pane::server_events::*;

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
