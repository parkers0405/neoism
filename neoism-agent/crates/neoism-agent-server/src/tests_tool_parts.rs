use super::*;

#[test]
fn command_template_expands_arguments_like_runtime_commands() {
    assert_eq!(
        expand_command_template("Review $1 then $2", "file.rs rest of args"),
        "Review file.rs then rest of args"
    );
    assert_eq!(
        expand_command_template("Review $ARGUMENTS", "one 'two three'"),
        "Review one 'two three'"
    );
    assert_eq!(
        expand_command_template("No placeholders", "extra args"),
        "No placeholders\n\nextra args"
    );
    assert_eq!(
        command_arguments("one 'two three' \"four five\" [Image 1]"),
        vec!["one", "two three", "four five", "[Image 1]"]
    );
}

#[test]
fn reasoning_part_mutation_helpers_update_text_and_time() {
    let session_id = Id::ascending(IdKind::Session);
    let message_id = Id::ascending(IdKind::Message);
    let part_id = Id::ascending(IdKind::Part);
    let mut parts = vec![Part::Reasoning(ReasoningPart {
        id: part_id.clone(),
        session_id,
        message_id,
        text: String::new(),
        time: PartTime {
            start: now_millis(),
            end: None,
        },
        metadata: None,
    })];

    append_text_delta(&mut parts, part_id.as_str(), "thinking");
    let part = finish_text_part(&mut parts, part_id.as_str(), None).unwrap();
    let Part::Reasoning(reasoning) = part else {
        panic!("expected reasoning part")
    };
    assert_eq!(reasoning.text, "thinking");
    assert!(reasoning.time.end.is_some());
}

#[test]
fn tool_part_mutation_helpers_track_call_lifecycle() {
    let session_id = Id::ascending(IdKind::Session);
    let message_id = Id::ascending(IdKind::Message);
    let part_id = Id::ascending(IdKind::Part);
    let mut parts = vec![Part::Tool(ToolPart {
        id: part_id.clone(),
        session_id: session_id.clone(),
        message_id: message_id.clone(),
        tool: "read".to_string(),
        call_id: "call_1".to_string(),
        state: ToolState::Pending {
            input: json!({}),
            raw: String::new(),
        },
        metadata: None,
    })];

    append_tool_input_delta(&mut parts, part_id.as_str(), r#"{"filePath":"src/lib.rs"}"#)
        .unwrap();
    let running = set_tool_running(
        &mut parts,
        part_id.clone(),
        &session_id,
        &message_id,
        "call_1".to_string(),
        "read".to_string(),
        json!({ "filePath": "src/lib.rs" }),
    );
    let Part::Tool(running) = running else {
        panic!("expected running tool part")
    };
    assert!(matches!(running.state, ToolState::Running { .. }));

    let completed = set_tool_completed(
        &mut parts,
        part_id.as_str(),
        "file contents".to_string(),
        "Read file".to_string(),
        json!({ "filePath": "src/lib.rs" }),
    )
    .unwrap();
    let Part::Tool(completed) = completed else {
        panic!("expected completed tool part")
    };
    let ToolState::Completed {
        output,
        title,
        time,
        ..
    } = completed.state
    else {
        panic!("expected completed tool state")
    };
    assert_eq!(output, "file contents");
    assert_eq!(title, "Read file");
    assert!(time.end.is_some());
}

#[test]
fn interrupted_tool_parts_preserve_input_and_set_metadata() {
    let session_id = Id::ascending(IdKind::Session);
    let message_id = Id::ascending(IdKind::Message);
    let part_id = Id::ascending(IdKind::Part);
    let input = json!({ "filePath": "src/lib.rs" });
    let mut parts = vec![Part::Tool(ToolPart {
        id: part_id.clone(),
        session_id,
        message_id,
        tool: "read".to_string(),
        call_id: "call_1".to_string(),
        state: ToolState::Running {
            input: input.clone(),
            time: PartTime {
                start: now_millis(),
                end: None,
            },
        },
        metadata: Some(json!({ "source": "test" })),
    })];

    let updated = mark_interrupted_tool_parts(&mut parts);

    assert_eq!(updated.len(), 1);
    let Part::Tool(tool) = &updated[0] else {
        panic!("expected interrupted tool part")
    };
    let ToolState::Error {
        input: error_input,
        error,
        time,
    } = &tool.state
    else {
        panic!("expected error state")
    };
    assert_eq!(error_input, &input);
    assert_eq!(error, "Tool execution aborted");
    assert!(time.end.is_some());
    assert_eq!(tool.metadata.as_ref().unwrap()["interrupted"], true);
    assert_eq!(tool.metadata.as_ref().unwrap()["source"], "test");
}

#[tokio::test]
async fn streamed_tool_call_executes_builtin_and_completes_part() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tool-call-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("input.txt"), "tool output").unwrap();
    let state = AppState::open_database(root.join("agent.sqlite3"))
        .await
        .unwrap();
    let session_id = Id::ascending(IdKind::Session);
    let message_id = Id::ascending(IdKind::Message);
    let part_id = Id::ascending(IdKind::Part);
    let mut parts = Vec::new();
    set_tool_running(
        &mut parts,
        part_id.clone(),
        &session_id,
        &message_id,
        "call_1".to_string(),
        "read".to_string(),
        json!({ "filePath": "input.txt" }),
    );

    let result = execute_tool_call(
        &state,
        root.to_str().unwrap(),
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: neoism_agent_core::PermissionAction::Allow,
        }],
        "read",
        json!({ "filePath": "input.txt" }),
    )
    .await
    .unwrap();
    let part = set_tool_completed(
        &mut parts,
        part_id.as_str(),
        result.output,
        result.title,
        result.metadata.unwrap(),
    )
    .unwrap();

    let Part::Tool(tool) = part else {
        panic!("expected tool part")
    };
    let ToolState::Completed { output, title, .. } = tool.state else {
        panic!("expected completed state")
    };
    assert!(title.contains("Read input.txt"));
    assert!(output.contains("1: tool output"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn streamed_tool_call_respects_wildcard_deny_permission() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tool-deny-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("input.txt"), "secret").unwrap();
    let state = AppState::open_database(root.join("agent.sqlite3"))
        .await
        .unwrap();

    let error = execute_tool_call(
        &state,
        root.to_str().unwrap(),
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: neoism_agent_core::PermissionAction::Deny,
        }],
        "read",
        json!({ "filePath": "input.txt" }),
    )
    .await
    .unwrap_err();
    assert!(error.contains("permission read"));
    assert!(error.contains("denied"));

    let _ = std::fs::remove_dir_all(root);
}
