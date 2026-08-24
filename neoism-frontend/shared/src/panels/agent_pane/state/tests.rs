use super::*;
use crate::panels::agent_pane::outbound::OutboundAgentCommand;
use crate::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry;
use crate::panels::agent_pane::state::side_panel::STATUS_LABEL_GRACE;

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
fn generic_live_subagent_update_cannot_replace_specific_title() {
    let mut pane = NeoismAgentPane::default();
    pane.side_panel
        .upsert_subagent("child", "Review web desktop parity", "explore");
    pane.side_panel
        .upsert_subagent("child", "subagent", "subagent");

    let child = pane
        .side_panel
        .subagents()
        .iter()
        .find(|entry| entry.id == "child")
        .expect("child row");
    assert_eq!(child.title, "Review web desktop parity");
    assert_eq!(child.time_label, "explore");
}

#[test]
fn generic_polled_subagent_metadata_cannot_replace_specific_title() {
    let mut pane = NeoismAgentPane::default();
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("root", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "Audit markdown interactions", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("root", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "subagent", "agent")
            .with_runtime_status(Some("blocked".to_string())),
    ]);

    let child = pane
        .side_panel
        .subagents()
        .iter()
        .find(|entry| entry.id == "child")
        .expect("child row");
    assert_eq!(child.title, "Audit markdown interactions");
    assert_eq!(child.time_label, "explore");
    assert_eq!(child.runtime_status.as_deref(), Some("blocked"));
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
fn hints_command_toggles_row_and_reclaims_chat_space() {
    let mut pane = NeoismAgentPane::default();
    let viewport = [0.0, 0.0, 1000.0, 800.0];
    let visible_rect =
        crate::panels::agent_pane::view::layout::chat_input_rect(&pane, viewport, 1.0);
    assert!(pane.input_help_visible());

    pane.execute_slash_text("/hints");
    let hidden_rect =
        crate::panels::agent_pane::view::layout::chat_input_rect(&pane, viewport, 1.0);
    assert!(!pane.input_help_visible());
    assert_eq!(
        hidden_rect[1] - visible_rect[1],
        22.0,
        "hiding the compact footer must move the composer into its reserved space"
    );
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::SetInputHelpVisible { visible: false }]
    );

    pane.execute_slash_text("/hints");
    assert!(pane.input_help_visible());
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::SetInputHelpVisible { visible: true }]
    );
}

#[test]
fn sidebar_command_toggles_visibility_and_persists_default() {
    let mut pane = NeoismAgentPane::default();
    assert!(!pane.side_panel().user_hidden());

    pane.execute_slash_text("/sidebar");

    assert!(pane.side_panel().user_hidden());
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::SetSidebarVisible { visible: false }]
    );
}

#[test]
fn slash_picker_command_prefix_beats_description_matches() {
    let mut picker = NeoismAgentPicker::new(
        NeoismAgentPickerKind::Slash,
        "Commands",
        crate::panels::agent_pane::command_controller::slash_options(),
        0,
    );

    picker.set_query("sess".to_string());

    assert_eq!(
        picker.options().first().map(|option| option.value.as_str()),
        Some("/sessions")
    );
    assert_eq!(
        picker.selected_option().map(|option| option.value.as_str()),
        Some("/sessions")
    );
    assert!(
        picker
            .options()
            .iter()
            .any(|option| option.value == "/compact"),
        "description matches should remain available below command-name matches"
    );
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
            message_id,
            text,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "hello world");
            assert_eq!(pane.messages[0].id.as_str(), message_id);
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
    assert_eq!(pane.queued_prompt_count, 0);
    assert_eq!(pane.queued_prompt_preview, None);
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
fn slash_clear_routes_to_the_server_like_desktop() {
    // `/clear` has no arm in `plan_slash_command`; the desktop dispatcher
    // forwards it through `run_server_command` (EnsureSession first when
    // no session exists yet). The shared pane must match, not keep its
    // old local-only arm.
    let mut pane = NeoismAgentPane::default();
    pane.execute_slash_text("/clear");
    let drained = pane.drain_pending_outbound();
    assert!(
        matches!(drained.first(), Some(OutboundAgentCommand::EnsureSession)),
        "expected EnsureSession first: {drained:?}"
    );
    assert!(
        matches!(
            drained.get(1),
            Some(OutboundAgentCommand::SlashCommand { name, args })
                if name == "clear" && args.is_empty()
        ),
        "expected SlashCommand clear: {drained:?}"
    );
}

#[test]
fn slash_undo_redo_queue_session_history_commands() {
    let mut pane = NeoismAgentPane::default();
    // Without a session the desktop surfaces "no session has started yet"
    // and queues nothing.
    pane.execute_slash_text("/undo");
    pane.execute_slash_text("/redo");
    assert!(pane.drain_pending_outbound().is_empty());

    pane.set_session_id(Some("sess-1".to_string()));
    pane.execute_slash_text("/undo");
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::UndoSession]
    );
    pane.execute_slash_text("/redo");
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::RedoSession]
    );
}

#[test]
fn slash_help_renders_the_desktop_command_sheet_locally() {
    let mut pane = NeoismAgentPane::default();
    pane.execute_slash_text("/help");
    assert!(pane.drain_pending_outbound().is_empty());
    let message = pane
        .messages
        .iter()
        .find(|message| {
            message.kind == NeoismAgentMessageKind::System && message.title == "Commands"
        })
        .expect("help must land as a local system message");
    for option in crate::panels::agent_pane::command_controller::slash_options() {
        assert!(
            message.text.contains(&option.title),
            "help sheet must list {}",
            option.title
        );
    }
}

#[test]
fn slash_yolo_toggles_skip_permissions_and_auto_answers() {
    let mut pane = NeoismAgentPane::default();
    assert!(!pane.skip_permissions_enabled());
    pane.enqueue_pending_permission(NeoismAgentPendingPermission {
        id: "perm-1".to_string(),
        session_id: String::new(),
        parent_session_id: None,
        source_agent: None,
        source_title: None,
        title: "Run command".to_string(),
        permission: "bash".to_string(),
        patterns: Vec::new(),
        selected: 0,
        responding: false,
    });
    assert!(pane.drain_pending_outbound().is_empty());

    pane.execute_slash_text("/yolo");
    assert!(pane.skip_permissions_enabled());
    // Turning it on immediately answers the pending request "Yes".
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::ReplyPermission {
            id: "perm-1".to_string(),
            reply: "once".to_string(),
        }]
    );

    pane.execute_slash_text("/yolo");
    assert!(!pane.skip_permissions_enabled());
}

#[test]
fn slash_fx_easter_eggs_queue_a_request_for_the_render_loop() {
    use crate::panels::agent_pane::view::fx::AgentFxKind;

    let mut pane = NeoismAgentPane::default();
    for (command, kind) in [
        ("/piss", AgentFxKind::Piss),
        ("/cuss", AgentFxKind::Cuss),
        ("/glitch", AgentFxKind::Glitch),
        ("/disco", AgentFxKind::Disco),
        ("/gangfight", AgentFxKind::GangFight),
        ("/praise", AgentFxKind::Praise),
    ] {
        pane.execute_slash_text(command);
        assert!(pane.is_animating(), "{command} must own the redraw loop");
        assert_eq!(pane.take_fx_request(), Some(kind), "{command}");
        // Consumed exactly once — the render loop stamps its own clock.
        assert_eq!(pane.take_fx_request(), None, "{command}");
        // The held prompt fires through the normal send path.
        pane.fire_fx_prompt();
        let drained = pane.drain_pending_outbound();
        assert!(
            drained
                .iter()
                .any(|cmd| matches!(cmd, OutboundAgentCommand::SendPrompt { .. })),
            "{command} must send its follow-up prompt: {drained:?}"
        );
        pane.start_new_conversation();
        pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    }
}

#[test]
fn slash_goal_surfaces_wire_gap_instead_of_inventing_protocol() {
    let mut pane = NeoismAgentPane::default();
    pane.execute_slash_text("/goal ship it");
    assert!(pane.drain_pending_outbound().is_empty());
    assert!(pane.messages.iter().any(|message| message.title == "Goal"
        && message.text.contains("no session has started yet")));

    pane.set_session_id(Some("sess-1".to_string()));
    pane.execute_slash_text("/goal");
    assert!(pane.drain_pending_outbound().is_empty());
    assert!(pane.messages.iter().any(|message| message.title == "Goal"
        && message
            .text
            .contains("aren't available over this connection")));
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
fn first_model_and_thinking_choices_request_insert_only_config_persistence() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_model("opencode/free".to_string());
    pane.apply_thinking("high".to_string());

    let drained = pane.drain_pending_outbound();
    assert!(drained.iter().any(|command| matches!(
        command,
        OutboundAgentCommand::PersistConfigChoice {
            model: Some(model),
            thinking: None,
        } if model == "opencode/free"
    )));
    assert!(drained.iter().any(|command| matches!(
        command,
        OutboundAgentCommand::PersistConfigChoice {
            model: None,
            thinking: Some(thinking),
        } if thinking == "high"
    )));
}

#[test]
fn provider_catalog_limit_survives_limitless_state_updates() {
    let mut pane = NeoismAgentPane::default();
    pane.set_model_context_limits(HashMap::from([(
        "openai/gpt-5.6-sol".to_string(),
        400_000,
    )]));

    // Config defaults may arrive after the provider catalog.
    pane.apply_provider_state(
        None,
        Some("openai/gpt-5.6-sol".to_string()),
        Some("build".to_string()),
        Some("medium".to_string()),
        None,
    );
    assert_eq!(pane.model_context_limit, Some(400_000));

    // Subsequent agent/thinking updates also omit the limit.
    pane.apply_provider_state(None, None, None, Some("high".to_string()), None);
    assert_eq!(pane.model_context_limit, Some(400_000));
}

#[test]
fn provider_catalog_limit_applies_when_catalog_arrives_after_model() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_provider_state(
        None,
        Some("openai/gpt-5.6-sol".to_string()),
        None,
        None,
        None,
    );
    assert_eq!(pane.model_context_limit, None);

    pane.set_model_context_limits(HashMap::from([(
        "openai/gpt-5.6-sol".to_string(),
        400_000,
    )]));
    assert_eq!(pane.model_context_limit, Some(400_000));
}

#[test]
fn idle_streaming_state_clears_status_label_after_grace() {
    let mut pane = NeoismAgentPane::default();

    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert!(pane.is_streaming());
    assert_eq!(pane.streaming_label(), "Crafting");

    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert!(!pane.is_streaming());
    // A transient idle reading holds the displayed label (and its clock)
    // so the status row never blinks out between events…
    assert_eq!(pane.streaming_label(), "Crafting");
    assert!(pane.has_status_activity());
    assert!(pane.streaming_elapsed_seconds().is_some());
    // …while idle sustained past the grace window clears it for real.
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_label(), "");
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_elapsed_seconds(), None);
}

#[test]
fn main_agent_verb_wins_while_it_streams_over_running_subagents() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    let original_clock = pane.subagent_waiting_started_at;
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );

    // The main agent starts talking (the user sent a prompt / the model
    // is responding): its own verb always wins over the aggregate
    // sub-agents label while it is actively streaming.
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::Generating
    );
    assert_eq!(pane.streaming_label(), "Crafting");
    pane.note_streaming(NeoismAgentStreamingState::Thinking, None);
    assert_eq!(pane.streaming_label(), "Pondering");

    // The main agent stops while the same child keeps running: only now
    // does "Sub-agents working" take over — and on the SAME waiting
    // clock (no restart, the child never stopped).
    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert_eq!(pane.streaming_label(), "Sub-agents working");
    assert_eq!(pane.subagent_waiting_started_at, original_clock);
}

/// The raw derivation can pass through Idle for a tick between events
/// (MessageEnd → next MessageStart, a child's inter-message idle edge).
/// The display must bridge those gaps: label, activity flag (which
/// reserves the status row's height) and clock all keep their values,
/// so the composer never drops a line and bounces mid-run.
#[test]
fn transient_idle_gap_keeps_status_label_and_row() {
    // Verb gap: MessageEnd → MessageStart.
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.messages = vec![NeoismAgentMessage::user("go")];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert_eq!(pane.streaming_label(), "Crafting");

    pane.note_streaming(NeoismAgentStreamingState::Idle, None); // gap opens
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::Generating
    );
    assert_eq!(pane.streaming_label(), "Crafting");
    assert!(pane.has_status_activity(), "status row must stay reserved");
    assert!(pane.streaming_elapsed_seconds().is_some());
    assert_eq!(
        pane.animation_reason(),
        Some("streaming_status_hold"),
        "hold must drive frames so it can expire on its own"
    );

    pane.note_streaming(NeoismAgentStreamingState::Generating, None); // gap closes
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::Generating
    );

    // Sub-agents gap: a child's idle edge zeroes the active count for a
    // beat before the next lifecycle event re-raises it.
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    assert_eq!(pane.streaming_label(), "Sub-agents working");

    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    pane.sync_subagent_waiting_clock(); // gap opens
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert_eq!(pane.streaming_label(), "Sub-agents working");
    assert!(pane.has_status_activity());

    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock(); // gap closes (respawn / next step)
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
}

#[test]
fn sustained_idle_clears_status_label_and_activity() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.messages = vec![NeoismAgentMessage::user("go")];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert_eq!(pane.streaming_label(), "Crafting");

    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);

    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(pane.streaming_label(), "");
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_elapsed_seconds(), None);
    // An expired hold must not keep owning redraw frames.
    assert_ne!(pane.animation_reason(), Some("streaming_status_hold"));
    assert_ne!(pane.animation_reason(), Some("streaming"));
}

#[test]
fn user_abort_clears_status_label_without_grace_lag() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert_eq!(pane.streaming_label(), "Crafting");

    pane.abort_session();

    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(pane.streaming_label(), "");
    assert!(!pane.has_status_activity());
}

#[test]
fn held_status_never_leaks_across_sessions() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel
        .set_viewed_session_id(Some("parent".to_string()));
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert_eq!(pane.streaming_label(), "Crafting");

    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert_eq!(pane.streaming_label(), "Crafting"); // held in-session

    // Switching to an unrelated session must not show the previous
    // conversation's held label for even one frame.
    pane.switch_session("unrelated".to_string());
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(pane.streaming_label(), "");
}

#[test]
fn viewed_child_shows_its_own_verb_not_the_sibling_aggregate() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child-1".to_string());
    pane.parent_session_id = Some("parent".to_string());
    pane.side_panel.ensure_subagent_main_entry("parent");
    pane.side_panel
        .upsert_subagent("child-2", "Write the docs", "build");
    pane.note_subagent_runtime("child-2".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();

    // While the viewed child streams, its own verb is the label even
    // though a sibling is running.
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::Generating
    );
    assert_eq!(pane.streaming_label(), "Crafting");

    // A child view never shows the parent's aggregate label: after its
    // own verb's grace hold expires, the display is Idle even though a
    // sibling is still running.
    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::Generating
    );
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(pane.streaming_label(), "");
}

#[test]
fn session_idle_clears_a_partially_restored_streaming_state() {
    let mut pane = NeoismAgentPane::default();
    pane.streaming_state = NeoismAgentStreamingState::Generating;
    pane.streaming_started_at = None;

    pane.note_session_idle();

    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(pane.streaming_label(), "");
}

#[test]
fn viewed_subagent_completion_clears_its_activity_label() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child-1".to_string());
    pane.parent_session_id = Some("parent".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    pane.note_subagent_event(
        "child-1".to_string(),
        BranchStatus::Completed,
        None,
        None,
        None,
        None,
    );

    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(pane.streaming_label(), "");
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
fn initial_session_skeleton_animates_until_sessions_load() {
    let mut pane = NeoismAgentPane::default();

    assert!(pane.side_panel.is_animating());
    pane.side_panel.set_sessions(Vec::new());
    assert!(!pane.side_panel.is_animating());
}

#[test]
fn subagent_session_suppresses_stale_main_composer_cursor() {
    let mut pane = NeoismAgentPane::default();
    let rect = [10.0, 20.0, 2.0, 18.0];
    pane.set_cursor_rect(Some(rect));
    assert_eq!(pane.cursor_rect(), Some(rect));

    pane.parent_session_id = Some("main".to_string());
    assert_eq!(pane.cursor_rect(), None);
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
    assert!(!pane.active_subagent_ids.contains("child-1"));
}

#[test]
fn runtime_child_keeps_waiting_status_before_sidebar_hydrates() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());

    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, Some(1));
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );

    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    pane.sync_subagent_waiting_clock();
    assert_eq!(pane.active_subagent_count(), 0);
    // The completion edge is bridged by the display grace hold, then
    // sustained idle clears the label.
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
}

fn child_permission(id: &str) -> NeoismAgentPendingPermission {
    NeoismAgentPendingPermission {
        id: id.to_string(),
        session_id: "child-1".to_string(),
        parent_session_id: Some("parent".to_string()),
        source_agent: None,
        source_title: None,
        title: "Run command".to_string(),
        permission: "bash".to_string(),
        patterns: Vec::new(),
        selected: 0,
        responding: false,
    }
}

#[test]
fn completed_child_ignores_late_permission_reply_activity() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.enqueue_pending_permission(child_permission("perm-1"));
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);

    assert!(pane.note_permission_replied("perm-1", Some("child-1")));
    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Completed)
    );
    assert!(pane.side_panel.branch_terminal_locked("child-1"));
}

#[test]
fn completed_child_ignores_stale_permission_request_activity() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);

    pane.enqueue_pending_permission(child_permission("perm-late"));

    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Completed)
    );
    assert!(pane.side_panel.branch_terminal_locked("child-1"));
}

#[test]
fn authoritative_child_continuation_reopens_completed_branch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);

    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, Some(2));

    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Active)
    );
    assert!(!pane.side_panel.branch_terminal_locked("child-1"));
}

#[test]
fn part_activity_hydrates_missing_row_but_terminal_straggler_does_not() {
    let mut live = NeoismAgentPane::default();
    live.session_id = Some("parent".to_string());
    assert!(live.note_subagent_part_activity(
        "child-1".to_string(),
        BranchStatus::Active,
        Some("responding".to_string()),
        Some(1),
    ));
    assert!(live
        .side_panel
        .subagents()
        .iter()
        .any(|entry| entry.id == "child-1"));

    let mut completed = NeoismAgentPane::default();
    completed.session_id = Some("parent".to_string());
    completed.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    assert!(!completed.note_subagent_part_activity(
        "child-1".to_string(),
        BranchStatus::Active,
        Some("responding".to_string()),
        Some(2),
    ));
    assert!(!completed
        .side_panel
        .subagents()
        .iter()
        .any(|entry| entry.id == "child-1"));
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
        "job_id: job-1 (use this with background_task_result)\nstatus: running\ncommand: cargo build",
        "completed",
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
    let mut running_result = NeoismAgentMessage::tool(
        "Background Task Result",
        "job_id: job-1\nstatus: running",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    running_result.detail = running_result.text.clone();
    pane.messages.push(running_result);
    pane.refresh_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::BackgroundTasks
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
    // The display grace hold bridges the collection edge; sustained idle
    // then clears it.
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::BackgroundTasks
    );
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.background_task_details_expanded());
}

#[test]
fn historical_unmatched_background_task_is_not_live_activity() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: stale-job\nstatus: running\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.animation_reason(), None);
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
        "completed",
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
fn delayed_live_user_echo_does_not_move_prompt_after_long_response() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("rare delayed echo").with_id("msg-user-1"),
        NeoismAgentMessage::assistant("long response\n".repeat(2_000))
            .with_id("msg-assistant-1"),
    ];
    let live_echo =
        crate::panels::agent_pane::api_mapping::part_block(&serde_json::json!({
            "id": "part-user-1",
            "messageID": "msg-user-1",
            "type": "text",
            "role": "user",
            "text": "rare delayed echo"
        }))
        .expect("user part");

    pane.upsert_part_message(live_echo);

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::User);
    assert_eq!(pane.messages[0].id, "msg-user-1");
    assert_eq!(pane.messages[1].kind, NeoismAgentMessageKind::Assistant);
}

#[test]
fn streamed_user_image_and_text_merge_in_either_order() {
    for image_first in [false, true] {
        let mut pane = NeoismAgentPane::default();
        let image = NeoismAgentImage {
            filename: "clipboard.png".to_string(),
            url: "data:image/png;base64,AA==".to_string(),
            mime: "image/png".to_string(),
        };
        let mut optimistic = NeoismAgentMessage::user("[image1] inspect this");
        optimistic.images.push(image.clone());
        pane.messages.push(optimistic);

        let mut text_part = NeoismAgentMessage::user("[image1] inspect this");
        text_part.id = "msg-user-1".to_string();
        let mut image_part = NeoismAgentMessage::user("");
        image_part.id = "msg-user-1".to_string();
        image_part.images.push(image.clone());

        if image_first {
            pane.upsert_part_message(image_part);
            pane.upsert_part_message(text_part);
        } else {
            pane.upsert_part_message(text_part);
            pane.upsert_part_message(image_part);
        }

        assert_eq!(pane.messages.len(), 1, "image_first={image_first}");
        assert_eq!(pane.messages[0].id, "msg-user-1");
        assert_eq!(pane.messages[0].text, "[image1] inspect this");
        assert_eq!(pane.messages[0].images, vec![image]);
    }
}

#[test]
fn empty_background_task_snapshot_clears_stale_running_jobs() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);

    let mut snapshot = NeoismAgentMessage::tool(
        "Background tasks",
        "No background tasks exist for this session yet.",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    snapshot.detail = snapshot.text.clone();
    pane.messages.push(snapshot);
    pane.refresh_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn cancelled_background_task_clears_running_job() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);
    let mut cancelled = NeoismAgentMessage::tool(
        "Background task",
        "job_id: job-1\nstatus: cancelled",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    cancelled.detail = cancelled.text.clone();
    pane.messages.push(cancelled);

    assert_eq!(pane.running_background_task_count(), 0);
    assert!(pane.active_background_task_summaries().is_empty());
}

/// The card the live `session.background_task.completed` path injects
/// (desktop `pane/ingest.rs::BackgroundTaskCompleted`) — same shape the
/// shared `api_mapping::background_completion_card` regenerates from the
/// persisted runtime prompt.
fn client_background_completion_card(job_id: &str) -> NeoismAgentMessage {
    let mut card = NeoismAgentMessage::tool(
        "background_task_result",
        format!("job_id: {job_id}\nstatus: completed\nbackground shell task finished"),
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    )
    .with_id(format!("background-task-{job_id}"));
    card.detail = card.text.clone();
    card
}

#[test]
fn background_completion_card_survives_history_snapshot_replacement() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![
        NeoismAgentMessage::user("kick off the build").with_id("u-1"),
        NeoismAgentMessage::assistant("Started.").with_id("a-1"),
    ]);
    pane.upsert_part_message(client_background_completion_card("job-1"));

    // A refresh lands BEFORE the server's queued completion prompt has
    // drained: the snapshot carries no trace of the card. It must survive,
    // anchored after the content that preceded it.
    pane.apply_history(vec![
        NeoismAgentMessage::user("kick off the build").with_id("u-1"),
        NeoismAgentMessage::assistant("Started.").with_id("a-1"),
    ]);

    assert_eq!(pane.messages.len(), 3);
    assert_eq!(pane.messages[2].id, "background-task-job-1");

    // Repeated refreshes (with newer turns appended) keep it in its
    // chronological slot — after "a-1", before the newer prompt.
    pane.apply_history(vec![
        NeoismAgentMessage::user("kick off the build").with_id("u-1"),
        NeoismAgentMessage::assistant("Started.").with_id("a-1"),
        NeoismAgentMessage::user("second question").with_id("u-2"),
    ]);
    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| message.id == "background-task-job-1")
            .count(),
        1
    );
    assert_eq!(pane.messages[2].id, "background-task-job-1");
    assert_eq!(pane.messages[3].id, "u-2");
}

#[test]
fn server_background_completion_copy_replaces_client_card_without_duplicate() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![NeoismAgentMessage::user("run it").with_id("u-1")]);
    pane.upsert_part_message(client_background_completion_card("job-1"));

    // Once the runtime prompt drains, snapshots carry the persisted copy
    // under the SAME durable id (`api_mapping::background_completion_card`)
    // with the full captured output in detail — it replaces the client
    // copy instead of duplicating or wiping it.
    let mut server_copy = NeoismAgentMessage::tool(
        "background_task_result",
        "job_id: job-1\nstatus: completed\nbackground shell task finished",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    )
    .with_id("background-task-job-1");
    server_copy.detail =
        "Background shell task finished.\njob_id: job-1\nstatus: completed\n<task_result>ok</task_result>"
            .to_string();
    pane.apply_history(vec![
        NeoismAgentMessage::user("run it").with_id("u-1"),
        server_copy,
    ]);

    let cards: Vec<_> = pane
        .messages
        .iter()
        .filter(|message| message.id == "background-task-job-1")
        .collect();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].detail.contains("<task_result>"), "server copy won");
}

#[test]
fn snapshot_with_matching_job_id_suppresses_card_reinsertion() {
    let mut pane = NeoismAgentPane::default();
    pane.upsert_part_message(client_background_completion_card("job-1"));

    // A server-visible completion for the same job under a DIFFERENT id
    // (e.g. a background_task_result tool-call row that reread the
    // finished output) counts as the server copy — no duplicate insert.
    let mut reread = NeoismAgentMessage::tool(
        "background_task_result",
        "job_id: job-1\nstatus: completed",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    )
    .with_id("prt-reread-1");
    reread.detail = reread.text.clone();
    pane.apply_history(vec![
        NeoismAgentMessage::user("check it").with_id("u-1"),
        reread,
    ]);

    assert!(pane
        .messages
        .iter()
        .all(|message| message.id != "background-task-job-1"));
    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| message.tool == "background_task_result")
            .count(),
        1
    );
}

#[test]
fn cached_background_completion_card_survives_park_restore_and_refresh() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-a".to_string()));

    // Completion for a parked session streams into its cache entry…
    pane.cache_upsert_part_message("sess-b", client_background_completion_card("job-9"));
    // …and the background history refresh (server copy not persisted yet)
    // keeps it through the snapshot merge.
    pane.apply_history_to_cache(
        "sess-b",
        vec![NeoismAgentMessage::user("bg question").with_id("u-1")],
        None,
    );
    {
        let cached = pane.session_cache.get("sess-b").expect("entry");
        assert!(cached
            .messages
            .iter()
            .any(|message| message.id == "background-task-job-9"));
    }

    // Switching to the parked session restores the card…
    pane.switch_session("sess-b".to_string());
    assert!(pane
        .messages
        .iter()
        .any(|message| message.id == "background-task-job-9"));

    // …and an active-session refresh after the restore still keeps it.
    pane.apply_history(vec![NeoismAgentMessage::user("bg question").with_id("u-1")]);
    assert!(pane
        .messages
        .iter()
        .any(|message| message.id == "background-task-job-9"));
}

#[test]
fn background_status_is_scoped_to_pane_session_messages() {
    let mut pane_with_job = NeoismAgentPane::default();
    pane_with_job.session_id = Some("session-a".to_string());
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "completed",
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
fn active_subagent_part_updates_do_not_restart_waiting_clock() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel
        .set_subagents(vec![NeoismAgentSessionEntry::new(
            "child-1", "child", "explore",
        )
        .with_runtime_status(Some("running".to_string()))]);
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    let original = pane.subagent_waiting_started_at;

    assert!(pane.note_subagent_part_activity(
        "child-1".to_string(),
        BranchStatus::Active,
        Some("read".to_string()),
        Some(1),
    ));
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.subagent_waiting_started_at, original);
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
    // Grace hold bridges the last child's completion edge, then
    // sustained idle clears the label and the row's reserved activity.
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
}

/// Force-stop / last-child-finished must not leave the parent footer on
/// "Sub-agents working". The children themselves are already idle; the
/// GUI roster was the thing still claiming they were Active.
#[test]
fn abort_settles_stale_working_subagents() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.ensure_subagent_main_entry("parent");
    pane.side_panel
        .upsert_subagent("child-1", "Map frontend", "explore");
    pane.side_panel
        .upsert_subagent("child-2", "Map backend", "explore");
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    pane.note_subagent_runtime("child-2".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    assert_eq!(pane.active_subagent_count(), 2);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert!(pane.subagent_waiting_started_at.is_some());

    pane.abort_session();

    assert_eq!(pane.active_subagent_count(), 0);
    assert!(pane.subagent_waiting_started_at.is_none());
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(
        pane.side_panel
            .branch_activity("child-1")
            .map(|activity| activity.status),
        Some(BranchStatus::Stopped)
    );
    assert_eq!(
        pane.side_panel
            .branch_activity("child-2")
            .map(|activity| activity.status),
        Some(BranchStatus::Stopped)
    );
}

/// A subagent conversation is an EXTENSION of the main chat: entering a
/// child must keep the parent-keyed roster (names, statuses, running
/// count) alive in the sidebar instead of clearing it.
#[test]
fn roster_survives_entering_subagent_session() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.ensure_subagent_main_entry("parent");
    pane.side_panel
        .upsert_subagent("child-1", "Fix the tests", "explore");
    pane.side_panel
        .upsert_subagent("child-2", "Write the docs", "build");
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    pane.note_subagent_runtime("child-2".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    assert_eq!(pane.active_subagent_count(), 2);

    // Cold switch into a child (no hydrated cache entry).
    pane.switch_session("child-1".to_string());

    assert_eq!(pane.session_id.as_deref(), Some("child-1"));
    assert_eq!(pane.parent_session_id.as_deref(), Some("parent"));
    assert!(pane.is_subagent_session());
    let roster: Vec<&str> = pane
        .side_panel
        .subagents()
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(roster, vec!["parent", "child-1", "child-2"]);
    assert_eq!(
        pane.side_panel
            .subagents()
            .iter()
            .find(|entry| entry.id == "child-2")
            .map(|entry| entry.title.as_str()),
        Some("Write the docs")
    );
    // The sibling still reads as running from the child's viewpoint.
    assert_eq!(pane.side_panel.active_child_count(Some("child-1")), 1);

    // Sibling lifecycle updates keep applying to the roster while the
    // child transcript is open.
    assert!(pane.note_family_session_streaming("child-2", false));
    assert!(matches!(
        pane.side_panel
            .branch_activity("child-2")
            .map(|activity| activity.status),
        Some(BranchStatus::Completed)
    ));
    assert_eq!(pane.side_panel.active_child_count(Some("child-1")), 0);
}

#[test]
fn roster_survives_cached_switch_into_subagent() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.ensure_subagent_main_entry("parent");
    pane.side_panel
        .upsert_subagent("child-1", "Fix the tests", "explore");
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    // Hydrate the child's cache slot so the switch takes the instant
    // restore path (`activate_cached_session`).
    pane.apply_history_to_cache("child-1", vec![NeoismAgentMessage::user("go")], None);
    assert!(pane.cached_session_is_hydrated("child-1"));

    pane.switch_session("child-1".to_string());

    assert_eq!(pane.session_id.as_deref(), Some("child-1"));
    // The live-only cache slot had no session metadata; the parent
    // linkage is derived from the roster so the child still opens as a
    // view-only subagent transcript.
    assert_eq!(pane.parent_session_id.as_deref(), Some("parent"));
    let roster: Vec<&str> = pane
        .side_panel
        .subagents()
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(roster, vec!["parent", "child-1"]);

    // Returning to the parent keeps the roster intact as well.
    pane.switch_session("parent".to_string());
    assert_eq!(pane.session_id.as_deref(), Some("parent"));
    assert!(pane.parent_session_id.is_none());
    let roster: Vec<&str> = pane
        .side_panel
        .subagents()
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(roster, vec!["parent", "child-1"]);
}

/// Clicking a Task card raises/expands it (that's also the navigation
/// affordance into the subagent). Returning to the parent must render
/// the timeline fresh — the raised card must NOT still be up showing
/// its title, demanding an extra click to dismiss.
#[test]
fn task_card_expansion_clears_on_subagent_round_trip() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.ensure_subagent_main_entry("parent");
    pane.side_panel
        .upsert_subagent("child-1", "Fix the tests", "explore");
    pane.messages = vec![task_tool_message("child-1", "running")];

    // The navigation click expands the Task card…
    pane.register_tool_hit_rect("task-card".to_string(), [0.0, 0.0, 200.0, 40.0]);
    assert!(pane.toggle_tool_at(10.0, 10.0));
    assert!(pane.tool_expanded("task-card"));
    assert!(pane.tool_expand_animating("task-card"));

    // …then the user enters the child and comes back to the parent.
    pane.switch_session("child-1".to_string());
    assert!(
        !pane.tool_expanded("task-card"),
        "child timeline must not inherit the parent's expansion"
    );
    pane.switch_session("parent".to_string());

    assert!(
        !pane.tool_expanded("task-card"),
        "returning must render the timeline fresh — no raised Task card"
    );
    assert!(!pane.tool_expand_animating("task-card"));
    assert!(!pane.any_tool_expand_animating());
}

/// Leave-and-return collapses the live-trace window. A parked parent
/// layout from the previous visit still contains the tool rows that
/// were visible while you were inside the child; restoring that cache
/// would paint leftover titles that vanish on click. The restore must
/// drop the parked layout and hide settled tools.
#[test]
fn settled_tool_titles_hide_after_subagent_round_trip() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.ensure_subagent_main_entry("parent");
    pane.side_panel
        .upsert_subagent("child-1", "Fix the tests", "explore");
    pane.messages = vec![
        NeoismAgentMessage::user("look around"),
        NeoismAgentMessage::tool(
            "Read(src/lib.rs)",
            "Read preview",
            "completed",
            "read",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("read-1"),
        NeoismAgentMessage::assistant("done"),
    ];
    pane.timeline_live_trace_start = Some(1);
    pane.timeline_live_trace_anchor = Some(pane.messages[0].id.clone());
    *pane.timeline_layout_cache.borrow_mut() = Some(TimelineLayoutCache {
        epoch: pane.timeline_layout_epoch,
        source_len: pane.messages.len(),
        width_bucket: 0,
        scale_bucket: 0,
        gap_bucket: 0,
        content_height: 120.0,
        pages: Vec::new(),
        rows: vec![TimelineLayoutRow {
            source_index: 1,
            source_end_index: 1,
            top: 40.0,
            height: 30.0,
            display_text: Some("Read(src/lib.rs)".to_string()),
            display_message: Some(pane.messages[1].clone()),
            markdown_blocks: None,
            tool_diff_sections: None,
            is_edit_tool: false,
        }],
    });

    pane.apply_history_to_cache("child-1", vec![NeoismAgentMessage::user("go")], None);
    pane.switch_session("child-1".to_string());
    pane.switch_session("parent".to_string());

    assert_eq!(pane.timeline_live_trace_start, None);
    assert!(pane.timeline_layout_cache.borrow().is_none());
    assert!(
        pane.tool_archived("read-1"),
        "settled tools must stay archived after leave-and-return"
    );
}

#[test]
fn group_child_selection_and_link_hover_clear_on_session_switch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.register_tool_hit_rect(
        "group-1::child::read-2".to_string(),
        [0.0, 0.0, 200.0, 40.0],
    );
    assert!(pane.toggle_tool_at(10.0, 10.0));
    assert_eq!(pane.selected_tool_group_child("group-1"), Some("read-2"));
    pane.register_link_hit_rect("src/main.rs".to_string(), [0.0, 50.0, 100.0, 12.0]);
    assert!(pane.update_link_hover_at(10.0, 55.0));
    assert!(pane.link_hovered("src/main.rs"));

    pane.switch_session("unrelated".to_string());

    assert_eq!(pane.selected_tool_group_child("group-1"), None);
    assert!(!pane.link_hovered("src/main.rs"));
    assert!(!pane.link_hover_active());
    // Stale hit rects from the previous timeline are gone too — a click
    // at the old coordinates falls through instead of re-toggling.
    assert!(!pane.toggle_tool_at(10.0, 10.0));
}

#[test]
fn roster_clears_when_leaving_the_session_family() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.ensure_subagent_main_entry("parent");
    pane.side_panel
        .upsert_subagent("child-1", "Fix the tests", "explore");

    pane.switch_session("unrelated".to_string());

    assert_eq!(pane.session_id.as_deref(), Some("unrelated"));
    assert!(pane.parent_session_id.is_none());
    assert!(pane.side_panel.subagents().is_empty());
}

#[test]
fn family_streaming_edges_only_touch_tracked_rows() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child-1".to_string());
    pane.parent_session_id = Some("parent".to_string());
    pane.side_panel.ensure_subagent_main_entry("parent");
    pane.side_panel
        .upsert_subagent("child-1", "Fix the tests", "explore");
    pane.side_panel
        .upsert_subagent("child-2", "Write the docs", "build");

    // Untracked sessions never pollute the family roster.
    assert!(!pane.note_family_session_streaming("stranger", true));
    assert!(pane
        .side_panel
        .subagents()
        .iter()
        .all(|entry| entry.id != "stranger"));

    // A tracked sibling's edges apply while a child is on screen…
    assert!(pane.note_family_session_streaming("child-2", true));
    assert_eq!(pane.side_panel.active_child_count(Some("child-1")), 1);
    assert!(pane.note_family_session_streaming("child-2", false));
    assert_eq!(pane.side_panel.active_child_count(Some("child-1")), 0);

    // …but a straggler active edge cannot resurrect a sibling whose
    // idle edge already latched the terminal lock.
    assert!(!pane.note_family_session_streaming("child-2", true));
    assert_eq!(pane.side_panel.active_child_count(Some("child-1")), 0);

    // The viewed session itself is never routed through the roster.
    assert!(!pane.note_family_session_streaming("child-1", true));
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
fn thousands_of_stream_deltas_coalesce_into_one_timeline_row() {
    let mut pane = NeoismAgentPane::default();
    let layout_epoch = pane.timeline_layout_epoch;

    for _ in 0..4_096 {
        pane.apply_part_delta(
            Some("answer-part".to_string()),
            Some("answer-part".to_string()),
            Some("text".to_string()),
            "x",
        );
    }

    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].id, "answer-part");
    assert_eq!(pane.messages[0].text.len(), 4_096);
    assert_eq!(pane.timeline_layout_epoch, layout_epoch);
    let dirty = pane.take_timeline_dirty_marks();
    assert_eq!(dirty.indices.into_iter().collect::<Vec<_>>(), vec![0]);
}

#[test]
fn empty_stream_deltas_do_not_invalidate_or_create_timeline_rows() {
    let mut pane = NeoismAgentPane::default();
    let layout_epoch = pane.timeline_layout_epoch;
    let content_revision = pane.timeline_content_revision;

    pane.apply_part_delta(
        Some("answer-part".to_string()),
        Some("answer-part".to_string()),
        Some("text".to_string()),
        "",
    );

    assert!(pane.messages.is_empty());
    assert_eq!(pane.timeline_layout_epoch, layout_epoch);
    assert_eq!(pane.timeline_content_revision, content_revision);
    assert!(pane.take_timeline_dirty_marks().indices.is_empty());
}

#[test]
fn message_id_delta_updates_its_row_instead_of_the_latest_same_kind() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::assistant("first").with_id("answer-1"),
        NeoismAgentMessage::assistant("second").with_id("answer-2"),
    ];

    pane.apply_part_delta(
        Some("answer-1".to_string()),
        None,
        Some("text".to_string()),
        " updated",
    );

    assert_eq!(pane.messages[0].text, "first updated");
    assert_eq!(pane.messages[1].text, "second");
    let dirty = pane.take_timeline_dirty_marks();
    assert_eq!(dirty.indices.into_iter().collect::<Vec<_>>(), vec![0]);
}

#[test]
fn finalizing_tool_patches_only_the_tool_and_its_spacing_neighbor() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::reasoning("plan").with_id("reason-1"),
        NeoismAgentMessage::tool(
            "Bash(cargo test)",
            "",
            "running",
            "bash",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-1"),
        NeoismAgentMessage::assistant("waiting").with_id("answer-1"),
    ];
    let layout_epoch = pane.timeline_layout_epoch;

    pane.finalize_tool_card(
        "tool-1",
        "completed",
        Some("all tests passed".to_string()),
        None,
    );

    assert_eq!(pane.messages[1].status, "completed");
    assert_eq!(pane.messages[1].text, "all tests passed");
    assert_eq!(pane.timeline_layout_epoch, layout_epoch);
    let dirty = pane.take_timeline_dirty_marks();
    assert_eq!(dirty.indices.into_iter().collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn removing_a_stream_part_forces_one_structural_rebuild_and_clears_patch_marks() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("question").with_id("user-1"),
        NeoismAgentMessage::assistant("answer").with_id("answer-1"),
    ];
    pane.apply_part_delta(
        Some("answer-1".to_string()),
        None,
        Some("text".to_string()),
        " tail",
    );
    let layout_epoch = pane.timeline_layout_epoch;

    pane.remove_part_message("answer-1");

    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.timeline_layout_epoch, layout_epoch.wrapping_add(1));
    assert!(pane.take_timeline_dirty_marks().indices.is_empty());
}

#[test]
fn unchanged_virtual_timeline_sync_does_not_revise_any_nodes() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("stable-session".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("question").with_id("user-1"),
        NeoismAgentMessage::reasoning("thought").with_id("reason-1"),
        NeoismAgentMessage::assistant("answer").with_id("answer-1"),
    ];
    pane.sync_virtual_timeline([0.0, 0.0, 500.0, 200.0], 500.0, 0.0, 0.0, 1.0, &[]);
    let revisions = pane
        .virtual_timeline
        .surface
        .nodes()
        .iter()
        .map(|node| node.revision)
        .collect::<Vec<_>>();
    let transcript_revision = pane.virtual_timeline.revision;

    pane.sync_virtual_timeline([0.0, 0.0, 500.0, 200.0], 500.0, 0.0, 40.0, 1.0, &[]);

    assert_eq!(pane.virtual_timeline.revision, transcript_revision);
    assert_eq!(
        pane.virtual_timeline
            .surface
            .nodes()
            .iter()
            .map(|node| node.revision)
            .collect::<Vec<_>>(),
        revisions
    );
}

#[test]
fn complete_stream_lifecycle_rehydrates_without_duplicates_or_stale_text() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![NeoismAgentMessage::user("fix it").with_id("user-1")];
    pane.note_streaming(NeoismAgentStreamingState::Thinking, None);
    pane.apply_part_delta(
        None,
        Some("reason-1".to_string()),
        Some("reasoning".to_string()),
        "planning",
    );
    pane.upsert_tool_card(
        "tool-1".to_string(),
        "bash".to_string(),
        "Bash(cargo test)".to_string(),
        "running".to_string(),
        String::new(),
        NeoismAgentOutputKind::Text,
        String::new(),
    );
    pane.finalize_tool_card(
        "tool-1",
        "completed",
        Some("tests passed".to_string()),
        None,
    );
    pane.apply_part_delta(
        None,
        Some("answer-1".to_string()),
        Some("text".to_string()),
        "Done.",
    );
    pane.note_session_idle();

    let mut stale_snapshot = pane.messages.clone();
    stale_snapshot
        .iter_mut()
        .find(|message| message.id == "answer-1")
        .expect("answer")
        .text
        .clear();
    let refreshed = pane.preserve_streamed_response_text(stale_snapshot);
    pane.apply_history(refreshed);

    assert_eq!(
        pane.messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["user-1", "reason-1", "tool-1", "answer-1"]
    );
    assert_eq!(pane.messages[2].status, "completed");
    assert_eq!(pane.messages[2].text, "tests passed");
    assert_eq!(pane.messages[3].text, "Done.");
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(pane.streaming_label(), "");
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
fn history_refresh_keeps_live_trace_anchored_to_its_turn() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("latest").with_id("latest"),
        NeoismAgentMessage::reasoning("thinking").with_id("reasoning"),
        NeoismAgentMessage::assistant("tool").with_id("tool"),
    ];
    pane.timeline_live_trace_start = Some(1);
    pane.timeline_live_trace_anchor = Some("latest".to_string());

    // The refresh prepends an older turn; the marker follows its anchored
    // turn instead of drifting, so everything revealed this visit stays
    // revealed.
    pane.apply_history(vec![
        NeoismAgentMessage::user("old"),
        NeoismAgentMessage::assistant("old answer").with_id("old-answer"),
        NeoismAgentMessage::user("latest").with_id("latest"),
        NeoismAgentMessage::assistant("durable answer").with_id("answer"),
    ]);

    assert_eq!(pane.timeline_live_trace_start, Some(3));
    assert_eq!(pane.messages[3].text, "durable answer");

    // A newer prompt must NOT collapse the anchored turn's trace.
    pane.apply_history(vec![
        NeoismAgentMessage::user("old"),
        NeoismAgentMessage::assistant("old answer").with_id("old-answer"),
        NeoismAgentMessage::user("latest").with_id("latest"),
        NeoismAgentMessage::assistant("durable answer").with_id("answer"),
        NeoismAgentMessage::user("newer").with_id("newer"),
        NeoismAgentMessage::assistant("newer answer").with_id("newer-answer"),
    ]);
    assert_eq!(pane.timeline_live_trace_start, Some(3));
}

#[test]
fn diff_file_toggle_does_not_move_or_reanchor_the_timeline() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.timeline_velocity_px_s = 75.0;
    pane.register_tool_hit_rect("tool-1:0".to_string(), [20.0, 150.0, 300.0, 60.0]);
    let scroll_before = pane.timeline_scroll_px;
    let velocity_before = pane.timeline_velocity_px_s;

    assert!(pane.toggle_tool_at(30.0, 160.0));

    assert!(pane.tool_expanded("tool-1:0"));
    assert_eq!(pane.timeline_scroll_px, scroll_before);
    assert_eq!(pane.timeline_velocity_px_s, velocity_before);
    assert!(pane.pending_timeline_anchor.is_none());
    assert!(pane.timeline_view_anchor.is_none());
    assert!(!pane.tool_expand_animating("tool-1"));
}

#[test]
fn markdown_horizontal_scroll_is_block_local_and_geometry_is_frame_local() {
    let mut pane = NeoismAgentPane::default();
    pane.register_markdown_horizontal_scroll_rect(
        "markdown:message-1:code:0".to_string(),
        [20.0, 100.0, 300.0, 120.0],
        240.0,
    );
    pane.register_markdown_horizontal_scroll_rect(
        "markdown:message-1:table:1".to_string(),
        [20.0, 240.0, 300.0, 120.0],
        400.0,
    );

    assert!(pane.update_markdown_horizontal_scroll_hover(40.0, 140.0));
    assert!(pane.markdown_horizontal_scrollbar_visible("markdown:message-1:code:0"));
    assert!(pane.update_markdown_horizontal_scroll_hover(800.0, 800.0));
    assert!(!pane.markdown_horizontal_scrollbar_visible("markdown:message-1:code:0"));

    assert_eq!(
        pane.scroll_markdown_horizontal_at(40.0, 140.0, 75.0),
        Some(true)
    );
    assert_eq!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:code:0", 240.0),
        75.0
    );
    assert_eq!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:table:1", 400.0),
        0.0
    );

    // Render-start clearing removes stale hit targets but deliberately keeps
    // the content offset, which is restored when that block is drawn again.
    pane.clear_tool_hit_rects();
    assert_eq!(pane.scroll_markdown_horizontal_at(40.0, 140.0, 20.0), None);
    assert_eq!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:code:0", 240.0),
        75.0
    );

    pane.register_markdown_horizontal_scrollbar(
        "markdown:message-1:code:0".to_string(),
        [20.0, 200.0, 300.0, 16.0],
        [20.0, 200.0, 100.0, 16.0],
        240.0,
    );
    assert!(pane.begin_markdown_horizontal_scrollbar_drag(50.0, 208.0));
    // Frame-local geometry may be rebuilt while the pointer remains down;
    // the captured drag itself must survive that redraw.
    pane.clear_tool_hit_rects();
    assert!(pane.markdown_horizontal_scrollbar_dragging());
    assert!(pane.drag_markdown_horizontal_scrollbar_to(150.0));
    assert!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:code:0", 240.0) > 75.0
    );
    assert!(pane.end_markdown_horizontal_scrollbar_drag());
}

#[test]
fn code_copy_feedback_animates_then_hands_back_to_copy_label() {
    let mut pane = NeoismAgentPane::default();
    pane.mark_code_copied("neoism-copy-ref://code-1");

    assert!(pane
        .code_copy_feedback_progress("neoism-copy-ref://code-1")
        .is_some());
    assert!(pane
        .code_copy_feedback_progress("neoism-copy-ref://other")
        .is_none());
    assert_eq!(pane.animation_reason(), Some("code_copy_feedback"));

    pane.copied_code_feedback = Some((
        "neoism-copy-ref://code-1".to_string(),
        Instant::now() - CODE_COPY_FEEDBACK_ANIMATION - Duration::from_millis(1),
    ));
    assert!(pane
        .code_copy_feedback_progress("neoism-copy-ref://code-1")
        .is_none());
    assert_ne!(pane.animation_reason(), Some("code_copy_feedback"));
}

#[test]
fn older_timeline_request_requires_leaving_bottom_follow() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());

    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert!(!pane.timeline_history.loading_older);
    assert!(pane.drain_pending_outbound().is_empty());

    pane.timeline_follow_bottom = false;
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert!(pane.timeline_history.loading_older);
    let commands = pane.drain_pending_outbound();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        OutboundAgentCommand::LoadOlderTimeline { limit: 64, .. }
    ));

    pane.timeline_history.loading_older = false;
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert!(pane.drain_pending_outbound().is_empty());

    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 900.0, 300.0);
    assert!(pane.scroll_timeline_pixels(10.0));
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 1);
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
fn anchor_restore_shifts_active_wheel_target_by_actual_scroll_delta() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_follow_bottom = false;
    pane.timeline_scroll_px = 200.0;
    pane.timeline_wheel_target_px = Some(250.0);

    pane.restore_timeline_view_anchor(300.0, 0.0);

    assert_eq!(pane.timeline_scroll_px, 300.0);
    assert_eq!(pane.timeline_wheel_target_px, Some(350.0));
}

#[test]
fn identical_pending_prompts_consume_server_occurrences_one_to_one() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("same").with_id("local-1"),
        NeoismAgentMessage::user("same").with_id("local-2"),
    ];
    pane.pending_user_prompts = vec!["same".to_string(), "same".to_string()];

    let merged = pane.merge_pending_user_prompts(vec![
        NeoismAgentMessage::user("same").with_id("server-1")
    ]);

    assert_eq!(merged.len(), 2);
    assert_eq!(pane.pending_user_prompts, vec!["same"]);
    assert!(merged.iter().any(|message| message.id == "local-2"));
}

#[test]
fn mouse_wheel_notch_uses_a_fixed_spring_target() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_wheel_pixels(24.0));
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(pane.timeline_wheel_target_px, Some(24.0));
    assert_eq!(pane.timeline_velocity_px_s, 0.0);
    assert!(pane.timeline_is_inertial());
    pane.timeline_last_tick_at = Instant::now().checked_sub(Duration::from_millis(16));

    assert!(pane.tick_timeline_scroll());
    assert!(pane.timeline_scroll_offset() > 0.0);
    assert!(pane.timeline_scroll_offset() < 24.0);
}

#[test]
fn consecutive_mouse_wheel_notches_accumulate_a_deterministic_target() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_wheel_pixels(24.0));
    assert!(pane.scroll_timeline_wheel_pixels(24.0));
    assert_eq!(pane.timeline_wheel_target_px, Some(48.0));
}

#[test]
fn active_scroll_advances_stream_layout_anchor_with_reader() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.set_timeline_view_anchor(
        Some(
            crate::panels::agent_pane::view::timeline::TimelineViewAnchorKey::at_source(
                0,
                1,
                "visible-message",
            ),
        ),
        24.0,
    );

    assert!(pane.scroll_timeline_wheel_pixels(24.0));
    let (_, immediate_offset) = pane.timeline_view_anchor().expect("anchor");
    assert_eq!(immediate_offset, 24.0);
    pane.timeline_last_tick_at = Instant::now().checked_sub(Duration::from_millis(16));
    assert!(pane.tick_timeline_scroll());
    let (_, spring_offset) = pane.timeline_view_anchor().expect("anchor");
    assert!(spring_offset > immediate_offset);

    // A streamed block above the anchor grows by 200 px in the same frame.
    // Restoring the logical row must keep its post-wheel screen position.
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);
    let shifted_row_top = 624.0;
    pane.restore_timeline_view_anchor(shifted_row_top, spring_offset);
    let restored_scroll_top = pane.max_timeline_scroll() - pane.timeline_scroll_offset();
    assert!((shifted_row_top - restored_scroll_top - spring_offset).abs() < 0.01);

    // The spring moves the same logical anchor along with the reader. If streamed
    // code or a completed apply-patch card remeasures in this frame, restoring
    // this anchor preserves the newly scrolled-to position.
    pane.timeline_last_tick_at = Instant::now().checked_sub(Duration::from_millis(16));
    assert!(pane.tick_timeline_scroll());
    let (_, inertial_offset) = pane.timeline_view_anchor().expect("anchor");
    assert!(inertial_offset > spring_offset);
}

#[test]
fn trackpad_pixels_keep_the_direct_response_path() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_pixels(17.0));
    assert_eq!(pane.timeline_scroll_offset(), 17.0);
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
fn timeline_growth_respects_upward_scroll_intent_near_bottom() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_pixels(1.0));
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1000.0, 300.0);

    assert!((pane.timeline_scroll_offset() - 101.0).abs() < 0.01);
}

#[test]
fn returning_to_timeline_bottom_restores_following() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_pixels(100.0));
    assert!(pane.scroll_timeline_pixels(-100.0));
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1000.0, 300.0);

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

#[test]
fn history_chunk_mid_stream_keeps_optimistic_echo_and_streamed_text() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-1".to_string()));
    // Optimistic user echo of a just-sent prompt (the server store
    // hasn't caught up yet) + assistant text streamed ahead of the
    // snapshot fetch.
    pane.messages.push(NeoismAgentMessage::user("do the thing"));
    pane.remember_pending_user_prompt("do the thing");
    pane.apply_part_delta(
        None,
        Some("part-1".to_string()),
        Some("text".to_string()),
        "streamed answer",
    );

    // A stale HistoryChunk lands mid-stream: it neither contains the
    // user echo nor the full streamed assistant text.
    pane.apply_history(vec![
        NeoismAgentMessage::assistant("streamed").with_id("part-1")
    ]);

    assert!(
        pane.messages.iter().any(|message| {
            message.kind == NeoismAgentMessageKind::User && message.text == "do the thing"
        }),
        "optimistic echo dropped: {:?}",
        pane.messages
    );
    let assistant = pane
        .messages
        .iter()
        .find(|message| message.id == "part-1")
        .expect("assistant part");
    assert_eq!(assistant.text, "streamed answer");
    // Once a later chunk carries the stored user prompt, the pending
    // echo resolves instead of duplicating.
    pane.apply_history(vec![
        NeoismAgentMessage::user("do the thing").with_id("user-1"),
        NeoismAgentMessage::assistant("streamed answer").with_id("part-1"),
    ]);
    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| message.kind == NeoismAgentMessageKind::User
                && message.text == "do the thing")
            .count(),
        1
    );
}

#[test]
fn history_chunk_canonicalizes_expanded_prompt_echoes() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-1".to_string()));
    pane.insert_text("please review ");
    pane.insert_pasted_text_attachment("line one\nline two\nline three".to_string());
    let composer_text = pane.input().trim().to_string();
    let expanded = pane.expand_text_attachments(&composer_text);
    assert_ne!(
        expanded.trim(),
        composer_text,
        "paste token must expand on send"
    );
    assert!(pane.submit());
    let _ = pane.drain_pending_outbound();

    // The server echoes the EXPANDED prompt back; the timeline must keep
    // ONE bubble in the compact composer form.
    pane.apply_history(vec![
        NeoismAgentMessage::user(expanded.trim()).with_id("user-1")
    ]);
    let users = pane
        .messages
        .iter()
        .filter(|message| message.kind == NeoismAgentMessageKind::User)
        .collect::<Vec<_>>();
    assert_eq!(users.len(), 1, "expected one bubble: {:?}", pane.messages);
    assert_eq!(users[0].text, composer_text);
}

#[test]
fn switching_sessions_does_not_leak_pending_echoes_into_the_new_transcript() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-1".to_string()));
    pane.messages.push(NeoismAgentMessage::user("old prompt"));
    pane.remember_pending_user_prompt("old prompt");

    pane.switch_session("sess-2".to_string());
    let _ = pane.drain_pending_outbound();
    pane.apply_history(vec![
        NeoismAgentMessage::user("their prompt").with_id("user-9"),
        NeoismAgentMessage::assistant("their answer").with_id("answer-9"),
    ]);

    assert!(
        !pane
            .messages
            .iter()
            .any(|message| message.text == "old prompt"),
        "stale echo resurrected: {:?}",
        pane.messages
    );
}

// ---------------------------------------------------------------
// @file mentions (host-fed candidates) + byte-based attachments
// ---------------------------------------------------------------

#[test]
fn typing_at_opens_file_mention_picker_ranked_by_fuzzy_score() {
    let mut pane = NeoismAgentPane::default();
    pane.set_file_mention_candidates(vec![
        "docs/guide.md".to_string(),
        "src/main.rs".to_string(),
        "./src\\models\\map.rs".to_string(),
    ]);

    pane.insert_text("open @ma");

    let picker = pane.picker().expect("file mention picker");
    assert_eq!(picker.kind, NeoismAgentPickerKind::FileMention);
    assert_eq!(pane.file_mention_query().as_deref(), Some("ma"));
    let values: Vec<&str> = picker
        .options()
        .iter()
        .map(|option| option.value.as_str())
        .collect();
    // Substring hits rank first, earlier hit position winning (desktop
    // fuzzy_score policy); the backslash candidate was normalized to
    // forward slashes and its leading "./" stripped.
    assert_eq!(values[0], "src/main.rs");
    assert!(values.contains(&"src/models/map.rs"));
    assert!(
        !values.contains(&"docs/guide.md"),
        "no 'm→a' subsequence exists in docs/guide.md: {values:?}"
    );
    let main = picker
        .options()
        .iter()
        .find(|option| option.value == "src/main.rs")
        .expect("main.rs row");
    assert_eq!(main.title, "@src/main.rs");
    assert_eq!(main.description, "file in src");
}

#[test]
fn candidates_arriving_while_mention_is_open_refresh_the_picker() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("see @gui");
    let picker = pane.picker().expect("file mention picker opens on @");
    assert_eq!(picker.kind, NeoismAgentPickerKind::FileMention);
    assert!(picker.options().is_empty(), "no candidates fed yet");

    pane.set_file_mention_candidates(vec!["docs/guide.md".to_string()]);

    let picker = pane.picker().expect("picker still open");
    assert_eq!(picker.options().len(), 1);
    assert_eq!(picker.options()[0].value, "docs/guide.md");
}

#[test]
fn committing_a_file_mention_inserts_the_token_and_closes_the_picker() {
    let mut pane = NeoismAgentPane::default();
    pane.set_file_mention_candidates(vec!["src/main.rs".to_string()]);
    pane.insert_text("open @main");

    assert!(pane.submit(), "Enter commits the picker, not the prompt");

    assert_eq!(pane.input(), "open @src/main.rs ");
    assert!(pane.picker().is_none());
    assert_eq!(pane.file_mention_query(), None);
}

#[test]
fn attach_file_bytes_tokens_follow_the_shared_attachment_policy() {
    let mut pane = NeoismAgentPane::default();

    assert!(pane.attach_file_bytes("shot.png", "image/png", b"png-bytes"));
    assert!(pane.attach_file_bytes("paper.pdf", "application/pdf", b"pdf-bytes"));
    assert!(pane.attach_file_bytes("notes.txt", "", b"text-bytes"));

    assert_eq!(pane.input(), "[image1] [pdf1] [file1: notes.txt] ");
    let images = pane.input_images();
    assert_eq!(images.len(), 1, "only the png is an image-rail chip");
    assert_eq!(images[0].filename, "shot.png");
    assert!(images[0].url.starts_with("data:image/png;base64,"));
}

#[test]
fn attach_file_bytes_rejects_oversized_and_empty_payloads() {
    let mut pane = NeoismAgentPane::default();
    assert!(!pane.attach_file_bytes("empty.png", "image/png", b""));
    let oversized = vec![0u8; (20 * 1024 * 1024) + 1];
    assert!(!pane.attach_file_bytes("big.png", "image/png", &oversized));
    assert_eq!(pane.input(), "");
    assert!(pane.input_images().is_empty());
    assert!(
        pane.drain_ui_events()
            .iter()
            .any(|event| matches!(event, NeoismAgentUiEvent::Notice { .. })),
        "oversized attach should surface a notice"
    );
}

#[test]
fn attach_clipboard_image_names_unnamed_pastes_like_desktop() {
    let mut pane = NeoismAgentPane::default();
    assert!(pane.attach_clipboard_image("", "image/png", b"bytes"));
    assert!(!pane.attach_clipboard_image("x.txt", "text/plain", b"bytes"));

    assert_eq!(pane.input(), "[image1] ");
    let images = pane.input_images();
    assert_eq!(images[0].filename, "clipboard-image-1.png");
}

#[test]
fn repeated_image_attachments_get_unique_tokens() {
    let mut pane = NeoismAgentPane::default();
    assert!(pane.attach_clipboard_image("", "image/png", b"one"));
    assert!(pane.attach_clipboard_image("", "image/png", b"two"));

    assert_eq!(pane.input(), "[image1] [image2] ");
    assert_eq!(pane.input_images().len(), 2);
}

#[test]
fn submitting_an_attached_image_ships_a_file_part_and_echoes_the_chip() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-1".to_string()));
    let _ = pane.drain_pending_outbound();

    assert!(pane.attach_clipboard_image("shot.png", "image/png", b"png"));
    pane.insert_text("what is this?");
    assert!(pane.submit());

    let drained = pane.drain_pending_outbound();
    match &drained[0] {
        OutboundAgentCommand::SendPrompt { text, parts, .. } => {
            assert_eq!(text, "[image1] what is this?");
            assert_eq!(parts[0]["type"], "text");
            assert_eq!(parts[1]["type"], "file");
            assert_eq!(parts[1]["filename"], "shot.png");
            assert_eq!(parts[1]["mime"], "image/png");
            assert!(parts[1]["url"]
                .as_str()
                .expect("url string")
                .starts_with("data:image/png;base64,"));
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
    // The transcript echo renders the image rail on the sent user card.
    let user = pane
        .messages
        .iter()
        .find(|message| message.kind == NeoismAgentMessageKind::User)
        .expect("user bubble");
    assert_eq!(user.images.len(), 1);
    assert_eq!(user.images[0].filename, "shot.png");
    // Attachments are consumed by the send.
    assert!(pane.input_images().is_empty());
}

// ---------------------------------------------------------------
// Multi-session background cache (desktop `CachedAgentSession` port):
// park on switch, instant restore on return, background streaming
// into caches, snapshot merges, LRU eviction.
// ---------------------------------------------------------------

#[test]
fn session_cache_park_restore_roundtrip_is_instant() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-a".to_string()));
    pane.apply_history(vec![
        NeoismAgentMessage::user("question a").with_id("ua-1"),
        NeoismAgentMessage::assistant("answer a").with_id("aa-1"),
    ]);
    pane.timeline_scroll_px = 123.5;
    pane.timeline_follow_bottom = false;
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    // Switching away PARKS the full conversation state under its id.
    pane.switch_session("sess-b".to_string());
    assert_eq!(pane.session_id.as_deref(), Some("sess-b"));
    assert!(pane.messages.is_empty(), "cold switch shows a fresh pane");
    assert!(
        pane.cached_session_is_hydrated("sess-a"),
        "parked session must be hydrated (instant-switch eligible)"
    );
    {
        let parked = pane.session_cache.get("sess-a").expect("parked entry");
        assert_eq!(parked.messages.len(), 2);
        assert!((parked.timeline_scroll_px - 123.5).abs() < f32::EPSILON);
        assert!(!parked.timeline_follow_bottom);
        assert!(parked.runtime.is_streaming(), "runtime UI parked too");
    }
    assert!(
        !pane.is_streaming(),
        "runtime UI must not leak across parks"
    );

    // Hydrate B so it parks as a distinct conversation.
    pane.apply_history(vec![NeoismAgentMessage::user("question b").with_id("ub-1")]);

    // Switching back restores INSTANTLY — messages/scroll/runtime are
    // present the moment `switch_session` returns, before any
    // HistoryChunk lands.
    pane.switch_session("sess-a".to_string());
    assert_eq!(pane.session_id.as_deref(), Some("sess-a"));
    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].text, "question a");
    assert_eq!(pane.messages[1].text, "answer a");
    assert!((pane.timeline_scroll_px - 123.5).abs() < f32::EPSILON);
    assert!(!pane.timeline_follow_bottom);
    assert!(pane.is_streaming(), "restored runtime keeps streaming UI");
    // …and B is parked in A's place.
    assert!(pane.cached_session_is_hydrated("sess-b"));
    // The instant restore still issues a background SwitchSession so
    // the host re-binds the stream + refreshes history for staleness.
    let drained = pane.drain_pending_outbound();
    let switches = drained
        .iter()
        .filter(|cmd| matches!(cmd, OutboundAgentCommand::SwitchSession { .. }))
        .count();
    assert_eq!(switches, 2, "both switches reach the host: {drained:?}");
}

#[test]
fn switching_to_the_active_session_is_a_noop() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-a".to_string()));
    pane.messages.push(NeoismAgentMessage::user("hi"));
    pane.switch_session("sess-a".to_string());
    assert_eq!(pane.messages.len(), 1, "no clear on same-session switch");
    assert!(
        pane.drain_pending_outbound().is_empty(),
        "no redundant SwitchSession for the active session"
    );
}

#[test]
fn background_events_stream_into_session_cache_not_dropped() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-a".to_string()));

    // Streamed deltas for a NON-active session land in its cache entry.
    pane.cache_apply_part_delta("sess-b", Some("part-1"), Some("text"), "hello");
    pane.cache_apply_part_delta("sess-b", Some("part-1"), Some("text"), " world");
    assert!(pane.messages.is_empty(), "active pane untouched");
    {
        let cached = pane.session_cache.get("sess-b").expect("cache entry");
        assert!(!cached.hydrated, "live-only until a snapshot lands");
        assert_eq!(cached.messages.len(), 1);
        assert_eq!(cached.messages[0].text, "hello world");
        assert!(cached.runtime.is_streaming(), "tail drives cached spinner");
    }

    pane.cache_note_session_idle("sess-b");
    assert!(!pane
        .session_cache
        .get("sess-b")
        .unwrap()
        .runtime
        .is_streaming());

    // Part removal + upsert route to the cache as well.
    pane.cache_upsert_part_message(
        "sess-b",
        NeoismAgentMessage::assistant("tail").with_id("part-2"),
    );
    pane.cache_remove_part_message("sess-b", "part-1");
    let cached = pane.session_cache.get("sess-b").expect("cache entry");
    assert_eq!(cached.messages.len(), 1);
    assert_eq!(cached.messages[0].id, "part-2");
}

#[test]
fn history_chunk_for_cached_session_merges_like_desktop() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-a".to_string()));

    // Live parts streamed into the background cache while the snapshot
    // request was in flight…
    pane.cache_apply_part_delta("sess-b", Some("part-1"), Some("text"), "hello wor");
    {
        let cached = pane.session_cache.get_mut("sess-b").expect("entry");
        // …plus an optimistic user echo that the snapshot has caught
        // up to (reconcile_cached_pending_user_prompts must retire it).
        cached.messages.insert(0, NeoismAgentMessage::user("hi"));
        cached.pending_user_prompts.push("hi".to_string());
    }

    // Snapshot: the same part with an OLDER text prefix — live wins.
    pane.apply_history_to_cache(
        "sess-b",
        vec![
            NeoismAgentMessage::user("hi").with_id("u-1"),
            NeoismAgentMessage::assistant("hello").with_id("part-1"),
        ],
        Some("cursor-1".to_string()),
    );

    let cached = pane.session_cache.get("sess-b").expect("entry");
    assert!(cached.hydrated);
    assert_eq!(
        cached.timeline_history.oldest_loaded_cursor.as_deref(),
        Some("cursor-1")
    );
    assert!(cached.pending_user_prompts.is_empty(), "echo resolved");
    assert_eq!(cached.messages.len(), 2, "no duplicate user bubble");
    assert_eq!(cached.messages[0].id, "u-1");
    assert_eq!(
        cached.messages[1].text, "hello wor",
        "stale snapshot must not truncate streamed text"
    );
}

#[test]
fn returning_from_child_keeps_parent_subagent_notice_in_place() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-child".to_string()));
    for message in [
        NeoismAgentMessage::user("oldest").with_id("u-1"),
        NeoismAgentMessage::assistant("before task").with_id("a-1"),
        NeoismAgentMessage::system(
            "Subagent",
            "Subagent finished.\ntask_id: sess-child\nstatus: completed",
        )
        .with_id("msg_subtask_completion_sess-child"),
        NeoismAgentMessage::user("after task").with_id("u-2"),
        NeoismAgentMessage::assistant("newest").with_id("a-2"),
    ] {
        pane.cache_upsert_part_message("sess-main", message);
    }

    // Reopening the parent fetches only its newest page. The completion
    // notice already streamed into the parent while the child was visible.
    pane.apply_history_to_cache(
        "sess-main",
        vec![
            NeoismAgentMessage::assistant("before task").with_id("a-1"),
            NeoismAgentMessage::user("after task").with_id("u-2"),
            NeoismAgentMessage::assistant("newest").with_id("a-2"),
        ],
        Some("older-cursor".to_string()),
    );
    pane.switch_session("sess-main".to_string());

    let ids = pane
        .messages
        .iter()
        .map(|message| message.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "u-1",
            "a-1",
            "msg_subtask_completion_sess-child",
            "u-2",
            "a-2"
        ]
    );
}

#[test]
fn cold_switch_seeds_from_background_streamed_parts() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-a".to_string()));
    pane.cache_apply_part_delta("sess-b", Some("p1"), Some("text"), "background answer");

    pane.switch_session("sess-b".to_string());
    assert_eq!(pane.session_id.as_deref(), Some("sess-b"));
    assert_eq!(pane.messages.len(), 1, "streamed parts show immediately");
    assert_eq!(pane.messages[0].text, "background answer");
    assert!(
        pane.is_streaming(),
        "cached runtime restored on cold switch"
    );
    assert!(
        !pane.session_cache.contains_key("sess-b"),
        "live-only entry consumed by the switch"
    );
    // The refresh snapshot reconciles rather than truncates.
    pane.apply_history(vec![
        NeoismAgentMessage::assistant("background").with_id("p1")
    ]);
    assert_eq!(pane.messages[0].text, "background answer");
}

#[test]
fn session_cache_eviction_is_lru_bounded_with_pins() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("active".to_string()));
    // Entry for the active session id (bridge/pane id skew scenario) —
    // must be pinned through eviction.
    pane.cache_apply_part_delta("active", Some("p"), Some("text"), "x");
    for index in 0..60 {
        pane.cache_apply_part_delta(&format!("bg-{index}"), Some("p"), Some("text"), "x");
    }
    assert_eq!(
        pane.session_cache.len(),
        40,
        "cache bounded at desktop's MAX_CACHED_SESSIONS"
    );
    assert!(
        pane.session_cache.contains_key("active"),
        "active session pinned"
    );
    assert!(
        pane.session_cache.contains_key("bg-59"),
        "most recent entry survives"
    );
}

#[test]
fn deleted_thread_evicts_its_cache_entry() {
    let mut pane = NeoismAgentPane::default();
    pane.set_session_id(Some("sess-a".to_string()));
    pane.cache_apply_part_delta("sess-b", Some("p"), Some("text"), "x");
    pane.clear_session_id_if("sess-b");
    assert!(!pane.session_cache.contains_key("sess-b"));
    assert_eq!(pane.session_id.as_deref(), Some("sess-a"));
}
