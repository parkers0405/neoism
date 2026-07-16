use super::*;

struct TestPane {
    side_panel: NeoismAgentSidePanel,
    messages: Vec<NeoismAgentMessage>,
}

impl AgentSidePanelPane for TestPane {
    type Message = NeoismAgentMessage;

    fn side_panel(&self) -> &NeoismAgentSidePanel {
        &self.side_panel
    }

    fn side_panel_mut(&mut self) -> &mut NeoismAgentSidePanel {
        &mut self.side_panel
    }

    fn has_conversation(&self) -> bool {
        true
    }

    fn maybe_refresh_side_panel_sessions(&mut self) {}

    fn maybe_refresh_side_panel_subagents(&mut self) {}

    fn directory_label(&self) -> String {
        String::new()
    }

    fn agent_label(&self) -> &str {
        "agent"
    }

    fn model(&self) -> &str {
        "model"
    }

    fn thinking_label(&self) -> &str {
        "thinking"
    }

    fn usage_detail_lines(&self) -> Vec<String> {
        Vec::new()
    }

    fn messages(&self) -> &[Self::Message] {
        &self.messages
    }

    fn session_id_str(&self) -> Option<&str> {
        None
    }
}

#[test]
fn runtime_status_wins_over_cached_branch_activity() {
    let mut pane = TestPane {
        side_panel: NeoismAgentSidePanel::default(),
        messages: Vec::new(),
    };
    pane.side_panel
        .set_branch_activity_status("child", BranchStatus::Active);
    let entry = NeoismAgentSessionEntry::new("child", "child", "codex")
        .with_runtime_status(Some("completed".to_string()));

    let activity = subagent_row_activity(&pane, &entry, false).unwrap();

    assert_eq!(activity.status, BranchStatus::Completed);
}

#[test]
fn finished_subagent_ignores_straggler_part_activity() {
    // Regression: a sub-agent that finished authoritatively must not
    // be dragged back to "responding"/"working" by a late part-level
    // activity delta from the child.
    let mut panel = NeoismAgentSidePanel::default();

    // Child runs, then finishes via an authoritative lifecycle signal.
    panel.set_branch_activity_status("child", BranchStatus::Active);
    panel.set_branch_activity_status("child", BranchStatus::Completed);
    assert!(panel.branch_terminal_locked("child"));

    // A straggler "responding" part delta arrives after completion.
    let applied = panel.note_subagent_part_activity(
        "child",
        BranchStatus::Active,
        Some("responding".to_string()),
        None,
    );

    assert!(!applied, "late part activity must be dropped");
    let activity = panel.branch_activity("child").unwrap();
    assert_eq!(activity.status, BranchStatus::Completed);
    assert_eq!(activity.current_tool, None);
}

#[test]
fn poll_reconciliation_latches_terminal_for_finished_subagent() {
    // Regression (the bug prior attempts missed): the authoritative backend
    // poll lands via `set_subagents`, NOT `set_branch_activity_status`. A
    // finished child reported by the poll must latch `terminal_locked` so a
    // straggler "responding" part delta can't drag the row back to
    // "working" — which is exactly how branches got stuck.
    let mut panel = NeoismAgentSidePanel::default();

    // Child is mid-run with a live "responding" part activity (not locked).
    panel.note_subagent_part_activity(
        "child",
        BranchStatus::Active,
        Some("responding".to_string()),
        None,
    );
    assert!(!panel.branch_terminal_locked("child"));

    // The poll reports the child finished.
    panel.set_subagents(vec![NeoismAgentSessionEntry::new(
        "child", "child", "explore",
    )
    .with_runtime_status(Some("completed".to_string()))]);

    assert!(
        panel.branch_terminal_locked("child"),
        "authoritative poll completion must latch terminal"
    );
    assert_eq!(
        panel.branch_activity("child").unwrap().status,
        BranchStatus::Completed
    );

    // A late straggler delta after the poll must be dropped.
    let applied = panel.note_subagent_part_activity(
        "child",
        BranchStatus::Active,
        Some("responding".to_string()),
        None,
    );
    assert!(!applied, "straggler after poll completion must be dropped");
    assert_eq!(
        panel.branch_activity("child").unwrap().status,
        BranchStatus::Completed
    );
}

#[test]
fn subagent_poll_keeps_running_while_active_and_stops_when_all_done() {
    // Regression: the poll was one-shot (`subagents_loaded` disabled it
    // forever), so a sub-agent's status froze at first load. It must keep
    // polling while any sub-agent is active, then stop once all terminal.
    let mut panel = NeoismAgentSidePanel::default();

    // First load with a running child: poll must remain armed.
    panel.set_subagents(vec![NeoismAgentSessionEntry::new("a", "a", "explore")
        .with_runtime_status(Some("running".to_string()))]);
    assert!(
        panel.should_refresh_subagents(),
        "must keep polling while a sub-agent is active"
    );

    // Child finishes: nothing left to poll for.
    panel.set_subagents(vec![NeoismAgentSessionEntry::new("a", "a", "explore")
        .with_runtime_status(Some("completed".to_string()))]);
    assert!(
        !panel.should_refresh_subagents(),
        "polling stops once every sub-agent is terminal"
    );
}

#[test]
fn first_seen_completed_subagent_is_hidden_immediately() {
    // Entering chat with an already-finished child must NOT start a fresh
    // 7s window — the row is hidden/pruned right away (no reappearing
    // completed sub-agents).
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "main session", "return"),
        NeoismAgentSessionEntry::new("done", "done", "explore")
            .with_runtime_status(Some("completed".to_string())),
    ]);
    let ids: Vec<&str> = panel.subagents().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["main"],
        "a sub-agent first observed as completed must be hidden, not shown"
    );
}

#[test]
fn live_completion_stamps_the_visibility_window() {
    // A sub-agent the user watched finish (active -> completed) DOES get
    // the 7s window so it lingers briefly before auto-hiding.
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_branch_activity_status("child", BranchStatus::Active);
    panel.set_branch_activity_status("child", BranchStatus::Completed);
    assert!(
        panel
            .branch_activity("child")
            .unwrap()
            .completed_at
            .is_some(),
        "a live active->completed edge must start the show-then-hide window"
    );
}

#[test]
fn respawned_subagent_clears_terminal_lock() {
    // A genuine respawn (authoritative active) re-opens the branch so
    // a re-spawned child reports activity again.
    let mut panel = NeoismAgentSidePanel::default();

    panel.set_branch_activity_status("child", BranchStatus::Completed);
    assert!(panel.branch_terminal_locked("child"));

    panel.set_branch_activity_status("child", BranchStatus::Active);
    assert!(!panel.branch_terminal_locked("child"));

    let applied = panel.note_subagent_part_activity(
        "child",
        BranchStatus::Active,
        Some("thinking".to_string()),
        None,
    );
    assert!(applied);
    let activity = panel.branch_activity("child").unwrap();
    assert_eq!(activity.status, BranchStatus::Active);
    assert_eq!(activity.current_tool.as_deref(), Some("thinking"));
}

#[test]
fn goal_json_parses_status_summary_and_paused() {
    let goal = SessionGoal::from_json(&serde_json::json!({
        "text": "ship the goal feature",
        "status": "blocked",
        "summary": "waiting on a backend field",
        "paused": true
    }))
    .expect("goal with text is parsed");
    assert_eq!(goal.text, "ship the goal feature");
    assert_eq!(goal.status, GoalStatus::Blocked);
    assert_eq!(goal.summary, "waiting on a backend field");
    assert!(goal.paused);
}

#[test]
fn goal_json_empty_text_renders_nothing() {
    assert!(SessionGoal::from_json(&serde_json::Value::Null).is_none());
    assert!(SessionGoal::from_json(&serde_json::json!({ "text": "   " })).is_none());
}

fn goal(text: &str, status: GoalStatus, updated: u64) -> SessionGoal {
    SessionGoal {
        text: text.to_string(),
        status,
        updated,
        ..Default::default()
    }
}

#[test]
fn stale_goal_poll_does_not_clobber_newer_live_goal() {
    // Repro for the flicker: a live event sets the new goal (v2), then a
    // slow poll that was already in flight returns the OLD goal (v1).
    // The stale poll must lose so the section doesn't blink back.
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_session_goal(Some(goal("old goal", GoalStatus::Active, 1)), 1);
    assert_eq!(panel.session_goal().unwrap().text, "old goal");

    // Newer live event wins.
    panel.set_session_goal(Some(goal("new goal", GoalStatus::Active, 2)), 2);
    assert_eq!(panel.session_goal().unwrap().text, "new goal");

    // Stale poll (v1) arrives late — dropped, no flicker.
    panel.set_session_goal(Some(goal("old goal", GoalStatus::Active, 1)), 1);
    assert_eq!(panel.session_goal().unwrap().text, "new goal");
}

#[test]
fn completed_goal_is_retired_from_the_section() {
    // A finished goal goes away like a finished sub-agent; active/blocked
    // stay. The version still advances so a stale poll can't resurrect it.
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_session_goal(Some(goal("ship it", GoalStatus::Active, 1)), 1);
    assert!(panel.session_goal().is_some());

    panel.set_session_goal(Some(goal("ship it", GoalStatus::Complete, 2)), 2);
    assert!(panel.session_goal().is_none(), "completed goal is hidden");

    // A stale poll of the now-completed goal must not bring it back.
    panel.set_session_goal(Some(goal("ship it", GoalStatus::Active, 1)), 1);
    assert!(panel.session_goal().is_none());

    // A genuinely newer goal still shows.
    panel.set_session_goal(Some(goal("next goal", GoalStatus::Active, 3)), 3);
    assert_eq!(panel.session_goal().unwrap().text, "next goal");
}

#[test]
fn blocked_goal_stays_until_a_newer_goal_replaces_it() {
    // A stale earlier goal must never linger over the current one: setting
    // a fresh Active goal replaces a blocked one outright.
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_session_goal(Some(goal("blocked thing", GoalStatus::Blocked, 1)), 1);
    assert_eq!(panel.session_goal().unwrap().status, GoalStatus::Blocked);

    panel.set_session_goal(Some(goal("current thing", GoalStatus::Active, 2)), 2);
    let shown = panel.session_goal().unwrap();
    assert_eq!(shown.text, "current thing");
    assert_eq!(shown.status, GoalStatus::Active);
}

#[test]
fn unversioned_poll_none_never_clears_a_live_goal() {
    // A poll that finds no goal (version 0) must not clear a goal a live
    // event set — authoritative clears arrive versioned.
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_session_goal(Some(goal("ship it", GoalStatus::Active, 5)), 5);
    panel.set_session_goal(None, 0);
    assert!(panel.session_goal().is_some(), "version-0 None is ignored");

    // A versioned clear (newer) does clear it.
    panel.set_session_goal(None, 6);
    assert!(panel.session_goal().is_none());
}

#[test]
fn content_scroll_clamps_to_known_overflow() {
    let mut panel = NeoismAgentSidePanel::default();
    // No overflow yet: scrolling does nothing.
    assert!(!panel.scroll_content_pixels(120.0));
    assert_eq!(panel.content_scroll_px(), 0.0);

    // Once the renderer reports 80px of overflow, the column can scroll
    // up to that bound and no further.
    panel.set_content_scroll_max(80.0);
    assert!(panel.scroll_content_pixels(50.0));
    assert_eq!(panel.content_scroll_px(), 50.0);
    assert!(panel.scroll_content_pixels(1000.0));
    assert_eq!(panel.content_scroll_px(), 80.0);
    // Can't scroll past the top either.
    assert!(panel.scroll_content_pixels(-1000.0));
    assert_eq!(panel.content_scroll_px(), 0.0);
}
