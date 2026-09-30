//! Roundtrip every agent-protocol variant through serde_json so the
//! daemon and the TS/wasm chrome stay in lock-step on the wire shape.

use neoism_protocol::agent::{
    AgentClientMessage, AgentServerMessage, Attachment, ContentKind, PermissionDecision,
    Role,
};

fn roundtrip_client(msg: &AgentClientMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: AgentClientMessage = serde_json::from_str(&json).expect("deserialize");
    let json_back = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json_back, "roundtrip mismatch: {json}");
}

fn roundtrip_server(msg: &AgentServerMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: AgentServerMessage = serde_json::from_str(&json).expect("deserialize");
    let json_back = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json_back, "roundtrip mismatch: {json}");
}

#[test]
fn client_send_message_roundtrip() {
    roundtrip_client(&AgentClientMessage::SendMessage {
        text: "hello world".into(),
        attachments: Vec::new(),
    });
    roundtrip_client(&AgentClientMessage::SendMessage {
        text: "look at this".into(),
        attachments: vec![Attachment {
            kind: "file".into(),
            path: Some("src/lib.rs".into()),
            bytes: Vec::new(),
        }],
    });
}

#[test]
fn client_cancel_roundtrip() {
    roundtrip_client(&AgentClientMessage::Cancel);
}

#[test]
fn client_new_thread_roundtrip() {
    roundtrip_client(&AgentClientMessage::NewThread);
}

#[test]
fn client_reply_permission_roundtrip() {
    roundtrip_client(&AgentClientMessage::ReplyPermission {
        request_id: 42,
        decision: PermissionDecision::Yes,
    });
    roundtrip_client(&AgentClientMessage::ReplyPermission {
        request_id: 99,
        decision: PermissionDecision::Always,
    });
    roundtrip_client(&AgentClientMessage::ReplyPermission {
        request_id: 1,
        decision: PermissionDecision::No,
    });
}

#[test]
fn server_disabled_roundtrip() {
    roundtrip_server(&AgentServerMessage::Disabled {
        reason: "set NEOISM_AGENT_API_KEY".into(),
    });
}

#[test]
fn server_message_start_roundtrip() {
    roundtrip_server(&AgentServerMessage::MessageStart {
        session_id: "sess_01".into(),
        role: Role::Assistant,
        message_id: "msg_01".into(),
    });
    roundtrip_server(&AgentServerMessage::MessageStart {
        session_id: "sess_01".into(),
        role: Role::User,
        message_id: "u1".into(),
    });
    roundtrip_server(&AgentServerMessage::MessageStart {
        session_id: "sess_01".into(),
        role: Role::System,
        message_id: "s1".into(),
    });
}

#[test]
fn server_content_delta_roundtrip() {
    roundtrip_server(&AgentServerMessage::ContentDelta {
        session_id: "sess_01".into(),
        message_id: "msg_01".into(),
        kind: ContentKind::Text,
        text: "Hello".into(),
    });
    roundtrip_server(&AgentServerMessage::ContentDelta {
        session_id: "sess_01".into(),
        message_id: "msg_01".into(),
        kind: ContentKind::Reasoning,
        text: "Considering...".into(),
    });
    roundtrip_server(&AgentServerMessage::ContentDelta {
        session_id: "sess_01".into(),
        message_id: "msg_compaction".into(),
        kind: ContentKind::Compaction,
        text: "Summary".into(),
    });
    roundtrip_server(&AgentServerMessage::ContentDelta {
        session_id: "sess_01".into(),
        message_id: "msg_01".into(),
        kind: ContentKind::Tool {
            name: "read_file".into(),
        },
        text: "{\"path\":\"x\"}".into(),
    });
}

#[test]
fn server_message_end_roundtrip() {
    roundtrip_server(&AgentServerMessage::MessageEnd {
        session_id: "sess_01".into(),
        message_id: "msg_01".into(),
        stop_reason: "end_turn".into(),
    });
}

#[test]
fn server_permission_request_roundtrip() {
    roundtrip_server(&AgentServerMessage::PermissionRequest {
        request_id: 7,
        tool: "write_file".into(),
        args: serde_json::json!({ "path": "tmp.txt" }),
    });
}

#[test]
fn server_error_roundtrip() {
    roundtrip_server(&AgentServerMessage::Error {
        message: "api 429".into(),
    });
}

#[test]
fn disabled_json_shape() {
    let msg = AgentServerMessage::Disabled {
        reason: "no key".into(),
    };
    let json = serde_json::to_string(&msg).expect("serialize");
    assert_eq!(json, r#"{"Disabled":{"reason":"no key"}}"#);
}
