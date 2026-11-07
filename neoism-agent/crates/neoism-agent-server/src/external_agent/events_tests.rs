use super::*;

/// The fake subprocess cannot finish until live events and the reconnect
/// baseline have been checked. This catches final-output-only projections.
#[cfg(unix)]
#[tokio::test]
async fn fake_acp_child_streams_interleaved_transcript_before_completion() {
    use std::os::unix::fs::PermissionsExt;
    for runtime in [
        ExternalRuntime::Codex,
        ExternalRuntime::Claude,
        ExternalRuntime::OpenCode,
    ] {
        let root = std::env::temp_dir().join(format!(
            "neoism-acp-transcript-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("adapter");
        let release = root.join("release");
        let updates = vec![
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Checking "}}),
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"files."}}),
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Need "}}),
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"evidence."}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"read-1","kind":"read","title":"Read file","status":"in_progress","rawInput":{"path":"old"}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"read-1","content":[{"type":"content","content":{"type":"text","text":"file contents"}}]}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"read-1","status":"completed","rawInput":{"path":"README.md"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"exec-1","kind":"execute","status":"in_progress","content":[{"type":"content","content":{"type":"text","text":"CODEX_TOOL_BEGIN\n"}}]}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"exec-1","_meta":{"terminal_output":{"terminal_id":"exec-1","data":"CODEX_TOOL_MIDDLE\n"}}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"exec-1","_meta":{"terminal_output_delta":{"terminal_id":"exec-1","data":"CODEX_TOOL_END\n"}}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"exec-1","status":"completed","_meta":{"terminal_exit":{"terminal_id":"exec-1","exit_code":0,"signal":null}}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"raw-1","kind":"read","status":"in_progress","rawOutput":{"stdout":"ok"}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"raw-1","status":"completed"}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"exec-1","title":"Command done"}),
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Found "}}),
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"it."}}),
        ];
        let wire = updates.into_iter().map(|update| {
            format!("'{}'", json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fake","update":update}}))
        }).collect::<Vec<_>>().join(" ");
        std::fs::write(&script, format!(r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1}}}}' ;;
    *'"method":"session/new"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"fake"}}}}' ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' {wire}
      while test ! -f '{release}'; do sleep 0.01; done
      printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
      ;;
  esac
done
"#, release = release.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let mut services = crate::standard_services();
        services.executables = Arc::new(
            crate::executable::test_support::FakeExecutableService::with(
                if matches!(runtime, ExternalRuntime::OpenCode) {
                    "opencode"
                } else {
                    "npx"
                },
                script,
            ),
        );
        let db = root.join("agent.sqlite3");
        let state = AppState::open_database_with_services(&db, services)
            .await
            .unwrap();
        let request = |parent_id| neoism_agent_core::CreateSessionRequest {
            parent_id,
            title: None,
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
            external_provider: None,
            external_options: None,
        };
        let parent = crate::session_routes::create_session_in_directory(
            &state,
            root.to_str().unwrap(),
            request(None),
            BTreeMap::new(),
        )
        .await
        .unwrap();
        let mut child = lifecycle::create_external_subtask_session(
            &state,
            &parent,
            "inspect files",
            "Inspect files",
            runtime,
        )
        .await
        .unwrap();
        child.extra.get_mut("externalAgent").unwrap()["runtime"] = json!("acp");
        state.inner.store.update_session(&child).await.unwrap();
        let model = external_model(runtime);
        let user_id = Id::ascending(IdKind::Message);
        append_external_user_message(
            &state,
            &child,
            user_id.clone(),
            "inspect files",
            runtime,
            &model,
            None,
        )
        .await
        .unwrap();
        let step = start_assistant_step(
            &state,
            &child.id,
            child.id.as_str(),
            &user_id,
            &child.directory,
            now_millis(),
            "build".into(),
            runtime.provider_id().into(),
            model.model_id.clone(),
            model.provider_id.clone(),
            false,
        )
        .await
        .unwrap();
        let live = step.live_message.clone();
        let mut events = state.subscribe();
        let task_state = state.clone();
        let task_child = child.clone();
        let run = tokio::spawn(async move {
            let result = run_acp_prompt(
                &task_state,
                &task_child,
                "inspect files",
                runtime,
                &step,
                &model,
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
            finish_provider_stream_success(
                &task_state,
                &task_child.id,
                task_child.id.as_str(),
                &step.assistant_id,
                &step.text_part_id,
                &step.live_message,
                &model,
                result.provider_response,
                HashMap::new(),
                true,
            )
            .await
            .unwrap()
        });
        let mut observed = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = events.recv().await.unwrap();
                let done = event.kind == event_type::MESSAGE_PART_DELTA
                    && event.properties["delta"] == "it.";
                observed.push(event);
                if done {
                    break;
                }
            }
        })
        .await
        .expect("transcript did not stream while prompt was blocked");
        assert!(!run.is_finished());
        let before = live.lock().await.clone();
        assert!(
            matches!(&before.info, MessageInfo::Assistant(info) if info.time.completed.is_none())
        );
        let parts = before
            .parts
            .iter()
            .filter(|part| !matches!(part, Part::StepStart(_)))
            .collect::<Vec<_>>();
        assert!(matches!(parts[0], Part::Text(text) if text.text == "Checking files."));
        assert!(
            matches!(parts[1], Part::Reasoning(text) if text.text == "Need evidence." && text.time.end.is_some())
        );
        assert!(matches!(parts[2], Part::Tool(tool) if matches!(&tool.state,
            neoism_agent_core::ToolState::Completed { input, output, .. } if input["path"] == "README.md" && output == "file contents")));
        assert!(matches!(parts[3], Part::Tool(tool) if matches!(&tool.state,
            neoism_agent_core::ToolState::Completed { output, .. } if output.starts_with("CODEX_TOOL_BEGIN\nCODEX_TOOL_MIDDLE\nCODEX_TOOL_END\n") && output.contains("exit_code"))));
        assert!(matches!(parts[4], Part::Tool(tool) if matches!(&tool.state,
            neoism_agent_core::ToolState::Completed { output, .. } if output.contains("ok"))));
        assert!(matches!(parts[5], Part::Text(text) if text.text == "Found it."));
        assert!(observed
            .iter()
            .any(
                |event| event.properties["part"]["state"]["status"] == "running"
                    && event.properties["part"]["metadata"]["terminalOutput"]
                        == "CODEX_TOOL_BEGIN\nCODEX_TOOL_MIDDLE\nCODEX_TOOL_END\n"
            ));
        assert!(observed
            .iter()
            .any(|event| event.properties["partType"] == "reasoning"
                && event.properties["delta"] == "evidence."));
        assert!(observed
            .iter()
            .any(
                |event| event.properties["part"]["metadata"]["terminalOutput"]
                    == "file contents"
            ));
        assert!(observed
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
        let (_, baseline) = state.subscribe_with_messages();
        assert!(baseline
            .iter()
            .any(|event| event.properties["part"]["text"] == "Found it."));
        assert!(baseline
            .iter()
            .any(|event| event.properties["part"]["text"] == "Need evidence."));
        std::fs::write(&release, "finish").unwrap();
        let final_message = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .unwrap()
            .unwrap();
        state.shutdown().await.unwrap();
        drop(state);
        let reopened = AppState::open_database(&db).await.unwrap();
        let saved = reopened
            .inner
            .store
            .list_messages(child.id.as_str())
            .await
            .unwrap();
        assert_eq!(json!(saved.last().unwrap()), json!(final_message));
        assert!(final_message.parts.iter().any(|part| matches!(part, Part::Tool(tool)
            if matches!(&tool.state, neoism_agent_core::ToolState::Completed { output, .. }
                if output.starts_with("CODEX_TOOL_BEGIN\nCODEX_TOOL_MIDDLE\nCODEX_TOOL_END\n")))));
        let replay = reopened
            .inner
            .store
            .list_events_after(
                crate::state::TenantQueryScope::LocalAll,
                0,
                5000,
                Some(child.id.as_str()),
            )
            .await
            .unwrap();
        assert!(!replay
            .iter()
            .any(|event| event.payload.kind == event_type::MESSAGE_PART_DELTA));
        assert!(replay
            .iter()
            .any(
                |event| event.payload.properties["part"]["type"] == "reasoning"
                    && event.payload.properties["part"]["text"] == "Need evidence."
            ));
        reopened.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn nested_terminal_fragments_are_linear_and_completion_is_replayable() {
    check_nested_terminal_stream("completed", false).await;
    check_nested_terminal_stream("interrupted", false).await;
    check_nested_terminal_stream("completed", true).await;
}

async fn check_nested_terminal_stream(status: &str, oversized_first: bool) {
    let root = std::env::temp_dir().join(format!(
        "neoism-acp-stream-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join("agent.sqlite3");
    let state = AppState::open_database(&db).await.unwrap();
    let mut child = crate::session_routes::create_session_in_directory(
        &state,
        root.to_str().unwrap(),
        neoism_agent_core::CreateSessionRequest {
            parent_id: None,
            title: None,
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
            external_provider: None,
            external_options: None,
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    child.extra.insert(
        "externalAgent".into(),
        json!({"runtime":"acp", "provider":"codex", "status":"running", "nested":true}),
    );
    state.inner.store.update_session(&child).await.unwrap();
    append_external_user_message(
        &state,
        &child,
        Id::ascending(IdKind::Message),
        "task",
        ExternalRuntime::Codex,
        &external_model(ExternalRuntime::Codex),
        None,
    )
    .await
    .unwrap();
    let mut events = state.subscribe();
    let cap = crate::tool::truncate::MAX_BYTES;
    let chunk = if oversized_first {
        "界".repeat(cap)
    } else {
        "éx\n".repeat(128)
    };
    let mut expected = String::new();
    let mut baseline = String::new();
    let mut fragment_bytes = 0;
    let mut full_boundaries = 0;
    for _ in 0..if oversized_first { 4 } else { 128 } {
        project_nested_output(
            &state,
            ExternalRuntime::Codex,
            child.id.as_str(),
            &chunk,
            true,
        )
        .await
        .unwrap();
        expected.push_str(&chunk);
        while let Ok(event) = events.try_recv() {
            if event.kind == event_type::MESSAGE_PART_DELTA {
                assert_eq!(event.properties["field"], "text");
                assert_eq!(event.properties["delta"], chunk);
                fragment_bytes += event.properties["delta"].as_str().unwrap().len();
                baseline.push_str(event.properties["delta"].as_str().unwrap());
            } else if event.kind == event_type::MESSAGE_PART_UPDATED {
                full_boundaries += 1;
                baseline = event.properties["part"]["text"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                if full_boundaries == 1 {
                    assert_eq!(baseline, "");
                } else {
                    assert!(baseline.contains("preview truncated"));
                }
            }
            assert!(baseline.len() < cap + 1024);
        }
        // The actual daemon reconnect baseline must stay bounded too, not
        // just the wire events observed by this particular subscriber.
        let (_, live_replay) = state.subscribe_with_messages();
        for event in live_replay {
            if event.kind == event_type::MESSAGE_PART_UPDATED
                && event.properties["sessionID"] == child.id.as_str()
            {
                if let Some(text) = event.properties["part"]["text"].as_str() {
                    assert!(text.len() < cap + 1024);
                    assert_eq!(text, baseline);
                }
            }
        }
        // No growing prefix is written to the authoritative message per chunk.
        let saved = state
            .inner
            .store
            .list_messages(child.id.as_str())
            .await
            .unwrap();
        let assistant = saved
            .iter()
            .find(|m| matches!(m.info, MessageInfo::Assistant(_)))
            .unwrap();
        assert_eq!(assistant_text(assistant).unwrap(), "");
    }
    assert_eq!(fragment_bytes, if oversized_first { 0 } else { cap });
    // One durable empty boundary and exactly one live cap-crossing replacement,
    // not a full preview for every chunk after the cap.
    assert_eq!(full_boundaries, 2);
    let path = acp_output_path(child.id.as_str(), "nested-text");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    state.shutdown().await.unwrap();
    drop(state);
    // Reconnecting (including a server restart) recovers the exact spool, not
    // merely the last durable preview or an interruption diagnostic.
    let state = AppState::open_database(&db).await.unwrap();
    let diagnostic = if status == "completed" {
        ""
    } else {
        "ACP task interrupted"
    };
    if !diagnostic.is_empty() {
        expected.push_str(&format!("\n{diagnostic}"));
    }
    finish_nested_external_session(
        &state,
        ExternalRuntime::Codex,
        child.id.as_str(),
        status,
        diagnostic,
    )
    .await
    .unwrap();
    // Repeated settlement must not append diagnostics twice.
    finish_nested_external_session(
        &state,
        ExternalRuntime::Codex,
        child.id.as_str(),
        status,
        diagnostic,
    )
    .await
    .unwrap();
    state.shutdown().await.unwrap();
    drop(state);
    let reopened = AppState::open_database(&db).await.unwrap();
    let saved = reopened
        .inner
        .store
        .list_messages(child.id.as_str())
        .await
        .unwrap();
    let assistant = saved
        .iter()
        .find(|m| matches!(m.info, MessageInfo::Assistant(_)))
        .unwrap();
    let preview = assistant_text(assistant).unwrap();
    assert!(preview.contains("preview truncated"));
    assert!(preview.contains(path.to_str().unwrap()));
    assert!(preview.len() < crate::tool::truncate::MAX_BYTES + 1024);
    assert!(
        matches!(&assistant.info, MessageInfo::Assistant(info) if info.time.completed.is_some())
    );
    let replay = reopened
        .inner
        .store
        .list_events_after(
            crate::state::TenantQueryScope::LocalAll,
            0,
            5000,
            Some(child.id.as_str()),
        )
        .await
        .unwrap();
    assert!(!replay
        .iter()
        .any(|e| e.payload.kind == event_type::MESSAGE_PART_DELTA));
    let snapshots = replay
        .iter()
        .filter(|e| {
            e.payload.kind == event_type::MESSAGE_PART_UPDATED
                && e.payload.properties["part"]["type"] == "text"
        })
        .collect::<Vec<_>>();
    // User prompt, initial empty boundary, and a single final snapshot only.
    assert_eq!(snapshots.len(), 3);
    assert_eq!(
        snapshots.last().unwrap().payload.properties["part"]["text"],
        preview
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    reopened.shutdown().await.unwrap();
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn acp_spool_preview_is_utf8_bounded_and_missing_is_not_empty() {
    let session = Id::ascending(IdKind::Session);
    let path = acp_output_path(session.as_str(), "preview-test");
    assert!(acp_output_preview(path.clone()).await.unwrap().is_none());
    let text = "界".repeat(crate::tool::truncate::MAX_BYTES);
    write_acp_output(path.clone(), text.clone(), false)
        .await
        .unwrap();
    let preview = acp_output_preview(path.clone()).await.unwrap().unwrap();
    assert!(preview.contains("preview truncated"));
    assert!(preview.len() < crate::tool::truncate::MAX_BYTES + 1024);
    assert!(crate::tool::truncate::is_managed_output_path(
        path.to_str().unwrap()
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    std::fs::remove_file(path).unwrap();
}
