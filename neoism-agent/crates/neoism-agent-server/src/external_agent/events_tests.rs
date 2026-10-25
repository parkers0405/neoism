use super::*;

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
