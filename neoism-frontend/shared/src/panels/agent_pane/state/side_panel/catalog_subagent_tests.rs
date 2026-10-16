use super::*;

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

fn family() -> NeoismAgentSidePanel {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("root", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "Research", "explore")
            .with_depth(1)
            .with_runtime_status(Some("running".into())),
        NeoismAgentSessionEntry::new("sibling", "Implementation", "build")
            .with_depth(1)
            .with_runtime_status(Some("running".into())),
    ]);
    panel
}

fn index(panel: &NeoismAgentSidePanel, id: &str) -> usize {
    panel
        .sessions()
        .iter()
        .position(|row| row.id == id)
        .unwrap()
}

#[test]
fn catalog_children_are_real_keyboard_stops_in_parent_order() {
    let mut panel = catalog();
    panel.sync_catalog_subagents_from(&family(), Some("root"));
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
}

#[test]
fn catalog_pointer_and_keyboard_resolve_identical_child() {
    let mut panel = catalog();
    panel.sync_catalog_subagents_from(&family(), Some("root"));
    panel.set_last_panel_height_rows(12);
    let height = panel.row_height();
    let rect = [0., 100., 240., height * 12.];
    panel.set_last_panel_rect(rect);
    panel.set_row_hit_rect(rect, height);
    let child = index(&panel, "child");
    assert_eq!(
        panel.hit_test_row(80., 100. + (child as f32 + 0.5) * height, rect),
        Some(child)
    );
    assert!(panel.catalog_clickable_at(80., 100. + (child as f32 + 0.5) * height));
    panel.set_selected(child);
    assert_eq!(panel.selected_session().unwrap().id, "child");
}

#[test]
fn catalog_refresh_preserves_child_identity_when_siblings_change() {
    let mut panel = catalog();
    let mut source = family();
    panel.sync_catalog_subagents_from(&source, Some("root"));
    panel.set_selected(index(&panel, "sibling"));
    source.subagents.remove(1);
    panel.sync_catalog_subagents_from(&source, Some("root"));
    assert_eq!(panel.selected_session().unwrap().id, "sibling");
    source.set_branch_activity_status("sibling", BranchStatus::WaitingPermission);
    panel.sync_catalog_subagents_from(&source, Some("root"));
    assert_eq!(
        panel.selected_session().unwrap().branch_status,
        Some(BranchStatus::WaitingPermission)
    );
}

#[test]
fn passive_catalog_highlight_follows_exact_view_without_moving_keyboard_candidate() {
    let mut panel = catalog();
    panel.sync_catalog_subagents_from(&family(), Some("root"));
    panel.set_selected(index(&panel, "next"));
    panel.set_catalog_viewed_root(Some("root"));
    panel.set_catalog_viewed_session(Some("child"));
    assert_eq!(
        panel.catalog_highlight_index(None),
        Some(index(&panel, "child"))
    );
    assert_eq!(panel.selected_session().unwrap().id, "next");
    panel.set_focused(true);
    assert_eq!(
        panel.catalog_highlight_index(None),
        Some(index(&panel, "next"))
    );
    panel.set_focused(false);
    panel.set_catalog_viewed_session(None);
    assert_eq!(panel.catalog_highlight_index(None), None);
}

#[test]
fn projected_children_follow_pins_search_and_project_collapse() {
    let mut panel = catalog();
    panel.sync_catalog_subagents_from(&family(), Some("root"));
    let roots = panel.all_sessions.clone();
    panel.set_catalog_directory(None);
    panel.set_sessions(roots);
    panel.sync_catalog_subagents_from(&family(), Some("root"));
    assert_eq!(panel.sessions()[index(&panel, "child")].depth, 2);
    panel.set_selected(index(&panel, "project:/work/a"));
    assert!(panel.toggle_selected_project());
    assert!(!panel.sessions().iter().any(|row| row.id == "child"));
    panel.set_session_query("Parent".into());
    assert_eq!(index(&panel, "child"), index(&panel, "root") + 1);
    panel.set_session_query(String::new());
    let root = NeoismAgentSessionEntry::new("root", "Parent", "")
        .with_directory(Some("/work/a".into()))
        .with_updated_ms(20)
        .with_pinned(true);
    panel.upsert_session(root);
    assert_eq!(index(&panel, "child"), index(&panel, "root") + 1);
    assert_eq!(panel.sessions()[index(&panel, "child")].depth, 1);
    assert_eq!(
        panel
            .sessions()
            .iter()
            .filter(|row| row.id == "child")
            .count(),
        1
    );
}

#[test]
fn lifecycle_pruning_does_not_repoint_the_catalog_cursor() {
    let mut panel = catalog();
    let source = family();
    panel.set_subagents(source.subagents.clone());
    panel.sync_catalog_subagents(Some("root"));
    panel.set_selected(index(&panel, "next"));
    panel.set_branch_activity_status_from_recovery("child", BranchStatus::Completed);
    assert!(panel.prune_expired_completed_subagents());
    assert_eq!(panel.selected_session().unwrap().id, "next");
    panel.sync_catalog_subagents(Some("root"));
    assert_eq!(panel.selected_session().unwrap().id, "next");
    assert!(!panel.sessions().iter().any(|row| row.id == "child"));
}

#[test]
fn empty_or_unrelated_recovery_does_not_erase_a_known_family() {
    let mut panel = catalog();
    panel.sync_catalog_subagents_from(&family(), Some("root"));
    panel.sync_catalog_subagents_from(&NeoismAgentSidePanel::default(), Some("root"));
    assert!(panel.sessions().iter().any(|row| row.id == "child"));
    let mut source = family();
    source.subagents.truncate(1);
    panel.sync_catalog_subagents_from(&source, Some("root"));
    assert!(!panel.sessions().iter().any(|row| row.id == "child"));
}

#[test]
fn catalog_projection_preserves_tool_and_viewed_historical_child() {
    let mut panel = catalog();
    let mut source = family();
    source.set_branch_activity_tool(
        "child",
        BranchStatus::Active,
        Some("Reading files".into()),
        None,
    );
    panel.sync_catalog_subagents_from(&source, Some("root"));
    assert_eq!(
        panel.sessions()[index(&panel, "child")]
            .branch_activity_label
            .as_deref(),
        Some("Reading files")
    );
    source.set_viewed_session_id(Some("child".into()));
    source.set_branch_activity_status_from_recovery("child", BranchStatus::Completed);
    panel.sync_catalog_subagents_from(&source, Some("root"));
    assert_eq!(
        panel.sessions()[index(&panel, "child")].branch_status,
        Some(BranchStatus::Completed)
    );
    source.set_viewed_session_id(Some("root".into()));
    panel.sync_catalog_subagents_from(&source, Some("root"));
    assert!(!panel.sessions().iter().any(|row| row.id == "child"));
}

#[test]
fn closed_families_drop_stale_running_rows_without_losing_other_families() {
    let mut panel = catalog();
    panel.upsert_session(
        NeoismAgentSessionEntry::new("other", "Other", "")
            .with_directory(Some("/work/a".into())),
    );
    panel.sync_catalog_subagents_from(&family(), Some("root"));
    let mut other = family();
    other.subagents[0].id = "other".into();
    other.subagents[1].id = "other-child".into();
    other.subagents.truncate(2);
    panel.sync_catalog_subagents_from(&other, Some("other"));
    panel.set_selected(index(&panel, "other-child"));
    let roots = HashSet::from(["other".to_string()]);
    assert!(panel.retain_catalog_subagent_families(&roots));
    assert!(!panel.sessions().iter().any(|row| row.id == "child"));
    assert_eq!(panel.selected_session().unwrap().id, "other-child");
    assert!(!panel.retain_catalog_subagent_families(&roots));
    assert!(panel.retain_catalog_subagent_families(&HashSet::new()));
    assert!(!panel
        .sessions()
        .iter()
        .any(|row| row.branch_root_id.is_some()));
}

#[test]
fn old_detail_rail_no_longer_takes_branch_keyboard_focus() {
    let mut source = family();
    source.set_mode(SidePanelMode::Subagents);
    assert!(!source.focusable());
}
