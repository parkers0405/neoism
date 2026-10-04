use neoism_agent_core::{EventPayload, Id, IdKind};
use neoism_agent_plugin_api::{
    PluginScope, ProcessHookInvokeRequest, ProcessHostBrokerRequest, ProcessHostFrame,
    ProcessInitializeRequest, ProcessInitializeResponse, ProcessPluginFrame,
    ProcessPluginOwner, ProcessServiceDeclaration, ProcessServiceDeclarations,
    ProcessStreamEnvelope, ProcessToolInvokeRequest, PROCESS_PLUGIN_V2_PROTOCOL,
};
use serde_json::json;

#[test]
fn process_v2_initialize_wire_is_stable_and_additive() {
    let request = ProcessInitializeRequest {
        protocol: PROCESS_PLUGIN_V2_PROTOCOL.into(),
        plugin_id: "dev.example.plugin".into(),
        instance_id: "instance-7".into(),
        directory: "/workspace".into(),
        config: json!({"enabled": true}),
        owner: None,
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "protocol": "neoism-plugin/2",
            "pluginId": "dev.example.plugin",
            "instanceId": "instance-7",
            "directory": "/workspace",
            "config": {"enabled": true}
        })
    );

    let response: ProcessInitializeResponse = serde_json::from_value(json!({
        "protocol": "neoism-plugin/2",
        "name": "Example",
        "futureField": true
    }))
    .unwrap();
    assert_eq!(response.protocol, PROCESS_PLUGIN_V2_PROTOCOL);
    assert!(response.tools.is_empty());
    assert!(response.services.is_empty());
}

#[test]
fn process_v2_frames_preserve_existing_tool_hook_and_event_shapes() {
    let tool = ProcessHostFrame {
        id: Some(2),
        method: "tool.invoke".into(),
        params: serde_json::to_value(ProcessToolInvokeRequest {
            tool: "echo".into(),
            directory: "/workspace".into(),
            session_id: Some("ses_1".into()),
            input: json!({"text": "hello"}),
        })
        .unwrap(),
    };
    assert_eq!(tool.params["sessionId"], "ses_1");

    let hook = ProcessHookInvokeRequest {
        hook: "chat.options".into(),
        context: json!({"sessionId": "ses_1"}),
        value: json!({"temperature": 0.2}),
    };
    assert_eq!(serde_json::to_value(hook).unwrap()["hook"], "chat.options");

    let event = EventPayload {
        id: Id::ascending(IdKind::Event),
        kind: "session.updated".into(),
        sequence: Some(4),
        properties: json!({"sessionId": "ses_1"}),
    };
    let event_frame = ProcessHostFrame {
        id: None,
        method: "event".into(),
        params: serde_json::to_value(event).unwrap(),
    };
    assert_eq!(event_frame.params["type"], "session.updated");
    assert_eq!(event_frame.params["sequence"], 4);

    let reply: ProcessPluginFrame = serde_json::from_value(json!({
        "id": 2,
        "result": {"title": "Echo", "output": "hello"}
    }))
    .unwrap();
    assert_eq!(reply.id, Some(2));
    assert!(reply.error.is_none());
    assert!(reply.method.is_none());
}

#[test]
fn process_owner_requires_plugin_and_instance_identity() {
    let owner = ProcessPluginOwner {
        plugin_id: "dev.example.plugin".into(),
        instance_id: "instance-7".into(),
        package_revision: None,
        registry_generation: None,
        scope: None,
        workspace_id: None,
        scope_id: None,
    };
    assert_eq!(
        serde_json::to_value(owner).unwrap(),
        json!({"pluginId": "dev.example.plugin", "instanceId": "instance-7"})
    );
}

#[test]
fn scoped_owner_and_broker_requests_have_stable_additive_shapes() {
    let owner = ProcessPluginOwner {
        plugin_id: "dev.example.session".into(),
        instance_id: "instance-4".into(),
        package_revision: Some("sha256:abc".into()),
        registry_generation: Some(4),
        scope: Some(PluginScope::Session),
        workspace_id: Some("workspace-opaque".into()),
        scope_id: Some("session-opaque".into()),
    };
    assert_eq!(
        serde_json::to_value(owner).unwrap()["scopeId"],
        "session-opaque"
    );
    assert_eq!(
        serde_json::to_value(ProcessHostBrokerRequest {
            operation: "sign-request".into(),
            input: json!({"resource":"opaque:request"}),
        })
        .unwrap(),
        json!({"operation":"sign-request","input":{"resource":"opaque:request"}})
    );
}

#[test]
fn process_v2_unary_services_have_a_stable_additive_wire_shape() {
    let services = ProcessServiceDeclarations {
        agents: vec![ProcessServiceDeclaration {
            id: "example.agents".into(),
            priority: 4,
        }],
        prompts: vec![ProcessServiceDeclaration {
            id: "example.prompts".into(),
            priority: 0,
        }],
        ..ProcessServiceDeclarations::default()
    };
    assert_eq!(
        serde_json::to_value(services).unwrap(),
        json!({
            "agents": [{"id": "example.agents", "priority": 4}],
            "commands": [],
            "skills": [],
            "systemContext": [],
            "prompts": [{"id": "example.prompts", "priority": 0}],
            "config": []
        })
    );
}

#[test]
fn reverse_requests_are_distinct_from_replies_and_carry_exact_owner() {
    let request: ProcessPluginFrame = serde_json::from_value(json!({
        "id": 9,
        "method": "host.config.get",
        "params": {"key": "theme"},
        "owner": {"pluginId": "dev.example.plugin", "instanceId": "instance-7"}
    }))
    .unwrap();
    assert_eq!(request.method.as_deref(), Some("host.config.get"));
    assert_eq!(request.owner.unwrap().instance_id, "instance-7");
    assert!(request.result.is_none());

    assert_eq!(
        serde_json::to_value(ProcessStreamEnvelope::Item {
            stream_id: "stream-1".into(),
            value: json!({"delta": "hello"}),
        })
        .unwrap(),
        json!({"kind": "item", "streamId": "stream-1", "value": {"delta": "hello"}})
    );
}
