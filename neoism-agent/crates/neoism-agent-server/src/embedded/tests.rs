use super::*;
use axum::{body::to_bytes, http::StatusCode};
use futures::StreamExt;
use neoism_agent_core::{Id, IdKind, MessageWithParts, Page, SessionInfo};
use serde_json::{json, Value};

fn root() -> PathBuf {
    let path = std::env::temp_dir()
        .join(format!("neoism-embedded-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn request(method: &str, uri: &str, value: Option<Value>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(value.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}

async fn json_response(response: Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn existing_tool_loop_multi_turn_streaming_and_reopen_work_without_listener() {
    let root = root();
    let db = root.join("agent.db");
    std::fs::write(root.join("input.txt"), "embedded tool result").unwrap();
    let host = EmbeddedAgent::open(EmbeddedOptions::new(&db, crate::standard_services()))
        .await
        .unwrap();
    let state = Arc::downgrade(&host.inner.state.inner);
    let response = host
        .request(request("GET", "/v2/events", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let mut events = response.into_body().into_data_stream();
    let uri = format!(
        "/v2/sessions?{}",
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("directory", root.to_str().unwrap())
            .finish()
    );
    let session: SessionInfo = serde_json::from_value(
        json_response(
            host.request(request("POST", &uri, Some(json!({"title":"embedded"}))))
                .await
                .unwrap(),
        )
        .await,
    )
    .unwrap();
    for text in ["read-tool: input.txt", "second turn"] {
        let response = host
            .request(request(
                "POST",
                &format!("/v2/sessions/{}/prompt", session.id),
                Some(json!({
                    "model": {"providerId":"neoism", "modelId":"stub"},
                    "parts": [{"type":"text", "text":text}]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let response = tokio::time::timeout(
            Duration::from_secs(15),
            host.request(request(
                "POST",
                &format!("/v2/sessions/{}/wait", session.id),
                None,
            )),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    let uri = format!("/v2/sessions/{}/messages", session.id);
    let saved =
        json_response(host.request(request("GET", &uri, None)).await.unwrap()).await;
    let page: Page<MessageWithParts> = serde_json::from_value(saved.clone()).unwrap();
    assert!(
        page.items.len() >= 5,
        "both turns must use the existing session"
    );
    assert!(saved.to_string().contains("Tool result received"));
    assert!(saved.to_string().contains("embedded tool result"));
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            if String::from_utf8_lossy(&event.unwrap()).contains("session.created") {
                return;
            }
        }
        panic!("session event was not streamed");
    })
    .await
    .unwrap();
    let other_handle = host.clone();
    host.close().await.unwrap();
    other_handle.close().await.unwrap();
    assert!(other_handle
        .request(request("GET", "/v2/health", None))
        .await
        .is_err());
    tokio::time::timeout(Duration::from_secs(5), async {
        while events.next().await.is_some() {}
    })
    .await
    .unwrap();
    drop(events);
    drop(other_handle);
    drop(host);
    assert!(
        state.upgrade().is_none(),
        "closed host must not retain its runtime"
    );
    let reopened =
        EmbeddedAgent::open(EmbeddedOptions::new(&db, crate::standard_services()))
            .await
            .unwrap();
    assert_eq!(
        json_response(reopened.request(request("GET", &uri, None)).await.unwrap()).await,
        saved
    );
    reopened.close().await.unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn invalid_hosted_services_fail_before_creating_state() {
    let root = root();
    let database = root.join("not-created").join("agent.db");
    let result = EmbeddedAgent::open(EmbeddedOptions::new(
        &database,
        crate::standard_services().for_hosted_control_plane(),
    ))
    .await;
    assert!(result.is_err());
    assert!(!database.parent().unwrap().exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn restart_claim_resumes_existing_history_without_repeating_completed_tool() {
    use neoism_agent_core::{MessageInfo, Part, PromptRequest};
    let root = root();
    let database = root.join("restart.db");
    std::fs::write(root.join("input.txt"), "result retained across restart").unwrap();
    let host = EmbeddedAgent::open(EmbeddedOptions::new(&database, crate::standard_services())).await.unwrap();
    let session: SessionInfo = serde_json::from_value(json_response(host.request(request(
        "POST", &format!("/v2/sessions?{}", url::form_urlencoded::Serializer::new(String::new()).append_pair("directory", root.to_str().unwrap()).finish()),
        Some(json!({"title": "restart"})),
    )).await.unwrap()).await).unwrap();
    let prompt: PromptRequest = serde_json::from_value(json!({
        "messageId": Id::ascending(IdKind::Message),
        "model": {"providerId": "neoism", "modelId": "stub"},
        "parts": [{"type": "text", "text": "read-tool: input.txt"}]
    })).unwrap();
    crate::session_prompt::append_prompt(&host.inner.state, session.id.as_str(), prompt.clone(), true).await.unwrap();
    let before = host.inner.state.inner.store.list_context_messages(session.id.as_str()).await.unwrap();
    let tool_count = before.iter().flat_map(|message| &message.parts).filter(|part| matches!(part, Part::Tool(_))).count();
    assert!(tool_count > 0);
    // Simulate a lost terminal commit: durable results exist but the write-ahead
    // claim still belongs to the old process. Shutdown must not consume it.
    host.inner.state.claim_prompt_run(session.id.as_str(), "orphaned", &prompt).await.unwrap();
    host.close().await.unwrap();
    drop(host);
    std::fs::remove_file(root.join("input.txt")).unwrap();

    let host = EmbeddedAgent::open(EmbeddedOptions::new(&database, crate::standard_services())).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(15), host.request(request(
        "POST", &format!("/v2/sessions/{}/wait", session.id), None,
    ))).await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let after = host.inner.state.inner.store.list_context_messages(session.id.as_str()).await.unwrap();
    assert!(after.len() > before.len(), "the existing loop must actually resume");
    assert_eq!(after.iter().filter(|message| matches!(message.info, MessageInfo::User(_))).count(), 1);
    assert_eq!(after.iter().flat_map(|message| &message.parts).filter(|part| matches!(part, Part::Tool(_))).count(), tool_count);
    assert!(serde_json::to_string(&after).unwrap().contains("result retained across restart"));
    host.close().await.unwrap();
    drop(host);
    let host = EmbeddedAgent::open(EmbeddedOptions::new(&database, crate::standard_services())).await.unwrap();
    assert_eq!(serde_json::to_value(host.inner.state.inner.store.list_context_messages(session.id.as_str()).await.unwrap()).unwrap(), serde_json::to_value(after).unwrap());
    host.close().await.unwrap();
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancelling_close_waiter_does_not_abandon_retirement() {
    let root = root();
    let host = EmbeddedAgent::open(EmbeddedOptions::new(
        root.join("agent.db"),
        crate::standard_services(),
    ))
    .await
    .unwrap();
    let guard = host.inner.admission.read().await;
    let mut stopping = host.inner.stopping.subscribe();
    let closing = host.clone();
    let waiter = tokio::spawn(async move { closing.close().await });
    tokio::time::timeout(Duration::from_secs(5), stopping.changed())
        .await
        .unwrap()
        .unwrap();
    waiter.abort();
    let _ = waiter.await;
    drop(guard);
    tokio::time::timeout(Duration::from_secs(5), host.close())
        .await
        .unwrap()
        .unwrap();
    assert!(host
        .request(request("GET", "/v2/health", None))
        .await
        .is_err());
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn restart_after_close_during_permission_wait_stops_old_turn_and_resumes() {
    let root = root();
    let database = root.join("permission-restart.db");
    std::fs::write(root.join("input.txt"), "permission-gated read").unwrap();
    let options = || EmbeddedOptions::new(&database, crate::standard_services())
        .with_config(json!({"dangerouslySkipPermissions": false})).unwrap();
    let host = EmbeddedAgent::open(options()).await.unwrap();
    let mut session: SessionInfo = serde_json::from_value(json_response(host.request(request(
        "POST", &format!("/v2/sessions?{}", url::form_urlencoded::Serializer::new(String::new()).append_pair("directory", root.to_str().unwrap()).finish()),
        Some(json!({"title": "permission restart"})),
    )).await.unwrap()).await).unwrap();
    session.permission = Some(serde_json::from_value(json!([
        {"permission": "read", "pattern": "*", "action": "ask"}
    ])).unwrap());
    host.inner.state.inner.store.update_session(&session).await.unwrap();
    let state = host.inner.state.clone();
    let id = session.id.to_string();
    let task = tokio::spawn(async move {
        crate::session_prompt::append_prompt(&state, &id, serde_json::from_value(json!({
            "model": {"providerId": "neoism", "modelId": "stub"},
            "parts": [{"type": "text", "text": "read-tool: input.txt"}]
        })).unwrap(), true).await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while host.inner.state.inner.permission_waiters.read().await.is_empty() {
            assert!(!task.is_finished(), "model turn ended before permission wait");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), host.close()).await.unwrap().unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
    assert!(host.inner.state.inner.session_coordinator.active_runs().await.is_empty());
    drop(host);

    let host = EmbeddedAgent::open(options()).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(10), host.request(request(
        "POST", &format!("/v2/sessions/{}/wait", session.id), None,
    ))).await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(host.inner.state.inner.store.list_pending_permissions().await.unwrap().is_empty());
    let messages = host.inner.state.inner.store.list_context_messages(session.id.as_str()).await.unwrap();
    assert!(messages.iter().filter(|message| matches!(message.info, neoism_agent_core::MessageInfo::Assistant(_))).count() >= 2);
    host.close().await.unwrap();
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn in_memory_transport_does_not_skip_authentication_or_enable_management() {
    let root = root();
    let host = EmbeddedAgent::open(EmbeddedOptions::new(
        root.join("agent.db"),
        crate::standard_services(),
    ))
    .await
    .unwrap();
    let mut unauthorized = request("GET", "/v2/sessions", None);
    unauthorized.headers_mut().insert(
        "authorization",
        format!(
            "Bearer {}invalid",
            neoism_agent_service_api::daemon_credential::PREFIX
        )
        .parse()
        .unwrap(),
    );
    let response = host.request(unauthorized).await.unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert!(!host.inner.state.inner.management_policy.is_enabled());
    host.close().await.unwrap();
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}
