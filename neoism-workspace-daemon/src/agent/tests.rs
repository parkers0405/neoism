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
        AgentServerMessage::SessionEvent { session_id, kind, .. } => {
            assert_eq!(session_id, "sess-root");
            assert_eq!(kind, "session.created");
        }
        other => panic!("unexpected message: {other:?}"),
    }
}
