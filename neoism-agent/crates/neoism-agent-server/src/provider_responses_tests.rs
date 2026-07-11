use super::*;

#[test]
fn builds_streaming_responses_request_body() {
    let body = responses_request_body(
        "gpt-5.3-codex",
        Some("high"),
        &[
            ProviderMessage::text(ProviderRole::System, "You are concise."),
            ProviderMessage::text(ProviderRole::User, "Hello"),
            ProviderMessage::assistant_tool_call(
                "Hi",
                vec![neoism_agent_core::ProviderToolCall {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    input: json!({ "path": "README.md" }),
                }],
            ),
            ProviderMessage::tool_result("call_1", "read", "README contents", false),
        ],
        &[ToolListItem {
            id: "read".to_string(),
            description: "Read files".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                }
            }),
        }],
    );

    assert_eq!(body["model"], "gpt-5.3-codex");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert_eq!(body["instructions"], "You are concise.");
    assert_eq!(body["input"].as_array().unwrap().len(), 4);
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["input"][0]["content"][0]["text"], "Hello");
    assert_eq!(body["input"][1]["role"], "assistant");
    assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
    assert_eq!(body["input"][2]["type"], "function_call");
    assert_eq!(body["input"][2]["call_id"], "call_1");
    assert_eq!(body["input"][2]["name"], "read");
    assert_eq!(body["input"][2]["arguments"], "{\"path\":\"README.md\"}");
    assert_eq!(body["input"][3]["type"], "function_call_output");
    assert_eq!(body["input"][3]["call_id"], "call_1");
    assert_eq!(body["input"][3]["output"], "README contents");
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "read");
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["reasoning"]["summary"], "auto");
}

#[test]
fn responses_request_body_defaults_gpt5_reasoning_summary() {
    let body = responses_request_body(
        "gpt-5.5",
        None,
        &[ProviderMessage::text(ProviderRole::User, "Hello")],
        &[],
    );

    assert_eq!(body["reasoning"]["effort"], "medium");
    assert_eq!(body["reasoning"]["summary"], "auto");
}

#[test]
fn responses_request_body_always_has_instructions() {
    let body = responses_request_body(
        "gpt-5.5",
        None,
        &[ProviderMessage::text(ProviderRole::User, "Hello")],
        &[],
    );

    assert_eq!(
        body["instructions"],
        "You are Neoism, a concise coding assistant."
    );
    assert_eq!(body["input"][0]["role"], "user");
}

#[test]
fn responses_request_body_includes_image_and_pdf_attachments() {
    let mut message = ProviderMessage::text(ProviderRole::User, "Inspect attachments");
    message
        .attachments
        .push(neoism_agent_core::ProviderAttachment {
            mime: "image/png".to_string(),
            url: "data:image/png;base64,abc".to_string(),
            filename: Some("shot.png".to_string()),
        });
    message
        .attachments
        .push(neoism_agent_core::ProviderAttachment {
            mime: "application/pdf".to_string(),
            url: "data:application/pdf;base64,def".to_string(),
            filename: Some("report.pdf".to_string()),
        });

    let body = responses_request_body("gpt-5.5", None, &[message], &[]);
    let content = body["input"][0]["content"].as_array().unwrap();

    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "Inspect attachments");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["image_url"], "data:image/png;base64,abc");
    assert_eq!(content[2]["type"], "input_file");
    assert_eq!(content[2]["filename"], "report.pdf");
    assert_eq!(content[2]["file_data"], "data:application/pdf;base64,def");
}

#[test]
fn parses_responses_function_call_events() {
    let mut parser = ResponsesSseParser::default();
    assert!(parser
            .push_line(
                r#"data: {"type":"response.output_item.added","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read","arguments":""}}"#,
            )
            .unwrap()
            .is_empty());
    assert!(parser
            .push_line(
                r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"path\":\"READ"}"#,
            )
            .unwrap()
            .is_empty());
    let events = parser
            .push_line(
                r#"data: {"type":"response.output_item.done","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read","arguments":"{\"path\":\"README.md\"}"}}"#,
            )
            .unwrap();

    assert_eq!(events.len(), 1);
    match &events[0] {
        ProviderStreamEvent::ToolCall { id, name, input } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "read");
            assert_eq!(input["path"], "README.md");
        }
        other => panic!("expected tool call, got {other:?}"),
    }

    let finish = parser
        .push_line(
            r#"data: {"type":"response.completed","response":{"status":"completed"}}"#,
        )
        .unwrap();
    match &finish[0] {
        ProviderStreamEvent::FinishStep { finish, .. } => {
            assert_eq!(finish.as_deref(), Some("tool-calls"));
        }
        other => panic!("expected finish step, got {other:?}"),
    }
}

#[test]
fn preserves_raw_patch_arguments_as_patch_text() {
    let mut parser = ResponsesSseParser::default();
    assert!(parser
            .push_line(
                r#"data: {"type":"response.output_item.added","item":{"id":"fc_patch","type":"function_call","call_id":"call_patch","name":"apply_patch","arguments":""}}"#,
            )
            .unwrap()
            .is_empty());
    let events = parser
            .push_line(
                r#"data: {"type":"response.output_item.done","item":{"id":"fc_patch","type":"function_call","call_id":"call_patch","name":"apply_patch","arguments":"*** Begin Patch\n*** Update File: TASK.md\n@@\n-old\n+new\n*** End Patch"}}"#,
            )
            .unwrap();

    assert_eq!(events.len(), 1);
    match &events[0] {
        ProviderStreamEvent::ToolCall { id, name, input } => {
            assert_eq!(id, "call_patch");
            assert_eq!(name, "apply_patch");
            assert!(input["patchText"]
                .as_str()
                .unwrap()
                .contains("*** Update File: TASK.md"));
        }
        other => panic!("expected tool call, got {other:?}"),
    }
}

#[test]
fn parses_output_text_delta_event_and_data_lines() {
    let mut parser = ResponsesSseParser::default();

    assert!(parser
        .push_line("event: response.output_text.delta")
        .unwrap()
        .is_empty());
    let events = parser
        .push_line(r#"data: {"type":"response.output_text.delta","delta":"Hel"}"#)
        .unwrap();

    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], ProviderStreamEvent::TextStart { .. }));
    match &events[1] {
        ProviderStreamEvent::TextDelta { id, delta } => {
            assert_eq!(id, TEXT_ID);
            assert_eq!(delta, "Hel");
        }
        other => panic!("expected text delta, got {other:?}"),
    }
}

#[test]
fn parses_reasoning_delta_if_present() {
    let events = parse_responses_sse_line(
        r#"data: {"type":"response.reasoning_text.delta","delta":"thinking"}"#,
    )
    .unwrap();

    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        ProviderStreamEvent::ReasoningStart { .. }
    ));
    match &events[1] {
        ProviderStreamEvent::ReasoningDelta { id, delta } => {
            assert_eq!(id, REASONING_ID);
            assert_eq!(delta, "thinking");
        }
        other => panic!("expected reasoning delta, got {other:?}"),
    }
}

#[test]
fn parses_reasoning_summary_events_like_opencode() {
    let mut parser = ResponsesSseParser::default();
    let added = parser
            .push_line(
                r#"data: {"type":"response.output_item.added","output_index":0,"item":{"id":"rs_1","type":"reasoning","summary":[]}}"#,
            )
            .unwrap();

    assert_eq!(added.len(), 1);
    match &added[0] {
        ProviderStreamEvent::ReasoningStart { id } => assert_eq!(id, "rs_1:0"),
        other => panic!("expected reasoning start, got {other:?}"),
    }

    assert!(parser
        .push_line(
            r#"data: {"type":"response.reasoning_summary_part.added","item_id":"rs_1","summary_index":0}"#,
        )
        .unwrap()
        .is_empty());

    let delta = parser
        .push_line(
            r#"data: {"type":"response.reasoning_summary_text.delta","item_id":"rs_1","summary_index":0,"delta":"checking files"}"#,
        )
        .unwrap();

    assert_eq!(delta.len(), 1);
    match &delta[0] {
        ProviderStreamEvent::ReasoningDelta { id, delta } => {
            assert_eq!(id, "rs_1:0");
            assert_eq!(delta, "checking files");
        }
        other => panic!("expected reasoning delta, got {other:?}"),
    }

    let done = parser
            .push_line(
                r#"data: {"type":"response.output_item.done","output_index":0,"item":{"id":"rs_1","type":"reasoning","summary":[{"type":"summary_text","text":"checking files"}]}}"#,
            )
            .unwrap();

    assert_eq!(done.len(), 1);
    match &done[0] {
        ProviderStreamEvent::ReasoningEnd { id } => assert_eq!(id, "rs_1:0"),
        other => panic!("expected reasoning end, got {other:?}"),
    }
}

#[test]
fn parses_completed_with_usage_and_closes_open_parts() {
    let mut parser = ResponsesSseParser::default();
    let _ = parser
        .push_line(r#"data: {"type":"response.output_text.delta","delta":"ok"}"#)
        .unwrap();
    let _ = parser
        .push_line(r#"data: {"type":"response.reasoning_text.delta","delta":"why"}"#)
        .unwrap();

    let events = parser
            .push_line(
                r#"data: {"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":9,"output_tokens":4,"output_tokens_details":{"reasoning_tokens":2}}}}"#,
            )
            .unwrap();

    assert_eq!(events.len(), 4);
    assert!(matches!(events[0], ProviderStreamEvent::TextEnd { .. }));
    assert!(matches!(
        events[1],
        ProviderStreamEvent::ReasoningEnd { .. }
    ));
    match &events[2] {
        ProviderStreamEvent::FinishStep {
            finish,
            input_tokens,
            output_tokens,
            reasoning_tokens,
            ..
        } => {
            assert_eq!(finish.as_deref(), Some("stop"));
            assert_eq!(*input_tokens, 9);
            assert_eq!(*output_tokens, 4);
            assert_eq!(*reasoning_tokens, 2);
        }
        other => panic!("expected finish step, got {other:?}"),
    }
    assert!(matches!(events[3], ProviderStreamEvent::Finish { .. }));
}

#[test]
fn parses_incomplete_event_as_continuable_finish() {
    let events = parse_responses_sse_line(
        r#"data: {"type":"response.incomplete","response":{"status":"incomplete","usage":{"input_tokens":3,"output_tokens":2}}}"#,
    )
    .unwrap();

    match &events[0] {
        ProviderStreamEvent::FinishStep {
            finish,
            input_tokens,
            output_tokens,
            ..
        } => {
            assert_eq!(finish.as_deref(), Some("incomplete"));
            assert_eq!(*input_tokens, 3);
            assert_eq!(*output_tokens, 2);
        }
        other => panic!("expected finish step, got {other:?}"),
    }
}

#[test]
fn parses_failed_event_as_error() {
    let events = parse_responses_sse_line(
        r#"data: {"type":"response.failed","response":{"error":{"message":"quota exceeded"}}}"#,
    )
    .unwrap();

    assert_eq!(events.len(), 1);
    match &events[0] {
        ProviderStreamEvent::Error { message } => assert_eq!(message, "quota exceeded"),
        other => panic!("expected error, got {other:?}"),
    }
}

#[test]
fn ignores_unknown_events() {
    let mut parser = ResponsesSseParser::default();

    assert!(parser
        .push_line("event: response.created")
        .unwrap()
        .is_empty());
    assert!(parser
        .push_line(r#"data: {"type":"response.created","id":"resp_123"}"#)
        .unwrap()
        .is_empty());
    assert!(parser.push_line(": keep-alive").unwrap().is_empty());
}

#[test]
fn responses_request_body_ultra_maps_to_max_effort_on_gpt56() {
    // GPT-5.6 ultra rides the wire as effort "max" (Codex's Ultra => Max);
    // no `multi_agent` body param — the codex backend rejects it.
    let body = responses_request_body("gpt-5.6-sol", Some("ultra"), &[], &[]);
    assert_eq!(body["reasoning"]["effort"], serde_json::json!("max"));
    assert!(body.get("multi_agent").is_none());

    // Older models: ultra degrades to the deepest broadly-supported effort.
    let body = responses_request_body("gpt-5.5", Some("ultra"), &[], &[]);
    assert!(body.get("multi_agent").is_none());
    assert_eq!(body["reasoning"]["effort"], serde_json::json!("xhigh"));
}

#[test]
fn responses_request_body_ultra_injects_delegation_instructions() {
    let task_tool = ToolListItem {
        id: "task".to_string(),
        description: "spawn a sub-agent".to_string(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    };
    let body =
        responses_request_body("gpt-5.6-sol", Some("ultra"), &[], &[task_tool.clone()]);
    let instructions = body["instructions"].as_str().unwrap_or_default();
    assert!(instructions.contains("multi-agent delegation is active"));

    // Without the task tool there is nothing to delegate to — no injection.
    let body = responses_request_body("gpt-5.6-sol", Some("ultra"), &[], &[]);
    let instructions = body["instructions"].as_str().unwrap_or_default();
    assert!(!instructions.contains("multi-agent delegation"));

    // Non-ultra requests are untouched.
    let body = responses_request_body("gpt-5.6-sol", Some("high"), &[], &[task_tool]);
    let instructions = body["instructions"].as_str().unwrap_or_default();
    assert!(!instructions.contains("multi-agent delegation"));
}
