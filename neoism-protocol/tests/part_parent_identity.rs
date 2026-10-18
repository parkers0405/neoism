use neoism_protocol::agent::{AgentServerMessage, ContentKind, HistoryMessageKind, Role};
use serde_json::json;

#[test]
fn live_part_parent_identity_roundtrips_and_is_optional_for_old_peers() {
    let events = [
        AgentServerMessage::ContentDelta {
            session_id: "root".into(),
            message_id: "reason-part".into(),
            parent_message_id: Some("assistant-old".into()),
            kind: ContentKind::Reasoning,
            text: "partial".into(),
        },
        AgentServerMessage::MessageUpdated {
            session_id: "root".into(),
            parent_message_id: Some("assistant-old".into()),
            message: serde_json::from_value(json!({
                "id": "reason-part", "role": Role::Assistant,
                "kind": HistoryMessageKind::Reasoning, "text": "partial"
            }))
            .unwrap(),
        },
    ];
    for event in events {
        let mut wire = serde_json::to_value(&event).unwrap();
        let decoded: AgentServerMessage = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
        let body = wire.as_object_mut().unwrap().values_mut().next().unwrap();
        assert_eq!(body["parent_message_id"], "assistant-old");
        body.as_object_mut().unwrap().remove("parent_message_id");
        let legacy: AgentServerMessage = serde_json::from_value(wire.clone()).unwrap();
        assert!(matches!(
            legacy,
            AgentServerMessage::ContentDelta {
                parent_message_id: None,
                ..
            } | AgentServerMessage::MessageUpdated {
                parent_message_id: None,
                ..
            }
        ));
        let legacy: AgentServerMessage = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap(), wire);
    }
}
