use super::*;
use crate::panels::agent_pane::state::side_panel::{GoalStatus, NeoismAgentSemanticMatch, SidePanelMode};

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

    fn context_usage(&self) -> Option<(u64, Option<u64>)> {
        None
    }

    fn messages(&self) -> &[Self::Message] {
        &self.messages
    }

    fn session_id_str(&self) -> Option<&str> {
        None
    }
}

#[test]
fn context_meter_fraction_preserves_usage_policy_edges() {
    assert_eq!(sections::context_fill_fraction(100, Some(400)), 0.25);
    assert_eq!(sections::context_fill_fraction(500, Some(400)), 1.0);
    assert_eq!(sections::context_fill_fraction(100, Some(0)), 0.35);
    assert_eq!(sections::context_fill_fraction(100, None), 0.35);
}

#[test]
fn context_meter_uses_compact_monochrome_label() {
    let label = sections::context_count_label(32_100, Some(400_000));
    assert_eq!(label, "32.1k / 400.0k tokens");
    assert!(!label.to_ascii_uppercase().contains("INPUT"));
    assert!(!label.to_ascii_uppercase().contains("OUTPUT"));
    assert_eq!(
        sections::context_count_label(1_200_000, None),
        "1.2m tokens"
    );
}

#[test]
fn usage_scramble_rearms_only_for_changed_values() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.update_usage_meter(32_100, Some(400_000));
    let revision = panel.usage_scramble_revision();
    assert!(panel.usage_scramble_elapsed_ms().is_some());

    panel.update_usage_meter(32_100, Some(400_000));
    assert_eq!(panel.usage_scramble_revision(), revision);

    panel.update_usage_meter(32_101, Some(400_000));
    assert_eq!(panel.usage_scramble_revision(), revision + 1);

    panel.update_usage_meter(32_101, Some(128_000));
    assert_eq!(panel.usage_scramble_revision(), revision + 2);
}

#[test]
fn usage_meter_hit_rect_is_dedicated_and_clearable() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_usage_rect([10.0, 20.0, 100.0, 30.0]);
    assert!(panel.usage_contains(50.0, 35.0));
    assert!(!panel.usage_contains(5.0, 35.0));
    panel.clear_usage_rect();
    assert!(!panel.usage_contains(50.0, 35.0));
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
fn snapshot_reconciliation_latches_terminal_for_finished_subagent() {
    // Regression (the bug prior attempts missed): the authoritative backend
    // snapshot lands via `set_subagents`, NOT `set_branch_activity_status`. A
    // finished child reported by the snapshot must latch `terminal_locked` so a
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

    // The recovery snapshot reports the child finished.
    panel.set_subagents(vec![NeoismAgentSessionEntry::new(
        "child", "child", "explore",
    )
    .with_runtime_status(Some("completed".to_string()))]);

    assert!(
        panel.branch_terminal_locked("child"),
        "authoritative snapshot completion must latch terminal"
    );
    assert_eq!(
        panel.branch_activity("child").unwrap().status,
        BranchStatus::Completed
    );

    // A late straggler delta after the snapshot must be dropped.
    let applied = panel.note_subagent_part_activity(
        "child",
        BranchStatus::Active,
        Some("responding".to_string()),
        None,
    );
    assert!(
        !applied,
        "straggler after snapshot completion must be dropped"
    );
    assert_eq!(
        panel.branch_activity("child").unwrap().status,
        BranchStatus::Completed
    );
}

#[test]
fn subagent_snapshot_refreshes_only_when_invalidated_by_an_event() {
    let mut panel = NeoismAgentSidePanel::default();

    // Bootstrap performs exactly one snapshot.
    assert!(panel.should_refresh_subagents());
    let generation = panel.begin_subagent_refresh().expect("bootstrap refresh");
    assert!(panel.complete_subagent_refresh(generation));
    panel.set_subagents(vec![NeoismAgentSessionEntry::new("a", "a", "explore")
        .with_runtime_status(Some("running".to_string()))]);
    assert!(
        !panel.should_refresh_subagents(),
        "active lifecycle must be event-driven, not periodically polled"
    );

    // A child/tree or reconnect event explicitly requests one recovery
    // snapshot, then the gate closes again.
    panel.mark_subagent_tree_dirty();
    assert!(panel.should_refresh_subagents());
    let generation = panel.begin_subagent_refresh().expect("event refresh");
    assert!(panel.complete_subagent_refresh(generation));
    panel.set_subagents(vec![NeoismAgentSessionEntry::new("a", "a", "explore")
        .with_runtime_status(Some("running".to_string()))]);
    assert!(!panel.should_refresh_subagents());
}

#[test]
fn failed_subagent_snapshot_waits_for_the_next_event() {
    let mut panel = NeoismAgentSidePanel::default();
    let generation = panel.begin_subagent_refresh().expect("bootstrap refresh");
    assert!(panel.complete_subagent_refresh(generation));
    panel.settle_failed_subagent_refresh();
    assert!(!panel.should_refresh_subagents());

    panel.mark_subagent_tree_dirty();
    assert!(panel.should_refresh_subagents());
}

#[test]
fn subagent_refresh_is_single_flight_and_rejects_stale_generations() {
    let mut panel = NeoismAgentSidePanel::default();

    let first = panel.begin_subagent_refresh().expect("first refresh");
    assert!(
        panel.begin_subagent_refresh().is_none(),
        "an in-flight refresh must serialize later polls"
    );

    // A live tree event invalidates the worker that was already fetching.
    panel.mark_subagent_tree_dirty();
    let second = panel.begin_subagent_refresh().expect("replacement refresh");
    assert_ne!(first, second);
    assert!(
        !panel.complete_subagent_refresh(first),
        "the old worker must not complete the new generation"
    );
    assert!(panel.complete_subagent_refresh(second));
}

#[test]
fn partial_recovery_snapshot_preserves_omitted_active_subagent() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    assert_eq!(panel.active_child_count(Some("main")), 1);

    // A stale/partial snapshot knows only about the root. The live active
    // child must remain visible and keep the footer count stable.
    panel.set_subagents(vec![NeoismAgentSessionEntry::new(
        "main",
        "main session",
        "return",
    )]);
    assert_eq!(
        panel
            .subagents()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["main", "child"]
    );
    assert_eq!(panel.active_child_count(Some("main")), 1);

    // An authoritative terminal edge permits the next snapshot to remove it.
    panel.set_branch_activity_status("child", BranchStatus::Completed);
    panel.set_subagents(vec![NeoismAgentSessionEntry::new(
        "main",
        "main session",
        "return",
    )]);
    assert_eq!(
        panel
            .subagents()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["main"]
    );
}

#[test]
fn status_omission_for_present_child_preserves_subtask_lifecycle() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);

    // `/session/status` is only the live SessionRun set. Omission cannot
    // terminalize the separate parent-owned task branch.
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(None),
    ]);

    assert_eq!(panel.active_child_count(Some("main")), 1);
    assert_eq!(
        panel
            .subagents()
            .iter()
            .find(|entry| entry.id == "child")
            .and_then(|entry| entry.runtime_status.as_deref()),
        Some("running")
    );
}

#[test]
fn stale_running_snapshot_cannot_resurrect_a_completed_subagent() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    panel.set_branch_activity_status("child", BranchStatus::Completed);

    // This snapshot began before the live completion event and still says
    // running. It must not restart the footer timer or active count.
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);

    assert_eq!(panel.active_child_count(Some("main")), 0);
    assert_eq!(
        panel
            .branch_activity("child")
            .map(|activity| activity.status),
        Some(BranchStatus::Completed)
    );
    assert_eq!(
        panel
            .subagents()
            .iter()
            .find(|entry| entry.id == "child")
            .and_then(|entry| entry.runtime_status.as_deref()),
        Some("completed"),
        "the visible row must retain the live terminal edge too"
    );
}

#[test]
fn live_completion_updates_parent_sidebar_without_opening_child() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "Review changes", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);

    // This is the lifecycle edge the parent stream receives at the same time
    // its inline Task card becomes completed. No child navigation or recovery
    // snapshot should be needed for the Branches row to catch up.
    panel.set_branch_activity_status("child", BranchStatus::Completed);

    let child = panel
        .subagents()
        .iter()
        .find(|entry| entry.id == "child")
        .expect("child remains visible for the completion grace window");
    assert_eq!(child.runtime_status.as_deref(), Some("completed"));
    assert_eq!(panel.active_child_count(Some("parent")), 0);
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
fn completed_subagent_stays_visible_while_its_transcript_is_open() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_viewed_session_id(Some("done".to_string()));
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "main session", "return"),
        NeoismAgentSessionEntry::new("done", "done", "explore")
            .with_runtime_status(Some("completed".to_string())),
    ]);

    assert!(panel.subagents().iter().any(|entry| entry.id == "done"));

    panel.set_viewed_session_id(Some("main".to_string()));
    assert!(panel.prune_expired_completed_subagents());
    assert!(!panel.subagents().iter().any(|entry| entry.id == "done"));
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
fn completed_goal_hides_but_a_newer_goal_still_appears() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_session_goal(Some(goal("ship it", GoalStatus::Active, 1)), 1);
    assert!(panel.session_goal().is_some());

    panel.set_session_goal(Some(goal("ship it", GoalStatus::Complete, 2)), 2);
    assert!(panel.session_goal().is_none(), "completed goal is hidden");

    // A stale active poll cannot resurrect the completed goal.
    panel.set_session_goal(Some(goal("ship it", GoalStatus::Active, 1)), 1);
    assert!(panel.session_goal().is_none());

    // A genuinely newer goal starts cleanly after completion.
    panel.set_session_goal(Some(goal("next goal", GoalStatus::Active, 3)), 3);
    let replacement = panel.session_goal().expect("newer goal appears");
    assert_eq!(replacement.text, "next goal");
    assert_eq!(replacement.status, GoalStatus::Active);
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
fn home_override_toggles_without_ending_chat() {
    // The "← Back" affordance flips the sessions view on/off; it never
    // touches the conversation, so this is pure view-state.
    let mut panel = NeoismAgentSidePanel::default();
    assert!(!panel.show_home_override());
    panel.toggle_home_override();
    assert!(panel.show_home_override());
    panel.toggle_home_override();
    assert!(!panel.show_home_override());
    panel.set_show_home_override(true);
    assert!(panel.show_home_override());
}

#[test]
fn running_dot_predicate_only_lights_active_sessions() {
    let running = NeoismAgentSessionEntry::new("a", "a", "")
        .with_runtime_status(Some("running".to_string()));
    assert!(session_entry_is_running(&running));

    let busy = NeoismAgentSessionEntry::new("b", "b", "")
        .with_runtime_status(Some("busy".to_string()));
    assert!(session_entry_is_running(&busy));

    let done = NeoismAgentSessionEntry::new("c", "c", "")
        .with_runtime_status(Some("completed".to_string()));
    assert!(!session_entry_is_running(&done));

    let blocked = NeoismAgentSessionEntry::new("d", "d", "")
        .with_runtime_status(Some("blocked".to_string()));
    assert!(!session_entry_is_running(&blocked));

    let idle = NeoismAgentSessionEntry::new("e", "e", "");
    assert!(!session_entry_is_running(&idle));
}

#[test]
fn back_affordance_joins_the_focus_chain() {
    // Arrow-up walks to the "← Back" affordance at the top of the panel,
    // and arrow-down walks back off it — mirroring the search-row hop.
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_mode(SidePanelMode::Subagents);
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("main", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    // Not focusable / not back-reachable until the button is actually drawn.
    assert!(!panel.back_focused());
    panel.set_back_button_rect([0.0, 0.0, 100.0, 20.0]);
    assert!(panel.focusable());

    // Cursor starts on the main row; arrow-up reaches Back.
    panel.select_prev();
    assert!(panel.back_focused());

    // Arrow-down drops back onto the first branch row.
    panel.select_next();
    assert!(!panel.back_focused());
    assert_eq!(panel.selected_index(), 0);

    // Dropping focus clears the Back cursor, and clearing the button rect
    // makes it un-reachable again.
    panel.focus_back();
    assert!(panel.back_focused());
    panel.set_focused(false);
    assert!(!panel.back_focused());
    panel.clear_back_button_rect();
    panel.focus_back();
    assert!(!panel.back_focused());
}

#[test]
fn only_back_focusable_when_no_list_rows() {
    // A chat with no real branches (just the main session) is focusable
    // solely via the Back affordance.
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_mode(SidePanelMode::Subagents);
    panel.set_subagents(vec![NeoismAgentSessionEntry::new(
        "main",
        "main session",
        "return",
    )]);
    panel.set_back_button_rect([0.0, 0.0, 100.0, 20.0]);
    assert!(panel.only_back_focusable());
    assert!(panel.focusable());
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

#[test]
fn semantic_matches_inject_selectable_excerpt_rows_under_their_session() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        NeoismAgentSessionEntry::new("ses-a", "Fix the parser", "1d"),
        NeoismAgentSessionEntry::new("ses-b", "Unrelated title", "2d"),
    ]);
    panel.set_session_query("tokenizer".to_string());
    // Title filter alone matches nothing.
    assert!(panel.sessions().iter().all(|entry| entry.is_header || entry.title != "Unrelated title"));

    panel.set_semantic_results(
        "tokenizer".to_string(),
        vec![NeoismAgentSemanticMatch {
            session_id: "ses-b".to_string(),
            excerpt: "we rewrote the   tokenizer
to handle unicode boundaries".to_string(),
            distance: 0.12,
        }],
    );

    let rows = panel.sessions();
    let session_ix = rows
        .iter()
        .position(|entry| entry.id == "ses-b" && !entry.is_excerpt)
        .expect("semantically matched session joins the filtered list");
    let excerpt_lines: Vec<&NeoismAgentSessionEntry> = rows[session_ix + 1..]
        .iter()
        .take_while(|entry| entry.is_excerpt)
        .collect();
    assert!(!excerpt_lines.is_empty(), "excerpt rows render under their session");
    assert!(
        excerpt_lines.iter().all(|entry| entry.id == "ses-b"),
        "activating any excerpt line resumes the session"
    );
    let joined = excerpt_lines
        .iter()
        .map(|entry| entry.title.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        joined,
        "we rewrote the tokenizer to handle unicode boundaries",
        "whitespace collapses and the chunk word-wraps across rows"
    );
    assert!(
        excerpt_lines
            .iter()
            .all(|entry| entry.title.chars().count() <= 42),
        "each wrapped line fits the column budget"
    );

    // Typing past the fetched query drops the stale excerpt rows.
    panel.set_session_query("tokenizer rewrite".to_string());
    assert!(panel.sessions().iter().all(|entry| !entry.is_excerpt));
}

#[test]
fn excerpt_rows_highlight_every_search_term_occurrence_case_insensitively() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![NeoismAgentSessionEntry::new("ses-a", "Untitled", "1d")]);
    panel.set_session_query("Token stream".to_string());
    panel.set_semantic_results(
        "Token stream".to_string(),
        vec![NeoismAgentSemanticMatch {
            session_id: "ses-a".to_string(),
            excerpt: "the TOKEN stream retokenizes each token per stream".to_string(),
            distance: 0.0,
        }],
    );

    let excerpts: Vec<&NeoismAgentSessionEntry> = panel
        .sessions()
        .iter()
        .filter(|entry| entry.is_excerpt)
        .collect();
    assert!(!excerpts.is_empty());
    let highlighted: Vec<String> = excerpts
        .iter()
        .flat_map(|entry| {
            entry
                .highlights
                .iter()
                .map(|&(start, end)| entry.title[start..end].to_ascii_lowercase())
        })
        .collect();
    // Both terms, all occurrences, regardless of case — including the
    // "token" inside "retokenizes".
    assert!(highlighted.iter().filter(|hit| hit.contains("token")).count() >= 3);
    assert!(highlighted.iter().filter(|hit| hit.contains("stream")).count() >= 2);
    // Ranges are sorted, non-overlapping, and on char boundaries.
    for entry in &excerpts {
        let mut previous_end = 0;
        for &(start, end) in &entry.highlights {
            assert!(start >= previous_end && end > start && end <= entry.title.len());
            assert!(entry.title.is_char_boundary(start) && entry.title.is_char_boundary(end));
            previous_end = end;
        }
    }
}

#[test]
fn overlapping_term_matches_merge_into_one_highlight_range() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![NeoismAgentSessionEntry::new("ses-a", "Untitled", "1d")]);
    panel.set_session_query("streams stream".to_string());
    panel.set_semantic_results(
        "streams stream".to_string(),
        vec![NeoismAgentSemanticMatch {
            session_id: "ses-a".to_string(),
            excerpt: "streams everywhere".to_string(),
            distance: 0.0,
        }],
    );
    let entry = panel
        .sessions()
        .iter()
        .find(|entry| entry.is_excerpt)
        .expect("excerpt row")
        .clone();
    // "stream" and "streams" overlap over the same word — one merged range.
    let over_streams: Vec<_> = entry
        .highlights
        .iter()
        .filter(|&&(start, _)| start == entry.title.find("streams").unwrap())
        .collect();
    assert_eq!(over_streams.len(), 1);
    assert_eq!(
        &entry.title[over_streams[0].0..over_streams[0].1],
        "streams"
    );
}

#[test]
fn long_excerpts_wrap_to_three_lines_and_end_with_an_ellipsis() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![NeoismAgentSessionEntry::new("ses-a", "Session", "1d")]);
    panel.set_session_query("needle".to_string());
    panel.set_semantic_results(
        "needle".to_string(),
        vec![NeoismAgentSemanticMatch {
            session_id: "ses-a".to_string(),
            excerpt: "word ".repeat(200),
            distance: 0.2,
        }],
    );
    let lines: Vec<String> = panel
        .sessions()
        .iter()
        .filter(|entry| entry.is_excerpt)
        .map(|entry| entry.title.clone())
        .collect();
    assert_eq!(lines.len(), 3, "chunks cap at three wrapped rows");
    assert!(lines.iter().all(|line| line.chars().count() <= 42));
    assert!(lines.last().unwrap().ends_with('…'));
}

#[test]
fn excerpt_wrap_follows_the_measured_column_budget() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![NeoismAgentSessionEntry::new("ses-a", "Session", "1d")]);
    panel.set_session_query("needle".to_string());
    panel.set_semantic_results(
        "needle".to_string(),
        vec![NeoismAgentSemanticMatch {
            session_id: "ses-a".to_string(),
            excerpt: "alpha beta gamma delta epsilon zeta".to_string(),
            distance: 0.2,
        }],
    );
    // A narrower panel re-wraps the same chunk into more, shorter rows.
    panel.set_result_wrap_columns(16);
    let narrow: Vec<String> = panel
        .sessions()
        .iter()
        .filter(|entry| entry.is_excerpt)
        .map(|entry| entry.title.clone())
        .collect();
    assert!(narrow.len() >= 2);
    assert!(narrow.iter().all(|line| line.chars().count() <= 16));

    panel.set_result_wrap_columns(160);
    let wide: Vec<String> = panel
        .sessions()
        .iter()
        .filter(|entry| entry.is_excerpt)
        .map(|entry| entry.title.clone())
        .collect();
    assert_eq!(wide, vec!["alpha beta gamma delta epsilon zeta".to_string()]);
}
