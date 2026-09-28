use super::*;
use neoism_protocol::agent::AgentServerMessage;
use tokio::sync::mpsc;

#[tokio::test]
async fn spawn_without_key_emits_disabled() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _session = AgentSession::spawn(None, String::new(), tx);
    let first = rx.recv().await.expect("disabled event");
    assert!(matches!(first, AgentServerMessage::Disabled { .. }));
}

#[tokio::test]
async fn send_message_without_key_drops_and_reannounces() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = AgentSession::spawn(None, String::new(), tx);
    let _ = rx.recv().await;
    session.send_message("hi".into(), Vec::new());
    let again = rx.recv().await.expect("re-disabled");
    assert!(matches!(again, AgentServerMessage::Disabled { .. }));
}

#[tokio::test]
async fn ping_round_trip_replies_with_pong() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = AgentSession::spawn(Some("k".to_string()), String::new(), tx);
    super::dispatch(&session, AgentClientMessage::Ping);
    // Skip any unsolicited pushes (the spawn path with a key
    // doesn't emit Disabled, but be defensive).
    loop {
        match rx.recv().await.expect("pong") {
            AgentServerMessage::Pong => return,
            AgentServerMessage::Disabled { .. } => continue,
            other => panic!("unexpected message: {other:?}"),
        }
    }
}

#[test]
fn forwarded_part_delta_uses_part_id_not_message_id() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    forward_agent_server_event(
        &tx,
        "sess-1",
        json!({
            "type": "message.part.delta",
            "properties": {
                "sessionID": "sess-1",
                "messageID": "msg-1",
                "partID": "part-1",
                "field": "text",
                "delta": "hello"
            }
        }),
    );

    match rx.try_recv().expect("content delta") {
        AgentServerMessage::ContentDelta {
            session_id,
            message_id,
            text,
            ..
        } => {
            assert_eq!(session_id, "sess-1");
            assert_eq!(message_id, "part-1");
            assert_eq!(text, "hello");
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn forwarded_part_update_uses_part_id() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    forward_agent_server_event(
        &tx,
        "sess-1",
        json!({
            "type": "message.part.updated",
            "properties": {
                "sessionID": "sess-1",
                "messageID": "msg-1",
                "part": {
                    "id": "part-1",
                    "type": "text",
                    "text": "hello"
                }
            }
        }),
    );

    match rx.try_recv().expect("message update") {
        AgentServerMessage::MessageUpdated { message, .. } => {
            assert_eq!(message.id, "part-1");
            assert_eq!(message.text, "hello");
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn session_created_with_parent_synthesizes_subagent_update() {
    use neoism_protocol::agent::SubagentStatus;
    // The agent server never emits a `subagent.*` event — a child
    // spawn is announced solely through `session.created` carrying
    // `info.parentId`. The translation must surface it as the
    // `SubagentUpdate` the side-panel roster consumes, parent link
    // included, even though the daemon's SSE stream is bound to a
    // DIFFERENT family session.
    let (tx, mut rx) = mpsc::unbounded_channel();
    forward_agent_server_event(
        &tx,
        "sess-viewed-sibling",
        json!({
            "type": "session.created",
            "properties": {
                "sessionID": "sess-child",
                "info": {
                    "id": "sess-child",
                    "parentId": "sess-parent",
                    "title": "Investigate flaky test",
                    "agent": "explore",
                    "time": { "created": 1_755_500_000_000u64, "updated": 1_755_500_000_000u64 }
                }
            }
        }),
    );

    match rx.try_recv().expect("subagent update") {
        AgentServerMessage::SubagentUpdate {
            session_id,
            status,
            title,
            agent,
            current_tool,
            started_at,
            parent_session_id,
            ..
        } => {
            assert_eq!(session_id, "sess-child");
            assert!(matches!(status, SubagentStatus::Running));
            assert_eq!(title.as_deref(), Some("Investigate flaky test"));
            assert_eq!(agent.as_deref(), Some("explore"));
            assert_eq!(current_tool, None);
            assert_eq!(started_at, Some(1_755_500_000_000));
            assert_eq!(parent_session_id.as_deref(), Some("sess-parent"));
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn session_created_without_parent_stays_a_raw_envelope() {
    // Root sessions carry no parent link: they must NOT be mistaken
    // for subagents — the event keeps falling through to the generic
    // `SessionEvent` envelope exactly as before this arm existed.
    let (tx, mut rx) = mpsc::unbounded_channel();
    forward_agent_server_event(
        &tx,
        "sess-root",
        json!({
            "type": "session.created",
            "properties": {
                "sessionID": "sess-root",
                "info": {
                    "id": "sess-root",
                    "title": "New conversation",
                    "time": { "created": 1u64, "updated": 1u64 }
                }
            }
        }),
    );

    match rx.try_recv().expect("raw envelope") {
        AgentServerMessage::SessionEvent {
            session_id, kind, ..
        } => {
            assert_eq!(session_id, "sess-root");
            assert_eq!(kind, "session.created");
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn permission_asked_maps_current_v2_payload() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    forward_agent_server_event(
        &tx,
        "sess-root",
        json!({
            "type": "permission.asked",
            "properties": {
                "id": "per-1",
                "sessionId": "sess-root",
                "title": "Allow bash?",
                "permission": "bash",
                "patterns": ["cargo check"],
                "metadata": { "tool": "bash", "input": { "command": "cargo check" } },
                "sourceAgent": "build"
            }
        }),
    );

    match rx.try_recv().expect("permission request") {
        AgentServerMessage::ToolUseRequest {
            request_id,
            tool,
            title,
            patterns,
            args,
            source_agent,
            ..
        } => {
            assert_eq!(request_id, "per-1");
            assert_eq!(tool, "bash");
            assert_eq!(title, "Allow bash?");
            assert_eq!(patterns, ["cargo check"]);
            assert_eq!(args["command"], "cargo check");
            assert_eq!(source_agent.as_deref(), Some("build"));
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn permission_replied_removes_request_by_permission_id() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    forward_agent_server_event(
        &tx,
        "sess-root",
        json!({
            "type": "permission.replied",
            "properties": { "requestID": "per-1", "reply": "once" }
        }),
    );

    assert!(matches!(
        rx.try_recv().expect("permission removal"),
        AgentServerMessage::PermissionRemoved { request_id, .. } if request_id == "per-1"
    ));
}

#[test]
fn thread_summary_serializes_only_persisted_acp_root_provider() {
    let base = json!({
        "id": "ses_1", "title": "Plan", "time": {"updated": 12},
        "externalAgent": {"runtime": "acp", "provider": "codex"}
    });
    let root = super::thread_summary_from_session(&base).unwrap();
    assert_eq!(root.external_provider.as_deref(), Some("codex"));
    let encoded = serde_json::to_value(&root).unwrap();
    assert_eq!(encoded["external_provider"], "codex");
    assert_eq!(
        serde_json::from_value::<neoism_protocol::agent::ThreadSummary>(encoded).unwrap(),
        root
    );

    let mut child = base.clone();
    child["parentId"] = json!("ses_parent");
    assert_eq!(
        super::thread_summary_from_session(&child)
            .unwrap()
            .external_provider,
        None
    );
    let mut native = base.clone();
    native.as_object_mut().unwrap().remove("externalAgent");
    let native = super::thread_summary_from_session(&native).unwrap();
    assert_eq!(native.external_provider, None);
    assert!(serde_json::to_value(native)
        .unwrap()
        .get("external_provider")
        .is_none());
    let mut untrusted = base;
    untrusted["externalAgent"]["provider"] = json!("unknown");
    assert_eq!(
        super::thread_summary_from_session(&untrusted)
            .unwrap()
            .external_provider,
        None
    );
}

#[test]
fn todo_snapshot_maps_persisted_acp_plan_for_web_reconnect() {
    let todos = super::todo_items_from_response(&json!([
        {"content": "first", "status": "completed", "priority": "high"},
        {"content": "second", "status": "in_progress", "priority": "medium"}
    ]));
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[0].status, "completed");
    assert_eq!(todos[1].content, "second");
}
