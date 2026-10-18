use super::*;
use neoism_agent_core::{CatalogBranchSnapshot, CatalogFamilySnapshot};

fn catalog() -> NeoismAgentSidePanel {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_catalog_directory(Some("/work/a".into()));
    panel.set_sessions(vec![
        NeoismAgentSessionEntry::new("root", "Parent", "")
            .with_directory(Some("/work/a".into()))
            .with_updated_ms(20),
        NeoismAgentSessionEntry::new("next", "Next conversation", "")
            .with_directory(Some("/work/a".into()))
            .with_updated_ms(10),
    ]);
    panel
}

fn snapshot(revision: u64, branches: &[(&str, &str)]) -> CatalogFamilySnapshot {
    CatalogFamilySnapshot {
        execution_id: Some("run-1".into()),
        family_revision: revision,
        directory: "/work/a".into(),
        branches: branches.iter().map(|(id, status)| CatalogBranchSnapshot {
            session_id: (*id).into(),
            parent_session_id: "root".into(),
            status: (*status).into(),
            started_at: Some(123),
            title: format!("Task {id}"),
            agent: Some("explore".into()),
            depth: 1,
            permission_pending: false,
        }).collect(),
    }
}

fn index(panel: &NeoismAgentSidePanel, id: &str) -> usize {
    panel.sessions().iter().position(|row| row.id == id).unwrap()
}

fn has(panel: &NeoismAgentSidePanel, id: &str) -> bool {
    panel.sessions().iter().any(|row| row.id == id)
}

#[test]
fn feed_children_are_keyboard_stops_and_pointer_targets() {
    let mut panel = catalog();
    assert!(panel.apply_catalog_family_snapshot("root", snapshot(1, &[
        ("child", "outstanding"), ("sibling", "outstanding"),
    ])));
    panel.set_selected(index(&panel, "root"));
    for expected in ["child", "sibling", "next"] {
        panel.select_next();
        assert_eq!(panel.selected_session().unwrap().id, expected);
    }
    panel.select_prev();
    assert_eq!(panel.selected_session().unwrap().id, "sibling");
    let child = &panel.sessions()[index(&panel, "child")];
    assert_eq!(child.branch_root_id.as_deref(), Some("root"));
    assert_eq!(child.directory.as_deref(), Some("/work/a"));
    assert_eq!(child.depth, 1);
    assert_eq!(child.branch_status, Some(BranchStatus::Active));
    assert_eq!(panel.sessions()[index(&panel, "root")].catalog_activity, None);
    panel.set_last_panel_height_rows(12);
    let height = panel.row_height();
    let rect = [0., 100., 240., height * 12.];
    panel.set_last_panel_rect(rect);
    panel.set_row_hit_rect(rect, height);
    let child = index(&panel, "child");
    assert_eq!(panel.hit_test_row(80., 100. + (child as f32 + 0.5) * height, rect), Some(child));
    assert!(panel.catalog_clickable_at(80., 100. + (child as f32 + 0.5) * height));
}

#[test]
fn families_replace_and_delete_independently_preserving_selection() {
    let mut panel = catalog();
    panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")]));
    panel.apply_catalog_family_snapshot("next", snapshot(1, &[("other", "outstanding")]));
    panel.set_selected(index(&panel, "other"));
    assert!(panel.apply_catalog_family_snapshot("root", snapshot(2, &[])));
    assert!(!has(&panel, "child"));
    assert!(has(&panel, "other"));
    assert_eq!(panel.selected_session().unwrap().id, "other");
    // An empty replacement retains its watermark, so older content cannot return.
    assert!(!panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")])));
    panel.remove_session("root");
    assert!(!panel.catalog_families.contains_key("root"));
    assert!(!panel.apply_catalog_family_snapshot("root", snapshot(3, &[("child", "outstanding")])));
    assert!(has(&panel, "other"));
    panel.remove_session("other");
    panel.tick_scroll();
    assert!(!has(&panel, "other"));
    assert!(!panel.catalog_families["next"].activities.contains_key("other"));
}

#[test]
fn scoped_reconnect_keeps_other_projects_observed() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        NeoismAgentSessionEntry::new("root", "A", "")
            .with_directory(Some("/work/a".into())),
        NeoismAgentSessionEntry::new("next", "B", "")
            .with_directory(Some("/work/b".into())),
    ]);
    panel.apply_catalog_family_snapshot("root", snapshot(2, &[("child", "outstanding")]));
    let mut other = snapshot(2, &[("other", "outstanding")]);
    other.directory = "/work/b".into();
    panel.apply_catalog_family_snapshot("next", other);
    panel.set_selected(index(&panel, "other"));
    assert!(panel.clear_catalog_families_in_scope(Some("/work/a")));
    assert!(!has(&panel, "child"));
    assert!(has(&panel, "other"));
    assert_eq!(panel.selected_session().unwrap().id, "other");
    assert!(!panel.catalog_families.contains_key("root"));
    assert!(panel.catalog_families.contains_key("next"));
    assert!(panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")])));
    assert!(panel.clear_catalog_families_in_scope(None));
    assert!(!has(&panel, "child"));
    assert!(!has(&panel, "other"));
}

#[test]
fn completion_expires_without_any_open_chat_and_never_restarts_grace() {
    let mut panel = catalog();
    assert!(panel.subagents().is_empty());
    panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")]));
    panel.apply_catalog_family_snapshot("root", snapshot(2, &[("child", "completed")]));
    assert_eq!(panel.sessions()[index(&panel, "child")].branch_status, Some(BranchStatus::Completed));
    assert!(panel.catalog_is_animating());
    let activity = panel.catalog_families.get_mut("root").unwrap().activities.get_mut("child").unwrap();
    activity.completed_at = Some(Instant::now() - SUBAGENT_HIDE_AFTER - Duration::from_secs(1));
    panel.set_selected(index(&panel, "next"));
    // The existing renderer tick performs expiry; no pane lifecycle calls needed.
    panel.tick_scroll();
    assert!(!has(&panel, "child"));
    assert!(!panel.catalog_completion_pending());
    assert_eq!(panel.selected_session().unwrap().id, "next");
    panel.apply_catalog_family_snapshot("root", snapshot(3, &[("child", "completed")]));
    assert!(!has(&panel, "child"));
    panel.apply_catalog_family_snapshot("root", snapshot(4, &[]));
    panel.apply_catalog_family_snapshot("root", snapshot(5, &[("child", "completed")]));
    assert!(!has(&panel, "child"));
    // Same/lower lifecycle revisions cannot revive a terminal child.
    panel.apply_catalog_family_snapshot("root", snapshot(5, &[("child", "outstanding")]));
    assert!(!has(&panel, "child"));
    assert!(!panel.apply_catalog_family_snapshot("root", snapshot(4, &[("child", "outstanding")])));
    // A genuine Task follow-up can reopen the child in the same execution.
    panel.apply_catalog_family_snapshot("root", snapshot(6, &[("child", "outstanding")]));
    assert!(has(&panel, "child"));
    let activity = &panel.catalog_families["root"].activities["child"];
    assert_eq!(activity.status, BranchStatus::Active);
    assert_eq!(activity.completed_at, None);
    assert!(!activity.terminal_locked);
}

#[test]
fn terminal_baseline_is_hidden_and_failed_live_branch_gets_longer_grace() {
    let mut panel = catalog();
    panel.apply_catalog_family_snapshot("root", snapshot(1, &[
        ("old", "completed"), ("old-failure", "failed"), ("live", "outstanding"),
    ]));
    assert!(!has(&panel, "old"));
    assert!(!has(&panel, "old-failure"));
    panel.apply_catalog_family_snapshot("root", snapshot(2, &[("live", "failed")]));
    assert_eq!(panel.sessions()[index(&panel, "live")].branch_status, Some(BranchStatus::Stopped));
    let activity = panel.catalog_families.get_mut("root").unwrap().activities.get_mut("live").unwrap();
    activity.completed_at = Some(Instant::now() - SUBAGENT_HIDE_AFTER - Duration::from_secs(1));
    panel.tick_scroll();
    assert!(has(&panel, "live"));
    panel.catalog_families.get_mut("root").unwrap().activities.get_mut("live").unwrap().completed_at =
        Some(Instant::now() - FAILED_SUBAGENT_HIDE_AFTER - Duration::from_secs(1));
    panel.tick_scroll();
    assert!(!has(&panel, "live"));
}

#[test]
fn revisions_and_execution_rollover_reject_delayed_traffic() {
    let mut panel = catalog();
    panel.apply_catalog_family_snapshot("root", snapshot(10, &[("child", "outstanding")]));
    assert!(!panel.apply_catalog_family_snapshot("root", snapshot(9, &[])));
    assert!(panel.apply_catalog_family_snapshot("root", snapshot(10, &[])));
    assert!(!has(&panel, "child"));
    let mut next = snapshot(1, &[("child", "outstanding")]);
    next.execution_id = Some("run-2".into());
    assert!(panel.apply_catalog_family_snapshot("root", next));
    assert!(!panel.apply_catalog_family_snapshot("root", snapshot(99, &[])));
    assert!(has(&panel, "child"));
    let mut baseline = snapshot(2, &[("child", "completed")]);
    baseline.execution_id = Some("run-2".into());
    panel.apply_catalog_family_snapshot("root", baseline);
    let mut rollover = snapshot(0, &[("child", "outstanding")]);
    rollover.execution_id = Some("run-3".into());
    assert!(panel.apply_catalog_family_snapshot("root", rollover));
    assert_eq!(panel.sessions()[index(&panel, "child")].branch_status, Some(BranchStatus::Active));
}

#[test]
fn reconnect_and_scope_reset_clear_rows_and_watermarks() {
    let mut panel = catalog();
    panel.apply_catalog_family_snapshot("root", snapshot(9, &[("child", "outstanding")]));
    panel.set_selected(index(&panel, "next"));
    assert!(panel.clear_catalog_families());
    assert_eq!(panel.selected_session().unwrap().id, "next");
    assert!(!has(&panel, "child"));
    assert!(!panel.clear_catalog_families());
    assert!(panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")])));
    panel.set_catalog_directory(Some("/work/b".into()));
    assert!(panel.catalog_families.is_empty());
    assert!(panel.catalog_subagents.is_empty());
    assert!(!panel.apply_catalog_family_snapshot("root", snapshot(2, &[("child", "outstanding")])));
    panel.set_catalog_directory(Some("/work/a".into()));
    assert!(panel.apply_catalog_family_snapshot("root", snapshot(0, &[("child", "outstanding")])));
    panel.invalidate_session_catalog();
    assert!(panel.catalog_families.is_empty());
}

#[test]
fn permission_changes_preserve_selected_row_and_viewport_anchor() {
    let mut panel = catalog();
    panel.apply_catalog_family_snapshot("root", snapshot(1, &[
        ("child", "outstanding"), ("sibling", "outstanding"),
    ]));
    panel.set_selected(index(&panel, "sibling"));
    panel.set_last_panel_height_rows(1);
    panel.scroll_px = index(&panel, "sibling") as f32 * panel.row_height();
    let mut update = snapshot(2, &[("sibling", "outstanding")]);
    update.branches[0].permission_pending = true;
    panel.apply_catalog_family_snapshot("root", update);
    assert_eq!(panel.selected_session().unwrap().id, "sibling");
    assert_eq!(panel.selected_session().unwrap().branch_status, Some(BranchStatus::WaitingPermission));
    assert_eq!(panel.session_viewport_anchor().unwrap().0, "sibling");
}

#[test]
fn pane_activity_cannot_overwrite_catalog_and_snapshot_directory_is_authoritative() {
    let mut panel = catalog();
    let roots = panel.all_sessions.clone();
    panel.set_catalog_directory(None);
    panel.set_sessions(roots);
    let mut feed = snapshot(1, &[("child", "outstanding")]);
    feed.directory = "/feed/directory".into();
    panel.apply_catalog_family_snapshot("root", feed);
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("different-root", "Other pane", ""),
        NeoismAgentSessionEntry::new("pane-child", "Stale child", ""),
    ]);
    panel.set_branch_activity_status("child", BranchStatus::Completed);
    panel.tick_scroll();
    assert_eq!(panel.sessions()[index(&panel, "child")].branch_status, Some(BranchStatus::Active));
    assert_eq!(panel.sessions()[index(&panel, "child")].directory.as_deref(), Some("/feed/directory"));
    assert!(!has(&panel, "pane-child"));
}

#[test]
fn equal_revision_permission_and_title_updates_are_canonical() {
    let mut panel = catalog();
    panel.apply_catalog_family_snapshot("root", snapshot(7, &[("child", "outstanding")]));
    panel.set_selected(index(&panel, "child"));

    let mut asked = snapshot(7, &[("child", "outstanding")]);
    asked.branches[0].permission_pending = true;
    asked.branches[0].title = "Renamed while waiting".into();
    assert!(panel.apply_catalog_family_snapshot("root", asked));
    let selected = panel.selected_session().unwrap();
    assert_eq!(selected.id, "child");
    assert_eq!(selected.branch_status, Some(BranchStatus::WaitingPermission));
    assert_eq!(selected.title, "Renamed while waiting");

    let mut replied = snapshot(7, &[("child", "outstanding")]);
    replied.branches[0].title = "Permission granted".into();
    assert!(panel.apply_catalog_family_snapshot("root", replied.clone()));
    assert_eq!(panel.selected_session().unwrap().branch_status, Some(BranchStatus::Active));
    assert_eq!(panel.selected_session().unwrap().title, "Permission granted");
    // Accepted duplicates are harmless; they do not move selection or lifecycle.
    assert!(panel.apply_catalog_family_snapshot("root", replied));
    assert_eq!(panel.selected_session().unwrap().id, "child");
    assert_eq!(panel.catalog_families["root"].activities["child"].completed_at, None);
}

#[test]
fn equal_revision_terminal_metadata_and_duplicates_preserve_completion_clock() {
    let mut panel = catalog();
    panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")]));
    panel.apply_catalog_family_snapshot("root", snapshot(2, &[("child", "completed")]));
    let completed_at = panel.catalog_families["root"].activities["child"].completed_at;
    assert!(completed_at.is_some());
    let mut renamed = snapshot(2, &[("child", "completed")]);
    renamed.branches[0].title = "Finished task renamed".into();
    assert!(panel.apply_catalog_family_snapshot("root", renamed.clone()));
    assert!(panel.apply_catalog_family_snapshot("root", renamed));
    assert_eq!(panel.sessions()[index(&panel, "child")].title, "Finished task renamed");
    assert_eq!(panel.catalog_families["root"].activities["child"].completed_at, completed_at);

    // A same-revision outstanding read may carry newer metadata, but cannot
    // erase terminal lifecycle or restart the completion grace.
    let mut stale = snapshot(2, &[("child", "outstanding")]);
    stale.branches[0].permission_pending = true;
    stale.branches[0].title = "Canonical title".into();
    assert!(panel.apply_catalog_family_snapshot("root", stale));
    let child = &panel.sessions()[index(&panel, "child")];
    assert_eq!(child.title, "Canonical title");
    assert_eq!(child.branch_status, Some(BranchStatus::Completed));
    assert_eq!(panel.catalog_families["root"].activities["child"].completed_at, completed_at);
    assert!(!panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")])));
    assert_eq!(panel.catalog_families["root"].activities["child"].completed_at, completed_at);

    assert!(panel.apply_catalog_family_snapshot("root", snapshot(3, &[("child", "outstanding")])));
    let activity = &panel.catalog_families["root"].activities["child"];
    assert_eq!(activity.status, BranchStatus::Active);
    assert_eq!(activity.completed_at, None);
    assert!(!activity.terminal_locked);
    assert_eq!(activity.started_at, Some(123));
    // The old terminal snapshot cannot close the newer continuation.
    assert!(!panel.apply_catalog_family_snapshot("root", snapshot(2, &[("child", "completed")])));
    assert_eq!(panel.sessions()[index(&panel, "child")].branch_status, Some(BranchStatus::Active));
}

#[test]
fn passive_highlight_does_not_move_keyboard_candidate() {
    let mut panel = catalog();
    panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")]));
    panel.set_selected(index(&panel, "next"));
    panel.set_catalog_viewed_root(Some("root"));
    panel.set_catalog_viewed_session(Some("child"));
    assert_eq!(panel.catalog_highlight_index(None), Some(index(&panel, "child")));
    assert_eq!(panel.selected_session().unwrap().id, "next");
    panel.set_focused(true);
    assert_eq!(panel.catalog_highlight_index(None), Some(index(&panel, "next")));
}

#[test]
fn feed_children_follow_pins_search_and_project_collapse() {
    let mut panel = catalog();
    let roots = panel.all_sessions.clone();
    panel.set_catalog_directory(None);
    panel.set_sessions(roots);
    panel.apply_catalog_family_snapshot("root", snapshot(1, &[("child", "outstanding")]));
    assert_eq!(panel.sessions()[index(&panel, "child")].depth, 2);
    panel.set_selected(index(&panel, "project:/work/a"));
    assert!(panel.toggle_selected_project());
    assert!(!has(&panel, "child"));
    panel.set_session_query("Parent".into());
    assert_eq!(index(&panel, "child"), index(&panel, "root") + 1);
    panel.set_session_query(String::new());
    panel.upsert_session(NeoismAgentSessionEntry::new("root", "Parent", "")
        .with_directory(Some("/work/a".into())).with_pinned(true));
    assert_eq!(index(&panel, "child"), index(&panel, "root") + 1);
    assert_eq!(panel.sessions()[index(&panel, "child")].depth, 1);
}
