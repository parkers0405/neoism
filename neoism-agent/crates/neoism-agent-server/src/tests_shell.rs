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
    let neoism_agent_core::ToolState::Completed { output, .. } = &tool.state else { panic!("{:?}", tool.state) };
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
    assert!(!root.join("forbidden.txt").exists());
    assert!(state.inner.session_coordinator.active_run(session.id.as_str()).await.is_none());
    let mut session = session;
    session.permission.as_mut().unwrap()[0].action = neoism_agent_core::PermissionAction::Allow;
    state.inner.store.update_session(&session).await.unwrap();
    let message: MessageWithParts = response_json(app(state.clone()).oneshot(request(Method::POST, &path, Some(json!({"command":"printf 'expected-stderr' >&2; exit 7"})))).await.unwrap()).await;
    assert!(matches!(&message.parts[0], Part::Tool(tool) if matches!(&tool.state, neoism_agent_core::ToolState::Error { error, .. } if error.contains("expected-stderr") && error.contains("7"))));
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
    let path = format!("/v2/sessions/{}/shell", session.id);
    let pending = {
        let state = state.clone();
        tokio::spawn(async move {
            app(state).oneshot(request(Method::POST, &path,
                Some(json!({"command":"printf started > started.txt; sleep 30; printf forbidden > finished.txt"})),
            )).await.unwrap()
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while !root.join("started.txt").exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.expect("real process should start");
    crate::session_actions::abort_session_run(&state, session.id.as_str()).await;
    let response = tokio::time::timeout(Duration::from_secs(10), pending).await.unwrap().unwrap();
    let message: MessageWithParts = response_json(response).await;
    assert!(matches!(&message.parts[0], Part::Tool(tool) if matches!(&tool.state, neoism_agent_core::ToolState::Error { .. })));
    assert!(!root.join("finished.txt").exists());
    assert!(state.inner.session_coordinator.active_run(session.id.as_str()).await.is_none());
    state.inner.store.close().await;
    std::fs::remove_dir_all(root).unwrap();
}