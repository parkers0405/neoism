
use super::*;
use std::fs;

#[test]
fn file_mention_options_filter_workspace_trash_dirs() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
    for ignored in [".claude", ".cache", ".neoism", "node_modules", "target"] {
        fs::create_dir_all(root.path().join(ignored)).unwrap();
        fs::write(root.path().join(ignored).join("main.rs"), "trash").unwrap();
    }

    let options = file_mention_options(root.path(), "main", 20);
    let values = options
        .into_iter()
        .map(|option| option.value)
        .collect::<Vec<_>>();

    assert_eq!(values, vec!["src/main.rs"]);
}

#[test]
fn idle_clears_status_but_keeps_trace_until_session_reset() {
    let mut pane = NeoismAgentPane::default();
    pane.messages.push(NeoismAgentMessage::user("question"));

    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert!(pane.is_streaming());
    assert_eq!(pane.streaming_label(), "Crafting");

    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("working").with_id("reasoning-1"),
    );
    assert_eq!(pane.timeline_live_trace_start, Some(1));

    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert!(!pane.is_streaming());
    assert_eq!(pane.streaming_label(), "");
    assert_eq!(pane.streaming_elapsed_seconds(), None);
    assert_eq!(pane.timeline_live_trace_start, Some(1));

    pane.reset_session_runtime_ui();
    assert_eq!(pane.timeline_live_trace_start, None);
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
fn connected_idle_event_stream_does_not_drive_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.event_stream = Some(AgentSessionEventStream::connected_for_test("sess-1"));

    assert_eq!(pane.animation_reason(), None);
}

#[test]
fn attached_session_counts_as_conversation_before_messages_load() {
    let mut pane = NeoismAgentPane::default();

    assert!(!pane.has_conversation());

    pane.session_id = Some("session-1".to_string());

    assert!(pane.has_conversation());
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
    pane.ensure_background_task_activity_clock();

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
    pane.ensure_background_task_activity_clock();

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
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_elapsed_seconds(), None);
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
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_elapsed_seconds(), None);
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn abort_session_queues_outbound_command_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    pane.abort_session();

    assert!(!pane.is_streaming());
    assert!(pane.abort_requested_at.is_some());
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::AbortSession]
    );
}

#[test]
fn abort_without_session_does_not_queue_outbound_command() {
    let mut pane = NeoismAgentPane::default();

    pane.abort_session();

    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn switch_session_queues_outbound_command_for_runtime() {
    let mut pane = NeoismAgentPane::default();

    pane.switch_session("sess-2".to_string());

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::SwitchSession {
            session_id: "sess-2".to_string()
        }]
    );
}

#[test]
fn compact_session_queues_outbound_command_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());

    pane.execute_slash_text("/compact");

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::CompactSession]
    );
    assert!(pane.messages.is_empty());
    assert_ne!(pane.streaming_label(), "Compacting");
}

#[test]
fn submit_prompt_queues_session_and_send_prompt_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("ship it");

    assert!(pane.submit());

    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 2);
    assert!(matches!(drained[0], OutboundAgentCommand::EnsureSession));
    match &drained[1] {
        OutboundAgentCommand::SendPrompt {
            text,
            agent,
            model,
            thinking,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "ship it");
            assert_eq!(agent.as_deref(), Some(DEFAULT_AGENT));
            assert_eq!(model, DEFAULT_MODEL);
            assert_eq!(thinking, &None);
            assert!(
                *transcript_echo,
                "idle submissions should be echoed into the transcript"
            );
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
    assert_eq!(pane.messages[0].text, "ship it");
    assert!(pane.is_streaming());
}

#[test]
fn submit_prompt_with_session_queues_send_prompt_only() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.insert_text("continue");

    assert!(pane.submit());

    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 1);
    match &drained[0] {
        OutboundAgentCommand::SendPrompt {
            text,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "continue");
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
    pane.session_id = Some("sess-1".to_string());
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
}

#[test]
fn submit_prompt_while_streaming_queues_bottom_preview_without_transcript_echo() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
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
fn server_expanded_user_part_merges_into_pasted_token_echo() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_paste("first line\nsecond line");
    assert!(pane.submit());
    assert_eq!(pane.messages[0].text, "[pasted 2 lines]");

    // The server streams the user part back EXPANDED, with its own id.
    let mut server_part = NeoismAgentMessage::user("first line\nsecond line");
    server_part.id = "srv-user-1".to_string();
    pane.upsert_part_message(server_part);

    let users: Vec<_> = pane
        .messages
        .iter()
        .filter(|message| message.kind == NeoismAgentMessageKind::User)
        .collect();
    assert_eq!(
        users.len(),
        1,
        "expanded server echo must not add a second user bubble"
    );
    assert_eq!(users[0].text, "[pasted 2 lines]");
    assert_eq!(users[0].id, "srv-user-1");
}

#[test]
fn history_refresh_compacts_expanded_pasted_user_text() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_paste("first line\nsecond line");
    assert!(pane.submit());

    let mut server_user = NeoismAgentMessage::user("first line\nsecond line");
    server_user.id = "srv-user-1".to_string();
    let compacted = pane.compact_inbound_user_texts(vec![server_user]);

    assert_eq!(compacted[0].text, "[pasted 2 lines]");
}

#[test]
fn backspace_removes_pasted_token_and_attachment_atomically() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("see ");
    pane.insert_paste("first line\nsecond line");
    assert!(pane.input.contains("[pasted 2 lines]"));
    assert_eq!(pane.input_attachments.len(), 1);

    // One backspace removes the whole token plus the trailing space
    // `insert_token` added.
    pane.backspace();

    assert_eq!(pane.input, "see ");
    assert!(
        pane.input_attachments.is_empty(),
        "deleting the token must drop its attachment"
    );
}

#[test]
fn dequeued_prompt_consumes_preview_and_appends_once_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("done"),
    ];
    pane.queued_prompt_count = 1;
    pane.queued_prompt_preview = Some("queued turn".to_string());

    assert!(pane.insert_dequeued_user_prompt("queued turn".to_string()));
    assert!(!pane.insert_dequeued_user_prompt("queued turn".to_string()));

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
fn unknown_slash_command_queues_session_and_slash_for_runtime() {
    let mut pane = NeoismAgentPane::default();

    pane.execute_slash_text("/login token=abc");

    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 2);
    assert!(matches!(drained[0], OutboundAgentCommand::EnsureSession));
    match &drained[1] {
        OutboundAgentCommand::SlashCommand { name, args } => {
            assert_eq!(name, "login");
            assert_eq!(args, "token=abc");
        }
        other => panic!("expected SlashCommand, got {other:?}"),
    }
}

#[test]
fn typing_space_after_slash_command_closes_picker_and_focuses_input() {
    let mut pane = NeoismAgentPane::default();

    for ch in ["/", "g", "o", "a", "l"] {
        pane.insert_text(ch);
    }
    // Picker is open while typing the bare command name.
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Slash));

    // Pressing space commits the command name and starts the argument:
    // the picker must dismiss and the caret must move to the composer.
    pane.insert_text(" ");
    assert!(pane.picker().is_none());
    assert_eq!(pane.input(), "/goal ");
    assert_eq!(pane.cursor_byte(), "/goal ".len());

    // Subsequent argument text lands in the input bar, not the picker.
    for ch in ["s", "h", "i", "p"] {
        pane.insert_text(ch);
    }
    assert!(pane.picker().is_none());
    assert_eq!(pane.input(), "/goal ship");
    assert_eq!(pane.cursor_byte(), "/goal ship".len());
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
fn backspace_updates_skill_mention_input_and_dismisses_when_trigger_removed() {
    let mut pane = NeoismAgentPane::default();

    pane.insert_text("$");
    pane.insert_text("neo");
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention));

    pane.backspace();
    assert_eq!(pane.input(), "$ne");
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention));

    pane.backspace();
    pane.backspace();
    assert_eq!(pane.input(), "$");
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention));

    pane.backspace();
    assert_eq!(pane.input(), "");
    assert!(pane.picker().is_none());
}

#[test]
fn enter_still_runs_argumentless_slash_command_from_picker() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());

    for ch in ["/", "c", "o", "m", "p", "a", "c", "t"] {
        pane.insert_text(ch);
    }
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Slash));

    // No trailing space: Enter commits the highlighted command.
    assert!(pane.submit());
    assert!(pane.picker().is_none());
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::CompactSession]
    );
}

#[test]
fn model_change_queues_context_limit_refresh_for_runtime() {
    let mut pane = NeoismAgentPane::default();

    pane.apply_model("claude-test".to_string());

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::RefreshModelContextLimit]
    );
}

#[test]
fn with_directory_queues_config_defaults_for_runtime() {
    let mut pane = NeoismAgentPane::with_directory(Some("/tmp/project".to_string()));

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::ApplyConfigDefaults]
    );
}

#[test]
fn permission_reply_queues_outbound_command_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.pending_permission = Some(test_permission(0));

    assert!(pane.respond_pending_permission(NeoismAgentPermissionChoice::Reject));

    assert!(pane.pending_permission.as_ref().unwrap().responding);
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::ReplyPermission {
            id: "perm-1".to_string(),
            reply: "reject".to_string(),
        }]
    );
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
        NeoismAgentMessage::compaction("summary body", "auto")
            .with_id("local-compaction"),
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
fn history_refresh_keeps_compaction_marker_in_original_slot() {
    let mut pane = NeoismAgentPane::default();
    // A compaction happened mid-conversation, then another turn streamed in.
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("first answer").with_id("answer-1"),
        NeoismAgentMessage::compaction("summary", "model").with_id("compaction-1"),
        NeoismAgentMessage::user("second"),
        NeoismAgentMessage::assistant("second answer").with_id("answer-2"),
    ];

    // The idle history refresh never echoes the local-only compaction marker.
    let server_messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("first answer").with_id("answer-1"),
        NeoismAgentMessage::user("second"),
        NeoismAgentMessage::assistant("second answer").with_id("answer-2"),
    ];

    let refreshed = pane.preserve_streamed_response_text(server_messages);

    assert_eq!(refreshed.len(), 5);
    assert_eq!(refreshed[2].kind, NeoismAgentMessageKind::Compaction);
    assert_eq!(refreshed[2].text, "summary");
    // The marker must not be appended past the latest assistant reply.
    assert_eq!(refreshed[4].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(refreshed[4].text, "second answer");
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
fn compaction_lifecycle_events_do_not_create_messages() {
    let mut pane = NeoismAgentPane::default();

    pane.start_compaction_message("event-1".to_string(), "auto".to_string());
    assert!(pane.messages.is_empty());
    assert_ne!(pane.streaming_label(), "Compacting");

    pane.apply_compaction_delta("summary");
    pane.finish_compaction_message("summary", "model");
    assert!(pane.messages.is_empty());
}

#[test]
fn persisted_compaction_is_only_compaction_message_source() {
    let mut pane = NeoismAgentPane::default();

    pane.start_compaction_message("event-1".to_string(), "auto".to_string());
    pane.apply_compaction_delta("event delta");
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
    pane.apply_compaction_delta("event delta tail");
    pane.finish_compaction_message(
        "compaction done\ncompaction summary\ncompaction model",
        "model",
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
fn completed_answer_stays_above_later_streamed_reasoning() {
    // The model answers (non-empty text), then opens a fresh thinking
    // block. The finished answer must keep its slot above the later
    // reasoning — it must not drop below it mid-stream.
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
    // A non-empty answer that already streamed keeps its slot even
    // when its reasoning part updates afterwards.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(NeoismAgentMessage::assistant("final").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought").with_id("reason-1"),
    );
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
    // Two finished answers then a fresh thinking block: every
    // completed answer keeps its chronological slot.
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
fn timeline_prepend_preserves_reader_position_when_scrolled_up() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.pending_timeline_prepend_height_px = Some(900.0);

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);

    let max_scroll = pane.max_timeline_scroll();
    let scroll_top = max_scroll - pane.timeline_scroll_offset();
    assert_eq!(scroll_top, 600.0);
}

#[test]
fn timeline_prepend_anchor_survives_until_content_height_grows() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.pending_timeline_prepend_height_px = Some(900.0);

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    assert_eq!(pane.pending_timeline_prepend_height_px, Some(900.0));

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);
    let max_scroll = pane.max_timeline_scroll();
    let scroll_top = max_scroll - pane.timeline_scroll_offset();
    assert_eq!(scroll_top, 600.0);
    assert_eq!(pane.pending_timeline_prepend_height_px, None);
}

#[test]
fn older_timeline_request_gate_reopens_after_success() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    // Pagination only fires while the reader is scrolling toward the top.
    pane.timeline_last_scroll_at = Some(Instant::now());

    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert!(pane.timeline_history.loading_older);
    assert_eq!(pane.drain_pending_outbound().len(), 1);

    // A second request is blocked by the rate-limit cooldown even after the
    // in-flight gate clears — preventing a back-to-back load cascade.
    pane.timeline_history.loading_older = false;
    pane.timeline_history.last_requested_session_id = None;
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 0);

    // Past the cooldown, scrolling toward the top loads the next page.
    pane.timeline_last_older_request_at =
        Some(Instant::now() - Duration::from_millis(500));
    pane.timeline_last_scroll_at = Some(Instant::now());
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 1);
}

#[test]
fn older_timeline_request_skipped_when_not_scrolling() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    // No recent scroll and not inertial → parked at the top, so we must
    // not auto-pull pages (this is what caused the load cascade).
    pane.timeline_last_scroll_at = None;

    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert!(!pane.timeline_history.loading_older);
    assert_eq!(pane.drain_pending_outbound().len(), 0);
}

#[test]
fn apply_older_page_prepends_and_keeps_loading_when_full() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    let mut current = NeoismAgentMessage::user("current");
    current.id = "m-current".to_string();
    pane.messages = vec![current];
    pane.timeline_history.loading_older = true;
    pane.timeline_history.last_requested_session_id = Some("session-1".to_string());

    let mut older = NeoismAgentMessage::user("older");
    older.id = "m-older".to_string();
    // A full page (raw_count == requested limit) means more may remain.
    pane.apply_older_timeline_page("session-1".to_string(), vec![older], 1, 1);

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].id, "m-older");
    assert!(pane.timeline_history.has_older);
    assert!(!pane.timeline_history.loading_older);
    assert_eq!(
        pane.timeline_history.oldest_loaded_cursor.as_deref(),
        Some("m-older")
    );
    // The prepend is folded incrementally, not via a full relayout.
    assert_eq!(pane.pending_timeline_prepend_count, Some(1));
}

#[test]
fn timeline_prepend_count_accumulates_and_invalidation_clears_it() {
    let mut pane = NeoismAgentPane::default();
    pane.timeline_live_trace_start = Some(10);
    pane.note_timeline_prepend(3);
    pane.note_timeline_prepend(2);
    assert_eq!(pane.pending_timeline_prepend_count, Some(5));
    assert_eq!(pane.timeline_live_trace_start, Some(15));

    // A full invalidation makes the incremental fold moot.
    pane.invalidate_timeline_layout();
    assert_eq!(pane.pending_timeline_prepend_count, None);

    // take consumes the pending fold exactly once.
    pane.note_timeline_prepend(4);
    assert_eq!(pane.take_timeline_prepend(), Some(4));
    assert_eq!(pane.take_timeline_prepend(), None);
}

#[test]
fn apply_older_page_stops_at_start_on_short_page() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    let mut current = NeoismAgentMessage::user("current");
    current.id = "m-current".to_string();
    pane.messages = vec![current];
    pane.timeline_history.loading_older = true;

    let mut older = NeoismAgentMessage::user("older");
    older.id = "m-older".to_string();
    // Server returned fewer messages than requested → reached the start.
    pane.apply_older_timeline_page("session-1".to_string(), vec![older], 1, 128);

    assert_eq!(pane.messages.len(), 2);
    assert!(!pane.timeline_history.has_older);
}

#[test]
fn apply_older_page_ignored_after_session_switch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-2".to_string());
    pane.timeline_history.loading_older = true;

    let mut older = NeoismAgentMessage::user("older");
    older.id = "m-older".to_string();
    pane.apply_older_timeline_page("session-1".to_string(), vec![older], 1, 1);

    assert!(pane.messages.is_empty());
    assert!(!pane.timeline_history.loading_older);
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
