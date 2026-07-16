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
