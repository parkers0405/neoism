use super::impl_core::INTRINSIC_TAB_MAX_WIDTH;
use super::impl_render::first_glyph_ink_center_y;
use super::*;

fn text_instance(
    pos_y: f32,
    bearing_y: i16,
    height: u32,
) -> sugarloaf::text::TextInstance {
    sugarloaf::text::TextInstance {
        pos: [0.0, pos_y],
        glyph_size: [8, height],
        bearings: [0, bearing_y],
        ..Default::default()
    }
}

#[test]
fn agent_logo_alignment_uses_leading_glyph_ink_center() {
    let instances = [
        text_instance(8.0, 3, 10),
        // A later descender must not pull the logo down.
        text_instance(8.0, 3, 15),
    ];

    assert_eq!(first_glyph_ink_center_y(&instances, 0, 1.0), Some(16.0));
}

#[test]
fn agent_logo_alignment_converts_physical_ink_to_logical_pixels() {
    let instances = [text_instance(16.0, 6, 20)];

    assert_eq!(first_glyph_ink_center_y(&instances, 0, 2.0), Some(16.0));
}

#[test]
fn agent_logo_alignment_ignores_instances_before_the_title() {
    let instances = [text_instance(100.0, 0, 10), text_instance(12.0, 2, 8)];

    assert_eq!(first_glyph_ink_center_y(&instances, 1, 1.0), Some(18.0));
    assert_eq!(first_glyph_ink_center_y(&instances, 2, 1.0), None);
    assert_eq!(first_glyph_ink_center_y(&instances, 1, 0.0), None);
}

fn terminal(title: &str, route: Option<usize>) -> BufferTab<()> {
    BufferTab {
        title: title.to_string(),
        modified: false,
        custom_icon: None,
        path: None,
        markdown: false,
        terminal_route_id: route,
        neoism_agent_route_id: None,
        chrome_page: None,
        agent_kind: None,
    }
}

#[test]
fn touch_scroll_is_direct_bounded_and_has_no_glide_target() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(
        (0..8)
            .map(|index| terminal(&format!("Terminal {index}"), Some(index)))
            .collect(),
        0,
    );
    let viewport = 300.0;
    assert!(tabs.scroll_touch_by(-37.0, viewport));
    assert_eq!(tabs.scroll_x, 37.0);
    assert_eq!(tabs.scroll_target_x, tabs.scroll_x);

    tabs.scroll_touch_by(-10_000.0, viewport);
    let max = tabs.geometry_widths().iter().sum::<f32>()
        + NEW_TAB_BTN_WIDTH * tabs.scale()
        - viewport;
    assert_eq!(tabs.scroll_x, max);
    assert_eq!(tabs.scroll_target_x, max);
    assert!(!tabs.is_animating());

    tabs.scroll_touch_by(10_000.0, viewport);
    assert_eq!(tabs.scroll_x, 0.0);
    assert_eq!(tabs.scroll_target_x, 0.0);
}

fn file(path: &str) -> BufferTab<()> {
    BufferTab {
        title: path.rsplit('/').next().unwrap_or(path).to_string(),
        modified: false,
        custom_icon: None,
        path: Some(PathBuf::from(path)),
        markdown: false,
        terminal_route_id: None,
        neoism_agent_route_id: None,
        chrome_page: None,
        agent_kind: None,
    }
}

#[test]
fn neoism_agent_tabs_use_short_product_title() {
    let mut tabs = BufferTabs::<()>::new();

    let first = tabs.open_neoism_agent(41);
    let second = tabs.open_neoism_agent(42);

    assert_eq!(tabs.tabs()[first].title, "Neoism");
    assert_eq!(tabs.tabs()[second].title, "Neoism 2");
}

#[test]
fn long_tab_title_is_ellipsized_within_its_pixel_budget() {
    let fitted = BufferTabs::<()>::fit_title(
        "A very long Monocraft EPUB filename that must stay inside its tab.epub",
        12.0,
        |_| 1.0,
    );

    assert!(fitted.ends_with(TITLE_ELLIPSIS));
    assert!(fitted.chars().count() <= 12);
}

#[test]
fn short_tab_title_is_not_needlessly_changed() {
    assert_eq!(
        BufferTabs::<()>::fit_title("Bible.epub", 40.0, |_| 1.0),
        "Bible.epub"
    );
}

#[test]
fn short_tab_width_tracks_content_and_close_reservation() {
    let without_close = BufferTabs::<()>::visual_tab_width(40.0, false, 1.0);
    let with_close = BufferTabs::<()>::visual_tab_width(40.0, true, 1.0);

    assert_eq!(without_close, 87.0);
    assert_eq!(with_close, 98.5);
    assert_eq!(
        with_close - without_close,
        CLOSE_BTN_SIZE * 0.5 + CLOSE_BTN_GAP
    );
    assert!(with_close < MAX_TAB_WIDTH);
}

#[test]
fn tiny_label_keeps_a_practical_visual_and_interaction_target() {
    let width = BufferTabs::<()>::visual_tab_width(1.0, false, 1.0);
    assert_eq!(width, MIN_TAB_WIDTH);
    assert!(width >= 44.0);
}

#[test]
fn one_long_tab_keeps_its_full_natural_width_in_a_wide_strip() {
    let natural = BufferTabs::<()>::visual_tab_geometry(420.0, 12.0, true, 1.0);

    assert!(natural.tab_width > MAX_TAB_WIDTH);
    assert_eq!(natural.title_clip_width, 420.0);
}

#[test]
fn content_sized_tab_has_only_the_declared_trailing_guard() {
    let title_width = 40.0;
    let width = BufferTabs::<()>::visual_tab_width(title_width, false, 1.0);
    let occupied = TAB_PADDING_X * 2.0 + ICON_FONT_SIZE + ICON_GAP + title_width;

    assert_eq!(width - occupied, TITLE_OVERHANG_GUARD);
}

#[test]
fn render_geometry_uses_measured_icon_and_reserves_close_exactly_once() {
    let title_width = 137.0;
    let icon_width = 16.0;
    let without_close =
        BufferTabs::<()>::visual_tab_geometry(title_width, icon_width, false, 1.0);
    let with_close =
        BufferTabs::<()>::visual_tab_geometry(title_width, icon_width, true, 1.0);

    assert_eq!(without_close.title_clip_width, title_width);
    assert_eq!(with_close.title_clip_width, title_width);
    assert_eq!(with_close.icon_width, icon_width);
    assert_eq!(
        with_close.close_reserved,
        CLOSE_BTN_SIZE * 0.5 + CLOSE_BTN_GAP
    );
    assert_eq!(
        with_close.tab_width - without_close.tab_width,
        with_close.close_reserved
    );
    assert_eq!(
        with_close.close_center_x,
        Some(with_close.tab_width - TAB_PADDING_X)
    );
}

#[test]
fn appending_tabs_past_overflow_never_resizes_existing_tabs() {
    let viewport = 430.0;
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(
        vec![
            file("one.rs"),
            file("medium-length-name.rs"),
            file("a-much-longer-descriptive-buffer-name.rs"),
        ],
        0,
    );
    let before = tabs.geometry_widths();
    let before_extent = before.iter().sum::<f32>() + NEW_TAB_BTN_WIDTH - viewport;

    let mut expanded = tabs.tabs().to_vec();
    expanded.push(file("another-buffer-after-overflow.rs"));
    expanded.push(file("and-one-more-buffer.rs"));
    tabs.set_tabs(expanded, 4);
    let after = tabs.geometry_widths();
    let after_extent = after.iter().sum::<f32>() + NEW_TAB_BTN_WIDTH - viewport;

    assert_eq!(&after[..before.len()], &before);
    assert!(before_extent > 0.0);
    assert!(after_extent > before_extent);
}

#[test]
fn removing_tabs_does_not_resize_remaining_tabs() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(
        vec![
            file("one.rs"),
            file("medium-length-name.rs"),
            file("a-much-longer-descriptive-buffer-name.rs"),
            file("four.rs"),
        ],
        0,
    );
    let before = tabs.geometry_widths();
    let remaining = vec![
        tabs.tabs()[0].clone(),
        tabs.tabs()[2].clone(),
        tabs.tabs()[3].clone(),
    ];
    tabs.set_tabs(remaining, 0);
    let after = tabs.geometry_widths();

    assert_eq!(after, vec![before[0], before[2], before[3]]);
}

#[test]
fn active_last_intrinsic_tab_can_be_fully_revealed() {
    let viewport = 300.0;
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(
        vec![
            file("one.rs"),
            file("two.rs"),
            file("three.rs"),
            file("four.rs"),
        ],
        3,
    );
    let widths = tabs.geometry_widths();

    tabs.ensure_index_visible_with_widths(3, viewport, &widths);

    let last_right = widths.iter().sum::<f32>();
    assert_eq!(tabs.scroll_target_x, last_right - viewport);
    assert!(last_right <= tabs.scroll_target_x + viewport);
}

#[test]
fn pathological_title_uses_stable_intrinsic_cap_and_close_spacing() {
    let geometry = BufferTabs::<()>::visual_tab_geometry(5_000.0, 12.0, true, 1.0);

    assert_eq!(geometry.tab_width, INTRINSIC_TAB_MAX_WIDTH);
    assert_eq!(
        geometry.close_center_x,
        Some(INTRINSIC_TAB_MAX_WIDTH - TAB_PADDING_X)
    );
    assert_eq!(
        geometry.close_reserved,
        CLOSE_BTN_SIZE * 0.5 + CLOSE_BTN_GAP
    );
    assert!(geometry.title_clip_width < 5_000.0);
}

#[test]
fn unicode_title_measurement_uses_supplied_advances_and_grapheme_boundaries() {
    let title = "文e\u{301}件名.rs";
    let measured = title
        .chars()
        .map(|character| if character.is_ascii() { 1.0 } else { 2.0 })
        .sum::<f32>();
    let natural = BufferTabs::<()>::visual_tab_geometry(measured, 12.0, true, 1.0);
    // The 72px interaction minimum may leave extra title room, but Unicode
    // demand must be based on the supplied glyph advances rather than bytes.
    assert!(natural.title_clip_width >= measured);

    let fitted = BufferTabs::<()>::fit_title(title, 5.0, |character| {
        if character.is_ascii() {
            1.0
        } else {
            2.0
        }
    });
    assert_eq!(fitted, "文…");
    assert!(!fitted.starts_with('\u{301}'));
}

#[test]
fn hover_surface_is_a_stable_editor_tab_not_a_scaled_pill() {
    let inactive = tab_surface_geometry(
        30.0,
        8.0,
        180.0,
        BUFFER_TABS_HEIGHT,
        1.0,
        TabSurfaceState::Inactive,
    );
    let hovered = tab_surface_geometry(
        30.0,
        8.0,
        180.0,
        BUFFER_TABS_HEIGHT,
        1.0,
        TabSurfaceState::Hovered,
    );
    let active = tab_surface_geometry(
        30.0,
        8.0,
        180.0,
        BUFFER_TABS_HEIGHT,
        1.0,
        TabSurfaceState::Active,
    );

    assert_eq!(
        (hovered.x, hovered.y, hovered.width, hovered.height),
        (30.0, 8.0, 180.0, 28.0)
    );
    assert_eq!(
        (active.x, active.y, active.width, active.height),
        (30.0, 8.0, 180.0, 28.0)
    );
    assert_eq!(inactive.top_radius, 0.0);
    assert_eq!(hovered.top_radius, 3.0);
    assert_eq!(active.top_radius, 3.0);
}

#[test]
fn ellipsis_is_not_emitted_when_it_cannot_fit_the_clip() {
    assert_eq!(BufferTabs::<()>::fit_title("long", 0.5, |_| 1.0), "");
}

#[test]
fn scaled_render_geometry_does_not_scale_measured_advances_twice() {
    let geometry = BufferTabs::<()>::visual_tab_geometry(274.0, 32.0, true, 2.0);

    assert_eq!(geometry.tab_width, 399.0);
    assert_eq!(geometry.title_clip_width, 274.0);
    assert_eq!(geometry.close_reserved, 23.0);
    assert_eq!(geometry.close_center_x, Some(375.0));
}

#[test]
fn focused_tab_reveal_scrolls_variable_slots_instead_of_shrinking_them() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(
        vec![file("first-long-file.rs"), file("second-long-file.rs")],
        1,
    );
    let widths = [190.0, 210.0];

    tabs.ensure_index_visible_with_widths(1, 240.0, &widths);

    assert_eq!(tabs.scroll_target_x, 160.0);
    assert!(tabs.pending_ensure_active);
}

#[test]
fn panel_hit_viewport_is_not_the_overflowing_content_width() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.strip_viewport_width = 240.0;
    tabs.layout = vec![(0.0, 190.0), (190.0, 210.0)];

    assert_eq!(tabs.last_strip_width(), 240.0);
}

#[test]
fn variable_width_drop_preview_uses_measured_edges() {
    let widths = [80.0, 140.0, 100.0];
    let before_second_midpoint =
        drop_preview_geometry_for_widths(10.0, 400.0, 10.0 + 80.0 + 69.0, 0.0, &widths);
    let after_second_midpoint =
        drop_preview_geometry_for_widths(10.0, 400.0, 10.0 + 80.0 + 71.0, 0.0, &widths);

    assert_eq!(before_second_midpoint.insert_index, 1);
    assert_eq!(before_second_midpoint.caret_x, 90.0);
    assert_eq!(after_second_midpoint.insert_index, 2);
    assert_eq!(after_second_midpoint.caret_x, 230.0);
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
fn note_custom_icon_follows_its_buffer_tab_and_rename() {
    let mut tabs = BufferTabs::<()>::new();
    let old = PathBuf::from("/vault/TASKS.md");
    let new = PathBuf::from("/vault/Tasks.md");
    tabs.open_markdown(old.clone());
    tabs.set_path_icon(&old, Some("\u{1f4a1}".to_string()));
    assert_eq!(tabs.tabs[0].custom_icon.as_deref(), Some("\u{1f4a1}"));

    tabs.rename_path(&old, new.clone());
    assert_eq!(tabs.tabs[0].path.as_deref(), Some(new.as_path()));
    assert_eq!(tabs.tabs[0].custom_icon.as_deref(), Some("\u{1f4a1}"));
}

#[test]
fn recreated_split_tab_restores_custom_presentation() {
    let path = PathBuf::from("/vault/Tasks.md");
    let mut source = BufferTabs::<()>::new();
    let source_ix = source.open_markdown(path.clone());
    source.set_title(source_ix, "My tasks");
    source.set_modified(&path, true);
    source.set_path_icon(&path, Some("\u{1f4a1}".to_string()));
    let moved = source.tabs()[source_ix].clone();

    let mut destination = BufferTabs::<()>::new();
    let dest_ix = destination.open_markdown(path);
    assert!(destination.restore_presentation_from(dest_ix, &moved));
    let restored = &destination.tabs()[dest_ix];
    assert_eq!(restored.title, "My tasks");
    assert!(restored.modified);
    assert_eq!(restored.custom_icon.as_deref(), Some("\u{1f4a1}"));
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

#[test]
fn ordinary_terminal_tab_can_drag_to_a_pane_destination() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 7", Some(7))], 0);
    tabs.set_visible(true);
    tabs.begin_drag(0, 40.0, 10.0, 0.0, 400.0);
    assert!(tabs.update_drag(80.0, 100.0, 0.0, 0.0, 400.0));
    match tabs.end_drag(true) {
        DragRelease::MoveOut { tab } => assert_eq!(tab.terminal_route_id, Some(7)),
        _ => panic!("a routed terminal tab must be movable"),
    }
}

#[test]
fn cancelling_buffer_tab_drag_never_commits_a_move() {
    let mut tabs = BufferTabs::<()>::new();
    tabs.set_tabs(vec![terminal("Terminal 7", Some(7))], 0);
    tabs.set_visible(true);

    tabs.begin_drag(0, 40.0, 10.0, 0.0, 400.0);
    assert!(tabs.cancel_drag());
    assert!(!tabs.is_dragging());
    assert!(!tabs.update_drag(120.0, 100.0, 0.0, 0.0, 400.0));
    assert!(matches!(tabs.end_drag(true), DragRelease::None));

    tabs.begin_drag(0, 40.0, 10.0, 0.0, 400.0);
    assert!(tabs.update_drag(120.0, 100.0, 0.0, 0.0, 400.0));
    assert!(tabs.cancel_drag());
    assert!(!tabs.is_dragging());
    assert!(matches!(tabs.end_drag(true), DragRelease::None));
}
