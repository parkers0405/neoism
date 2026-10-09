use neoism_protocol::agent::{AgentServerMessage, ThreadSummary};

#[test]
fn catalog_activity_keeps_older_thread_snapshots_readable() {
    let thread: ThreadSummary = serde_json::from_value(serde_json::json!({
        "session_id": "root", "title": "Main chat", "busy": true
    }))
    .unwrap();
    assert!(thread.catalog_activity.is_none());
    assert!(thread.busy);
    let value = serde_json::to_value(thread).unwrap();
    assert!(value.get("catalog_activity").is_none());
}

#[test]
fn catalog_activity_messages_and_snapshots_round_trip() {
    for activity in ["idle", "running", "background", "permission"] {
        let message = AgentServerMessage::CatalogActivity {
            session_id: "root".into(),
            activity: activity.into(),
        };
        let decoded: AgentServerMessage =
            serde_json::from_value(serde_json::to_value(message).unwrap()).unwrap();
        assert!(matches!(decoded, AgentServerMessage::CatalogActivity {
            session_id, activity: received
        } if session_id == "root" && received == activity));
        let thread: ThreadSummary = serde_json::from_value(serde_json::json!({
            "session_id": "root", "title": "Main chat", "catalog_activity": activity
        }))
        .unwrap();
        assert_eq!(thread.catalog_activity.as_deref(), Some(activity));
    }
}
