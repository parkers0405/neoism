//! Web-side contract tests for the shared header API used by pointer routing.
use neoism_protocol::agent::AgentClientMessage;
use neoism_ui::panels::agent_pane::{
    conversation_header::Direction,
    outbound::OutboundAgentCommand,
    protocol_mapping::{
        map_outbound_command, AgentProtocolMapping, AgentProtocolMappingContext,
    },
    state::{
        side_panel::{BranchStatus, NeoismAgentSessionEntry},
        NeoismAgentPane,
    },
};

const LINK: [f32; 4] = [10.0, 10.0, 100.0, 24.0];
const NEXT_LINK: [f32; 4] = [120.0, 10.0, 100.0, 24.0];

fn roster(ids: &[(&str, usize)]) -> Vec<NeoismAgentSessionEntry> {
    ids.iter()
        .map(|(id, depth)| {
            let mut entry = NeoismAgentSessionEntry::new(*id, *id, "");
            entry.depth = *depth;
            if *depth > 0 {
                entry.runtime_status = Some("running".into());
                entry.branch_status = Some(BranchStatus::Active);
            }
            entry
        })
        .collect()
}

fn child_pane() -> NeoismAgentPane {
    let mut pane = NeoismAgentPane::default();
    pane.switch_session("root".into());
    pane.replace_input("unsent root draft");
    pane.system_message("Root", "cached root transcript");
    pane.side_panel_mut().set_subagents(roster(&[
        ("root", 0),
        ("parent", 1),
        ("nested", 2),
        ("last", 1),
    ]));
    // Keep every child runtime-active: completed-child pruning must not decide
    // this fixture's visible roster or navigation order.
    for id in ["parent", "nested", "last"] {
        pane.note_subagent_runtime(id.into(), BranchStatus::Active, None);
    }
    pane.switch_session("parent".into());
    pane.switch_session("nested".into());
    pane.drain_pending_outbound();
    assert_eq!(pane.session_id_str(), Some("nested"));
    pane
}

fn register(pane: &mut NeoismAgentPane, direction: Direction, target: &str) {
    pane.conversation_header
        .register(direction, target, LINK, LINK, &[]);
}

fn assert_switch(pane: &mut NeoismAgentPane, from: &str, to: &str) {
    let commands = pane.drain_pending_outbound();
    assert!(matches!(commands.as_slice(),
        [OutboundAgentCommand::SwitchSession { session_id }] if session_id == to));
    let context = AgentProtocolMappingContext {
        active_session_id: Some(from.into()),
        ..Default::default()
    };
    let AgentProtocolMapping::Messages(messages) =
        map_outbound_command(commands.into_iter().next().unwrap(), &context)
    else {
        panic!("navigation must use the existing session-switch mapping");
    };
    assert!(matches!(messages.as_slice(), [
        AgentClientMessage::SwitchThread { session_id },
        AgentClientMessage::GetHistory { session_id: history_id, .. },
        AgentClientMessage::ResumeStream { session_id: stream_id },
    ] if session_id == to && history_id == to && stream_id == to));
    assert_eq!(pane.session_id_str(), Some(to));
    assert!(pane.conversation_header.hit_at(20.0, 20.0).is_none());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn previous_walks_prior_children_then_back_restores_cached_root() {
    let mut pane = child_pane();
    let navigation = pane.conversation_navigation();
    assert_eq!(navigation.previous, Some("parent"));
    assert_eq!(navigation.target(Direction::Previous), Some("parent"));
    assert!(!navigation.previous_is_main);
    register(&mut pane, Direction::Previous, "parent");
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), Some("parent"));
    assert!(pane.activate_conversation_navigation(20.0, 20.0));
    assert_switch(&mut pane, "nested", "parent");

    let navigation = pane.conversation_navigation();
    assert_eq!(navigation.previous, Some("root"));
    assert!(navigation.previous_is_main); // Renderer labels this link Back, not Prev.
    register(&mut pane, Direction::Previous, "root");
    assert!(pane.activate_conversation_navigation(20.0, 20.0));
    assert_switch(&mut pane, "parent", "root");
    assert_eq!(pane.input(), "unsent root draft");
    assert!(!pane.messages().is_empty());
    assert_eq!(pane.side_panel().subagents().len(), 4);
    assert_eq!(pane.conversation_navigation().previous, None);
    assert_eq!(pane.conversation_navigation().next, None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    assert!(!pane.has_pending_outbound());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn next_follows_roster_order_including_nested_children_without_wrapping() {
    let mut pane = child_pane();
    pane.switch_session("parent".into());
    pane.drain_pending_outbound();
    for (from, to) in [("parent", "nested"), ("nested", "last")] {
        let navigation = pane.conversation_navigation();
        assert_eq!(navigation.next, Some(to));
        assert_eq!(navigation.target(Direction::Next), Some(to));
        register(&mut pane, Direction::Next, to);
        assert!(pane.activate_conversation_navigation(20.0, 20.0));
        assert_switch(&mut pane, from, to);
    }
    assert_eq!(pane.conversation_navigation().previous, Some("nested"));
    assert_eq!(pane.conversation_navigation().next, None);
    register(&mut pane, Direction::Next, "root");
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    assert!(!pane.has_pending_outbound());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn both_directions_register_independently_and_clear_together() {
    let mut pane = child_pane();
    register(&mut pane, Direction::Previous, "parent");
    pane.conversation_header
        .register(Direction::Next, "last", NEXT_LINK, NEXT_LINK, &[]);
    let hit = pane.conversation_header.hit_at(20.0, 20.0).unwrap();
    assert_eq!(hit.direction, Direction::Previous);
    assert_eq!(hit.target, "parent");
    assert_eq!(pane.conversation_navigation_at(130.0, 20.0), Some("last"));
    pane.conversation_header.clear();
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert_eq!(pane.conversation_navigation_at(130.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(130.0, 20.0));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn stale_neighbor_and_reordered_direction_are_not_interactive() {
    let mut pane = child_pane();
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    register(&mut pane, Direction::Previous, "root");
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));

    register(&mut pane, Direction::Next, "last");
    pane.side_panel_mut().set_subagents(roster(&[
        ("root", 0),
        ("parent", 1),
        ("nested", 2),
    ]));
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));

    register(&mut pane, Direction::Previous, "parent");
    pane.side_panel_mut().set_subagents(roster(&[
        ("root", 0),
        ("nested", 2),
        ("parent", 1),
    ]));
    assert_eq!(pane.conversation_navigation().next, Some("parent"));
    // Same live target, but now in the opposite direction: the old hit is stale.
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    assert_eq!(pane.session_id_str(), Some("nested"));
    assert!(!pane.has_pending_outbound());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn overlay_occlusion_and_misses_do_not_switch_sessions() {
    let mut pane = child_pane();
    pane.conversation_header
        .register(Direction::Previous, "parent", LINK, LINK, &[LINK]);
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    register(&mut pane, Direction::Previous, "parent");
    pane.set_prompt_picker_rect(Some(LINK));
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    pane.set_prompt_picker_rect(None);
    pane.composer_interactions.occlusion_rects.push(LINK);
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    pane.composer_interactions.occlusion_rects.clear();
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), Some("parent"));
    assert!(!pane.activate_conversation_navigation(200.0, 20.0));
    assert!(!pane.activate_conversation_navigation(f32::NAN, 20.0));
    assert_eq!(pane.session_id_str(), Some("nested"));
    assert!(!pane.has_pending_outbound());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn root_and_unrelated_conversations_have_no_header_action() {
    let mut pane = child_pane();
    for id in ["root", "unrelated"] {
        pane.switch_session(id.into());
        pane.drain_pending_outbound();
        assert_eq!(pane.conversation_navigation().previous, None);
        assert_eq!(pane.conversation_navigation().next, None);
        for direction in [Direction::Previous, Direction::Next] {
            register(&mut pane, direction, "parent");
            assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
            assert!(!pane.activate_conversation_navigation(20.0, 20.0));
        }
        assert!(!pane.has_pending_outbound());
    }
}
