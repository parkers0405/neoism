use super::*;
use crate::panels::agent_pane::api_mapping::part_block;
use serde_json::json;

fn composition() -> NeoismAgentMessage {
    part_block(&json!({"id":"p", "type":"tool", "tool":"compose", "state":{"status":"running", "input":{"code":"return 42;"}},
        "metadata":{"composition":{"toolCalls":[
            {"id":"a", "path":"neoism.read", "input":{"filePath":"src/lib.rs"}, "status":"running", "presentation":{"tool":"read", "output":"one"}},
            {"id":"b", "path":"neoism.read", "input":{"filePath":"src/lib.rs"}, "status":"running", "presentation":{"tool":"read", "output":"two"}}
        ]}}})).unwrap()
}
fn click(pane: &mut NeoismAgentPane, id: &str) {
    pane.tool_hit_rects.clear();
    pane.register_tool_hit_rect(id.into(), [0.0, 0.0, 100.0, 30.0]);
    assert!(pane.toggle_tool_at(10.0, 10.0));
}
#[test]
fn composition_clicks_are_independent_and_parent_preserves_children_on_refresh() {
    let mut pane = NeoismAgentPane::default();
    pane.set_text_reveal_enabled(false);
    let m = composition(); pane.upsert_part_message(m.clone());
    click(&mut pane, "p/compose/a"); click(&mut pane, "p/compose/b");
    assert!(pane.tool_expanded("p/compose/a") && pane.tool_expanded("p/compose/b"));
    assert!(pane.selected_tool_group_child.is_none());
    click(&mut pane, "p"); click(&mut pane, "p");
    pane.upsert_part_message(m.clone());
    pane.apply_history(vec![m]);
    assert!(!pane.tool_expanded("p"));
    assert!(pane.tool_expanded("p/compose/a") && pane.tool_expanded("p/compose/b"));
}
#[test]
fn composition_owner_resolves_live_archive_diff_and_actual_lookup() {
    let mut pane = NeoismAgentPane::default(); pane.upsert_part_message(composition());
    assert_eq!(pane.tool_owner_index("p/compose/a"), Some(0));
    assert_eq!(pane.tool_owner_index("p/compose/a:0"), Some(0));
    assert_eq!(pane.tool_message_by_id("p/compose/a").unwrap().tool, "read");
    assert!(!pane.tool_archived("p/compose/a:0"));
    pane.timeline_live_trace_start = None;
    assert!(pane.tool_archived("p/compose/a:0"));
    click(&mut pane, "p/compose/a:0");
    assert!(pane.timeline_dirty_message_indices.contains(&0));
}
#[test]
fn composition_child_only_update_changes_measure_and_virtual_identity_and_motion() {
    let mut pane = NeoismAgentPane::default(); let mut m = composition();
    pane.upsert_part_message(m.clone());
    let key = NeoismAgentPane::timeline_measure_key(&m, 500.0, 1.0, false, false);
    let virtual_before = virtual_agent_markdown(&m);
    let epoch = pane.timeline_content_revision;
    m.composition_children[0].status = "completed".into();
    m.composition_children[0].detail = "new child-only output".into();
    pane.upsert_part_message(m.clone());
    assert_ne!(key, NeoismAgentPane::timeline_measure_key(&m, 500.0, 1.0, false, false));
    assert_ne!(virtual_before, virtual_agent_markdown(&m));
    assert!(!virtual_agent_markdown(&m).contains("new child-only output"));
    assert!(pane.timeline_content_revision > epoch);
    assert!(pane.timeline_dirty_message_indices.contains(&0));
    assert_eq!(pane.tool_motion_sample("p/compose/a").outgoing_status.as_deref(), Some("running"));
    click(&mut pane, "p/compose/a");
    assert!(pane.tool_expand_animating("p"));
}
