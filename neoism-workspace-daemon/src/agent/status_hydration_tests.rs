use super::*;
use events::SessionStatusEvidence;

fn busy() -> Value {
    json!({"root": {"type": "busy", "startedAt": 41}})
}

fn publish(
    evidence: &SessionStatusEvidence,
    tx: &AgentReplySender,
    generation: u64,
    statuses: Value,
) {
    if let Some(messages) = handlers::session_status_hydration_messages("root", &statuses) {
        evidence.publish_hydration(tx, "root", generation, messages);
    }
}

#[test]
fn old_busy_hydration_after_live_idle_is_suppressed() {
    let evidence = SessionStatusEvidence::default();
    let (tx, mut rx) = reply_queue::channel();
    // Capture before the REST fetch; hold its busy response until after SSE idle.
    let generation = evidence.begin_hydration("root");
    evidence.forward(&tx, "root", json!({
        "type": "session.idle", "properties": {"sessionID": "root"}
    }));
    publish(&evidence, &tx, generation, busy());
    assert!(matches!(rx.try_recv().unwrap(), AgentServerMessage::SessionIdle { .. }));
    assert!(rx.try_recv().is_err());
}

#[test]
fn old_idle_hydration_after_newer_live_busy_is_suppressed() {
    for statuses in [json!({}), json!({"root": {"type": "idle"}})] {
        let evidence = SessionStatusEvidence::default();
        let (tx, mut rx) = reply_queue::channel();
        let generation = evidence.begin_hydration("root");
        evidence.forward(&tx, "root", json!({
            "type": "session.status", "properties": {
                "sessionId": "root", "status": {"type": "busy", "startedAt": 42}
            }
        }));
        publish(&evidence, &tx, generation, statuses);
        assert!(matches!(rx.try_recv().unwrap(), AgentServerMessage::QueueUpdate {
            started_at: Some(42), ..
        }));
        assert!(rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn in_flight_status_fetch_cannot_overwrite_live_evidence() {
    for (snapshot, live_status) in [(busy(), "idle"), (json!({}), "busy")] {
        let fetched = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let fetched_handler = fetched.clone();
        let release_handler = release.clone();
        let app = axum::Router::new().route("/v2/health", axum::routing::get(|| async {
            axum::Json(json!({"healthy": true, "version": "test", "providerCredentialStore": "test"}))
        })).route("/v2/sessions/status", axum::routing::get(move || {
            let fetched = fetched_handler.clone();
            let release = release_handler.clone();
            let snapshot = snapshot.clone();
            async move {
                fetched.notify_one();
                release.notified().await;
                axum::Json(snapshot)
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (tx, mut rx) = reply_queue::channel();
        let mut session = AgentSession::spawn(Some("test".into()), String::new(), tx);
        let inner = Arc::get_mut(&mut session.inner).unwrap();
        inner.agent_server = format!("http://{address}");
        inner.agent_http = reqwest::Client::builder().no_proxy().build().unwrap();
        let inner = session.inner.clone();
        let hydration = tokio::spawn(async move {
            handlers::push_session_running_state(inner, "root".into()).await;
        });
        tokio::time::timeout(Duration::from_secs(5), fetched.notified()).await.unwrap();
        session.inner.status_evidence.forward(&session.inner.tx, "root", json!({
            "type": "session.status", "properties": {
                "sessionID": "root", "status": {"type": live_status, "startedAt": 42}
            }
        }));
        release.notify_one();
        hydration.await.unwrap();
        let live = rx.try_recv().unwrap();
        match live_status {
            "idle" => assert!(matches!(live, AgentServerMessage::SessionIdle { .. })),
            _ => assert!(matches!(live, AgentServerMessage::QueueUpdate { started_at: Some(42), .. })),
        }
        assert!(rx.try_recv().is_err(), "delayed REST snapshot must be suppressed");
        server.abort();
    }
}

#[test]
fn overlapping_hydration_only_latest_request_can_publish() {
    let evidence = SessionStatusEvidence::default();
    let (tx, mut rx) = reply_queue::channel();
    let old = evidence.begin_hydration("root");
    let new = evidence.begin_hydration("root");
    publish(&evidence, &tx, new, json!({}));
    publish(&evidence, &tx, old, busy());
    assert!(matches!(rx.try_recv().unwrap(), AgentServerMessage::SessionIdle { .. }));
    assert!(matches!(rx.try_recv().unwrap(), AgentServerMessage::QueueUpdate { started_at: None, .. }));
    assert!(rx.try_recv().is_err());
}

#[test]
fn live_parts_invalidate_only_their_source_session_hydration() {
    for source_field in ["sessionID", "sessionId"] {
        let evidence = SessionStatusEvidence::default();
        let (tx, mut rx) = reply_queue::channel();
        let root = evidence.begin_hydration("root");
        let child = evidence.begin_hydration("child");
        let mut properties = json!({"partID": "part", "messageID": "message", "delta": "text"});
        properties[source_field] = json!("child");
        evidence.forward(&tx, "root", json!({"type": "message.part.delta", "properties": properties}));
        // Discard the live part; its source is a child even on the root stream.
        while rx.try_recv().is_ok() {}
        evidence.publish_hydration(&tx, "child", child, vec![AgentServerMessage::SessionIdle {
            session_id: "child".into(),
        }]);
        assert!(rx.try_recv().is_err());
        publish(&evidence, &tx, root, busy());
        assert!(matches!(rx.try_recv().unwrap(), AgentServerMessage::QueueUpdate {
            session_id, started_at: Some(41), ..
        } if session_id == "root"));
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn live_parts_without_source_use_bound_session() {
    let evidence = SessionStatusEvidence::default();
    let (tx, mut rx) = reply_queue::channel();
    let generation = evidence.begin_hydration("root");
    evidence.forward(&tx, "root", json!({"type": "message.part.delta", "properties": {}}));
    while rx.try_recv().is_ok() {}
    publish(&evidence, &tx, generation, json!({}));
    assert!(rx.try_recv().is_err());
}

#[test]
fn malformed_hydration_never_invents_idle() {
    for statuses in [
        Value::Null, json!([]), json!("error"), json!({"root": null}),
        json!({"root": {}}), json!({"root": {"type": "unknown"}}),
        json!({"error": "not a status map"}), json!({"other": null}),
    ] {
        assert!(handlers::session_status_hydration_messages("root", &statuses).is_none());
    }
}

#[tokio::test]
async fn concurrent_stream_starts_keep_one_registered_task() {
    let (tx, _rx) = reply_queue::channel();
    let session = AgentSession::spawn(Some("test".into()), String::new(), tx);
    // This current-thread runtime cannot poll the network tasks while the
    // callers join, so all callers must observe the same live task identity.
    let runtime = tokio::runtime::Handle::current();
    let barrier = std::sync::Barrier::new(8);
    let ids = std::thread::scope(|scope| {
        let mut callers = Vec::new();
        for _ in 0..8 {
            let inner = &session.inner;
            let runtime = &runtime;
            let barrier = &barrier;
            callers.push(scope.spawn(move || {
                let _entered = runtime.enter();
                barrier.wait();
                events::start_event_stream(inner, "root");
                inner.stream_handles.lock().get("root").unwrap().id()
            }));
        }
        callers.into_iter().map(|caller| caller.join().unwrap()).collect::<Vec<_>>()
    });
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_eq!(session.inner.stream_handles.lock().len(), 1);
    events::stop_event_stream(&session.inner, "root");
}