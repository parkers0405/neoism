use super::*;

#[tokio::test]
async fn live_baseline_includes_active_children_and_rejects_stale_runtime() {
    let path = std::env::temp_dir().join(format!(
        "neoism-live-family-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let runtime = |revision, finished| {
        EventPayload::new(
            event_type::SESSION_EXECUTION_UPDATED,
            json!({
                "sessionID": "root", "runtime": {"rootSessionId": "root", "familyRevision": 1,
                    "execution": {"executionId": "run-1", "revision": revision, "finished": finished}, "branches": [{"sessionId": "child", "status": if finished {"completed"} else {"outstanding"}}]}
            }),
        )
    };
    state.publish(runtime(2, false));
    assert_eq!(
        state.subscribe_with_messages().1[0].properties["runtime"]["branches"][0]
            ["sessionId"],
        "child"
    );
    state.publish(runtime(3, true));
    state.publish(runtime(2, false));
    assert!(state.subscribe_with_messages().1.is_empty());
    state.publish(runtime(4, false));
    assert_eq!(state.subscribe_with_messages().1.len(), 1);
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn live_baseline_does_not_dedupe_completion_committed_during_replay() {
    let path = std::env::temp_dir().join(format!(
        "neoism-live-final-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session = neoism_agent_core::new_session_id();
    state
        .inner
        .store
        .insert_session(&store_test_session(&session, now_millis()))
        .await
        .unwrap();
    seed_live_message(&state, &session);
    live_delta(&state, &session, "b-text", "partial");
    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/v2/events?sessionId={session}&since=0"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut final_event = EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({
            "sessionID": session, "part": {"id": "b-text", "sessionId": session, "messageId": "live-message", "type": "text", "text": "final answer"}
        }),
    );
    state.allocate_event_sequence(&mut final_event);
    state.inner.store.append_event(&final_event).await.unwrap();
    state.publish_committed(final_event);
    let mut body = response.into_body().into_data_stream();
    let mut frames = String::new();
    for _ in 0..4 {
        let frame = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        frames.push_str(std::str::from_utf8(&frame).unwrap());
    }
    assert!(
        frames.find("partial").unwrap() < frames.find("final answer").unwrap(),
        "{frames}"
    );
    assert_eq!(frames.matches("final answer").count(), 1);
    drop(body);
    drop(state);
    cleanup_sqlite_files(&path);
}

fn seed_live_message(state: &AppState, session: &Id) {
    state.publish(EventPayload::new(event_type::MESSAGE_UPDATED, json!({
        "sessionID": session,
        "info": {"id": "live-message", "sessionId": session, "role": "assistant", "time": {"created": 1}}
    })));
    for (id, kind) in [("a-reasoning", "reasoning"), ("b-text", "text")] {
        state.publish(EventPayload::new(event_type::MESSAGE_PART_UPDATED, json!({
            "sessionID": session,
            "part": {"id": id, "sessionId": session, "messageId": "live-message", "type": kind, "text": "", "time": {"start": 1}}
        })));
    }
}

fn live_delta(state: &AppState, session: &Id, part: &str, delta: &str) {
    state.publish_live(EventPayload::new(event_type::MESSAGE_PART_DELTA, json!({
        "sessionID": session, "messageID": "live-message", "partID": part,
        "partType": if part == "b-text" {"text"} else {"reasoning"}, "field": "text", "delta": delta
    })));
}

#[tokio::test]
async fn live_baseline_and_subscription_partition_concurrent_tokens_exactly_once() {
    let path = std::env::temp_dir().join(format!(
        "neoism-live-baseline-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session = neoism_agent_core::new_session_id();
    seed_live_message(&state, &session);
    live_delta(&state, &session, "a-reasoning", "thought");
    let writer_state = state.clone();
    let writer_session = session.clone();
    let writer = std::thread::spawn(move || {
        for _ in 0..1000 {
            live_delta(&writer_state, &writer_session, "b-text", "x");
        }
    });
    let (mut receiver, baseline) = state.subscribe_with_messages();
    writer.join().unwrap();
    let mut text = baseline
        .iter()
        .find(|e| e.properties["part"]["id"] == "b-text")
        .unwrap()
        .properties["part"]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        baseline
            .iter()
            .find(|e| e.properties["part"]["id"] == "a-reasoning")
            .unwrap()
            .properties["part"]["text"],
        "thought"
    );
    assert!(baseline.iter().all(|e| e.sequence.is_none()));
    while let Ok(event) = receiver.try_recv() {
        assert_eq!(event.kind, event_type::MESSAGE_PART_DELTA);
        text.push_str(event.properties["delta"].as_str().unwrap());
    }
    assert_eq!(text, "x".repeat(1000));
    let (_, second) = state.subscribe_with_messages();
    assert_ne!(
        baseline[0].id, second[0].id,
        "reconnect baselines must bypass event-ID dedupe"
    );
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn live_baseline_honors_retry_reset_removal_and_completion() {
    let path = std::env::temp_dir().join(format!(
        "neoism-live-reset-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session = neoism_agent_core::new_session_id();
    seed_live_message(&state, &session);
    live_delta(&state, &session, "b-text", "abandoned attempt");
    seed_live_message(&state, &session);
    live_delta(&state, &session, "b-text", "fresh");
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_REMOVED,
        json!({
            "sessionID": session, "messageID": "live-message", "partID": "a-reasoning"
        }),
    ));
    let (_, baseline) = state.subscribe_with_messages();
    assert_eq!(baseline.len(), 2);
    assert_eq!(baseline[1].properties["part"]["text"], "fresh");
    state.publish(EventPayload::new(event_type::MESSAGE_UPDATED, json!({
        "sessionID": session,
        "info": {"id": "live-message", "sessionId": session, "role": "assistant", "time": {"created": 1, "completed": 2}}
    })));
    assert!(
        state.subscribe_with_messages().1.is_empty(),
        "completed text must not accumulate in RAM"
    );
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn live_baseline_precedes_new_tokens_on_scoped_sse_and_reconnect() {
    for resume in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "neoism-live-sse-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(path.clone()).await.unwrap();
        let session = neoism_agent_core::new_session_id();
        state
            .inner
            .store
            .insert_session(&store_test_session(&session, now_millis()))
            .await
            .unwrap();
        seed_live_message(&state, &session);
        live_delta(&state, &session, "b-text", "before join ");
        // Flush the durable writer before asking to replay, without persisting deltas.
        for _ in 0..100 {
            if state
                .inner
                .store
                .list_events_after(crate::state::TenantQueryScope::LocalAll, 0, 100, None)
                .await
                .unwrap()
                .len()
                >= 3
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v2/events?sessionId={}&{}",
                        session,
                        if resume { "since=0" } else { "tail=true" }
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        live_delta(&state, &session, "b-text", "after join");
        let mut body = response.into_body().into_data_stream();
        let mut text = String::new();
        for _ in 0..4 {
            let frame = tokio::time::timeout(Duration::from_secs(2), body.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            text.push_str(std::str::from_utf8(&frame).unwrap());
        }
        assert!(text.contains("before join "), "{text}");
        assert!(text.contains("after join"), "{text}");
        assert!(
            text.find("before join ").unwrap() < text.find("after join").unwrap(),
            "{text}"
        );
        assert_eq!(
            text.matches("\"type\":\"message.part.delta\"").count(),
            1,
            "{text}"
        );
        drop(body);
        drop(state);
        cleanup_sqlite_files(&path);
    }
}
