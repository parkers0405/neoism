use super::*;

#[tokio::test]
async fn queue_snapshot_retries_after_a_complete_idle_run_idle_cycle() {
    let path = std::env::temp_dir().join(format!(
        "neoism-queue-snapshot-aba-{}.db", Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session: SessionInfo = response_json(
        app(state.clone()).oneshot(request(Method::POST, "/v2/sessions", Some(json!({}))))
            .await.unwrap(),
    ).await;
    let session_id = session.id.as_str();
    for text in ["primary", "waiting followup"] {
        let prompt: PromptRequest = serde_json::from_value(json!({
            "parts": [{"type": "text", "text": text}]
        })).unwrap();
        state.inner.store.enqueue_prompt_with_delivery(session_id, &prompt, "queue")
            .await.unwrap();
    }
    let (admission_id, primary, _, _) = state.inner.store
        .peek_prompt_admission(session_id, false).await.unwrap().unwrap();
    let (sampled_tx, sampled_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    state.inner.session_coordinator.queue_snapshot_gates.lock().await
        .insert(session_id.to_string(), (sampled_tx, resume_rx));
    let reader = {
        let state = state.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            crate::session_queue::session_queue(
                axum::extract::State(state), axum::extract::Path(session_id),
            ).await.unwrap().0
        })
    };
    tokio::time::timeout(Duration::from_secs(2), sampled_rx)
        .await.expect("reader must sample the original queue").unwrap();
    let before = state.inner.session_coordinator.queue_identity(session_id).await;
    assert!(before.0.is_none());
    // Complete the entire ownership cycle while the snapshot holds stale rows.
    // No worker is spawned, so finish removes the coordinator Entry entirely.
    let run = crate::session_run::start_session_run_for_prompt(
        &state, &session.id, primary.message_id.as_ref().map(ToString::to_string),
    ).await.unwrap();
    state.inner.store.handoff_prompt_admission(&admission_id, session_id, &run.id)
        .await.unwrap();
    crate::session_run::finish_session_run(&state, session_id, &run.id).await;
    let after = state.inner.session_coordinator.queue_identity(session_id).await;
    assert!(after.0.is_none());
    assert_ne!(before.2, after.2, "idle ABA must change the ownership generation");
    resume_tx.send(()).unwrap();
    let info = tokio::time::timeout(Duration::from_secs(2), reader)
        .await.expect("reader must retry without deadlocking").unwrap();
    let info = serde_json::to_value(info).unwrap();
    assert_eq!(info["running"], false);
    assert_eq!(info["count"], 1, "stale primary must not survive the retry");
    assert_eq!(info["preview"], "waiting followup");
    assert_eq!(info["items"].as_array().unwrap().len(), 1);
    state.inner.store.close().await;
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn status_hydration_includes_admitted_work_before_worker_start() {
    let path = std::env::temp_dir().join(format!(
        "neoism-admitted-status-{}.db", Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone().oneshot(request(
            Method::POST,
            "/v2/sessions",
            Some(json!({"model": {"providerId": "neoism", "id": "stub"}})),
        )).await.unwrap(),
    ).await;
    let session_id = session.id.as_str();
    let startup = state.inner.session_coordinator.prompt_admission_guard(session_id).await;
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        app.clone().oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{session_id}/prompt-async"),
            Some(json!({
                "model": {"providerId": "neoism", "modelId": "stub"},
                "parts": [{"type": "text", "text": "admitted before startup"}]
            })),
        )),
    ).await.expect("POST admission must not wait for worker startup").unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(state.inner.session_coordinator.active_run(session_id).await.is_none());
    // Check normal POST-success hydration, then missing and explicitly stale
    // Idle cache entries. Durable admission must win in all three cases.
    for cache in [None, Some(None), Some(Some(SessionStatus::Idle))] {
        if let Some(cache) = cache {
            let mut statuses = state.inner.statuses.write().await;
            if let Some(status) = cache {
                statuses.insert(session_id.to_string(), status);
            } else {
                statuses.remove(session_id);
            }
        }
        let statuses: serde_json::Value = response_json(
            app.clone().oneshot(request(Method::GET, "/v2/sessions/status", None))
                .await.unwrap(),
        ).await;
        assert_eq!(statuses[session_id]["type"], "busy");
        assert_eq!(statuses[session_id]["queue"]["count"], 1);
        assert_eq!(statuses[session_id]["queue"]["preview"], "admitted before startup");
    }
    drop(startup);
    wait_for_stub_queue_settlement(&state, session_id, 1).await;
    state.inner.store.close().await;
    cleanup_sqlite_files(&path);
}

/// Freeze the real startup boundary: ownership is claimed, but the durable
/// recovery row has not yet been handed off. No worker/provider timing involved.
#[tokio::test]
async fn queue_snapshot_excludes_only_the_claimed_primary_before_handoff() {
    async fn snapshot(state: &AppState, session_id: &str) -> serde_json::Value {
        let info = tokio::time::timeout(
            Duration::from_secs(2),
            crate::session_queue::session_queue(
                axum::extract::State(state.clone()),
                axum::extract::Path(session_id.to_string()),
            ),
        )
        .await
        .expect("snapshots must not reacquire admission locks")
        .unwrap();
        serde_json::to_value(info.0).unwrap()
    }

    let path = std::env::temp_dir().join(format!(
        "neoism-queue-snapshot-{}.db", Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.oneshot(request(Method::POST, "/v2/sessions", Some(json!({}))))
            .await.unwrap(),
    ).await;
    let session_id = session.id.as_str();
    // Legacy requests without a message ID receive one when peeked. Snapshot
    // identity must work for those too, not just IDs supplied by V2 clients.
    let primary: PromptRequest = serde_json::from_value(json!({
        "parts": [{"type": "text", "text": "same text"}]
    })).unwrap();
    state.inner.store.enqueue_prompt_with_delivery(session_id, &primary, "queue")
        .await.unwrap();
    let (admission_id, primary, _, _) = state.inner.store
        .peek_prompt_admission(session_id, false).await.unwrap().unwrap();
    assert_eq!(snapshot(&state, session_id).await["count"], 1);
    let mut events = state.subscribe();
    let run = crate::session_run::start_session_run_for_prompt(
        &state, &session.id, primary.message_id.as_ref().map(ToString::to_string),
    ).await.unwrap();
    assert_eq!(state.inner.store.queued_prompt_count(session_id).await.unwrap(), 1);
    let info = snapshot(&state, session_id).await;
    assert_eq!(info["running"], true);
    assert_eq!(info["count"], 0);
    assert!(info["preview"].is_null());
    while let Ok(event) = events.try_recv() {
        if event.kind == event_type::SESSION_STATUS {
            assert_eq!(event.properties["status"]["queue"]["count"], 0);
        }
    }

    // Distinct identities, even identical text, are genuine waiting followups.
    for delivery in ["queue", "steer"] {
        let followup: PromptRequest = serde_json::from_value(json!({
            "messageId": Id::ascending(IdKind::Message),
            "parts": [{"type": "text", "text": "same text"}]
        })).unwrap();
        state.inner.store.enqueue_prompt_with_delivery(session_id, &followup, delivery)
            .await.unwrap();
    }
    let prompt_guard = state.inner.session_coordinator.prompt_admission_guard(session_id).await;
    let execution_guard = crate::execution_activity::admission_guard(&state, session_id).await;
    let before = snapshot(&state, session_id).await;
    assert_eq!(before["count"], 2);
    assert_eq!(before["preview"], "same text");
    assert_eq!(before["items"].as_array().unwrap().len(), 2);
    state.inner.store.handoff_prompt_admission(&admission_id, session_id, &run.id)
        .await.unwrap();
    assert_eq!(snapshot(&state, session_id).await, before);
    drop(execution_guard);
    drop(prompt_guard);
    crate::session_run::finish_session_run(&state, session_id, &run.id).await;
    assert_eq!(snapshot(&state, session_id).await["count"], 2);
    state.inner.store.close().await;
    cleanup_sqlite_files(&path);
}

/// Exercise the actual queue worker and streaming provider, not noReply or a
/// synthetic installed run. The prompt lock lets the burst case commit both
/// requests before the worker can claim either one, without timing sleeps.
async fn stub_queue_rollover_after_settlement(burst: bool) {
    let path = std::env::temp_dir().join(format!(
        "neoism-stub-queue-rollover-{}.db",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/v2/sessions",
                Some(json!({
                    "model": {"providerId": "neoism", "id": "stub"}
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    // This is the same ordered broadcast consumed by the SSE routes.
    let mut events = state.subscribe();
    let first_id = Id::ascending(IdKind::Message).to_string();
    let post = |message_id: String, text: &'static str| {
        let app = app.clone();
        let uri = format!("/v2/sessions/{}/prompt-async", session.id);
        async move {
            let response = app
                .oneshot(request(
                    Method::POST,
                    &uri,
                    Some(json!({
"messageId": message_id,
                "model": {"providerId": "neoism", "modelId": "stub"},
                        "parts": [{"type": "text", "text": text}]
                    })),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
        }
    };
    post(first_id.clone(), "first rollover turn").await;
    let first = wait_for_stub_queue_settlement(&state, session.id.as_str(), 1).await;
    assert_eq!(first.root_message_id, first_id);

    let mut observed = Vec::new();
    while let Ok(event) = events.try_recv() {
        observed.push(event);
    }
    let dequeue = observed
        .iter()
        .position(|event| {
            event.kind == event_type::SESSION_QUEUE_UPDATED
                && event.properties["action"] == "dequeue"
                && event.properties["queue"]["count"] == 0
        })
        .expect("handoff must publish the empty queue");
    let reply = observed
        .iter()
        .position(|event| event.kind == event_type::MESSAGE_PART_DELTA)
        .expect("stub must stream a real provider reply");
    assert!(
        dequeue < reply,
        "queue must dequeue before provider reply, not after completion"
    );

    let second_id = Id::ascending(IdKind::Message).to_string();
    let third_id = Id::ascending(IdKind::Message).to_string();
    if burst {
        let admission = state
            .inner
            .session_coordinator
            .prompt_admission_guard(session.id.as_str())
            .await;
        post(second_id.clone(), "second rollover turn").await;
        post(third_id.clone(), "third rollover turn").await;
        assert_eq!(
            state
                .inner
                .store
                .queued_prompt_count(session.id.as_str())
                .await
                .unwrap(),
            2
        );
        drop(admission);
    } else {
        post(second_id.clone(), "second rollover turn").await;
    }
    let final_execution = wait_for_stub_queue_settlement(
        &state,
        session.id.as_str(),
        if burst { 3 } else { 2 },
    )
    .await;
    assert_ne!(
        first.execution_id, final_execution.execution_id,
        "a settled execution must roll over despite the retained admission row"
    );
    assert_eq!(final_execution.root_session_id, session.id.to_string());
    // Queued turns may share their newly admitted execution, but never the
    // already-finished first root.
    assert!(
        final_execution.root_message_id == second_id
            || (burst && final_execution.root_message_id == third_id)
    );
    let messages = state
        .inner
        .store
        .list_messages(session.id.as_str())
        .await
        .unwrap();
    for id in [&first_id, &second_id]
        .into_iter()
        .chain(burst.then_some(&third_id))
    {
        assert_eq!(messages.iter().filter(|m| {
            matches!(&m.info, MessageInfo::User(info) if info.id.to_string() == *id)
        }).count(), 1, "each distinct messageID must be persisted once");
    }
    state.inner.store.close().await;
    cleanup_sqlite_files(&path);
}

async fn wait_for_stub_queue_settlement(
    state: &AppState,
    session_id: &str,
    turns: usize,
) -> neoism_agent_core::ExecutionActivitySnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let messages = state.inner.store.list_messages(session_id).await.unwrap();
            let replies = messages.iter().filter(|m| {
                matches!(&m.info, MessageInfo::Assistant(_))
                    && m.parts.iter().any(|p| matches!(p, Part::Text(t) if t.text.contains("Received:")))
            }).count();
            let execution = state.inner.store.get_execution_activity(session_id).await.unwrap();
            if replies >= turns
                && execution.as_ref().is_some_and(|e| e.finished && e.active_segments.is_empty())
                && state.inner.store.queued_prompt_count(session_id).await.unwrap() == 0
                && state.inner.session_coordinator.active_run(session_id).await.is_none()
                && !state.inner.session_coordinator.worker_active(session_id).await
            {
                return execution.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("stub replies, finished execution, empty queue and no run/worker must all settle")
}

#[tokio::test]
async fn stub_queue_second_prompt_rolls_over_finished_execution() {
    stub_queue_rollover_after_settlement(false).await;
}

#[tokio::test]
async fn stub_queue_two_prompts_after_settlement_roll_over_finished_execution() {
    stub_queue_rollover_after_settlement(true).await;
}

/// A failure after the durable user insert must not lose the turn or turn an
/// exact retry into a no-op before execution initialization.
#[tokio::test]
async fn retained_admission_recovers_post_save_without_replaying_handoff() {
    let path = std::env::temp_dir().join(format!(
        "neoism-admission-{}.db",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/v2/sessions",
                Some(json!({"model": {"providerId": "neoism", "id": "stub"}})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let prompt: PromptRequest = serde_json::from_value(json!({
        "model": {"providerId": "neoism", "modelId": "stub"},
        "parts": [{"type": "text", "text": "recover durable admission"}]
    }))
    .unwrap();
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(session.id.as_str(), &prompt, "queue")
        .await
        .unwrap();
    let (id, queued, _, saved) = state
        .inner
        .store
        .peek_prompt_admission(session.id.as_str(), false)
        .await
        .unwrap()
        .unwrap();
    assert!(!saved);
    state
        .inner
        .session_coordinator
        .fail_prompt_after_save
        .lock()
        .await
        .insert(session.id.to_string());
    let mut admission_events = state.subscribe();
    let error = crate::session_prompt::append_prompt_admission(
        &state,
        session.id.as_str(),
        queued.clone(),
        true,
        Some((&id, saved, "queue")),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("injected post-save"));
    assert_eq!(
        state.inner.store.prompt_admission_saved(&id).await.unwrap(),
        Some(true),
        "the saved recovery token must survive until durable run handoff"
    );
    while let Ok(event) = admission_events.try_recv() {
        assert!(
            !(event.kind == event_type::SESSION_QUEUE_UPDATED
                && event.properties["action"] == "dequeue"),
            "a failed pre-handoff admission must not publish dequeue"
        );
    }
    assert!(state
        .inner
        .session_coordinator
        .active_run(session.id.as_str())
        .await
        .is_none());
    assert_eq!(
        state
            .inner
            .store
            .queued_prompt_count(session.id.as_str())
            .await
            .unwrap(),
        1
    );
    let (_, retry, _, saved) = state
        .inner
        .store
        .peek_prompt_admission(session.id.as_str(), false)
        .await
        .unwrap()
        .unwrap();
    assert!(
        saved,
        "user insert and recovery marker must commit together"
    );
    assert_eq!(retry.message_id, queued.message_id);
    drop(app);
    drop(state);
    let state = AppState::open_database(path.clone()).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        state
            .inner
            .session_coordinator
            .wait_until_settled(session.id.as_str()),
    )
    .await
    .unwrap();
    assert_eq!(
        state
            .inner
            .store
            .queued_prompt_count(session.id.as_str())
            .await
            .unwrap(),
        0
    );
    let messages = state
        .inner
        .store
        .list_messages(session.id.as_str())
        .await
        .unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|m| matches!(m.info, MessageInfo::User(_)))
            .count(),
        1
    );
    assert!(messages
        .iter()
        .any(|m| matches!(m.info, MessageInfo::Assistant(_))));
    // A new admission of the same message must dedupe, not restart generation.
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(session.id.as_str(), &retry, "queue")
        .await
        .unwrap();
    let (id, retry, _, saved) = state
        .inner
        .store
        .peek_prompt_admission(session.id.as_str(), false)
        .await
        .unwrap()
        .unwrap();
    assert!(!saved);
    crate::session_prompt::append_prompt_admission(
        &state,
        session.id.as_str(),
        retry,
        true,
        Some((&id, saved, "queue")),
    )
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
        messages.len()
    );
    assert_eq!(
        state
            .inner
            .store
            .queued_prompt_count(session.id.as_str())
            .await
            .unwrap(),
        0
    );
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn cancelled_admission_caller_cannot_strand_committed_work() {
    let path = std::env::temp_dir().join(format!(
        "neoism-admission-cancel-{}.db",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.oneshot(request(Method::POST, "/v2/sessions", Some(json!({}))))
            .await
            .unwrap(),
    )
    .await;
    let prompt: PromptRequest = serde_json::from_value(json!({
        "noReply": true, "parts": [{"type": "text", "text": "survive caller cancellation"}]
    })).unwrap();
    let (started, observed) = tokio::sync::oneshot::channel();
    state
        .inner
        .session_coordinator
        .prompt_admission_started
        .lock()
        .await
        .insert(session.id.to_string(), started);
    let writer = state.inner.store.lock_writer_for_test().await;
    let caller = tokio::spawn({
        let state = state.clone();
        let id = session.id.to_string();
        async move {
            crate::session_queue::enqueue_prompt_request_with_delivery(
                &state, &id, prompt, "queue",
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), observed)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(writer);
    tokio::time::timeout(Duration::from_secs(5), async {
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
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached admission must commit, wake and execute without its caller");
    tokio::time::timeout(
        Duration::from_secs(5),
        state
            .inner
            .session_coordinator
            .wait_until_settled(session.id.as_str()),
    )
    .await
    .unwrap();
    assert_eq!(
        state
            .inner
            .store
            .queued_prompt_count(session.id.as_str())
            .await
            .unwrap(),
        0
    );
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn duplicate_queued_submission_restarts_missing_worker() {
    let path = std::env::temp_dir().join(format!(
        "neoism-admission-wake-{}.db",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.oneshot(request(Method::POST, "/v2/sessions", Some(json!({}))))
            .await
            .unwrap(),
    )
    .await;
    let prompt: PromptRequest = serde_json::from_value(json!({
        "messageId": Id::ascending(IdKind::Message), "noReply": true,
        "parts": [{"type": "text", "text": "wake retained work"}]
    }))
    .unwrap();
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(session.id.as_str(), &prompt, "queue")
        .await
        .unwrap();
    assert!(
        !state
            .inner
            .session_coordinator
            .worker_active(session.id.as_str())
            .await
    );
    crate::session_queue::enqueue_prompt_request_with_delivery(
        &state,
        session.id.as_str(),
        prompt,
        "queue",
    )
    .await
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        state
            .inner
            .session_coordinator
            .wait_until_settled(session.id.as_str()),
    )
    .await
    .unwrap();
    assert_eq!(
        state
            .inner
            .store
            .queued_prompt_count(session.id.as_str())
            .await
            .unwrap(),
        0
    );
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
    cleanup_sqlite_files(&path);
}

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
async fn session_abort_requests_cancellation_without_releasing_active_run() {
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
    assert!(state
        .inner
        .session_coordinator
        .active_run(&session_id)
        .await
        .is_some());
    assert!(state.inner.statuses.read().await.contains_key(&session_id));

    // Key repeat or multiple attached clients may send abort more than once.
    // Cancellation remains idempotent while teardown still owns the run slot.
    let cancelled_again: bool = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{session_id}/abort"),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    assert!(cancelled_again);
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
    let session_id = session.id;
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
    // exists yet and the error can be just `command aborted`. If the command
    // did start, already-emitted output is kept.
    assert!(
        error.contains("started")
            || error.contains("(no output)")
            || error.eq_ignore_ascii_case("command aborted"),
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
    // Wait for durable consumption, including removing the queue entry. Allow
    // scheduler and storage contention when the complete suite runs together.
    let messages = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let messages = restarted
                .inner
                .store
                .list_messages(session.id.as_str())
                .await
                .unwrap();
            if !messages.is_empty()
                && queued_prompt_count(&restarted, session.id.as_str()).await == 0
            {
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

    restarted.shutdown().await.unwrap();
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
    // Hold parent queue ownership while inspecting admission, rather than
    // depending on projections delaying the worker's spawn.
    assert!(
        state
            .inner
            .session_coordinator
            .wake(parent.id.as_str())
            .await
    );
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

    // Hold parent queue ownership while inspecting admission, rather than
    // depending on projections delaying the worker's spawn.
    assert!(
        state
            .inner
            .session_coordinator
            .wake(parent.id.as_str())
            .await
    );
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
    // Hold parent queue ownership while inspecting admission, rather than
    // depending on projections delaying the worker's spawn.
    assert!(
        state
            .inner
            .session_coordinator
            .wake(parent.id.as_str())
            .await
    );
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
