use super::history_from_agent_message;
use crate::agent::events::history_from_part;
use neoism_protocol::agent::{AgentServerMessage, HistoryMessage};
use neoism_ui::panels::agent_pane::{
    api_mapping::{message_blocks_from_response, part_block},
    server_events::{apply_agent_event_to_pane, map_history},
    state::{NeoismAgentMessage, NeoismAgentPane},
};
use serde_json::{json, Value};

fn compose_part(status: &str) -> Value {
    let trace = json!({"code":"return await tools.read();", "value":"selected parent output", "toolCalls":[
        {"id":"read", "path":"neoism.read", "status":"completed", "input":{"filePath":"src/lib.rs"},
         "presentation":{"tool":"read", "output":"<path>src/lib.rs</path>\n<content>\n1: retained child output\n</content>"}},
        {"id":"mcp", "path":"firecrawl.search", "status":"error", "input":{"query":"x"}, "error":"service failed",
         "presentation":{"tool":"execute", "output":"retained MCP failure", "isError":true,
          "metadata":{"mcp":{"client":"firecrawl", "tool":"search", "runtimeId":"firecrawl.search"}}}},
        {"id":"partial", "path":"neoism.grep", "status":"partial_unknown", "input":{"pattern":"x"}, "error":"interrupted"}
    ]});
    let mut part = json!({"id":"compose-part", "messageID":"assistant", "type":"tool", "tool":"compose",
        "state":{"status":status, "input":{"code":"return await tools.read();"}},
        "metadata":{"toolBatchID":"batch"}});
    if status == "running" {
        part["metadata"]["composition"] = trace;
    } else {
        part["state"]["metadata"] = json!({"composition":trace});
    }
    part
}

fn wire_roundtrip(event: AgentServerMessage) -> AgentServerMessage {
    serde_json::from_slice(&serde_json::to_vec(&event).unwrap()).unwrap()
}

fn assert_transported(actual: &NeoismAgentMessage, expected: &NeoismAgentMessage) {
    // Output projection now survives transport too, recursively.
    assert_eq!(actual, expected);
}

#[test]
fn composition_transport_real_part_live_and_history_roundtrip() {
    for status in ["running", "completed"] {
        let part = compose_part(status);
        let expected = part_block(&part).unwrap();
        assert_eq!(expected.composition_children.len(), 3);
        assert!(expected.detail.contains("selected parent output"));
        assert!(!expected.detail.contains("retained child output"));
        let children = &expected.composition_children;
        assert_eq!(children[0].id, "compose-part/compose/read");
        assert_eq!(children[0].tool, "read");
        assert!(children[0].detail.contains("retained child output"));
        assert_eq!(children[1].status, "error");
        assert_eq!(children[1].mcp.as_ref().unwrap().server.as_deref(), Some("firecrawl"));
        assert!(children[1].detail.contains("service failed"));
        assert_eq!(children[2].status, "partial_unknown");
        assert!(children[2].detail.contains("Output not retained"));

        // The exact live SSE lowering and the exact GetHistory expansion.
        let live = history_from_part(&part).unwrap();
        let history = message_blocks_from_response(&[json!({
            "info":{"id":"assistant", "role":"assistant", "sessionID":"session",
                "parentID":"user", "time":{"created":1}, "mode":"build", "agent":"build",
                "path":{"cwd":"/workspace", "root":"/workspace"}, "cost":0,
                "tokens":{"input":0, "output":0, "reasoning":0, "cache":{"read":0, "write":0}},
                "modelID":"test", "providerID":"test"}, "parts":[part]
        })], true).into_iter().map(history_from_agent_message).collect::<Vec<_>>();
        assert_eq!(history, vec![live.clone()]);
        for event in [
            AgentServerMessage::MessageUpdated {
                session_id: "session".into(), message: live,
                parent_message_id: Some("assistant".into()),
            },
            AgentServerMessage::HistoryChunk {
                session_id: "session".into(), messages: history, next_cursor: None,
            },
        ] {
            let mut pane = NeoismAgentPane::new();
            apply_agent_event_to_pane(&mut pane, wire_roundtrip(event));
            let actual = pane.messages().iter().find(|m| m.id == expected.id).unwrap();
            assert_transported(actual, &expected);
            assert!(!actual.composition_child);
            assert!(actual.composition_children.iter().all(|c| c.composition_child));
            for child in &expected.composition_children {
                assert_transported(pane.tool_message_by_id(&child.id).unwrap(), child);
            }
        }
    }
}

#[test]
fn composition_transport_recursive_and_legacy_default() {
    let mut mapped = part_block(&compose_part("completed")).unwrap();
    let nested = mapped.composition_children[1].clone();
    mapped.composition_children[0].composition_children.push(nested);
    let wire = history_from_agent_message(mapped.clone());
    let encoded = serde_json::to_value(&wire).unwrap();
    assert!(encoded["composition_children"][1].get("composition_children").is_none());
    assert_transported(&map_history(serde_json::from_value(encoded).unwrap()), &mapped);

    let mut legacy = serde_json::to_value(wire).unwrap();
    legacy.as_object_mut().unwrap().remove("composition_children");
    let legacy: HistoryMessage = serde_json::from_value(legacy).unwrap();
    assert!(legacy.composition_children.is_empty());
    assert!(!serde_json::to_value(&legacy).unwrap().as_object().unwrap().contains_key("composition_children"));
    assert!(map_history(legacy).composition_children.is_empty());
}