use super::*;
use neoism_agent_core::{CreateSessionRequest, ToolState};

async fn fixture() -> (AppState, SessionInfo, Arc<NativeProjector>, PathBuf) {
    let directory = std::env::temp_dir().join(format!("neoism-native-projection-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(&directory).unwrap();
    let state = AppState::open_database(directory.join("agent.sqlite3")).await.unwrap();
    let mut session = crate::session_routes::create_session_in_directory(&state, directory.to_str().unwrap(),
        CreateSessionRequest { parent_id: None, title: Some("Native projection test".into()),
            agent: None, model: None, permission: None, workspace_id: None,
            external_provider: None, external_options: None }, BTreeMap::new()).await.unwrap();
    session.extra.insert("externalAgent".into(), json!({"provider":"codex","runtime":"codex-app-server"}));
    state.inner.store.update_session(&session).await.unwrap();
    let step = start_assistant_step(&state, &session.id, session.id.as_str(), &Id::ascending(IdKind::Message),
        &session.directory, now_millis(), "native".into(), "codex".into(), String::new(), "codex".into(), false)
        .await.unwrap();
    let references = Arc::strong_count(&state.inner);
    let projector = NativeProjector::new(&state, &session, ExternalRuntime::Codex, &step, Arc::new(AtomicBool::new(false)));
    assert_eq!(references, Arc::strong_count(&state.inner), "warm registry must not retain AppState");
    (state, session, projector, directory)
}

#[tokio::test]
async fn mixed_stream_seals_boundaries_and_preserves_background_tools() {
    let (state, session, projector, directory) = fixture().await;
    for event in [
        json!({"type":"text.delta","id":"answer","text":"Before "}),
        json!({"type":"reasoning.delta","id":"thought","text":"Considering"}),
        json!({"type":"reasoning.completed","id":"thought"}),
        json!({"type":"tool.started","id":"exec","name":"bash","input":{"command":"pwd"}}),
        json!({"type":"tool.output","id":"exec","text":"initial\n","mode":"replace"}),
        json!({"type":"tool.output","id":"exec","text":"later\n","mode":"append"}),
        json!({"type":"text.delta","id":"answer","text":"after"}),
        json!({"type":"text.completed","id":"answer","text":"Before after"}),
        json!({"type":"model.updated","model":"confirmed-native-model","contextWindow":12345}),
        json!({"type":"usage.updated","usage":{"input":17,"output":9,"reasoning":3,"cacheRead":4}}),
    ] { projector.apply(&event).await.unwrap(); }
    projector.finish("completed").await.unwrap();
    assert!(projector.has_background_work().await);
    projector.apply(&json!({"type":"text.delta","id":"late","text":"MUST NOT LEAK"})).await.unwrap();
    projector.apply(&json!({"type":"tool.completed","id":"exec","output":"initial\nlater\n","exitCode":0})).await.unwrap();
    assert!(!projector.has_background_work().await);
    let response = projector.response(&external_model(ExternalRuntime::Codex), &json!({"finish":"completed"})).await;
    assert_eq!(response.text, "Before after");
    assert_eq!(response.model_id, "confirmed-native-model");
    assert_eq!((response.input_tokens, response.output_tokens, response.reasoning_tokens, response.cache_read_tokens), (17, 9, 3, 4));
    assert_eq!(response.total_tokens, None, "do not invent a provider total");
    let messages = state.inner.store.list_messages(session.id.as_str()).await.unwrap();
    let message = messages.last().unwrap();
    let tool = message.parts.iter().find_map(|p| match p { Part::Tool(t) => Some(t), _ => None }).unwrap();
    let ToolState::Completed { input, output, .. } = &tool.state else { panic!("tool not completed") };
    assert_eq!(input["command"], "pwd", "sparse completion retains input");
    assert_eq!(output, "initial\nlater\n");
    let path = tool.metadata.as_ref().unwrap()["outputPath"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "initial\nlater\n");
    assert_eq!(message.parts.iter().filter(|p| matches!(p, Part::Text(_))).count(), 2);
    state.shutdown().await.unwrap();
    drop(projector);
    drop(state);
    let reopened = AppState::open_database(directory.join("agent.sqlite3")).await.unwrap();
    let recovered = reopened.inner.store.list_messages(session.id.as_str()).await.unwrap();
    assert_eq!(serde_json::to_value(&recovered).unwrap(), serde_json::to_value(&messages).unwrap());
    reopened.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn native_children_are_persistent_noncontrollable_scopes() {
    let (state, session, projector, directory) = fixture().await;
    projector.apply(&json!({"type":"conversation.updated","sessionId":"real-root"})).await.unwrap();
    let native = json!({"id":"real-child-scope","parentId":"real-root","parentToolCallId":"spawn-1","title":"Investigate"});
    projector.apply(&json!({"type":"text.delta","scope":native,"id":"text","text":"child output"})).await.unwrap();
    let child_id = {
        let data = projector.data.lock().await;
        data.children["real-child-scope"].session.id.clone()
    };
    let child = state.inner.store.get_session(child_id.as_str()).await.unwrap().unwrap();
    assert_eq!(child.parent_id, Some(session.id.clone()));
    let metadata = &child.extra["externalAgent"];
    assert_eq!(metadata["nested"], true);
    assert_eq!(metadata["nativeProjection"], true);
    assert!(metadata.get("nativeSessionId").is_none(), "scope IDs must not become resumable IDs");
    assert!(!ExternalRuntime::Codex.owns_session(&child));
    projector.finish("completed").await.unwrap();
    assert!(projector.has_background_work().await);
    projector.apply(&json!({"type":"child.updated","scope":native,"status":"completed"})).await.unwrap();
    assert!(!projector.has_background_work().await);
    let saved = state.inner.store.get_session(session.id.as_str()).await.unwrap().unwrap();
    assert_eq!(saved.extra["externalAgent"]["nativeSessionId"], "real-root");
    assert!(projector.apply(&json!({"type":"conversation.updated","sessionId":"silently-created-root"})).await.is_err());
    state.shutdown().await.unwrap();
    drop(projector); drop(state);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn interruption_marks_unknown_outcome_but_accepts_late_confirmation() {
    let (state, _, projector, directory) = fixture().await;
    projector.apply(&json!({"type":"tool.started","id":"exec","name":"bash","input":{"command":"sleep 1"}})).await.unwrap();
    projector.finish("interrupted").await.unwrap();
    {
        let data = projector.data.lock().await;
        let message = data.root.live.lock().await;
        let tool = message.parts.iter().find_map(|p| match p { Part::Tool(t) => Some(t), _ => None }).unwrap();
        assert!(matches!(&tool.state, ToolState::Error { error, .. } if error.contains("not confirmed")));
        assert_eq!(tool.metadata.as_ref().unwrap()["outcomeUnconfirmed"], true);
    }
    projector.apply(&json!({"type":"tool.completed","id":"exec","output":"actually finished"})).await.unwrap();
    {
        let data = projector.data.lock().await;
        let message = data.root.live.lock().await;
        assert!(message.parts.iter().any(|p| matches!(p, Part::Tool(t) if matches!(t.state, ToolState::Completed { .. }))));
    }
    state.shutdown().await.unwrap();
    drop(projector); drop(state);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn spool_reconciles_snapshots_without_duplicates_and_has_bounded_preview() {
    let path = output_path("native-spool-test", Id::ascending(IdKind::Part).as_str());
    let initial = "初期\n".repeat(20_000);
    spool(path.clone(), initial.clone(), false).await.unwrap();
    spool(path.clone(), "tail\n".into(), true).await.unwrap();
    let expected = format!("{initial}tail\n");
    spool(path.clone(), expected.clone(), false).await.unwrap();
    // A sparse final aggregate must not erase bytes already claimed upstream.
    spool(path.clone(), initial, false).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    let preview = preview(path.clone()).await.unwrap();
    assert!(preview.len() < 66_000);
    assert!(preview.contains("full output:"));
    spool(path.clone(), "corrected native snapshot".into(), false).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "corrected native snapshot");
    let _ = std::fs::remove_file(path);
}

#[test]
fn invalid_scopes_cannot_fall_back_to_root() {
    assert!(scope_key(&json!({"title":"unattributed"})).is_err());
    assert_eq!(scope_key(&Value::Null).unwrap(), None);
    assert_eq!(scope_key(&json!({"parentToolCallId":"real-call"})).unwrap(), Some("tool:real-call".into()));
}

#[tokio::test]
async fn interactions_use_current_policy_and_questions_are_keyed_and_cancellable() {
    let (state, mut session, projector, directory) = fixture().await;
    session.permission = Some(vec![
        PermissionRule { permission: "edit".into(), pattern: "*".into(), action: PermissionAction::Allow },
        PermissionRule { permission: "edit".into(), pattern: "secret.txt".into(), action: PermissionAction::Deny },
        PermissionRule { permission: "bash".into(), pattern: "rm *".into(), action: PermissionAction::Deny },
    ]);
    state.inner.store.update_session(&session).await.unwrap();
    assert_eq!(projector.interaction("permission.request", &json!({"id":"approval","toolCallId":"native-call",
        "toolName":"fileChange","input":{"changes":[{"path":"public.txt"},{"path":"secret.txt"}]}})).await.unwrap()["decision"], "deny");
    assert_eq!(projector.interaction("permission.request", &json!({"id":"approval2",
        "toolName":"Bash","input":{"command":"rm secret.txt"}})).await.unwrap()["decision"], "deny");
    let inherited = json!({"id":"inherited-child","parentToolCallId":"spawn"});
    projector.apply(&json!({"type":"child.updated","scope":inherited,"status":"running"})).await.unwrap();
    session.permission.as_mut().unwrap().push(PermissionRule {
        permission: "read".into(), pattern: "private.txt".into(), action: PermissionAction::Deny,
    });
    state.inner.store.update_session(&session).await.unwrap();
    assert_eq!(projector.interaction("permission.request", &json!({"id":"current-parent-rule",
        "scope":inherited,"toolName":"Read","input":{"path":"private.txt"}})).await.unwrap()["decision"], "deny");
    let mut events = state.subscribe();
    let task_projector = projector.clone();
    let approval = tokio::spawn(async move {
        task_projector.interaction("permission.request", &json!({"id":"unknown-request","toolCallId":"actual-native-call",
            "toolName":"UnknownExecute","input":{"command":"do-something"}})).await
    });
    let permission_request = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == event_type::PERMISSION_ASKED {
                break serde_json::from_value::<neoism_agent_core::PermissionRequestInfo>(event.properties).unwrap();
            }
        }
    }).await.unwrap();
    assert_eq!(permission_request.session_id, session.id.to_string());
    assert_eq!(permission_request.tool.as_ref().unwrap()["callID"], "actual-native-call");
    let pending = state.inner.permission_waiters.write().await.remove(&permission_request.id).unwrap();
    pending.sender.send(Err("denied by user".into())).unwrap();
    assert_eq!(approval.await.unwrap().unwrap()["decision"], "deny");
    let request = json!({"id":"native-question","questions":[{"id":"first","question":"Which?"},{"id":"second","question":"Why?"}]});
    let task_projector = projector.clone();
    let task = tokio::spawn(async move { task_projector.interaction("question.request", &request).await });
    let question = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == event_type::QUESTION_ASKED { break serde_json::from_value::<neoism_agent_core::QuestionRequestInfo>(event.properties).unwrap(); }
        }
    }).await.unwrap();
    assert_eq!(question.session_id, session.id.to_string());
    let pending = state.inner.question_waiters.write().await.remove(&question.id).unwrap();
    pending.sender.send(Ok(vec![vec!["A".into()], vec!["Because".into()]])).unwrap();
    let answers = task.await.unwrap().unwrap();
    assert_eq!(answers, json!({"answers":{"first":["A"],"second":["Because"]}}));
    assert!(!state.inner.questions.read().await.contains_key(&question.id));
    let task_projector = projector.clone();
    let task = tokio::spawn(async move {
        task_projector.interaction("question.request", &json!({"id":"cancel-me","questions":[{"id":"q","question":"Wait?"}]})).await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if events.recv().await.unwrap().kind == event_type::QUESTION_ASKED { break; }
        }
    }).await.unwrap();
    projector.cancellation.store(true, Ordering::SeqCst);
    assert!(task.await.unwrap().is_err());
    assert!(state.inner.question_waiters.read().await.is_empty());
    assert!(state.inner.questions.read().await.is_empty());
    state.shutdown().await.unwrap();
    drop(projector); drop(state);
    let _ = std::fs::remove_dir_all(directory);
}