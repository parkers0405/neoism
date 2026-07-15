use super::*;

#[tokio::test]
async fn session_abort_cancels_active_run() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-abort-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id().to_string();
    let cancellation = Arc::new(AtomicBool::new(false));
    state.inner.runs.write().await.insert(
        session_id.clone(),
        SessionRun {
            id: "test-run".to_string(),
            started_at: 0,
            cancel: cancellation.clone(),
        },
    );
    let busy = busy_status(0, None);
    state
        .inner
        .statuses
        .write()
        .await
        .insert(session_id.clone(), busy);
    let app = app(state.clone());

    let cancelled: bool = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{session_id}/abort"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;

    assert!(cancelled);
    assert!(cancellation.load(Ordering::SeqCst));
    assert!(!state.inner.runs.read().await.contains_key(&session_id));
    assert!(!state.inner.statuses.read().await.contains_key(&session_id));
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn provider_event_poll_returns_when_cancelled_without_provider_event() {
    let cancellation = Arc::new(AtomicBool::new(false));
    let (_tx, rx) =
        tokio::sync::mpsc::unbounded_channel::<anyhow::Result<ProviderStreamEvent>>();
    let mut events: provider::ProviderEventStream =
        Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));
    let cancel = cancellation.clone();
    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.store(true, Ordering::SeqCst);
    });

    let poll = tokio::time::timeout(
        Duration::from_secs(1),
        next_provider_stream_event(&mut events, &cancellation, Duration::from_secs(1)),
    )
    .await
    .expect("provider event poll should observe cancellation");

    assert!(matches!(poll, ProviderEventPoll::Cancelled));
    cancel_task.await.unwrap();
}

#[tokio::test]
async fn provider_event_poll_times_out_without_provider_event() {
    let cancellation = Arc::new(AtomicBool::new(false));
    let (_tx, rx) =
        tokio::sync::mpsc::unbounded_channel::<anyhow::Result<ProviderStreamEvent>>();
    let mut events: provider::ProviderEventStream =
        Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));

    let poll =
        next_provider_stream_event(&mut events, &cancellation, Duration::from_millis(20))
            .await;

    assert!(matches!(poll, ProviderEventPoll::TimedOut));
}

#[tokio::test]
async fn session_abort_cancels_running_bash_tool() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-abort-bash-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session_id = neoism_agent_core::new_session_id();
    let cancellation = Arc::new(AtomicBool::new(false));
    state.inner.runs.write().await.insert(
        session_id.to_string(),
        SessionRun {
            id: "test-run".to_string(),
            started_at: 0,
            cancel: cancellation,
        },
    );
    let tool_state = state.clone();
    let tool_session_id = session_id.clone();
    let message_id = Id::ascending(IdKind::Message);
    let directory = root.to_string_lossy().to_string();
    let task = tokio::spawn(async move {
        execute_tool_call_with_permission_wait(
            &tool_state,
            &tool_session_id,
            &message_id,
            &directory,
            vec![PermissionRule {
                permission: "*".to_string(),
                pattern: "*".to_string(),
                action: PermissionAction::Allow,
            }],
            "call_bash_cancel",
            "bash",
            json!({
                "command": "printf started; sleep 30; printf finished",
                "description": "Cancelable bash",
                "timeout": 60_000,
            }),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    let cancelled: bool = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{session_id}/abort"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;

    assert!(cancelled);
    let error = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("bash tool should stop shortly after abort")
        .unwrap()
        .unwrap_err();
    assert!(error.contains("bash command aborted"), "{error}");
    assert!(error.contains("started"), "{error}");

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn prompt_async_queues_while_session_is_running() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-prompt-queue-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let run = SessionRun {
        id: "active-run".to_string(),
        started_at: 0,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    state
        .inner
        .runs
        .write()
        .await
        .insert(session.id.to_string(), run);

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/session/{}/prompt_async", session.id),
            Some(json!({
                "noReply": true,
                "parts": [{ "type": "text", "text": "queued turn" }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(
        queued_prompt_count(&state, session.id.as_str()).await,
        1,
        "queued prompt should stay visible while the active run is alive"
    );
    let statuses: HashMap<String, SessionStatus> = response_json(
        app.clone()
            .oneshot(request(Method::GET, "/session/status", None))
            .await
            .unwrap(),
    )
    .await;
    match statuses.get(session.id.as_str()) {
        Some(SessionStatus::Busy {
            queue:
                Some(SessionQueueStatus {
                    count: 1,
                    preview: Some(preview),
                }),
        }) => assert_eq!(preview, "queued turn"),
        other => panic!("expected busy queue status, got {other:?}"),
    }

    finish_session_run(&state, session.id.as_str(), "active-run").await;
    let messages = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let messages = state
                .inner
                .store
                .list_messages(session.id.as_str())
                .await
                .unwrap();
            if !messages.is_empty() {
                break messages;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();

    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].parts.first(),
        Some(Part::Text(TextPart { text, .. })) if text == "queued turn"
    ));
    assert_eq!(queued_prompt_count(&state, session.id.as_str()).await, 0);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let worker_done = !state
                .inner
                .prompt_queue_workers
                .read()
                .await
                .contains(session.id.as_str());
            let idle = !state
                .inner
                .statuses
                .read()
                .await
                .contains_key(session.id.as_str());
            if worker_done && idle {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn queued_prompt_can_be_appended_to_active_run() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-prompt-steer-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    state.inner.runs.write().await.insert(
        session.id.to_string(),
        SessionRun {
            id: "active-run".to_string(),
            started_at: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        },
    );

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/session/{}/prompt_async", session.id),
            Some(json!({
                "noReply": true,
                "parts": [{ "type": "text", "text": "steer this turn" }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(queued_prompt_count(&state, session.id.as_str()).await, 1);

    let drained = crate::session_queue::drain_queued_prompts_into_active_run(
        &state,
        session.id.as_str(),
    )
    .await;
    assert_eq!(drained, 1);
    assert_eq!(queued_prompt_count(&state, session.id.as_str()).await, 0);
    assert!(state
        .inner
        .runs
        .read()
        .await
        .contains_key(session.id.as_str()));

    let messages = state
        .inner
        .store
        .list_messages(session.id.as_str())
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].parts.first(),
        Some(Part::Text(TextPart { text, .. })) if text == "steer this turn"
    ));

    finish_session_run(&state, session.id.as_str(), "active-run").await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let worker_done = !state
                .inner
                .prompt_queue_workers
                .read()
                .await
                .contains(session.id.as_str());
            let idle = !state
                .inner
                .statuses
                .read()
                .await
                .contains_key(session.id.as_str());
            if worker_done && idle {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_queue_routes_inspect_pop_and_clear() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-queue-routes-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    state.inner.runs.write().await.insert(
        session.id.to_string(),
        SessionRun {
            id: "active-run".to_string(),
            started_at: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        },
    );

    for text in ["first queued turn", "second queued turn"] {
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{}/prompt_async", session.id),
                Some(json!({
                    "noReply": true,
                    "parts": [{ "type": "text", "text": text }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    let queue: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/session/{}/queue", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(queue["count"], 2);
    assert_eq!(queue["items"][0]["text"], "first queued turn");
    assert_eq!(queue["items"][1]["text"], "second queued turn");

    let popped: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{}/queue/pop", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(popped["removed"], 1);
    assert_eq!(popped["queue"]["count"], 1);
    assert_eq!(popped["queue"]["items"][0]["text"], "second queued turn");

    let cleared: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::DELETE,
                &format!("/session/{}/queue", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(cleared["removed"], 1);
    assert_eq!(cleared["queue"]["count"], 0);
    assert!(state
        .inner
        .runs
        .read()
        .await
        .contains_key(session.id.as_str()));

    finish_session_run(&state, session.id.as_str(), "active-run").await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let worker_done = !state
                .inner
                .prompt_queue_workers
                .read()
                .await
                .contains(session.id.as_str());
            let idle = !state
                .inner
                .statuses
                .read()
                .await
                .contains_key(session.id.as_str());
            if worker_done && idle {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn prompt_queue_survives_server_restart() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-queue-restart-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    state
        .inner
        .store
        .enqueue_prompt(
            session.id.as_str(),
            &PromptRequest {
                message_id: None,
                model: None,
                agent: None,
                no_reply: true,
                system: None,
                tools: None,
                parts: vec![PromptPart::Text {
                    text: "queued before restart".to_string(),
                }],
            },
        )
        .await
        .unwrap();
    state.inner.store.close().await;

    let restarted = AppState::open_database(db_path.clone()).await.unwrap();
    let messages = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let messages = restarted
                .inner
                .store
                .list_messages(session.id.as_str())
                .await
                .unwrap();
            if !messages.is_empty() {
                break messages;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();

    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].parts.first(),
        Some(Part::Text(TextPart { text, .. })) if text == "queued before restart"
    ));
    assert_eq!(
        queued_prompt_count(&restarted, session.id.as_str()).await,
        0
    );

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn prompt_returns_conflict_while_session_is_running() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-busy-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    state.inner.runs.write().await.insert(
        session.id.to_string(),
        SessionRun {
            id: "test-run".to_string(),
            started_at: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        },
    );

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/session/{}/message", session.id),
            Some(json!({
                "parts": [{ "type": "text", "text": "should conflict" }]
            })),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let messages: Vec<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/session/{}/message", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(messages.is_empty());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}
