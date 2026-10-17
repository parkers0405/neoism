use super::side_panel::NeoismAgentSessionEntry;
use super::*;
use crate::panels::agent_pane::conversation_header::{ConversationNavigation, Direction};

fn child_pane() -> NeoismAgentPane {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("main".into());
    pane.messages = vec![NeoismAgentMessage::user("parent transcript")];
    pane.input = "parent draft".into();
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "Main", ""),
        NeoismAgentSessionEntry::new("child", "First", "")
            .with_depth(1)
            .with_runtime_status(Some("running".into())),
        NeoismAgentSessionEntry::new("nested", "Nested", "")
            .with_depth(2)
            .with_runtime_status(Some("running".into())),
        NeoismAgentSessionEntry::new("last", "Last", "")
            .with_depth(1)
            .with_runtime_status(Some("running".into())),
    ]);
    for id in ["child", "nested", "last"] {
        pane.apply_history_to_cache(id, vec![NeoismAgentMessage::user(id)], None);
    }
    pane.switch_session("nested".into());
    pane.parent_session_id = Some("child".into());
    pane.drain_pending_outbound();
    pane
}

fn register(pane: &mut NeoismAgentPane, direction: Direction, target: &str) {
    let x = if direction == Direction::Previous {
        10.0
    } else {
        200.0
    };
    let rect = [x, 10.0, 70.0, 22.0];
    pane.conversation_header
        .register(direction, target, rect, rect, &[]);
}

fn activate(pane: &mut NeoismAgentPane, direction: Direction, expected: &str) {
    register(pane, direction, expected);
    let x = if direction == Direction::Previous {
        20.0
    } else {
        210.0
    };
    assert!(pane.activate_conversation_navigation(x, 20.0));
    assert_eq!(pane.session_id.as_deref(), Some(expected));
    assert!(pane.conversation_header.hit_at(x, 20.0).is_none());
}

#[test]
fn conversation_navigation_next_prev_then_back_restores_main_state() {
    let mut pane = child_pane();
    assert_eq!(
        pane.conversation_navigation(),
        ConversationNavigation {
            previous: Some("child"),
            next: Some("last"),
            previous_is_main: false,
        }
    );
    activate(&mut pane, Direction::Next, "last");
    assert_eq!(pane.conversation_navigation().next, None);
    activate(&mut pane, Direction::Previous, "nested");
    activate(&mut pane, Direction::Previous, "child");
    assert!(pane.conversation_navigation().previous_is_main);
    activate(&mut pane, Direction::Previous, "main");
    assert!(pane.parent_session_id.is_none());
    assert_eq!(pane.messages[0].text, "parent transcript");
    assert_eq!(pane.input, "parent draft");
    assert_eq!(pane.side_panel.subagents().len(), 4);
    assert_eq!(
        pane.conversation_navigation(),
        ConversationNavigation::default()
    );
    assert!(pane.drain_pending_outbound().iter().any(|command| matches!(
        command, OutboundAgentCommand::SwitchSession { session_id } if session_id == "main"
    )));
}

#[test]
fn conversation_navigation_rejects_stale_direction_after_roster_reordering() {
    let mut pane = child_pane();
    register(&mut pane, Direction::Previous, "child");
    register(&mut pane, Direction::Next, "last");
    let rows = pane.side_panel.subagents().to_vec();
    pane.side_panel.set_subagents(vec![
        rows[0].clone(),
        rows[3].clone(),
        rows[2].clone(),
        rows[1].clone(),
    ]);
    assert_eq!(pane.conversation_navigation().previous, Some("last"));
    assert_eq!(pane.conversation_navigation().next, Some("child"));
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    assert!(!pane.activate_conversation_navigation(210.0, 20.0));
    assert!(!pane.has_pending_outbound());
}

#[test]
fn conversation_navigation_misses_and_family_switch_clear_both_links() {
    let mut pane = child_pane();
    register(&mut pane, Direction::Previous, "old-main");
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    register(&mut pane, Direction::Next, "last");
    assert!(!pane.activate_conversation_navigation(100.0, 20.0));
    pane.switch_session("unrelated".into());
    assert!(pane.conversation_header.hit_at(20.0, 20.0).is_none());
    assert!(pane.conversation_header.hit_at(210.0, 20.0).is_none());
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    assert!(!pane.activate_conversation_navigation(210.0, 20.0));
}

#[test]
fn conversation_navigation_picker_and_modal_overlays_win_over_header() {
    let mut pane = child_pane();
    register(&mut pane, Direction::Previous, "child");
    let rect = [10.0, 10.0, 70.0, 22.0];
    pane.set_prompt_picker_rect(Some(rect));
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    pane.set_prompt_picker_rect(None);
    pane.composer_interactions.occlusion_rects.push(rect);
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), None);
    assert!(!pane.activate_conversation_navigation(20.0, 20.0));
    pane.composer_interactions.occlusion_rects.clear();
    assert_eq!(pane.conversation_navigation_at(20.0, 20.0), Some("child"));
}

#[test]
fn conversation_navigation_hover_repaints_each_link_independently() {
    let mut pane = child_pane();
    register(&mut pane, Direction::Previous, "child");
    register(&mut pane, Direction::Next, "last");
    assert!(pane.update_link_hover_at(20.0, 20.0));
    assert!(!pane.update_link_hover_at(21.0, 20.0));
    assert!(pane.update_link_hover_at(210.0, 20.0));
    assert!(!pane.update_link_hover_at(211.0, 20.0));
    assert!(pane.update_link_hover_at(100.0, 100.0));
}

#[test]
fn conversation_navigation_new_chat_clears_both_links() {
    let mut pane = child_pane();
    register(&mut pane, Direction::Previous, "child");
    register(&mut pane, Direction::Next, "last");
    pane.start_new_conversation();
    assert!(pane.conversation_header.hit_at(20.0, 20.0).is_none());
    assert!(pane.conversation_header.hit_at(210.0, 20.0).is_none());
    assert_eq!(
        pane.conversation_navigation(),
        ConversationNavigation::default()
    );
}

#[test]
fn conversation_navigation_exists_before_child_messages_arrive() {
    let mut pane = child_pane();
    pane.messages.clear();
    assert!(!pane.has_conversation());
    assert_eq!(pane.conversation_navigation().previous, Some("child"));
    activate(&mut pane, Direction::Previous, "child");
    activate(&mut pane, Direction::Previous, "main");
    assert_eq!(pane.messages[0].text, "parent transcript");
}

#[test]
fn conversation_navigation_skips_hidden_completed_siblings_but_retains_viewed_child() {
    let mut pane = child_pane();
    pane.side_panel
        .set_branch_activity_status_from_recovery("child", BranchStatus::Completed);
    assert_eq!(pane.conversation_navigation().previous, Some("main"));
    assert!(pane.conversation_navigation().previous_is_main);
    pane.side_panel
        .set_branch_activity_status_from_recovery("nested", BranchStatus::Completed);
    assert_eq!(pane.conversation_navigation().next, Some("last"));
    activate(&mut pane, Direction::Next, "last");
    assert_eq!(pane.conversation_navigation().previous, Some("main"));
}
