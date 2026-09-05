use super::*;

/// A normal root prompt may carry first-party per-turn system instructions
/// (for example an explicitly attached skill). Those instructions must not
/// make execution admission treat the prompt as a child/internal continuation.
#[tokio::test]
async fn root_prompt_with_skill_system_starts_model_generation() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-root-skill-prompt-{}",
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
                &format!("/v2/sessions?directory={}", root.display()),
                Some(json!({
                    "model": {
                        "providerId": "neoism",
                        "id": "stub"
                    }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;

    let response = app
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/prompt", session.id),
            Some(json!({
                "model": { "providerId": "neoism", "modelId": "stub" },
                "system": "The user selected these skills for this request. Load each selected skill with the skill tool before applying it:\n- neoism-yolo-release",
                "parts": [{ "type": "text", "text": "confirm skill prompt" }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let messages = state
                .inner
                .store
                .list_messages(session.id.as_str())
                .await
                .unwrap();
            if messages
                .iter()
                .any(|message| matches!(message.info, MessageInfo::Assistant(_)))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("root skill prompt should produce an assistant reply");

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

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
    state
        .inner
        .session_coordinator
        .install_run(
            &session_id.clone(),
            SessionRun {
                id: "test-run".to_string(),
                started_at: 0,
                cancel: cancellation.clone(),
            },
        )
        .await;
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
                &format!("/v2/sessions/{session_id}/abort"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;

    assert!(cancelled);
    assert!(cancellation.load(Ordering::SeqCst));
    assert!(!state
        .inner
        .session_coordinator
        .active_run(&session_id)
        .await
        .is_some());
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
    state
        .inner
        .session_coordinator
        .install_run(
            &session_id.to_string(),
            SessionRun {
                id: "test-run".to_string(),
                started_at: 0,
                cancel: cancellation,
            },
        )
        .await;
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
                &format!("/v2/sessions/{session_id}/abort"),
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
    assert!(
        error.to_ascii_lowercase().contains("command aborted"),
        "{error}"
    );
    // Cancellation may win while the process is starting (including during
    // one-time login-environment hydration), in which case no command output
    // exists yet. If the command did start, already-emitted output is kept.
    assert!(
        error.contains("started") || error.contains("(no output)"),
        "{error}"
    );
    assert!(!error.contains("finished"), "{error}");

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn prompt_async_preserves_the_sender_author_end_to_end() {
    // A guest sends a prompt carrying its presence-name `author`. The WHOLE
    // server chain — the canonical asynchronous prompt route →
    // enqueue (serde round-trip through the store) → drain → append_prompt →
    // persisted user message — must carry it, so a remote viewer renders the
    // true sender instead of "You". This is the headless proof of the server
    // half of the shared-author feature.
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-author-{}",
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
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/prompt-async", session.id),
            Some(json!({
                "noReply": true,
                "author": "piss-desktop",
                "parts": [{ "type": "text", "text": "hi from the peer" }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let user = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let messages = state
                .inner
                .store
                .list_messages(session.id.as_str())
                .await
                .unwrap();
            if let Some(user) = messages
                .into_iter()
                .find(|m| matches!(m.info, neoism_agent_core::MessageInfo::User(_)))
            {
                break user;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the user message should persist");

    // Assert the SERIALIZED JSON the frontend actually reads (`info.author`),
    // not just the in-memory struct — MessageInfo is `#[serde(tag = "role")]`
    // so UserMessage fields flatten to info's top level.
    let json = serde_json::to_value(&user).unwrap();
    assert_eq!(
        json["info"]["author"].as_str(),
        Some("piss-desktop"),
        "the persisted user message's info JSON (what the frontend reads) must carry the author: {json:#}"
    );

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
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
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
        .session_coordinator
        .install_run(&session.id.to_string(), run)
        .await;

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/prompt-async", session.id),
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
            .oneshot(request(Method::GET, "/v2/sessions/status", None))
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
                .session_coordinator
                .worker_active(session.id.as_str())
                .await;
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
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    state
        .inner
        .session_coordinator
        .install_run(
            &session.id.to_string(),
            SessionRun {
                id: "active-run".to_string(),
                started_at: 0,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )
        .await;

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/prompt", session.id),
            Some(json!({
                "delivery": "steer",
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
        .session_coordinator
        .active_run(session.id.as_str())
        .await
        .is_some());

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
                .session_coordinator
                .worker_active(session.id.as_str())
                .await;
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
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    state
        .inner
        .session_coordinator
        .install_run(
            &session.id.to_string(),
            SessionRun {
                id: "active-run".to_string(),
                started_at: 0,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )
        .await;

    for text in ["first queued turn", "second queued turn"] {
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/prompt-async", session.id),
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
                &format!("/v2/sessions/{}/queue", session.id),
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
                &format!("/v2/sessions/{}/queue/pop", session.id),
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
                &format!("/v2/sessions/{}/queue", session.id),
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
        .session_coordinator
        .active_run(session.id.as_str())
        .await
        .is_some());

    finish_session_run(&state, session.id.as_str(), "active-run").await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let worker_done = !state
                .inner
                .session_coordinator
                .worker_active(session.id.as_str())
                .await;
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
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(
            session.id.as_str(),
            &PromptRequest {
                message_id: None,
                model: None,
                agent: None,
                no_reply: true,
                system: None,
                tools: None,
                author: None,
                parts: vec![PromptPart::Text {
                    text: "queued before restart".to_string(),
                }],
            },
            "queue",
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
async fn prompt_steers_while_session_is_running() {
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
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    state
        .inner
        .session_coordinator
        .install_run(
            &session.id.to_string(),
            SessionRun {
                id: "test-run".to_string(),
                started_at: 0,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )
        .await;

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/prompt", session.id),
            Some(json!({
                "parts": [{ "type": "text", "text": "should conflict" }]
            })),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let messages: Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(messages.items.is_empty());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn prompt_message_ids_are_idempotent_and_conflict_on_reuse() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-idempotent-prompt-{}",
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
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let message_id = Id::ascending(IdKind::Message).to_string();
    let body = json!({
        "messageId": message_id,
        "noReply": true,
        "parts": [{ "type": "text", "text": "exactly once" }]
    });

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/prompt", session.id),
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if state
                .inner
                .store
                .list_messages(session.id.as_str())
                .await
                .unwrap()
                .len()
                == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        state
            .inner
            .store
            .list_messages(session.id.as_str())
            .await
            .unwrap()
            .len(),
        1
    );

    let conflict = app
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/prompt", session.id),
            Some(json!({
                "messageId": message_id,
                "noReply": true,
                "parts": [{ "type": "text", "text": "different" }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

/// Regression: a run claiming the session between the drain worker's pop and
/// its `append_prompt` (the user submits right as the queue drains) must NOT
/// lose the popped prompt — the exact window that silently dropped the last
/// subagent/background-task completion notification. The prompt goes back to
/// the FRONT of the durable queue and delivers once the run finishes.
#[tokio::test]
async fn run_conflict_mid_drain_requeues_prompt_instead_of_dropping_it() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-requeue-{}",
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
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let session_id = session.id.to_string();

    let completion_request = |text: &str| PromptRequest {
        message_id: None,
        model: None,
        agent: None,
        no_reply: false,
        system: None,
        tools: None,
        author: None,
        parts: vec![PromptPart::Text {
            text: text.to_string(),
        }],
    };
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(
            &session_id,
            &completion_request("subagent finished: last one"),
            "continue",
        )
        .await
        .unwrap();
    // Worker pops the completion (delivery tag comes back with it).
    let (popped, delivery) = state
        .inner
        .store
        .pop_queued_prompt_with_delivery(&session_id, None)
        .await
        .unwrap()
        .expect("queued completion");
    assert_eq!(delivery, "continue");

    // A run claims the session inside the pop->append window.
    state
        .inner
        .session_coordinator
        .install_run(
            &session_id.clone(),
            SessionRun {
                id: "user-turn".to_string(),
                started_at: 0,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )
        .await;
    let error = crate::session_prompt::append_prompt(
        &state,
        &session_id,
        popped.clone(),
        !popped.no_reply,
    )
    .await
    .expect_err("append during an active run must conflict");
    assert!(error.is_conflict());
    assert_eq!(
        error.to_string(),
        crate::session_prompt::SESSION_RUNNING_CONFLICT
    );

    // Requeue puts it back at the FRONT, ahead of the other queued prompt.
    state
        .inner
        .store
        .requeue_prompt_front_with_delivery(&session_id, &popped, &delivery)
        .await
        .unwrap();
    let entries = state
        .inner
        .store
        .list_queued_prompt_entries(&session_id)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(matches!(
        entries[0].0.parts.first(),
        Some(PromptPart::Text { text }) if text == "subagent finished: last one"
    ));
    assert_eq!(entries[0].1, "continue");

    // Run finishes; the requeued completion is the next prompt out, and a
    // plain append (no stub reply, so no provider round trip) lands it in
    // the transcript — nothing was lost.
    state.inner.session_coordinator.abort_run(&session_id).await;
    let (redelivered, redelivery) = state
        .inner
        .store
        .pop_queued_prompt_with_delivery(&session_id, None)
        .await
        .unwrap()
        .expect("requeued completion still queued");
    assert_eq!(redelivery, "continue");
    assert!(matches!(
        redelivered.parts.first(),
        Some(PromptPart::Text { text }) if text == "subagent finished: last one"
    ));
    crate::session_prompt::append_prompt(&state, &session_id, redelivered, false)
        .await
        .expect("append succeeds once the run is gone");
    let messages = state.inner.store.list_messages(&session_id).await.unwrap();
    assert!(messages.iter().any(|message| {
        matches!(message.info, MessageInfo::User(_))
            && matches!(
                message.parts.first(),
                Some(Part::Text(TextPart { text, .. })) if text == "subagent finished: last one"
            )
    }));
    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

/// Regression: a pending subagent-completion notification held at the moment
/// the last child finished (here: wedged behind a STALE Busy status for the
/// child — the old `parent_has_active_subtasks` consulted the derived
/// statuses map) must deliver when the PARENT's own run ends. Previously
/// only child lifecycle events re-attempted delivery, so this stranded
/// forever: sidebar showed complete, the main model was never told.
#[tokio::test]
async fn held_subtask_completion_delivers_when_parent_turn_ends() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-parent-reconcile-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let parent: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    // Finished child with a durable pending completion (the outbox entry
    // `mark_subtask_completion_pending` writes).
    let child_id = neoism_agent_core::new_session_id();
    let completion_id = Id::ascending(IdKind::Message);
    let mut child_extra = BTreeMap::new();
    child_extra.insert(
        "subtaskCompletions".to_string(),
        json!([{
            "id": completion_id,
            "pending": true,
            "status": "completed",
            "result": "the last subagent result",
            "completedAt": 42,
        }]),
    );
    child_extra.insert("subtaskPersistenceVersion".to_string(), json!(1));
    let child = SessionInfo {
        id: child_id.clone(),
        slug: "held-child".to_string(),
        project_id: parent.project_id.clone(),
        workspace_id: parent.workspace_id.clone(),
        directory: parent.directory.clone(),
        path: parent.path.clone(),
        parent_id: Some(parent.id.clone()),
        title: "Held child".to_string(),
        agent: Some("build".to_string()),
        model: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: 1,
            updated: 42,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: child_extra,
    };
    state.inner.store.insert_session(&child).await.unwrap();
    let workspace = crate::agent_tool_registry::acquire_workspace_plugin_snapshot(
        &state,
        &child.directory,
    )
    .await
    .unwrap();
    workspace
        .runtime
        .subagents()
        .unwrap()
        .track(child.id.to_string())
        .await;
    // The stale Busy status that used to wedge delivery forever.
    state
        .inner
        .statuses
        .write()
        .await
        .insert(child_id.to_string(), busy_status(1, None));

    // Parent finishes a turn (the user was talking to the main agent).
    state
        .inner
        .session_coordinator
        .install_run(
            &parent.id.to_string(),
            SessionRun {
                id: "parent-turn".to_string(),
                started_at: 0,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )
        .await;
    finish_session_run(&state, parent.id.as_str(), "parent-turn").await;

    // The held completion is now queued for the parent as a "continue"
    // notification carrying the child's result.
    let entries = state
        .inner
        .store
        .list_queued_prompt_entries(parent.id.as_str())
        .await
        .unwrap();
    assert_eq!(entries.len(), 1, "completion notification must be queued");
    assert_eq!(entries[0].1, "continue");
    assert!(matches!(
        entries[0].0.parts.first(),
        Some(PromptPart::Text { text }) if text.contains("the last subagent result")
    ));

    // Queue admission is not delivery: the durable outbox remains pending.
    let stored_child = state
        .inner
        .store
        .get_session(child_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_child
            .extra
            .get("subtaskCompletions")
            .and_then(serde_json::Value::as_array)
            .and_then(|records| records.first())
            .and_then(|record| record.get("pending"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

/// Regression for the deterministic missed notification: the task tool's
/// child-already-running branch QUEUES the continue-prompt ("wrap up") and
/// returns — no spawn wrapper exists on that path, so nothing ever published
/// the completion when the child finished. The child now carries a
/// notify-on-idle marker; its queue worker's exit (true idle) publishes the
/// completion through the standard outbox → the parent is notified.
#[tokio::test]
async fn queued_continue_prompt_notifies_parent_when_child_goes_idle() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-deferred-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let parent: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    let child_id = neoism_agent_core::new_session_id();
    let child = SessionInfo {
        id: child_id.clone(),
        slug: "queued-continue-child".to_string(),
        project_id: parent.project_id.clone(),
        workspace_id: parent.workspace_id.clone(),
        directory: parent.directory.clone(),
        path: parent.path.clone(),
        parent_id: Some(parent.id.clone()),
        title: "Wrap-up child".to_string(),
        agent: Some("build".to_string()),
        model: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: 1,
            updated: 1,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    state.inner.store.insert_session(&child).await.unwrap();

    // The task tool queues the steer prompt and sets the marker.
    let generation = Id::ascending(IdKind::Message);
    crate::session_actions::mark_subtask_notify_on_idle(
        &state,
        child_id.as_str(),
        &generation,
    )
    .await
    .unwrap();
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(
            child_id.as_str(),
            &PromptRequest {
                message_id: Some(generation),
                model: None,
                agent: None,
                no_reply: true,
                system: None,
                tools: None,
                author: None,
                parts: vec![PromptPart::Text {
                    text: "wrap up with what you have".to_string(),
                }],
            },
            "steer",
        )
        .await
        .unwrap();

    // The child's queue worker runs the prompt and exits at true idle.
    assert!(
        state
            .inner
            .session_coordinator
            .wake(child_id.as_str())
            .await
    );
    crate::session_queue::drain_prompt_queue(state.clone(), child_id.to_string()).await;

    // Parent got the completion notification.
    let entries = state
        .inner
        .store
        .list_queued_prompt_entries(parent.id.as_str())
        .await
        .unwrap();
    assert_eq!(entries.len(), 1, "parent must be notified");
    assert_eq!(entries[0].1, "continue");
    assert!(matches!(
        entries[0].0.parts.first(),
        Some(PromptPart::Text { text })
            if text.contains("Subagent finished.") && text.contains(child_id.as_str())
    ));

    // Marker clears at true idle, but queue admission does not acknowledge the
    // durable completion before append/model delivery succeeds.
    let stored_child = state
        .inner
        .store
        .get_session(child_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(stored_child.extra.get("subtaskNotifyOnIdle").is_none());
    assert_eq!(
        stored_child
            .extra
            .get("subtaskCompletions")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.first())
            .and_then(|value| value.get("pending"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn queued_child_prompt_supersedes_older_wrapper_and_notifies_at_final_idle() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-child-queue-race-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let parent: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let child_id = neoism_agent_core::new_session_id();
    let child = SessionInfo {
        id: child_id.clone(),
        slug: "queued-race-child".to_string(),
        project_id: parent.project_id.clone(),
        workspace_id: parent.workspace_id.clone(),
        directory: parent.directory.clone(),
        path: parent.path.clone(),
        parent_id: Some(parent.id.clone()),
        title: "Queued race child".to_string(),
        agent: Some("build".to_string()),
        model: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: 1,
            updated: 1,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    state.inner.store.insert_session(&child).await.unwrap();
    let queued_generation = Id::ascending(IdKind::Message);
    crate::session_actions::mark_subtask_notify_on_idle(
        &state,
        child_id.as_str(),
        &queued_generation,
    )
    .await
    .unwrap();
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(
            child_id.as_str(),
            &PromptRequest {
                message_id: Some(queued_generation),
                model: None,
                agent: None,
                no_reply: true,
                system: None,
                tools: None,
                author: None,
                parts: vec![PromptPart::Text {
                    text: "follow up before finishing".to_string(),
                }],
            },
            "steer",
        )
        .await
        .unwrap();
    assert!(
        state
            .inner
            .session_coordinator
            .wake(child_id.as_str())
            .await
    );

    // The first run's wrapper must not terminalize a child whose continuation
    // worker is already waiting.
    crate::session_actions::publish_background_subtask_finished(
        &state,
        child_id.as_str(),
        &Id::ascending(IdKind::Message),
        "completed",
        "first result, not final",
    )
    .await;
    let stored_child = state
        .inner
        .store
        .get_session(child_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(stored_child.extra.get("subtaskCompletion").is_none());
    assert!(state
        .inner
        .store
        .list_queued_prompt_entries(parent.id.as_str())
        .await
        .unwrap()
        .is_empty());

    crate::session_queue::drain_prompt_queue(state.clone(), child_id.to_string()).await;

    let queued = state
        .inner
        .store
        .list_queued_prompt_entries(parent.id.as_str())
        .await
        .unwrap();
    assert_eq!(queued.len(), 1, "parent must receive final completion");
    assert_eq!(queued[0].1, "continue");
    assert!(matches!(
        queued[0].0.parts.first(),
        Some(PromptPart::Text { text })
            if text.contains("Subagent finished.") && text.contains(child_id.as_str())
    ));

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}
