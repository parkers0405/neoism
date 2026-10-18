use neoism_protocol::agent::AgentServerMessage;
use serde_json::json;

#[test]
fn compact_admission_request_identity_is_optional_and_round_trips() {
    use neoism_protocol::agent::AgentClientMessage;
    let legacy: AgentClientMessage = serde_json::from_value(json!({
        "Compact": {"session_id": "origin"}
    })).unwrap();
    assert!(matches!(legacy, AgentClientMessage::Compact { request_id: None, .. }));
    let correlated = AgentClientMessage::Compact {
        session_id: "origin".into(), request_id: Some("local-request".into()),
    };
    let encoded = serde_json::to_value(&correlated).unwrap();
    assert_eq!(encoded["Compact"]["request_id"], "local-request");
    let decoded: AgentClientMessage = serde_json::from_value(encoded).unwrap();
    assert!(matches!(decoded, AgentClientMessage::Compact { session_id, request_id: Some(id) }
        if session_id == "origin" && id == "local-request"));
}

#[test]
fn compaction_continuation_is_optional_and_round_trips() {
    let legacy = json!({"Compaction": {
        "session_id": "session", "phase": "Ended", "text": "summary"
    }});
    let decoded: AgentServerMessage = serde_json::from_value(legacy.clone()).unwrap();
    assert!(matches!(
        decoded,
        AgentServerMessage::Compaction {
            run_continues: None,
            ..
        }
    ));
    let mut continuing = legacy;
    continuing["Compaction"]["run_continues"] = json!(true);
    let decoded: AgentServerMessage = serde_json::from_value(continuing).unwrap();
    assert!(matches!(
        decoded,
        AgentServerMessage::Compaction {
            run_continues: Some(true),
            ..
        }
    ));
    assert_eq!(
        serde_json::to_value(decoded).unwrap()["Compaction"]["run_continues"],
        true
    );
}
