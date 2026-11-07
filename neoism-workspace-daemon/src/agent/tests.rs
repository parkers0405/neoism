use super::*;
use neoism_protocol::agent::AgentServerMessage;

#[test]
fn transport_created_at_live_tool_retains_original_start() {
    for status in ["running", "completed"] {
        let part = json!({"id":"tool", "type":"tool", "tool":"bash",
            "state":{"status":status, "input":{"command":"pwd"}, "output":"/workspace",
                "time":{"start":1200, "end":9000, "updated":9500}}});
        assert_eq!(history_from_part(&part).unwrap().created_at, 1200);
    }
    let text = json!({"id":"text", "type":"text", "text":"hello",
        "time":{"start":1300, "end":8000}, "state":{"time":{"start":1400}}});
    assert_eq!(history_from_part(&text).unwrap().created_at, 1300);
    let no_start = json!({"id":"text", "type":"text", "text":"hello",
        "time":{"end":8000, "updated":9000}});
    assert_eq!(history_from_part(&no_start).unwrap().created_at, 0);
}

#[test]
fn transport_created_at_history_preserves_user_tool_and_canonical_order() {
    let items = vec![
        json!({"info":{"id":"assistant", "role":"assistant", "sessionID":"session",
            "parentID":"user", "time":{"created":1100,"completed":9000,"updated":9500},
            "mode":"build", "agent":"build", "path":{"cwd":"/workspace","root":"/workspace"},
            "cost":0, "tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},
            "modelID":"test", "providerID":"test"},
            "parts":[
                {"id":"answer", "type":"text", "text":"done", "time":{"start":1600,"end":9000}},
                {"id":"reason", "type":"reasoning", "text":"thinking", "time":{"start":1150}},
                {"id":"tool", "type":"tool", "tool":"bash", "state":{"status":"completed",
                    "input":{"command":"pwd"}, "output":"/workspace", "time":{"start":1200,"end":8000}}},
                {"id":"fallback", "type":"text", "text":"no start", "time":{"end":9000,"updated":9500}}
            ]}),
        json!({"info":{"id":"user", "role":"user", "sessionID":"session", "time":{"created":1000,"updated":9500},
            "agent":"build", "model":{"providerID":"test","modelID":"test"}},
            "parts":[{"id":"user-part", "type":"text", "text":"hello", "time":{"start":1050}}]})
    ];
    let canonical =
        neoism_ui::panels::agent_pane::api_mapping::message_blocks_from_response(&items, true);
    let history = history_messages_from_response(&items);
    assert_eq!(
        history.iter().map(|m| &m.id).collect::<Vec<_>>(),
        canonical.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
    assert_eq!(history.first().unwrap().id, "user");
    for (id, expected) in [
        ("user", 1000),
        ("tool", 1200),
        ("answer", 1600),
        ("reason", 1150),
        ("fallback", 1100),
    ] {
        assert_eq!(
            history.iter().find(|m| m.id == id).unwrap().created_at,
            expected,
            "{id}"
        );
    }
}

#[tokio::test]
async fn spawn_without_key_emits_disabled() {
    let (tx, mut rx) = reply_queue::channel();
    let _session = AgentSession::spawn(None, String::new(), tx);
    let first = rx.recv().await.expect("disabled event");
    assert!(matches!(first, AgentServerMessage::Disabled { .. }));
}

#[tokio::test]
async fn send_message_without_key_drops_and_reannounces() {
    let (tx, mut rx) = reply_queue::channel();
    let session = AgentSession::spawn(None, String::new(), tx);
    let _ = rx.recv().await;
    session.send_message("hi".into(), Vec::new());
    let again = rx.recv().await.expect("re-disabled");
    assert!(matches!(again, AgentServerMessage::Disabled { .. }));
}

#[tokio::test]
async fn ping_round_trip_replies_with_pong() {
    let (tx, mut rx) = reply_queue::channel();
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
    let (tx, mut rx) = reply_queue::channel();
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
            parent_message_id,
            text,
            ..
        } => {
            assert_eq!(session_id, "sess-1");
            assert_eq!(message_id, "part-1");
            assert_eq!(parent_message_id.as_deref(), Some("msg-1"));
            assert_eq!(text, "hello");
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn forwarded_part_update_uses_part_id() {
    let (tx, mut rx) = reply_queue::channel();
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
        AgentServerMessage::MessageUpdated { message, parent_message_id, .. } => {
            assert_eq!(message.id, "part-1");
            assert_eq!(parent_message_id.as_deref(), Some("msg-1"));
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
    let (tx, mut rx) = reply_queue::channel();
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
    let (tx, mut rx) = reply_queue::channel();
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
    let (tx, mut rx) = reply_queue::channel();
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
    let (tx, mut rx) = reply_queue::channel();
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
    for (provider, driver) in [("codex", "codex-app-server"), ("claude", "claude-agent-sdk"), ("opencode", "opencode-server")] {
        child["externalAgent"] = json!({"provider":provider,"runtime":driver});
        assert_eq!(super::thread_summary_from_session(&child).unwrap().external_provider.as_deref(), Some(provider));
        child["externalAgent"]["runtime"] = json!("wrong-driver");
        assert!(super::thread_summary_from_session(&child).unwrap().external_provider.is_none());
    }
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
    let todos = super::handlers::todo_items_from_response(&json!([
        {"content": "first", "status": "completed", "priority": "high"},
        {"content": "second", "status": "in_progress", "priority": "medium"}
    ]));
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[0].status, "completed");
    assert_eq!(todos[1].content, "second");
}
