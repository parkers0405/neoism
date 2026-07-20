use super::*;

fn terminal(title: &str, route: Option<usize>) -> BufferTab<()> {
    BufferTab {
        title: title.to_string(),
        modified: false,
        path: None,
        markdown: false,
        terminal_route_id: route,
        neoism_agent_route_id: None,
        chrome_page: None,
        agent_kind: None,
    }
}

fn file(path: &str) -> BufferTab<()> {
    BufferTab {
        title: path.rsplit('/').next().unwrap_or(path).to_string(),
        modified: false,
        path: Some(PathBuf::from(path)),
        markdown: false,
        terminal_route_id: None,
        neoism_agent_route_id: None,
        chrome_page: None,
        agent_kind: None,
    }
}

#[test]
fn active_close_plan_closes_terminal_route_when_not_in_editor() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(
        vec![terminal("Terminal 2", Some(44)), file("src/lib.rs")],
        0,
    );

    assert_eq!(
        tabs.active_close_plan(false, None),
        BufferTabClosePlan::CloseTerminalRoute { route_id: 44 }
    );
    assert_eq!(tabs.active(), 0);
}

#[test]
fn active_close_plan_recovers_remembered_editor_path() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(
        vec![
            terminal("Terminal 1", None),
            file("src/lib.rs"),
            file("src/main.rs"),
        ],
        0,
    );

    assert_eq!(
        tabs.active_close_plan(true, Some(Path::new("src/main.rs"))),
        BufferTabClosePlan::CloseTab { index: 2 }
    );
    assert_eq!(tabs.active(), 2);
}

#[test]
fn active_close_plan_falls_back_to_first_closeable_target() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(
        vec![
            terminal("Terminal 1", None),
            file("src/lib.rs"),
            file("src/main.rs"),
        ],
        0,
    );

    assert_eq!(
        tabs.active_close_plan(true, Some(Path::new("missing.rs"))),
        BufferTabClosePlan::CloseTab { index: 1 }
    );
    assert_eq!(tabs.active(), 1);
}

#[test]
fn active_close_plan_ignores_terminal_when_no_closeable_target_exists() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 1", None)], 0);

    assert_eq!(
        tabs.active_close_plan(true, None),
        BufferTabClosePlan::Ignore
    );
    assert_eq!(tabs.active(), 0);
}

#[test]
fn shared_policy_selects_and_moves_with_desktop_ordering() {
    assert_eq!(
        apply_buffer_tab_policy(
            BufferTabPolicyInput {
                len: 3,
                active: 0,
                closeable: Vec::new(),
            },
            BufferTabPolicyOperation::SelectPrevious,
        )
        .active,
        2
    );

    let moved = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 3,
            active: 1,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::MoveNext,
    );
    assert_eq!(moved.move_from, Some(1));
    assert_eq!(moved.move_to, Some(2));
    assert_eq!(moved.active, 2);
    assert!(moved.changed);
}

#[test]
fn shared_policy_selects_by_number_and_rejects_invalid_targets() {
    let selected = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 5,
            active: 1,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::SelectIndex { index: 3 },
    );
    assert_eq!(selected.active, 3);
    assert!(selected.changed);

    let same = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 5,
            active: 3,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::SelectIndex { index: 3 },
    );
    assert_eq!(same.active, 3);
    assert!(!same.changed);

    let invalid = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 5,
            active: 3,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::SelectIndex { index: 9 },
    );
    assert_eq!(invalid.active, 3);
    assert!(!invalid.changed);
}

#[test]
fn shared_policy_blocks_move_past_edges() {
    let at_start = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 3,
            active: 0,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::MovePrevious,
    );
    assert_eq!(at_start.move_from, None);
    assert_eq!(at_start.move_to, None);
    assert_eq!(at_start.active, 0);
    assert!(!at_start.changed);

    let at_end = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 3,
            active: 2,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::MoveNext,
    );
    assert_eq!(at_end.move_from, None);
    assert_eq!(at_end.move_to, None);
    assert_eq!(at_end.active, 2);
    assert!(!at_end.changed);
}

#[test]
fn shared_policy_closes_only_closeable_tabs_and_clamps_focus() {
    let blocked = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 1,
            active: 0,
            closeable: vec![false],
        },
        BufferTabPolicyOperation::CloseActive,
    );
    assert_eq!(blocked.remove_index, None);
    assert_eq!(blocked.active, 0);
    assert!(!blocked.changed);

    let closed = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 4,
            active: 3,
            closeable: vec![false, true, true, true],
        },
        BufferTabPolicyOperation::CloseActive,
    );
    assert_eq!(closed.remove_index, Some(3));
    assert_eq!(closed.active, 2);
    assert!(closed.changed);
}

#[test]
fn shared_policy_close_index_rebases_active_like_desktop_close_at() {
    let before_active = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 4,
            active: 3,
            closeable: vec![true, true, true, true],
        },
        BufferTabPolicyOperation::CloseIndex { index: 1 },
    );
    assert_eq!(before_active.remove_index, Some(1));
    assert_eq!(before_active.active, 2);

    let active_tab = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 4,
            active: 2,
            closeable: vec![true, true, true, true],
        },
        BufferTabPolicyOperation::CloseIndex { index: 2 },
    );
    assert_eq!(active_tab.remove_index, Some(2));
    assert_eq!(active_tab.active, 2);

    let blocked = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 2,
            active: 1,
            closeable: vec![true, false],
        },
        BufferTabPolicyOperation::CloseIndex { index: 1 },
    );
    assert_eq!(blocked.remove_index, None);
    assert_eq!(blocked.active, 1);
    assert!(!blocked.changed);
}

#[test]
fn shared_policy_reorder_rebases_active_index_for_drag_paths() {
    let moved_active = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 5,
            active: 1,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::Reorder { from: 1, to: 3 },
    );
    assert_eq!(moved_active.move_from, Some(1));
    assert_eq!(moved_active.move_to, Some(3));
    assert_eq!(moved_active.active, 3);

    let shifted_left = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 5,
            active: 3,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::Reorder { from: 1, to: 4 },
    );
    assert_eq!(shifted_left.active, 2);

    let shifted_right = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 5,
            active: 1,
            closeable: Vec::new(),
        },
        BufferTabPolicyOperation::Reorder { from: 4, to: 0 },
    );
    assert_eq!(shifted_right.active, 2);
}

// ── workspace_active_path_for_target ────────────────────────────

#[test]
fn workspace_active_path_file_inserts() {
    let target = BufferTabTarget::File(PathBuf::from("src/lib.rs"));
    assert_eq!(
        workspace_active_path_for_target(Some(&target)),
        WorkspaceActivePathUpdate::Insert(PathBuf::from("src/lib.rs"))
    );
}

#[test]
fn workspace_active_path_markdown_inserts() {
    let target = BufferTabTarget::Markdown(PathBuf::from("README.md"));
    assert_eq!(
        workspace_active_path_for_target(Some(&target)),
        WorkspaceActivePathUpdate::Insert(PathBuf::from("README.md"))
    );
}

#[test]
fn workspace_active_path_agent_removes() {
    assert_eq!(
        workspace_active_path_for_target(Some(&BufferTabTarget::NeoismAgent(7))),
        WorkspaceActivePathUpdate::Remove
    );
    assert_eq!(
        workspace_active_path_for_target(None),
        WorkspaceActivePathUpdate::Remove
    );
}

#[test]
fn workspace_active_path_after_close_keeps_when_unset() {
    let target = BufferTabTarget::File(PathBuf::from("src/lib.rs"));
    assert_eq!(
        workspace_active_path_after_close(Some(&target), false),
        WorkspaceActivePathUpdate::Keep
    );
    assert_eq!(
        workspace_active_path_after_close(None, false),
        WorkspaceActivePathUpdate::Keep
    );
}

#[test]
fn workspace_active_path_after_close_uses_target_when_present() {
    let target = BufferTabTarget::Markdown(PathBuf::from("NOTES.md"));
    assert_eq!(
        workspace_active_path_after_close(Some(&target), true),
        WorkspaceActivePathUpdate::Insert(PathBuf::from("NOTES.md"))
    );
    assert_eq!(
        workspace_active_path_after_close(None, true),
        WorkspaceActivePathUpdate::Remove
    );
}

#[test]
fn buf_enter_guard_only_on_insert() {
    assert_eq!(
        WorkspaceActivePathUpdate::Insert(PathBuf::from("a.rs")).buf_enter_guard(),
        Some(PathBuf::from("a.rs"))
    );
    assert_eq!(WorkspaceActivePathUpdate::Remove.buf_enter_guard(), None);
    assert_eq!(WorkspaceActivePathUpdate::Keep.buf_enter_guard(), None);
}

// ── buffer_tab_target_label ─────────────────────────────────────

#[test]
fn target_label_covers_all_variants() {
    assert_eq!(
        buffer_tab_target_label(Some(&BufferTabTarget::File(PathBuf::from(
            "src/lib.rs"
        )))),
        "src/lib.rs"
    );
    assert_eq!(
        buffer_tab_target_label(Some(&BufferTabTarget::Markdown(PathBuf::from(
            "NOTES.md"
        )))),
        "markdown:NOTES.md"
    );
    assert_eq!(
        buffer_tab_target_label(Some(&BufferTabTarget::NeoismAgent(7))),
        "neoism-agent:7"
    );
    assert_eq!(buffer_tab_target_label(None), "<none>");
}

// ── classify_strip_click ────────────────────────────────────────

fn workspace_geom() -> WorkspaceStripGeometry {
    WorkspaceStripGeometry {
        x_left: 100.0,
        y_top: 40.0,
        width: 800.0,
        height: 28.0,
    }
}

#[test]
fn classify_strip_click_pane_hit_wins_over_workspace() {
    let outcome = classify_strip_click(
        Some((9, TabHit::Activate(3))),
        Some(workspace_geom()),
        Some(TabHit::Close(0)),
        500.0,
        48.0,
    );
    assert_eq!(
        outcome,
        StripClickOutcome::PaneActivate {
            strip: StripKey::Pane(9),
            index: 3,
        }
    );
}

#[test]
fn classify_strip_click_pane_close_routes_through_pane_strip() {
    let outcome = classify_strip_click(Some((4, TabHit::Close(2))), None, None, 0.0, 0.0);
    assert_eq!(
        outcome,
        StripClickOutcome::PaneClose {
            strip: StripKey::Pane(4),
            index: 2,
        }
    );
}

#[test]
fn classify_strip_click_workspace_activate_and_close() {
    let outcome = classify_strip_click(
        None,
        Some(workspace_geom()),
        Some(TabHit::Activate(1)),
        150.0,
        48.0,
    );
    assert_eq!(outcome, StripClickOutcome::WorkspaceActivate { index: 1 });

    let outcome = classify_strip_click(
        None,
        Some(workspace_geom()),
        Some(TabHit::Close(2)),
        500.0,
        48.0,
    );
    assert_eq!(outcome, StripClickOutcome::WorkspaceClose { index: 2 });
}

#[test]
fn classify_strip_click_absorbs_misses_inside_workspace_strip() {
    let outcome = classify_strip_click(None, Some(workspace_geom()), None, 500.0, 48.0);
    assert_eq!(outcome, StripClickOutcome::WorkspaceAbsorb);
}

#[test]
fn classify_strip_click_passes_through_outside_strip_rect() {
    let outcome = classify_strip_click(None, Some(workspace_geom()), None, 10.0, 10.0);
    assert_eq!(outcome, StripClickOutcome::Pass);
}

#[test]
fn classify_strip_click_passes_through_when_workspace_hidden() {
    let outcome = classify_strip_click(None, None, None, 200.0, 50.0);
    assert_eq!(outcome, StripClickOutcome::Pass);
}

// ── reinsert_tab_plan ───────────────────────────────────────────

#[test]
fn reinsert_tab_plan_workspace_path() {
    assert_eq!(
        reinsert_tab_plan(StripKey::Workspace, false),
        ReinsertTabPlan {
            strip: StripKey::Workspace,
            kind: ReinsertTabKind::Path,
        }
    );
}

#[test]
fn reinsert_tab_plan_pane_markdown() {
    assert_eq!(
        reinsert_tab_plan(StripKey::Pane(9), true),
        ReinsertTabPlan {
            strip: StripKey::Pane(9),
            kind: ReinsertTabKind::Markdown,
        }
    );
}

// ── tear_out_source_cleanup ─────────────────────────────────────

#[test]
fn tear_out_workspace_source_never_drops_strip() {
    // Workspace strip owns the workspace terminal too — never
    // drop it, even if all editor tabs are gone.
    assert_eq!(
        tear_out_source_cleanup(StripKey::Workspace, 0),
        TearOutSourceCleanup {
            drop_source_pane_tabs: false
        }
    );
    assert_eq!(
        tear_out_source_cleanup(StripKey::Workspace, 3),
        TearOutSourceCleanup {
            drop_source_pane_tabs: false
        }
    );
}

#[test]
fn tear_out_pane_source_drops_when_empty() {
    assert_eq!(
        tear_out_source_cleanup(StripKey::Pane(4), 0),
        TearOutSourceCleanup {
            drop_source_pane_tabs: true
        }
    );
}

#[test]
fn tear_out_pane_source_keeps_strip_when_non_empty() {
    assert_eq!(
        tear_out_source_cleanup(StripKey::Pane(4), 2),
        TearOutSourceCleanup {
            drop_source_pane_tabs: false
        }
    );
}

// ── drop_preview_geometry ───────────────────────────────────────

#[test]
fn drop_preview_geometry_basic_slot_math() {
    // 5 tabs of width 100, strip starts at x=0 width 500.
    // Pointer at x=240 → local_x=240 → 240/100=2.4 → round to 2.
    let geom = drop_preview_geometry(0.0, 500.0, 240.0, 0.0, 5, 100.0);
    assert_eq!(geom.insert_index, 2);
    assert_eq!(geom.tab_width, 100.0);
    assert!((geom.caret_x - 200.0).abs() < 0.01);
}

#[test]
fn drop_preview_geometry_scroll_offsets_caret() {
    // Strip scrolled right by 50. Mouse at x=190 → local_x=240
    // → insert at 2 → caret = 200 - 50 = 150.
    let geom = drop_preview_geometry(0.0, 500.0, 190.0, 50.0, 5, 100.0);
    assert_eq!(geom.insert_index, 2);
    assert!((geom.caret_x - 150.0).abs() < 0.01);
}

#[test]
fn drop_preview_geometry_clamps_past_last_tab() {
    // Pointer way past the strip — clamp insert index to count.
    // caret_x lands at the right edge of the last tab slot
    // (x_left + count * tab_width = 0 + 3*100 = 300), which is
    // already inside the strip rect so the rect-clamp is a no-op.
    let geom = drop_preview_geometry(0.0, 500.0, 9999.0, 0.0, 3, 100.0);
    assert_eq!(geom.insert_index, 3);
    assert!((geom.caret_x - 300.0).abs() < 0.01);
}

#[test]
fn drop_preview_geometry_clamps_caret_to_left_edge_when_mouse_before() {
    // Pointer to the left of the strip — local_x clamps to 0,
    // caret pins to strip left.
    let geom = drop_preview_geometry(100.0, 500.0, 20.0, 0.0, 5, 100.0);
    assert_eq!(geom.insert_index, 0);
    assert!((geom.caret_x - 100.0).abs() < 0.01);
}

#[test]
fn drop_preview_geometry_zero_tab_count_treats_as_single_slot() {
    // Empty strip — single drop slot.
    let geom = drop_preview_geometry(0.0, 400.0, 50.0, 0.0, 0, 100.0);
    assert!(geom.insert_index <= 1);
    assert_eq!(geom.tab_width, 100.0);
}

#[test]
fn drop_preview_geometry_handles_zero_tab_width_safely() {
    // Defensive — tab_width=0 should not divide-by-zero.
    let geom = drop_preview_geometry(0.0, 400.0, 200.0, 0.0, 3, 0.0);
    assert!(geom.tab_width > 0.0);
    assert!(geom.caret_x.is_finite());
}

// ── drop_preview_update ─────────────────────────────────────────

#[test]
fn drop_preview_update_emits_when_dest_differs_from_source() {
    let upd = drop_preview_update(StripKey::Workspace, Some(StripKey::Pane(3)), 123.0);
    assert_eq!(
        upd,
        Some(DropPreviewUpdate {
            target: StripKey::Pane(3),
            mouse_x: 123.0,
        })
    );
}

#[test]
fn drop_preview_update_clears_when_dest_matches_source() {
    // Same-strip drops never paint a cross-strip preview.
    assert_eq!(
        drop_preview_update(StripKey::Pane(7), Some(StripKey::Pane(7)), 88.0),
        None
    );
}

#[test]
fn drop_preview_update_clears_when_dest_missing() {
    assert_eq!(drop_preview_update(StripKey::Workspace, None, 88.0), None);
}

// ── tab_drag_release_kind ───────────────────────────────────────

#[test]
fn tab_drag_release_kind_markdown_wins_over_file_when_marker_set() {
    assert_eq!(
        tab_drag_release_kind(true, true, false),
        TabDragReleaseKind::Markdown
    );
}

#[test]
fn tab_drag_release_kind_plain_path_routes_to_file() {
    assert_eq!(
        tab_drag_release_kind(true, false, false),
        TabDragReleaseKind::File
    );
}

#[test]
fn tab_drag_release_kind_agent_when_no_path_but_agent_kind_set() {
    assert_eq!(
        tab_drag_release_kind(false, false, true),
        TabDragReleaseKind::Agent
    );
}

#[test]
fn tab_drag_release_kind_drop_when_neither_path_nor_agent() {
    // Terminal tabs that lost their handle land here.
    assert_eq!(
        tab_drag_release_kind(false, false, false),
        TabDragReleaseKind::Drop
    );
}

#[test]
fn tab_drag_release_kind_path_wins_over_agent_when_both_set() {
    // Defensive — if a degenerate tab carries both a path and an
    // agent_kind, the file/markdown branch wins (matches legacy
    // ordering in `handle_buffer_tabs_drag_release`).
    assert_eq!(
        tab_drag_release_kind(true, false, true),
        TabDragReleaseKind::File
    );
}

// ── new_pane_strip_init ─────────────────────────────────────────

#[test]
fn new_pane_strip_init_markdown_picks_markdown_kind() {
    let init = new_pane_strip_init(1.5, true);
    assert_eq!(init.scale, 1.5);
    assert_eq!(init.kind, ReinsertTabKind::Markdown);
}

#[test]
fn new_pane_strip_init_plain_path_picks_path_kind() {
    let init = new_pane_strip_init(1.0, false);
    assert_eq!(init.scale, 1.0);
    assert_eq!(init.kind, ReinsertTabKind::Path);
}

// ── trailing "+" new-tab button ─────────────────────────────────

#[test]
fn focus_cursor_range_includes_plus_slot() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 1", None), file("src/lib.rs")], 0);
    tabs.visible = true;
    tabs.set_focused(true);

    // Cursor starts on the active tab (index 0), not the "+".
    assert_eq!(tabs.focused_index(), 0);
    assert!(!tabs.focused_on_new_tab());

    // Right onto tab 1, then right again lands on the "+" slot
    // (index == tabs.len()).
    assert!(tabs.move_focused(false));
    assert_eq!(tabs.focused_index(), 1);
    assert!(!tabs.focused_on_new_tab());

    assert!(tabs.move_focused(false));
    assert_eq!(tabs.focused_index(), tabs.tabs().len());
    assert!(tabs.focused_on_new_tab());

    // Left from the "+" returns to the last real tab.
    assert!(tabs.move_focused(true));
    assert_eq!(tabs.focused_index(), 1);
    assert!(!tabs.focused_on_new_tab());
}

#[test]
fn focus_cursor_reaches_plus_with_single_tab() {
    // Even a single-tab strip can move the cursor onto the "+":
    // there are `len + 1` slots, so `move_focused` is not a no-op.
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 1", None)], 0);
    tabs.visible = true;
    tabs.set_focused(true);

    assert_eq!(tabs.focused_index(), 0);
    assert!(tabs.move_focused(false));
    assert!(tabs.focused_on_new_tab());
    assert_eq!(tabs.focused_index(), 1);
}

#[test]
fn focused_on_new_tab_requires_focus() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 1", None)], 0);
    tabs.visible = true;
    // Park the cursor on the "+" index but leave the strip unfocused.
    tabs.focused_index = tabs.tabs().len();
    assert!(!tabs.focused_on_new_tab());
}

#[test]
fn hit_test_reports_new_tab_inside_plus_rect() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 1", None)], 0);
    tabs.visible = true;
    // Simulate what `render_with_icons` records: a "+" rect to the
    // right of the last tab at [x, y, w, h] in window coords.
    tabs.new_tab_rect = Some([300.0, 40.0, 30.0, 28.0]);

    // A point inside the rect resolves to the new-tab hit.
    assert_eq!(
        tabs.hit_test(310.0, 50.0, 0.0, 40.0, 800.0),
        Some(TabHit::NewTab)
    );
    // A point just left of the rect is not a new-tab hit.
    assert_ne!(
        tabs.hit_test(290.0, 50.0, 0.0, 40.0, 800.0),
        Some(TabHit::NewTab)
    );
}

#[test]
fn hit_test_skips_new_tab_when_strip_hidden() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 1", None)], 0);
    tabs.visible = false;
    tabs.new_tab_rect = Some([300.0, 40.0, 30.0, 28.0]);
    assert_eq!(tabs.hit_test(310.0, 50.0, 0.0, 40.0, 800.0), None);
}

#[test]
fn set_hover_accepts_new_tab() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 1", None)], 0);
    tabs.visible = true;
    assert!(tabs.set_hover(Some(TabHit::NewTab)));
    assert_eq!(tabs.hover, Some(TabHit::NewTab));
}

#[test]
fn classify_strip_click_absorbs_new_tab_hit() {
    // The host handles the "+" before calling the policy; the policy
    // absorbs defensively so the click never leaks to the pane.
    let outcome = classify_strip_click(
        None,
        Some(workspace_geom()),
        Some(TabHit::NewTab),
        500.0,
        48.0,
    );
    assert_eq!(outcome, StripClickOutcome::WorkspaceAbsorb);
}
