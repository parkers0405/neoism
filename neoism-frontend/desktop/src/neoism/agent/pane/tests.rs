use super::*;

#[test]
fn pending_tool_raw_delta_reconstructs_late_baseline_and_respects_boundary() {
    let snapshot = |raw: &str, status: &str| crate::neoism::agent::api::part_block(&serde_json::json!({
        "id":"tool", "messageID":"m", "sessionID":"s", "type":"tool", "tool":"apply_patch",
        "state":{"status":status, "input":{}, "raw":raw}
    })).unwrap();
    let mut pane = NeoismAgentPane::default();
    pane.upsert_part_message(snapshot("", "pending"));
    let fragment = "\n+🦀\\\"patch";
    for _ in 0..100 {
        pane.apply_part_delta(Some("m".into()), Some("tool".into()), Some("tool-input".into()), fragment);
    }
    assert_eq!(pane.messages[0].detail, fragment.repeat(100));
    let text = pane.messages[0].text.clone();
    let mut late = NeoismAgentPane::default();
    late.upsert_part_message(snapshot(&fragment.repeat(100), "pending"));
    late.apply_part_delta(Some("m".into()), Some("tool".into()), Some("tool-input".into()), fragment);
    assert_eq!(late.messages[0].detail, fragment.repeat(101));
    assert_eq!(late.messages[0].text, text);
    let mut cached = vec![snapshot(&fragment.repeat(100), "pending")];
    apply_cached_part_delta(&mut cached, Some("tool"), Some("tool-input"), fragment);
    assert_eq!(cached[0].detail, fragment.repeat(101));
    pane.upsert_part_message(snapshot("", "running"));
    assert!(pane.messages[0].detail.is_empty());
    pane.apply_part_delta(None, Some("tool".into()), Some("tool-input".into()), "ignored");
    assert!(pane.messages[0].detail.is_empty());
    pane.apply_part_delta(None, Some("missing".into()), Some("tool-input".into()), "ignored");
    assert_eq!(pane.messages.len(), 1);
}

#[test]
fn question_rail_rejects_non_user_targets_without_moving() {
    use neoism_ui::panels::agent_pane::question_rail::TimelineQuestionTarget;

    for source_index in [0, 99] {
        for message_id in ["answer", ""] {
            let mut pane = NeoismAgentPane::default();
            pane.messages = vec![
                NeoismAgentMessage::assistant("answer").with_id(message_id),
                NeoismAgentMessage::user("question").with_id(message_id),
            ];
            pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 5000.0, 200.0);
            pane.set_scrollbar_geometry(Some([380.0, 0.0, 10.0, 100.0]), None);
            pane.set_timeline_question_rail_targets(std::rc::Rc::new(vec![
                TimelineQuestionTarget {
                    source_index,
                    message_id: message_id.into(),
                    top: 1000.0,
                    assistant_source_index: None,
                },
            ]));
            let before = pane.timeline_scroll_offset();
            assert!(!pane.begin_scrollbar_drag(385.0, 50.0));
            assert_eq!(pane.timeline_scroll_offset(), before);
            assert!(pane.timeline_view_anchor().is_none());
            assert!(!pane.scrollbar_dragging());
        }
    }
}

#[test]
fn question_rail_selects_turns_and_anchors_before_lazy_remeasurement() {
    use neoism_ui::panels::agent_pane::question_rail::TimelineQuestionTarget;
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("first").with_id("q1"),
        NeoismAgentMessage::assistant("very long answer").with_id("a1"),
        NeoismAgentMessage::user("second").with_id("q2"),
        NeoismAgentMessage::assistant("short answer").with_id("a2"),
        NeoismAgentMessage::user("third").with_id("q3"),
    ];
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 5000.0, 200.0);
    pane.set_scrollbar_geometry(Some([380.0, 10.0, 10.0, 100.0]), Some([380.0, 10.0, 10.0, 2.0]));
    pane.set_timeline_question_rail_targets(std::rc::Rc::new(vec![
        TimelineQuestionTarget { source_index: 0, message_id: "q1".into(), top: 0.0, assistant_source_index: Some(1) },
        TimelineQuestionTarget { source_index: 2, message_id: "q2".into(), top: 4000.0, assistant_source_index: Some(3) },
        TimelineQuestionTarget { source_index: 4, message_id: "q3".into(), top: 4400.0, assistant_source_index: None },
    ]));
    pane.timeline_velocity_px_s = 800.0;
    pane.timeline_wheel_target_px = Some(300.0);
    assert!(pane.begin_scrollbar_drag(385.0, 60.0));
    // Halfway along the rail means question two, not halfway through answer one.
    assert_eq!(pane.timeline_scroll_offset(), 800.0);
    assert!(!pane.timeline_follow_bottom());
    assert_eq!(pane.timeline_velocity_px_s, 0.0);
    assert_eq!(pane.timeline_wheel_target_px, None);
    assert!(pane.timeline_view_anchor_matches(2, 5));
    assert_eq!(pane.timeline_view_anchor().unwrap().1, 0.0);
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 6000.0, 200.0);
    pane.restore_timeline_view_anchor(4500.0, 0.0);
    assert_eq!(pane.timeline_scroll_offset(), 1300.0);
    assert!(pane.timeline_view_anchor_matches(2, 5));
    // History prepend invalidates indices, but not the stable message identity.
    pane.messages.insert(0, NeoismAgentMessage::user("older").with_id("old"));
    assert!(pane.drag_scrollbar_to(385.0, 60.0));
    assert!(pane.timeline_view_anchor_matches(3, 6));
    assert!(pane.drag_scrollbar_to(385.0, 110.0));
    assert!(pane.timeline_view_anchor_matches(5, 6));
    pane.session_id = Some("another-session".into());
    assert!(pane.scrollbar_hit(385.0, 60.0).is_none());
    assert!(!pane.drag_scrollbar_to(385.0, 60.0));
    pane.set_scrollbar_geometry(None, None);
    assert!(pane.timeline_question_targets.is_none());
    assert!(!pane.scrollbar_dragging());
}

#[test]
fn question_rail_hover_uses_all_questions_and_handles_empty_or_single_turn() {
    use neoism_ui::panels::agent_pane::question_rail::TimelineQuestionTarget;
    let mut pane = NeoismAgentPane::default();
    let targets = (0..25).map(|index| TimelineQuestionTarget {
        source_index: index, message_id: index.to_string(), top: index as f32 * 100.0, assistant_source_index: None,
    }).collect::<Vec<_>>();
    pane.set_scrollbar_geometry(Some([380.0, 0.0, 10.0, 240.0]), Some([380.0, 0.0, 10.0, 2.0]));
    pane.messages = (0..25).map(|index| NeoismAgentMessage::user("question").with_id(index.to_string())).collect();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 5000.0, 200.0);
    pane.set_timeline_question_rail_targets(std::rc::Rc::new(targets.clone()));
    assert!(pane.begin_scrollbar_drag(385.0, 130.0));
    assert!(pane.timeline_view_anchor_matches(13, 25));
    assert!(pane.end_scrollbar_drag());
    assert!(pane.update_scrollbar_hover(Some((385.0, 120.0))));
    assert!(!pane.update_scrollbar_hover(Some((385.0, 121.0))));
    assert!(pane.update_scrollbar_hover(Some((385.0, 130.0))));
    pane.set_scrollbar_geometry(Some([380.0, 1.0, 10.0, 240.0]), Some([380.0, 0.0, 10.0, 2.0]));
    assert!(pane.update_scrollbar_hover(Some((385.0, 130.0))));
    pane.set_timeline_question_rail_targets(std::rc::Rc::new(vec![targets[0].clone()]));
    assert!(pane.begin_scrollbar_drag(385.0, 200.0));
    assert!(pane.timeline_view_anchor_matches(0, 25));
    assert!(pane.end_scrollbar_drag());
    assert!(pane.update_scrollbar_hover(Some((385.0, 100.0))));
    assert!(!pane.update_scrollbar_hover(Some((385.0, 200.0))));
    pane.set_timeline_question_rail_targets(std::rc::Rc::new(vec![]));
    assert!(pane.update_scrollbar_hover(Some((385.0, 100.0))));
    assert!(pane.scrollbar_hit(385.0, 100.0).is_none());
    assert!(!pane.begin_scrollbar_drag(385.0, 100.0));
}

#[test]
fn connected_mcp_done_closes_picker_without_changing_the_draft() {
    use neoism_ui::panels::agent_pane::state::picker::McpAuthorizationDetail;
    let mut pane = NeoismAgentPane::default();
    pane.input = "unsent draft".into();
    pane.cursor_byte = pane.input.len();
    let mut picker = NeoismAgentPicker::new(
        NeoismAgentPickerKind::McpActions,
        "example actions",
        Vec::new(),
        0,
    );
    picker.set_mcp_authorization(McpAuthorizationDetail {
        name: "example".into(),
        body: "Waiting".into(),
        status: "Not connected".into(),
        busy: true,
        url: None,
    });
    assert_eq!(picker.options().last().unwrap().title, "Back");
    pane.picker = Some(picker);
    pane.execute_mcp_action(
        &serde_json::json!({"name":"example", "action":"oauth_done"}),
    );
    assert!(pane.picker.is_some());
    pane.picker
        .as_mut()
        .unwrap()
        .set_mcp_authorization(McpAuthorizationDetail {
            name: "example".into(),
            body: "Connected".into(),
            status: "Connected".into(),
            busy: false,
            url: None,
        });
    let options = pane.picker.as_ref().unwrap().options();
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].title, "Done");
    let action = serde_json::from_str(&options[0].value).unwrap();
    pane.execute_mcp_action(&action);
    assert!(pane.picker.is_none());
    assert_eq!(pane.input, "unsent draft");
    assert_eq!(pane.cursor_byte, pane.input.len());
    assert!(pane.drain_ui_events().is_empty());
}

#[test]
fn mcp_inline_authorization_lifecycle() {
    use neoism_ui::panels::agent_pane::state::picker::McpAuthorizationDetail;
    let mut pane = NeoismAgentPane::default();
    pane.input = "unsent draft".into();
    pane.cursor_byte = pane.input.len();
    let options = neoism_ui::panels::agent_pane::state::mcp_action_options(
        &serde_json::json!({"name":"example", "enabled":true, "status":{"status":"needs_auth"}}),
    );
    pane.picker = Some(NeoismAgentPicker::new(
        NeoismAgentPickerKind::McpActions,
        "example actions",
        options.clone(),
        0,
    ));
    let activity = pane.open_mcp_oauth_activity("example".into(), false);
    assert!(pane.picker.as_ref().unwrap().mcp_authorization.is_some());
    let visible_actions = pane.picker.as_ref().unwrap().options().to_vec();
    pane.insert_text("invisible filter");
    pane.backspace();
    pane.insert_paste("pasted filter");
    pane.set_picker_query("programmatic filter".into());
    assert_eq!(
        pane.picker.as_ref().unwrap().options(),
        visible_actions.as_slice()
    );
    assert!(pane.picker.as_ref().unwrap().query.is_empty());
    assert_eq!(pane.input, "unsent draft");
    assert!(pane.drain_ui_events().is_empty());
    pane.close_picker();
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::McpActions
    );
    assert_eq!(pane.picker.as_ref().unwrap().options(), options.as_slice());
    assert!(pane.picker.as_ref().unwrap().mcp_authorization.is_none());
    assert!(!activity.is_cancelled());
    let detail = McpAuthorizationDetail {
        name: "example".into(),
        body: "waiting".into(),
        status: "Not connected".into(),
        busy: true,
        url: Some("https://Example.com/oauth)?state=a)b#Frag)".into()),
    };
    assert!(pane.apply_mcp_oauth_progress(activity.clone(), detail.clone()));
    assert!(pane.picker.as_ref().unwrap().mcp_authorization.is_none());
    pane.reattach_mcp_oauth("example");
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .mcp_authorization
            .as_ref()
            .unwrap()
            .url,
        detail.url
    );
    pane.execute_mcp_action(
        &serde_json::json!({"name":"example", "action":"oauth_back"}),
    );
    assert!(pane.picker.as_ref().unwrap().mcp_authorization.is_none());
    pane.picker = None;
    pane.apply_mcp_oauth_progress(activity.clone(), detail);
    assert!(pane.picker.is_none());
    assert!(!activity.is_cancelled());
    assert_eq!(pane.input, "unsent draft");
    assert_eq!(pane.cursor_byte, pane.input.len());
    assert!(pane.drain_ui_events().is_empty());
    drop(pane);
    assert!(activity.is_cancelled());
}

#[test]
fn mcp_inline_authorization_ignores_stale_attempts_and_scopes() {
    use neoism_ui::panels::agent_pane::state::picker::McpAuthorizationDetail;
    let mut pane = NeoismAgentPane::default();
    pane.picker = Some(NeoismAgentPicker::new(
        NeoismAgentPickerKind::McpActions,
        "example actions",
        Vec::new(),
        0,
    ));
    let old = pane.open_mcp_oauth_activity("example".into(), false);
    let current = pane.open_mcp_oauth_activity("example".into(), false);
    assert!(old.is_cancelled());
    let detail = McpAuthorizationDetail {
        name: "example".into(),
        body: "stale".into(),
        status: "Not connected".into(),
        busy: false,
        url: None,
    };
    assert!(!pane.apply_mcp_oauth_progress(old, detail.clone()));
    assert_ne!(
        pane.picker
            .as_ref()
            .unwrap()
            .mcp_authorization
            .as_ref()
            .unwrap()
            .body,
        "stale"
    );
    pane.picker = Some(NeoismAgentPicker::new(
        NeoismAgentPickerKind::McpActions,
        "other actions",
        Vec::new(),
        0,
    ));
    assert!(pane.apply_mcp_oauth_progress(current.clone(), detail.clone()));
    assert!(pane.picker.as_ref().unwrap().mcp_authorization.is_none());
    pane.directory = Some("/another/workspace".into());
    assert!(!pane.apply_mcp_oauth_progress(current.clone(), detail.clone()));
    assert!(current.is_cancelled());
    let changed_server = pane.open_mcp_oauth_activity("other".into(), false);
    pane.server = "http://another-host:4096".into();
    pane.cancel_out_of_scope_mcp_oauth();
    assert!(changed_server.is_cancelled());
    assert!(pane.picker.is_none());
    assert!(!pane.apply_mcp_oauth_progress(changed_server, detail));
}

#[test]
fn completed_mcp_authorization_does_not_replace_reopened_management_actions() {
    use neoism_ui::panels::agent_pane::state::picker::McpAuthorizationDetail;
    let mut pane = NeoismAgentPane::default();
    pane.picker = Some(NeoismAgentPicker::new(
        NeoismAgentPickerKind::McpActions,
        "example actions",
        Vec::new(),
        0,
    ));
    let activity = pane.open_mcp_oauth_activity("example".into(), false);
    pane.picker = None;
    assert!(pane.apply_mcp_oauth_progress(
        activity,
        McpAuthorizationDetail {
            name: "example".into(),
            body: "Connected".into(),
            status: "Connected".into(),
            busy: false,
            url: None,
        }
    ));
    pane.picker = Some(NeoismAgentPicker::new(
        NeoismAgentPickerKind::McpActions,
        "example actions",
        Vec::new(),
        0,
    ));
    pane.reattach_mcp_oauth("example");
    assert!(pane.picker.as_ref().unwrap().mcp_authorization.is_none());
    pane.open_mcp_oauth_activity("example".into(), true);
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .mcp_authorization
            .as_ref()
            .unwrap()
            .status,
        "Connecting"
    );
}

#[test]
fn agent_dialog_handoff_dismisses_picker_and_hit_geometry_before_host_draw() {
    for kind in [
        NeoismAgentPickerKind::Slash,
        NeoismAgentPickerKind::Mcp,
        NeoismAgentPickerKind::McpActions,
    ] {
        let mut pane = NeoismAgentPane::default();
        pane.server = "invalid://agent-server".into();
        pane.input = "unsent draft".into();
        pane.cursor_byte = pane.input.len();
        let mut picker = NeoismAgentPicker::new(kind, "vercel actions", Vec::new(), 0);
        picker.last_rect = Some([10.0, 20.0, 400.0, 300.0]);
        pane.picker = Some(picker);
        assert!(pane.picker_card_rect().is_some());
        assert!(pane.picker_contains_point(30.0, 40.0));

        let body = "Opened your browser to authorize this MCP server.\n\nAuthorization URL:\n[https://example.com/oauth](https://example.com/oauth)";
        pane.system_message("vercel", body);

        // The host draws the pane before drawing the queued dialog. No picker
        // material, labels, or stale chrome occlusion may survive that handoff.
        assert!(pane.picker().is_none());
        assert!(pane.picker_card_rect().is_none());
        assert!(!pane.picker_contains_point(30.0, 40.0));
        assert_eq!(pane.input, "unsent draft");
        assert_eq!(pane.cursor_byte, pane.input.len());
        assert!(matches!(
            pane.drain_ui_events().as_slice(),
            [NeoismAgentUiEvent::Dialog { title, body: dialog_body, .. }]
                if title == "vercel" && dialog_body == body
        ));
        assert!(pane.picker().is_none());
    }
}

#[test]
fn linked_dialog_handoff_preserves_unmatched_parentheses_in_structured_url() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "invalid://agent-server".into();
    pane.input = "unsent draft".into();
    pane.cursor_byte = pane.input.len();
    let mut picker = NeoismAgentPicker::new(
        NeoismAgentPickerKind::McpActions,
        "vercel actions",
        Vec::new(),
        0,
    );
    picker.last_rect = Some([10.0, 20.0, 400.0, 300.0]);
    pane.picker = Some(picker);
    let url = "https://Example.com/oauth)?state=a)b&redirect_uri=https%3A%2F%2Fexample.com%2Fcb#Frag)";
    let body = "Opened your browser to authorize this MCP server.";

    // Exercise a generic linked dialog without a request or browser call.
    pane.push_dialog(
        "vercel",
        body,
        Some(("Open authorization page".into(), url.into())),
    );

    assert!(pane.picker().is_none());
    assert!(pane.picker_card_rect().is_none());
    assert!(!pane.picker_contains_point(30.0, 40.0));
    assert_eq!(pane.input, "unsent draft");
    assert_eq!(pane.cursor_byte, pane.input.len());
    assert_eq!(
        pane.drain_ui_events(),
        vec![NeoismAgentUiEvent::Dialog {
            title: "vercel".into(),
            body: body.into(),
            link: Some(("Open authorization page".into(), url.into())),
        }]
    );
}

#[test]
fn agent_dialog_handoff_preserves_picker_for_notices_and_empty_dialogs() {
    let mut pane = NeoismAgentPane::default();
    pane.picker = Some(NeoismAgentPicker::new(
        NeoismAgentPickerKind::McpActions,
        "vercel actions",
        Vec::new(),
        0,
    ));
    pane.push_dialog("vercel", " \n ", None);
    assert!(pane.picker().is_some());
    assert!(pane.drain_ui_events().is_empty());

    pane.system_message("vercel", "MCP server returned no authorization URL");
    assert!(pane.picker().is_some());
    assert!(matches!(
        pane.drain_ui_events().as_slice(),
        [NeoismAgentUiEvent::Notice { .. }]
    ));
}

#[test]
fn provider_readiness_blocks_native_generation_bypasses_before_side_effects() {
    for session_id in [None, Some("existing".to_string())] {
        let mut pane = NeoismAgentPane::default();
        pane.server = "invalid://agent-server".into();
        pane.session_id = session_id.clone();
        seed_prompt_readiness(&mut pane, Default::default());
        pane.input = "keep draft".into();
        pane.cursor_byte = pane.input.len();
        assert!(pane
            .send_prepared_prompt("direct".into(), true, "message".into())
            .is_err());
        pane.queue_send_prompt_command(
            "queued".into(),
            "direct".into(),
            vec![],
            None,
            None,
            "".into(),
            None,
            neoism_protocol::agent::PromptDelivery::Steer,
            true,
        );
        pane.execute_server_command("forwarded".into(), "args".into());
        pane.execute_slash_text("/init");
        pane.execute_slash_text("/goal ship it");
        pane.start_goal_prompt("goal prompt".into());
        pane.fx_pending_prompt = Some("fx prompt".into());
        pane.fire_fx_prompt();
        if session_id.is_none() {
            let error = pane.execute_ensure_session_command().unwrap_err();
            assert!(error.contains("/connect"));
        }
        assert_eq!(pane.session_id, session_id);
        assert_eq!(pane.input, "keep draft");
        assert_eq!(pane.cursor_byte, pane.input.len());
        assert!(pane.messages.is_empty());
        assert!(!pane.is_streaming());
        assert!(pane.pending_prompt_dispatches.is_empty());
        assert!(!pane.prompt_dispatch_in_flight);
        assert_eq!(pane.queued_prompt_count, 0);
        assert!(pane.drain_pending_outbound().is_empty());
        assert!(pane.ui_events.iter().any(|event| matches!(event,
            NeoismAgentUiEvent::Notice { message, .. } if message.contains("/connect"))));
    }
}

#[test]
fn provider_readiness_checks_the_queued_model_and_exempts_current_external_source() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "invalid://agent-server".into();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true;
    pane.queue_send_prompt_command(
        "invalid".into(),
        "hello".into(),
        vec![],
        None,
        None,
        "other/model".into(),
        None,
        neoism_protocol::agent::PromptDelivery::Steer,
        true,
    );
    assert!(pane.pending_prompt_dispatches.is_empty());
    assert!(pane.ui_events.iter().any(|event| matches!(event,
        NeoismAgentUiEvent::Notice { message, .. } if message.contains("/models"))));
    pane.session_id = Some("external".into());
    pane.new_chat_source =
        neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::Codex;
    seed_prompt_readiness(&mut pane, Default::default());
    assert!(!pane.no_providers_connected());
    assert!(pane
        .send_prepared_prompt("external".into(), true, "external-message".into())
        .is_ok());
    pane.queue_send_prompt_command(
        "external-queued".into(),
        "hello".into(),
        vec![],
        None,
        None,
        "".into(),
        None,
        neoism_protocol::agent::PromptDelivery::Steer,
        true,
    );
    assert_eq!(pane.pending_prompt_dispatches.len(), 2);
}

#[test]
fn provider_readiness_rejects_without_consuming_or_navigating() {
    use neoism_ui::panels::agent_pane::api_mapping::ProviderReadiness;
    for (catalog, model, action) in [
        (None, "", "still loading"),
        (
            Some(json!({"all": [], "connected": []})),
            "stale/model",
            "/connect",
        ),
        (
            Some(json!({"all": [], "connected": ["env-provider"]})),
            "",
            "/models",
        ),
        (
            Some(
                json!({"all": [{"id": "env-provider", "models": {"good": {}}}], "connected": ["env-provider"], "default": {"env-provider": "good"}}),
            ),
            "",
            "/models",
        ),
        (
            Some(
                json!({"all": [{"id": "env-provider", "models": {"good": {}}}], "connected": ["env-provider"]}),
            ),
            "other/model",
            "/models",
        ),
    ] {
        for session_id in [None, Some("existing".to_string())] {
            if catalog.is_none() && session_id.is_some() {
                continue;
            }
            let mut pane = NeoismAgentPane::default();
            pane.server = "invalid://agent-server".into();
            pane.session_id = session_id.clone();
            if let Some(readiness) = catalog.as_ref().and_then(ProviderReadiness::from_json) {
                seed_prompt_readiness(&mut pane, readiness);
            }
            pane.model = model.into();
            pane.insert_paste("large pasted prompt\nsecond line");
            let input = pane.input.clone();
            let cursor = pane.cursor_byte;
            let attachments = pane.input_attachments.len();
            assert!(pane.submit());
            assert_eq!(pane.input, input);
            assert_eq!(pane.cursor_byte, cursor);
            assert_eq!(pane.input_attachments.len(), attachments);
            assert_eq!(pane.session_id, session_id);
            assert!(pane.messages.is_empty());
            assert!(!pane.is_streaming());
            assert!(pane
                .drain_pending_outbound()
                .iter()
                .all(|command| !matches!(
                    command,
                    OutboundAgentCommand::EnsureSession
                        | OutboundAgentCommand::SendPrompt { .. }
                )));
            assert!(pane.ui_events.iter().any(|event| matches!(event,
                NeoismAgentUiEvent::Notice { message, .. } if message.contains(action))));
        }
    }
}

#[test]
fn provider_readiness_unknown_existing_chat_submits_on_first_attempt() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "invalid://existing-chat-readiness".into();
    pane.session_id = Some("existing".into());
    pane.model = "saved/model".into();
    pane.prompt_dispatch_in_flight = true;
    pane.insert_text("send without waiting for discovery");
    assert!(pane.submit());
    assert!(pane.input.is_empty());
    assert_eq!(pane.pending_prompt_dispatches.len(), 1);
    assert_eq!(pane.pending_prompt_dispatches[0].origin_session_id.as_deref(), Some("existing"));
    assert!(pane.drain_ui_events().is_empty());
    assert!(pane.send_prepared_prompt("direct".into(), true, "direct-message".into()).is_ok());
    assert_eq!(pane.pending_prompt_dispatches.len(), 2);
}

#[test]
fn provider_readiness_stale_queued_model_rejection_remains_authoritative() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "invalid://queued-model-readiness".into();
    seed_prompt_catalog(&mut pane);
    pane.session_id = Some("existing".into());
    pane.provider_readiness_checked_at = Instant::now().checked_sub(Duration::from_secs(31));
    assert!(pane.current_provider_readiness().is_some());
    assert!(pane.prompt_readiness_error_for_model("test/new-model").unwrap().contains("/models"));
    pane.prompt_dispatch_in_flight = true;
    pane.queue_send_prompt_command(
        "queued".into(), "hello".into(), vec![], None, None,
        "test/new-model".into(), None,
        neoism_protocol::agent::PromptDelivery::Steer, true,
    );
    assert!(pane.pending_prompt_dispatches.is_empty());
    assert!(pane.drain_ui_events().iter().any(|event| matches!(event,
        NeoismAgentUiEvent::Notice { message, .. } if message.contains("/models"))));
}

#[test]
fn provider_readiness_keeps_local_connect_and_external_cli_available() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "invalid://agent-server".into();
    seed_prompt_readiness(&mut pane, Default::default());
    assert!(pane.no_providers_connected());
    pane.input = "/connect ".into();
    assert!(pane.submit());
    assert!(pane.pending_connect.is_some());
    assert!(pane.messages.is_empty());
    pane.close_connect();
    pane.new_chat_source =
        neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::Codex;
    assert!(!pane.no_providers_connected());
    pane.insert_text("external prompt");
    assert!(pane.submit());
    assert!(
        pane.prompt_dispatch_in_flight,
        "submission starts the worker without a frame"
    );
    assert!(pane.drain_pending_outbound().is_empty());
}

fn seed_prompt_readiness(
    pane: &mut NeoismAgentPane,
    readiness: neoism_ui::panels::agent_pane::api_mapping::ProviderReadiness,
) {
    let scope = super::super::api::catalog_scope(&pane.server, pane.directory.as_deref());
    assert!(pane.apply_provider_readiness(scope, readiness));
}

fn seed_prompt_catalog(pane: &mut NeoismAgentPane) {
    pane.model = "test/model".into();
    seed_prompt_readiness(
        pane,
        neoism_ui::panels::agent_pane::api_mapping::ProviderReadiness {
            connected: vec!["test".into()],
            models: vec!["test/model".into()],
        pending: false,
        identity: None,
        },
    );
}

fn composition_fixture() -> NeoismAgentMessage {
    let mut parent = neoism_ui::panels::agent_pane::api_mapping::part_block(&json!({
        "id": "compose-root", "type": "tool", "tool": "compose",
        "state": {"status": "running", "input": {"code": "await tools.read()"}}
    }))
    .unwrap();
    parent.composition_children = ["one", "two"].into_iter().map(|call| {
        let mut child = neoism_ui::panels::agent_pane::api_mapping::part_block(&json!({
            "id": format!("compose-root/compose/{call}"), "type": "tool", "tool": "read",
            "state": {"status": "running", "input": {"filePath": "src/lib.rs"}}
        })).unwrap();
        child.composition_child = true;
        child.text = format!("output {call}");
        child
    }).collect();
    parent.into()
}

#[test]
fn composition_children_map_to_shared_renderer_without_parent_expansion() {
    use neoism_ui::panels::agent_pane::view::tool_message::AgentToolMessage;
    let parent = composition_fixture();
    assert_eq!(parent.tool, "compose");
    assert!(parent.detail.contains("await tools.read()"));
    let children = AgentToolMessage::composition_children(&parent);
    assert!(children.iter().all(|child| child.composition_child));
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].id, "compose-root/compose/one");
    assert_eq!(children[1].text, "output two");
    let mut shared = children[0].clone();
    shared.composition_children = vec![children[1].clone()];
    let mapped = NeoismAgentMessage::from(shared.clone());
    assert_eq!(mapped.composition_children, shared.composition_children);
    let pane = NeoismAgentPane::default();
    assert!(!pane.tool_expanded(&parent.id));
    assert_eq!(AgentToolMessage::composition_children(&parent), children);
    assert!(NeoismAgentMessage::assistant("other")
        .composition_children
        .is_empty());
}

#[test]
fn composition_two_children_toggle_independently_with_owner_archive_and_anchor() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![composition_fixture()];
    pane.timeline_live_trace_start = Some(1);
    pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    let one = "compose-root/compose/one";
    let two = "compose-root/compose/two";
    pane.register_tool_hit_rect(one.into(), [0.0, 150.0, 300.0, 20.0]);
    pane.register_tool_hit_rect(two.into(), [0.0, 180.0, 300.0, 20.0]);
    assert!(pane.tool_archived(one));
    assert_eq!(pane.tool_owner_index(&format!("{two}:0")), Some(0));
    assert!(pane.toggle_tool_at(10.0, 160.0));
    assert!(pane.tool_expanded(one));
    assert!(!pane.tool_expanded(two));
    assert!(!pane.tool_expanded("compose-root"));
    assert!(pane.timeline_view_anchor.is_some());
    assert!(pane.take_timeline_dirty_marks().indices.contains(&0));
    assert!(pane.toggle_tool_at(10.0, 190.0));
    assert!(pane.tool_expanded(one) && pane.tool_expanded(two));
    assert!(pane.toggle_tool_at(10.0, 160.0));
    assert!(!pane.tool_expanded(one) && pane.tool_expanded(two));
    assert!(pane.selected_tool_group_child.is_none());
    pane.register_tool_hit_rect(format!("{two}:0"), [0.0, 210.0, 300.0, 20.0]);
    assert!(pane.toggle_tool_at(10.0, 220.0));
    assert!(pane.tool_expanded(&format!("{two}:0")));
    assert!(pane.tool_expanded(two));
    assert!(pane.tool_archived(&format!("{two}:0")));
    assert!(pane.take_timeline_dirty_marks().indices.contains(&0));
    for animation in pane.tool_expand_anims.values_mut() {
        animation.started_at = Instant::now() - TOOL_EXPAND_ANIMATION;
    }
    assert!(pane.take_timeline_dirty_marks().indices.contains(&0));
}

#[test]
fn composition_child_only_update_invalidates_measurement_and_records_live_motion() {
    let mut pane = NeoismAgentPane::default();
    let parent = composition_fixture();
    pane.upsert_part_message(parent.clone());
    let _ = pane.take_timeline_dirty_marks();
    let before = NeoismAgentPane::timeline_measure_key(&parent, 400.0, 1.0, false, false);
    let mut updated = parent.clone();
    updated.composition_children[0].status = "completed".into();
    updated.composition_children[0]
        .text
        .push_str(" more output");
    let after = NeoismAgentPane::timeline_measure_key(&updated, 400.0, 1.0, false, false);
    assert_ne!(before, after);
    pane.upsert_part_message(updated.clone());
    assert_eq!(
        pane.messages[0].composition_children,
        updated.composition_children
    );
    assert!(pane.take_timeline_dirty_marks().indices.contains(&0));
    assert_eq!(
        pane.tool_motion_sample("compose-root/compose/one")
            .outgoing_status,
        Some("running")
    );
}

#[test]
#[cfg(unix)]
fn conditional_workspace_catalog_scope_is_local_home_or_filesystem_root_only() {
    let home = Path::new("/home/catalog-user");
    let scope = |directory, joined| {
        NeoismAgentPane::catalog_scope_for_workspace(Some(directory), joined, Some(home))
    };
    assert_eq!(scope("/home/catalog-user", false), None);
    assert_eq!(scope("/", false), None);
    assert_eq!(scope("/", true), None);
    assert_eq!(scope("C:\\", true), None);
    assert_eq!(scope("D:/", true), None);
    assert_eq!(scope("C:\\project", true), Some("C:\\project".into()));
    assert_eq!(
        scope("/home/catalog-user/project", false),
        Some("/home/catalog-user/project".into())
    );
    assert_eq!(
        scope("/home/catalog-user", true),
        Some("/home/catalog-user".into())
    );
    assert_eq!(
        scope("/remote/project", true),
        Some("/remote/project".into())
    );
}

#[test]
fn selecting_foreign_chat_keeps_workspace_catalog_scope() {
    let mut pane = NeoismAgentPane::default();
    pane.directory = Some("/workspace/project".into());
    pane.set_catalog_directory(Some("/workspace/project".into()));
    let mut cached = CachedAgentSession::live_only();
    cached.hydrated = true;
    cached.state.directory = Some("/foreign/project".into());
    cached.messages = vec![NeoismAgentMessage::assistant("foreign chat")];
    pane.session_cache.insert("foreign".into(), cached);
    pane.runtime_hydrated_sessions.insert("foreign".into());
    pane.switch_session("foreign".into());
    assert_eq!(pane.session_directory(), Some("/foreign/project"));
    assert_eq!(
        pane.side_panel.catalog_directory(),
        Some("/workspace/project")
    );
}

#[test]
fn catalog_scope_change_discards_old_search_and_resets_stream() {
    let mut pane = NeoismAgentPane::default();
    let old_generation = pane.catalog_generation;
    pane.semantic_in_flight = true;
    pane.semantic_pending_query = Some("stale".into());
    pane.session_catalog_stream =
        Some(AgentSessionCatalogStream::with_events_for_test([]));
    pane.set_catalog_directory(Some("/project".into()));
    assert!(pane.session_catalog_stream.is_none());
    assert!(!pane.semantic_in_flight);
    assert!(pane.semantic_pending_query.is_none());
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::SemanticSessionHits {
            catalog_generation: old_generation,
            query: "stale".into(),
            hits: Some(Vec::new()),
            transcript_unavailable: true,
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(!pane.semantic_unavailable);
    let generation = pane.catalog_generation;
    pane.set_catalog_directory(Some("/project".into()));
    assert_eq!(pane.catalog_generation, generation);
}

#[test]
fn catalog_callbacks_reject_endpoint_scope_and_generation_changes() {
    let mut pane = NeoismAgentPane::default();
    let server = pane.server.clone();
    let generation = pane.catalog_generation;
    assert!(pane.catalog_callback_is_current(&server, generation, None));
    assert!(!pane.catalog_callback_is_current("http://different", generation, None));
    assert!(!pane.catalog_callback_is_current(&server, generation.wrapping_add(1), None));
    pane.set_catalog_directory(Some("/project".into()));
    assert!(!pane.catalog_callback_is_current(&server, generation, None));
    assert!(!pane.catalog_callback_is_current(&server, pane.catalog_generation, None));
    assert!(pane.catalog_callback_is_current(
        &server,
        pane.catalog_generation,
        Some("/project")
    ));

    let request = pane.side_panel.next_session_request_generation();
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::SidePanelSessionsRefreshed {
            server: "http://different".into(),
            catalog_generation: pane.catalog_generation,
            directory: Some("/project".into()),
            generation: request,
            requested_cursor: None,
            result: Ok((
                vec![NeoismAgentSessionEntry::new("stale", "Stale", "")],
                None,
            )),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.side_panel.catalog_session("stale").is_none());
}

#[test]
fn project_callbacks_cannot_populate_a_new_catalog_scope() {
    let mut pane = NeoismAgentPane::default();
    let server = pane.server.clone();
    let old_generation = pane.catalog_generation;
    pane.set_catalog_directory(Some("/new-project".into()));
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::SessionProjectsRefreshed {
            server: server.clone(),
            catalog_generation: old_generation,
            generation: 0,
            result: Ok(crate::neoism::agent::api::SessionProjects {
                projects: vec![("/old-project".into(), 10)],
                pinned_sessions: vec![NeoismAgentSessionEntry::new(
                    "stale-pin",
                    "Pin",
                    "",
                )
                .with_pinned(true)],
            }),
        })
        .unwrap();
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::ProjectSessionPage {
            server,
            catalog_generation: old_generation,
            directory: "/old-project".into(),
            requested_cursor: None,
            generation: 0,
            result: Ok((
                vec![NeoismAgentSessionEntry::new("old", "Old", "")
                    .with_directory(Some("/old-project".into()))],
                None,
            )),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(!pane.side_panel.project_pagination_enabled());
    assert!(pane.side_panel.catalog_session("old").is_none());
    assert!(pane.side_panel.catalog_session("stale-pin").is_none());
    assert_eq!(pane.side_panel.catalog_directory(), Some("/new-project"));
}

#[test]
fn discovery_pins_share_endpoint_and_request_generation_guards() {
    let mut pane = NeoismAgentPane::default();
    let request = pane.side_panel.next_session_request_generation();
    for (server, catalog_generation, generation) in [
        (
            "http://different".to_owned(),
            pane.catalog_generation,
            request,
        ),
        (
            pane.server.clone(),
            pane.catalog_generation.wrapping_add(1),
            request,
        ),
        (
            pane.server.clone(),
            pane.catalog_generation,
            request.wrapping_add(1),
        ),
    ] {
        pane.background_tx
            .send(NeoismAgentBackgroundUpdate::SessionProjectsRefreshed {
                server,
                catalog_generation,
                generation,
                result: Ok(crate::neoism::agent::api::SessionProjects {
                    projects: vec![],
                    pinned_sessions: vec![NeoismAgentSessionEntry::new(
                        "rejected-pin",
                        "Pin",
                        "",
                    )
                    .with_pinned(true)],
                }),
            })
            .unwrap();
    }
    pane.drain_background_updates();
    assert!(pane.side_panel.catalog_session("rejected-pin").is_none());
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::SessionProjectsRefreshed {
            server: pane.server.clone(),
            catalog_generation: pane.catalog_generation,
            generation: request,
            result: Ok(crate::neoism::agent::api::SessionProjects {
                projects: vec![],
                pinned_sessions: vec![NeoismAgentSessionEntry::new(
                    "accepted-pin",
                    "Pin",
                    "",
                )
                .with_directory(Some("/archived".into()))
                .with_pinned(true)],
            }),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.side_panel.catalog_session("accepted-pin").is_some());
}

#[test]
fn scoped_catalog_rejects_foreign_upserts_from_all_scope_stream() {
    let mut pane = NeoismAgentPane::default();
    pane.set_catalog_directory(Some("/project".into()));
    pane.session_catalog_stream = Some(AgentSessionCatalogStream::with_events_for_test(
        [
            serde_json::json!({"id":"foreign-update", "sequence":1, "source":"neoism-agent",
                "schemaVersion":"1.0.0", "timestamp":1,
                "type":"session.updated", "subject":{"kind":"session", "id":"foreign"}, "data":{"info":{
                    "id":"foreign", "title":"Foreign", "directory":"/other", "time":{"updated":1}
            }}}),
            serde_json::json!({"id":"local-update", "sequence":2, "source":"neoism-agent",
                "schemaVersion":"1.0.0", "timestamp":1,
                "type":"session.updated", "subject":{"kind":"session", "id":"local"}, "data":{"info":{
                    "id":"local", "title":"Local", "directory":"/project", "time":{"updated":1}
            }}}),
        ],
    ));
    pane.drain_live_session_updates();
    assert!(pane.side_panel.catalog_session("foreign").is_none());
    assert!(pane.side_panel.catalog_session("local").is_some());
}

#[test]
fn old_project_search_hit_preserves_directory_and_titles_survive_unavailable_transcripts()
{
    use crate::neoism::agent::api::NeoismAgentSemanticSessionHit;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;
    let mut pane = NeoismAgentPane::default();
    pane.directory = Some("/active-project".into());
    pane.side_panel.set_session_query("Older chat".into());
    let entry = NeoismAgentSessionEntry::new("older", "Older chat", "")
        .with_directory(Some("/saved/old-project".into()))
        .with_source(ConversationSource::Codex);
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::SemanticSessionHits {
            catalog_generation: pane.catalog_generation,
            query: "Older chat".into(),
            hits: Some(vec![NeoismAgentSemanticSessionHit {
                session_id: entry.id.clone(),
                title: entry.title.clone(),
                entry,
                excerpt: String::new(),
                distance: 0.0,
            }]),
            transcript_unavailable: true,
        })
        .unwrap();
    assert!(pane.drain_background_updates());
    let row = pane
        .side_panel
        .sessions()
        .iter()
        .find(|row| row.id == "older")
        .unwrap();
    assert_eq!(row.directory.as_deref(), Some("/saved/old-project"));
    assert_eq!(row.source, ConversationSource::Codex);
    assert_eq!(pane.directory.as_deref(), Some("/active-project"));
    assert!(pane.semantic_unavailable);
    // Coalesce without starting an HTTP thread: unavailable vectors must not
    // stop the next server-filtered title query from entering the queue.
    pane.semantic_in_flight = true;
    pane.kick_semantic_session_search("another title".into());
    assert_eq!(
        pane.semantic_pending_query.as_deref(),
        Some("another title")
    );
}

#[test]
fn left_catalog_activity_without_a_chat_stream() {
    use neoism_agent_core::CatalogActivity;
    let mut pane = NeoismAgentPane::default();
    pane.side_panel
        .set_sessions(vec![NeoismAgentSessionEntry::new("root", "Main chat", "")]);
    assert!(pane.event_stream.is_none());
    for (activity, expected) in [
        ("running", CatalogActivity::Running),
        ("background", CatalogActivity::Background),
        ("permission", CatalogActivity::Permission),
        ("idle", CatalogActivity::Idle),
    ] {
        let envelope = serde_json::json!({
            "id": "evt-activity", "sequence": 1, "source": "session",
            "schemaVersion": "1.0.0", "timestamp": 1,
            "type": "session.catalog.activity", "subject": {"kind": "session", "id": "root"},
            "data": {"sessionID": "root", "activity": activity}
        });
        pane.session_catalog_stream =
            Some(AgentSessionCatalogStream::with_events_for_test([envelope]));
        assert!(pane.drain_live_session_updates());
        let row = pane
            .side_panel
            .sessions()
            .iter()
            .find(|row| row.id == "root")
            .unwrap();
        assert_eq!(row.catalog_activity, Some(expected));
        assert!(row.runtime_status.is_none());
        assert!(pane.event_stream.is_none());
        pane.session_catalog_stream = Some(
            AgentSessionCatalogStream::with_events_for_test([
                serde_json::json!({"type":"session.status", "properties":{"sessionID":"root", "status":{"type":"idle"}}}),
            ]),
        );
        assert!(!pane.drain_live_session_updates());
        assert_eq!(
            pane.side_panel
                .sessions()
                .iter()
                .find(|row| row.id == "root")
                .unwrap()
                .catalog_activity,
            Some(expected)
        );
    }
}

#[test]
fn left_catalog_families_survive_last_chat_view_close() {
    use neoism_ui::panels::agent_pane::state::side_panel::BranchStatus;
    let mut catalog = NeoismAgentPane::default();
    catalog.set_catalog_directory(Some("/project".into()));
    catalog.side_panel.set_sessions(vec![
        NeoismAgentSessionEntry::new("root-a", "A", "")
            .with_directory(Some("/project".into())),
        NeoismAgentSessionEntry::new("root-b", "B", "")
            .with_directory(Some("/project".into())),
    ]);
    let family_event = |root: &str, revision: u64, status: &str, permission: bool| {
        serde_json::json!({
            "id": format!("family-{root}-{revision}-{permission}"),
            "sequence": revision, "source": "session", "schemaVersion": "1.0.0",
            "timestamp": 1, "type": "session.catalog.family",
            "subject": {"kind": "session", "id": root},
            "data": {"sessionID": root, "snapshot": {
                "directory": "/project", "executionId": format!("execution-{root}"),
                "familyRevision": revision,
                "branches": [{"sessionId": format!("{root}-child"),
                    "parentSessionId": root, "status": status, "title": "Research",
                    "agent": "explore", "depth": 1, "permissionPending": permission}]
            }}
        })
    };
    let mut chat = NeoismAgentPane::default();
    chat.session_id = Some("root-a".into());
    catalog.session_catalog_stream =
        Some(AgentSessionCatalogStream::with_events_for_test([
            family_event("root-a", 1, "outstanding", false),
            family_event("root-b", 1, "outstanding", false),
        ]));
    assert!(catalog.drain_live_session_updates());
    let branch_status = |catalog: &NeoismAgentPane, id: &str| {
        catalog
            .side_panel
            .sessions()
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| row.branch_status)
    };
    assert_eq!(
        branch_status(&catalog, "root-a-child"),
        Some(BranchStatus::Active)
    );
    assert_eq!(
        branch_status(&catalog, "root-b-child"),
        Some(BranchStatus::Active)
    );

    drop(chat);
    assert!(catalog.event_stream.is_none());
    assert_eq!(
        branch_status(&catalog, "root-a-child"),
        Some(BranchStatus::Active)
    );
    catalog.session_catalog_stream =
        Some(AgentSessionCatalogStream::with_events_for_test([
            family_event("root-a", 1, "outstanding", true),
        ]));
    assert!(catalog.drain_live_session_updates());
    assert_eq!(
        branch_status(&catalog, "root-a-child"),
        Some(BranchStatus::WaitingPermission)
    );

    catalog.session_catalog_stream =
        Some(AgentSessionCatalogStream::with_events_for_test([
            family_event("root-a", 2, "completed", false),
            family_event("root-a", 1, "outstanding", false),
        ]));
    assert!(catalog.drain_live_session_updates());
    assert_eq!(
        branch_status(&catalog, "root-a-child"),
        Some(BranchStatus::Completed)
    );
    assert_eq!(
        branch_status(&catalog, "root-b-child"),
        Some(BranchStatus::Active)
    );
    assert!(catalog.event_stream.is_none());

    catalog.session_catalog_stream =
        Some(AgentSessionCatalogStream::with_events_for_test([
            serde_json::json!({
                "id": "deleted-root-a", "sequence": 3, "source": "session",
                "schemaVersion": "1.0.0", "timestamp": 1, "type": "session.deleted",
                "subject": {"kind": "session", "id": "root-a"},
                "data": {"info": {"id": "root-a"}}
            }),
        ]));
    assert!(catalog.drain_live_session_updates());
    assert_eq!(branch_status(&catalog, "root-a-child"), None);
    assert_eq!(
        branch_status(&catalog, "root-b-child"),
        Some(BranchStatus::Active)
    );
}

#[test]
fn running_tool_animation_is_owned_only_by_the_last_visible_paint() {
    use neoism_ui::panels::agent_pane::view::{
        timeline::AgentTimelinePane, tool_message::AgentToolPane,
    };
    let mut pane = NeoismAgentPane::default();
    pane.messages
        .push(NeoismAgentMessage::assistant("Settled conversation"));
    assert_eq!(pane.animation_reason(), None);
    AgentToolPane::set_visible_running_tool_active(&mut pane, true);
    assert_eq!(pane.animation_reason(), Some("running_tool"));
    AgentTimelinePane::set_visible_running_tool_active(&mut pane, false);
    assert_eq!(pane.animation_reason(), None);
}

#[test]
fn provider_slash_collision_inserts_draft_and_routes_as_prompt() {
    use neoism_ui::panels::agent_pane::state::external_options::ExternalOptions;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;
    let mut pane = NeoismAgentPane::default();
    pane.new_chat_source = ConversationSource::ClaudeCode;
    pane.session_id = Some("root".into());
    pane.external_options = Some(ExternalOptions::parse(&serde_json::json!({
        "provider":"claude","externalSessionId":"provider-1","modeFallback":false,
        "selectedOptions":{},"configOptions":[],"availableCommands":[{"name":"compact","description":"Provider compact","input":{"hint":"args"}}]
    }), "claude").unwrap());
    pane.input = "/comp".into();
    pane.cursor_byte = pane.input.len();
    pane.sync_input_pickers();
    let picker = pane.picker.as_ref().expect("provider slash picker");
    assert_eq!(picker.kind, NeoismAgentPickerKind::Slash);
    assert_eq!(
        picker.selected_option().unwrap().description,
        "Provider compact"
    );
    assert!(pane.commit_picker());
    assert_eq!(pane.input, "/compact ");
    assert!(pane.drain_pending_outbound().is_empty());
    pane.input.push_str("keep this argument");
    pane.prompt_dispatch_in_flight = true;
    assert!(pane.submit());
    assert_eq!(
        pane.pending_prompt_dispatches.front().unwrap().parts[0]["text"],
        "/compact keep this argument"
    );
    assert_eq!(
        super::submit::provider_slash_draft("/comp user args", "compact"),
        "/compact user args"
    );
}

#[test]
fn opencode_compact_uses_verified_acp_action_without_duplicating_provider_command() {
    use neoism_ui::panels::agent_pane::state::external_options::ExternalOptions;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;
    let mut pane = NeoismAgentPane::default();
    pane.new_chat_source = ConversationSource::OpenCode;
    pane.session_id = Some("root".into());
    pane.external_options = Some(ExternalOptions::parse(&serde_json::json!({
        "provider":"opencode","externalSessionId":"provider-1","modeFallback":false,
        "selectedOptions":{},"configOptions":[],"availableCommands":[]
    }), "opencode").unwrap());
    pane.input = "/compact".into();
    pane.sync_slash_picker();
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .selected_option()
            .unwrap()
            .value,
        "__opencode_compact"
    );
    assert!(pane.commit_picker());
    assert_eq!(pane.input, "/compact ");
    pane.prompt_dispatch_in_flight = true;
    assert!(pane.submit());
    assert_eq!(
        pane.pending_prompt_dispatches.front().unwrap().parts[0]["text"],
        "/compact"
    );
    pane.external_options
        .as_mut()
        .unwrap()
        .available_commands
        .push(
            neoism_ui::panels::agent_pane::state::external_options::ExternalCommand {
                name: "compact".into(),
                description: "Provider compact".into(),
                input_hint: None,
            },
        );
    pane.input = "/compact".into();
    pane.sync_slash_picker();
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .selected_option()
            .unwrap()
            .value,
        "compact"
    );
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .options()
            .iter()
            .filter(|row| row.title == "/compact")
            .count(),
        1
    );
}

#[test]
fn provider_empty_slash_catalog_never_shows_neoism_commands() {
    use neoism_ui::panels::agent_pane::state::external_options::ExternalOptions;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;
    let mut pane = NeoismAgentPane::default();
    pane.new_chat_source = ConversationSource::Codex;
    pane.session_id = Some("root".into());
    pane.external_options = Some(
        ExternalOptions::parse(
            &serde_json::json!({
                "provider":"codex","externalSessionId":null,"modeFallback":false,
                "selectedOptions":{},"configOptions":[],"availableCommands":[]
            }),
            "codex",
        )
        .unwrap(),
    );
    pane.input = "/compact args".into();
    pane.sync_input_pickers();
    assert!(pane.picker.is_none());
    pane.prompt_dispatch_in_flight = true;
    assert!(pane.submit());
    assert_eq!(
        pane.pending_prompt_dispatches.front().unwrap().parts[0]["text"],
        "/compact args"
    );
}

#[test]
fn external_model_slash_opens_confirmed_provider_choices() {
    use neoism_ui::panels::agent_pane::state::external_options::ExternalOptions;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;
    let mut pane = NeoismAgentPane::default();
    pane.new_chat_source = ConversationSource::ClaudeCode;
    pane.external_options = Some(ExternalOptions::parse(&serde_json::json!({
        "provider":"claude","externalSessionId":"provider-1","modeFallback":false,
        "selectedOptions":{},"configOptions":[{"id":"provider-model","name":"Model","category":"model","type":"select","currentValue":"a","options":[{"value":"a","name":"A"},{"value":"b","name":"B"}]}],
        "availableCommands":[]
    }), "claude").unwrap());
    pane.input = "/model".into();
    pane.sync_slash_picker();
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .selected_option()
            .unwrap()
            .value,
        "__external_config:provider-model"
    );
    assert!(pane.submit());
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::ExternalOption
    );
    assert_eq!(
        pane.external_picker_option_id.as_deref(),
        Some("provider-model")
    );
    assert_eq!(pane.input, "");
}

#[test]
fn exact_model_selects_provider_config_before_colliding_command() {
    use neoism_ui::panels::agent_pane::state::external_options::ExternalOptions;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;
    let mut pane = NeoismAgentPane::default();
    pane.new_chat_source = ConversationSource::ClaudeCode;
    pane.external_options = Some(ExternalOptions::parse(&serde_json::json!({
        "provider":"claude","externalSessionId":"provider-1","modeFallback":false,
        "selectedOptions":{},"configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"a","options":[{"value":"a","name":"A"}]}],
        "availableCommands":[{"name":"model","description":"Provider model command"},{"name":"skill","description":"Provider skill command"}]
    }), "claude").unwrap());
    pane.input = "/model".into();
    pane.sync_slash_picker();
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .selected_option()
            .unwrap()
            .value,
        "__external_config:model"
    );
    assert!(pane.commit_picker());
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::ExternalOption
    );
    assert_eq!(pane.input, "");
    pane.input = "/model".into();
    pane.sync_slash_picker();
    pane.picker.as_mut().unwrap().move_selection(1);
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .selected_option()
            .unwrap()
            .value,
        "__provider_external_model"
    );
    assert!(pane.commit_picker());
    assert_eq!(pane.input, "/model ");
    pane.input = "/skill".into();
    pane.sync_slash_picker();
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .selected_option()
            .unwrap()
            .value,
        "skill"
    );
}

#[test]
fn local_skill_picker_is_immediate_without_provider_commands() {
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;
    let mut pane = NeoismAgentPane::default();
    pane.new_chat_source = ConversationSource::ClaudeCode;
    pane.input = "/skill".into();
    pane.sync_slash_picker();
    assert_eq!(
        pane.picker
            .as_ref()
            .unwrap()
            .selected_option()
            .unwrap()
            .value,
        "__local_external_skill"
    );
    assert!(pane.commit_picker());
    assert_eq!(
        pane.picker.as_ref().unwrap().kind,
        NeoismAgentPickerKind::Skill
    );
    assert_eq!(pane.input, "");
    let option = NeoismAgentPickerOption::new("review", "Review files", "", "review");
    pane.apply_skill_mention(option);
    assert_eq!(pane.input.trim(), "$review");
    assert!(pane.input_attachments.is_empty());
}

#[test]
fn external_root_usage_reports_known_tokens_with_and_without_limit() {
    let mut pane = NeoismAgentPane::default();
    pane.new_chat_source =
        neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::Codex;
    let mut message = NeoismAgentMessage::assistant("answer");
    message.usage = Some(NeoismAgentUsage {
        input: 4200,
        output: 0,
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        total: 4200,
        cost_micros: 0,
        context_limit: None,
    });
    pane.messages.push(message);
    assert_eq!(pane.context_usage(), Some((4200, None)));
    pane.model_context_limit = Some(128_000);
    assert_eq!(pane.context_usage(), Some((4200, Some(128_000))));
}

#[test]
fn active_preload_rebinds_context_limit_to_hydrated_model() {
    use neoism_ui::panels::agent_pane::api_mapping::session_state_from_json;

    let mut pane = NeoismAgentPane::default();
    pane.server = "http://127.0.0.1:0".to_string();
    pane.session_id = Some("session".into());
    pane.model = "openai/old-model".into();
    pane.model_context_limit = Some(128_000);
    let mut message = NeoismAgentMessage::assistant("answer");
    message.usage = Some(NeoismAgentUsage {
        input: 4200,
        output: 0,
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        total: 4200,
        cost_micros: 0,
        context_limit: None,
    });
    pane.messages.push(message);

    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::SessionMetadataLoaded {
            session_id: "session".into(),
            result: Ok(session_state_from_json(&serde_json::json!({
                "id": "session",
                "model": { "providerId": "openai", "modelId": "gpt-6-astra" }
            }))),
        })
        .unwrap();
    pane.drain_background_updates();

    assert_eq!(pane.model, "openai/gpt-6-astra");
    assert_eq!(pane.context_usage(), Some((4200, None)));

    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::ModelContextLimitRefreshed {
            model: "openai/old-model".into(),
            limit: Some(128_000),
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.context_usage(), Some((4200, None)));

    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::ModelContextLimitRefreshed {
            model: "openai/gpt-6-astra".into(),
            limit: Some(272_000),
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.context_usage(), Some((4200, Some(272_000))));
}

#[test]
fn provider_plan_live_refresh_clear_and_reload_never_revives_old_todowrite() {
    use neoism_ui::panels::agent_pane::api_mapping::session_state_from_json;
    let mut pane = NeoismAgentPane::default();
    let root = "root".to_string();
    pane.session_id = Some(root.clone());
    let old = NeoismAgentMessage::tool(
        "Todos",
        "",
        "completed",
        "todowrite",
        NeoismAgentOutputKind::Todos,
        "",
        vec![NeoismAgentTodo {
            status: "pending".into(),
            content: "Old".into(),
        }],
    );
    let plan = vec![NeoismAgentTodo {
        status: "in_progress".into(),
        content: "Live".into(),
    }];
    pane.plan_todo_events.insert(root.clone(), plan.clone());
    pane.current_plan_todos = Some(plan.clone());
    pane.messages = vec![old.clone()];
    super::ingest::update_todos_message(&mut pane.messages, plan);
    let mut refreshed =
        merge_session_snapshot(vec![old.clone()], pane.messages.clone(), false);
    super::ingest::apply_authoritative_plan(
        &mut refreshed,
        pane.current_plan_todos.as_ref(),
        true,
    );
    assert_eq!(refreshed.last().unwrap().todos[0].content, "Live");

    pane.plan_todo_events.insert(root.clone(), vec![]);
    pane.current_plan_todos = Some(vec![]);
    super::ingest::update_todos_message(&mut pane.messages, vec![]);
    let persisted = session_state_from_json(
        &serde_json::json!({"externalAgent":{"provider":"claude","planTodos":[]}}),
    );
    assert_eq!(persisted.plan_todos, Some(vec![]));
    let mut reloaded = merge_session_snapshot(vec![old], pane.messages.clone(), false);
    super::ingest::apply_authoritative_plan(
        &mut reloaded,
        pane.current_plan_todos.as_ref(),
        true,
    );
    assert_eq!(reloaded.last().unwrap().id, "todos-snapshot");
    assert_eq!(
        reloaded.last().unwrap().kind,
        NeoismAgentMessageKind::System
    );
    assert!(reloaded.last().unwrap().todos.is_empty());
}

#[test]
fn live_baseline_family_hydration_survives_unrelated_text_tokens() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.runtime_status_requests.insert("root".into(), 1);
    pane.note_session_runtime_event("root");
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
            session_id: "root".into(),
            request_generation: 1,
            runtime_revision: 0,
            result: Ok(HashMap::new()),
            runtime: Ok(super::super::api::FamilyRuntimeSnapshot {
                root_session_id: "root".into(),
                family_revision: 2,
                branches: vec![("child".into(), "outstanding".into(), Some(1))],
                ..Default::default()
            }),
            permissions: Ok(Vec::new()),
            questions: Ok(Vec::new()),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.active_subagent_ids.contains("child"));
}

#[test]
fn live_baseline_replaces_cached_text_before_following_deltas() {
    let mut pane = NeoismAgentPane::default();
    pane.messages
        .push(NeoismAgentMessage::assistant("prefix old attempt").with_id("p"));
    pane.upsert_part_message(NeoismAgentMessage::assistant("prefix ").with_id("p"));
    pane.apply_part_delta(None, Some("p".into()), Some("text".into()), "new");
    assert_eq!(pane.messages[0].text, "prefix new");
    // A deliberate retry reset, unlike a late empty normal snapshot, may
    // clear the previous attempt before the retry's deltas arrive.
    pane.retry_reset_pending = true;
    pane.upsert_part_message(NeoismAgentMessage::assistant("").with_id("p"));
    assert!(!pane.retry_reset_pending);
    pane.apply_part_delta(None, Some("p".into()), Some("text".into()), "retry");
    assert_eq!(pane.messages[0].text, "retry");
    let mut cached = vec![NeoismAgentMessage::assistant("old").with_id("p")];
    upsert_cached_part_message(
        &mut cached,
        NeoismAgentMessage::assistant("").with_id("p"),
    );
    apply_cached_part_delta(&mut cached, Some("p"), Some("text"), "fresh");
    assert_eq!(cached[0].text, "fresh");
}
use neoism_ui::panels::agent_pane::state::side_panel::STATUS_LABEL_GRACE;
use std::fs;

#[test]
fn new_draft_idle_hydration_cannot_settle_unconfirmed_primary() {
    for windows_policy in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.status_timing = status_timing::DesktopStatusTiming::for_test(windows_policy);
        pane.note_prompt_admission("first");
        pane.note_streaming(NeoismAgentStreamingState::Generating, None);
        let started = pane.streaming_started_at;
        // Session creation is visible before its first prompt POST is admitted.
        pane.session_id = Some("root".into());
        pane.apply_runtime_status_for_session("root", &HashMap::new());
        assert!(pane.is_streaming());
        assert_eq!(pane.streaming_started_at, started);
        assert!(!pane.terminal_idle_sessions.contains("root"));
        assert_eq!(pane.status_timing.queue_count(1, None), 0);
        assert_eq!(pane.status_timing.queue_count(3, None), 2);
        assert!(!pane.status_timing.dequeue(Some("followup")));
        pane.apply_runtime_status_for_session("root", &HashMap::new());
        assert_eq!(pane.streaming_started_at, started);
        assert!(pane.status_timing.dequeue(Some("first")));
        assert_eq!(pane.status_timing.queue_count(2, None), 2);
        // Once the primary is admitted, real authoritative idle still settles.
        pane.apply_runtime_status_for_session("root", &HashMap::new());
        assert!(!pane.is_streaming());
        assert!(pane.terminal_idle_sessions.contains("root"));
    }
}

#[test]
fn confirmed_admission_allows_idle_recovery_without_dequeue() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.note_prompt_admission("first");
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.status_timing.confirm_admission();
    pane.apply_runtime_status_for_session("root", &HashMap::new());
    assert!(!pane.is_streaming());
    assert!(!pane.status_timing.awaiting_admission());
}

#[test]
fn first_model_clock_does_not_count_admission_wall_time() {
    for windows_policy in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.status_timing = status_timing::DesktopStatusTiming::for_test(windows_policy);
        pane.note_prompt_admission("first");
        pane.note_streaming(NeoismAgentStreamingState::Generating, None);
        pane.streaming_started_at = Some(Instant::now() - Duration::from_secs(5));
        assert_eq!(pane.streaming_label(), "Crafting");
        assert_eq!(pane.streaming_elapsed_seconds(), Some(0.0));
        pane.status_timing.confirm_admission();
        assert_eq!(pane.streaming_elapsed_seconds(), Some(0.0));
        pane.session_id = Some("root".into());
        assert_eq!(pane.streaming_elapsed_seconds(), Some(0.0));
        let activity = neoism_ui::panels::agent_pane::state::ExecutionActivityState {
            execution_id: "execution".into(),
            root_session_id: "root".into(),
            revision: 1,
            ..Default::default()
        };
        assert!(pane.apply_execution_activity(activity.clone()));
        assert_eq!(pane.streaming_elapsed_seconds(), Some(0.0));
        assert!(pane.apply_execution_activity(
            neoism_ui::panels::agent_pane::state::ExecutionActivityState {
                completed_ms: 2_000,
                revision: 2,
                ..activity.clone()
            }
        ));
        assert_eq!(pane.streaming_elapsed_seconds(), Some(2.0));
        assert!(!pane.apply_execution_activity(activity));
        assert_eq!(pane.streaming_elapsed_seconds(), Some(2.0));
    }
}

#[test]
fn windows_admission_split_frames_and_batched_frames_converge() {
    for windows_policy in [false, true] {
        for batched in [false, true] {
            let mut pane = NeoismAgentPane::default();
            pane.session_id = Some("root".into());
            pane.status_timing =
                status_timing::DesktopStatusTiming::for_test(windows_policy);
            pane.status_timing.admit("first");
            pane.messages
                .push(NeoismAgentMessage::user("hello").with_id("first"));
            pane.note_streaming(NeoismAgentStreamingState::Generating, None);
            let started = pane.streaming_started_at;
            let queue = AgentSessionUpdate::QueueStatus {
                count: 1,
                preview: Some("hello".into()),
                started_at: None,
            };
            let dequeue = AgentSessionUpdate::DequeuedPrompt {
                text: "hello".into(),
                message_id: Some("first".into()),
                author: None,
            };
            let updates = if batched {
                vec![queue, dequeue]
            } else {
                vec![queue]
            };
            pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
                "root", updates,
            ));
            pane.drain_live_session_updates();
            assert_eq!(pane.streaming_label(), "Crafting");
            if !batched {
                assert_eq!(pane.queued_prompt_count, 0);
                pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
                    "root",
                    [AgentSessionUpdate::DequeuedPrompt {
                        text: "hello".into(),
                        message_id: Some("first".into()),
                        author: None,
                    }],
                ));
                pane.drain_live_session_updates();
            }
            assert_eq!(pane.queued_prompt_count, 0);
            assert_eq!(pane.messages.len(), 1);
            assert_eq!(pane.streaming_started_at, started);
        }
    }
}

#[test]
fn windows_admission_cache_and_hydration_preserve_real_followup() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.status_timing = status_timing::DesktopStatusTiming::for_test(true);
    pane.status_timing.admit("first");
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    let start = pane.streaming_started_at;
    let mut runtime = pane.take_session_runtime_ui();
    runtime.apply_queue_status(2, Some("next".into()), None);
    assert_eq!(runtime.queued_prompt_count, 1);
    assert!(runtime.status_timing.dequeue(Some("first")));
    runtime.apply_queue_status(1, Some("next".into()), None);
    pane.restore_session_runtime_ui(runtime);
    assert_eq!(pane.queued_prompt_count, 1);
    let statuses = HashMap::from([(
        "root".into(),
        super::super::api::SessionStatusSnapshot {
            kind: "busy".into(),
            queue_count: 1,
            preview: Some("next".into()),
            started_at: Some(100),
            ..Default::default()
        },
    )]);
    pane.apply_runtime_status_for_session("root", &statuses);
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(pane.streaming_started_at, start);
    pane.apply_runtime_status_for_session("root", &statuses);
    assert_eq!(pane.streaming_started_at, start);
    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert_eq!(pane.status_timing.queue_count(1, None), 1);
}

#[test]
fn new_prompt_does_not_borrow_finished_execution_timer() {
    use neoism_ui::panels::agent_pane::state::ExecutionActivityState;
    for windows_policy in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.session_id = Some("root".into());
        pane.status_timing = status_timing::DesktopStatusTiming::for_test(windows_policy);
        let previous = ExecutionActivityState {
            execution_id: "execution-a".into(),
            root_session_id: "root".into(),
            completed_ms: 90_000,
            finished: true,
            revision: 1,
            ..Default::default()
        };
        assert!(pane.apply_execution_activity(previous.clone()));
        assert_eq!(pane.streaming_elapsed_seconds(), Some(90.0));
        let revision = pane.session_runtime_revision("root");
        pane.note_prompt_admission("next");
        pane.note_streaming(NeoismAgentStreamingState::Generating, None);
        assert_eq!(pane.streaming_label(), "Crafting");
        assert_eq!(pane.session_runtime_revision("root"), revision + 1);
        assert_eq!(pane.streaming_elapsed_seconds(), Some(0.0));
        assert!(!pane.apply_execution_activity(previous));
        assert!(pane.apply_execution_activity(ExecutionActivityState {
            execution_id: "execution-b".into(),
            root_session_id: "root".into(),
            revision: 1,
            completed_ms: 2_000,
            ..Default::default()
        }));
        assert_eq!(pane.streaming_elapsed_seconds(), Some(2.0));
    }
}

#[test]
fn new_prompt_keeps_live_family_execution_and_real_queue() {
    for windows_policy in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.session_id = Some("root".into());
        pane.status_timing = status_timing::DesktopStatusTiming::for_test(windows_policy);
        pane.apply_execution_activity(
            neoism_ui::panels::agent_pane::state::ExecutionActivityState {
                execution_id: "execution-a".into(),
                root_session_id: "root".into(),
                completed_ms: 9_000,
                revision: 1,
                ..Default::default()
            },
        );
        pane.note_prompt_admission("next");
        assert_eq!(pane.streaming_elapsed_seconds(), Some(9.0));
        // An authoritative active run means the submitted request really waits,
        // even if the local pane thought it was idle when Enter arrived.
        assert_eq!(pane.status_timing.queue_count(1, Some(100)), 1);
    }
}

#[test]
fn file_mention_options_filter_workspace_trash_dirs() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
    for ignored in [".claude", ".cache", ".neoism", "node_modules", "target"] {
        fs::create_dir_all(root.path().join(ignored)).unwrap();
        fs::write(root.path().join(ignored).join("main.rs"), "trash").unwrap();
    }

    let search = neoism_agent_workspace_search_fff::FffWorkspaceSearchService::new();
    let pin = Mutex::new(None);
    let options = file_mention_options(&search, &pin, root.path(), "main", 20);
    let values = options
        .into_iter()
        .map(|option| option.value)
        .collect::<Vec<_>>();

    assert_eq!(values, vec!["src/main.rs"]);
}

#[test]
fn retry_reset_wipes_partial_text_so_restreams_do_not_double() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_part_delta(
        Some("msg-a".into()),
        Some("part-t".into()),
        Some("text".into()),
        "partial reply ",
    );
    // Provider overload → server announces the retry, then re-seeds the
    // SAME text part with empty text before re-streaming.
    pane.retry_reset_pending = true;
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("").with_id("part-t".to_string()),
    );
    pane.apply_part_delta(
        Some("msg-a".into()),
        Some("part-t".into()),
        Some("text".into()),
        "fresh reply",
    );
    let row = pane
        .messages
        .iter()
        .find(|message| message.id == "part-t")
        .expect("text row survives the retry wipe");
    assert_eq!(row.text, "fresh reply");
    assert!(!pane.retry_reset_pending, "wipe consumes the pending flag");
}

#[test]
fn generated_image_hydration_updates_the_existing_assistant_card() {
    let mut pane = NeoismAgentPane::default();
    let mut message = NeoismAgentMessage::assistant("").with_id("part-image");
    message
        .images
        .push(neoism_ui::panels::agent_pane::state::NeoismAgentImage {
            filename: "generated-image.jpg".to_string(),
            url: "/v2/artifacts/art-image/content".into(),
            mime: "image/jpeg".to_string(),
        });
    pane.upsert_part_message(message.clone());
    pane.retry_reset_pending = true;

    message.images[0].url = "data:image/jpeg;base64,aW1hZ2U=".into();
    pane.upsert_part_message(message.clone());

    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].images, message.images);
    assert!(
        pane.retry_reset_pending,
        "images must not consume a text retry reset"
    );
}

#[test]
fn late_empty_snapshot_without_retry_never_regresses_streamed_text() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_part_delta(
        Some("msg-a".into()),
        Some("part-t".into()),
        Some("text".into()),
        "streamed text",
    );
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("").with_id("part-t".to_string()),
    );
    let row = pane
        .messages
        .iter()
        .find(|message| message.id == "part-t")
        .expect("text row still present");
    assert_eq!(row.text, "streamed text");
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
    // A transient idle reading holds the displayed label; sustained
    // idle past the grace window clears it for real.
    assert_eq!(pane.streaming_label(), "Crafting");
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_label(), "");
    assert_eq!(pane.streaming_elapsed_seconds(), None);
    assert_eq!(pane.timeline_live_trace_start, Some(1));

    pane.reset_session_runtime_ui();
    assert_eq!(pane.timeline_live_trace_start, None);
}

#[test]
fn viewed_subagent_terminal_status_clears_and_latches_activity() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child-1".to_string());
    pane.parent_session_id = Some("parent".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);

    assert!(pane.reconcile_viewed_subagent_runtime("child-1", BranchStatus::Completed));
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.child_part_can_drive_streaming("child-1"));
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
fn completed_child_ignores_late_permission_reply_event() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.enqueue_pending_permission(child_permission("perm-1"));
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::PermissionReplied {
            request_id: "perm-1".to_string(),
            session_id: Some("child-1".to_string()),
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Completed)
    );
    assert!(pane.side_panel.branch_terminal_locked("child-1"));
}

#[test]
fn completed_child_ignores_stale_permission_request_event() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::PermissionAsked(child_permission(
            "perm-late",
        ))],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Completed)
    );
    assert!(pane.side_panel.branch_terminal_locked("child-1"));
}

#[test]
fn child_metadata_update_never_changes_or_recreates_native_lifecycle() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.upsert_live_subagent_entry(
        "child-1",
        Some("Old title".to_string()),
        Some("explore".to_string()),
    );
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, Some(1));
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::SubagentMetadata {
            session_id: "child-1".to_string(),
            title: Some("Renamed child".to_string()),
            agent: Some("build".to_string()),
        }],
    ));

    pane.drain_server_updates();

    let child = pane
        .side_panel
        .subagents()
        .iter()
        .find(|entry| entry.id == "child-1")
        .expect("tracked child");
    assert_eq!(child.title, "Renamed child");
    assert_eq!(child.time_label, "build");
    assert_eq!(
        pane.side_panel
            .branch_activity("child-1")
            .map(|activity| activity.status),
        Some(BranchStatus::Active)
    );

    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        2,
        [("child-1".to_string(), "completed".to_string(), Some(1))],
    ));
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::SubagentMetadata {
            session_id: "child-1".to_string(),
            title: Some("Late rename".to_string()),
            agent: None,
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(pane.active_subagent_count(), 0);
    assert!(pane
        .side_panel
        .subagents()
        .iter()
        .all(|entry| entry.id != "child-1"));
}

#[test]
fn stale_busy_child_event_cannot_reopen_completed_branch_but_newer_runtime_can() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        8,
        [("child-1".to_string(), "completed".to_string(), None)],
    ));
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::SubagentStatus {
            session_id: "child-1".to_string(),
            status: "busy".to_string(),
            started_at: Some(2),
            title: None,
            agent: None,
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Completed)
    );
    assert!(pane.side_panel.branch_terminal_locked("child-1"));
    assert!(!pane.active_subagent_ids.contains("child-1"));
    pane.side_panel.prune_expired_completed_subagents();
    assert!(pane
        .side_panel
        .subagents()
        .iter()
        .all(|entry| entry.id != "child-1"));

    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        9,
        [("child-1".to_string(), "outstanding".to_string(), Some(3))],
    ));
    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Active)
    );
    assert!(!pane.side_panel.branch_terminal_locked("child-1"));
    assert!(pane.active_subagent_ids.contains("child-1"));
}

#[test]
fn terminal_revision_rejects_delayed_active_snapshot_then_allows_continuation() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    assert!(pane.apply_runtime_lifecycle_snapshot(
        Some(
            neoism_ui::panels::agent_pane::state::ExecutionActivityState {
                execution_id: "execution-a".to_string(),
                root_session_id: "parent".to_string(),
                revision: 1,
                ..Default::default()
            }
        ),
        "parent".to_string(),
        8,
        [("child-1".to_string(), "outstanding".to_string(), Some(1))],
    ));
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    assert!(pane.note_subagent_terminal_revision(
        "child-1",
        Some("parent"),
        Some("execution-a"),
        Some(10),
    ));

    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        9,
        [("child-1".to_string(), "outstanding".to_string(), Some(1))],
    ));
    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Completed)
    );
    assert!(!pane.active_subagent_ids.contains("child-1"));

    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        11,
        [("child-1".to_string(), "outstanding".to_string(), Some(2))],
    ));
    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Active)
    );
    assert!(!pane.note_subagent_terminal_revision(
        "child-1",
        Some("parent"),
        Some("execution-a"),
        Some(10),
    ));
}

#[test]
fn terminal_runtime_snapshot_settles_parent_task_sidebar_and_footer() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    let mut task = NeoismAgentMessage::tool(
        "Task(inspect project)",
        "task_id: child-1\nstatus: queued",
        "queued",
        "task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    task.detail = task.text.clone();
    pane.messages.push(task);
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, Some(1));

    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        2,
        [("child-1".to_string(), "completed".to_string(), Some(1))],
    ));

    assert_eq!(pane.messages[0].status, "completed");
    assert!(pane.messages[0].text.contains("status: completed"));
    assert_eq!(pane.active_subagent_count(), 0);
    assert!(!pane.has_status_activity());
    assert!(pane
        .side_panel
        .subagents()
        .iter()
        .all(|entry| entry.id != "child-1"));
}

#[test]
fn equal_revision_empty_snapshot_clears_locally_resurrected_children() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        8,
        std::iter::empty::<(String, String, Option<u64>)>(),
    ));

    for child in ["child-1", "child-2"] {
        pane.upsert_live_subagent_entry(
            child,
            Some(child.to_string()),
            Some("explore".into()),
        );
        pane.note_subagent_runtime(child.to_string(), BranchStatus::Active, Some(1));
    }
    assert_eq!(pane.active_subagent_count(), 2);

    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        8,
        std::iter::empty::<(String, String, Option<u64>)>(),
    ));
    assert_eq!(pane.active_subagent_count(), 0);
    assert!(!pane.has_status_activity());
    assert!(pane
        .side_panel
        .subagents()
        .iter()
        .all(|entry| !entry.id.starts_with("child-")));
}

#[test]
fn terminal_runtime_snapshot_settles_viewed_child_streaming_label() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child-1".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Working, None);
    assert!(pane.is_streaming());

    assert!(pane.apply_branch_lifecycle_snapshot(
        "parent".to_string(),
        2,
        [("child-1".to_string(), "completed".to_string(), Some(1))],
    ));

    assert!(!pane.is_streaming());
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Idle);
}

#[test]
fn stale_runtime_response_cannot_resurrect_finished_desktop_child() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    assert!(pane.apply_runtime_lifecycle_snapshot(
        Some(
            neoism_ui::panels::agent_pane::state::ExecutionActivityState {
                execution_id: "execution-a".to_string(),
                root_session_id: "root".to_string(),
                revision: 2,
                finished: true,
                ..Default::default()
            }
        ),
        "root".to_string(),
        2,
        [("child-1".to_string(), "completed".to_string(), Some(1))],
    ));
    assert!(!pane.apply_runtime_lifecycle_snapshot(
        Some(
            neoism_ui::panels::agent_pane::state::ExecutionActivityState {
                execution_id: "execution-a".to_string(),
                root_session_id: "root".to_string(),
                revision: 1,
                finished: false,
                ..Default::default()
            }
        ),
        "root".to_string(),
        3,
        [("child-1".to_string(), "outstanding".to_string(), Some(1))],
    ));
    assert_eq!(pane.active_subagent_count(), 0);
    assert!(!pane.has_status_activity());
    assert!(pane
        .side_panel
        .subagents()
        .iter()
        .all(|entry| entry.id != "child-1"));
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
    pane.note_streaming(NeoismAgentStreamingState::Working, None);
    assert_eq!(pane.streaming_label(), "Sub-agents working");

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

#[test]
fn failed_subagent_refresh_preserves_sidebar_and_footer_activity() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    assert_eq!(pane.active_subagent_count(), 1);

    pane.side_panel.mark_subagent_tree_dirty();
    let generation = pane
        .side_panel
        .begin_subagent_refresh()
        .expect("refresh generation");
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed {
            session_id: "parent".to_string(),
            generation,
            result: Err("temporary transport failure".to_string()),
        })
        .unwrap();

    pane.drain_background_updates();

    assert_eq!(
        pane.side_panel
            .subagents()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["parent", "child"]
    );
    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert!(
        !pane.side_panel.should_refresh_subagents(),
        "a failed recovery snapshot must wait for the next lifecycle/reconnect edge"
    );
}

#[test]
fn catalog_picker_skeletons_keep_drafts_and_finish_on_success_or_error() {
    for kind in [
        NeoismAgentPickerKind::Model,
        NeoismAgentPickerKind::Agent,
        NeoismAgentPickerKind::Session,
    ] {
        for failed in [false, true] {
            let mut pane = NeoismAgentPane::default();
            pane.input = "unsent draft".into();
            pane.picker = Some(NeoismAgentPicker::new_loading(kind, "Catalog"));
            pane.picker.as_mut().unwrap().set_query("match".into());
            assert!(pane.commit_picker());
            assert_eq!(pane.input, "unsent draft");
            let picker = pane.picker.as_ref().unwrap();
            assert!(picker.loading);
            assert!(picker.options().is_empty());
            assert!(picker.selected_option().is_none());

            let result = if failed {
                Err("match failure".to_string())
            } else {
                Ok(vec![NeoismAgentPickerOption::new("match", "", "", "match")])
            };
            let update = match kind {
                NeoismAgentPickerKind::Model => {
                    NeoismAgentBackgroundUpdate::ModelOptionsRefreshed {
                        scope: super::super::api::catalog_scope(&pane.server, pane.directory.as_deref()),
                        result,
                    }
                }
                NeoismAgentPickerKind::Agent => {
                    NeoismAgentBackgroundUpdate::AgentOptionsRefreshed {
                        scope: super::super::api::catalog_scope(&pane.server, pane.directory.as_deref()),
                        result,
                    }
                }
                NeoismAgentPickerKind::Session => {
                    NeoismAgentBackgroundUpdate::SessionOptionsRefreshed {
                        catalog_generation: pane.catalog_generation,
                        result,
                    }
                }
                _ => unreachable!(),
            };
            pane.background_sender().send(update).unwrap();
            assert!(pane.drain_background_updates());
            let picker = pane.picker.as_ref().unwrap();
            assert!(!picker.loading);
            assert_eq!(picker.query, "match");
            assert!(!picker.options().is_empty());
            assert_eq!(pane.input, "unsent draft");
            assert!(pane.messages.is_empty());
            assert!(pane.session_id.is_none());
        }
    }
}

#[test]
fn thinking_picker_is_ready_immediately_with_current_reasoning_selected() {
    let mut pane = NeoismAgentPane::default();
    pane.model = "openai/gpt-5.6".into();
    pane.thinking = Some("ultra".into());
    pane.open_thinking_picker();
    let picker = pane.picker.as_ref().unwrap();
    assert_eq!(picker.kind, NeoismAgentPickerKind::Thinking);
    assert!(!picker.loading);
    assert_eq!(picker.selected_option().unwrap().value, "ultra");
}

#[test]
fn background_results_wake_the_window_and_replace_restored_placeholder_titles() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel
        .ensure_subagent_main_entry("parent".to_string());
    pane.side_panel
        .upsert_subagent("child", "subagent", "subagent");

    let wake_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let wake_count_for_callback = wake_count.clone();
    pane.set_event_wake(AgentEventWake::for_test(move || {
        wake_count_for_callback.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));

    pane.side_panel.mark_subagent_tree_dirty();
    let generation = pane
        .side_panel
        .begin_subagent_refresh()
        .expect("refresh generation");
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed {
            session_id: "parent".to_string(),
            generation,
            result: Ok(vec![
                NeoismAgentSessionEntry::new("parent", "main session", "return"),
                NeoismAgentSessionEntry::new(
                    "child",
                    "Inspect session catalogue (@explore subagent)",
                    "explore",
                )
                .with_runtime_status(Some("running".to_string())),
            ]),
        })
        .unwrap();

    assert_eq!(wake_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(pane.drain_background_updates());
    assert_eq!(
        pane.side_panel
            .subagents()
            .iter()
            .find(|entry| entry.id == "child")
            .map(|entry| entry.title.as_str()),
        Some("Inspect session catalogue (@explore subagent)")
    );

    // Draining releases the coalesced wake, so another independent result
    // schedules another frame rather than getting stranded.
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::AgentOptionsRefreshed {
            scope: super::super::api::catalog_scope(&pane.server, pane.directory.as_deref()),
            result: Ok(Vec::new()),
        })
        .unwrap();
    assert_eq!(wake_count.load(std::sync::atomic::Ordering::Relaxed), 2);
}

#[test]
fn event_stream_reconnect_requests_exactly_one_recovery_snapshot() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    assert!(!pane.side_panel.should_refresh_subagents());

    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::EventStreamReconnected],
    ));
    pane.drain_server_updates();

    assert!(pane.side_panel.should_refresh_subagents());
    let generation = pane
        .side_panel
        .begin_subagent_refresh()
        .expect("one reconnect snapshot");
    assert!(pane.side_panel.complete_subagent_refresh(generation));
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    assert!(!pane.side_panel.should_refresh_subagents());
}

#[test]
fn live_subagent_activity_invalidates_older_terminal_snapshot() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.messages = vec![task_tool_message("child", "running")];
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Active, Some(1));
    pane.side_panel.mark_subagent_tree_dirty();
    let stale_generation = pane
        .side_panel
        .begin_subagent_refresh()
        .expect("start stale refresh");

    // A newer live delta arrives while the old snapshot is still in flight.
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::SubagentActivity {
            session_id: "child".to_string(),
            status: "active".to_string(),
            current_tool: Some("read".to_string()),
            started_at: Some(2),
        }],
    ));
    pane.drain_server_updates();

    // The older split snapshot falsely says the same child completed. Its
    // invalidated generation must be ignored atomically, without one frame
    // of a completed task card or missing footer/sidebar row.
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed {
            session_id: "parent".to_string(),
            generation: stale_generation,
            result: Ok(vec![
                NeoismAgentSessionEntry::new("parent", "main session", "return"),
                NeoismAgentSessionEntry::new("child", "child", "explore")
                    .with_runtime_status(Some("completed".to_string())),
            ]),
        })
        .unwrap();
    pane.drain_background_updates();
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(pane.messages[0].status, "running");
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert!(pane
        .side_panel
        .subagents()
        .iter()
        .any(|entry| entry.id == "child"));
}

#[test]
fn stale_running_snapshot_cannot_resurrect_task_or_footer_after_completion() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.messages = vec![task_tool_message("child", "running")];
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Active, Some(1));
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Completed, None);

    // A recovery request that began before completion returns stale running.
    // Side-panel reconciliation keeps the terminal lock, and task/runtime
    // reconciliation must consume that effective state rather than raw data.
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.reconcile_task_message_statuses();
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 0);
    assert_eq!(pane.messages[0].status, "completed");
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(pane.side_panel.branch_terminal_locked("child"));
}

#[test]
fn child_background_completion_stays_out_of_main_transcript() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::BackgroundTaskCompleted {
            session_id: "child".to_string(),
            job_id: "job-child".to_string(),
            status: "completed".to_string(),
        }],
    ));

    pane.drain_server_updates();

    assert!(pane
        .messages
        .iter()
        .all(|message| message.id != "background-task-job-child"));
    assert!(pane.session_cache["child"]
        .messages
        .iter()
        .any(|message| message.id == "background-task-job-child"));
}

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
fn background_completion_card_survives_messages_snapshot_replacement() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [
            // The live completion event injects the card…
            AgentSessionUpdate::BackgroundTaskCompleted {
                session_id: "parent".to_string(),
                job_id: "job-1".to_string(),
                status: "completed".to_string(),
            },
            // …then a full-transcript refresh lands BEFORE the server's
            // queued completion prompt drained — no trace of the card in
            // the snapshot. It must survive the replacement.
            AgentSessionUpdate::Messages {
                messages: vec![
                    NeoismAgentMessage::user("kick off the build").with_id("u-1"),
                    NeoismAgentMessage::assistant("Started.").with_id("a-1"),
                ],
                oldest_cursor: None,
            },
        ],
    ));

    pane.drain_server_updates();

    assert_eq!(pane.messages.len(), 3);
    assert_eq!(pane.messages[2].id, "background-task-job-1");
}

#[test]
fn background_completion_never_relocates_streaming_content_or_durable_turns() {
    let mut pane = NeoismAgentPane::default();
    let launch = NeoismAgentMessage::tool(
        "Run timer",
        "job_id: job-1\nstatus: running",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    )
    .with_id("tool-launch");
    pane.remember_live_part_parent("tool-launch", Some("msg-active"));
    pane.upsert_part_message(launch.clone());
    pane.remember_live_part_parent("answer", Some("msg-active"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("Streaming before finish ").with_id("answer"),
    );
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    pane.upsert_part_message(client_background_completion_card("job-1"));
    pane.apply_part_delta(
        Some("msg-active".to_string()),
        Some("answer".to_string()),
        Some("text".to_string()),
        "and after finish",
    );

    assert_eq!(
        pane.messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["tool-launch", "answer", "background-task-job-1"]
    );
    assert!(pane.messages[1].text.ends_with("and after finish"));

    // The completion event has no transcript anchor. Once the persisted
    // runtime prompt arrives, its durable server position wins — including
    // a user turn that did not exist when the live event was received.
    let mut server_copy = client_background_completion_card("job-1");
    server_copy.detail = "Background shell task finished.\njob_id: job-1\nstatus: completed\n<background_task_result>ok</background_task_result>".to_string();
    pane.messages = pane.preserve_background_completion_cards(vec![
        launch,
        NeoismAgentMessage::assistant("Streaming before finish and after finish")
            .with_id("answer"),
        NeoismAgentMessage::user("newer turn").with_id("user-newer"),
        server_copy,
    ]);

    assert_eq!(
        pane.messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "tool-launch",
            "answer",
            "user-newer",
            "background-task-job-1"
        ]
    );
    assert!(pane.messages[3].detail.contains("<background_task_result>"));
}

#[test]
fn server_background_completion_copy_replaces_client_card_without_duplicate() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    // Persisted runtime prompt as the shared mapping regenerates it —
    // the SAME durable id the live event used.
    let server_copy = neoism_ui::panels::agent_pane::api_mapping::message_blocks(&json!({
        "info": {
            "id": "msg_background_completion_job-1",
            "role": "user",
            "sessionId": "ses_test",
            "time": { "created": 0 },
            "agent": "build",
            "model": { "providerId": "openai", "modelId": "gpt-5.6-sol" }
        },
        "parts": [{
            "id": "prt-background-done",
            "type": "text",
            "text": "Background shell task finished.\njob_id: job-1\nstatus: completed"
        }]
    }))
    .into_iter()
    .map(NeoismAgentMessage::from)
    .next()
    .expect("mapped completion card");
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [
            AgentSessionUpdate::BackgroundTaskCompleted {
                session_id: "parent".to_string(),
                job_id: "job-1".to_string(),
                status: "completed".to_string(),
            },
            AgentSessionUpdate::Messages {
                messages: vec![
                    NeoismAgentMessage::user("kick off the build").with_id("u-1"),
                    server_copy,
                ],
                oldest_cursor: None,
            },
        ],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| message.id == "background-task-job-1")
            .count(),
        1,
        "server copy replaces the client copy, no duplicate"
    );
}

#[test]
fn background_completion_runtime_ids_never_reorder_visible_replies() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("thanks").with_id("msg_010000000002human"),
        NeoismAgentMessage::assistant("Both timers are running.").with_id("prt-running"),
    ];
    pane.remember_live_part_parent("prt-running", Some("msg_047100000000assistant"));

    // The live completion event has no durable transcript position. The
    // 15-second answer can begin before the persisted runtime-prompt part
    // catches up and gives the hidden sentinel its synthetic parent id.
    pane.upsert_part_message(client_background_completion_card("job-15"));
    pane.apply_part_delta(
        Some("msg_047200000000assistant".into()),
        Some("prt-answer-15".into()),
        Some("text".into()),
        "15-second timer finished; the 30-second timer is still running.",
    );
    pane.remember_live_part_parent(
        "background-task-job-15",
        Some("msg_background_completion_job-15"),
    );

    // The next completion is already mapped when its answer starts. A
    // lexicographic sort must not use `msg_background_completion_*` as an
    // ordering anchor: it is a reserved runtime id, not an ascending id.
    pane.upsert_part_message(client_background_completion_card("job-30"));
    pane.remember_live_part_parent(
        "background-task-job-30",
        Some("msg_background_completion_job-30"),
    );
    pane.apply_part_delta(
        Some("msg_047300000000assistant".into()),
        Some("prt-answer-30".into()),
        Some("text".into()),
        "30-second timer finished.",
    );

    // A fast follow-up is still optimistic (empty id) when its answer starts.
    // Later assistant output must remain below that human turn.
    pane.messages.push(NeoismAgentMessage::user("NICE"));
    pane.apply_part_delta(
        Some("msg_047400000000assistant".into()),
        Some("prt-answer-nice".into()),
        Some("text".into()),
        "Both completed on schedule.",
    );

    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| {
                matches!(
                    message.kind,
                    NeoismAgentMessageKind::User | NeoismAgentMessageKind::Assistant
                )
            })
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        vec![
            "thanks",
            "Both timers are running.",
            "15-second timer finished; the 30-second timer is still running.",
            "30-second timer finished.",
            "NICE",
            "Both completed on schedule.",
        ]
    );
}

#[test]
fn retry_status_includes_a_compact_provider_reason() {
    let mut pane = NeoismAgentPane::default();
    pane.note_streaming(
        NeoismAgentStreamingState::Retrying,
        Some("Our servers are currently overloaded. Please try again later.".to_string()),
    );

    assert_eq!(pane.streaming_label(), "Retrying · Provider overloaded");
}

#[test]
fn wordmark_startup_defaults_off_without_disabling_logo_clicks() {
    use neoism_ui::panels::agent_pane::view::wordmark::WordmarkState;

    let mut pane = NeoismAgentPane::default();
    pane.wordmark.play_startup_animation();
    assert!(!pane.wordmark.animation.is_active());
    pane.set_splash_auto_animate(true);
    pane.wordmark.play_startup_animation();
    assert!(!pane.wordmark.animation.is_active());
    pane.set_splash_auto_animate(false);
    pane.wordmark.rect = Some([0.0, 0.0, 300.0, 50.0]);
    assert!(pane.pop_wordmark_click(100.0, 25.0));
    assert!(pane.wordmark.animation.is_active());
}

#[test]
fn wordmark_startup_is_once_per_agent_tab() {
    use neoism_ui::panels::agent_pane::view::wordmark::WordmarkState;

    let mut pane = NeoismAgentPane::default();
    pane.set_splash_auto_animate(true);
    assert!(pane.wordmark.startup_pending);
    assert!(!pane.wordmark.animation.is_active());
    pane.wordmark.play_startup_animation();
    assert!(!pane.wordmark.startup_pending);
    assert_eq!(pane.animation_reason(), Some("wordmark"));
    pane.wordmark.animation.cancel();
    pane.wordmark.play_startup_animation();
    assert!(!pane.wordmark.animation.is_active());
    let new_tab = NeoismAgentPane::default();
    assert!(new_tab.wordmark.startup_pending);
}

#[test]
fn cancelled_wordmark_click_does_not_drive_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.wordmark.animation.replay();
    pane.wordmark.animation.cancel();

    assert_ne!(pane.animation_reason(), Some("wordmark"));
}

#[test]
fn activity_overlap_reserves_background_menu_without_disabling_primary_word() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("activity-root".into());
    pane.messages.push(NeoismAgentMessage::user("prompt"));
    // Primary Background glyph padding/motion extends into the child row.
    pane.register_activity_status_rect([10.0, 20.0, 100.0, 30.0]);
    pane.register_background_status_rect([10.0, 40.0, 120.0, 24.0]);
    assert!(pane.activity_status_contains(20.0, 30.0));
    assert!(pane.pop_activity_status_click(20.0, 30.0));
    let effect = pane.activity_animation_frame().unwrap().effect;
    for (x, y) in [(20.0, 45.0), (10.0, 40.0), (110.0, 50.0)] {
        assert!(pane.background_status_contains(x, y));
        assert!(
            !pane.activity_status_contains(x, y),
            "child menu owns overlap and its boundary"
        );
        assert!(!pane.pop_activity_status_click(x, y));
        assert_eq!(
            pane.activity_animation_frame().unwrap().effect,
            effect,
            "menu press must not restart replay"
        );
    }
    // A menu-free primary word still replays, even when its label is Background.
    assert!(pane.pop_activity_status_click(20.0, 30.0));
    assert_ne!(pane.activity_animation_frame().unwrap().effect, effect);
    pane.clear_tool_hit_rects();
    assert!(!pane.activity_status_contains(20.0, 30.0));
    assert!(!pane.pop_activity_status_click(20.0, 30.0));
    assert_eq!(
        pane.animation_reason(),
        None,
        "skipping status render must not leave stale hit/redraw state"
    );
}

#[test]
fn activity_replay_bag_is_independent_and_restarts_in_conversation() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("activity-root".into());
    pane.messages.push(NeoismAgentMessage::user("prompt"));
    pane.wordmark.animation.cancel();
    pane.register_activity_status_rect([10.0, 20.0, 40.0, 12.0]);
    let mut effects = Vec::new();
    for _ in 0..15 {
        assert!(pane.pop_activity_status_click(20.0, 25.0));
        let frame = pane.activity_animation_frame().unwrap();
        assert!(frame.progress < 0.1, "a second click restarts immediately");
        assert!(
            !effects.contains(&frame.effect),
            "one complete shuffled bag"
        );
        effects.push(frame.effect);
        assert_eq!(pane.animation_reason(), Some("agent_activity_word"));
    }
    assert!(pane.pop_activity_status_click(20.0, 25.0));
    assert_ne!(
        pane.activity_animation_frame().unwrap().effect,
        *effects.last().unwrap()
    );
    assert!(!pane.wordmark.animation.is_active());
}

#[test]
fn activity_hits_are_frame_local_clipped_and_preserve_background_menu() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("activity-root".into());
    pane.messages.push(NeoismAgentMessage::user("prompt"));
    pane.register_background_status_rect([60.0, 20.0, 40.0, 12.0]);
    assert!(!pane.pop_activity_status_click(20.0, 25.0));
    // Only the clipped, visible primary word is supplied by the renderer.
    pane.register_activity_status_rect([10.0, 20.0, 20.0, 12.0]);
    pane.register_activity_status_rect([0.0, 0.0, 0.0, 20.0]);
    pane.register_activity_status_rect([f32::NAN, 0.0, 20.0, 20.0]);
    assert!(!pane.activity_status_contains(31.0, 25.0));
    assert!(!pane.activity_status_contains(f32::NAN, 25.0));
    assert!(pane.pop_activity_status_click(10.0, 20.0));
    assert!(!pane.pop_activity_status_click(70.0, 25.0));
    assert!(pane.background_status_contains(70.0, 25.0));
    pane.clear_tool_hit_rects();
    assert!(!pane.activity_status_contains(20.0, 25.0));
    assert!(
        pane.activity_animation_frame().is_some(),
        "clearing hits is not cancellation"
    );
    assert_eq!(
        pane.animation_reason(),
        None,
        "offscreen status cannot spin"
    );
    pane.register_activity_status_rect([10.0, 20.0, 20.0, 12.0]);
    assert_eq!(pane.animation_reason(), Some("agent_activity_word"));
    pane.reset_transient_timeline_interactions();
    assert!(pane.activity_animation_frame().is_none());
    assert!(!pane.activity_status_contains(20.0, 25.0));
}

#[test]
fn activity_replay_scope_and_expiry_do_not_leave_idle_redraw() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("activity-root".into());
    pane.messages.push(NeoismAgentMessage::user("prompt"));
    pane.register_activity_status_rect([10.0, 20.0, 20.0, 12.0]);
    assert!(pane.pop_activity_status_click(20.0, 25.0));
    pane.session_id = Some("other-root".into());
    assert!(!pane.activity_status_contains(20.0, 25.0));
    assert!(pane.activity_animation_frame().is_none());
    pane.register_activity_status_rect([10.0, 20.0, 20.0, 12.0]);
    assert!(pane.activity_animation_frame().is_none());
    assert_ne!(pane.animation_reason(), Some("agent_activity_word"));
    assert!(pane.pop_activity_status_click(20.0, 25.0));
    std::thread::sleep(std::time::Duration::from_millis(2800));
    assert!(pane.activity_animation_frame().is_none());
    assert_eq!(
        pane.animation_reason(),
        None,
        "expired replay leaves no idle redraw owner"
    );
}

#[test]
fn fresh_wordmark_click_drives_pixel_animation() {
    let mut pane = NeoismAgentPane::default();
    assert!(!pane.wordmark_hit(100.0, 40.0));
    pane.wordmark.rect = Some([10.0, 20.0, 300.0, 50.0]);
    assert!(pane.wordmark_hit(100.0, 40.0));
    assert!(pane.wordmark_hit(10.0, 20.0));
    assert!(pane.wordmark_hit(310.0, 70.0));
    assert!(!pane.wordmark_hit(311.0, 40.0));
    assert!(!pane.wordmark_hit(100.0, 71.0));
    assert!(!pane.wordmark_hit(f32::NAN, 40.0));
    assert!(!pane.pop_wordmark_click(0.0, 0.0));
    assert!(pane.pop_wordmark_click(100.0, 40.0));

    assert_eq!(pane.animation_reason(), Some("wordmark"));
}

#[test]
fn connected_idle_event_stream_does_not_drive_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.event_stream = Some(AgentSessionEventStream::connected_for_test("sess-1"));

    assert_eq!(pane.animation_reason(), Some("agent_home_wordmark"));
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
fn attached_session_counts_as_conversation_before_messages_load() {
    let mut pane = NeoismAgentPane::default();

    assert!(!pane.has_conversation());

    pane.session_id = Some("session-1".to_string());

    assert!(pane.has_conversation());
}

#[test]
fn execution_timer_uses_family_total_for_root_and_own_total_for_child() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.apply_execution_activity(
        neoism_ui::panels::agent_pane::state::ExecutionActivityState {
            execution_id: "execution".into(),
            root_session_id: "root".into(),
            completed_ms: 8_000,
            session_activities: [(
                "child".into(),
                neoism_ui::panels::agent_pane::state::ProviderActivityState {
                    completed_ms: 3_000,
                    ..Default::default()
                },
            )]
            .into(),
            revision: 1,
            ..Default::default()
        },
    );
    assert_eq!(pane.streaming_elapsed_seconds(), Some(8.0));
    pane.session_id = Some("child".into());
    pane.parent_session_id = Some("root".into());
    assert_eq!(pane.streaming_elapsed_seconds(), Some(3.0));
}

#[test]
fn live_execution_owns_redraw_across_transient_run_idle() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.messages.push(NeoismAgentMessage::user("prompt"));
    pane.apply_execution_activity(
        neoism_ui::panels::agent_pane::state::ExecutionActivityState {
            execution_id: "execution".into(),
            root_session_id: "root".into(),
            active_segments: [("provider".into(), 1_000)].into(),
            revision: 1,
            finished: false,
            ..Default::default()
        },
    );
    pane.note_session_run_idle("root");
    assert_eq!(pane.animation_reason(), Some("streaming"));
}

#[test]
fn live_execution_without_visible_status_does_not_reserve_status_row() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.messages.push(NeoismAgentMessage::user("prompt"));
    pane.apply_execution_activity(
        neoism_ui::panels::agent_pane::state::ExecutionActivityState {
            execution_id: "execution".into(),
            root_session_id: "root".into(),
            active_segments: [("provider".into(), 1_000)].into(),
            revision: 1,
            finished: false,
            ..Default::default()
        },
    );
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert_eq!(pane.streaming_label(), "Crafting");
    pane.note_session_run_idle("root");
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);

    assert_eq!(pane.animation_reason(), Some("streaming"));
    assert_eq!(pane.streaming_label(), "");
    assert!(!pane.has_status_activity());
}

#[test]
fn finished_execution_does_not_reserve_status_row() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_execution_activity(
        neoism_ui::panels::agent_pane::state::ExecutionActivityState {
            execution_id: "execution".into(),
            root_session_id: "root".into(),
            completed_ms: 8_000,
            revision: 2,
            finished: true,
            ..Default::default()
        },
    );
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_label(), "");
    assert_eq!(pane.streaming_elapsed_seconds(), Some(8.0));
}

#[test]
fn completed_child_does_not_reserve_status_row_for_active_family() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".into());
    pane.parent_session_id = Some("root".into());
    pane.apply_execution_activity(
        neoism_ui::panels::agent_pane::state::ExecutionActivityState {
            execution_id: "execution".into(),
            root_session_id: "root".into(),
            completed_ms: 8_000,
            revision: 2,
            finished: false,
            ..Default::default()
        },
    );
    assert!(pane.apply_branch_lifecycle_snapshot(
        "root".into(),
        1,
        [("child".into(), "completed".into(), Some(10))],
    ));
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_label(), "");
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
    pane.ensure_background_task_activity_clock();

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
    pane.ensure_background_task_activity_clock();

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
fn background_task_count_settles_when_live_upsert_adds_completion() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo check",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);
    pane.ensure_background_task_activity_clock();
    assert_eq!(pane.running_background_task_count(), 1);

    let mut completed = NeoismAgentMessage::tool(
        "Background Task Result",
        "Background shell task finished.\njob_id: job-1\nstatus: completed",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    completed.detail = completed.text.clone();
    // Production ingress owns cache reconciliation; callers do not need a
    // separate clock refresh after the completion part lands.
    pane.upsert_part_message(completed);

    assert_eq!(pane.running_background_task_count(), 0);
    assert!(!pane.has_status_activity());
    assert!(pane.active_background_task_summaries().is_empty());
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
    assert!(pane.active_background_task_summaries().is_empty());
    assert_eq!(pane.streaming_label(), "");
    assert_eq!(pane.animation_reason(), None);
}

#[test]
fn background_authority_survives_completion_and_delayed_launch_parts() {
    let mut pane = NeoismAgentPane::default();
    let stale = NeoismAgentMessage::tool(
        "Old release monitor",
        "job_id: old-monitor\nstatus: running",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    )
    .with_id("old-launch");
    pane.upsert_part_message(stale.clone());
    pane.apply_running_background_tasks("server-a", 10, &[("latest-check".into(), 100)]);
    assert_eq!(
        pane.active_background_task_summaries(),
        vec!["latest-check · running"]
    );
    pane.apply_running_background_tasks("server-a", 11, &[]);
    // Production finish ordering: authoritative empty, then completion card.
    pane.upsert_part_message(client_background_completion_card("latest-check"));
    assert_eq!(pane.running_background_task_count(), 0);
    pane.upsert_part_message(stale);
    assert_eq!(pane.running_background_task_count(), 0);
    assert!(pane.active_background_task_summaries().is_empty());
    assert!(!pane.has_status_activity());
    assert!(!pane.apply_running_background_tasks(
        "server-a",
        10,
        &[("latest-check".into(), 100)]
    ));
    pane.apply_running_background_tasks("server-a", 11, &[]);
    assert_eq!(pane.running_background_task_count(), 0);
}

#[test]
fn background_reconnect_empty_is_independent_of_execution_hydration_race() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.apply_running_background_tasks("old-server", 100, &[("old-job".into(), 100)]);
    pane.runtime_status_requests.insert("root".into(), 7);
    pane.note_session_runtime_event("root");
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
            session_id: "root".into(),
            request_generation: 7,
            runtime_revision: 0,
            result: Ok(HashMap::new()),
            runtime: Ok(super::super::api::FamilyRuntimeSnapshot {
                root_session_id: "root".into(),
                running_background_tasks: Some(Vec::new()),
                background_jobs_epoch: Some("new-server".into()),
                background_jobs_revision: Some(0),
                ..Default::default()
            }),
            permissions: Ok(Vec::new()),
            questions: Ok(Vec::new()),
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert!(!pane.apply_running_background_tasks(
        "old-server",
        101,
        &[("old-job".into(), 100)]
    ));
}

#[test]
fn background_family_authority_survives_child_switch_and_duplicate_completion() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".into());
    pane.session_cache.insert("child".into(), child);
    pane.apply_running_background_tasks("server", 1, &[("child-job".into(), 100)]);
    pane.activate_cached_session("child");
    assert_eq!(pane.running_background_task_count(), 1);
    assert_eq!(
        pane.active_background_task_summaries(),
        vec!["child-job · running"]
    );
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::BackgroundTasksUpdated {
                epoch: "server".into(),
                revision: 2,
                tasks: Vec::new(),
            },
            AgentSessionUpdate::BackgroundTaskCompleted {
                session_id: "child".into(),
                job_id: "child-job".into(),
                status: "completed".into(),
            },
            AgentSessionUpdate::BackgroundTaskCompleted {
                session_id: "child".into(),
                job_id: "child-job".into(),
                status: "completed".into(),
            },
        ],
    ));
    pane.drain_server_updates();
    assert_eq!(
        pane.messages
            .iter()
            .filter(|m| m.id == "background-task-child-job")
            .count(),
        1
    );
    assert_eq!(pane.running_background_task_count(), 0);
    pane.activate_cached_session("root");
    assert_eq!(pane.running_background_task_count(), 0);
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn background_runtime_rejects_stale_revision_and_accepts_new_server_epoch() {
    let mut pane = NeoismAgentPane::default();
    assert!(pane.apply_running_background_tasks("server-a", 2, &[("job-1".into(), 100)],));
    assert_eq!(pane.running_background_task_count(), 1);
    assert!(!pane.apply_running_background_tasks("server-a", 1, &[]));
    assert_eq!(pane.running_background_task_count(), 1);
    assert!(pane.apply_running_background_tasks("server-b", 0, &[]));
    assert_eq!(pane.running_background_task_count(), 0);
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
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_elapsed_seconds(), None);
    assert!(pane.active_background_task_summaries().is_empty());
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
        "No background tasks are running.",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    snapshot.detail = snapshot.text.clone();
    pane.messages.push(snapshot);
    pane.ensure_background_task_activity_clock();

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
fn cross_project_cached_chat_restores_execution_directory_without_workspace_switch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("project-a-chat".into());
    pane.directory = Some("/tmp/project-a".into());
    let mut cached = CachedAgentSession::live_only();
    cached.hydrated = true;
    cached.state.directory = Some("/tmp/project-b".into());
    pane.session_cache.insert("project-b-chat".into(), cached);
    pane.runtime_hydrated_sessions
        .insert("project-b-chat".into());
    pane.activate_cached_session("project-b-chat");
    assert_eq!(pane.session_directory(), Some("/tmp/project-b"));
    assert!(!pane
        .ui_events
        .iter()
        .any(|event| matches!(event, NeoismAgentUiEvent::SwitchWorkspace { .. })));
    pane.activate_cached_session("project-a-chat");
    assert_eq!(pane.session_directory(), Some("/tmp/project-a"));
    assert!(!pane
        .ui_events
        .iter()
        .any(|event| matches!(event, NeoismAgentUiEvent::SwitchWorkspace { .. })));
}

fn parked_layout_for_test(epoch: u64, source_len: usize) -> TimelineLayoutCache {
    TimelineLayoutCache {
        question_targets: Default::default(),
        epoch,
        artifact_revision: None,
        source_len,
        width_bucket: 640,
        scale_bucket: 100,
        gap_bucket: 12,
        content_height: 900.0,
        pages: Vec::new(),
        rows: Vec::new(),
        estimated_prefix_rows: 0,
        estimated_suffix_start: 0,
    }
}

#[test]
fn conversation_latency_warm_switch_restores_parked_layout_and_dirty_marks_within_family() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.messages = vec![NeoismAgentMessage::user("question").with_id("user")];
    pane.timeline_layout_epoch = 42;
    pane.timeline_live_trace_start = Some(1);
    pane.timeline_live_trace_anchor = Some("user".into());
    pane.timeline_layout_cache.replace(Some(parked_layout_for_test(42, 1)));
    pane.timeline_dirty_message_ids.insert("user".into());
    pane.timeline_dirty_message_indices.insert(0);
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".into());
    pane.session_cache.insert("child".into(), child);
    pane.runtime_hydrated_sessions.extend(["root".into(), "child".into()]);

    pane.activate_cached_session("child");
    assert!(pane.timeline_dirty_message_ids.is_empty());
    pane.activate_cached_session("root");

    let cache = pane.timeline_layout_cache.borrow();
    let cache = cache.as_ref().expect("unchanged family layout was parked");
    assert_eq!(cache.epoch, 42);
    assert_eq!(pane.timeline_layout_epoch, 42);
    assert_eq!(
        (cache.width_bucket, cache.scale_bucket, cache.gap_bucket),
        (640, 100, 12)
    );
    assert_eq!(cache.source_len, 1);
    assert_eq!(pane.timeline_live_trace_start, Some(1));
    assert!(pane.timeline_dirty_message_ids.contains("user"));
    assert!(pane.timeline_dirty_message_indices.contains(&0));
}

#[test]
fn conversation_latency_parking_discards_layout_when_reset_or_merge_changes_geometry() {
    for reason in ["expanded", "animation", "selection", "prepend", "merge", "trace"] {
        let mut pane = NeoismAgentPane::default();
        pane.session_id = Some("root".into());
        pane.messages = vec![NeoismAgentMessage::user("question").with_id("user")];
        pane.timeline_layout_cache.replace(Some(parked_layout_for_test(0, 1)));
        match reason {
            "expanded" => {
                pane.expanded_tool_ids.insert("tool".into());
            }
            "animation" => {
                pane.tool_expand_anims.insert(
                    "tool".into(),
                    ToolExpandAnimation {
                        started_at: Instant::now(),
                        expanding: false,
                        start_progress: 1.0,
                    },
                );
            }
            "selection" => {
                pane.selected_tool_group_child = Some(("group".into(), "child".into()));
            }
            "prepend" => pane.pending_timeline_prepend_count = Some(1),
            "merge" => {
                let mut live = CachedAgentSession::live_only();
                live.messages.push(NeoismAgentMessage::assistant("new").with_id("new"));
                pane.session_cache.insert("root".into(), live);
            }
            "trace" => pane.timeline_live_trace_start = Some(0),
            _ => unreachable!(),
        }
        pane.cache_current_session(reason != "trace");
        assert!(pane.session_cache["root"].timeline_layout_cache.is_none(), "{reason}");
    }
}

#[test]
fn conversation_latency_cached_layout_is_invalidated_across_family_boundary() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("other".into());
    let mut cached = CachedAgentSession::live_only();
    cached.hydrated = true;
    cached.timeline_layout_epoch = 42;
    cached.timeline_layout_cache = Some(parked_layout_for_test(42, 0));
    cached.timeline_live_trace_start = Some(0);
    pane.session_cache.insert("root".into(), cached);

    pane.activate_cached_session("root");

    assert!(pane.timeline_layout_cache.borrow().is_none());
    assert_eq!(pane.timeline_layout_epoch, 43);
    assert_eq!(pane.timeline_live_trace_start, None);
}

#[test]
fn conversation_latency_ongoing_child_trace_reveal_invalidates_restored_layout() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".into());
    child.timeline_layout_epoch = 42;
    child.timeline_layout_cache = Some(parked_layout_for_test(42, 0));
    child.runtime.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.session_cache.insert("child".into(), child);
    pane.runtime_hydrated_sessions.insert("child".into());

    pane.activate_cached_session("child");

    assert_eq!(pane.timeline_live_trace_start, Some(0));
    assert_eq!(pane.timeline_layout_epoch, 43);
    assert!(pane.timeline_layout_cache.borrow().is_none());
}

#[test]
fn conversation_latency_existing_conversation_discards_only_queued_draft_defaults() {
    let mut pane = NeoismAgentPane::with_directory(Some("/tmp/project".into()));
    pane.push_outbound(OutboundAgentCommand::AbortSession);
    pane.discard_queued_draft_defaults();
    assert_eq!(pane.drain_pending_outbound(), vec![OutboundAgentCommand::AbortSession]);
    assert!(!matches!(pane.config_chip_transition(), Some(
        neoism_ui::panels::agent_pane::state::ConfigChipTransition::Loading { .. }
    )));
    // New Chat still uses the normal defaults path.
    pane.apply_config_defaults();
    assert_eq!(pane.drain_pending_outbound(), vec![OutboundAgentCommand::ApplyConfigDefaults]);
}

#[test]
fn warm_switch_activates_cached_session_without_outbound_or_network_repair() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.messages = vec![NeoismAgentMessage::user("root")];
    let mut cached = CachedAgentSession::live_only();
    cached.hydrated = true;
    cached.messages = vec![NeoismAgentMessage::assistant("child")];
    cached.state.parent_id = Some("sess-1".to_string());
    cached.timeline_layout_epoch = 42;
    cached.timeline_live_trace_start = Some(0);
    cached.runtime.queued_prompt_count = 2;
    cached
        .runtime
        .note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.session_cache.insert("sess-2".to_string(), cached);
    pane.runtime_hydrated_sessions.insert("sess-2".to_string());

    pane.switch_session("sess-2".to_string());

    assert_eq!(pane.session_id.as_deref(), Some("sess-2"));
    assert_eq!(pane.messages[0].text, "child");
    assert_eq!(pane.timeline_layout_epoch, 42);
    assert_eq!(pane.queued_prompt_count, 2);
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert_eq!(pane.session_cache["sess-1"].messages[0].text, "root");
    assert!(pane.drain_pending_outbound().is_empty());
    assert!(!pane.runtime_status_requests.contains_key("sess-2"));
}

#[test]
fn subagent_visit_preserves_parent_live_tool_trace() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("question").with_id("user-root"),
        task_tool_message("child", "completed"),
    ];
    pane.timeline_live_trace_start = Some(1);
    pane.timeline_live_trace_anchor = Some("user-root".to_string());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    child.messages = vec![NeoismAgentMessage::assistant("child answer")];
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions
        .extend(["root".to_string(), "child".to_string()]);

    pane.switch_session("child".to_string());
    pane.switch_session("root".to_string());

    assert_eq!(pane.timeline_live_trace_start, Some(1));
    assert_eq!(
        pane.timeline_live_trace_anchor.as_deref(),
        Some("user-root")
    );
}

#[test]
fn leaving_conversation_family_settles_live_tool_trace() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root-a".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("question").with_id("user-a"),
        task_tool_message("child-a", "completed"),
    ];
    pane.timeline_live_trace_start = Some(1);
    pane.timeline_live_trace_anchor = Some("user-a".to_string());
    let mut other = CachedAgentSession::live_only();
    other.hydrated = true;
    other.messages = vec![NeoismAgentMessage::assistant("other conversation")];
    pane.session_cache.insert("root-b".to_string(), other);
    pane.runtime_hydrated_sessions
        .extend(["root-a".to_string(), "root-b".to_string()]);

    pane.switch_session("root-b".to_string());
    pane.switch_session("root-a".to_string());

    assert_eq!(pane.timeline_live_trace_start, None);
    assert_eq!(pane.timeline_live_trace_anchor, None);
}

#[test]
fn opening_ongoing_child_reveals_already_emitted_tool_history() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    child.messages = vec![
        NeoismAgentMessage::user("first investigation"),
        NeoismAgentMessage::tool(
            "Read(src/lib.rs)",
            "src/lib.rs",
            "completed",
            "read",
            NeoismAgentOutputKind::Text,
            "rust",
            Vec::new(),
        ),
        NeoismAgentMessage::user("continue the investigation"),
        NeoismAgentMessage::reasoning("checking prior state"),
    ];
    child
        .runtime
        .note_streaming(NeoismAgentStreamingState::Working, Some("Read".to_string()));
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child".to_string());

    pane.switch_session("child".to_string());

    assert_eq!(pane.timeline_live_trace_start, Some(0));
    assert_eq!(pane.messages.len(), 4);
}

#[test]
fn cold_runtime_hydration_reveals_ongoing_child_tool_history() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("investigate"),
        NeoismAgentMessage::tool(
            "Grep(query)",
            "query",
            "running",
            "grep",
            NeoismAgentOutputKind::Text,
            "text",
            Vec::new(),
        ),
    ];
    let statuses = HashMap::from([(
        "child".to_string(),
        super::super::api::SessionStatusSnapshot {
            kind: "busy".to_string(),
            ..Default::default()
        },
    )]);

    pane.apply_runtime_status_for_session("child", &statuses);

    assert_eq!(pane.timeline_live_trace_start, Some(0));
}

#[test]
fn older_pages_stay_visible_for_ongoing_child_inspector() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Working, None);
    pane.timeline_live_trace_start = Some(0);

    pane.note_timeline_prepend(40);

    assert_eq!(pane.timeline_live_trace_start, Some(0));
    assert_eq!(pane.take_timeline_prepend(), Some(40));
}

#[test]
fn session_titles_follow_cached_switches_and_reset_for_new_drafts() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("titled".into());
    pane.session_title = Some("Server title".into());
    let mut untitled = CachedAgentSession::live_only();
    untitled.hydrated = true;
    pane.session_cache.insert("untitled".into(), untitled);

    pane.activate_cached_session("untitled");
    assert_eq!(pane.session_title(), None);
    pane.activate_cached_session("titled");
    assert_eq!(pane.session_title(), Some("Server title"));
    pane.create_new_session();
    assert_eq!(pane.session_title(), None);
}

#[test]
fn live_session_title_changes_and_blank_resets_are_authoritative() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.session_title = Some("Old title".into());
    for title in [Some("Updated title"), None, Some("")] {
        pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
            "root",
            [AgentSessionUpdate::SessionMetadataUpdated {
                title: title.map(str::to_string),
                agent: None,
                model: None,
                connection_id: None,
                thinking: None,
            }],
        ));
        assert!(pane.drain_live_session_updates());
        assert_eq!(
            pane.session_title(),
            if title == Some("") {
                None
            } else {
                Some("Updated title")
            }
        );
    }
}

#[test]
fn offscreen_root_stream_is_live_before_switching_back() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::assistant("child transcript")];
    let mut root = CachedAgentSession::live_only();
    root.hydrated = true;
    root.state.parent_id = None;
    root.messages = vec![NeoismAgentMessage::user("question")];
    pane.session_cache.insert("root".to_string(), root);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::QueueStatus {
                count: 1,
                preview: Some("next".to_string()),
                started_at: Some(1),
            },
            AgentSessionUpdate::GoalUpdated {
                goal: Some(SessionGoal {
                    text: "keep shipping".to_string(),
                    updated: 9,
                    ..SessionGoal::default()
                }),
                version: 9,
            },
            AgentSessionUpdate::SessionMetadataUpdated {
                title: Some("Root conversation".to_string()),
                agent: Some("plan".to_string()),
                model: Some("openai/gpt-5.6".to_string()),
                connection_id: Some(Some("conn-work".to_string())),
                thinking: Some(Some("high".to_string())),
            },
            AgentSessionUpdate::PartDelta {
                message_id: Some("assistant-message".to_string()),
                part_id: Some("answer".to_string()),
                kind: Some("text".to_string()),
                delta: "streamed while viewing child".to_string(),
            },
        ],
    ));

    assert!(pane.drain_server_updates());
    pane.switch_session("root".to_string());

    assert_eq!(pane.session_id.as_deref(), Some("root"));
    assert!(pane
        .messages
        .iter()
        .any(|message| message.text == "streamed while viewing child"));
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(
        pane.side_panel
            .session_goal()
            .map(|goal| goal.text.as_str()),
        Some("keep shipping")
    );
    assert_eq!(pane.agent.as_deref(), Some("plan"));
    assert_eq!(pane.model, "openai/gpt-5.6");
    assert_eq!(pane.connection_id.as_deref(), Some("conn-work"));
    assert_eq!(pane.thinking.as_deref(), Some("high"));
    assert_eq!(pane.session_title(), Some("Root conversation"));
    assert!(pane.runtime_status_requests.get("root").is_none());
}

#[test]
fn parented_view_is_tracked_before_its_roster_hydrates() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::user("question")];

    // A cached activation can start the family-root stream before the async
    // subagent roster contains the viewed child. Seeded ownership is what
    // lets the stream decoder classify the child's very first delta.
    assert_eq!(
        pane.tracked_sessions_for_stream("root"),
        vec!["child".to_string()]
    );

    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::ChildPartDelta {
            session_id: "child".to_string(),
            message_id: Some("assistant-message".to_string()),
            part_id: Some("answer".to_string()),
            kind: Some("text".to_string()),
            delta: "visible child token".to_string(),
        }],
    ));

    assert!(pane.drain_server_updates());
    assert!(pane
        .messages
        .iter()
        .any(|message| message.text == "visible child token"));
    assert!(!pane.session_cache.contains_key("child"));
}

#[test]
fn child_completion_and_parent_continuation_stay_live_during_child_view() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::assistant("child transcript")];
    let mut root = CachedAgentSession::live_only();
    root.hydrated = true;
    root.messages = vec![task_tool_message("child", "running")];
    pane.session_cache.insert("root".to_string(), root);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::SubagentCompleted {
                task_id: "child".to_string(),
                status: "completed".to_string(),
                title: None,
                agent: None,
                root_session_id: None,
                execution_id: None,
                family_revision: None,
            },
            AgentSessionUpdate::PartDelta {
                message_id: Some("parent-answer".to_string()),
                part_id: Some("answer".to_string()),
                kind: Some("text".to_string()),
                delta: "parent resumed while child was open".to_string(),
            },
        ],
    ));

    pane.drain_server_updates();
    pane.switch_session("root".to_string());

    let task = pane
        .messages
        .iter()
        .find(|message| message.tool == "task")
        .expect("parent task card");
    assert_eq!(task.status, "completed");
    assert!(task.detail.contains("status: completed"));
    assert!(pane
        .messages
        .iter()
        .any(|message| message.text == "parent resumed while child was open"));
}

#[test]
fn late_completed_tool_part_cannot_resurrect_idle_parent_status() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Working, Some("Task".to_string()));
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::SessionIdle,
            AgentSessionUpdate::PartUpdated {
                message: task_tool_message("child", "completed"),
                parent_message_id: Some("assistant-message".to_string()),
            },
        ],
    ));

    pane.drain_server_updates();

    assert!(!pane.is_streaming());
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(pane
        .messages
        .iter()
        .any(|message| message.tool == "task" && message.status == "completed"));
}

#[test]
fn switching_families_replaces_stale_event_stream_root() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root-a".to_string());
    pane.session_tree_root_id = Some("root-a".to_string());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root-b".to_string());
    pane.session_cache.insert("child-b".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child-b".to_string());

    pane.switch_session("child-b".to_string());

    assert_eq!(pane.session_tree_root_id.as_deref(), Some("root-b"));
    assert_eq!(
        pane.event_stream
            .as_ref()
            .map(AgentSessionEventStream::session_id),
        Some("root-b")
    );
}

use neoism_ui::panels::agent_pane::conversation_header::Direction;

fn set_navigation_roster(pane: &mut NeoismAgentPane, children: &[&str]) {
    let mut roster = vec![NeoismAgentSessionEntry::new("root", "Main session", "")];
    roster.extend(children.iter().map(|id| {
        NeoismAgentSessionEntry::new(*id, *id, "")
            .with_runtime_status(Some("running".into()))
    }));
    pane.side_panel.set_subagents(roster);
    for id in children {
        pane.side_panel
            .set_branch_activity_status(*id, BranchStatus::Active);
    }
}

#[test]
fn conversation_navigation_follows_roster_including_nested_children_without_wrap() {
    let mut pane = NeoismAgentPane::default();
    pane.session_tree_root_id = Some("root".into());
    pane.parent_session_id = Some("root".into());
    set_navigation_roster(&mut pane, &["child", "nested", "sibling"]);
    pane.session_id = Some("child".into());
    let nav = pane.conversation_navigation();
    assert_eq!(nav.previous, Some("root"));
    assert!(nav.previous_is_main);
    assert_eq!(nav.next, Some("nested"));

    pane.session_id = Some("nested".into());
    pane.parent_session_id = Some("child".into());
    let nav = pane.conversation_navigation();
    assert_eq!(nav.previous, Some("child"));
    assert!(!nav.previous_is_main);
    assert_eq!(nav.next, Some("sibling"));

    pane.session_id = Some("sibling".into());
    let nav = pane.conversation_navigation();
    assert_eq!(nav.previous, Some("nested"));
    assert_eq!(nav.next, None);

    pane.session_id = Some("absent".into());
    let nav = pane.conversation_navigation();
    assert_eq!(nav.previous, Some("root"));
    assert!(nav.previous_is_main);
    assert_eq!(nav.next, None);

    pane.session_id = Some("root".into());
    pane.parent_session_id = None;
    let nav = pane.conversation_navigation();
    assert_eq!(nav.previous, None);
    assert_eq!(nav.next, None);
}

#[test]
fn conversation_navigation_rejects_stale_direction_after_roster_reorder() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("nested".into());
    pane.parent_session_id = Some("child".into());
    pane.session_tree_root_id = Some("root".into());
    set_navigation_roster(&mut pane, &["child", "nested", "sibling"]);
    let rect = [10., 10., 100., 24.];
    pane.conversation_header
        .register(Direction::Previous, "child", rect, rect, &[]);
    assert_eq!(pane.conversation_navigation_at(15., 15.), Some("child"));
    set_navigation_roster(&mut pane, &["sibling", "nested", "child"]);
    assert_eq!(pane.conversation_navigation().next, Some("child"));
    assert_eq!(pane.conversation_navigation_at(15., 15.), None);
    assert!(!pane.activate_conversation_navigation(15., 15.));
    let next_rect = [140., 10., 100., 24.];
    pane.conversation_header.register(
        Direction::Next,
        "child",
        next_rect,
        next_rect,
        &[],
    );
    assert_eq!(pane.conversation_navigation_at(150., 15.), Some("child"));
    assert_eq!(pane.conversation_navigation_at(15., 15.), None);
    assert_eq!(pane.conversation_navigation_at(300., 15.), None);
}

#[test]
fn conversation_navigation_targets_true_root_and_rejects_missing_or_current() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("grandchild".into());
    pane.parent_session_id = Some("child".into());
    pane.session_tree_root_id = Some("root".into());
    assert_eq!(pane.conversation_navigation().previous, Some("root"));
    pane.session_tree_root_id = Some("grandchild".into());
    assert_eq!(pane.conversation_navigation().previous, None);
    pane.session_tree_root_id = Some(String::new());
    assert_eq!(pane.conversation_navigation().previous, None);
    pane.session_tree_root_id = None;
    assert_eq!(pane.conversation_navigation().previous, None);
    pane.session_tree_root_id = Some("root".into());
    pane.parent_session_id = None;
    assert_eq!(pane.conversation_navigation().previous, None);
    pane.parent_session_id = Some("child".into());
    pane.session_id = None;
    assert_eq!(pane.conversation_navigation().previous, None);
    assert!(!pane.activate_conversation_navigation(15., 15.));
}

#[test]
fn conversation_navigation_rejects_stale_clipped_and_occluded_geometry() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".into());
    pane.parent_session_id = Some("root".into());
    pane.session_tree_root_id = Some("root".into());
    let rect = [10., 10., 100., 24.];
    let clip = [0., 0., 200., 100.];
    pane.conversation_header
        .register(Direction::Previous, "old-root", rect, clip, &[]);
    assert_eq!(pane.conversation_navigation_at(15., 15.), None);
    assert!(!pane.activate_conversation_navigation(15., 15.));
    pane.conversation_header
        .register(Direction::Previous, "root", rect, clip, &[]);
    assert_eq!(pane.conversation_navigation_at(15., 15.), Some("root"));
    assert!(pane.update_link_hover_at(15., 15.));
    assert!(pane.link_hover_active());
    pane.prompt_picker_rect = Some(rect);
    assert_eq!(pane.conversation_navigation_at(15., 15.), None);
    pane.prompt_picker_rect = None;
    pane.composer_interactions.occlusion_rects.push(rect);
    assert_eq!(pane.conversation_navigation_at(15., 15.), None);
    pane.composer_interactions.occlusion_rects.clear();
    assert_eq!(pane.conversation_navigation_at(150., 15.), None);
    pane.conversation_header
        .register(Direction::Previous, "root", rect, clip, &[rect]);
    assert_eq!(pane.conversation_navigation_at(15., 15.), None);
    pane.conversation_header.register(
        Direction::Previous,
        "root",
        rect,
        [120., 0., 80., 100.],
        &[],
    );
    assert_eq!(pane.conversation_navigation_at(15., 15.), None);
    pane.conversation_header.clear();
    assert_eq!(pane.conversation_navigation_at(15., 15.), None);
}

#[test]
fn conversation_navigation_geometry_clears_on_session_and_home_reset() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".into());
    pane.parent_session_id = Some("root".into());
    pane.session_tree_root_id = Some("root".into());
    let rect = [10., 10., 100., 24.];
    pane.conversation_header
        .register(Direction::Previous, "root", rect, rect, &[]);
    pane.switch_session("child".into());
    assert_eq!(
        pane.conversation_header
            .hit_at(15., 15.)
            .map(|hit| hit.target),
        None
    );
    pane.conversation_header
        .register(Direction::Previous, "root", rect, rect, &[]);
    pane.create_new_session();
    assert_eq!(
        pane.conversation_header
            .hit_at(15., 15.)
            .map(|hit| hit.target),
        None
    );
    assert_eq!(pane.conversation_navigation().previous, None);
}

#[test]
fn conversation_navigation_restores_cached_root_draft_transcript_and_permissions() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.session_tree_root_id = Some("root".into());
    pane.input = "unfinished parent draft".into();
    pane.messages
        .push(NeoismAgentMessage::user("parent transcript"));
    set_navigation_roster(&mut pane, &["child", "grandchild"]);
    pane.enqueue_pending_permission(child_permission("perm-back"));
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".into());
    pane.session_cache.insert("child".into(), child);
    pane.runtime_hydrated_sessions.insert("child".into());
    pane.session_preloads_in_flight.insert("child".into());
    let mut nested = CachedAgentSession::live_only();
    nested.hydrated = true;
    nested.state.parent_id = Some("child".into());
    nested
        .messages
        .push(NeoismAgentMessage::user("nested transcript"));
    pane.session_cache.insert("grandchild".into(), nested);
    pane.runtime_hydrated_sessions.insert("grandchild".into());
    pane.runtime_hydrated_sessions.insert("root".into());
    // Suppress background preload requests: navigation must use the cache now.
    pane.session_preloads_in_flight.insert("grandchild".into());
    pane.session_preloads_in_flight.insert("root".into());
    pane.switch_session("grandchild".into());
    assert_eq!(pane.parent_session_id.as_deref(), Some("child"));
    assert_eq!(pane.conversation_navigation().previous, Some("child"));
    assert!(!pane.conversation_navigation().previous_is_main);
    assert_eq!(pane.conversation_navigation().next, None);
    assert!(pane.input.is_empty());
    pane.conversation_header.register(
        Direction::Previous,
        "child",
        [10., 10., 100., 24.],
        [0., 0., 200., 100.],
        &[],
    );
    assert!(pane.activate_conversation_navigation(15., 15.));
    assert_eq!(pane.session_id_str(), Some("child"));
    assert_eq!(pane.conversation_navigation().previous, Some("root"));
    assert!(pane.conversation_navigation().previous_is_main);
    assert_eq!(pane.conversation_navigation().next, Some("grandchild"));
    pane.conversation_header.register(
        Direction::Previous,
        "root",
        [10., 10., 100., 24.],
        [0., 0., 200., 100.],
        &[],
    );
    assert!(pane.activate_conversation_navigation(15., 15.));
    assert_eq!(pane.session_id_str(), Some("root"));
    assert_eq!(pane.input, "unfinished parent draft");
    assert!(pane
        .messages
        .iter()
        .any(|message| message.text == "parent transcript"));
    assert!(!pane
        .messages
        .iter()
        .any(|message| message.text == "nested transcript"));
    assert_eq!(
        pane.pending_permission()
            .map(|permission| permission.id.as_str()),
        Some("perm-back")
    );
    assert_eq!(
        pane.conversation_header
            .hit_at(15., 15.)
            .map(|hit| hit.target),
        None
    );
    assert!(!pane.activate_conversation_navigation(15., 15.));
}

#[test]
fn approval_card_persists_across_parent_child_navigation() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("root", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "USB agent", "general"),
    ]);
    let mut permission = child_permission("perm-family");
    permission.session_id = "child".to_string();
    pane.enqueue_pending_permission(permission);
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child".to_string());

    pane.switch_session("child".to_string());
    assert!(pane.is_subagent_session());
    assert_eq!(
        pane.pending_permission()
            .map(|permission| permission.id.as_str()),
        Some("perm-family")
    );

    pane.switch_session("root".to_string());
    assert!(!pane.is_subagent_session());
    assert_eq!(
        pane.pending_permission()
            .map(|permission| permission.id.as_str()),
        Some("perm-family")
    );
}

#[test]
fn clicking_the_active_detail_child_returns_to_main() {
    use neoism_ui::panels::agent_pane::state::side_panel::SidePanelMode;

    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.detail_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("root", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "Worker", "general"),
    ]);

    pane.detail_panel.set_mode(SidePanelMode::Subagents);
    assert!(pane.activate_clicked_detail_panel_subagent(1));
    assert_eq!(pane.session_id.as_deref(), Some("root"));
    assert_eq!(pane.detail_panel.selected_index(), 0);
}

#[test]
fn approval_card_does_not_leak_into_an_unrelated_family() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root-a".to_string());
    pane.session_tree_root_id = Some("root-a".to_string());
    pane.enqueue_pending_permission(child_permission("perm-family-a"));
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root-b".to_string());
    pane.session_cache.insert("child-b".to_string(), child);

    pane.switch_session("child-b".to_string());

    assert!(pane.pending_permission().is_none());
}

#[test]
fn windows_local_submit_invalidates_pre_submit_idle_hydration_only() {
    for windows_policy in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.session_id = Some("root".into());
        pane.status_timing = status_timing::DesktopStatusTiming::for_test(windows_policy);
        pane.runtime_status_requests.insert("root".into(), 7);
        let revision = pane.session_runtime_revision("root");
        pane.note_prompt_admission("first");
        pane.note_streaming(NeoismAgentStreamingState::Generating, None);
        pane.background_sender()
            .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
                session_id: "root".into(),
                request_generation: 7,
                runtime_revision: revision,
                result: Ok(HashMap::new()),
                runtime: Err("not requested".into()),
                permissions: Ok(Vec::new()),
                questions: Ok(Vec::new()),
            })
            .unwrap();
        pane.drain_background_updates();
        assert!(pane.is_streaming());
        assert!(!pane.runtime_status_requests.contains_key("root"));
    }
}

#[test]
fn stale_runtime_poll_cannot_overwrite_newer_live_state() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.note_session_runtime_event("root");
    pane.runtime_status_requests.insert("root".to_string(), 7);
    let statuses = HashMap::from([(
        "root".to_string(),
        super::super::api::SessionStatusSnapshot::default(),
    )]);
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
            session_id: "root".to_string(),
            request_generation: 7,
            runtime_revision: 0,
            result: Ok(statuses),
            runtime: Err("not requested in this stale-poll test".to_string()),
            permissions: Ok(Vec::new()),
            questions: Ok(Vec::new()),
        })
        .unwrap();

    pane.drain_background_updates();

    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert!(!pane.runtime_status_requests.contains_key("root"));
}

#[test]
fn activation_snapshot_rehydrates_missed_permission() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.runtime_status_requests.insert("root".to_string(), 1);
    let mut permission = child_permission("perm-missed");
    permission.session_id = "root".to_string();
    permission.parent_session_id = None;
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
            session_id: "root".to_string(),
            request_generation: 1,
            runtime_revision: 0,
            result: Ok(HashMap::new()),
            runtime: Err("runtime omitted".to_string()),
            permissions: Ok(vec![permission]),
            questions: Ok(Vec::new()),
        })
        .unwrap();

    pane.drain_background_updates();

    assert_eq!(
        pane.pending_permission()
            .map(|permission| permission.id.as_str()),
        Some("perm-missed")
    );
}

#[test]
fn activation_snapshot_rehydrates_missed_question() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.runtime_status_requests.insert("root".to_string(), 1);
    let question =
        neoism_ui::panels::agent_pane::question_policy::question_request_from_event(
            &serde_json::json!({
                "id": "question-missed",
                "sessionId": "root",
                "questions": [{
                    "question": "Continue?",
                    "options": [{ "label": "Yes", "description": "Continue now" }]
                }]
            }),
        );
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
            session_id: "root".to_string(),
            request_generation: 1,
            runtime_revision: 0,
            result: Ok(HashMap::new()),
            runtime: Err("runtime omitted".to_string()),
            permissions: Ok(Vec::new()),
            questions: Ok(vec![question]),
        })
        .unwrap();

    pane.drain_background_updates();

    assert_eq!(
        pane.pending_question().map(|question| question.id.as_str()),
        Some("question-missed")
    );
}

#[test]
fn activation_snapshot_removes_answered_permission() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    let mut permission = child_permission("perm-answered");
    permission.session_id = "root".to_string();
    permission.parent_session_id = None;
    pane.enqueue_pending_permission(permission);
    pane.runtime_status_requests.insert("root".to_string(), 1);
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
            session_id: "root".to_string(),
            request_generation: 1,
            runtime_revision: 0,
            result: Ok(HashMap::new()),
            runtime: Err("runtime omitted".to_string()),
            permissions: Ok(Vec::new()),
            questions: Ok(Vec::new()),
        })
        .unwrap();

    pane.drain_background_updates();

    assert!(pane.pending_permission().is_none());
}

#[test]
fn permission_reply_event_blocks_stale_snapshot_resurrection() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    let mut permission = child_permission("perm-race");
    permission.session_id = "root".to_string();
    permission.parent_session_id = None;
    pane.enqueue_pending_permission(permission.clone());
    pane.runtime_status_requests.insert("root".to_string(), 1);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::PermissionReplied {
            request_id: "perm-race".to_string(),
            session_id: Some("root".to_string()),
        }],
    ));
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
            session_id: "root".to_string(),
            request_generation: 1,
            runtime_revision: 0,
            result: Ok(HashMap::new()),
            runtime: Err("runtime omitted".to_string()),
            permissions: Ok(vec![permission]),
            questions: Ok(Vec::new()),
        })
        .unwrap();

    pane.drain_server_updates();

    assert!(pane.pending_permission().is_none());
    assert!(!pane.runtime_status_requests.contains_key("root"));
}

#[test]
fn omitted_child_from_live_run_snapshot_remains_outstanding() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "Map frontend", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );

    pane.apply_runtime_status_for_session("parent", &HashMap::new());

    assert_eq!(pane.active_subagent_count(), 1);
    assert!(pane.subagent_waiting_started_at.is_some());
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert_eq!(
        pane.side_panel
            .branch_activity("child")
            .map(|activity| activity.status),
        Some(BranchStatus::Active)
    );
}

#[test]
fn terminal_child_stragglers_update_text_without_resurrecting_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Completed, None);
    pane.session_cache
        .insert("child".to_string(), CachedAgentSession::live_only());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::ChildPartDelta {
            session_id: "child".to_string(),
            message_id: Some("message".to_string()),
            part_id: Some("answer".to_string()),
            kind: Some("text".to_string()),
            delta: "late suffix".to_string(),
        }],
    ));

    pane.drain_server_updates();

    let child = &pane.session_cache["child"];
    assert_eq!(child.messages[0].text, "late suffix");
    assert_eq!(
        child.runtime.streaming_state,
        NeoismAgentStreamingState::Idle
    );
}

#[test]
fn reconnect_child_snapshot_repairs_inactive_transcript() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.messages = vec![NeoismAgentMessage::assistant("old").with_id("answer")];
    pane.session_cache.insert("child".to_string(), child);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::ChildMessages {
            session_id: "child".to_string(),
            messages: vec![NeoismAgentMessage::assistant("complete").with_id("answer")],
            oldest_cursor: Some("cursor".to_string()),
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(pane.session_cache["child"].messages[0].text, "complete");
    assert_eq!(
        pane.session_cache["child"]
            .timeline_history
            .oldest_loaded_cursor
            .as_deref(),
        Some("cursor")
    );
}

#[test]
fn child_compaction_runtime_is_routed_to_the_child_cache() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.session_cache
        .insert("child".to_string(), CachedAgentSession::live_only());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::CompactionStarted {
            session_id: "child".to_string(),
            id: "compact".to_string(),
            reason: "auto".to_string(),
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.session_cache["child"].runtime.streaming_state,
        NeoismAgentStreamingState::Compacting
    );
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Idle);
}

#[test]
fn inactive_prompt_failure_settles_the_origin_session() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::user("ship it")];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child".to_string());
    pane.switch_session("child".to_string());
    pane.prompt_dispatch_in_flight = true;
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatchFailed {
            server: pane.server.clone(),
            queued: false,
            request: None,
            origin_session_id: Some("root".to_string()),
            origin_draft_id: 0,
            error: "offline".to_string(),
        })
        .unwrap();

    pane.drain_background_updates();

    let root = &pane.session_cache["root"];
    assert_eq!(
        root.runtime.streaming_state,
        NeoismAgentStreamingState::Idle
    );
    assert!(root.messages.iter().any(|message| {
        message.kind == NeoismAgentMessageKind::System && message.text == "offline"
    }));
}

#[test]
fn inactive_prompt_success_reconciles_server_echo_without_duplicate_user() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::user("ship it")];
    pane.remember_pending_user_prompt("ship it");
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.event_stream = Some(AgentSessionEventStream::connected_for_test("root"));
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child".to_string());
    pane.switch_session("child".to_string());
    pane.prompt_dispatch_in_flight = true;
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatched {
            server: pane.server.clone(),
            session_title: None,
            origin_session_id: Some("root".to_string()),
            origin_draft_id: 0,
            session_id: "root".to_string(),
            transcript_echo: Some("ship it".to_string()),
            event_stream: None,
        })
        .unwrap();
    pane.drain_background_updates();
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::Messages {
            messages: vec![NeoismAgentMessage::user("ship it").with_id("server-user")],
            oldest_cursor: None,
        }],
    ));

    pane.drain_server_updates();

    let root = &pane.session_cache["root"];
    assert_eq!(
        root.messages
            .iter()
            .filter(|message| {
                message.kind == NeoismAgentMessageKind::User && message.text == "ship it"
            })
            .count(),
        1
    );
    assert!(root.pending_user_prompts.is_empty());
}

#[test]
fn selecting_current_session_cancels_pending_cold_switch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.pending_session_switch = Some("cold-child".to_string());

    pane.switch_session("root".to_string());

    assert!(pane.pending_session_switch.is_none());
}

#[test]
fn proactive_preload_queue_is_concurrency_and_memory_bounded() {
    let mut pane = NeoismAgentPane::default();
    pane.session_preloads_in_flight.insert("busy-1".to_string());
    pane.session_preloads_in_flight.insert("busy-2".to_string());
    for index in 0..20 {
        pane.ensure_session_preloaded(format!("child-{index}"), false);
    }

    assert_eq!(pane.session_preloads_in_flight.len(), 2);
    assert_eq!(pane.session_preload_queue.len(), 10);

    pane.pending_session_switch = Some("child-19".to_string());
    pane.ensure_session_preloaded("child-19".to_string(), false);
    assert!(!pane.session_preloads_in_flight.contains("child-19"));
    assert_eq!(pane.session_preloads_in_flight.len(), 2);
    assert_eq!(pane.session_preload_queue.front().unwrap().0, "child-19");
    for index in 20..100 {
        pane.pending_session_switch = Some(format!("child-{index}"));
        pane.ensure_session_preloaded(format!("child-{index}"), false);
    }
    assert_eq!(pane.session_preloads_in_flight.len(), 2);
    assert_eq!(pane.session_preload_queue.len(), 1);
    assert_eq!(pane.session_preload_queue.front().unwrap().0, "child-99");
}

#[test]
fn hidden_pane_live_drain_leaves_blocking_outbound_work_queued() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.abort_session();

    pane.drain_live_session_updates();

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::AbortSession]
    );
}

#[test]
fn compact_session_queues_outbound_command_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());

    pane.execute_slash_text("/compact");

    assert!(matches!(pane.drain_pending_outbound().as_slice(),
        [OutboundAgentCommand::CompactSessionRequest { session_id, .. }] if session_id == "sess-1"
    ));
    assert!(pane.messages.is_empty());
    // Idle /compact shows Compacting the instant it is submitted, without
    // the mid-run "Compaction queued" line.
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Compacting);
    assert!(pane.pending_compact.is_none());
    assert_eq!(pane.queued_prompt_count(), 0);
    // A second /compact while it is pending does not double-submit.
    pane.execute_slash_text("/compact");
    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn idle_compact_failure_reverts_optimistic_compacting() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.execute_slash_text("/compact");
    let (_, request_id) = pane.optimistic_compact.clone().unwrap();
    pane.compact_request_failed("sess-1", Some(&request_id), "rejected".into());
    assert!(pane.optimistic_compact.is_none());
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Idle);
}

#[test]
fn idle_compact_hands_off_to_server_lifecycle() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.execute_slash_text("/compact");
    let (_, request_id) = pane.optimistic_compact.clone().unwrap();
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::CompactionStarted {
            session_id: "root".into(),
            id: "compact".into(),
            reason: "manual".into(),
        }],
    ));
    pane.drain_server_updates();
    assert!(pane.optimistic_compact.is_none());
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Compacting);
    // A late failure for the confirmed request must not stop the real run.
    pane.compact_request_failed("root", Some(&request_id), "late".into());
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Compacting);
}

#[test]
fn submit_prompt_captures_origin_before_any_frame_drain() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true; // hold the worker to inspect admission
    pane.session_id = Some("origin".into());
    pane.insert_text("ship it");
    assert!(pane.submit());
    let request = pane.pending_prompt_dispatches.front().unwrap();
    assert_eq!(request.origin_session_id.as_deref(), Some("origin"));
    assert_eq!(request.parts[0]["text"], "ship it");
    assert_eq!(request.message_id, pane.messages[0].id);
    assert_eq!(request.agent.as_deref(), Some(DEFAULT_AGENT));
    assert_eq!(request.model, "test/model");
    assert!(request.thinking.is_none());
    assert!(request.transcript_echo.is_some());
    assert!(pane.drain_pending_outbound().is_empty());
    pane.session_id = Some("different".into());
    pane.create_new_session();
    assert_eq!(
        pane.pending_prompt_dispatches
            .front()
            .unwrap()
            .origin_session_id
            .as_deref(),
        Some("origin")
    );
}

#[test]
fn prompt_dispatch_is_off_thread_and_serialized() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.server = "invalid://agent-server".to_string();
    pane.session_id = Some("sess-1".to_string());

    pane.insert_text("first");
    assert!(pane.submit());
    assert!(!pane.drain_outbound_commands());
    assert!(pane.prompt_dispatch_in_flight);
    assert!(pane.pending_prompt_dispatches.is_empty());

    // A second send must wait behind the first worker instead of issuing a
    // concurrent POST that could overtake it at the backend.
    pane.insert_text("second");
    assert!(pane.submit());
    assert!(!pane.drain_outbound_commands());
    assert!(pane.prompt_dispatch_in_flight);
    assert_eq!(pane.pending_prompt_dispatches.len(), 1);
}

#[test]
fn completed_prompt_does_not_attach_to_a_replaced_draft() {
    let mut pane = NeoismAgentPane::default();
    let original_draft_id = pane.prompt_draft_id;
    pane.prompt_dispatch_in_flight = true;

    pane.create_new_session();
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatched {
            server: pane.server.clone(),
            session_title: None,
            origin_session_id: None,
            origin_draft_id: original_draft_id,
            session_id: "old-draft-session".to_string(),
            transcript_echo: Some("old prompt".to_string()),
            event_stream: None,
        })
        .unwrap();

    assert!(pane.drain_background_updates());
    assert_eq!(pane.session_id, None);
    assert!(pane.pending_user_prompts.is_empty());
}

#[test]
fn submit_prompt_with_session_is_admitted_without_ensure_session() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true;
    pane.session_id = Some("sess-1".to_string());
    pane.insert_text("continue");
    assert!(pane.submit());
    assert!(pane.drain_pending_outbound().is_empty());
    let request = pane.pending_prompt_dispatches.front().unwrap();
    assert_eq!(request.origin_session_id.as_deref(), Some("sess-1"));
    assert_eq!(request.parts[0]["text"], "continue");
    assert_eq!(request.transcript_echo.as_deref(), Some("continue"));
}

#[test]
fn setting_goal_starts_an_agent_turn_with_the_goal_text() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true;
    pane.session_id = Some("sess-1".to_string());
    pane.start_goal_prompt("ship the durable goal".to_string());
    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].text, "ship the durable goal");
    assert!(pane.is_streaming());
    let request = pane.pending_prompt_dispatches.front().unwrap();
    assert_eq!(request.parts[0]["text"], "ship the durable goal");
    assert!(request.transcript_echo.is_some());
}

#[test]
fn setting_goal_during_a_run_queues_its_agent_turn() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true;
    pane.session_id = Some("sess-1".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.start_goal_prompt("next durable goal".to_string());
    assert!(pane.messages.is_empty());
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(
        pane.queued_prompt_preview.as_deref(),
        Some("next durable goal")
    );
    let request = pane.pending_prompt_dispatches.front().unwrap();
    assert_eq!(request.parts[0]["text"], "next durable goal");
    assert!(request.transcript_echo.is_none());
}

#[test]
fn submit_pasted_text_expands_outbound_but_keeps_transcript_token() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true;
    pane.session_id = Some("sess-1".to_string());
    pane.insert_paste("first line\nsecond line");
    assert!(pane.submit());
    let request = pane.pending_prompt_dispatches.front().unwrap();
    assert_eq!(request.parts[0]["text"], "first line\nsecond line");
    assert_eq!(
        request.transcript_echo.as_deref(),
        Some("[pasted 2 lines]")
    );
    assert_eq!(pane.messages[0].text, "[pasted 2 lines]");
}

#[test]
fn connect_secret_paste_fills_the_secret_field_not_the_composer() {
    let mut pane = NeoismAgentPane::default();
    pane.picker = Some(NeoismAgentPicker::new(
        NeoismAgentPickerKind::ConnectSecret,
        "Enter API key",
        Vec::new(),
        0,
    ));
    let key = format!("sk-test-{}", "a".repeat(512));

    pane.insert_paste(&key);

    let picker = pane.picker().expect("secret picker remains open");
    assert_eq!(picker.query, key);
    assert!(pane.input().is_empty(), "paste must not enter the composer");
}

#[test]
fn pending_question_paste_fills_the_typed_answer_not_the_composer() {
    let mut pane = NeoismAgentPane::default();
    let question =
        neoism_ui::panels::agent_pane::question_policy::question_request_from_event(
            &serde_json::json!({
                "id": "question-email",
                "sessionId": "session-1",
                "questions": [{
                    "question": "Who should receive the recording?",
                    "options": []
                }]
            }),
        );
    pane.enqueue_pending_question(question);

    pane.insert_paste("dusty@elevatedintx.com");

    assert_eq!(
        pane.pending_question()
            .map(|question| question.typed.as_str()),
        Some("dusty@elevatedintx.com")
    );
    assert!(pane.input().is_empty(), "paste must not enter the composer");
}

#[test]
fn submit_prompt_while_streaming_queues_bottom_preview_without_transcript_echo() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true;
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
    let request = pane.pending_prompt_dispatches.front().unwrap();
    assert_eq!(request.parts[0]["text"], "queued turn");
    assert!(request.transcript_echo.is_none());
}

#[test]
fn server_expanded_user_part_merges_into_pasted_token_echo() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true;
    pane.insert_paste("first line\nsecond line");
    assert!(pane.submit());
    assert_eq!(pane.messages[0].text, "[pasted 2 lines]");

    // The server streams the user part back EXPANDED, under the same client
    // message id the prompt was sent with (the message-id idempotence
    // contract round-trips it).
    let mut server_part = NeoismAgentMessage::user("first line\nsecond line");
    server_part.id = pane.messages[0].id.clone();
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
    assert!(
        users[0].id.starts_with("msg_"),
        "the merged bubble keeps the round-tripped client message id"
    );
}

#[test]
fn streamed_first_prompt_image_and_text_merge_in_either_order() {
    for image_first in [false, true] {
        let mut pane = NeoismAgentPane::default();
        let image = neoism_ui::panels::agent_pane::state::NeoismAgentImage {
            filename: "clipboard.png".to_string(),
            url: "data:image/png;base64,AA==".into(),
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
fn history_refresh_compacts_expanded_pasted_user_text() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.prompt_dispatch_in_flight = true;
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
fn queued_steer_echo_follows_streamed_output_despite_older_client_id() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![NeoismAgentMessage::user("look around").with_id("msg_0100")];
    pane.remember_live_part_parent("commentary", Some("msg_0300"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("I'll look through the architecture")
            .with_id("commentary"),
    );

    // Both steers were sent before this assistant step was created, but
    // admitted only after its commentary had already appeared in the GUI.
    for (id, text) in [("msg_0200", "not a subagent"), ("msg_0201", "you do it")] {
        pane.upsert_part_message(NeoismAgentMessage::user(text).with_id(id));
        pane.insert_dequeued_user_prompt(text.into(), Some(id.into()), None);
    }
    pane.remember_live_part_parent("thinking", Some("msg_0400"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Inspecting the project myself").with_id("thinking"),
    );
    assert_eq!(
        pane.messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["msg_0100", "commentary", "msg_0200", "msg_0201", "thinking"],
    );

    // Updating an earlier streamed block must not regroup those user turns.
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("I'll look through the architecture and core paths")
            .with_id("commentary"),
    );
    assert_eq!(pane.messages[1].id, "commentary");
    assert_eq!(pane.messages[2].id, "msg_0200");
    assert_eq!(pane.messages[3].id, "msg_0201");
}

#[test]
fn queued_steer_dequeue_before_echo_preserves_live_and_snapshot_order() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![NeoismAgentMessage::user("look around").with_id("msg_0100")];
    pane.remember_live_part_parent("commentary", Some("msg_0300"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("Starting the review").with_id("commentary"),
    );
    pane.queued_prompt_count = 2;
    for (id, text) in [("msg_0200", "not a subagent"), ("msg_0201", "you do it")] {
        pane.insert_dequeued_user_prompt(text.into(), Some(id.into()), Some("Parker".into()));
        let mut echo = NeoismAgentMessage::user(text).with_id(id);
        echo.author = Some("Parker".into());
        pane.upsert_part_message(echo);
    }
    assert_eq!(pane.queued_prompt_count, 0);
    let expected = ["msg_0100", "commentary", "msg_0200", "msg_0201"];
    assert_eq!(pane.messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), expected);
    let refreshed = pane.preserve_streamed_response_text(pane.messages.clone());
    assert_eq!(refreshed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), expected);
    assert_eq!(refreshed.len(), 4);
    assert_eq!(refreshed[2].author.as_deref(), Some("Parker"));
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

    assert!(pane.insert_dequeued_user_prompt(
        "queued turn".to_string(),
        Some("msg-queued-1".to_string()),
        Some("Parker".to_string()),
    ));
    assert!(!pane.insert_dequeued_user_prompt(
        "queued turn".to_string(),
        Some("msg-queued-1".to_string()),
        Some("Parker".to_string()),
    ));

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
    assert_eq!(pane.messages.last().unwrap().id, "msg-queued-1");
    assert_eq!(
        pane.messages.last().unwrap().author.as_deref(),
        Some("Parker")
    );
}

#[test]
fn dequeued_prompt_and_server_echo_merge_by_message_id_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    assert!(pane.insert_dequeued_user_prompt(
        "[pasted 2 lines]".to_string(),
        Some("msg-queued-1".to_string()),
        Some("Parker".to_string()),
    ));
    let mut server_echo = NeoismAgentMessage::user("first line\nsecond line");
    server_echo.id = "msg-queued-1".to_string();
    server_echo.author = Some("Parker".to_string());

    pane.upsert_part_message(server_echo);

    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].id, "msg-queued-1");
    assert_eq!(pane.messages[0].author.as_deref(), Some("Parker"));
}

#[test]
fn unknown_slash_command_queues_session_and_slash_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);

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
    assert!(matches!(pane.drain_pending_outbound().as_slice(),
        [OutboundAgentCommand::CompactSessionRequest { session_id, .. }] if session_id == "sess-1"
    ));
}

#[test]
fn model_change_queues_context_limit_refresh_for_runtime() {
    let mut pane = NeoismAgentPane::default();

    // Account discovery now runs off-thread; this test covers the committed
    // model transition after reconciliation, without requiring a live server.
    pane.apply_model_with_connection("claude-test".to_string(), None);

    // A model change persists the choice into the unified config AND refreshes
    // the context limit for the new model.
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![
            OutboundAgentCommand::PersistConfigChoice {
                model: Some("claude-test".to_string()),
                thinking: None,
            },
            OutboundAgentCommand::RefreshModelContextLimit,
        ]
    );
}

#[test]
fn zero_token_compaction_usage_does_not_reset_context_meter() {
    let mut pane = NeoismAgentPane::default();
    let mut completed = NeoismAgentMessage::assistant("done");
    completed.usage = Some(NeoismAgentUsage {
        input: 18_000,
        output: 500,
        reasoning: 250,
        cache_read: 31_250,
        cache_write: 0,
        total: 50_000,
        cost_micros: 0,
        context_limit: Some(400_000),
    });
    let mut compacted = NeoismAgentMessage::compaction("summary", "auto");
    compacted.usage = Some(NeoismAgentUsage {
        input: 0,
        output: 0,
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        total: 0,
        cost_micros: 0,
        context_limit: Some(400_000),
    });
    pane.messages.extend([completed, compacted]);

    assert_eq!(pane.latest_usage().map(|usage| usage.total), Some(50_000));
}

#[test]
fn agent_model_and_thinking_changes_preserve_composer_draft() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("keep this question");
    let draft = pane.input().to_string();

    pane.apply_model("openai/gpt-test".to_string());
    assert_eq!(pane.input(), draft);

    pane.apply_thinking("high".to_string());
    assert_eq!(pane.input(), draft);

    pane.apply_agent("plan".to_string());
    assert_eq!(pane.input(), draft);
}

#[test]
fn tab_mode_switch_rearms_agent_chip_transition() {
    let mut pane = NeoismAgentPane::default();
    assert_eq!(pane.agent_label(), "Build");
    assert_eq!(pane.agent_label_changed_elapsed_ms(), None);

    pane.toggle_mode();

    assert_eq!(pane.agent_label(), "Plan");
    assert!(pane.agent_label_changed_elapsed_ms().is_some());
    // The status-chip click wave takes priority while both animations run.
    assert_eq!(pane.animation_reason(), Some("status_chip_activation"));
    let (chip, activated_at) = pane.status_chip_activated.as_mut().unwrap();
    assert_eq!(*chip, 0);
    *activated_at = Instant::now() - Duration::from_millis(280);
    assert_eq!(pane.animation_reason(), Some("agent_label_transition"));
}

#[test]
fn with_directory_queues_config_defaults_for_runtime() {
    let mut pane = NeoismAgentPane::with_directory(Some("/tmp/project".to_string()));

    assert!(matches!(
        pane.config_chip_transition(),
        Some(neoism_ui::panels::agent_pane::state::ConfigChipTransition::Loading { .. })
    ));
    assert_eq!(pane.animation_reason(), Some("config_chip_hydration"));
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::ApplyConfigDefaults]
    );

    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::ConfigDefaultsFailed(pane.config_defaults_target()))
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.config_chip_transition(), None);
}

#[test]
fn switching_to_joined_agent_server_reloads_that_hosts_defaults() {
    let mut pane = NeoismAgentPane::default();
    seed_prompt_catalog(&mut pane);
    pane.directory = Some("/tmp/project".to_string());
    pane.model = "local/model".to_string();
    pane.thinking = Some("high".to_string());

    pane.switch_server("http://peer:7981/agent/workspaces/ws/".to_string());

    assert_eq!(pane.server, "http://peer:7981/agent/workspaces/ws");
    assert!(pane.provider_readiness.is_none());
    assert_eq!(pane.directory.as_deref(), Some("/tmp/project"));
    assert!(pane.model.is_empty());
    assert_eq!(pane.thinking, None);
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::ApplyConfigDefaults]
    );
}

#[test]
fn explicit_hosting_handoff_preserves_selection_but_not_local_cached_history() {
    let root = std::env::temp_dir()
        .join(format!("neoism-host-selection-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut local =
        NeoismAgentPane::with_directory(Some(root.to_string_lossy().into_owned()));
    local.session_id = Some("ses_host_continuity".into());
    local.remember_hosted_selection(&root, 9898);
    // Server switching must not require the guest filesystem to resolve the
    // host's path. The remembered selection is sufficient even without it.
    std::fs::remove_dir_all(&root).unwrap();
    let mut joined =
        NeoismAgentPane::with_directory(Some(root.to_string_lossy().into_owned()));
    joined.switch_server("http://127.0.0.1:9898/agent/workspaces/hosted".into());
    assert!(joined.pending_session_switch.is_none());
    assert!(joined.messages.is_empty());
    assert_eq!(joined.session_id.as_deref(), Some("ses_host_continuity"));
    let mut unrelated =
        NeoismAgentPane::with_directory(Some(root.to_string_lossy().into_owned()));
    unrelated.switch_server("http://127.0.0.1:9899/agent/workspaces/other".into());
    assert!(unrelated.pending_session_switch.is_none());
    let _ = std::fs::remove_dir_all(root);
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
fn child_hydration_merges_snapshot_without_losing_streamed_prefix() {
    let snapshot = vec![
        NeoismAgentMessage::user("inspect it").with_id("msg-user"),
        NeoismAgentMessage::assistant("").with_id("part-answer"),
    ];
    let live =
        vec![NeoismAgentMessage::assistant("the streamed prefix").with_id("part-answer")];

    let merged = merge_session_snapshot(snapshot, live, false);

    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1].text, "the streamed prefix");
}

#[test]
fn stale_pending_snapshot_cannot_regress_completed_edit() {
    let completed = NeoismAgentMessage::tool(
        "Edit(src/lib.rs)",
        "final diff",
        "completed",
        "edit",
        NeoismAgentOutputKind::Text,
        "rust",
        Vec::new(),
    )
    .with_id("edit-part");
    let pending = NeoismAgentMessage::tool(
        "Edit(src/lib.rs)",
        "partial diff",
        "pending",
        "edit",
        NeoismAgentOutputKind::Text,
        "rust",
        Vec::new(),
    )
    .with_id("edit-part");

    let merged = merge_session_snapshot(vec![pending], vec![completed], false);
    assert_eq!(merged[0].status, "completed");
    assert_eq!(merged[0].text, "final diff");
}

#[test]
fn disjoint_latest_page_does_not_put_old_cached_edits_after_latest_answer() {
    let cached = vec![
        NeoismAgentMessage::user("old task").with_id("old-user"),
        NeoismAgentMessage::tool(
            "Edit(old.rs)",
            "old edit",
            "completed",
            "edit",
            NeoismAgentOutputKind::Text,
            "rust",
            Vec::new(),
        )
        .with_id("old-edit"),
    ];
    let latest = vec![
        NeoismAgentMessage::user("new task").with_id("new-user"),
        NeoismAgentMessage::assistant("latest answer").with_id("new-answer"),
    ];

    let merged = merge_session_snapshot(latest.clone(), cached.clone(), true);
    assert_eq!(merged[0].id, "old-user");
    assert_eq!(merged[1].id, "old-edit");
    assert_eq!(merged.last().unwrap().id, "new-answer");

    let live_only = merge_session_snapshot(latest, cached, false);
    assert_eq!(live_only.last().unwrap().id, "old-edit");
}

#[test]
fn returning_to_warm_cache_refreshes_history_without_delaying_display() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "http://127.0.0.1:0".to_string();
    pane.session_id = Some("other".to_string());
    let mut cached = CachedAgentSession::live_only();
    cached.hydrated = true;
    cached.messages = vec![NeoismAgentMessage::assistant("old reply")];
    pane.session_cache.insert("returned".to_string(), cached);

    pane.switch_session("returned".to_string());

    assert_eq!(pane.session_id.as_deref(), Some("returned"));
    assert_eq!(pane.messages[0].text, "old reply");
    assert!(pane.session_preloads_in_flight.contains("returned"));
    assert!(pane.timeline_follow_bottom);
}

#[test]
fn imported_history_preload_discloses_text_only_without_changing_title_or_source() {
    use neoism_ui::panels::agent_pane::api_mapping::session_state_from_json;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("imported".into());
    let state = session_state_from_json(&serde_json::json!({
        "id":"imported", "title":"Original provider title", "extra":{"externalAgent":{
            "provider":"codex", "historyState":"text_only", "historyToolEvents":2,
            "historyIncompleteContent":true
        }}
    }));
    pane.background_tx.send(NeoismAgentBackgroundUpdate::SessionMetadataLoaded {
        session_id: "imported".into(), result: Ok(state),
    }).unwrap();
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::SessionPreloaded {
            session_id: "imported".into(),
            raw_count: 1,
            requested_limit: 32,
            messages: vec![NeoismAgentMessage::assistant("actual imported text")],
            oldest_cursor: None,
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.session_title(), Some("Original provider title"));
    assert_eq!(pane.conversation_source(), ConversationSource::Codex);
    assert_eq!(
        pane.messages.len(),
        2,
        "one imported reply plus the hidden authoritative plan marker"
    );
    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(pane.messages[0].text, "actual imported text");
    assert_eq!(pane.messages[1].id, "todos-snapshot");
    assert_eq!(pane.messages[1].kind, NeoismAgentMessageKind::System);
    assert_eq!(pane.messages[1].tool, "todowrite");
    assert!(pane.messages[1].todos.is_empty());
    assert_eq!(pane.imported_history().unwrap().tool_events, 2);
    pane.cache_current_session(false);
    pane.session_id = Some("fresh".into());
    assert!(
        pane.imported_history().is_none(),
        "never leak another session's note"
    );
    assert_eq!(
        pane.session_sources.get("imported"),
        Some(&ConversationSource::Codex)
    );
    pane.new_chat_source = ConversationSource::Neoism;
    pane.session_cache
        .get_mut("imported")
        .unwrap()
        .model_context_limit = Some(1024);
    pane.activate_cached_session("imported");
    assert_eq!(pane.conversation_source(), ConversationSource::Codex);
    assert_eq!(pane.session_title(), Some("Original provider title"));
    assert!(pane.imported_history().is_some());
}

#[test]
fn preloaded_disjoint_page_keeps_newest_reply_after_parked_history() {
    let mut pane = NeoismAgentPane::default();
    let mut cached = CachedAgentSession::live_only();
    cached.hydrated = true;
    cached.messages = vec![NeoismAgentMessage::assistant("old edit").with_id("old-edit")];
    pane.session_cache.insert("returned".to_string(), cached);
    pane.background_tx
        .send(NeoismAgentBackgroundUpdate::SessionPreloaded {
            session_id: "returned".to_string(),
            raw_count: 2,
            requested_limit: 32,
            messages: vec![
                NeoismAgentMessage::user("new task").with_id("new-user"),
                NeoismAgentMessage::assistant("latest reply").with_id("new-answer"),
            ],
            oldest_cursor: None,
        })
        .unwrap();

    pane.drain_background_updates();

    let messages = &pane.session_cache.get("returned").unwrap().messages;
    assert_eq!(messages[0].id, "old-edit");
    assert_eq!(messages.last().unwrap().id, "new-answer");
}

#[test]
fn partial_snapshot_keeps_cached_history_and_subagent_notice_in_order() {
    let notice = NeoismAgentMessage::system(
        "Subagent",
        "Subagent finished.\ntask_id: ses-child\nstatus: completed",
    )
    .with_id("msg_subtask_completion_ses-child");
    let live = vec![
        NeoismAgentMessage::user("oldest").with_id("u-1"),
        NeoismAgentMessage::assistant("before task").with_id("a-1"),
        notice,
        NeoismAgentMessage::user("after task").with_id("u-2"),
        NeoismAgentMessage::assistant("newest").with_id("a-2"),
    ];
    let snapshot = vec![
        NeoismAgentMessage::assistant("before task").with_id("a-1"),
        NeoismAgentMessage::user("after task").with_id("u-2"),
        NeoismAgentMessage::assistant("newest").with_id("a-2"),
    ];

    let merged = merge_session_snapshot(snapshot, live, false);
    let ids = merged
        .iter()
        .map(|message| message.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        vec![
            "u-1",
            "a-1",
            "msg_subtask_completion_ses-child",
            "u-2",
            "a-2"
        ]
    );
}

#[test]
fn runtime_completion_rehydrate_does_not_append_cached_part_at_bottom() {
    let live = crate::neoism::agent::api::part_block(&json!({
        "id": "prt-background-done",
        "messageID": "msg_background_completion_job_123",
        "type": "text",
        "role": "user",
        "text": "Background shell task finished.\njob_id: job_123\nstatus: completed"
    }))
    .expect("live runtime completion");
    // The persisted runtime prompt maps to the SAME durable card id as the
    // live broadcast (`background-task-{job}`), so rehydration merges them
    // into one row instead of appending a duplicate at the bottom.
    let snapshot = neoism_ui::panels::agent_pane::api_mapping::message_blocks(&json!({
        "info": {
            "id": "msg_background_completion_job_123",
            "role": "user",
            "sessionId": "ses_test",
            "time": { "created": 0 },
            "agent": "build",
            "model": { "providerId": "openai", "modelId": "gpt-5.6-sol" }
        },
        "parts": [{
            "id": "prt-background-done",
            "type": "text",
            "text": "Background shell task finished.\njob_id: job_123\nstatus: completed"
        }]
    }))
    .into_iter()
    .map(NeoismAgentMessage::from)
    .collect::<Vec<_>>();

    let merged = merge_session_snapshot(snapshot, vec![live], false);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "background-task-job_123");
    assert_eq!(merged[0].kind, NeoismAgentMessageKind::Tool);
    assert_eq!(merged[0].tool, "background_task_result");
}

#[test]
fn child_live_cache_accumulates_deltas_before_navigation() {
    let mut messages = Vec::new();

    apply_cached_part_delta(
        &mut messages,
        Some("part-answer"),
        Some("text"),
        "the prefix",
    );
    apply_cached_part_delta(
        &mut messages,
        Some("part-answer"),
        Some("text"),
        " and suffix",
    );

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "part-answer");
    assert_eq!(messages[0].text, "the prefix and suffix");
}

#[test]
fn disconnected_idle_stream_resubscribes_and_reconciles() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session".to_string());
    pane.server = "http://127.0.0.1:0".to_string();
    let mut stream = AgentSessionEventStream::connected_for_test("session");
    stream.disconnect_for_test();
    pane.event_stream = Some(stream);

    pane.drain_server_updates();

    assert_eq!(pane.event_stream.as_ref().unwrap().session_id(), "session");
    assert!(!pane.event_stream.as_ref().unwrap().is_disconnected());
    assert!(pane.last_stream_resubscribe_at.is_some());
}

#[test]
fn returning_to_cached_session_opens_at_latest_reply() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("old".to_string());
    pane.messages = vec![NeoismAgentMessage::assistant("old reply")];
    pane.timeline_scroll_px = 240.0;
    pane.timeline_follow_bottom = false;
    pane.cache_current_session(false);
    pane.session_id = Some("other".to_string());
    pane.messages = vec![NeoismAgentMessage::user("other")];

    pane.activate_cached_session("old");

    assert_eq!(pane.messages.last().unwrap().text, "old reply");
    assert_eq!(pane.timeline_scroll_px, 0.0);
    assert!(pane.timeline_follow_bottom);
}

#[test]
fn inactive_session_cache_is_bounded() {
    let mut pane = NeoismAgentPane::default();
    for index in 0..45 {
        pane.session_cache
            .insert(format!("session-{index}"), CachedAgentSession::live_only());
    }

    pane.trim_session_cache();

    assert_eq!(pane.session_cache.len(), 40);
}

#[test]
fn optimistic_prompt_echo_does_not_archive_streaming_edit_cards() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session".to_string());
    let mut edit = NeoismAgentMessage::tool(
        "Edit(src/lib.rs)",
        "changed file",
        "completed",
        "edit",
        NeoismAgentOutputKind::Text,
        "rust",
        Vec::new(),
    )
    .with_id("edit-1");
    edit.detail = r#"{"neoismToolDetail":"edit","input":{"filePath":"src/lib.rs","oldString":"old","newString":"new"},"metadata":null}"#.to_string();
    pane.messages = vec![NeoismAgentMessage::user("fix this"), edit.clone()];
    pane.timeline_live_trace_anchor = Some(String::new());
    pane.timeline_live_trace_start = Some(1);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "session",
        [AgentSessionUpdate::Messages {
            messages: vec![
                NeoismAgentMessage::user("fix this").with_id("user-1"),
                edit,
                NeoismAgentMessage::user("next task").with_id("user-2"),
            ],
            oldest_cursor: None,
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(pane.timeline_live_trace_anchor.as_deref(), Some("user-1"));
    assert_eq!(pane.timeline_live_trace_start, Some(1));
    assert!(!pane.tool_archived("edit-1"));
}

#[test]
fn trace_boundary_change_invalidates_cached_edit_layout() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("old").with_id("user-1"),
        NeoismAgentMessage::tool(
            "Edit(src/lib.rs)",
            "changed file",
            "completed",
            "edit",
            NeoismAgentOutputKind::Text,
            "rust",
            Vec::new(),
        )
        .with_id("edit-1"),
        NeoismAgentMessage::user("new").with_id("user-2"),
    ];
    pane.timeline_live_trace_start = Some(1);
    pane.timeline_live_trace_anchor = Some("user-2".to_string());
    let epoch = pane.timeline_layout_epoch;

    pane.rebase_current_turn_trace();

    assert_eq!(pane.timeline_live_trace_start, Some(3));
    assert_eq!(pane.timeline_layout_epoch, epoch.wrapping_add(1));
}

#[test]
fn transcript_refresh_keeps_live_trace_anchored_to_its_turn() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("old"),
        NeoismAgentMessage::user("latest").with_id("latest"),
        NeoismAgentMessage::reasoning("thinking").with_id("reasoning"),
        NeoismAgentMessage::assistant("tool").with_id("tool"),
    ];
    pane.timeline_live_trace_start = Some(2);
    pane.timeline_live_trace_anchor = Some("latest".to_string());

    // A refresh inserts an older answer above the anchored turn; the marker
    // must follow its turn (id anchor), not jump rows or drift to a newer
    // boundary. Turns revealed during a visit stay revealed until the
    // session is left.
    pane.messages = vec![
        NeoismAgentMessage::user("old"),
        NeoismAgentMessage::assistant("old answer").with_id("old-answer"),
        NeoismAgentMessage::user("latest").with_id("latest"),
        NeoismAgentMessage::assistant("durable answer").with_id("answer"),
    ];
    pane.rebase_current_turn_trace();

    assert_eq!(pane.timeline_live_trace_start, Some(3));
    assert_eq!(pane.messages[3].text, "durable answer");

    // A newer prompt arriving must NOT move the boundary forward: the
    // anchored turn's trace stays visible for the rest of the visit.
    pane.messages
        .push(NeoismAgentMessage::user("newer").with_id("newer"));
    pane.messages
        .push(NeoismAgentMessage::assistant("newer answer").with_id("newer-answer"));
    pane.rebase_current_turn_trace();
    assert_eq!(pane.timeline_live_trace_start, Some(3));

    // An unresolved optimistic anchor must not jump past previously live
    // rows just because a newer user prompt has arrived.
    pane.timeline_live_trace_anchor = Some(String::new());
    pane.rebase_current_turn_trace();
    assert_eq!(pane.timeline_live_trace_start, Some(3));
    assert_eq!(pane.timeline_live_trace_anchor.as_deref(), Some("latest"));
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

    // Compactions are now server-persisted (see
    // `persisted_compaction_is_only_compaction_message_source`), so the idle
    // history snapshot re-sends the marker in its original slot rather than
    // dropping it — the refresh is server-authoritative and preserves it.
    let server_messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("first answer").with_id("answer-1"),
        NeoismAgentMessage::compaction("summary", "model").with_id("compaction-1"),
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
fn completed_task_card_drops_stale_running_explanation() {
    let mut pane = NeoismAgentPane::default();
    let mut message = task_tool_message("child-1", "running");
    message.detail.push_str(
        "\n\nThe subagent is running in the background and the user can still message the main session.",
    );
    pane.messages = vec![message];

    pane.set_task_message_status("child-1", "completed");

    assert!(pane.messages[0].detail.contains("status: completed"));
    assert!(!pane.messages[0]
        .detail
        .contains("running in the background"));
    assert!(pane.messages[0]
        .detail
        .contains("The subagent is no longer running."));
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
fn streaming_compaction_retains_activity_across_status_refresh_and_deltas() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::CompactionStarted {
                session_id: "root".into(),
                id: "marker".into(),
                reason: "manual".into(),
            },
            AgentSessionUpdate::PartUpdated {
                message: NeoismAgentMessage::compaction("", "summary").with_id("summary"),
                parent_message_id: None,
            },
            AgentSessionUpdate::PartDelta {
                message_id: Some("summary".into()),
                part_id: Some("summary-text".into()),
                kind: Some("compaction".into()),
                delta: "first chunk".into(),
            },
        ],
    ));
    pane.drain_server_updates();
    assert_eq!(pane.streaming_label(), "Compacting");
    assert!(pane.is_streaming());

    // Reconciliation must recover the summary's activity, not generic Thinking.
    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    let statuses = HashMap::from([(
        "root".into(),
        super::super::api::SessionStatusSnapshot {
            kind: "busy".into(),
            ..Default::default()
        },
    )]);
    pane.apply_runtime_status_for_session("root", &statuses);
    assert_eq!(pane.streaming_label(), "Compacting");
    let started = pane.streaming_started_at;

    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::PartDelta {
            message_id: Some("summary".into()),
            part_id: Some("summary-text".into()),
            kind: Some("compaction".into()),
            delta: " second chunk".into(),
        }],
    ));
    pane.drain_server_updates();
    assert_eq!(pane.streaming_label(), "Compacting");
    assert_eq!(pane.streaming_started_at, started);
    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].text, "first chunk second chunk");

    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::CompactionEnded {
            session_id: "root".into(),
            summary: "first chunk second chunk".into(),
            kind: "model".into(),
            run_continues: None,
        }],
    ));
    pane.drain_server_updates();
    pane.apply_runtime_status_for_session("root", &HashMap::new());
    assert!(!pane.is_streaming());
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Idle);
}

#[test]
fn dequeue_snapshot_preserves_remaining_compaction_indicator() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.queued_prompt_count = 2;
    pane.queued_prompt_preview = Some("next turn".into());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::DequeuedPrompt {
                text: "next turn".into(),
                message_id: Some("queued-followup".into()),
                author: None,
            },
            AgentSessionUpdate::QueueSnapshot {
                running: None,
                action: None,
                session_id: "root".into(),
                count: Some(1),
                preview: Some("Compaction queued".into()),
            },
        ],
    ));
    pane.drain_server_updates();
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(
        pane.queued_prompt_preview.as_deref(),
        Some("Compaction queued")
    );
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
}

#[test]
fn local_compact_feedback_is_immediate_idempotent_and_reconciled() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.execute_slash_text("/compact");
    assert_eq!(pane.queued_prompt_count(), 1);
    assert_eq!(pane.queued_prompt_preview(), Some("Compaction queued"));
    assert_eq!(pane.queued_prompt_count, 0);
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    pane.execute_slash_text("/compact");
    assert_eq!(pane.queued_prompt_count(), 1);
    assert_eq!(pane.drain_pending_outbound().len(), 1);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::QueueSnapshot {
            running: None,
            action: None,
            session_id: "root".into(),
            count: Some(1),
            preview: Some("Compaction queued".into()),
        }],
    ));
    pane.drain_server_updates();
    assert!(pane.pending_compact.is_none());
    assert_eq!(pane.queued_prompt_count(), 1);
    pane.execute_slash_text("/compact");
    assert!(pane.drain_pending_outbound().is_empty());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::QueueSnapshot {
            running: None,
            action: None,
            session_id: "root".into(),
            count: Some(0),
            preview: None,
        }],
    ));
    pane.drain_server_updates();
    let _ = pane
        .background_sender()
        .send(NeoismAgentBackgroundUpdate::CompactAccepted {
            session_id: "root".into(),
        });
    pane.drain_background_updates();
    assert_eq!(pane.queued_prompt_count(), 0);
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
}

#[test]
fn local_compact_failure_and_stop_preserve_real_prompts_and_newer_request() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.queued_prompt_count = 2;
    pane.queued_prompt_preview = Some("ordinary".into());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.execute_slash_text("/compact");
    let first = pane.pending_compact.clone().unwrap();
    assert_eq!(pane.queued_prompt_count(), 3);
    assert_eq!(pane.queued_prompt_preview(), Some("ordinary"));
    pane.compact_request_failed("root", Some(&first), "rejected".into());
    assert_eq!(pane.queued_prompt_count(), 2);
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    pane.execute_slash_text("/compact");
    let second = pane.pending_compact.clone().unwrap();
    pane.compact_request_failed("root", Some(&first), "late failure".into());
    assert_eq!(pane.pending_compact.as_deref(), Some(second.as_str()));
    pane.abort_session();
    assert_eq!(pane.queued_prompt_count(), 2);
    assert_eq!(pane.queued_prompt_preview(), Some("ordinary"));
    assert!(pane.pending_compact.is_none());
}

#[test]
fn local_compact_feedback_parks_with_origin_and_clears_on_cached_failure() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("origin".into());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.execute_slash_text("/compact");
    let request_id = pane.pending_compact.clone().unwrap();
    pane.cache_current_session(false);
    pane.session_id = Some("other".into());
    assert!(pane.pending_compact.is_none());
    assert_eq!(
        pane.session_cache["origin"]
            .runtime
            .pending_compact
            .as_deref(),
        Some(request_id.as_str())
    );
    let _ = pane
        .background_sender()
        .send(NeoismAgentBackgroundUpdate::CompactFailed {
            session_id: "origin".into(),
            request_id: Some(request_id),
            error: "rejected".into(),
        });
    pane.drain_background_updates();
    assert!(pane.session_cache["origin"]
        .runtime
        .pending_compact
        .is_none());
    assert_eq!(pane.queued_prompt_count(), 0);
    assert_eq!(
        pane.session_cache["origin"].runtime.streaming_state,
        NeoismAgentStreamingState::Generating
    );
}

#[test]
fn compaction_lifecycle_and_status_snapshot_reconcile_local_feedback() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.execute_slash_text("/compact");
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::CompactionStarted {
            session_id: "root".into(),
            id: "compact".into(),
            reason: "manual".into(),
        }],
    ));
    pane.drain_server_updates();
    assert!(pane.pending_compact.is_none());
    assert_eq!(pane.queued_prompt_count(), 0);
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Compacting);
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.execute_slash_text("/compact");
    assert_eq!(pane.queued_prompt_count(), 1);
    pane.apply_runtime_status_for_session("root", &HashMap::new());
    assert!(pane.pending_compact.is_none());
    assert_eq!(pane.queued_prompt_count(), 0);
    let mut cached = CachedAgentRuntime::default();
    cached.pending_compact = Some("cached".into());
    cached.apply_queue_status(2, Some("ordinary".into()), None);
    assert!(cached.pending_compact.is_none());
    assert_eq!(cached.queued_prompt_count, 2);
}

#[test]
fn compact_admission_and_late_failure_never_stop_an_active_stream() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("current".into());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    let _ = pane
        .background_sender()
        .send(NeoismAgentBackgroundUpdate::CompactAccepted {
            session_id: "current".into(),
        });
    pane.drain_background_updates();
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert!(pane.messages.is_empty());

    let _ = pane
        .background_sender()
        .send(NeoismAgentBackgroundUpdate::CompactFailed {
            session_id: "origin".into(),
            request_id: None,
            error: "late admission failure".into(),
        });
    pane.drain_background_updates();
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert!(pane.messages.is_empty());
    assert!(pane.session_cache["origin"]
        .messages
        .iter()
        .any(|message| message.text.contains("late admission failure")));
    pane.compact_request_failed("current", None, "current failure".into());
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
}

#[test]
fn queued_compaction_and_continuation_do_not_idle_the_run() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::QueueSnapshot {
                running: None,
                action: None,
                session_id: "root".into(),
                count: None,
                preview: Some("Compaction queued".into()),
            },
            AgentSessionUpdate::QueueSnapshot {
                running: None,
                action: None,
                session_id: "inactive".into(),
                count: Some(1),
                preview: Some("Compaction queued".into()),
            },
        ],
    ));
    pane.drain_server_updates();
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(
        pane.session_cache["inactive"].runtime.queued_prompt_count,
        1
    );
    assert!(pane.messages.is_empty());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::QueueSnapshot {
                running: None,
                action: None,
                session_id: "root".into(),
                count: Some(0),
                preview: None,
            },
            AgentSessionUpdate::CompactionStarted {
                session_id: "root".into(),
                id: "compact".into(),
                reason: "manual".into(),
            },
            AgentSessionUpdate::CompactionEnded {
                session_id: "root".into(),
                summary: "summary".into(),
                kind: "model".into(),
                run_continues: Some(true),
            },
        ],
    ));
    pane.drain_server_updates();
    assert_eq!(pane.queued_prompt_count, 0);
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert!(pane.is_streaming());
}

#[test]
fn abort_preserves_ordinary_queue_until_authoritative_snapshot() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.queued_prompt_count = 2;
    pane.queued_prompt_preview = Some("ordinary queued prompt".into());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.abort_session();
    assert_eq!(pane.queued_prompt_count(), 2);
    assert_eq!(pane.queued_prompt_preview(), Some("ordinary queued prompt"));

    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::QueueSnapshot {
            running: None,
            action: None,
            session_id: "root".into(),
            count: Some(1),
            preview: Some("ordinary queued prompt".into()),
        }],
    ));
    pane.drain_server_updates();
    assert_eq!(pane.queued_prompt_count(), 1);
    assert_eq!(pane.queued_prompt_preview(), Some("ordinary queued prompt"));
}

#[test]
fn cached_compaction_tail_restores_compacting_activity() {
    let mut runtime = CachedAgentRuntime::default();
    let messages = vec![NeoismAgentMessage::compaction("partial summary", "summary")];
    runtime.refresh_streaming_from_tail(&messages);
    assert_eq!(
        runtime.streaming_state,
        NeoismAgentStreamingState::Compacting
    );
    assert!(runtime.is_streaming());
    let started = runtime.streaming_started_at;
    runtime.refresh_streaming_from_tail(&messages);
    assert_eq!(runtime.streaming_started_at, started);
    runtime.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert!(!runtime.is_streaming());
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
        Some("compaction".to_string()),
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
fn stale_history_preserves_streamed_compaction_text_and_slot() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("before").with_id("user-before"),
        NeoismAgentMessage::compaction("partial summary", "summary")
            .with_id("assistant-compaction"),
        NeoismAgentMessage::assistant("after").with_id("assistant-after"),
    ];

    let refreshed = pane.preserve_streamed_response_text(vec![
        NeoismAgentMessage::user("before").with_id("user-before"),
        NeoismAgentMessage::compaction("", "summary").with_id("assistant-compaction"),
        NeoismAgentMessage::assistant("after").with_id("assistant-after"),
    ]);

    assert_eq!(refreshed.len(), 3);
    assert_eq!(refreshed[1].id, "assistant-compaction");
    assert_eq!(refreshed[1].kind, NeoismAgentMessageKind::Compaction);
    assert_eq!(refreshed[1].text, "partial summary");
    assert_eq!(refreshed[2].id, "assistant-after");
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
fn live_compaction_reasoning_start_deltas_end_keep_summary_separate() {
    let mut pane = NeoismAgentPane::default();
    // The card uses the message id, without an entry in the part-parent map.
    pane.upsert_part_message(
        NeoismAgentMessage::compaction("", "summary").with_id("msg-summary"),
    );
    pane.remember_live_part_parent("reason-1", Some("msg-summary"));
    pane.upsert_part_message(NeoismAgentMessage::reasoning("").with_id("reason-1"));
    assert_eq!(pane.messages[0].id, "reason-1");
    assert_eq!(pane.messages[1].id, "msg-summary");

    for delta in ["Identify ", "the goal"] {
        pane.apply_part_delta(
            Some("msg-summary".into()),
            Some("reason-1".into()),
            Some("reasoning".into()),
            delta,
        );
    }
    assert_eq!(pane.messages[0].text, "Identify the goal");
    assert!(pane.messages[1].text.is_empty());
    // End/upsert must keep the same order and identity, not duplicate rows.
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Identify the goal").with_id("reason-1"),
    );
    // A second reasoning part can begin with a delta before its start/upsert.
    pane.apply_part_delta(
        Some("msg-summary".into()),
        Some("reason-2".into()),
        Some("reasoning".into()),
        "Preserve state",
    );
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Preserve state").with_id("reason-2"),
    );
    pane.apply_part_delta(
        Some("msg-summary".into()),
        Some("summary-text".into()),
        Some("compaction".into()),
        "## Goal",
    );
    pane.upsert_part_message(
        NeoismAgentMessage::compaction("## Goal", "summary").with_id("msg-summary"),
    );
    assert_eq!(
        pane.messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["reason-1", "reason-2", "msg-summary"]
    );
    assert_eq!(pane.messages[0].text, "Identify the goal");
    assert_eq!(pane.messages[1].text, "Preserve state");
    assert_eq!(pane.messages[2].text, "## Goal");
    assert_eq!(pane.messages[2].kind, NeoismAgentMessageKind::Compaction);
}

#[test]
fn live_compaction_reasoning_late_parent_repairs_only_its_card() {
    let mut pane = NeoismAgentPane::default();
    pane.upsert_part_message(
        NeoismAgentMessage::compaction("Summary", "summary").with_id("msg-summary"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Thought").with_id("reason-1"),
    );
    assert_eq!(pane.messages[0].id, "msg-summary");
    pane.remember_live_part_parent("reason-1", Some("msg-summary"));
    assert_eq!(pane.messages[0].id, "reason-1");
    assert_eq!(pane.messages[1].id, "msg-summary");
}

#[test]
fn live_compaction_reasoning_does_not_move_unrelated_groups() {
    let mut pane = NeoismAgentPane::default();
    pane.remember_live_part_parent("answer-other", Some("msg-other"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("Other answer").with_id("answer-other"),
    );
    // Even misleading part-parent metadata cannot make another message-keyed
    // compaction card a member of this reasoning's group.
    pane.remember_live_part_parent("msg-other-summary", Some("msg-summary"));
    pane.upsert_part_message(
        NeoismAgentMessage::compaction("Other summary", "summary")
            .with_id("msg-other-summary"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::compaction("Own summary", "summary").with_id("msg-summary"),
    );
    pane.remember_live_part_parent("tool-other", Some("msg-other"));
    pane.upsert_part_message(
        NeoismAgentMessage::tool(
            "Bash",
            "",
            "completed",
            "bash",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-other"),
    );
    pane.remember_live_part_parent("reason-1", Some("msg-summary"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Own thought").with_id("reason-1"),
    );
    assert_eq!(
        pane.messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "answer-other",
            "msg-other-summary",
            "tool-other",
            "reason-1",
            "msg-summary"
        ]
    );
    // Reasoning for a different message must not move the compaction card.
    pane.remember_live_part_parent("reason-unrelated", Some("msg-no-card"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Unrelated").with_id("reason-unrelated"),
    );
    assert_eq!(pane.messages[4].id, "msg-summary");
    assert_eq!(pane.messages[5].id, "reason-unrelated");
}

#[test]
fn live_compaction_reasoning_does_not_cross_user_turn_boundary() {
    let mut pane = NeoismAgentPane::default();
    pane.upsert_part_message(
        NeoismAgentMessage::compaction("Prior summary", "summary").with_id("msg-summary"),
    );
    pane.messages
        .push(NeoismAgentMessage::user("New request").with_id("user-next"));
    pane.remember_live_part_parent("reason-late", Some("msg-summary"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Late thought").with_id("reason-late"),
    );
    assert_eq!(pane.messages[0].id, "msg-summary");
    assert_eq!(pane.messages[1].id, "user-next");
    assert_eq!(pane.messages[2].id, "reason-late");
}

#[test]
fn delayed_reasoning_end_uses_its_assistant_message_group_order() {
    let mut pane = NeoismAgentPane::default();
    pane.remember_live_part_parent("text-1", Some("assistant-message-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("final answer").with_id("text-1"),
    );
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
    pane.remember_live_part_parent("tool-1", Some("assistant-message-1"));
    pane.remember_live_part_parent("reason-1", Some("assistant-message-1"));

    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Clarifying task ID reuse and results")
            .with_id("reason-1"),
    );

    assert_eq!(pane.messages[0].id, "tool-1");
    assert_eq!(pane.messages[1].id, "reason-1");
    assert_eq!(pane.messages[2].id, "text-1");
}

#[test]
fn late_parent_metadata_repairs_live_reasoning_order_before_settle() {
    let mut pane = NeoismAgentPane::default();
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("Required release work").with_id("text-1"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Finalizing release and performance details")
            .with_id("reason-1"),
    );
    assert_eq!(pane.messages[0].id, "text-1");

    pane.remember_live_part_parent("text-1", Some("msg-response"));
    pane.remember_live_part_parent("reason-1", Some("msg-response"));

    assert_eq!(pane.messages[0].id, "reason-1");
    assert_eq!(pane.messages[1].id, "text-1");
}

#[test]
fn delayed_subagent_completion_stays_before_its_live_assistant_response() {
    let mut pane = NeoismAgentPane::default();
    pane.remember_live_part_parent("answer", Some("msg_03a800b6c001assistant"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("## Second-pass verdict").with_id("answer"),
    );

    pane.upsert_part_message(
        NeoismAgentMessage::system("Subagent", "Subagent finished.")
            .with_id("msg_03a800995001completion"),
    );
    pane.remember_live_part_parent("thinking", Some("msg_03a800b6c001assistant"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Summarizing confirmed findings")
            .with_id("thinking"),
    );

    assert_eq!(
        pane.messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["msg_03a800995001completion", "thinking", "answer"]
    );
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
fn tool_hover_uses_only_current_click_targets_without_expanding() {
    let mut pane = NeoismAgentPane::default();
    assert!(!pane.tool_hit(30.0, 160.0));
    pane.register_tool_hit_rect("tool-1".to_string(), [20.0, 150.0, 300.0, 30.0]);
    pane.register_tool_hit_rect("tool-1:0".to_string(), [20.0, 200.0, 300.0, 25.0]);
    assert!(pane.tool_hit(30.0, 160.0));
    assert!(pane.tool_hit(30.0, 210.0));
    assert!(!pane.tool_hit(30.0, 190.0));
    assert!(!pane.tool_hit(0.0, 160.0));
    assert!(pane.expanded_tool_ids.is_empty());
    assert!(pane.tool_expand_anims.is_empty());
    pane.clear_tool_hit_rects();
    assert!(!pane.tool_hit(30.0, 160.0));
}

#[test]
fn diff_file_toggle_keeps_position_and_anchors_reader_owned_remeasurement() {
    let mut pane = NeoismAgentPane::default();
    pane.messages.push(
        NeoismAgentMessage::tool(
            "ApplyPatch",
            "patch",
            "completed",
            "apply_patch",
            NeoismAgentOutputKind::Text,
            "diff",
            Vec::new(),
        )
        .with_id("tool-1"),
    );
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.timeline_velocity_px_s = 75.0;
    pane.register_tool_hit_rect("tool-1:0".to_string(), [20.0, 150.0, 300.0, 60.0]);
    let scroll_before = pane.timeline_scroll_px;
    let velocity_before = pane.timeline_velocity_px_s;

    assert!(pane.toggle_tool_at(30.0, 160.0));

    assert!(pane.tool_expanded("tool-1:0"));
    assert_eq!(pane.timeline_scroll_px, scroll_before);
    assert_eq!(velocity_before, 75.0);
    assert_eq!(pane.timeline_velocity_px_s, 0.0);
    assert!(!pane.timeline_follow_bottom);
    let anchor = pane.pending_timeline_anchor.as_ref().unwrap();
    assert_eq!(anchor.content_y, 450.0);
    assert_eq!(anchor.screen_y, 150.0);
    assert_eq!(pane.timeline_view_anchor.as_ref().unwrap().screen_offset, 50.0);
    assert!(!pane.tool_expand_animating("tool-1"));
}

#[test]
fn read_group_expansion_tracks_source_and_remeasures_after_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![NeoismAgentMessage::tool(
        "Read(src/a.rs)",
        "output",
        "completed",
        "read",
        NeoismAgentOutputKind::Text,
        "",
        Vec::new(),
    )
    .with_id("read-a")];
    pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.register_tool_hit_rect("read-a..".to_string(), [0.0, 150.0, 300.0, 30.0]);
    for expanding in [true, false] {
        assert!(pane.toggle_tool_at(10.0, 160.0));
        assert_eq!(pane.tool_expanded("read-a.."), expanding);
        assert!(pane.tool_expand_animating("read-a"));
        assert!(pane.timeline_view_anchor.is_some());
        let _ = pane.take_timeline_dirty_marks();
        pane.tool_expand_anims
            .get_mut("read-a..")
            .unwrap()
            .started_at = Instant::now() - TOOL_EXPAND_ANIMATION;
        let settled = pane.take_timeline_dirty_marks();
        assert!(settled.indices.contains(&0));
        assert!(!pane.any_tool_expand_animating());
        assert_eq!(
            pane.tool_expand_progress("read-a.."),
            if expanding { 1.0 } else { 0.0 }
        );
        assert!(pane.take_timeline_dirty_marks().ids.is_empty());
    }
}

#[test]
fn read_group_child_clicks_anchor_scroll_and_toggle_only_the_selected_read() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_velocity_px_s = 75.0;
    pane.register_tool_hit_rect(
        "read-a..::child::123".to_string(),
        [0.0, 150.0, 300.0, 20.0],
    );
    assert!(pane.toggle_tool_at(10.0, 160.0));
    assert_eq!(pane.selected_tool_group_child("read-a.."), Some("123"));
    assert!(pane.pending_timeline_anchor.is_some());
    assert_eq!(pane.timeline_velocity_px_s, 0.0);
    assert!(!pane.tool_expanded("read-a..::child::123"));
    assert!(!pane.any_tool_expand_animating());
    assert!(pane.toggle_tool_at(10.0, 160.0));
    assert_eq!(pane.selected_tool_group_child("read-a.."), None);
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
    pane.clear_tool_hit_rects();
    assert!(pane.markdown_horizontal_scrollbar_dragging());
    assert!(pane.drag_markdown_horizontal_scrollbar_to(150.0));
    assert!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:code:0", 240.0) > 75.0
    );
    assert!(pane.end_markdown_horizontal_scrollbar_drag());
}

#[test]
fn timeline_growth_preserves_reader_position_when_scrolled_up() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    assert!(pane.scroll_timeline_pixels(200.0));

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
fn duplicate_id_structural_snapshot_is_not_a_stable_source_prefix() {
    let previous = vec![
        NeoismAgentMessage::user("first").with_id("duplicate"),
        NeoismAgentMessage::user("second").with_id("duplicate"),
    ];
    let incoming = vec![
        NeoismAgentMessage::user("second").with_id("duplicate"),
        NeoismAgentMessage::user("replacement").with_id("duplicate"),
        NeoismAgentMessage::assistant("tail").with_id("tail"),
    ];

    assert!(!super::ingest::stable_timeline_source_prefix(
        &previous, &incoming
    ));
}

#[test]
fn timeline_without_scroll_range_does_not_claim_trackpad_or_wheel() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 120.0, 300.0);
    assert!(!pane.scroll_timeline_pixels(30.0));
    assert!(!pane.scroll_timeline_wheel_pixels(-60.0));
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
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
        Some(TimelineViewAnchorKey::at_source(0, 1, "visible-message")),
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
fn measured_prepend_ignores_simultaneous_live_tail_growth() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.pending_timeline_prepend_height_px = Some(900.0);
    pane.pending_timeline_prepend_delta_px = Some(200.0);

    // 200px was inserted above; the other 100px grew below in the live turn.
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1200.0, 300.0);

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
fn older_timeline_request_skipped_while_following_bottom() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());

    pane.maybe_request_older_timeline_page(0.0, 500.0);

    assert!(!pane.timeline_history.loading_older);
    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn older_timeline_request_gate_reopens_after_success() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    pane.timeline_follow_bottom = false;
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert!(pane.timeline_history.loading_older);
    let commands = pane.drain_pending_outbound();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        OutboundAgentCommand::LoadOlderTimeline { limit: 64, .. }
    ));

    // The in-flight gate blocks duplicates.
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 0);

    // Completing a page does not cascade into another request just because
    // hidden tool rows left the viewport near the boundary.
    pane.timeline_history.loading_older = false;
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 0);

    // Explicit movement farther into history re-arms exactly one page.
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 900.0, 300.0);
    assert!(pane.scroll_timeline_pixels(10.0));
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 1);
}

#[test]
fn older_timeline_request_skipped_below_boundary() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    pane.timeline_follow_bottom = false;

    pane.maybe_request_older_timeline_page(800.0, 500.0);
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
    pane.apply_older_timeline_page(
        "session-1".to_string(),
        vec![older],
        1,
        1,
        Some("raw-oldest".to_string()),
        false,
    );

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].id, "m-older");
    assert!(pane.timeline_history.has_older);
    assert!(!pane.timeline_history.loading_older);
    assert_eq!(
        pane.timeline_history.last_requested_session_id.as_deref(),
        Some("session-1")
    );
    assert_eq!(
        pane.timeline_history.oldest_loaded_cursor.as_deref(),
        Some("raw-oldest")
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
    pane.apply_older_timeline_page(
        "session-1".to_string(),
        vec![older],
        1,
        128,
        Some("raw-oldest".to_string()),
        true,
    );

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
    pane.apply_older_timeline_page(
        "session-1".to_string(),
        vec![older],
        1,
        1,
        Some("raw-oldest".to_string()),
        false,
    );

    assert!(pane.messages.is_empty());
    assert!(!pane.timeline_history.loading_older);
}

#[test]
fn timeline_growth_keeps_following_stream_at_bottom() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 0.0;
    pane.apply_part_delta(None, None, Some("text".into()), "live");

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);

    assert_eq!(pane.timeline_scroll_offset(), 200.0);
    assert!(pane.timeline_follow_bottom);
    assert_eq!(pane.timeline_wheel_target_px, Some(0.0));
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
    pane.apply_part_delta(None, None, Some("text".into()), "live after returning");
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1000.0, 300.0);

    assert_eq!(pane.timeline_scroll_offset(), 100.0);
    assert!(pane.timeline_follow_bottom);
    assert_eq!(pane.timeline_wheel_target_px, Some(0.0));
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

#[test]
fn transcript_selection_can_start_in_whitespace_beside_text() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 200.0, 200.0);
    pane.register_selectable_line("select me", [20.0, 30.0, 70.0, 20.0]);

    assert!(pane.begin_selection_at(300.0, 40.0));
    assert!(pane.has_active_selection());
}

#[test]
fn transcript_selection_distinguishes_plain_click_from_drag() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 200.0, 200.0);
    pane.register_selectable_line("select me", [20.0, 30.0, 70.0, 20.0]);

    assert!(pane.begin_selection_on_text_at(30.0, 40.0));
    assert!(pane.selection_is_plain_click());
    assert_eq!(pane.end_selection(), None);

    assert!(pane.begin_selection_on_text_at(30.0, 40.0));
    assert!(pane.drag_selection_to(70.0, 40.0));
    assert!(!pane.selection_is_plain_click());
    assert!(pane.end_selection().is_some());
    assert!(pane.selectable_line_highlight(0).is_some());
    assert!(!pane.has_active_selection());
    assert!(!pane.drag_selection_to(90.0, 40.0));
    assert_eq!(pane.end_selection(), None);
    pane.register_selectable_line("another row", [20.0, 60.0, 100.0, 20.0]);
    assert!(!pane.has_active_selection());
    assert!(pane.clear_selection());
    assert!(pane.selectable_line_highlight(0).is_none());
}

#[test]
fn transcript_selection_copies_rows_scrolled_out_of_view() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 40.0], 120.0, 40.0);
    pane.timeline_scroll_px = 80.0;
    pane.register_selectable_line("first", [0.0, 10.0, 50.0, 18.0]);
    pane.register_selectable_line("middle", [0.0, 30.0, 60.0, 18.0]);
    assert!(pane.begin_selection_at(0.0, 10.0));
    pane.timeline_scroll_px = 40.0;
    pane.selectable_lines_len = 0; // The next frame renders only the scrolled viewport.
    pane.register_selectable_line("last", [0.0, 10.0, 40.0, 18.0]);
    assert!(pane.drag_selection_to(40.0, 10.0));
    assert_eq!(pane.end_selection().as_deref(), Some("first\nmiddle\nlast"));
    assert!(pane.selection_history.is_empty());
}

#[test]
fn transcript_selection_preserves_registered_blank_lines() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 200.0, 200.0);
    pane.register_selectable_line("first", [0.0, 20.0, 50.0, 18.0]);
    pane.register_selectable_line("", [0.0, 40.0, 12.0, 18.0]);
    pane.register_selectable_line("second", [0.0, 60.0, 60.0, 18.0]);

    assert!(pane.begin_selection_at(0.0, 20.0));
    assert!(pane.drag_selection_to(60.0, 60.0));
    assert_eq!(pane.end_selection().as_deref(), Some("first\n\nsecond"));
}

#[test]
fn transcript_selection_uses_exact_proportional_caret_stops() {
    use neoism_ui::panels::agent_pane::selection_model::SelectableCaretStop;

    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 200.0, 200.0);
    pane.register_selectable_line_with_caret_stops(
        "WW ii",
        [10.0, 20.0, 50.0, 18.0],
        &[
            SelectableCaretStop {
                byte_offset: 0,
                x: 10.0,
            },
            SelectableCaretStop {
                byte_offset: 1,
                x: 25.0,
            },
            SelectableCaretStop {
                byte_offset: 2,
                x: 40.0,
            },
            SelectableCaretStop {
                byte_offset: 3,
                x: 45.0,
            },
            SelectableCaretStop {
                byte_offset: 4,
                x: 48.0,
            },
            SelectableCaretStop {
                byte_offset: 5,
                x: 51.0,
            },
        ],
    );

    assert!(pane.begin_selection_at(45.1, 25.0));
    assert!(pane.drag_selection_to(51.0, 25.0));
    assert_eq!(pane.end_selection().as_deref(), Some("ii"));
}

#[test]
fn task_row_click_navigates_from_detail_or_text_for_any_status() {
    for kind in [
        NeoismAgentMessageKind::Tool,
        NeoismAgentMessageKind::Subtask,
    ] {
        for status in ["running", "completed", "error", "stopped"] {
            for detail_has_id in [true, false] {
                let mut pane = NeoismAgentPane::default();
                pane.session_id = Some("parent".to_string());
                let mut message = task_tool_message("child", status).with_id("task-row");
                message.kind = kind;
                message.title = "A descriptive title, not a session id".to_string();
                if detail_has_id {
                    // Detail metadata wins over an older output marker.
                    message.text = "task_id: stale-child".to_string();
                } else {
                    message.detail.clear();
                }
                pane.messages = vec![message];
                pane.register_tool_hit_rect(
                    "task-row".to_string(),
                    [0.0, 0.0, 200.0, 40.0],
                );

                assert!(!pane.toggle_tool_at(300.0, 10.0));
                assert_eq!(pane.session_id.as_deref(), Some("parent"));
                assert!(pane.toggle_tool_at(10.0, 10.0));
                assert_eq!(pane.session_id.as_deref(), Some("child"));
                assert!(!pane.tool_expanded("task-row"));
                assert!(!pane.tool_expand_animating("task-row"));
            }
        }
    }
}

#[test]
fn task_row_without_child_id_and_non_task_rows_still_expand() {
    for is_task in [true, false] {
        let mut pane = NeoismAgentPane::default();
        pane.session_id = Some("parent".to_string());
        let mut message = task_tool_message("child", "completed").with_id("tool-row");
        if is_task {
            message.text = "No child session yet".to_string();
            message.detail = "task_id:   ".to_string();
        } else {
            message.tool = "read".to_string();
        }
        pane.messages = vec![message];
        pane.register_tool_hit_rect("tool-row".to_string(), [0.0, 0.0, 200.0, 40.0]);

        for expanded in [true, false] {
            assert!(pane.toggle_tool_at(10.0, 10.0));
            assert_eq!(pane.session_id.as_deref(), Some("parent"));
            assert_eq!(pane.tool_expanded("tool-row"), expanded);
        }
    }
}

#[test]
fn task_metadata_does_not_change_diff_or_group_child_clicks() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.messages = vec![task_tool_message("child", "running").with_id("task-row")];
    pane.register_tool_hit_rect("task-row:0".to_string(), [0.0, 0.0, 200.0, 40.0]);
    assert!(pane.toggle_tool_at(10.0, 10.0));
    assert!(pane.tool_expanded("task-row:0"));
    assert_eq!(pane.session_id.as_deref(), Some("parent"));

    pane.register_tool_hit_rect(
        "task-row::child::read-1".to_string(),
        [0.0, 50.0, 200.0, 40.0],
    );
    assert!(pane.toggle_tool_at(10.0, 60.0));
    assert_eq!(pane.selected_tool_group_child("task-row"), Some("read-1"));
    assert_eq!(pane.session_id.as_deref(), Some("parent"));
}

/// Historical Task rows are navigation evidence, not running authority.
#[test]
fn task_click_restores_pruned_family_before_cold_and_cached_navigation() {
    for cached in [false, true] {
        for task_status in ["completed", "running", "error"] {
            for known_terminal in [false, true] {
                let mut pane = NeoismAgentPane::default();
                pane.session_id = Some("root".into());
                pane.side_panel.ensure_subagent_main_entry("root");
                for child in ["child", "old-sibling"] {
                    pane.side_panel
                        .upsert_subagent(child, "Old worker", "subagent");
                    pane.side_panel.set_branch_activity_status_from_recovery(
                        child,
                        BranchStatus::Completed,
                    );
                }
                assert!(pane.side_panel.prune_expired_completed_subagents());
                assert_eq!(pane.side_panel.subagents().len(), 1);
                if !known_terminal {
                    pane.side_panel
                        .retain_authoritative_branches(&Default::default());
                }
                if cached {
                    let mut child = CachedAgentSession::live_only();
                    child.hydrated = true;
                    pane.session_cache.insert("child".into(), child);
                }
                let mut task =
                    task_tool_message("child", task_status).with_id("old-task");
                task.title = "Inspect navigation regression".into();
                pane.messages = vec![task];
                pane.register_tool_hit_rect("old-task".into(), [0.0, 0.0, 200.0, 40.0]);

                assert!(pane.toggle_tool_at(10.0, 10.0));
                assert_eq!(pane.session_id.as_deref(), Some("child"));
                assert_eq!(pane.parent_session_id.as_deref(), Some("root"));
                assert!(pane.is_subagent_session());
                let expected = if task_status == "error" && !known_terminal {
                    BranchStatus::Stopped
                } else {
                    BranchStatus::Completed
                };
                assert_eq!(
                    pane.side_panel.branch_activity("child").unwrap().status,
                    expected
                );
                assert!(
                    pane.side_panel
                        .branch_activity("child")
                        .unwrap()
                        .terminal_locked
                );
                assert_eq!(pane.active_subagent_count(), 0);
                assert_eq!(pane.side_panel.active_child_count(Some("root")), 0);
                assert_eq!(
                    pane.side_panel.subagents()[1].title,
                    "Inspect navigation regression"
                );

                // Omitted roster and authority snapshots cannot erase the view.
                for _ in 0..2 {
                    pane.side_panel
                        .set_subagents(vec![NeoismAgentSessionEntry::new(
                            "root",
                            "main session",
                            "return",
                        )]);
                    pane.side_panel
                        .retain_authoritative_branches(&Default::default());
                    pane.side_panel.prune_expired_completed_subagents();
                    assert_eq!(
                        pane.side_panel
                            .subagents()
                            .iter()
                            .map(|entry| entry.id.as_str())
                            .collect::<Vec<_>>(),
                        vec!["root", "child"],
                    );
                    assert_eq!(
                        pane.side_panel.branch_activity("child").unwrap().status,
                        expected
                    );
                }
                if cached {
                    pane.side_panel.set_mode(
                        crate::neoism::agent::side_panel::SidePanelMode::Subagents,
                    );
                    pane.side_panel.set_selected(0);
                    assert!(pane.activate_side_panel_subagent());
                } else {
                    pane.switch_session("root".into());
                }
                assert_eq!(pane.session_id.as_deref(), Some("root"));
                assert!(pane.parent_session_id.is_none());
                pane.side_panel.prune_expired_completed_subagents();
                assert_eq!(pane.side_panel.subagents().len(), 1);
                assert_eq!(pane.active_subagent_count(), 0);
            }
        }
    }
}

#[test]
fn task_click_restores_missing_main_and_preserves_known_activity_or_runtime() {
    for authority in ["missing", "activity", "runtime"] {
        let mut pane = NeoismAgentPane::default();
        // Enter from a nested transcript with the roster already discarded.
        pane.session_id = Some("sibling".into());
        pane.parent_session_id = Some("root".into());
        if authority == "activity" {
            pane.side_panel.set_branch_activity_tool(
                "child",
                BranchStatus::WaitingPermission,
                Some("shell".into()),
                Some(123),
            );
        } else if authority == "runtime" {
            pane.side_panel.set_subagents(vec![
                NeoismAgentSessionEntry::new("root", "main session", "return"),
                NeoismAgentSessionEntry::new("child", "Worker", "subagent")
                    .with_runtime_status(Some("running".into())),
            ]);
        }
        pane.messages = vec![task_tool_message("child", "error").with_id("task")];
        pane.register_tool_hit_rect("task".into(), [0.0, 0.0, 200.0, 40.0]);
        assert!(pane.toggle_tool_at(10.0, 10.0));
        assert_eq!(pane.parent_session_id.as_deref(), Some("root"));
        assert_eq!(pane.side_panel.subagents()[0].id, "root");
        assert_eq!(pane.side_panel.subagents()[1].id, "child");
        match authority {
            "activity" => {
                let activity = pane.side_panel.branch_activity("child").unwrap();
                assert_eq!(activity.status, BranchStatus::WaitingPermission);
                assert_eq!(activity.current_tool.as_deref(), Some("shell"));
                assert_eq!(activity.started_at, Some(123));
            }
            "runtime" => {
                assert_eq!(
                    pane.side_panel.subagents()[1].runtime_status.as_deref(),
                    Some("running")
                );
                assert_eq!(
                    pane.side_panel.branch_activity("child").unwrap().status,
                    BranchStatus::Active,
                );
            }
            _ => assert_eq!(
                pane.side_panel.branch_activity("child").unwrap().status,
                BranchStatus::Stopped
            ),
        }
    }
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

#[test]
fn streaming_reveal_ingestion_captures_history_before_live_update_and_skips_replay() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![NeoismAgentMessage::assistant("history").with_id("row")];
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("history more").with_id("row"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("history more").with_id("row"),
    );
    let pending = pane.text_reveal.pending("row");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].text, "history");
    assert!(pending[0].at.is_none());
    assert_eq!(pending[1].text, "history more");
    assert!(pending[1].at.is_some());
    pane.set_text_reveal_enabled(false);
    pane.apply_part_delta(None, Some("row".into()), Some("text".into()), " disabled");
    assert!(pane.text_reveal.pending("row").is_empty());
}

#[test]
fn tool_motion_live_history_and_selected_body() {
    let mut pane = NeoismAgentPane::default();
    let make_row = |status: &str, detail: &str| {
        let mut row = NeoismAgentMessage::tool(
            "Read",
            "request-metadata",
            status,
            "read",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        );
        row.id = "motion-row".into();
        row.detail = detail.into();
        row
    };
    pane.messages.push(make_row("running", "historical output"));
    assert!(pane
        .tool_motion_state()
        .sample("motion-row")
        .deadline
        .is_none());
    pane.upsert_part_message(make_row("running", "historical output more"));
    assert!(pane
        .tool_motion_state()
        .sample("motion-row")
        .deadline
        .is_none());
    let pending = pane.tool_text_reveal_state().pending("motion-row");
    assert_eq!(pending[0].text, "historical output");
    assert_eq!(pending[1].text, "historical output more");
    pane.upsert_part_message(make_row("completed", "final output"));
    let sample = pane.tool_motion_state().sample("motion-row");
    assert_eq!(sample.opacity, 1.0);
    assert_eq!(sample.outgoing_status, Some("running"));
    assert!(!pane
        .tool_motion
        .is_animating_for(pane.session_id.as_deref()));
    let epoch = pane.timeline_layout_epoch;
    pane.tool_motion.mark_visible(sample);
    assert_eq!(
        pane.timeline_layout_epoch, epoch,
        "paint-only motion cannot revise layout"
    );
    assert_eq!(pane.animation_reason(), Some("tool_motion"));
    pane.clear_tool_hit_rects();
    assert_ne!(pane.animation_reason(), Some("tool_motion"));
    pane.set_text_reveal_enabled(false);
    assert!(pane
        .tool_motion_state()
        .sample("motion-row")
        .deadline
        .is_none());
}

fn tool_expansion_view_test_pane(id: &str, row_top: f32) -> NeoismAgentPane {
    use neoism_ui::panels::agent_pane::view::timeline::TimelineLayoutRow;
    let mut pane = NeoismAgentPane::default();
    for name in if id.ends_with("..") {
        vec!["open-read", "open-read-2"]
    } else {
        vec!["open-read"]
    } {
        let mut message = NeoismAgentMessage::tool(
            "Read",
            "output",
            "completed",
            "read",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id(name);
        message.tool_batch_id = Some("open-batch".into());
        pane.messages.push(message);
    }
    pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    *pane.timeline_layout_cache.borrow_mut() = Some(TimelineLayoutCache {
        question_targets: Default::default(),
        epoch: pane.timeline_layout_epoch,
        artifact_revision: None,
        source_len: pane.messages.len(),
        width_bucket: 0,
        scale_bucket: 0,
        gap_bucket: 0,
        content_height: 900.0,
        pages: Vec::new(),
        estimated_prefix_rows: 0,
        estimated_suffix_start: 1,
        rows: vec![TimelineLayoutRow {
            source_index: 0,
            source_end_index: pane.messages.len() - 1,
            top: row_top,
            height: 30.0,
            display_text: None,
            display_message: None,
            markdown_blocks: None,
            tool_diff_sections: None,
            is_edit_tool: false,
        }],
    });
    let header_y = (100.0 + row_top - 600.0).max(100.0);
    pane.register_tool_hit_rect(id.into(), [0.0, header_y, 300.0, 20.0]);
    pane
}

#[test]
fn tool_expansion_clipped_single_source_uses_logical_row_origin() {
    let mut pane = tool_expansion_view_test_pane("open-read", 590.0);
    assert!(pane.toggle_tool_at(10.0, 105.0));
    let screen_offset = pane.timeline_view_anchor.as_ref().unwrap().screen_offset;
    // The hit rectangle starts at the viewport edge, but the row starts above it.
    assert_eq!(screen_offset, -10.0);
    assert!(pane.tool_expand_animating("open-read"));
    pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 940.0, 300.0);
    pane.restore_timeline_view_anchor(590.0, screen_offset);
    assert_eq!(
        pane.max_timeline_scroll() - pane.timeline_scroll_offset(),
        600.0
    );
}

#[test]
fn tool_expansion_grouped_last_source_uses_logical_row_origin() {
    let mut pane = tool_expansion_view_test_pane("open-read..", 590.0);
    pane.clear_tool_hit_rects();
    // Exercise a hit attributed to the last source covered by the grouped row.
    pane.register_tool_hit_rect("open-read-2".into(), [0.0, 100.0, 300.0, 20.0]);
    assert!(pane.toggle_tool_at(10.0, 105.0));
    assert_eq!(
        pane.timeline_view_anchor.as_ref().unwrap().screen_offset,
        -10.0
    );
    assert!(pane.tool_expand_animating("open-read-2"));
}

#[test]
fn tool_expansion_settled_and_stable_frames_keep_the_clicked_header() {
    for (id, row_top) in [
        ("open-read", 650.0),
        ("open-read..", 650.0),
        ("open-read", 590.0),
        ("open-read..", 590.0),
    ] {
        let mut pane = tool_expansion_view_test_pane(id, row_top);
        let header_y = (100.0 + row_top - 600.0).max(100.0);
        assert!(pane.toggle_tool_at(10.0, header_y + 5.0));
        assert!(!pane.timeline_follow_bottom);
        let screen_offset = pane.timeline_view_anchor.as_ref().unwrap().screen_offset;
        assert_eq!(screen_offset, row_top - 600.0);
        for height in [940.0, 1000.0] {
            pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], height, 300.0);
            pane.restore_timeline_view_anchor(row_top, screen_offset);
            assert_eq!(
                pane.max_timeline_scroll() - pane.timeline_scroll_offset(),
                600.0
            );
        }
        pane.tool_expand_anims.get_mut(id).unwrap().started_at =
            Instant::now() - Duration::from_millis(70);
        assert!(pane.toggle_tool_at(10.0, header_y + 5.0));
        pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 960.0, 300.0);
        pane.restore_timeline_view_anchor(row_top, screen_offset);
        assert_eq!(
            pane.max_timeline_scroll() - pane.timeline_scroll_offset(),
            600.0
        );
        pane.tool_expand_anims.get_mut(id).unwrap().started_at =
            Instant::now() - Duration::from_millis(40);
        assert!(pane.toggle_tool_at(10.0, header_y + 5.0));
        pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 1020.0, 300.0);
        pane.restore_timeline_view_anchor(row_top, screen_offset);
        assert_eq!(
            pane.max_timeline_scroll() - pane.timeline_scroll_offset(),
            600.0
        );
        pane.tool_expand_anims.get_mut(id).unwrap().started_at =
            Instant::now() - TOOL_EXPAND_ANIMATION;
        assert!(pane.take_timeline_dirty_marks().indices.contains(&0));
        pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 1100.0, 300.0);
        pane.restore_timeline_view_anchor(row_top, screen_offset);
        assert!(pane.pending_timeline_anchor.is_none());
        for _ in 0..3 {
            pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 1100.0, 300.0);
            assert_eq!(
                pane.max_timeline_scroll() - pane.timeline_scroll_offset(),
                600.0
            );
            assert!(!pane.timeline_follow_bottom);
            assert_eq!(pane.timeline_wheel_target_px, None);
        }
    }
}

#[test]
fn live_follow_budget_excludes_history_remeasurement_but_keeps_spring() {
    let mut pane = NeoismAgentPane::default();
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 1000.0, 300.0);
    pane.timeline_live_growth = true;
    pane.set_timeline_metrics_with_live_growth(rect, 31_024.0, 300.0, Some(24.0));
    assert_eq!(pane.timeline_scroll_px, 24.0);
    assert_eq!(
        pane.timeline_scroll_owner,
        TimelineScrollOwner::FollowBottom
    );
    assert_eq!(pane.timeline_wheel_target_px, Some(0.0));
    pane.set_timeline_metrics_with_live_growth(rect, 41_024.0, 300.0, Some(0.0));
    assert_eq!(pane.timeline_scroll_px, 24.0);
    assert!(pane.timeline_is_inertial());
    assert!(pane.timeline_follow_bottom());
}

#[test]
fn geometry_only_rebuild_during_live_revision_does_not_start_follow_spring() {
    let mut pane = NeoismAgentPane::default();
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 1000.0, 300.0);
    pane.timeline_live_growth = true;
    pane.set_timeline_metrics_with_live_growth(rect, 31_000.0, 300.0, Some(0.0));
    assert_eq!(pane.timeline_scroll_px, 0.0);
    assert!(!pane.timeline_is_inertial());
    assert!(pane.timeline_follow_bottom());
}

#[test]
fn tool_expansion_clears_live_follow_and_keeps_manual_override() {
    let mut pane = tool_expansion_view_test_pane("open-read", 650.0);
    pane.timeline_scroll_owner = TimelineScrollOwner::FollowBottom;
    pane.timeline_wheel_target_px = Some(0.0);
    pane.timeline_velocity_px_s = -150.0;
    pane.timeline_last_tick_at = Some(Instant::now());
    assert!(pane.toggle_tool_at(10.0, 155.0));
    assert!(!pane.timeline_follow_bottom);
    assert_eq!(pane.timeline_wheel_target_px, None);
    assert_eq!(pane.timeline_velocity_px_s, 0.0);
    assert_eq!(pane.timeline_last_tick_at, None);
    assert!(!pane.tick_timeline_scroll());
    pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 980.0, 300.0);
    assert!(pane.scroll_timeline_pixels(50.0));
    assert!(pane.pending_timeline_anchor.is_none());
    pane.tool_expand_anims
        .get_mut("open-read")
        .unwrap()
        .started_at = Instant::now() - TOOL_EXPAND_ANIMATION;
    pane.take_timeline_dirty_marks();
    let screen_offset = pane.timeline_view_anchor.as_ref().unwrap().screen_offset;
    pane.set_timeline_metrics([0.0, 100.0, 400.0, 300.0], 1100.0, 300.0);
    pane.restore_timeline_view_anchor(650.0, screen_offset);
    assert_eq!(
        pane.max_timeline_scroll() - pane.timeline_scroll_offset(),
        550.0
    );
    assert!(!pane.timeline_follow_bottom);
    assert!(pane.scroll_timeline_pixels(-pane.max_timeline_scroll()));
    assert!(pane.timeline_follow_bottom);
}

#[test]
fn tool_motion_expansion_reversal_preserves_current_progress() {
    let mut pane = NeoismAgentPane::default();
    pane.register_tool_hit_rect("motion-row".into(), [0.0, 0.0, 100.0, 30.0]);
    assert!(pane.toggle_tool_at(10.0, 10.0));
    pane.tool_expand_anims
        .get_mut("motion-row")
        .unwrap()
        .started_at = Instant::now() - Duration::from_millis(70);
    let before = pane.tool_expand_progress("motion-row");
    assert!(before > 0.1 && before < 1.0);
    assert!(pane.toggle_tool_at(10.0, 10.0));
    let captured = pane.tool_expand_anims["motion-row"].start_progress;
    assert!((captured - before).abs() < 0.02);
    assert!((pane.tool_expand_progress("motion-row") - before).abs() < 0.02);
    pane.tool_expand_anims
        .get_mut("motion-row")
        .unwrap()
        .started_at = Instant::now() - Duration::from_millis(40);
    let closing = pane.tool_expand_progress("motion-row");
    assert!(closing < captured);
    assert!(pane.toggle_tool_at(10.0, 10.0));
    assert!((pane.tool_expand_anims["motion-row"].start_progress - closing).abs() < 0.02);
    pane.set_text_reveal_enabled(false);
    assert_eq!(pane.tool_expand_progress("motion-row"), 1.0);
    assert!(!pane.tool_expand_animating("motion-row"));
}

#[test]
fn tool_motion_actual_group_identity_samples_all_source_members_not_previews() {
    let mut pane = NeoismAgentPane::default();
    let row = |id: &str, batch: &str, status: &str| {
        let mut row = NeoismAgentMessage::tool(
            "Read file",
            "output",
            status,
            "read",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id(id.to_string());
        row.tool_batch_id = Some(batch.into());
        row
    };
    // The first completion is historical/expired; the fifth is outside the
    // four-member preview but still owns the batch's final completion fade.
    pane.messages = vec![
        row("first", "batch", "completed"),
        row("second", "batch", "completed"),
        row("third", "batch", "completed"),
        row("fourth", "batch", "completed"),
        row("fifth", "batch", "running"),
        row("unrelated", "next-batch", "running"),
    ];
    pane.upsert_part_message(row("fifth", "batch", "completed"));
    let last = pane.tool_motion_sample("fifth");
    pane.tool_motion_state()
        .record("unrelated", Some("streaming"), "completed");
    let group = pane.tool_motion_sample("first..");
    assert_eq!(
        group.opacity, 1.0,
        "later members cannot re-arrive the group"
    );
    assert_eq!(group.offset_y, 0.0);
    assert_eq!(
        group.outgoing_status,
        Some("running"),
        "next batch must not contribute its streaming label"
    );
    assert_eq!(group.deadline, last.deadline);
    assert!(group.status_progress < 1.0);
    assert_eq!(
        pane.tool_motion_sample("first..::child::unrelated")
            .outgoing_status,
        Some("streaming")
    );
}

#[test]
fn tool_motion_settled_source_groups_sample_sharp_without_active_ledger() {
    let mut pane = NeoismAgentPane::default();
    for i in 0..32 {
        let mut row = NeoismAgentMessage::tool(
            "Read file",
            "history",
            "completed",
            "read",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id(format!("history-{i}"));
        row.tool_batch_id = Some("batch".into());
        pane.messages.push(row);
    }
    assert!(!pane.tool_motion_state().has_active_motion());
    let sample = pane.tool_motion_sample("history-0..");
    assert_eq!(sample.opacity, 1.0);
    assert_eq!(sample.offset_y, 0.0);
    assert_eq!(sample.status_progress, 1.0);
    assert!(sample.outgoing_status.is_none());
    assert!(sample.deadline.is_none());
    assert!(!pane.tool_motion_state().has_active_motion());
}

// Mirrored in desktop/shared: exercise real pane state with clock-free spring steps.
fn follow_scroll_apply_test_history(pane: &mut NeoismAgentPane) {
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "a",
        [AgentSessionUpdate::Messages {
            messages: pane.messages.clone(),
            oldest_cursor: None,
        }],
    ));
    pane.drain_server_updates();
}

#[test]
fn follow_scroll_history_page_refinements_do_not_start_follow_motion() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("a".into());
    pane.messages = vec![NeoismAgentMessage::assistant("history").with_id("tail")];
    follow_scroll_apply_test_history(&mut pane);
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 700.0, 300.0);
    pane.timeline_history.loading_older = true;
    pane.apply_older_timeline_page(
        "a".into(),
        vec![NeoismAgentMessage::user("older history").with_id("prefix")],
        1,
        64,
        None,
        true,
    );
    for height in [900.0, 1200.0, 1800.0] {
        pane.set_timeline_metrics(rect, height, 300.0);
        assert_eq!(pane.timeline_scroll_offset(), 0.0);
        assert_eq!(pane.timeline_wheel_target_px, None);
        assert!(pane.timeline_follow_bottom);
    }
}

#[test]
fn follow_scroll_live_arrival_during_initial_history_fetch_can_overflow() {
    let mut pane = NeoismAgentPane::default();
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.timeline_history.loading_older = true;
    pane.set_timeline_metrics(rect, 100.0, 300.0);
    assert!(!pane.timeline_history_position_hydrated);
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("first live reply").with_id("live"),
    );
    pane.set_timeline_metrics(rect, 500.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 200.0);
    assert_eq!(
        pane.timeline_scroll_owner,
        TimelineScrollOwner::FollowBottom
    );
    assert!(pane.timeline_follow_bottom);
}

#[test]
fn follow_scroll_history_refinements_start_at_bottom_on_every_frame() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("a".into());
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 0.0, 300.0);
    pane.messages = vec![NeoismAgentMessage::assistant("history").with_id("old")];
    follow_scroll_apply_test_history(&mut pane);
    for height in [100.0, 500.0, 1400.0, 1300.0, 2400.0] {
        pane.set_timeline_metrics(rect, height, 300.0);
        // This is the render.rs first-frame draw coordinate, before any tick.
        let max_scroll = (height - 300.0_f32).max(0.0);
        assert_eq!(max_scroll - pane.timeline_scroll_offset(), max_scroll);
        assert_eq!(pane.timeline_scroll_offset(), 0.0);
        assert_eq!(pane.timeline_wheel_target_px, None);
        assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
    }
    pane.apply_part_delta(Some("old".into()), None, Some("text".into()), " live tail");
    follow_scroll_apply_test_history(&mut pane); // idle echo before final delta paint
    pane.set_timeline_metrics(rect, 2500.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 100.0);
    assert_eq!(
        pane.timeline_scroll_owner,
        TimelineScrollOwner::FollowBottom
    );
    assert!(pane.timeline_follow_bottom);
    // A refresh is history, but may not cancel the final live spring.
    pane.messages.insert(
        0,
        NeoismAgentMessage::user("older history").with_id("older"),
    );
    // Deliver a changed snapshot, not an identical echo of the live revision.
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "a",
        [AgentSessionUpdate::Messages {
            messages: pane.messages.clone(),
            oldest_cursor: None,
        }],
    ));
    pane.messages.remove(0);
    pane.drain_server_updates();
    let lag = pane.timeline_scroll_offset();
    pane.set_timeline_metrics(rect, 2800.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), lag);
    for _ in 0..120 {
        pane.step_owned_timeline_spring(1.0 / 60.0);
        pane.set_timeline_metrics(rect, 2800.0, 300.0);
    }
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(!pane.timeline_is_inertial());
}

#[test]
fn follow_scroll_warm_cache_hidden_live_growth_is_history_on_reopen() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("a".into());
    pane.messages = vec![NeoismAgentMessage::assistant("old").with_id("part")];
    follow_scroll_apply_test_history(&mut pane);
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 900.0, 300.0);
    pane.apply_part_delta(Some("part".into()), None, Some("text".into()), " live");
    pane.set_timeline_metrics(rect, 1100.0, 300.0);
    assert!(pane.timeline_scroll_offset() > 0.0);
    let mut other = CachedAgentSession::live_only();
    other.hydrated = true;
    other.messages = vec![NeoismAgentMessage::assistant("other")];
    pane.session_cache.insert("b".into(), other);
    pane.switch_session("b".into());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "b",
        [AgentSessionUpdate::ChildPartDelta {
            session_id: "a".into(),
            message_id: Some("part".into()),
            part_id: None,
            kind: Some("text".into()),
            delta: " hidden tail".into(),
        }],
    ));
    pane.drain_server_updates();
    pane.switch_session("a".into());
    assert!(pane.messages[0].text.ends_with("hidden tail"));
    for height in [100.0, 1200.0, 3200.0, 4000.0] {
        pane.set_timeline_metrics(rect, height, 300.0);
        let max_scroll = (height - 300.0_f32).max(0.0);
        assert_eq!(max_scroll - pane.timeline_scroll_offset(), max_scroll);
        assert_eq!(pane.timeline_wheel_target_px, None);
        assert!(!pane.timeline_is_inertial());
    }
    pane.scroll_timeline_pixels(150.0);
    let top = pane.max_timeline_scroll() - pane.timeline_scroll_offset();
    pane.set_timeline_metrics(rect, 4500.0, 300.0);
    assert_eq!(
        pane.max_timeline_scroll() - pane.timeline_scroll_offset(),
        top
    );
    assert!(!pane.timeline_follow_bottom);
}

#[test]
fn follow_scroll_reopened_trace_is_not_new_output() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("a".into());
    let tool = NeoismAgentMessage::tool(
        "Read file",
        "",
        "completed",
        "read",
        NeoismAgentOutputKind::Text,
        "one line",
        Vec::new(),
    )
    .with_id("old-tool");
    pane.messages = vec![
        NeoismAgentMessage::user("Inspect the project").with_id("prompt"),
        NeoismAgentMessage::reasoning("Previous reasoning").with_id("reasoning"),
        tool.clone(),
        NeoismAgentMessage::assistant("Existing reply").with_id("reply"),
    ];
    follow_scroll_apply_test_history(&mut pane);
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 900.0, 300.0);
    pane.upsert_part_message(tool.clone().with_id("new-tool"));
    // A second event before paint must not overwrite disclosure provenance.
    pane.upsert_part_message(tool.clone().with_id("batched-tool"));
    assert!(pane.timeline_trace_reveal_pending);
    pane.set_timeline_metrics(rect, 3180.0, 300.0);
    assert_eq!(
        (pane.timeline_scroll_offset(), pane.timeline_wheel_target_px),
        (0.0, None)
    );
    assert!(!pane.timeline_live_growth);
    pane.set_timeline_metrics(rect, 3200.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    pane.upsert_part_message(tool.with_id("next-tool"));
    pane.set_timeline_metrics(rect, 3248.0, 300.0);
    assert_eq!(
        (pane.timeline_scroll_offset(), pane.timeline_wheel_target_px),
        (48.0, Some(0.0))
    );
}

#[test]
fn follow_scroll_cached_short_tool_and_queued_echo_open_bottom() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("a".into());
    pane.messages =
        vec![NeoismAgentMessage::assistant("Existing reply").with_id("reply")];
    follow_scroll_apply_test_history(&mut pane);
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 900.0, 300.0);
    let mut other = CachedAgentSession::live_only();
    other.hydrated = true;
    pane.session_cache.insert("b".into(), other);
    pane.switch_session("b".into());
    let tool = NeoismAgentMessage::tool(
        "Read file",
        "",
        "completed",
        "read",
        NeoismAgentOutputKind::Text,
        "one line",
        Vec::new(),
    )
    .with_id("short-tool");
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "a",
        [AgentSessionUpdate::PartUpdated {
            message: tool.clone(),
            parent_message_id: Some("reply".into()),
        }],
    ));
    pane.drain_live_session_updates();
    let cached = pane.session_cache.get("a").unwrap();
    assert_eq!(cached.timeline_content_height_px, 900.0);
    assert!(cached.messages.iter().any(|m| m.id == "short-tool"));
    pane.switch_session("a".into());
    assert_eq!(pane.timeline_viewport_rect, None);
    let echo = pane.messages.clone();
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "a",
        [
            AgentSessionUpdate::PartUpdated {
                message: tool.clone(),
                parent_message_id: Some("reply".into()),
            },
            AgentSessionUpdate::SessionIdle,
            AgentSessionUpdate::Messages {
                messages: echo,
                oldest_cursor: None,
            },
        ],
    ));
    pane.drain_server_updates();
    pane.set_timeline_metrics(rect, 948.0, 300.0);
    assert_eq!(
        (pane.timeline_scroll_offset(), pane.timeline_wheel_target_px),
        (0.0, None)
    );
    // Replay after the first returned frame does flag live provenance, but
    // unchanged geometry does NOT reproduce scroll motion.
    pane.upsert_part_message(tool);
    pane.set_timeline_metrics(rect, 948.0, 300.0);
    assert_eq!(
        (pane.timeline_scroll_offset(), pane.timeline_wheel_target_px),
        (0.0, None)
    );
    // A genuinely fresh visible delta must retain the existing smooth follow.
    pane.apply_part_delta(Some("reply".into()), None, Some("text".into()), " More.");
    pane.set_timeline_metrics(rect, 972.0, 300.0);
    assert_eq!(
        (pane.timeline_scroll_offset(), pane.timeline_wheel_target_px),
        (24.0, Some(0.0))
    );
}

fn follow_scroll_test_pane() -> NeoismAgentPane {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 900.0, 300.0);
    pane.apply_part_delta(None, None, Some("text".into()), "live");
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1100.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 200.0);
    assert_eq!(
        pane.timeline_scroll_owner,
        TimelineScrollOwner::FollowBottom
    );
    pane
}

#[test]
fn follow_scroll_growth_preserves_absolute_top_velocity_clock_and_latch() {
    let mut pane = follow_scroll_test_pane();
    assert_eq!(pane.timeline_wheel_target_px, Some(0.0));
    let clock = pane.timeline_last_tick_at;
    pane.step_owned_timeline_spring(1.0 / 60.0);
    let lag = pane.timeline_scroll_offset();
    let velocity = pane.timeline_velocity_px_s;
    assert!(lag > 0.0 && lag < 200.0);
    assert!(velocity < 0.0);
    assert!(pane.timeline_follow_bottom);
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    assert!((pane.timeline_scroll_offset() - lag - 100.0).abs() < 0.001);
    assert_eq!(pane.timeline_velocity_px_s, velocity);
    assert_eq!(pane.timeline_last_tick_at, clock);
    assert_eq!(pane.timeline_wheel_target_px, Some(0.0));
    assert!(pane.timeline_follow_bottom);
    assert!(pane.timeline_is_inertial());
}

#[test]
fn follow_scroll_established_fit_then_first_overflow_is_not_initial_load() {
    let mut pane = NeoismAgentPane::default();
    assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 200.0, 300.0);
    assert!(!pane.timeline_is_inertial());
    pane.apply_part_delta(None, None, Some("text".into()), "live overflow");
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 400.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 100.0);
    assert_eq!(
        pane.timeline_scroll_owner,
        TimelineScrollOwner::FollowBottom
    );
    assert!(pane.timeline_follow_bottom);
}

#[test]
fn follow_scroll_large_extent_preserves_fractional_lag_and_settles() {
    let mut pane = NeoismAgentPane::default();
    let viewport = [0.0, 0.0, 400.0, 256.0];
    let height = 67_108_864.0;
    pane.set_timeline_metrics(viewport, height, 256.0);
    pane.apply_part_delta(None, None, Some("text".into()), "live");
    pane.set_timeline_metrics(viewport, height + 128.0, 256.0);
    assert!(pane.timeline_is_inertial());
    let mut frames = 0;
    while pane.timeline_is_inertial() && frames < 120 {
        pane.step_owned_timeline_spring(1.0 / 60.0);
        let lag = pane.timeline_scroll_offset();
        pane.set_timeline_metrics(viewport, height + 128.0, 256.0);
        assert_eq!(pane.timeline_scroll_offset(), lag);
        frames += 1;
    }
    assert!(
        frames < 120,
        "rounding must not keep the follow spring alive"
    );
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(pane.timeline_velocity_px_s, 0.0);
    assert_eq!(pane.timeline_last_tick_at, None);
}

#[test]
fn follow_scroll_idle_final_payload_settles_bounded_and_clears_clock() {
    let mut pane = follow_scroll_test_pane();
    // No streaming qualification: the final payload/status removal can precede paint.
    let mut frames = 0;
    while pane.timeline_is_inertial() && frames < 120 {
        pane.step_owned_timeline_spring(1.0 / 60.0);
        pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1100.0, 300.0);
        assert!(pane.timeline_follow_bottom);
        frames += 1;
    }
    assert!(frames < 120);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(pane.timeline_velocity_px_s, 0.0);
    assert_eq!(pane.timeline_wheel_target_px, None);
    assert_eq!(pane.timeline_last_tick_at, None);
    assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
    assert!(!pane.tick_timeline_scroll());
}

#[test]
fn follow_scroll_pixel_halfpage_wheel_and_selection_interrupt() {
    for input in 0..4 {
        let mut pane = follow_scroll_test_pane();
        pane.step_owned_timeline_spring(1.0 / 60.0);
        let before = pane.timeline_scroll_offset();
        match input {
            0 => {
                assert!(pane.scroll_timeline_pixels(10.0));
            }
            1 => {
                assert!(pane.scroll_timeline_half_page(true));
            }
            2 => {
                assert!(pane.scroll_timeline_wheel_pixels(10.0));
                assert!(
                    (pane.timeline_wheel_target_px.unwrap() - before - 10.0).abs()
                        < 0.001
                );
                assert_eq!(pane.timeline_velocity_px_s, 0.0);
            }
            _ => {
                assert!(pane.scroll_for_drag_edge(0.0));
            }
        }
        assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
        assert!(!pane.timeline_follow_bottom);
        let old = pane.timeline_scroll_offset();
        pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
        assert!((pane.timeline_scroll_offset() - old - 100.0).abs() < 0.001);
        assert!(!pane.timeline_follow_bottom);
    }
}

#[test]
fn follow_scroll_negative_manual_wheel_does_not_keep_automatic_ownership() {
    let mut pane = follow_scroll_test_pane();
    assert!(pane.scroll_timeline_wheel_pixels(-10.0));
    assert_eq!(pane.timeline_wheel_target_px, Some(190.0));
    assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
    assert!(!pane.timeline_follow_bottom);
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 300.0);
    assert_eq!(pane.timeline_wheel_target_px, Some(290.0));
}

#[test]
fn follow_scroll_initial_session_reset_and_historical_position_are_immediate() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1100.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(!pane.timeline_is_inertial());
    pane.apply_part_delta(None, None, Some("text".into()), "live");
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    assert!(pane.timeline_is_inertial());
    pane.reset_timeline_navigation_for_session_switch();
    pane.timeline_scroll_px = 250.0;
    pane.timeline_follow_bottom = false;
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 250.0);
    assert!(!pane.timeline_is_inertial());
    assert_eq!(pane.timeline_last_tick_at, None);
    pane.reset_timeline_navigation_for_session_switch();
    pane.timeline_scroll_px = 0.0;
    pane.timeline_follow_bottom = true;
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1500.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(!pane.timeline_is_inertial());
}

#[test]
fn follow_scroll_shrink_status_removal_resize_and_zero_range_are_valid() {
    let mut pane = follow_scroll_test_pane();
    pane.step_owned_timeline_spring(1.0 / 60.0);
    let top = pane.max_timeline_scroll() - pane.timeline_scroll_offset();
    // Remove the final status row while idle, then enlarge the viewport.
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1070.0, 300.0);
    assert!(
        (pane.max_timeline_scroll() - pane.timeline_scroll_offset() - top).abs() < 0.001
    );
    assert_eq!(pane.timeline_wheel_target_px, Some(0.0));
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 500.0], 1070.0, 500.0);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(!pane.timeline_is_inertial());
    assert!(pane.timeline_follow_bottom);
    pane.apply_part_delta(None, None, Some("text".into()), "live after resize");
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 500.0], 1200.0, 500.0);
    assert!(pane.timeline_is_inertial());
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 500.0], 100.0, 500.0);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(pane.timeline_last_tick_at, None);
    assert_eq!(pane.timeline_wheel_target_px, None);
    assert!(!pane.timeline_is_inertial());
}

#[test]
fn follow_scroll_prepend_and_logical_anchor_take_priority() {
    let mut pane = follow_scroll_test_pane();
    pane.pending_timeline_prepend_delta_px = Some(100.0);
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 200.0);
    assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
    assert!(!pane.timeline_is_inertial());
    assert!(!pane.timeline_follow_bottom);
    let mut pane = follow_scroll_test_pane();
    pane.pending_timeline_anchor = Some(TimelineAnchor {
        content_y: 500.0,
        screen_y: 100.0,
    });
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 500.0);
    assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
    assert!(!pane.timeline_is_inertial());
}

#[test]
fn follow_scroll_animation_optout_snaps_follow_but_preserves_manual_motion() {
    let mut pane = follow_scroll_test_pane();
    pane.set_text_reveal_enabled(false);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(pane.timeline_follow_bottom);
    assert!(!pane.timeline_is_inertial());
    assert_eq!(pane.timeline_last_tick_at, None);
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(!pane.timeline_is_inertial());
    assert!(pane.scroll_timeline_wheel_pixels(40.0));
    let target = pane.timeline_wheel_target_px;
    let clock = pane.timeline_last_tick_at;
    pane.set_text_reveal_enabled(false);
    assert_eq!(pane.timeline_wheel_target_px, target);
    assert_eq!(pane.timeline_last_tick_at, clock);
    assert!(pane.timeline_is_inertial());
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1300.0, 300.0);
    assert_eq!(pane.timeline_wheel_target_px, Some(140.0));
    assert!(!pane.timeline_follow_bottom);
}

#[test]
fn follow_scroll_ticks_and_metrics_do_not_impersonate_history_gestures() {
    let mut pane = follow_scroll_test_pane();
    pane.session_id = Some("follow-session".to_string());
    let timestamp = Instant::now() - std::time::Duration::from_secs(10);
    pane.timeline_last_scroll_at = Some(timestamp);
    pane.timeline_history.last_requested_session_id = Some("previous-page".to_string());
    pane.timeline_last_tick_at =
        Some(Instant::now() - std::time::Duration::from_millis(16));
    assert!(pane.tick_timeline_scroll());
    assert_eq!(pane.timeline_last_scroll_at, Some(timestamp));
    assert_eq!(
        pane.timeline_history.last_requested_session_id.as_deref(),
        Some("previous-page")
    );
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    assert_eq!(pane.timeline_last_scroll_at, Some(timestamp));
    assert_eq!(
        pane.timeline_history.last_requested_session_id.as_deref(),
        Some("previous-page")
    );
    // A visually lagging follower can be near the top, but isn't a history reader.
    pane.maybe_request_older_timeline_page(0.0, 300.0);
    assert!(!pane.timeline_history.loading_older);
    assert_eq!(
        pane.timeline_history.last_requested_session_id.as_deref(),
        Some("previous-page")
    );
    assert!(pane.timeline_follow_bottom);
    // User scrolling still timestamps and releases the history latch as before.
    assert!(pane.scroll_timeline_wheel_pixels(10.0));
    assert!(pane.timeline_last_scroll_at.unwrap() > timestamp);
    assert_eq!(pane.timeline_history.last_requested_session_id, None);
    pane.maybe_request_older_timeline_page(0.0, 300.0);
    assert!(pane.timeline_history.loading_older);
}

#[test]
fn timeline_source_revision_tracks_dirty_updates_without_full_layout_invalidation() {
    let mut pane = NeoismAgentPane::default();
    let epoch = pane.timeline_layout_epoch;
    let revision = pane.timeline_source_revision();
    pane.mark_timeline_message_dirty_at(0);
    assert_ne!(pane.timeline_source_revision(), revision);
    assert_eq!(pane.timeline_layout_epoch, epoch);
    let revision = pane.timeline_source_revision();
    pane.mark_timeline_message_and_next_dirty_at(0);
    assert_ne!(pane.timeline_source_revision(), revision);
    assert_eq!(pane.timeline_layout_epoch, epoch);
    let revision = pane.timeline_source_revision();
    pane.invalidate_timeline_layout();
    assert_ne!(pane.timeline_source_revision(), revision);
}

#[test]
fn timeline_scroll_rail_hover_redraws_only_visual_changes() {
    let mut pane = NeoismAgentPane::default();
    assert!(!pane.update_scrollbar_hover(Some((390.0, 100.0))));
    pane.set_scrollbar_geometry(
        Some([382.0, 99.0, 18.0, 102.0]), Some([382.0, 99.0, 18.0, 2.0]),
    );
    assert!(!pane.update_scrollbar_hover(Some((370.0, 100.0))));
    assert!(pane.update_scrollbar_hover(Some((390.0, 100.0))));
    assert!(pane.update_scrollbar_hover(Some((390.0, 150.0))));
    for i in 0..10_000 {
        assert!(!pane.update_scrollbar_hover(Some((385.0 + (i % 10) as f32, 150.0))));
    }
    // Real geometry changes under a stationary pointer still invalidate.
    pane.set_scrollbar_geometry(
        Some([382.0, 99.0, 18.0, 102.0]), Some([382.0, 109.0, 18.0, 2.0]),
    );
    assert!(pane.update_scrollbar_hover(Some((390.0, 150.0))));
    assert!(!pane.update_scrollbar_hover(Some((390.0, 150.0))));
    assert!(pane.update_scrollbar_hover(Some((370.0, 150.0))));
    assert!(!pane.update_scrollbar_hover(Some((370.0, 150.0))));
    assert!(pane.update_scrollbar_hover(Some((390.0, 150.0))));
    assert!(pane.update_scrollbar_hover(None)); // Native window leave.
    assert!(!pane.update_scrollbar_hover(None));
    assert!(pane.update_scrollbar_hover(Some((390.0, 150.0))));
    pane.set_scrollbar_geometry(None, None);
    assert!(pane.update_scrollbar_hover(Some((390.0, 150.0))));
    assert!(!pane.update_scrollbar_hover(Some((390.0, 150.0))));
}

#[test]
fn timeline_scroll_rail_click_and_drag_reach_both_history_endpoints() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1200.0, 300.0);
    pane.set_scrollbar_geometry(
        Some([382.0, 99.0, 18.0, 102.0]),
        Some([382.0, 199.0, 18.0, 2.0]),
    );
    assert!(!pane.begin_scrollbar_drag(390.0, 90.0));
    assert!(pane.begin_scrollbar_drag(390.0, 100.0));
    assert_eq!(pane.timeline_scroll_offset(), 900.0);
    assert!(!pane.timeline_follow_bottom);
    assert!(pane.scrollbar_dragging());
    assert!(pane.drag_scrollbar_to(390.0, 200.0));
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(pane.timeline_follow_bottom);
    assert!(pane.end_scrollbar_drag());
    assert!(!pane.scrollbar_dragging());
    assert!(!pane.drag_scrollbar_to(390.0, 100.0));
    assert!(pane.begin_scrollbar_drag(390.0, 150.0));
    assert_eq!(pane.timeline_scroll_offset(), 450.0);
    assert!(pane.end_scrollbar_drag());
}

#[test]
fn follow_scroll_scrollbar_drag_and_current_session_jump_are_explicit_overrides() {
    let mut pane = follow_scroll_test_pane();
    pane.set_scrollbar_geometry(
        Some([380.0, 0.0, 10.0, 300.0]),
        Some([380.0, 100.0, 10.0, 80.0]),
    );
    assert!(pane.begin_scrollbar_drag(385.0, 120.0));
    assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
    assert!(!pane.timeline_follow_bottom);
    assert!(!pane.timeline_is_inertial());
    assert_eq!(pane.timeline_last_tick_at, None);
    let mut pane = follow_scroll_test_pane();
    pane.session_id = Some("current".to_string());
    pane.side_panel
        .set_sessions(vec![NeoismAgentSessionEntry::new("current", "Chat", "now")]);
    pane.side_panel.set_selected(0);
    assert_eq!(
        pane.side_panel
            .selected_session()
            .map(|entry| entry.id.as_str()),
        Some("current")
    );
    pane.pending_timeline_anchor = Some(TimelineAnchor {
        content_y: 500.0,
        screen_y: 100.0,
    });
    pane.activate_side_panel_selection();
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(pane.timeline_scroll_owner, TimelineScrollOwner::Wheel);
    assert!(pane.timeline_follow_bottom);
    assert!(!pane.timeline_is_inertial());
    assert_eq!(pane.timeline_last_tick_at, None);
    assert!(pane.pending_timeline_anchor.is_none());
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1800.0, 300.0);
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(pane.timeline_wheel_target_px, None);
}

#[test]
fn follow_scroll_final_tick_does_not_update_user_timestamp() {
    let mut pane = follow_scroll_test_pane();
    pane.timeline_scroll_px = 0.2;
    pane.timeline_velocity_px_s = -5.0;
    let timestamp = Instant::now() - std::time::Duration::from_secs(10);
    pane.timeline_last_scroll_at = Some(timestamp);
    pane.timeline_last_tick_at =
        Some(Instant::now() - std::time::Duration::from_millis(16));
    assert!(pane.tick_timeline_scroll());
    assert!(!pane.timeline_is_inertial());
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(pane.timeline_last_scroll_at, Some(timestamp));
    assert_eq!(pane.timeline_last_tick_at, None);
}

#[test]
fn follow_scroll_initial_history_hydration_after_empty_paint_is_immediate() {
    for live_painted in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.session_id = Some("session".to_string());
        pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 100.0, 300.0);
        if live_painted {
            pane.messages
                .push(NeoismAgentMessage::assistant("live").with_id("loaded"));
        }
        pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
            "session",
            [AgentSessionUpdate::Messages {
                messages: vec![NeoismAgentMessage::assistant("history").with_id("loaded")],
                oldest_cursor: None,
            }],
        ));
        pane.drain_server_updates();
        assert_eq!(pane.messages.len(), 1);
        pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 1100.0, 300.0);
        assert_eq!(pane.timeline_scroll_offset(), 0.0);
        assert_eq!(pane.timeline_last_tick_at, None);
        assert!(!pane.timeline_is_inertial());
    }
}
