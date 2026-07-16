use super::*;
use std::collections::BTreeSet;

#[test]
fn close_unfocused_tabs_keeps_current_and_removes_others_descending() {
    assert_eq!(
        close_unfocused_tabs_plan(5, 2),
        Some(CloseUnfocusedTabsPlan {
            retained_index: 2,
            active_index_after: 0,
            remove_indices_desc: vec![4, 3, 1, 0],
        })
    );
}

#[test]
fn close_unfocused_tabs_keeps_first_tab() {
    assert_eq!(
        close_unfocused_tabs_plan(4, 0),
        Some(CloseUnfocusedTabsPlan {
            retained_index: 0,
            active_index_after: 0,
            remove_indices_desc: vec![3, 2, 1],
        })
    );
}

#[test]
fn close_unfocused_tabs_keeps_last_tab() {
    assert_eq!(
        close_unfocused_tabs_plan(4, 3),
        Some(CloseUnfocusedTabsPlan {
            retained_index: 3,
            active_index_after: 0,
            remove_indices_desc: vec![2, 1, 0],
        })
    );
}

#[test]
fn close_unfocused_tabs_handles_single_tab() {
    assert_eq!(
        close_unfocused_tabs_plan(1, 0),
        Some(CloseUnfocusedTabsPlan {
            retained_index: 0,
            active_index_after: 0,
            remove_indices_desc: Vec::new(),
        })
    );
}

#[test]
fn close_unfocused_tabs_rejects_invalid_selection() {
    assert_eq!(close_unfocused_tabs_plan(0, 0), None);
    assert_eq!(close_unfocused_tabs_plan(2, 2), None);
}

#[test]
fn workspace_tab_click_ignores_out_of_range_tabs() {
    assert_eq!(
        workspace_tab_click_plan(3, 1, 3, false, true),
        WorkspaceTabClickPlan::Ignore
    );
}

#[test]
fn workspace_tab_click_control_toggles_picker_without_switching() {
    assert_eq!(
        workspace_tab_click_plan(3, 1, 2, true, false),
        WorkspaceTabClickPlan::ToggleColorPicker { tab: 2 }
    );
}

#[test]
fn workspace_tab_click_switches_to_clicked_tab() {
    assert_eq!(
        workspace_tab_click_plan(4, 1, 3, false, false),
        WorkspaceTabClickPlan::BeginDrag {
            tab: 3,
            switch_to: Some(3),
            close_color_picker: false,
        }
    );
}

#[test]
fn workspace_tab_click_keeps_current_tab_and_closes_open_picker() {
    assert_eq!(
        workspace_tab_click_plan(4, 2, 2, false, true),
        WorkspaceTabClickPlan::BeginDrag {
            tab: 2,
            switch_to: None,
            close_color_picker: true,
        }
    );
}

#[test]
fn visual_ordered_pane_ids_sorts_top_to_bottom_then_left_to_right() {
    let panes = [
        SessionPaneRect::new("bottom-left", [0.0, 200.0, 100.0, 100.0]),
        SessionPaneRect::new("top-right", [200.0, 0.0, 100.0, 100.0]),
        SessionPaneRect::new("top-left", [0.0, 0.0, 100.0, 100.0]),
        SessionPaneRect::new("middle", [100.0, 100.0, 100.0, 100.0]),
    ];

    assert_eq!(
        visual_ordered_pane_ids(panes),
        vec!["top-left", "top-right", "middle", "bottom-left"]
    );
}

#[test]
fn adjacent_visual_pane_id_wraps_when_requested() {
    let ordered = [10, 20, 30];

    assert_eq!(adjacent_visual_pane_id(&ordered, 10, true, true), Some(30));
    assert_eq!(adjacent_visual_pane_id(&ordered, 30, false, true), Some(10));
}

#[test]
fn adjacent_visual_pane_id_stops_at_edges_without_wrap() {
    let ordered = [10, 20, 30];

    assert_eq!(adjacent_visual_pane_id(&ordered, 10, true, false), None);
    assert_eq!(adjacent_visual_pane_id(&ordered, 30, false, false), None);
    assert_eq!(
        adjacent_visual_pane_id(&ordered, 20, false, false),
        Some(30)
    );
}

#[test]
fn adjacent_visual_pane_id_requires_current_and_multiple_panes() {
    assert_eq!(adjacent_visual_pane_id(&[10], 10, false, true), None);
    assert_eq!(adjacent_visual_pane_id(&[10, 20], 30, false, true), None);
}

#[test]
fn hidden_split_drag_reveal_requires_collapsed_multi_pane_layout() {
    assert_eq!(
        hidden_split_drag_reveal_route(
            false,
            2,
            Some(42),
            800.0,
            12.0,
            0.0,
            24.0,
            1000.0,
            true,
        ),
        None
    );
    assert_eq!(
        hidden_split_drag_reveal_route(
            true,
            1,
            Some(42),
            800.0,
            12.0,
            0.0,
            24.0,
            1000.0,
            true,
        ),
        None
    );
}

#[test]
fn hidden_split_drag_reveals_first_secondary_route_from_top_edge_zone() {
    assert_eq!(
        hidden_split_drag_reveal_route(
            true,
            3,
            Some(42),
            721.0,
            12.0,
            0.0,
            24.0,
            1000.0,
            false,
        ),
        Some(42)
    );
    assert_eq!(
        hidden_split_drag_reveal_route(
            true,
            3,
            Some(42),
            720.0,
            12.0,
            0.0,
            24.0,
            1000.0,
            false,
        ),
        None
    );
}

#[test]
fn hidden_split_drag_reveals_from_host_affordance() {
    assert_eq!(
        hidden_split_drag_reveal_route(
            true,
            2,
            Some(7),
            10.0,
            80.0,
            0.0,
            24.0,
            1000.0,
            true,
        ),
        Some(7)
    );
}

#[test]
fn hidden_split_drag_reveal_has_no_target_without_secondary_route() {
    assert_eq!(
        hidden_split_drag_reveal_route(
            true, 2, None, 900.0, 12.0, 0.0, 24.0, 1000.0, true,
        ),
        None
    );
}

#[test]
fn pane_strip_position_top_aligned_uses_chrome_row() {
    // Pane sits at rect.top = min_top (jitter under tolerance) -> snap
    // to the workspace chrome row at logical y = 30.0; x and width
    // convert from physical to logical px.
    let input = PaneStripGeomInput {
        rect_left_phys: 400.0,
        rect_top_phys: 1.0,
        rect_width_phys: 800.0,
        scaled_margin_left_phys: 32.0,
        scaled_margin_top_phys: 16.0,
        chrome_top_logical: 30.0,
        min_top_phys: 0.0,
        scale_factor: 2.0,
    };
    let (x, y, w) = pane_strip_position(input);
    assert!((x - 216.0).abs() < 0.001);
    assert!((y - 30.0).abs() < 0.001);
    assert!((w - 400.0).abs() < 0.001);
}

#[test]
fn pane_strip_position_stacked_pane_renders_inside_its_area() {
    // Stacked pane (rect.top well past min_top) renders the strip at
    // its own logical y derived from rect_top + scaled_margin_top.
    let input = PaneStripGeomInput {
        rect_left_phys: 0.0,
        rect_top_phys: 600.0,
        rect_width_phys: 1000.0,
        scaled_margin_left_phys: 0.0,
        scaled_margin_top_phys: 20.0,
        chrome_top_logical: 30.0,
        min_top_phys: 0.0,
        scale_factor: 2.0,
    };
    let (_x, y, _w) = pane_strip_position(input);
    assert!((y - 310.0).abs() < 0.001);
}

#[test]
fn is_pane_top_aligned_tolerates_taffy_jitter_and_rejects_real_rows() {
    assert!(is_pane_top_aligned(2.5, 0.0));
    assert!(is_pane_top_aligned(0.0, 2.5));
    assert!(!is_pane_top_aligned(5.0, 0.0));
    assert!(!is_pane_top_aligned(0.0, 8.0));
}

#[test]
fn buffer_tabs_scroll_dx_horizontal_swipe_negates_to_forward() {
    // Trackpad swipe-right (x > 0) reveals content to the right ->
    // returned dx should be positive (forward in scroll_by).
    let dx = buffer_tabs_scroll_dx(SessionScrollDelta::Pixels { x: 12.0, y: 0.0 }, 0.01);
    assert!(dx < 0.0);

    // And swipe-left -> backward.
    let dx_left =
        buffer_tabs_scroll_dx(SessionScrollDelta::Pixels { x: -12.0, y: 0.0 }, 0.01);
    assert!(dx_left > 0.0);
}

#[test]
fn buffer_tabs_scroll_dx_wheel_down_advances_forward() {
    // Plain mouse wheel without horizontal axis falls through to y.
    let dx = buffer_tabs_scroll_dx(SessionScrollDelta::Lines { x: 0.0, y: 1.0 }, 0.01);
    // y > 0 (wheel down) -> dx negative under shared "swipe = swipe
    // direction" sign convention, matching native scroll_by.
    assert!((dx + 60.0).abs() < 0.001);
}

#[test]
fn buffer_tabs_scroll_dx_pixel_picks_dominant_axis() {
    // Pixel delta with |y| > |x| should use y.
    let dx = buffer_tabs_scroll_dx(SessionScrollDelta::Pixels { x: 2.0, y: 10.0 }, 0.01);
    assert!((dx + 10.0).abs() < 0.001);
}

#[test]
fn buffer_tabs_scroll_dx_below_epsilon_returns_zero() {
    let dx = buffer_tabs_scroll_dx(SessionScrollDelta::Pixels { x: 0.0, y: 0.001 }, 0.01);
    assert_eq!(dx, 0.0);
}

#[test]
fn rebase_tab_indexed_map_for_move_rewrites_keys() {
    let mut map: std::collections::HashMap<usize, &'static str> =
        std::collections::HashMap::new();
    map.insert(0, "a");
    map.insert(1, "b");
    map.insert(2, "c");
    map.insert(3, "d");

    rebase_tab_indexed_map_for_move(&mut map, 1, 3);

    // Vec::remove(1) + insert(3): a=0, c=1, d=2, b=3.
    let expected: std::collections::HashMap<usize, &'static str> =
        [(0, "a"), (1, "c"), (2, "d"), (3, "b")]
            .into_iter()
            .collect();
    assert_eq!(map, expected);
}

#[test]
fn rebase_tab_indexed_map_for_move_is_noop_when_from_equals_to() {
    let mut map: std::collections::HashMap<usize, i32> =
        [(0, 10), (1, 20)].into_iter().collect();
    rebase_tab_indexed_map_for_move(&mut map, 1, 1);
    assert_eq!(map.get(&0), Some(&10));
    assert_eq!(map.get(&1), Some(&20));
}

#[test]
fn rebase_tab_indexed_btreemap_for_move_matches_hashmap_variant() {
    let mut map: BTreeMap<usize, &'static str> = [(0, "a"), (1, "b"), (2, "c"), (3, "d")]
        .into_iter()
        .collect();
    rebase_tab_indexed_btreemap_for_move(&mut map, 3, 0);
    // Vec::remove(3) + insert(0): d=0, a=1, b=2, c=3.
    assert_eq!(map.get(&0), Some(&"d"));
    assert_eq!(map.get(&1), Some(&"a"));
    assert_eq!(map.get(&2), Some(&"b"));
    assert_eq!(map.get(&3), Some(&"c"));
}

#[test]
fn rebase_tab_indexed_map_for_remove_drops_and_shifts() {
    let mut map: std::collections::HashMap<usize, &'static str> =
        [(0, "a"), (1, "b"), (2, "c"), (3, "d")]
            .into_iter()
            .collect();
    rebase_tab_indexed_map_for_remove(&mut map, 1);
    let expected: std::collections::HashMap<usize, &'static str> =
        [(0, "a"), (1, "c"), (2, "d")].into_iter().collect();
    assert_eq!(map, expected);
}

fn build_layout_with_split(
    base_route: u64,
    new_route: u64,
    axis: SplitAxis,
    placement: SplitPlacement,
) -> SessionLayout {
    let mut layout = SessionLayout::new(
        SessionLeafSpec::new(SessionLeafKind::Terminal).with_external_id(base_route),
    );
    layout
        .split_focused(
            axis,
            placement,
            SessionLeafSpec::new(SessionLeafKind::Editor).with_external_id(new_route),
        )
        .expect("split should succeed");
    layout
}

#[test]
fn session_layout_active_route_set_collects_all_active_externals() {
    let layout =
        build_layout_with_split(10, 20, SplitAxis::Vertical, SplitPlacement::After);
    let routes = session_layout_active_route_set(&layout).expect("routes set");
    let expected: BTreeSet<u64> = [10, 20].into_iter().collect();
    assert_eq!(routes, expected);
}

#[test]
fn session_layout_active_route_set_returns_none_if_any_leaf_lacks_external_id() {
    let mut layout = SessionLayout::new(SessionLeafSpec::new(SessionLeafKind::Terminal));
    layout
        .split_focused(
            SplitAxis::Horizontal,
            SplitPlacement::After,
            SessionLeafSpec::new(SessionLeafKind::Editor).with_external_id(2),
        )
        .expect("split should succeed");
    assert_eq!(session_layout_active_route_set(&layout), None);
}

#[test]
fn session_layout_close_focused_route_pair_returns_sibling_route() {
    let layout =
        build_layout_with_split(7, 9, SplitAxis::Horizontal, SplitPlacement::After);
    // After the split the new leaf (route 9) is focused, so closing the
    // focused leaf should refocus the original (route 7).
    assert_eq!(
        session_layout_close_focused_route_pair(&layout),
        Some((9, 7))
    );
}

#[test]
fn session_layout_close_focused_route_pair_is_none_for_last_leaf() {
    let layout = SessionLayout::new(
        SessionLeafSpec::new(SessionLeafKind::Terminal).with_external_id(1),
    );
    assert_eq!(session_layout_close_focused_route_pair(&layout), None);
}

#[test]
fn session_layout_split_focused_route_set_previews_without_mutating() {
    let layout = SessionLayout::new(
        SessionLeafSpec::new(SessionLeafKind::Terminal).with_external_id(1),
    );
    let preview = session_layout_split_focused_route_set(
        &layout,
        SplitAxis::Vertical,
        SplitPlacement::After,
        SessionLeafSpec::new(SessionLeafKind::Editor).with_external_id(2),
    )
    .expect("preview should succeed");
    let expected: BTreeSet<u64> = [1, 2].into_iter().collect();
    assert_eq!(preview, expected);
    // Original layout still has only the original leaf.
    assert_eq!(layout.active_leaves().len(), 1);
}

#[test]
fn session_leaf_external_id_looks_up_via_layout() {
    let layout = SessionLayout::new(
        SessionLeafSpec::new(SessionLeafKind::Agent).with_external_id(42),
    );
    let focused = layout.focused_leaf();
    assert_eq!(session_leaf_external_id(&layout, focused), Some(42));
}

#[test]
fn session_layout_first_secondary_route_skips_workspace_root() {
    let layout =
        build_layout_with_split(100, 200, SplitAxis::Vertical, SplitPlacement::After);
    assert_eq!(
        session_layout_first_secondary_route(&layout, 100),
        Some(200)
    );
    // When the only leaf is the workspace itself there is nothing to pick.
    let solo = SessionLayout::new(
        SessionLeafSpec::new(SessionLeafKind::Terminal).with_external_id(7),
    );
    assert_eq!(session_layout_first_secondary_route(&solo, 7), None);
}

#[test]
fn session_layout_secondary_routes_lists_in_layout_order() {
    let layout =
        build_layout_with_split(1, 2, SplitAxis::Vertical, SplitPlacement::After);
    assert_eq!(session_layout_secondary_routes(&layout, 1), vec![2]);
    assert!(session_layout_secondary_routes(&layout, 9).contains(&1));
}

#[test]
fn session_layout_focus_adjacent_route_moves_to_sibling() {
    let layout =
        build_layout_with_split(1, 2, SplitAxis::Vertical, SplitPlacement::After);
    // After the split, route 2 is focused. Going previous lands on 1.
    assert_eq!(
        session_layout_focus_adjacent_route(layout.clone(), true, false, 2),
        Some(1)
    );
    // Going forward without wrap stays put — no move, return None.
    assert_eq!(
        session_layout_focus_adjacent_route(layout, false, false, 2),
        None
    );
}

#[test]
fn session_layout_focus_adjacent_route_wraps_when_requested() {
    let layout =
        build_layout_with_split(1, 2, SplitAxis::Vertical, SplitPlacement::After);
    // Forward + wrap from the last leaf circles back to route 1.
    assert_eq!(
        session_layout_focus_adjacent_route(layout, false, true, 2),
        Some(1)
    );
}

#[test]
fn session_layout_focus_adjacent_route_returns_none_when_same_route() {
    let layout = SessionLayout::new(
        SessionLeafSpec::new(SessionLeafKind::Terminal).with_external_id(5),
    );
    // Single leaf can't move — wrap collapses to self, helper returns None.
    assert_eq!(
        session_layout_focus_adjacent_route(layout, false, true, 5),
        None
    );
}

#[test]
fn session_layout_focus_edge_route_returns_first_or_last() {
    let layout =
        build_layout_with_split(10, 20, SplitAxis::Vertical, SplitPlacement::After);
    assert_eq!(
        session_layout_focus_edge_route(layout.clone(), false),
        Some(10)
    );
    assert_eq!(session_layout_focus_edge_route(layout, true), Some(20));
}

struct FakeGrid {
    slot: Option<ClosingContextSlot>,
}

impl ContextGridLike for FakeGrid {
    fn owns_route(&self, _route_id: u64) -> bool {
        self.slot.is_some()
    }

    fn describe_closing_route(
        &self,
        _grid_index: usize,
        _route_id: u64,
    ) -> ClosingContextSlot {
        self.slot
            .expect("describe_closing_route called only when owns_route is true")
    }
}

fn fake_slot(
    grid_index: usize,
    workspace_id: Option<u64>,
    is_workspace_root: bool,
    shell_pid: u32,
    is_terminal_context: bool,
) -> ClosingContextSlot {
    ClosingContextSlot {
        grid_index,
        workspace_id,
        is_workspace_root,
        shell_pid,
        is_terminal_context,
    }
}

#[test]
fn find_closing_workspace_descriptor_picks_owning_grid() {
    let slot = fake_slot(2, Some(7), true, 4321, true);
    let grids = vec![
        FakeGrid { slot: None },
        FakeGrid { slot: None },
        FakeGrid { slot: Some(slot) },
        FakeGrid { slot: None },
    ];
    let found = find_closing_workspace_descriptor(&grids, 99);
    assert_eq!(found, Some(slot));
}

#[test]
fn find_closing_workspace_descriptor_none_when_nobody_owns() {
    let grids = vec![FakeGrid { slot: None }, FakeGrid { slot: None }];
    assert_eq!(find_closing_workspace_descriptor(&grids, 99), None);
}

#[test]
fn find_closing_workspace_descriptor_returns_first_owner_in_order() {
    let first = fake_slot(0, Some(1), false, 100, true);
    let second = fake_slot(1, Some(2), true, 200, true);
    let grids = vec![
        FakeGrid { slot: Some(first) },
        FakeGrid { slot: Some(second) },
    ];
    let found = find_closing_workspace_descriptor(&grids, 99);
    assert_eq!(found, Some(first));
}

// ── PaneLayoutSnapshot → SessionLayout mirror ──────────────────
//
// These exercise the shared layout-mapping function that drives web
// split-pane parity with the desktop. The rect helper mirrors the
// WASM `push_pane_rects` walk so the assertions check the geometry
// every frontend ultimately renders.

use neoism_protocol::workspace::{
    PaneLayoutSnapshot, PaneLayoutSnapshotNode, PaneSplitAxis,
    PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
};
use std::path::PathBuf;

fn snapshot_leaf(external_id: u64, path: Option<&str>) -> PaneLayoutSnapshotNode {
    PaneLayoutSnapshotNode::Leaf {
        pane_external_id: external_id,
        surface_id: format!("surface-{external_id}"),
        session_id: "session".to_string(),
        path: path.map(PathBuf::from),
        route_id: Some(external_id),
    }
}

fn snapshot_from(root: PaneLayoutSnapshotNode, focused: u64) -> PaneLayoutSnapshot {
    PaneLayoutSnapshot {
        schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
        workspace_id: "ws".to_string(),
        focused_pane_external_id: focused,
        root,
    }
}

/// Pure copy of the WASM `push_pane_rects` recursion so the test can
/// assert the exact normalized rectangles the web overlay draws.
fn rects(
    layout: &SessionLayout,
    node: SessionNodeId,
    rect: (f32, f32, f32, f32),
    out: &mut Vec<(u64, (f32, f32, f32, f32))>,
) {
    match layout.node(node).unwrap() {
        SessionNode::Leaf(leaf) => {
            out.push((leaf.external_id.unwrap(), rect));
        }
        SessionNode::Split(split) => {
            let (x, y, w, h) = rect;
            match split.axis {
                SplitAxis::Horizontal => {
                    let first_w = w * split.ratio;
                    rects(layout, split.first, (x, y, first_w, h), out);
                    rects(layout, split.second, (x + first_w, y, w - first_w, h), out);
                }
                SplitAxis::Vertical => {
                    let first_h = h * split.ratio;
                    rects(layout, split.first, (x, y, w, first_h), out);
                    rects(layout, split.second, (x, y + first_h, w, h - first_h), out);
                }
            }
        }
    }
}

fn rect_map(layout: &SessionLayout) -> BTreeMap<u64, (f32, f32, f32, f32)> {
    let mut out = Vec::new();
    rects(
        layout,
        layout.active_tab().root,
        (0.0, 0.0, 1.0, 1.0),
        &mut out,
    );
    out.into_iter().collect()
}

fn approx(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-4, "expected {b}, got {a}");
}

#[test]
fn snapshot_single_leaf_is_one_full_pane() {
    let snapshot = snapshot_from(snapshot_leaf(7, Some("a.rs")), 7);
    let layout = SessionLayout::from_pane_layout_snapshot(&snapshot).unwrap();
    assert_eq!(layout.active_leaf_external_ids(), vec![7]);
    assert_eq!(layout.focused_external_id(), Some(7));
    let map = rect_map(&layout);
    approx(map[&7].2, 1.0);
    approx(map[&7].3, 1.0);
}

#[test]
fn snapshot_horizontal_split_maps_to_left_right_with_ratio() {
    let snapshot = snapshot_from(
        PaneLayoutSnapshotNode::Split {
            axis: PaneSplitAxis::Horizontal,
            ratios: vec![0.7, 0.3],
            children: vec![snapshot_leaf(1, None), snapshot_leaf(2, None)],
        },
        2,
    );
    let layout = SessionLayout::from_pane_layout_snapshot(&snapshot).unwrap();
    assert_eq!(layout.focused_external_id(), Some(2));
    let map = rect_map(&layout);
    // Left pane sits at x=0 with 70% width; right pane fills the rest.
    approx(map[&1].0, 0.0);
    approx(map[&1].2, 0.7);
    approx(map[&2].0, 0.7);
    approx(map[&2].2, 0.3);
}

#[test]
fn snapshot_three_way_split_preserves_proportions() {
    // N-ary 25/50/25 vertical split must lower to nested binary
    // splits that still paint thirds at the same y offsets.
    let snapshot = snapshot_from(
        PaneLayoutSnapshotNode::Split {
            axis: PaneSplitAxis::Vertical,
            ratios: vec![0.25, 0.5, 0.25],
            children: vec![
                snapshot_leaf(1, None),
                snapshot_leaf(2, None),
                snapshot_leaf(3, None),
            ],
        },
        1,
    );
    let layout = SessionLayout::from_pane_layout_snapshot(&snapshot).unwrap();
    let map = rect_map(&layout);
    approx(map[&1].1, 0.0);
    approx(map[&1].3, 0.25);
    approx(map[&2].1, 0.25);
    approx(map[&2].3, 0.5);
    approx(map[&3].1, 0.75);
    approx(map[&3].3, 0.25);
}

#[test]
fn snapshot_nested_split_keeps_nesting() {
    // Outer horizontal split; the right child is itself a vertical
    // split. The mapping must keep that nesting so the web renders
    // the same tree the desktop does.
    let snapshot = snapshot_from(
        PaneLayoutSnapshotNode::Split {
            axis: PaneSplitAxis::Horizontal,
            ratios: vec![0.5, 0.5],
            children: vec![
                snapshot_leaf(1, None),
                PaneLayoutSnapshotNode::Split {
                    axis: PaneSplitAxis::Vertical,
                    ratios: vec![0.5, 0.5],
                    children: vec![snapshot_leaf(2, None), snapshot_leaf(3, None)],
                },
            ],
        },
        3,
    );
    let layout = SessionLayout::from_pane_layout_snapshot(&snapshot).unwrap();
    let map = rect_map(&layout);
    approx(map[&1].2, 0.5);
    approx(map[&1].3, 1.0);
    // Right column is split top/bottom.
    approx(map[&2].0, 0.5);
    approx(map[&2].3, 0.5);
    approx(map[&3].0, 0.5);
    approx(map[&3].1, 0.5);
    approx(map[&3].3, 0.5);
}

#[test]
fn snapshot_tabs_collapse_to_active_child() {
    let snapshot = snapshot_from(
        PaneLayoutSnapshotNode::Tabs {
            active: 1,
            children: vec![snapshot_leaf(1, None), snapshot_leaf(2, None)],
        },
        2,
    );
    let layout = SessionLayout::from_pane_layout_snapshot(&snapshot).unwrap();
    // Only the active tab occupies the pane region.
    assert_eq!(layout.active_leaf_external_ids(), vec![2]);
    assert_eq!(layout.focused_external_id(), Some(2));
}

#[test]
fn snapshot_roundtrips_through_serde_like_the_wire() {
    // The web feeds the daemon's JSON blob straight through; ensure a
    // serialized snapshot deserializes and mirrors identically.
    let snapshot = snapshot_from(
        PaneLayoutSnapshotNode::Split {
            axis: PaneSplitAxis::Horizontal,
            ratios: vec![0.6, 0.4],
            children: vec![snapshot_leaf(1, None), snapshot_leaf(2, None)],
        },
        1,
    );
    let json = serde_json::to_string(&snapshot).unwrap();
    let back: PaneLayoutSnapshot = serde_json::from_str(&json).unwrap();
    let layout = SessionLayout::from_pane_layout_snapshot(&back).unwrap();
    let map = rect_map(&layout);
    approx(map[&1].2, 0.6);
    approx(map[&2].2, 0.4);
}
