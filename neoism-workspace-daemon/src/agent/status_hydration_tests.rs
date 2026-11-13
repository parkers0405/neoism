use super::*;
use events::SessionStatusEvidence;

fn busy() -> Value {
    json!({"root": {"type": "busy", "startedAt": 41}})
}

#[tokio::test]
async fn catalog_activity_during_list_wins_but_later_list_can_recover_missed_events() {
    for (snapshot, live) in [
        ("running", "idle"),
        ("idle", "running"),
        ("running", "background"),
        ("running", "permission"),
    ] {
        let fetched = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let fetched_handler = fetched.clone();
        let release_handler = release.clone();
        let app = axum::Router::new().route("/v2/health", axum::routing::get(|| async {
            axum::Json(json!({"healthy":true,"version":"test","providerCredentialStore":"test"}))
        })).route("/v2/sessions", axum::routing::get(move || {
            let fetched = fetched_handler.clone();
            let release = release_handler.clone();
            async move {
                fetched.notify_one();
                release.notified().await;
                axum::Json(json!({"items":[
                    {"id":"root", "title":"Retained metadata", "catalogActivity":snapshot},
                    {"id":"other", "catalogActivity":"background"}
                ], "cursor":{"next":"next-page"}}))
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

        let result = tokio::time::timeout(Duration::from_secs(5), async {
            for in_flight_event in [true, false] {
                let inner = session.inner.clone();
                let list = tokio::spawn(async move {
                    handlers::fetch_thread_page(inner, None, None, Some("requested-page".into())).await;
                });
                fetched.notified().await;
                if in_flight_event {
                    session.inner.activity_evidence.forward(&session.inner.tx, json!({
                        "type":"session.catalog.activity",
                        "properties":{"sessionID":"root", "activity":live}
                    }));
                    assert!(matches!(rx.recv().await.unwrap(), AgentServerMessage::CatalogActivity {
                        session_id, activity
                    } if session_id == "root" && activity == live));
                }
                release.notify_one();
                list.await.unwrap();
                let AgentServerMessage::ThreadList { threads, requested_cursor, next_cursor } = rx.recv().await.unwrap() else {
                    panic!("expected catalog list");
                };
                let root = threads.iter().find(|thread| thread.session_id == "root").unwrap();
                let expected = if in_flight_event { live } else { snapshot };
                assert_eq!(root.catalog_activity.as_deref(), Some(expected));
                assert_eq!(root.busy, expected == "running");
                assert_eq!(root.title, "Retained metadata");
                assert_eq!(threads.iter().find(|thread| thread.session_id == "other").unwrap().catalog_activity.as_deref(), Some("background"));
                assert_eq!(requested_cursor.as_deref(), Some("requested-page"));
                assert_eq!(next_cursor.as_deref(), Some("next-page"));
                assert!(rx.try_recv().is_err());
            }
        }).await;
        server.abort();
        result.expect("catalog list race test timed out");
    }
}

#[test]
fn catalog_activity_newer_list_wins_per_row_over_older_response() {
    let evidence = events::CatalogActivityEvidence::default();
    let (tx, mut rx) = reply_queue::channel();
    let old = evidence.begin_list();
    let new = evidence.begin_list();
    let newer = handlers::thread_summaries_from_sessions(&json!({"items":[
        {"id":"root", "catalogActivity":"idle"},
        {"id":"other", "catalogActivity":"running"}
    ]}), 24);
    evidence.publish_list(&tx, new, newer, None, None);
    let _ = rx.try_recv().unwrap();
    let older = handlers::thread_summaries_from_sessions(&json!({"items":[
        {"id":"root", "catalogActivity":"running"},
        {"id":"other", "catalogActivity":"idle"},
        {"id":"older-page-only", "catalogActivity":"permission"}
    ]}), 24);
    evidence.publish_list(&tx, old, older, Some("page".into()), None);
    let AgentServerMessage::ThreadList { threads, requested_cursor, .. } = rx.try_recv().unwrap() else {
        panic!("expected catalog list");
    };
    for (id, expected) in [("root", "idle"), ("other", "running"), ("older-page-only", "permission")] {
        let row = threads.iter().find(|row| row.session_id == id).unwrap();
        assert_eq!(row.catalog_activity.as_deref(), Some(expected));
        assert_eq!(row.busy, expected == "running");
    }
    assert_eq!(requested_cursor.as_deref(), Some("page"));
}

#[tokio::test]
async fn switch_thread_initial_stream_retry_hydrates_background_without_blocking_output() {
    use axum::response::IntoResponse;
    use futures::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let attempts = Arc::new(AtomicUsize::new(0));
    let stream_attempts = attempts.clone();
    let runtime_attempts = attempts.clone();
    let status_fetched = Arc::new(tokio::sync::Notify::new());
    let release_status = Arc::new(tokio::sync::Notify::new());
    let fetched_handler = status_fetched.clone();
    let release_handler = release_status.clone();
    let app = axum::Router::new()
        .route("/v2/health", axum::routing::get(|| async {
            axum::Json(json!({"healthy":true,"version":"test","providerCredentialStore":"test"}))
        }))
        .route("/v2/sessions/root", axum::routing::get(|| async { axum::Json(json!({"id":"root"})) }))
        .route("/v2/events", axum::routing::get(move || {
            let attempt = stream_attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt == 0 {
                    return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                let events = [
                    json!({"type":"message.part.delta","data":{
                        "sessionID":"root","messageID":"message","partID":"part","field":"text","delta":"Live output"
                    }}),
                    json!({"type":"question.asked","data":{
                        "id":"live-question","sessionID":"root","questions":[{"question":"Proceed?"}]
                    }}),
                ].map(|value| Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(value.to_string())));
                axum::response::Sse::new(futures::stream::iter(events).chain(futures::stream::pending())).into_response()
            }
        }))
        .route("/v2/sessions/status", axum::routing::get(move || {
            let fetched = fetched_handler.clone();
            let release = release_handler.clone();
            async move {
                fetched.notify_one();
                release.notified().await;
                axum::Json(json!({}))
            }
        }))
        .route("/v2/sessions/root/runtime", axum::routing::get(move || {
            assert_eq!(runtime_attempts.load(Ordering::SeqCst), 2, "hydrate only after successful subscription");
            async {
                axum::Json(json!({
                    "rootSessionId":"root", "familyRevision":9, "branches":[],
                    "backgroundJobsEpoch":"server-a", "backgroundJobsRevision":7,
                    "runningBackgroundTasks":[{"jobId":"job-1","sessionId":"root","startedAt":123}]
                }))
            }
        }))
        .route("/v2/sessions/root/todos", axum::routing::get(|| async { axum::Json(json!([])) }))
        .route("/v2/interactions/questions", axum::routing::get(|| async { axum::Json(json!([])) }))
        .route("/v2/interactions/permissions", axum::routing::get(|| async { axum::Json(json!([])) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (tx, mut rx) = reply_queue::channel();
    let mut session = AgentSession::spawn(Some("test".into()), String::new(), tx);
    let inner = Arc::get_mut(&mut session.inner).unwrap();
    inner.agent_server = format!("http://{address}");
    inner.agent_http = reqwest::Client::builder().no_proxy().build().unwrap();
    dispatcher::dispatch(&session, AgentClientMessage::SwitchThread { session_id: "root".into() });

    let result = tokio::time::timeout(Duration::from_secs(5), async {
        status_fetched.notified().await;
        let (mut switched, mut output, mut runtime, mut todos, mut baseline, mut question) =
            (false, false, false, false, false, false);
        while !(switched && output && runtime && todos && question) {
            match rx.recv().await.unwrap() {
                AgentServerMessage::ThreadSwitched { session_id } => { assert_eq!(session_id, "root"); switched = true; }
                AgentServerMessage::ContentDelta { text, .. } => { assert_eq!(text, "Live output"); output = true; }
                AgentServerMessage::RuntimeSnapshot { session_id, snapshot } => {
                    assert_eq!(session_id, "root");
                    assert!(snapshot.branches_authoritative);
                    assert_eq!(snapshot.family_revision, 9);
                    assert_eq!(snapshot.background_jobs_epoch.as_deref(), Some("server-a"));
                    assert_eq!(snapshot.background_jobs_revision, Some(7));
                    let jobs = snapshot.running_background_tasks.unwrap();
                    assert_eq!(jobs.len(), 1, "restore the running background count without a live jobs event");
                    assert_eq!(jobs[0].job_id, "job-1");
                    runtime = true;
                }
                AgentServerMessage::TodoUpdate { todos: items, .. } => { assert!(items.is_empty()); todos = true; }
                AgentServerMessage::QuestionsUpdated { .. } => baseline = true,
                AgentServerMessage::QuestionAsked { request, .. } => {
                    assert!(baseline, "release live questions after interaction hydration, without waiting for status");
                    assert_eq!(request["id"], "live-question");
                    question = true;
                }
                _ => {}
            }
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }).await;
    release_status.notify_one();
    events::stop_event_stream(&session.inner, "root");
    server.abort();
    result.expect("initial retry must restore background count and forward output/interactions while status is pending");
}

#[tokio::test]
async fn attaching_stream_forwards_output_while_interactions_are_loading() {
    use futures::StreamExt;
    let fetched = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let fetched_handler = fetched.clone();
    let release_handler = release.clone();
    let app = axum::Router::new()
        .route("/v2/health", axum::routing::get(|| async {
            axum::Json(json!({"healthy":true,"version":"test","providerCredentialStore":"test"}))
        }))
        .route("/v2/events", axum::routing::get(|| async {
            let events = [
                json!({"type":"message.part.delta","data":{
                    "sessionID":"child","messageID":"message","partID":"part","field":"text","delta":"Already working"
                }}),
                json!({"type":"question.asked","data":{
                    "id":"live-question","sessionID":"root","questions":[{"question":"Proceed?"}]
                }}),
            ].map(|value| Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(value.to_string())));
            axum::response::Sse::new(futures::stream::iter(events).chain(futures::stream::pending()))
        }))
        .route("/v2/sessions/root/runtime", axum::routing::get(|| async {
            axum::Json(json!({"rootSessionId":"root","branches":[]}))
        }))
        .route("/v2/interactions/questions", axum::routing::get(move || {
            let fetched = fetched_handler.clone();
            let release = release_handler.clone();
            async move {
                fetched.notify_one();
                release.notified().await;
                axum::Json(json!([]))
            }
        }))
        .route("/v2/interactions/permissions", axum::routing::get(|| async { axum::Json(json!([])) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (tx, mut rx) = reply_queue::channel();
    let mut session = AgentSession::spawn(Some("test".into()), String::new(), tx);
    let inner = Arc::get_mut(&mut session.inner).unwrap();
    inner.agent_server = format!("http://{address}");
    inner.agent_http = reqwest::Client::builder().no_proxy().build().unwrap();
    events::start_event_stream(&session.inner, "root");
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        fetched.notified().await;
        loop {
            if let AgentServerMessage::ContentDelta { session_id, text, .. } = rx.recv().await.unwrap() {
                assert_eq!(session_id, "child");
                assert_eq!(text, "Already working");
                break;
            }
        }
        release.notify_one();
        let mut snapshot = false;
        loop {
            match rx.recv().await.unwrap() {
                AgentServerMessage::QuestionsUpdated { requests, .. } => { assert!(requests.is_empty()); snapshot = true; }
                AgentServerMessage::QuestionAsked { request, .. } => {
                    assert!(snapshot, "replay live interactions after their baseline, not before it");
                    assert_eq!(request["id"], "live-question");
                    break;
                }
                _ => {}
            }
        }
    }).await;
    events::stop_event_stream(&session.inner, "root");
    server.abort();
    result.expect("live child output must not wait for pending interaction HTTP reads");
}

#[tokio::test]
async fn reconnect_recovers_family_questions_empty_snapshots_and_permissions() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let connections = Arc::new(AtomicUsize::new(0));
    let stream_connections = connections.clone();
    let question_connections = connections.clone();
    let app = axum::Router::new()
        .route("/v2/health", axum::routing::get(|| async {
            axum::Json(json!({"healthy":true,"version":"test","providerCredentialStore":"test"}))
        }))
        .route("/v2/events", axum::routing::get(move || {
            stream_connections.fetch_add(1, Ordering::SeqCst);
            async { ([("content-type", "text/event-stream")], ": connected\n\n") }
        }))
        .route("/v2/sessions/status", axum::routing::get(|| async { axum::Json(json!({})) }))
        .route("/v2/sessions/root/runtime", axum::routing::get(|| async {
            axum::Json(json!({"rootSessionId":"root","branches":[
                {"sessionId":"child","parentSessionId":"root","status":"blocked"},
                {"sessionId":"invalid","parentSessionId":"root","status":"blocked"}
            ]}))
        }))
        .route("/v2/sessions/root/todos", axum::routing::get(|| async { axum::Json(json!([])) }))
        .route("/v2/interactions/questions", axum::routing::get(
            move |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>| {
                let generation = question_connections.load(Ordering::SeqCst);
                async move {
                axum::Json(match query.get("sessionId").map(String::as_str) {
                    Some("child") => json!([{"id":format!("q{generation}"),"sessionId":"child","questions":[{"question":"Proceed?"}]}]),
                    Some("invalid") => Value::Null,
                    _ => json!([]),
                })
            }}))
        .route("/v2/interactions/permissions", axum::routing::get(
            |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                axum::Json(match query.get("sessionId").map(String::as_str) {
                    Some("child") => json!([{"id":"p","sessionId":"child","title":"Allow read?","permission":"read"}]),
                    Some("invalid") => Value::Null,
                    _ => json!([]),
                })
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
    let stream = tokio::spawn(async move { events::run_event_stream(inner, "root".into()).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        let (mut root, mut initial_child, mut recovered_child, mut permission, mut empty_permissions, mut child_permissions) = (false, false, false, false, false, false);
        while !(root && initial_child && recovered_child && permission && empty_permissions && child_permissions) {
            match rx.recv().await.unwrap() {
                AgentServerMessage::QuestionsUpdated { session_id, requests } => {
                    match session_id.as_str() {
                        "root" => { assert!(requests.is_empty()); root = true; }
                        "child" => {
                            assert_eq!(requests[0]["sessionId"], "child");
                            match requests[0]["id"].as_str() {
                                Some("q1") => initial_child = true,
                                Some("q2") => recovered_child = true,
                                _ => {}
                            }
                        }
                        _ => panic!("malformed snapshot must not clear pending questions"),
                    }
                }
                AgentServerMessage::ToolUseRequest { session_id, request_id, .. } if request_id == "p" => {
                    assert_eq!(session_id, "child"); permission = true;
                }
                AgentServerMessage::PermissionsUpdated { session_id, requests } => match session_id.as_str() {
                    "root" => { assert!(requests.is_empty()); empty_permissions = true; }
                    "child" => { assert_eq!(requests[0]["id"], "p"); child_permissions = true; }
                    _ => panic!("malformed permission poll must not clear pending requests"),
                },
                _ => {}
            }
        }
    }).await.unwrap();
    assert!(connections.load(Ordering::SeqCst) >= 2, "must exercise automatic reconnect, not ResumeStream");
    stream.abort();
    server.abort();
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