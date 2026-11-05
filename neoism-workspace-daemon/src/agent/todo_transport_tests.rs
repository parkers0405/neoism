use super::history_from_agent_message;
use crate::agent::events::history_from_part;
use neoism_protocol::agent::{HistoryMessage, HistoryOutputKind};
use neoism_ui::panels::agent_pane::{
    icon::AgentKind,
    server_events::map_history,
    state::{NeoismAgentMessage, NeoismAgentOutputKind, NeoismAgentPane},
};
use serde_json::json;

fn part(state: serde_json::Value) -> HistoryMessage {
    history_from_part(&json!({"id":"call", "type":"tool", "tool":"todowrite", "state":state})).unwrap()
}

fn roundtrip(message: HistoryMessage) -> NeoismAgentMessage {
    map_history(serde_json::from_slice(&serde_json::to_vec(&message).unwrap()).unwrap())
}

fn full() -> HistoryMessage {
    part(json!({"status":"completed", "input":{"todos":[{"content":"First", "status":"completed"}]}}))
}

#[test]
fn todos_transport_empty_clear_and_missing_metadata_remain_distinct() {
    let full = full();
    let clear = part(json!({"status":"completed", "input":{"todos":[]}}));
    let metadata = part(json!({"status":"completed"}));
    assert_eq!(full.output_kind, Some(HistoryOutputKind::Todos));
    assert_eq!(clear.output_kind, Some(HistoryOutputKind::Todos));
    assert_eq!(metadata.output_kind, Some(HistoryOutputKind::Text));
    let mut pane = NeoismAgentPane::new();
    pane.upsert_part_message(roundtrip(full.clone()));
    pane.upsert_part_message(roundtrip(metadata.clone()));
    assert_eq!(pane.messages()[0].todos.len(), 1);
    pane.upsert_part_message(roundtrip(clear.clone()));
    pane.upsert_part_message(roundtrip(metadata.clone()));
    assert!(pane.messages()[0].todos.is_empty());
    pane.apply_history(vec![roundtrip(metadata)]);
    assert!(pane.messages()[0].todos.is_empty());
    assert_eq!(pane.messages()[0].output_kind, NeoismAgentOutputKind::Todos);
    pane.apply_todos(vec![]);
    assert_eq!(pane.messages().len(), 1, "canonical empty checklist needs no duplicate marker");

    // The same explicit clear also survives the full-history replacement path.
    pane.upsert_part_message(roundtrip(full));
    pane.apply_history(vec![roundtrip(clear)]);
    assert!(pane.messages()[0].todos.is_empty());
}

#[test]
fn todos_transport_completed_history_is_preserved_but_empty_plan_masks_projection() {
    let mut pane = NeoismAgentPane::new();
    pane.upsert_part_message(roundtrip(full()));
    let historical = pane.messages()[0].clone();
    pane.apply_todos(vec![]);
    assert_eq!(pane.messages()[0], historical);
    assert_eq!(pane.messages().len(), 2);
    let latest = pane.messages().iter().rev().find(|m| m.output_kind == NeoismAgentOutputKind::Todos).unwrap();
    assert!(latest.todos.is_empty());
    assert_eq!(latest.id, "todos-snapshot");
    let wire_marker = history_from_agent_message(latest.clone());
    assert_eq!(roundtrip(wire_marker).output_kind, NeoismAgentOutputKind::Todos);

    let metadata = part(json!({"status":"completed"}));
    pane.upsert_part_message(roundtrip(metadata.clone()));
    pane.apply_history(vec![roundtrip(metadata)]);
    assert_eq!(pane.messages()[0].id, historical.id);
    assert_eq!(pane.messages()[0].todos, historical.todos);
    assert!(pane.messages().last().unwrap().todos.is_empty());
    assert_eq!(pane.messages().last().unwrap().output_kind, NeoismAgentOutputKind::Todos);
}

#[test]
fn todos_transport_recursive_output_kind_and_legacy_inference() {
    let mut parent = part(json!({"status":"completed"}));
    parent.composition_children = vec![part(json!({"status":"completed", "input":{"todos":[]}}))];
    let mapped = roundtrip(parent);
    assert_eq!(mapped.composition_children[0].output_kind, NeoismAgentOutputKind::Todos);
    let lowered = history_from_agent_message(mapped);
    assert_eq!(lowered.composition_children[0].output_kind, Some(HistoryOutputKind::Todos));

    let mut legacy = serde_json::to_value(full()).unwrap();
    legacy.as_object_mut().unwrap().remove("output_kind");
    let legacy: HistoryMessage = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.output_kind, None);
    assert!(serde_json::to_value(&legacy).unwrap().get("output_kind").is_none());
    assert_eq!(roundtrip(legacy).output_kind, NeoismAgentOutputKind::Todos);
}

#[test]
fn task_agent_identity_survives_reconstruction_and_recursive_history_transport() {
    let mut messages = Vec::new();
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::OpenCode, AgentKind::Neoism] {
        let message = history_from_part(&json!({
            "id": kind.id(), "type": "tool", "tool": "task",
            "state": {
                "status": "completed",
                "metadata": {"agent": kind.id()},
                "input": {"description": "Unbranded task", "subagent_type": "explore"}
            }
        })).unwrap();
        assert_eq!(message.agent_kind.as_deref(), Some(kind.id()));
        assert_eq!(serde_json::to_value(&message).unwrap()["agent_kind"], kind.id());
        let mapped = roundtrip(message);
        assert_eq!(mapped.agent_kind, Some(kind));
        let mut pane = NeoismAgentPane::new();
        pane.apply_history(vec![mapped]);
        assert_eq!(pane.messages()[0].agent_kind, Some(kind));
        messages.push(history_from_agent_message(pane.messages()[0].clone()));
    }

    let mut parent = messages.remove(0);
    parent.composition_children = messages;
    let mapped = roundtrip(parent.clone());
    assert_eq!(mapped.agent_kind, Some(AgentKind::Claude));
    assert_eq!(mapped.composition_children.iter().map(|child| child.agent_kind).collect::<Vec<_>>(),
        vec![Some(AgentKind::Codex), Some(AgentKind::OpenCode), Some(AgentKind::Neoism)]);
    assert_eq!(history_from_agent_message(mapped), parent);
}

#[test]
fn task_agent_history_absence_and_unknown_identity_do_not_infer_from_labels() {
    let legacy = json!({
        "id": "legacy-task", "role": "Assistant", "kind": "Tool", "tool": "task",
        "title": "Claude Codex OpenCode", "text": "openai/codex"
    });
    let message: HistoryMessage = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(message.agent_kind, None);
    assert!(serde_json::to_value(&message).unwrap().get("agent_kind").is_none());
    let mapped = roundtrip(message);
    assert_eq!(mapped.agent_kind, None);
    assert!(serde_json::to_value(history_from_agent_message(mapped)).unwrap().get("agent_kind").is_none());

    for identity in [serde_json::Value::Null, json!("unknown"), json!("ask Claude"), json!("openai/codex")] {
        let mut wire = legacy.clone();
        wire["agent_kind"] = identity;
        assert_eq!(roundtrip(serde_json::from_value(wire).unwrap()).agent_kind, None);
    }
}