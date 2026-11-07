#![cfg(not(windows))]
use super::*;

async fn fixture(action: &str) -> (AppState, SessionInfo, PathBuf) {
    let root = std::env::temp_dir().join(format!("neoism-shell-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(&root).unwrap();
    let state = AppState::open_database(root.join("agent.db")).await.unwrap();
    let session = response_json(app(state.clone()).oneshot(request(Method::POST,
        &format!("/v2/sessions?directory={}", root.display()),
        Some(json!({"permission":[{"permission":"bash","pattern":"*","action":action}]})),
    )).await.unwrap()).await;
    (state, session, root)
}

#[tokio::test]
async fn shell_executes_real_cwd_persists_tool_result_without_a_model_turn() {
    let (state, session, root) = fixture("allow").await;
    // This provider does not exist: direct shell must not resolve/invoke it.
    let body = json!({"messageId":"shell-once", "command":"printf 'real-shell-output\\n'; pwd", "model":{"providerId":"missing-provider","modelId":"not-a-model"}});
    let path = format!("/v2/sessions/{}/shell", session.id);
    let capability: Value = response_json(app(state.clone()).oneshot(request(Method::GET, &path, None)).await.unwrap()).await;
    assert_eq!(capability["execution"], "direct");
    assert!(state.inner.session_coordinator.active_run(session.id.as_str()).await.is_none());
    let response = app(state.clone()).oneshot(request(Method::POST, &path, Some(body.clone()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let message: MessageWithParts = response_json(response).await;
    let Part::Tool(tool) = &message.parts[0] else { panic!("expected normal tool block") };
    let neoism_agent_core::ToolState::Completed { output, metadata, .. } = &tool.state else { panic!("{:?}", tool.state) };
    assert_eq!(metadata["directShell"], true);
    assert_eq!(metadata["command"], body["command"]);
    assert_eq!(metadata["cwd"], root.to_str().unwrap());
    assert_eq!(metadata["exitCode"], 0);
    assert_eq!(tool.metadata.as_ref().unwrap()["directShell"], true);
    assert!(output.contains("real-shell-output"));
    assert!(output.contains(root.to_str().unwrap()), "{output}");
    assert_eq!(tool.tool, "bash");
    let stored = state.inner.store.get_message(session.id.as_str(), tool.message_id.as_str()).await.unwrap().unwrap();
    let context = crate::message_model::provider_messages(std::slice::from_ref(&stored));
    assert!(context.iter().any(|message| message.content.contains("real-shell-output")), "shell output must remain available to the next model turn");
    assert_eq!(serde_json::to_value(stored).unwrap(), serde_json::to_value(message).unwrap());
    assert!(state.inner.session_coordinator.active_run(session.id.as_str()).await.is_none());
    assert_eq!(app(state.clone()).oneshot(request(Method::POST, &path, Some(body))).await.unwrap().status(), StatusCode::CONFLICT);
    state.inner.store.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn shell_denial_never_executes_and_failures_are_normal_tool_errors() {
    let (state, session, root) = fixture("deny").await;
    let path = format!("/v2/sessions/{}/shell", session.id);
    let response = app(state.clone()).oneshot(request(Method::POST, &path, Some(json!({"command":"printf forbidden > forbidden.txt"})))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let message: MessageWithParts = response_json(response).await;
    assert!(matches!(&message.parts[0], Part::Tool(tool) if matches!(&tool.state, neoism_agent_core::ToolState::Error { .. })));
    let Part::Tool(denied) = &message.parts[0] else { panic!("expected tool") };
    assert_eq!(denied.metadata.as_ref().unwrap()["directShell"], true);
    assert_eq!(denied.metadata.as_ref().unwrap()["output"], "");
    assert!(denied.metadata.as_ref().unwrap().get("exitCode").is_none());
    assert!(!root.join("forbidden.txt").exists());
    assert!(state.inner.session_coordinator.active_run(session.id.as_str()).await.is_none());
    let mut session = session;
    session.permission.as_mut().unwrap()[0].action = neoism_agent_core::PermissionAction::Allow;
    state.inner.store.update_session(&session).await.unwrap();
    let message: MessageWithParts = response_json(app(state.clone()).oneshot(request(Method::POST, &path, Some(json!({"command":"printf 'expected-stderr' >&2; exit 7"})))).await.unwrap()).await;
    assert!(matches!(&message.parts[0], Part::Tool(tool) if matches!(&tool.state, neoism_agent_core::ToolState::Error { error, .. } if error.contains("expected-stderr") && error.contains("7"))));
    let Part::Tool(failed) = &message.parts[0] else { panic!("expected tool") };
    assert_eq!(failed.metadata.as_ref().unwrap()["exitCode"], 7);
    assert!(failed.metadata.as_ref().unwrap()["output"].as_str().unwrap().contains("expected-stderr"));
    let stored = state.inner.store.get_message(session.id.as_str(), failed.message_id.as_str()).await.unwrap().unwrap();
    assert_eq!(serde_json::to_value(stored).unwrap(), serde_json::to_value(message).unwrap());
    assert_eq!(app(state.clone()).oneshot(request(Method::POST, &path, Some(json!({"command":"  "})))).await.unwrap().status(), StatusCode::BAD_REQUEST);
    state.inner.store.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn shell_respects_busy_slot_and_abort_cancels_permission_wait() {
    let (state, session, root) = fixture("ask").await;
    let path = format!("/v2/sessions/{}/shell", session.id);
    let pending = {
        let state = state.clone(); let path = path.clone();
        tokio::spawn(async move { app(state).oneshot(request(Method::POST, &path, Some(json!({"command":"printf forbidden > forbidden.txt"})))).await.unwrap() })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.inner.permission_waiters.read().await.is_empty() { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.expect("permission prompt must appear before execution");
    assert!(!root.join("forbidden.txt").exists());
    let busy = app(state.clone()).oneshot(request(Method::POST, &path, Some(json!({"command":"printf duplicate"})))).await.unwrap();
    assert_eq!(busy.status(), StatusCode::CONFLICT);
    crate::session_actions::abort_session_run(&state, session.id.as_str()).await;
    let response = tokio::time::timeout(Duration::from_secs(10), pending).await.unwrap().unwrap();
    let message: MessageWithParts = response_json(response).await;
    assert!(matches!(&message.parts[0], Part::Tool(tool) if matches!(&tool.state, neoism_agent_core::ToolState::Error { .. })));
    assert!(!root.join("forbidden.txt").exists());
    assert!(state.inner.session_coordinator.active_run(session.id.as_str()).await.is_none());
    state.inner.store.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn shell_abort_stops_the_real_process_before_releasing_ownership() {
    let (state, session, root) = fixture("allow").await;
    let (mut events, _) = state.subscribe_with_messages();
    let path = format!("/v2/sessions/{}/shell", session.id);
    let pending = {
        let state = state.clone();
        tokio::spawn(async move {
            app(state).oneshot(request(Method::POST, &path,
                Some(json!({"command":"printf live-output; printf started > started.txt; sleep 30; printf forbidden > finished.txt"})),
            )).await.unwrap()
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while !root.join("started.txt").exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.expect("real process should start");
    let running = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            let part = &event.properties["part"];
            if part["state"]["status"] == "running" && part["metadata"]["output"] == "live-output" {
                break part.clone();
            }
        }
    }).await.expect("bounded output must stream before completion");
    assert_eq!(running["metadata"]["directShell"], true);
    assert_eq!(running["metadata"]["cwd"], root.to_str().unwrap());
    let stored = state.inner.store.get_message(session.id.as_str(), running["messageId"].as_str().unwrap()).await.unwrap().unwrap();
    let Part::Tool(stored_tool) = &stored.parts[0] else { panic!("expected tool") };
    assert_eq!(stored_tool.metadata.as_ref().unwrap()["directShell"], true);
    crate::session_actions::abort_session_run(&state, session.id.as_str()).await;
    let response = tokio::time::timeout(Duration::from_secs(10), pending).await.unwrap().unwrap();
    let message: MessageWithParts = response_json(response).await;
    assert!(matches!(&message.parts[0], Part::Tool(tool) if matches!(&tool.state, neoism_agent_core::ToolState::Error { .. })));
    let Part::Tool(tool) = &message.parts[0] else { panic!("expected tool") };
    assert_eq!(tool.metadata.as_ref().unwrap()["directShell"], true);
    assert_eq!(tool.metadata.as_ref().unwrap()["output"], "live-output");
    assert!(!root.join("finished.txt").exists());
    assert!(state.inner.session_coordinator.active_run(session.id.as_str()).await.is_none());
    state.inner.store.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn shell_retains_output_beyond_model_tool_summary_but_bounds_capture() {
    let (state, session, root) = fixture("allow").await;
    let path = format!("/v2/sessions/{}/shell", session.id);
    for exit in [0, 7] {
        let command = format!("awk 'BEGIN {{ for (i=0; i<3000; i++) print \"terminal-output-line\" }}'; printf stderr-tail >&2; exit {exit}");
        let message: MessageWithParts = response_json(app(state.clone()).oneshot(request(Method::POST, &path, Some(json!({"command":command})))).await.unwrap()).await;
        let Part::Tool(tool) = &message.parts[0] else { panic!("expected tool") };
        let output = match &tool.state {
            neoism_agent_core::ToolState::Completed { output, .. } => output.as_str(),
            neoism_agent_core::ToolState::Error { .. } => tool.metadata.as_ref().unwrap()["output"].as_str().unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(output.matches("terminal-output-line").count(), 3000);
        assert!(output.ends_with("stderr-tail"));
        assert_eq!(tool.metadata.as_ref().unwrap()["exitCode"], exit);
        let stored = state.inner.store.get_message(session.id.as_str(), tool.message_id.as_str()).await.unwrap().unwrap();
        assert_eq!(serde_json::to_value(stored).unwrap(), serde_json::to_value(message).unwrap());
    }
    let message: MessageWithParts = response_json(app(state.clone()).oneshot(request(Method::POST, &path,
        Some(json!({"command":"head -c 1100000 /dev/zero | tr '\\000' x"})),
    )).await.unwrap()).await;
    let Part::Tool(tool) = &message.parts[0] else { panic!("expected tool") };
    let neoism_agent_core::ToolState::Completed { output, metadata, .. } = &tool.state else { panic!("expected completed") };
    assert_eq!(output.bytes().filter(|byte| *byte == b'x').count(), 1024 * 1024);
    assert!(output.contains("1 MiB per-stream safety limit"));
    assert_eq!(metadata["captureTruncated"], true);
    state.inner.store.close().await;
    std::fs::remove_dir_all(root).unwrap();
}