
use super::*;
use crate::panels::agent_pane::outbound::OutboundAgentCommand;
use crate::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry;

#[test]
fn runtime_branch_status_policy_maps_daemon_statuses() {
    assert_eq!(
        branch_status_from_runtime("completed"),
        BranchStatus::Completed
    );
    assert_eq!(branch_status_from_runtime("idle"), BranchStatus::Completed);
    assert_eq!(
        branch_status_from_runtime("blocked"),
        BranchStatus::WaitingPermission
    );
    assert_eq!(
        branch_status_from_runtime("retry"),
        BranchStatus::WaitingPermission
    );
    assert_eq!(branch_status_from_runtime("error"), BranchStatus::Stopped);
    assert_eq!(branch_status_from_runtime("failed"), BranchStatus::Stopped);
    assert_eq!(branch_status_from_runtime("stopped"), BranchStatus::Stopped);
    assert_eq!(branch_status_from_runtime("running"), BranchStatus::Active);
    assert_eq!(branch_status_from_runtime("unknown"), BranchStatus::Active);
}

#[test]
fn runtime_task_message_status_policy_maps_known_statuses() {
    assert_eq!(
        task_message_status_from_runtime("completed"),
        Some("completed")
    );
    assert_eq!(task_message_status_from_runtime("idle"), Some("completed"));
    assert_eq!(task_message_status_from_runtime("error"), Some("error"));
    assert_eq!(task_message_status_from_runtime("stopped"), Some("error"));
    assert_eq!(task_message_status_from_runtime("failed"), Some("error"));
    assert_eq!(task_message_status_from_runtime("running"), Some("running"));
    assert_eq!(task_message_status_from_runtime("active"), Some("running"));
    assert_eq!(task_message_status_from_runtime("busy"), Some("running"));
    assert_eq!(task_message_status_from_runtime("blocked"), Some("running"));
    assert_eq!(task_message_status_from_runtime("retry"), Some("running"));
    assert_eq!(task_message_status_from_runtime("unknown"), None);
}

#[test]
fn pending_outbound_starts_empty() {
    let mut pane = NeoismAgentPane::default();
    assert!(!pane.has_pending_outbound());
    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn expired_wordmark_click_does_not_drive_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.wordmark.click_started = Some(
        Instant::now()
            .checked_sub(WORDMARK_CLICK_ANIMATION + Duration::from_millis(1))
            .unwrap(),
    );

    assert_ne!(pane.animation_reason(), Some("wordmark"));
}

#[test]
fn fresh_wordmark_click_drives_short_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.wordmark.click_started = Some(Instant::now());

    assert_eq!(pane.animation_reason(), Some("wordmark"));
}

#[test]
fn drain_pending_outbound_preserves_fifo_order_and_is_one_shot() {
    let mut pane = NeoismAgentPane::default();
    let commands = vec![
        OutboundAgentCommand::EnsureSession,
        OutboundAgentCommand::SwitchSession {
            session_id: "sess-1".to_string(),
        },
        OutboundAgentCommand::SlashCommand {
            name: "login".to_string(),
            args: "token=abc".to_string(),
        },
    ];
    for command in commands.clone() {
        pane.push_outbound(command);
    }

    assert!(pane.has_pending_outbound());
    assert_eq!(pane.drain_pending_outbound(), commands);
    assert!(!pane.has_pending_outbound());
    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn typing_slash_opens_command_picker_with_options() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("/");
    let picker = pane.picker().expect("slash picker opens on /");
    assert_eq!(picker.kind, NeoismAgentPickerKind::Slash);
    assert!(
        !picker.options().is_empty(),
        "slash options must list commands"
    );
    // Filtering by a command prefix keeps matches.
    pane.insert_text("mo");
    let picker = pane.picker().expect("picker stays open while filtering");
    assert!(!picker.options().is_empty(), "/mo should match /model");
}

#[test]
fn model_picker_headers_are_not_selectable() {
    let mut picker = NeoismAgentPicker::new(
        NeoismAgentPickerKind::Model,
        "Select model",
        vec![
            NeoismAgentPickerOption::header("OpenCode Zen"),
            NeoismAgentPickerOption::model(
                "Big Pickle",
                "OpenCode Zen",
                "Free",
                "opencode/big-pickle",
            ),
            NeoismAgentPickerOption::header("OpenAI"),
            NeoismAgentPickerOption::model("GPT-5", "OpenAI", "128k ctx", "openai/gpt-5"),
        ],
        0,
    );

    assert_eq!(
        picker.selected_option().map(|option| option.value.as_str()),
        Some("opencode/big-pickle")
    );
    picker.move_selection(1);
    assert_eq!(
        picker.selected_option().map(|option| option.value.as_str()),
        Some("openai/gpt-5")
    );
    picker.move_selection(-1);
    assert_eq!(
        picker.selected_option().map(|option| option.value.as_str()),
        Some("opencode/big-pickle")
    );
}

#[test]
fn typing_skill_mention_keeps_query_visible_in_input() {
    let mut pane = NeoismAgentPane::default();

    pane.insert_text("$");
    pane.insert_text("neo");

    assert_eq!(pane.input(), "$neo");
    assert_eq!(pane.cursor_byte(), "$neo".len());
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention));
}

#[test]
fn sessions_picker_requests_refresh_and_shows_loading_until_catalog_arrives() {
    let mut pane = NeoismAgentPane::default();
    let _ = pane.drain_pending_outbound();

    pane.open_sessions_picker();

    let picker = pane.picker().expect("sessions picker");
    assert_eq!(picker.kind, NeoismAgentPickerKind::Session);
    assert_eq!(picker.options()[0].title, "Loading sessions...");
    assert!(matches!(
        pane.drain_pending_outbound().as_slice(),
        [OutboundAgentCommand::RefreshSessions { .. }]
    ));
}

#[test]
fn sessions_catalog_replaces_open_loading_picker() {
    let mut pane = NeoismAgentPane::default();
    let _ = pane.drain_pending_outbound();
    pane.open_sessions_picker();

    pane.set_session_options(vec![NeoismAgentPickerOption::new(
        "Build web agent",
        "",
        "just now",
        "sess-1",
    )]);

    let picker = pane.picker().expect("sessions picker");
    assert_eq!(picker.kind, NeoismAgentPickerKind::Session);
    assert_eq!(picker.options().len(), 1);
    assert_eq!(picker.options()[0].value, "sess-1");
}

#[test]
fn submit_plain_prompt_queues_send_prompt_with_ensure_session_when_no_session() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("hello world");
    pane.submit();

    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 2, "expected EnsureSession + SendPrompt");
    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| message.kind == NeoismAgentMessageKind::User)
            .count(),
        1,
        "submit should append the user prompt exactly once"
    );
    assert!(
        matches!(drained[0], OutboundAgentCommand::EnsureSession),
        "first command should be EnsureSession when no session yet"
    );
    match &drained[1] {
        OutboundAgentCommand::SendPrompt {
            text,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "hello world");
            assert!(
                *transcript_echo,
                "idle submissions should be echoed into the transcript"
            );
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
    assert!(!pane.has_pending_outbound(), "drain should empty the queue");
}

#[test]
fn submit_plain_prompt_skips_ensure_session_when_session_exists() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-1".to_string()));
    // The first drain may surface an `ApplyConfigDefaults` from a
    // construction-time call; we don't care about that here.
    let _ = pane.drain_pending_outbound();
    pane.insert_text("hi");
    pane.submit();

    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 1);
    match &drained[0] {
        OutboundAgentCommand::SendPrompt {
            text,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "hi");
            assert!(
                *transcript_echo,
                "idle submissions should be echoed into the transcript"
            );
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
}

#[test]
fn submit_pasted_text_expands_outbound_but_keeps_transcript_token() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-1".to_string()));
    let _ = pane.drain_pending_outbound();
    pane.insert_paste("first line\nsecond line");

    assert!(pane.submit());

    let drained = pane.drain_pending_outbound();
    match &drained[0] {
        OutboundAgentCommand::SendPrompt {
            text,
            parts,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "first line\nsecond line");
            assert_eq!(parts[0]["text"], "first line\nsecond line");
            assert!(*transcript_echo);
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
    assert_eq!(pane.messages[0].text, "[pasted 2 lines]");
    assert_eq!(pane.pending_user_prompts, vec!["[pasted 2 lines]"]);
}

#[test]
fn submit_plain_prompt_while_streaming_queues_without_transcript_echo() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-1".to_string()));
    let _ = pane.drain_pending_outbound();
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("still running"),
    ];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.insert_text("queued turn");

    assert!(pane.submit());

    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| is_user_prompt(message, "queued turn"))
            .count(),
        0,
        "queued submissions should stay out of the transcript until dequeue"
    );
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(pane.queued_prompt_preview.as_deref(), Some("queued turn"));
    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 1);
    match &drained[0] {
        OutboundAgentCommand::SendPrompt {
            text,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "queued turn");
            assert!(
                !*transcript_echo,
                "streaming submissions should remain in the queue preview"
            );
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
}

#[test]
fn dequeued_prompt_consumes_preview_and_appends_once() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("done"),
    ];
    pane.queued_prompt_count = 1;
    pane.queued_prompt_preview = Some("queued turn".to_string());

    pane.note_dequeued_prompt("queued turn".to_string());
    pane.note_dequeued_prompt("queued turn".to_string());

    assert_eq!(pane.queued_prompt_count, 0);
    assert_eq!(pane.queued_prompt_preview, None);
    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| is_user_prompt(message, "queued turn"))
            .count(),
        1
    );
    assert_eq!(pane.messages.last().unwrap().text, "queued turn");
}

#[test]
fn slash_compact_queues_compact_session_only_with_session() {
    // Call `execute_slash_text` directly to bypass the slash
    // picker (which `submit()` commits to its own selection). The
    // dispatcher itself is the unit under test.
    let mut pane = NeoismAgentPane::default();
    pane.execute_slash_text("/compact");
    assert!(pane.drain_pending_outbound().is_empty());

    pane.set_session_id(Some("sess-1".to_string()));
    pane.execute_slash_text("/compact");
    let drained = pane.drain_pending_outbound();
    assert!(
        drained
            .iter()
            .any(|cmd| matches!(cmd, OutboundAgentCommand::CompactSession)),
        "expected CompactSession in drain: {drained:?}",
    );
}

#[test]
fn slash_abort_queues_abort_session_only_with_session() {
    let mut pane = NeoismAgentPane::default();
    pane.execute_slash_text("/abort");
    assert!(pane.drain_pending_outbound().is_empty());

    pane.set_session_id(Some("sess-1".to_string()));
    pane.execute_slash_text("/abort");
    let drained = pane.drain_pending_outbound();
    assert!(
        drained
            .iter()
            .any(|cmd| matches!(cmd, OutboundAgentCommand::AbortSession)),
        "expected AbortSession in drain: {drained:?}",
    );
}

#[test]
fn unknown_slash_command_is_queued_as_slash_command() {
    let mut pane = NeoismAgentPane::default();
    pane.execute_slash_text("/login token=abc");
    let drained = pane.drain_pending_outbound();
    let queued = drained
        .iter()
        .find_map(|cmd| match cmd {
            OutboundAgentCommand::SlashCommand { name, args } => {
                Some((name.clone(), args.clone()))
            }
            _ => None,
        })
        .expect("expected SlashCommand entry");
    assert_eq!(queued.0, "login");
    assert_eq!(queued.1, "token=abc");
}

#[test]
fn slash_clear_is_inline_with_no_outbound_command() {
    let mut pane = NeoismAgentPane::default();
    pane.messages
        .push(NeoismAgentMessage::user("first".to_string()));
    pane.execute_slash_text("/clear");
    assert!(pane.messages.is_empty());
    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn switch_session_queues_switch_session_command() {
    let mut pane = NeoismAgentPane::default();
    pane.switch_session("sess-77".to_string());
    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 1);
    match &drained[0] {
        OutboundAgentCommand::SwitchSession { session_id } => {
            assert_eq!(session_id, "sess-77");
        }
        other => panic!("expected SwitchSession, got {other:?}"),
    }
}

#[test]
fn with_directory_queues_apply_config_defaults() {
    let mut pane = NeoismAgentPane::with_directory(Some("/tmp/wd".to_string()));
    let drained = pane.drain_pending_outbound();
    assert!(
        drained
            .iter()
            .any(|cmd| matches!(cmd, OutboundAgentCommand::ApplyConfigDefaults)),
        "expected ApplyConfigDefaults from with_directory: {drained:?}",
    );
}

#[test]
fn idle_streaming_state_clears_status_label() {
    let mut pane = NeoismAgentPane::default();

    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert!(pane.is_streaming());
    assert_eq!(pane.streaming_label(), "Crafting");

    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert!(!pane.is_streaming());
    assert_eq!(pane.streaming_label(), "");
    assert_eq!(pane.streaming_elapsed_seconds(), None);
}

#[test]
fn stale_idle_snapshot_keeps_streamed_assistant_text_by_id() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("what"),
        NeoismAgentMessage::assistant("streamed final answer").with_id("part-1"),
    ];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    let refreshed = pane.preserve_streamed_response_text(vec![
        NeoismAgentMessage::user("what"),
        NeoismAgentMessage::assistant("").with_id("part-1"),
    ]);

    assert_eq!(refreshed.len(), 2);
    assert_eq!(refreshed[1].text, "streamed final answer");
}

#[test]
fn stale_idle_snapshot_does_not_append_orphan_streamed_assistant_tail() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("what"),
        NeoismAgentMessage::assistant("streamed final answer").with_id("part-1"),
    ];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    let refreshed =
        pane.preserve_streamed_response_text(vec![NeoismAgentMessage::user("what")]);

    assert_eq!(refreshed, vec![NeoismAgentMessage::user("what")]);
}

#[test]
fn stale_idle_snapshot_does_not_append_orphan_streamed_tool_card() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("edit"),
        NeoismAgentMessage::tool(
            "ApplyPatch(src/lib.rs)",
            "applying patch",
            "running",
            "apply_patch",
            NeoismAgentOutputKind::Text,
            "rust",
            Vec::new(),
        )
        .with_id("tool-1"),
    ];
    pane.note_streaming(
        NeoismAgentStreamingState::Working,
        Some("ApplyPatch".to_string()),
    );

    let refreshed =
        pane.preserve_streamed_response_text(vec![NeoismAgentMessage::user("edit")]);

    assert_eq!(refreshed, vec![NeoismAgentMessage::user("edit")]);
}

#[test]
fn history_refresh_keeps_server_order_for_late_user_and_reasoning() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::reasoning("old local thought").with_id("reasoning-1"),
        NeoismAgentMessage::assistant("old local final").with_id("answer-1"),
    ];

    let server_messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::reasoning("server thought").with_id("reasoning-1"),
        NeoismAgentMessage::assistant("server final").with_id("answer-1"),
        NeoismAgentMessage::user("second"),
        NeoismAgentMessage::reasoning("server thought 2").with_id("reasoning-2"),
        NeoismAgentMessage::assistant("server final 2").with_id("answer-2"),
    ];

    let refreshed = pane.preserve_streamed_response_text(server_messages);

    assert_eq!(refreshed[3].kind, NeoismAgentMessageKind::User);
    assert_eq!(refreshed[3].text, "second");
    assert_eq!(refreshed[4].kind, NeoismAgentMessageKind::Reasoning);
    assert_eq!(refreshed[5].kind, NeoismAgentMessageKind::Assistant);
}

#[test]
fn history_refresh_does_not_duplicate_compaction_summary() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("compact this"),
        NeoismAgentMessage::compaction("summary body", "auto").with_id("compaction-1"),
    ];

    let refreshed = pane.preserve_streamed_response_text(vec![
        NeoismAgentMessage::user("compact this"),
        NeoismAgentMessage::assistant("summary body").with_id("server-summary"),
    ]);

    assert_eq!(refreshed.len(), 2);
    assert_eq!(
        refreshed
            .iter()
            .filter(|message| message.text == "summary body")
            .count(),
        1
    );
}

#[test]
fn subagent_rehydrate_does_not_complete_task_without_explicit_status() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "running")];

    pane.reconcile_task_message_statuses();

    assert_eq!(pane.messages[0].status, "running");
    assert!(pane.messages[0].detail.contains("status: running"));
}

#[test]
fn subagent_rehydrate_completes_task_from_explicit_child_status() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "running")];
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child-1", "child", "codex")
            .with_runtime_status(Some("completed".to_string())),
    ]);

    pane.reconcile_task_message_statuses();

    assert_eq!(pane.messages[0].status, "completed");
    assert!(pane.messages[0].detail.contains("status: completed"));
}

#[test]
fn stale_active_subagent_id_does_not_revert_completed_task_to_running() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "running")];
    pane.active_subagent_ids.insert("child-1".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child-1", "child", "codex")
            .with_runtime_status(Some("completed".to_string())),
    ]);

    pane.reconcile_task_message_statuses();

    assert_eq!(pane.messages[0].status, "completed");
    assert!(pane.messages[0].detail.contains("status: completed"));
}

#[test]
fn later_stale_running_task_snapshot_does_not_replace_completed_card() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "completed")];

    let refreshed = pane
        .preserve_streamed_response_text(vec![task_tool_message("child-1", "running")]);

    assert_eq!(refreshed[0].status, "completed");
    assert!(refreshed[0].detail.contains("status: completed"));
}

#[test]
fn subagent_rehydrate_resets_reused_child_task_to_running() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "completed")];
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child-1", "child", "codex")
            .with_runtime_status(Some("running".to_string())),
    ]);

    pane.reconcile_task_message_statuses();

    assert_eq!(pane.messages[0].status, "running");
    assert!(pane.messages[0].detail.contains("status: running"));
}

#[test]
fn permission_selection_moves_in_visual_order() {
    let mut pane = NeoismAgentPane::default();
    pane.pending_permission = Some(test_permission(0));

    pane.move_permission_selection(-1);
    assert_eq!(pane.pending_permission.as_ref().unwrap().selected, 1);

    pane.move_permission_selection(1);
    assert_eq!(pane.pending_permission.as_ref().unwrap().selected, 0);

    pane.move_permission_selection(1);
    assert_eq!(pane.pending_permission.as_ref().unwrap().selected, 2);

    pane.move_permission_selection(1);
    assert_eq!(pane.pending_permission.as_ref().unwrap().selected, 1);
}

#[test]
fn permission_reply_queues_outbound_and_marks_responding() {
    let mut pane = NeoismAgentPane::default();
    pane.pending_permission = Some(test_permission(0));

    assert!(pane.respond_pending_permission(NeoismAgentPermissionChoice::Always));

    assert!(pane.pending_permission.as_ref().unwrap().responding);
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::ReplyPermission {
            id: "perm-1".to_string(),
            reply: "always".to_string(),
        }]
    );
}

#[test]
fn permission_reply_completion_clears_or_reenables_permission() {
    let mut pane = NeoismAgentPane::default();
    pane.pending_permission = Some(test_permission(0));
    pane.respond_pending_permission(NeoismAgentPermissionChoice::Reject);

    assert!(pane.permission_reply_failed("perm-1", "network down"));
    assert!(!pane.pending_permission.as_ref().unwrap().responding);
    assert_eq!(
        pane.messages.last().map(|message| message.title.as_str()),
        Some("Permission")
    );

    assert!(pane.respond_pending_permission(NeoismAgentPermissionChoice::Once));
    assert!(pane.permission_reply_succeeded("perm-1", "once"));
    assert!(pane.pending_permission.is_none());
}

#[test]
fn stale_idle_snapshot_keeps_streamed_tool_detail_by_id() {
    let mut pane = NeoismAgentPane::default();
    let mut streamed = NeoismAgentMessage::tool(
        "Edit(src/lib.rs)",
        "src/lib.rs",
        "running",
        "edit",
        NeoismAgentOutputKind::Text,
        "rust",
        Vec::new(),
    )
    .with_id("tool-1");
    streamed.detail =
        r#"{"neoismToolDetail":"edit","metadata":{"snapshots":[]}}"#.to_string();
    pane.messages = vec![NeoismAgentMessage::user("edit"), streamed];
    pane.note_streaming(NeoismAgentStreamingState::Working, Some("Edit".to_string()));

    let refreshed = pane.preserve_streamed_response_text(vec![
        NeoismAgentMessage::user("edit"),
        NeoismAgentMessage::tool(
            "Edit(src/lib.rs)",
            "completed",
            "completed",
            "edit",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-1"),
    ]);

    let tool = refreshed
        .iter()
        .find(|message| message.id == "tool-1")
        .expect("tool is preserved");
    assert_eq!(tool.status, "completed");
    assert!(tool.detail.contains("neoismToolDetail"));
    assert_eq!(tool.lang, "rust");
}

#[test]
fn terminal_tool_part_without_matching_id_replaces_single_running_tool() {
    let mut pane = NeoismAgentPane::default();
    pane.upsert_tool_card(
        "call-edit".to_string(),
        "edit".to_string(),
        "Edit(src/lib.rs)".to_string(),
        "running".to_string(),
        String::new(),
        NeoismAgentOutputKind::Text,
        "text".to_string(),
    );

    let completed = NeoismAgentMessage::tool(
        "Edit(src/lib.rs)",
        "Updated src/lib.rs",
        "completed",
        "edit",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    )
    .with_id("part-edit".to_string());
    pane.upsert_part_message(completed);

    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].id, "part-edit");
    assert_eq!(pane.messages[0].status, "completed");
    assert_eq!(pane.messages[0].text, "Updated src/lib.rs");
}

#[test]
fn compaction_lifecycle_events_do_not_create_messages() {
    let mut pane = NeoismAgentPane::default();

    pane.note_compaction(CompactionPhase::Started, None, Some("auto".to_string()));
    assert!(pane.messages.is_empty());
    assert_eq!(pane.streaming_label(), "Compacting");

    pane.note_compaction(CompactionPhase::Delta, Some("summary".to_string()), None);
    pane.note_compaction(
        CompactionPhase::Ended,
        Some("summary".to_string()),
        Some("model".to_string()),
    );
    assert!(pane.messages.is_empty());
}

#[test]
fn persisted_compaction_is_only_compaction_message_source() {
    let mut pane = NeoismAgentPane::default();

    pane.note_compaction(CompactionPhase::Started, None, Some("auto".to_string()));
    pane.note_compaction(
        CompactionPhase::Delta,
        Some("event delta".to_string()),
        None,
    );
    assert!(pane.messages.is_empty());

    pane.upsert_part_message(
        NeoismAgentMessage::compaction("", "summary").with_id("assistant-compaction"),
    );
    pane.apply_part_delta(
        Some("assistant-compaction".to_string()),
        Some("text-part".to_string()),
        Some("text".to_string()),
        "real summary",
    );
    pane.note_compaction(
        CompactionPhase::Delta,
        Some("event delta tail".to_string()),
        None,
    );
    pane.note_compaction(
        CompactionPhase::Ended,
        Some("compaction done\ncompaction summary\ncompaction model".to_string()),
        Some("model".to_string()),
    );

    let compactions: Vec<_> = pane
        .messages
        .iter()
        .filter(|message| message.kind == NeoismAgentMessageKind::Compaction)
        .collect();
    assert_eq!(compactions.len(), 1);
    assert_eq!(compactions[0].id, "assistant-compaction");
    assert_eq!(compactions[0].text, "real summary");
}

#[test]
fn history_refresh_replaces_legacy_compaction_with_persisted_summary() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("compact"),
        NeoismAgentMessage::compaction("stale local summary", "auto")
            .with_id("compaction-1"),
    ];

    let refreshed = pane.preserve_streamed_response_text(vec![
        NeoismAgentMessage::user("compact"),
        NeoismAgentMessage::compaction("real summary", "summary")
            .with_id("assistant-compaction"),
    ]);

    let compactions: Vec<_> = refreshed
        .iter()
        .filter(|message| message.kind == NeoismAgentMessageKind::Compaction)
        .collect();
    assert_eq!(compactions.len(), 1);
    assert_eq!(compactions[0].id, "assistant-compaction");
    assert_eq!(compactions[0].text, "real summary");
}

#[test]
fn running_background_task_count_tracks_started_and_collected_jobs() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "running",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);
    pane.refresh_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 1);
    assert!(pane.has_status_activity());
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::BackgroundTasks
    );
    assert_eq!(pane.streaming_label(), "Background");
    assert!(pane.streaming_elapsed_seconds().is_some());
    assert!(!pane.background_task_details_expanded());
    assert_eq!(
        pane.active_background_task_summaries(),
        vec!["job-1 · running · cargo build".to_string()]
    );
    let mut result = NeoismAgentMessage::tool(
        "Background Task Result",
        "job_id: job-1\nstatus: completed",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    result.detail = result.text.clone();
    pane.messages.push(result);
    pane.refresh_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.background_task_details_expanded());
}

#[test]
fn completed_background_task_tool_is_not_counted_as_running() {
    let mut pane = NeoismAgentPane::default();
    let mut task = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: completed\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    task.detail = task.text.clone();
    pane.messages.push(task);
    pane.refresh_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_state_changed_elapsed(), None);
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn runtime_background_finish_notice_clears_running_job() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "running",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);

    let mut notice = NeoismAgentMessage::assistant(
            "A background shell task has finished.\njob_id: job-1\nstatus: completed\ncommand: cargo build",
        );
    notice.detail = notice.text.clone();
    pane.messages.push(notice);
    pane.refresh_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_state_changed_elapsed(), None);
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn background_status_is_scoped_to_pane_session_messages() {
    let mut pane_with_job = NeoismAgentPane::default();
    pane_with_job.session_id = Some("session-a".to_string());
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "running",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane_with_job.messages.push(started);
    pane_with_job.refresh_background_task_activity_clock();

    let mut other_pane = NeoismAgentPane::default();
    other_pane.session_id = Some("session-b".to_string());
    other_pane.refresh_background_task_activity_clock();

    assert_eq!(pane_with_job.running_background_task_count(), 1);
    assert_eq!(
        pane_with_job.streaming_state(),
        NeoismAgentStreamingState::BackgroundTasks
    );
    assert_eq!(other_pane.running_background_task_count(), 0);
    assert_eq!(
        other_pane.streaming_state(),
        NeoismAgentStreamingState::Idle
    );
}

#[test]
fn completed_subagents_do_not_leave_composer_status_stuck() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.active_subagent_ids.insert("child".to_string());
    pane.active_subagent_started_at
        .insert("child".to_string(), 1);
    pane.side_panel
        .set_subagents(vec![NeoismAgentSessionEntry::new(
            "child", "child", "explore",
        )
        .with_runtime_status(Some("completed".to_string()))]);
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_state_changed_elapsed(), None);
}

#[test]
fn subagent_composer_status_tracks_only_active_children() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.active_subagent_ids.insert("done".to_string());
    pane.active_subagent_ids.insert("running".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("done", "done", "explore")
            .with_runtime_status(Some("completed".to_string())),
        NeoismAgentSessionEntry::new("running", "running", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert!(pane.has_status_activity());

    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("done", "done", "explore")
            .with_runtime_status(Some("completed".to_string())),
        NeoismAgentSessionEntry::new("running", "running", "explore")
            .with_runtime_status(Some("completed".to_string())),
    ]);
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
}

#[test]
fn virtual_timeline_commits_exact_row_heights_and_groups_hidden_nodes() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("first").with_id("m1"),
        NeoismAgentMessage::assistant("read one").with_id("m2"),
        NeoismAgentMessage::assistant("read two").with_id("m3"),
    ];
    pane.timeline_layout_epoch = 7;
    let rows = vec![
        TimelineVirtualRowMeasurement {
            source_index: 0,
            source_end_index: 0,
            height: 100.0,
            visual_line_count: 5,
        },
        TimelineVirtualRowMeasurement {
            source_index: 1,
            source_end_index: 2,
            height: 200.0,
            visual_line_count: 10,
        },
    ];

    pane.sync_virtual_timeline([0.0, 0.0, 500.0, 120.0], 500.0, 300.0, 90.0, 1.0, &rows);

    assert_eq!(pane.virtual_timeline.surface.nodes().len(), 3);
    let content_height = pane.virtual_timeline.surface.content_height();
    assert!(
        (content_height - 300.0).abs() < 0.01,
        "content_height={content_height}"
    );
    assert!(pane.virtual_timeline_visible_nodes() > 0);
    assert_eq!(pane.virtual_timeline_visible_source_range(), Some((0, 2)));
}

#[test]
fn virtual_timeline_patches_changed_message_without_replacing_transcript() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("patch-session".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("first").with_id("m1"),
        NeoismAgentMessage::assistant("old").with_id("m2"),
    ];
    pane.timeline_layout_epoch = 7;
    pane.sync_virtual_timeline([0.0, 0.0, 500.0, 120.0], 500.0, 0.0, 0.0, 1.0, &[]);
    let first_node_id = pane.virtual_timeline.surface.nodes()[0].id;
    let old_revision = pane.virtual_timeline.surface.nodes()[1].revision;

    pane.messages[1].text = "old\nnew streamed tail".to_string();
    pane.timeline_layout_epoch = 8;
    pane.sync_virtual_timeline([0.0, 0.0, 500.0, 120.0], 500.0, 0.0, 0.0, 1.0, &[]);

    assert_eq!(pane.virtual_timeline.surface.nodes()[0].id, first_node_id);
    assert!(pane.virtual_timeline.surface.nodes()[1].revision > old_revision);
    assert_eq!(
        pane.virtual_timeline.surface.nodes()[1]
            .content
            .as_ref()
            .unwrap()
            .byte_len,
        "old\nnew streamed tail".len() as u64
    );
}

#[test]
fn virtual_timeline_measurements_are_not_rebuilt_on_plain_scroll() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("scroll-measure-session".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("first").with_id("m1"),
        NeoismAgentMessage::assistant("second").with_id("m2"),
    ];
    pane.timeline_layout_epoch = 3;
    let rows = vec![
        TimelineVirtualRowMeasurement {
            source_index: 0,
            source_end_index: 0,
            height: 80.0,
            visual_line_count: 4,
        },
        TimelineVirtualRowMeasurement {
            source_index: 1,
            source_end_index: 1,
            height: 120.0,
            visual_line_count: 6,
        },
    ];

    assert!(pane.virtual_timeline_needs_measurements(500.0, 1.0, rows.len(), 200.0));
    pane.sync_virtual_timeline([0.0, 0.0, 500.0, 120.0], 500.0, 200.0, 0.0, 1.0, &rows);
    assert!(!pane.virtual_timeline_needs_measurements(500.0, 1.0, rows.len(), 200.0));

    pane.sync_virtual_timeline([0.0, 0.0, 500.0, 120.0], 500.0, 200.0, 70.0, 1.0, &[]);
    assert!(!pane.virtual_timeline_needs_measurements(500.0, 1.0, rows.len(), 200.0));

    assert!(pane.virtual_timeline_needs_measurements(500.0, 1.0, rows.len(), 220.0));
}

#[test]
fn completed_answer_stays_above_later_streamed_reasoning() {
    // The model answers (non-empty text), *then* opens a fresh
    // thinking block. The finished answer must keep its slot above the
    // later reasoning — it must not drop below it mid-stream.
    let mut pane = NeoismAgentPane::default();

    pane.apply_part_delta(
        None,
        Some("text-1".to_string()),
        Some("text".to_string()),
        "final",
    );
    pane.apply_part_delta(
        None,
        Some("reason-1".to_string()),
        Some("reasoning".to_string()),
        "thought",
    );

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(pane.messages[0].text, "final");
    assert_eq!(pane.messages[1].kind, NeoismAgentMessageKind::Reasoning);
    assert_eq!(pane.messages[1].text, "thought");
}

#[test]
fn empty_assistant_placeholder_drops_below_reasoning() {
    // A provider that opens the turn with a blank text part before it
    // streams reasoning: the empty placeholder is pulled below so the
    // thinking renders first, then fills in.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(NeoismAgentMessage::assistant("").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought").with_id("reason-1"),
    );

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].id, "reason-1");
    assert_eq!(pane.messages[1].id, "text-1");
}

#[test]
fn streamed_final_part_inserts_after_existing_reasoning() {
    let mut pane = NeoismAgentPane::default();

    pane.apply_part_delta(
        None,
        Some("reason-1".to_string()),
        Some("reasoning".to_string()),
        "thought",
    );
    pane.apply_part_delta(
        None,
        Some("text-1".to_string()),
        Some("text".to_string()),
        "final",
    );

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::Reasoning);
    assert_eq!(pane.messages[1].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(pane.messages[1].text, "final");
}

#[test]
fn updated_reasoning_part_does_not_pull_finished_answer_below_it() {
    // A non-empty answer that already streamed must keep its slot even
    // when its reasoning part updates afterwards — chronological order
    // is preserved for finished text.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(NeoismAgentMessage::assistant("final").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought").with_id("reason-1"),
    );
    // Answer landed first, reasoning after — order is kept.
    assert_eq!(pane.messages[0].id, "text-1");
    assert_eq!(pane.messages[1].id, "reason-1");

    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought done").with_id("reason-1"),
    );

    assert_eq!(pane.messages[0].id, "text-1");
    assert_eq!(pane.messages[1].id, "reason-1");
    assert_eq!(pane.messages[1].text, "thought done");
}

#[test]
fn new_reasoning_does_not_drop_completed_answers_below_it() {
    // Two finished answers followed by a fresh thinking block: every
    // completed answer keeps its chronological slot, the new reasoning
    // appends at the tail.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(
        NeoismAgentMessage::assistant("old final").with_id("text-old"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("new final").with_id("text-new"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("new thought").with_id("reason-new"),
    );

    assert_eq!(pane.messages[0].id, "text-old");
    assert_eq!(pane.messages[1].id, "text-new");
    assert_eq!(pane.messages[2].id, "reason-new");
}

#[test]
fn reasoning_part_does_not_move_previous_turn_final_below_new_user_prompt() {
    let mut pane = NeoismAgentPane::default();

    pane.messages.push(NeoismAgentMessage::user("first"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("old final").with_id("text-old"),
    );
    pane.messages.push(NeoismAgentMessage::user("second"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("new thought").with_id("reason-new"),
    );

    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::User);
    assert_eq!(pane.messages[1].id, "text-old");
    assert_eq!(pane.messages[2].kind, NeoismAgentMessageKind::User);
    assert_eq!(pane.messages[3].id, "reason-new");
}

#[test]
fn reasoning_after_finished_answer_and_tool_keeps_chronological_order() {
    // answer → tool → reasoning, all chronological. The finished
    // answer is non-empty so nothing reorders; reasoning appends last.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(NeoismAgentMessage::assistant("final").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::tool(
            "Bash(echo ok)",
            "",
            "completed",
            "bash",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-1"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("post tool thought").with_id("reason-1"),
    );

    assert_eq!(pane.messages[0].id, "text-1");
    assert_eq!(pane.messages[1].id, "tool-1");
    assert_eq!(pane.messages[2].id, "reason-1");
}

#[test]
fn untagged_reasoning_delta_does_not_append_to_final_text() {
    let mut pane = NeoismAgentPane::default();

    pane.apply_part_delta(None, None, Some("text".to_string()), "final");
    pane.apply_part_delta(None, None, Some("reasoning".to_string()), "thought");

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(pane.messages[0].text, "final");
    assert_eq!(pane.messages[1].kind, NeoismAgentMessageKind::Reasoning);
    assert_eq!(pane.messages[1].text, "thought");
}

#[test]
fn updated_final_part_does_not_reorder_past_later_tool() {
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought").with_id("reason-1"),
    );
    pane.upsert_part_message(NeoismAgentMessage::assistant("final").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::tool(
            "Bash(echo ok)",
            "",
            "running",
            "bash",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-1"),
    );

    assert_eq!(pane.messages[0].id, "reason-1");
    assert_eq!(pane.messages[1].id, "text-1");
    assert_eq!(pane.messages[2].id, "tool-1");
}

#[test]
fn history_refresh_rebases_live_trace_to_durable_turn() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("latest"),
        NeoismAgentMessage::reasoning("thinking").with_id("reasoning"),
        NeoismAgentMessage::assistant("tool").with_id("tool"),
    ];
    pane.timeline_live_trace_start = Some(1);

    pane.apply_history(vec![
        NeoismAgentMessage::user("old"),
        NeoismAgentMessage::assistant("old answer").with_id("old-answer"),
        NeoismAgentMessage::user("latest"),
        NeoismAgentMessage::assistant("durable answer").with_id("answer"),
    ]);

    assert_eq!(pane.timeline_live_trace_start, Some(3));
    assert_eq!(pane.messages[3].text, "durable answer");
}

#[test]
fn tool_expand_preserves_clicked_card_top_when_content_height_changes() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.register_tool_hit_rect("tool-1".to_string(), [20.0, 150.0, 300.0, 60.0]);

    assert!(pane.toggle_tool_at(30.0, 160.0));
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);

    let max_scroll = pane.max_timeline_scroll();
    let scroll_top = max_scroll - pane.timeline_scroll_offset();
    assert_eq!(scroll_top, 400.0);
    assert!(pane.tool_expanded("tool-1"));
}

#[test]
fn timeline_growth_preserves_reader_position_when_scrolled_up() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);

    let max_scroll = pane.max_timeline_scroll();
    let scroll_top = max_scroll - pane.timeline_scroll_offset();
    assert_eq!(scroll_top, 400.0);
}

#[test]
fn timeline_growth_keeps_following_stream_at_bottom() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 0.0;

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);

    assert_eq!(pane.timeline_scroll_offset(), 0.0);
}

#[test]
fn ctrl_u_d_half_page_scroll_moves_timeline_by_half_viewport() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_half_page(true));
    assert_eq!(pane.timeline_scroll_offset(), 150.0);

    assert!(pane.scroll_timeline_half_page(false));
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
}

fn task_tool_message(task_id: &str, status: &str) -> NeoismAgentMessage {
    let mut message = NeoismAgentMessage::tool(
        "Task(child)",
        format!("task_id: {task_id}\nstatus: {status}"),
        status,
        "task",
        NeoismAgentOutputKind::Text,
        "",
        Vec::new(),
    );
    message.detail = format!("task_id: {task_id}\nstatus: {status}");
    message
}

fn test_permission(selected: usize) -> NeoismAgentPendingPermission {
    NeoismAgentPendingPermission {
        id: "perm-1".to_string(),
        session_id: "session-1".to_string(),
        parent_session_id: None,
        source_agent: None,
        source_title: None,
        title: "Run command".to_string(),
        permission: "shell".to_string(),
        patterns: Vec::new(),
        selected,
        responding: false,
    }
}

fn sample_connect_catalog() -> (serde_json::Value, serde_json::Value) {
    let providers = json!({
        "all": [
            { "id": "anthropic", "name": "Anthropic" },
            { "id": "openai", "name": "OpenAI" },
        ],
        "connected": ["anthropic"],
    });
    let auth = json!({
        "anthropic": [{ "type": "api", "label": "Manually enter API Key" }],
        "openai": [{ "type": "oauth", "label": "Sign in with OpenAI" }],
    });
    (providers, auth)
}

#[test]
fn connect_slash_opens_provider_picker_and_requests_catalog() {
    let mut pane = NeoismAgentPane::default();
    pane.execute_slash_text("/connect");
    let picker = pane.picker().expect("connect picker opens on /connect");
    assert_eq!(picker.kind, NeoismAgentPickerKind::Connect);
    // The catalog fetch is queued for the host.
    assert!(pane.drain_pending_outbound().iter().any(|command| matches!(
        command,
        OutboundAgentCommand::RefreshConnectProviders { .. }
    )));
}

#[test]
fn connect_catalog_populates_provider_rows_with_connected_marker() {
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    let (providers, auth) = sample_connect_catalog();
    pane.apply_connect_catalog(providers, auth);
    let picker = pane.picker().expect("connect picker stays open");
    assert_eq!(picker.kind, NeoismAgentPickerKind::Connect);
    assert!(picker
        .options()
        .iter()
        .any(|option| option.title == "Popular" && option.is_header));
    // Connected provider gets the checkmark and a "connected" footer.
    assert!(picker
        .options()
        .iter()
        .any(|option| option.value == "anthropic"
            && option.title.starts_with('✓')
            && option.footer == "connected"));
}

#[test]
fn connect_api_key_path_queues_store_command() {
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    let (providers, auth) = sample_connect_catalog();
    pane.apply_connect_catalog(providers, auth);
    let _ = pane.drain_pending_outbound();

    // Stage 1 → 2: pick Anthropic.
    assert_eq!(
        pane.picker()
            .and_then(|picker| picker.selected_option())
            .map(|option| option.value.clone()),
        Some("anthropic".to_string()),
        "connected popular provider is the default selection"
    );
    assert!(pane.commit_picker());
    let picker = pane.picker().expect("auth-method picker opens");
    assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectAuth);
    // First row is the disconnect affordance (Anthropic is connected).
    assert_eq!(
        picker.selected_option().map(|option| option.value.clone()),
        Some(connect::DISCONNECT_VALUE.to_string())
    );

    // Move to the API-key method row and commit.
    pane.move_picker_selection(1);
    assert!(pane.commit_picker());
    let picker = pane.picker().expect("secret entry opens");
    assert_eq!(picker.kind, NeoismAgentPickerKind::ConnectSecret);
    assert_eq!(picker.search_placeholder.as_deref(), Some("API key"));

    // Type a key into the secret row and commit.
    pane.insert_text("sk-test-123");
    assert!(pane.commit_picker());
    let stored = pane.drain_pending_outbound();
    assert!(stored.iter().any(|command| matches!(
        command,
        OutboundAgentCommand::ConnectStoreApiKey { provider_id, key }
            if provider_id == "anthropic" && key == "sk-test-123"
    )));

    // Host confirms → flow closes.
    pane.note_connect_finished("Anthropic".to_string());
    assert!(pane.picker().is_none());
}

#[test]
fn connect_secret_escape_steps_back_to_auth_method() {
    let mut pane = NeoismAgentPane::default();
    pane.open_connect_picker();
    let (providers, auth) = sample_connect_catalog();
    pane.apply_connect_catalog(providers, auth);
    pane.commit_picker(); // Connect → ConnectAuth (Anthropic)
    pane.move_picker_selection(1); // API-key method
    pane.commit_picker(); // ConnectAuth → ConnectSecret
    assert_eq!(
        pane.picker().map(|picker| picker.kind),
        Some(NeoismAgentPickerKind::ConnectSecret)
    );
    // ESC steps back to the auth-method stage rather than dismissing.
    pane.close_picker();
    assert_eq!(
        pane.picker().map(|picker| picker.kind),
        Some(NeoismAgentPickerKind::ConnectAuth)
    );
    // ESC again → back to the provider list.
    pane.close_picker();
    assert_eq!(
        pane.picker().map(|picker| picker.kind),
        Some(NeoismAgentPickerKind::Connect)
    );
    // ESC again → dismissed entirely.
    pane.close_picker();
    assert!(pane.picker().is_none());
}
