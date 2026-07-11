use super::*;

use std::collections::{BTreeMap, BTreeSet};

/// Pure logical-px (x, y, width) for a per-pane tab strip given a pane
/// rectangle and the grid's chrome alignment metadata.
///
/// This is the policy shared by every native call site that places a pane
/// strip (`pane_strip_geometry`, `pane_strip_hit_at`, `strip_at_point`,
/// `render_pane_tabs`, `render_tab_drop_preview`, `apply_pane_chrome_offsets`).
/// Web frontends substitute their own physical-px rects + a scale of 1.0.
pub fn pane_strip_position(input: PaneStripGeomInput) -> (f32, f32, f32) {
    let PaneStripGeomInput {
        rect_left_phys,
        rect_top_phys,
        rect_width_phys,
        scaled_margin_left_phys,
        scaled_margin_top_phys,
        chrome_top_logical,
        min_top_phys,
        scale_factor,
    } = input;
    let top_aligned = is_pane_top_aligned(rect_top_phys, min_top_phys);
    let x = (rect_left_phys + scaled_margin_left_phys) / scale_factor;
    let y = if top_aligned {
        chrome_top_logical
    } else {
        (rect_top_phys + scaled_margin_top_phys) / scale_factor
    };
    let w = rect_width_phys / scale_factor;
    (x, y, w)
}

/// Whether a pane rectangle sits on the same visual "top row" as the
/// smallest-top pane in its grid.
///
/// Pulled out so the chrome-offset path can short-circuit without forming
/// the rest of the [`PaneStripGeomInput`] tuple.
#[inline]
pub fn is_pane_top_aligned(rect_top_phys: f32, min_top_phys: f32) -> bool {
    (rect_top_phys - min_top_phys).abs() < PANE_TOP_ALIGN_TOLERANCE_PX
}

/// Horizontal scroll delta (logical px) for a one-row, mostly-horizontal
/// strip (buffer tabs, breadcrumbs).
///
/// Sign convention matches the native trackpad/wheel:
/// - Positive returned value advances scroll forward (reveals content on
///   the right).
/// - Trackpad horizontal swipe returns x in the swipe direction, so we
///   negate to keep "swipe right" mapped to "reveal right".
/// - Plain mouse wheels (no horizontal axis) fall through to the vertical
///   delta - wheel-down maps to forward scroll.
/// - Pixel deltas pick whichever axis dominates.
///
/// Returns `0.0` when the resulting delta is below `epsilon` so callers can
/// early-out on noise without re-implementing the threshold.
pub fn buffer_tabs_scroll_dx(delta: SessionScrollDelta, epsilon: f32) -> f32 {
    let dx = match delta {
        SessionScrollDelta::Lines { x, y } => {
            if x.abs() > 0.0 {
                -x * BUFFER_TABS_WHEEL_LINE_TO_PX
            } else {
                -y * BUFFER_TABS_WHEEL_LINE_TO_PX
            }
        }
        SessionScrollDelta::Pixels { x, y } => {
            if x.abs() > y.abs() {
                -x
            } else {
                -y
            }
        }
    };
    if dx.abs() < epsilon {
        0.0
    } else {
        dx
    }
}

/// Returns the tab index reached by keyboard-style previous/next navigation.
///
/// This is the shared policy behind desktop tab cycling and the web tab model:
/// navigation wraps at both ends, and an empty tab list has no valid target.
pub fn adjacent_tab_index(
    tab_count: usize,
    current: usize,
    previous: bool,
) -> Option<usize> {
    if tab_count == 0 || current >= tab_count {
        return None;
    }
    if previous {
        Some(if current == 0 {
            tab_count - 1
        } else {
            current - 1
        })
    } else {
        Some(if current + 1 == tab_count {
            0
        } else {
            current + 1
        })
    }
}

/// Target index for moving the active tab one slot left/right with wrap.
pub fn active_tab_move_target(
    tab_count: usize,
    current: usize,
    previous: bool,
) -> Option<usize> {
    if tab_count <= 1 {
        return None;
    }
    adjacent_tab_index(tab_count, current, previous)
}

/// Active index after closing the tab at `closing`.
///
/// Closing a middle tab keeps the same numeric slot focused, which now contains
/// the next tab. Closing the last tab falls back to the new last tab.
pub fn active_tab_index_after_close(tab_count: usize, closing: usize) -> Option<usize> {
    if tab_count <= 1 || closing >= tab_count {
        return None;
    }
    Some(closing.min(tab_count - 2))
}

/// Rebase an existing tab-indexed side table after removing `removed`.
pub fn rebase_tab_index_after_remove(index: usize, removed: usize) -> Option<usize> {
    if index < removed {
        Some(index)
    } else if index > removed {
        Some(index - 1)
    } else {
        None
    }
}

/// Rebase `current` after a `Vec::remove(from) + Vec::insert(to)` tab reorder.
pub fn rebase_tab_index_after_move(current: usize, from: usize, to: usize) -> usize {
    if current == from {
        to
    } else if from < to {
        if current > from && current <= to {
            current - 1
        } else {
            current
        }
    } else if current >= to && current < from {
        current + 1
    } else {
        current
    }
}

/// Rebase a tab-indexed side table (e.g. titles, badge state) after the
/// host performs a `Vec::remove(from) + Vec::insert(to)` workspace reorder.
///
/// This is the value-preserving counterpart of [`rebase_tab_index_after_move`]:
/// every entry's key is rewritten through that policy. The map is taken by
/// `&mut` and rebuilt in place using `std::mem::take` to avoid the borrow
/// gymnastics callers would otherwise need.
pub fn rebase_tab_indexed_map_for_move<V, S>(
    map: &mut std::collections::HashMap<usize, V, S>,
    from: usize,
    to: usize,
) where
    S: std::hash::BuildHasher + Default,
{
    if from == to {
        return;
    }
    let old = std::mem::take(map);
    for (idx, value) in old {
        map.insert(rebase_tab_index_after_move(idx, from, to), value);
    }
}

/// `BTreeMap` variant of [`rebase_tab_indexed_map_for_move`] for hosts that
/// keep tab side tables in ordered maps.
pub fn rebase_tab_indexed_btreemap_for_move<V>(
    map: &mut BTreeMap<usize, V>,
    from: usize,
    to: usize,
) {
    if from == to {
        return;
    }
    let old = std::mem::take(map);
    for (idx, value) in old {
        map.insert(rebase_tab_index_after_move(idx, from, to), value);
    }
}

/// Drop entries whose tab index was just removed and rebase the rest.
///
/// Mirrors the desktop `remove_title_at_index` step that runs after
/// `close_current_context`: entries above `removed_index` shift down by one,
/// the entry at `removed_index` is discarded, and below-index entries stay put.
pub fn rebase_tab_indexed_map_for_remove<V, S>(
    map: &mut std::collections::HashMap<usize, V, S>,
    removed_index: usize,
) where
    S: std::hash::BuildHasher + Default,
{
    let old = std::mem::take(map);
    for (idx, value) in old {
        if let Some(next) = rebase_tab_index_after_remove(idx, removed_index) {
            map.insert(next, value);
        }
    }
}

/// Shared policy for "close other tabs" style commands.
///
/// The focused tab is retained and becomes index 0 after compaction. Removal
/// indices are returned in descending order so host frontends can apply them to
/// Vec-backed state without rebasing each pending removal.
pub fn close_unfocused_tabs_plan(
    tab_count: usize,
    current_index: usize,
) -> Option<CloseUnfocusedTabsPlan> {
    if tab_count == 0 || current_index >= tab_count {
        return None;
    }

    Some(CloseUnfocusedTabsPlan {
        retained_index: current_index,
        active_index_after: 0,
        remove_indices_desc: (0..tab_count)
            .rev()
            .filter(|index| *index != current_index)
            .collect(),
    })
}

/// Shared policy for moving the active buffer tab between the workspace strip
/// and split-pane strips.
///
/// Host frontends still execute the move because file buffers, agent panes, and
/// terminal PTYs are host resources. This function only decides the target:
/// workspace tabs move into the first available split or tear out into a new
/// split, while pane-local tabs move back to the workspace strip.
pub fn active_tab_move_to_split_stack_plan(
    source: SessionTabStripRef,
    first_secondary_route: Option<u64>,
    _tab_kind: SessionMovableTabKind,
) -> SessionTabMovePlan {
    let destination = match source {
        SessionTabStripRef::Workspace => first_secondary_route
            .map(SessionTabMoveDestination::ExistingPane)
            .unwrap_or(SessionTabMoveDestination::NewSplit),
        SessionTabStripRef::Pane(_) => SessionTabMoveDestination::Workspace,
    };
    SessionTabMovePlan {
        source,
        destination,
    }
}

/// Orders split-pane buffer strips for picker-style UI.
///
/// Renderer-backed panes come first in the active session route order. Any
/// surviving host tab strips without a live renderer route are appended in
/// numeric order so they remain reachable while keeping the main pane order
/// stable.
pub fn ordered_secondary_routes_with_orphans(
    secondary_routes: impl IntoIterator<Item = u64>,
    tab_strip_routes: impl IntoIterator<Item = u64>,
) -> Vec<u64> {
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    for route in secondary_routes {
        if seen.insert(route) {
            ordered.push(route);
        }
    }
    let mut orphans = Vec::new();
    for route in tab_strip_routes {
        if seen.insert(route) {
            orphans.push(route);
        }
    }
    orphans.sort_unstable();
    ordered.extend(orphans);
    ordered
}

/// Resolves which buffer-tab strip owns keyboard actions for the focused pane.
///
/// The workspace strip is the fallback whenever focus is on the workspace
/// route, the focused route is unknown, or the focused route has no pane-local
/// tab strip. Host frontends still own the actual tab collections; this helper
/// only centralizes the route/strip policy used by desktop and web.
pub fn focused_tab_strip(
    workspace_external_id: Option<u64>,
    focused_external_id: Option<u64>,
    pane_tab_routes: impl IntoIterator<Item = u64>,
) -> SessionTabStripRef {
    let Some(focused_external_id) = focused_external_id else {
        return SessionTabStripRef::Workspace;
    };
    if workspace_external_id == Some(focused_external_id) {
        return SessionTabStripRef::Workspace;
    }
    if pane_tab_routes
        .into_iter()
        .any(|route| route == focused_external_id)
    {
        SessionTabStripRef::Pane(focused_external_id)
    } else {
        SessionTabStripRef::Workspace
    }
}

/// Translate a [`SessionLeafId`] into the host-owned external id, if any.
///
/// Free-function form of `layout.leaf(leaf).and_then(|l| l.external_id)`,
/// matched in shape to the rest of this module so the desktop
/// `context::manager` mirror code can drop the private `session_leaf_route`
/// helper that used to live there.
pub fn session_leaf_external_id(
    layout: &SessionLayout,
    leaf: SessionLeafId,
) -> Option<u64> {
    layout.leaf(leaf).and_then(|leaf| leaf.external_id)
}

/// Set of every active-tab leaf's external id, or `None` if any leaf is missing
/// an external id (which makes the layout useless as a route mirror).
pub fn session_layout_active_route_set(layout: &SessionLayout) -> Option<BTreeSet<u64>> {
    layout
        .active_leaves()
        .into_iter()
        .map(|leaf| session_leaf_external_id(layout, leaf))
        .collect()
}

/// Decide which external id will be focused after closing the focused leaf,
/// without mutating the input.
///
/// Returns `Some((closing_route, focus_route_after_close))`. Returns `None`
/// when the focused leaf cannot be closed via the shared model (e.g. it is the
/// last leaf in its tab and its tab is the only tab — host must collapse the
/// whole workspace instead).
pub fn session_layout_close_focused_route_pair(
    layout: &SessionLayout,
) -> Option<(u64, u64)> {
    let closing_leaf = layout.focused_leaf();
    let closing_route = session_leaf_external_id(layout, closing_leaf)?;
    let mut preview = layout.clone();
    let focus_leaf = preview.close_focused_leaf().ok()??;
    preview.validate().ok()?;
    let focus_route = session_leaf_external_id(&preview, focus_leaf)?;
    Some((closing_route, focus_route))
}

/// Compute the route set that a split would produce, without mutating the
/// input. The host uses this to debug-assert that its renderer-owned grid
/// produced the same routes the shared model would have.
pub fn session_layout_split_focused_route_set(
    layout: &SessionLayout,
    axis: SplitAxis,
    placement: SplitPlacement,
    spec: SessionLeafSpec,
) -> Option<BTreeSet<u64>> {
    let preview = layout.preview_split_focused(axis, placement, spec).ok()?;
    Some(preview.active_external_ids_after.into_iter().collect())
}

/// First non-workspace route in `layout`, used by desktop callers to pick a
/// deterministic split-pane fallback when the workspace root is excluded.
///
/// `None` means the layout has no secondary pane to focus.
pub fn session_layout_first_secondary_route(
    layout: &SessionLayout,
    workspace_route: u64,
) -> Option<u64> {
    layout.first_external_id_except(workspace_route)
}

/// Every non-workspace route in `layout` in mirrored layout order.
///
/// Empty vec means the workspace has no secondary panes.
pub fn session_layout_secondary_routes(
    layout: &SessionLayout,
    workspace_route: u64,
) -> Vec<u64> {
    layout.external_ids_except(workspace_route)
}

/// Pure adapter for [`SessionLayout::focus_adjacent_leaf`] that returns the
/// new focus's external id, or `None` when focus did not move (already at
/// the wrap edge, missing external id, or the same route).
///
/// Takes the layout by value-mutable clone — desktop callers want the
/// pre-mutation layout preserved and only need the target route. The check
/// `target_route != current_route` is intentional so callers can fall back
/// to their renderer-owned split navigation when the policy says "no move".
pub fn session_layout_focus_adjacent_route(
    mut layout: SessionLayout,
    previous: bool,
    wrap: bool,
    current_route: u64,
) -> Option<u64> {
    let before = layout.focused_leaf();
    let after = layout.focus_adjacent_leaf(previous, wrap).ok()?;
    if before == after {
        return None;
    }
    let target_route = session_leaf_external_id(&layout, after)?;
    (target_route != current_route).then_some(target_route)
}

/// Pure adapter for [`SessionLayout::focus_edge_leaf`] that returns the
/// first or last visible leaf's external id, or `None` when the layout has
/// no leaves or the edge leaf is missing an external id.
pub fn session_layout_focus_edge_route(
    mut layout: SessionLayout,
    last: bool,
) -> Option<u64> {
    let target_leaf = layout.focus_edge_leaf(last).ok()?;
    session_leaf_external_id(&layout, target_leaf)
}

/// Plans a workspace-strip tab click after host hit testing has selected a tab.
///
/// The host owns pointer geometry, drag state, color-picker contents, and route
/// switching. The shared policy decides which tab-local action should run and
/// which session index should become active for a normal click.
pub fn workspace_tab_click_plan(
    tab_count: usize,
    current_index: usize,
    clicked_tab: usize,
    control_key: bool,
    color_picker_open: bool,
) -> WorkspaceTabClickPlan {
    if clicked_tab >= tab_count {
        return WorkspaceTabClickPlan::Ignore;
    }

    if control_key {
        return WorkspaceTabClickPlan::ToggleColorPicker { tab: clicked_tab };
    }

    WorkspaceTabClickPlan::BeginDrag {
        tab: clicked_tab,
        switch_to: (clicked_tab != current_index).then_some(clicked_tab),
        close_color_picker: color_picker_open,
    }
}

/// Selects the route to reveal when a drag hovers over collapsed split panes.
///
/// Desktop and web own their actual hit testing and focus changes. The shared
/// policy is intentionally narrower: only collapsed multi-pane layouts can be
/// revealed, and the first secondary route is the deterministic target when
/// the pointer is over either the workspace strip's right-edge reveal zone or
/// another host-provided reveal affordance.
pub fn hidden_split_drag_reveal_route(
    splits_hidden: bool,
    pane_count: usize,
    first_secondary_route: Option<u64>,
    mouse_x: f32,
    mouse_y: f32,
    chrome_top: f32,
    strip_height: f32,
    logical_width: f32,
    reveal_affordance_hit: bool,
) -> Option<u64> {
    if !splits_hidden || pane_count <= 1 {
        return None;
    }
    let over_hidden_top_edge = mouse_y >= chrome_top
        && mouse_y < chrome_top + strip_height
        && mouse_x > logical_width * 0.72;
    if over_hidden_top_edge || reveal_affordance_hit {
        first_secondary_route
    } else {
        None
    }
}

/// Chooses the nearest pane in the requested horizontal direction.
///
/// Desktop supplies Taffy-backed rectangles and web can supply DOM/grid
/// rectangles, but the policy is shared: prefer panes in the requested
/// direction, then panes with vertical overlap, then shortest edge distance,
/// then closest vertical center.
pub fn nearest_horizontal_pane<K: Copy + PartialEq>(
    current: SessionPaneRect<K>,
    candidates: impl IntoIterator<Item = SessionPaneRect<K>>,
    right: bool,
) -> Option<K> {
    let current_right = current.left + current.width;
    let current_bottom = current.top + current.height;
    let current_center_y = current.top + current.height * 0.5;

    candidates
        .into_iter()
        .filter(|candidate| candidate.id != current.id)
        .filter_map(|candidate| {
            let right_edge = candidate.left + candidate.width;
            let bottom = candidate.top + candidate.height;
            let edge_distance = if right {
                candidate.left - current_right
            } else {
                current.left - right_edge
            };
            if edge_distance < -1.0 {
                return None;
            }
            let overlaps_y = current.top < bottom && candidate.top < current_bottom;
            let center_y = candidate.top + candidate.height * 0.5;
            Some((
                candidate.id,
                overlaps_y,
                edge_distance.max(0.0),
                (center_y - current_center_y).abs(),
            ))
        })
        .min_by(|a, b| {
            let overlap_rank_a = if a.1 { 0 } else { 1 };
            let overlap_rank_b = if b.1 { 0 } else { 1 };
            overlap_rank_a
                .cmp(&overlap_rank_b)
                .then_with(|| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal))
        })
        .map(|(id, _, _, _)| id)
}

/// Orders panes by visual reading order: top-to-bottom, then left-to-right.
///
/// Hosts provide renderer-specific rectangles; the shared policy only decides
/// deterministic focus order from those bounds.
pub fn visual_ordered_pane_ids<K: Copy>(
    panes: impl IntoIterator<Item = SessionPaneRect<K>>,
) -> Vec<K> {
    let mut panes: Vec<_> = panes.into_iter().collect();
    panes.sort_by(|a, b| {
        a.top
            .partial_cmp(&b.top)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.left
                    .partial_cmp(&b.left)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    panes.into_iter().map(|pane| pane.id).collect()
}

/// Returns the adjacent pane in visual order for split focus cycling.
pub fn adjacent_visual_pane_id<K: Copy + PartialEq>(
    ordered: &[K],
    current: K,
    previous: bool,
    wrap: bool,
) -> Option<K> {
    if ordered.len() <= 1 {
        return None;
    }
    let current_pos = ordered.iter().position(|id| *id == current)?;
    if previous {
        if current_pos == 0 {
            wrap.then(|| ordered[ordered.len() - 1])
        } else {
            Some(ordered[current_pos - 1])
        }
    } else if current_pos + 1 >= ordered.len() {
        wrap.then(|| ordered[0])
    } else {
        Some(ordered[current_pos + 1])
    }
}
