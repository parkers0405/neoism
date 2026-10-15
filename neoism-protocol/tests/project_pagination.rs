use neoism_protocol::agent::{
    AgentClientMessage, AgentServerMessage, SessionProjectSummary,
};

#[test]
fn discovery_pins_are_additive_camel_case_and_lossless() {
    let legacy = serde_json::json!({"SessionProjects": {
        "catalog_identity":"endpoint:root:epoch", "generation":7, "items":[]
    }});
    let decoded: AgentServerMessage = serde_json::from_value(legacy).unwrap();
    let AgentServerMessage::SessionProjects { pinned_sessions, .. } = decoded else { panic!() };
    assert!(pinned_sessions.is_empty());
    let pins = serde_json::json!([{"id":"old-pin", "directory":"/older/project", "pinned":true,
        "time":{"updated":1}, "catalogActivity":"permission", "externalAgent":{
            "provider":"codex", "sourceKey":"stable-source", "historyState":"neoism_only"
        }}]);
    let wire = serde_json::json!({"SessionProjects": {
        "catalog_identity":"endpoint:root:epoch", "generation":8, "items":[], "pinnedSessions":pins
    }});
    let decoded: AgentServerMessage = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
}

#[test]
fn project_requests_preserve_exact_identity_directory_cursor_and_generation() {
    let requests = [
        AgentClientMessage::ListSessionProjects {
            catalog_identity: "endpoint:root:epoch".into(),
            generation: 19,
        },
        AgentClientMessage::ListProjectSessions {
            catalog_identity: "endpoint:root:epoch".into(),
            directory: "/project with spaces/α".into(),
            cursor: Some("opaque+/=&".into()),
            generation: 2,
        },
    ];
    for request in requests {
        let encoded = serde_json::to_value(&request).unwrap();
        let decoded: AgentClientMessage =
            serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }
}

#[test]
fn project_results_and_errors_roundtrip_independent_generations() {
    let messages = [
        AgentServerMessage::SessionProjects {
            catalog_identity: "endpoint:root:epoch".into(),
            generation: 19,
            pinned_sessions: vec![serde_json::json!({"id":"old-pin", "directory":"/archived", "pinned":true, "externalAgent":{"sourceKey":"stable-key", "historyState":"neoism_only"}})],
            items: vec![SessionProjectSummary {
                directory: "/a".into(),
                updated_ms: 123,
            }],
        },
        AgentServerMessage::SessionProjectsFailed {
            catalog_identity: "endpoint:root:epoch".into(),
            generation: 19,
            message: "denied".into(),
        },
        AgentServerMessage::ProjectSessionPage {
            catalog_identity: "endpoint:root:epoch".into(),
            directory: "/b".into(),
            requested_cursor: Some("page2".into()),
            generation: 2,
            threads: vec![],
            next_cursor: Some("page3".into()),
        },
        AgentServerMessage::ProjectSessionPageFailed {
            catalog_identity: "endpoint:root:epoch".into(),
            directory: "/a".into(),
            requested_cursor: None,
            generation: 1,
            message: "offline".into(),
        },
        AgentServerMessage::SessionProjectsChanged,
    ];
    for message in messages {
        let encoded = serde_json::to_value(&message).unwrap();
        let decoded: AgentServerMessage =
            serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }
    assert_eq!(
        serde_json::to_value(SessionProjectSummary {
            directory: "/a".into(),
            updated_ms: 123
        })
        .unwrap(),
        serde_json::json!({"directory":"/a", "updatedMs":123})
    );
}
