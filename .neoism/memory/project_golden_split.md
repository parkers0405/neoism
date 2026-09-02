---
name: project_golden_split
description: Golden-standard shared split/pane system — geometry solver + PaneGrid controller in neoism-ui
metadata: 
  node_type: memory
  type: project
  originSessionId: 7189e8ae-873c-46b8-b9d5-986f841206b3
---

Goal (started 2026-06-19): real Zed/VS-Code-style splits for ALL tab types
(md, nvim editor, agent, terminal, draw), built in the shared `neoism-ui`
layer as a discoverable chrome piece. Replaces the neglected desktop-only
split (Ctrl+Shift+R/D spawns a fresh terminal regardless of source tab; NO
drag-to-split ever existed — only divider resize). Note: "Alt+Shift+T" was
never a split binding.

**Canonical model already existed**: `shared/src/session_layout/tree.rs`
`SessionTree` (n-ary, recursive, fully op'd + tested: split_focused,
close_focused, set_ratio, resize_event, move_tab, detach_leaf,
insert_leaf_as_tab_sibling, wrap_leaf_in_tabbed, focus_next_visual). Ratios
are CUMULATIVE shares (ratios[i]=cumulative up to child i; last=remainder);
`ratios_to_shares` is the canonical conversion (mirrored desktop
`layout/grid/rebuild.rs`).

**Landed (all tested, cargo check/test green on neoism-ui + neoism):**
- `session_layout/geometry.rs` — THE single geometry brain: `solve(tree,
  content, gap, divider_tol) -> SolvedLayout {panes, dividers}`, plus
  `pane_at`, `divider_at`, `drop_zone_at` (edge_frac → Left/Right/Top/
  Bottom/Center drop zones for drag-to-split). 6 tests.
- `panels/pane_grid.rs` — `PaneGrid` controller wrapping `SessionTree`:
  set_content/solved/panes, split_focused, close_focused, focus_visual,
  focus_at, divider drag (begin/update), surface drag-to-split
  (begin/update/drop), emits host-agnostic `PaneGridAction`
  {OpenPane, ClosePane, FocusPane, Relayout} drained via take_actions().
  Host handle = per-leaf `external_id` (route/pane id). 6 tests.
- `tree.rs`: added `leaf_mut()`.
- `chrome.rs`: `pub pane_grid: PaneGrid` field (constructed Terminal/ext 0)
  — discoverable like git_diff_panel/notes_sidebar. NOT a PanelKey (it's
  the content substrate, not a focus-stack overlay).
- desktop `layout/grid/dragsplit.rs`: `ContextGrid::pane_drop_zone_at(x,y)`
  drives the SHARED `geometry::solve` over the grid's canonical
  SessionTree → `PaneDropTarget{node, placement, highlight}`. Desktop's
  first real adoption of the shared brain. #[allow(dead_code)] until the
  drag-input pass wires it. Leaf-kind fix (draw/extensions) landed in BOTH
  manager.rs:4839 + grid/mod.rs:698.

**Decision (user, 2026-06-19):** desktop adopts PaneGrid as canonical +
remove legacy to be clean; web waits (don't break it; legacy.rs
SessionLayout still feeds web via from_pane_layout_snapshot).
KEY RISK: full Taffy removal = re-key the 4300-line ContextGrid off Taffy
NodeId onto SessionTreeLeafId (NodeId is the storage key everywhere). No
geometry test net + no runtime validation in agent env → must be done in
verified increments, runtime-tested by user. Geometry-engine swap alone
has zero user-visible benefit (Taffy geometry already correct); the wins
come from drag-to-split + any-type panes via the controller.

**Cleanup landed (desktop legacy mirror removed):** stripped dead legacy
`SessionLayout` validation/mirror from desktop — borders.rs
`session_layout_resize_border_ratio`+`first_panel_route_in_subtree` (was
trace-only); manager.rs `current_grid_session_layout` (no callers),
`session_split_intent_route_set`, `session_layout_route_set`,
`debug_assert_session_split_mirrored` + unused imports. NOTE: legacy
`SessionLayout` is STILL functionally load-bearing on desktop via
`session_layout_for_grid` + secondary-route policies
(`session_layout_first_secondary_route`/`_secondary_routes`/
`_close_current_grid_route`) and `session_leaf_route` (manager.rs:2936) —
can't delete legacy.rs until those move to SessionTree too.
Also: shared `geometry::solve_with(tree, content, &SolveOpts{gap_x,gap_y,
margin,divider_tol})` added (per-axis gap + per-panel margin, single-pane
fills content) = desktop-parity geometry; `solve()` is the simple wrapper.

**FLIP LANDED (2026-06-19, cargo-check green, NEEDS USER RUNTIME TEST):**
SessionTree is now the geometry/borders/resize AUTHORITY on desktop; Taffy
no longer computes geometry, draws borders, or resizes.
- layout.rs: `compute_layout`→`recompute_rects_from_tree` uses
  `pane_geometry::solve_with(session_tree, content, SolveOpts{gap_x/y from
  column/row_gap*scale, margin from panel margin*scale})`; writes
  layout_rect via leaf_to_node; `propagate_stacked_rects` fans the active
  member's slot rect across each stacked group; `apply_hidden_split_layout_rect`
  now uses content rect not Taffy. Deleted update_layout_rects +
  sync_stacked_layout_rects.
- borders.rs: REWRITTEN solver-based — find_border_at_position/get_panel_borders
  from solved dividers; resize_border takes new_ratio→`tree.set_ratio`;
  move_divider_*→`tree.resize_event(Some(axis),±amount/extent)`. Deleted
  walk_separators + get_panel_size.
- border.rs PanelBorder: now {direction, anchor_leaf:SessionTreeLeafId,
  node_extent, start_ratio} (Copy, no NodeId). resize.rs ResizeState
  dropped original_sizes. mouse.rs: new_ratio = start_ratio + delta/node_extent.
- neighbors.rs now dead (#[allow(dead_code)]); splits.rs set_panel_size dead.
RUNTIME-TEST: split render geometry, divider drag, Ctrl+Shift+Alt+arrows
move-divider, border draw positions, single-pane full, STACKED TABS
positioning (highest risk), nested-split resize anchor/gap resolution.

**Runtime feedback round 1 (user tested the flip — "generally correct"):**
Fixed: deleted neighbors.rs (dead); set_panel_size #[allow(dead_code)];
hover-scroll (#4) — scroll.rs routes wheel to pane under cursor via
find_context_at_position + set_current_node_without_layout before
screen.scroll; close-last-tab collapses split pane (#7) — pane_tab_close
terminal branch now calls new `collapse_empty_split_pane(route_id)` helper
(guarded by node_by_route_id, safe vs double-remove), shared with the
non-terminal branch.
STILL OPEN (render/visibility, need runtime iteration — likely flip
regressions in stacked-tab + editor paths):
- #2 drag a tab into a pane → pane blank until you click tab/back (stacked
  active-member visibility after merge; rich_text visibility not set).
- #6 dragging divider between two nvim panes → right pane OVERLAYS instead
  of left shrinking (editor reflow/clip on resize_border; geometry recompute
  fine, render path suspect).
- #3 "weird line through right split" — probably the editor scrollbar
  (run.rs push_panel_state for editor panes), confirm not a stray border.
- #5 open a file while focus on right split → should open in that split.

**Runtime feedback round 2 — FLIP REGRESSIONS FIXED (root-caused):**
- nvim split OVERLAY (terminals fine): editor panes carry a file-tab strip
  → they are `Tabbed` groups; terminals are plain leaves. layout.rs
  `propagate_stacked_rects` copied the *assumed-active* member's rect
  (stale full-width) instead of the member `solve` actually wrote. Fixed:
  recompute_rects_from_tree now tracks `written` nodes; propagate copies
  the written/visible member's rect. THIS is why terminals worked and nvim
  didn't.
- close last tab on split → pane stayed: splits.rs `remove_node`/`take_node`
  called `sync_session_tree()` AFTER `apply_taffy_layout` — geometry reads
  the SessionTree, so layout used the stale tree (closed pane still
  present) and the survivor kept half width. Fixed: sync BEFORE layout.
- split overlay round 1: split_right/down set `self.current=new` BEFORE
  apply_taffy_layout; editor.resize now unconditional (not gated by
  `visible`) so a mid-split pane reflows.
LESSON: any Taffy-first mutation that calls apply_taffy_layout MUST
sync_session_tree first (geometry is SessionTree-sourced now).

**Runtime feedback round 3 fixes:**
- close wrong pane / split space kept: `collapse_empty_split_pane`
  (panes.rs) now calls new `ContextManager::remove_grid_route(route, sl)`
  which removes the exact node by route via grid.remove_node — bypasses
  the fragile legacy `session_layout_close_current_grid_route` (focus-based)
  that closed the FOCUSED pane not the clicked one.
- "+" on secondary pane: handle_buffer_tabs_click now handles `pane_hit ==
  TabHit::NewTab` → `create_pane_terminal_tab(route)` →
  `ContextManager::add_stacked_terminal_on_route` (stacks terminal on that
  pane via grid.add_stacked_context_on_parent). Workspace "+" unchanged.
- #1 split opens NEXT FILE (editor) not terminal: panes.rs `split_right`/
  `split_down` call `split_next_file(down)` → `next_split_file` (next
  NodeKind::File in file_tree.nodes() after current().editor_path, wraps)
  → `ContextManager::split_editor`; falls back to terminal split if no
  files. #2 ctrl+alt+arrows → MoveDivider* added in bindings/platform/
  linux.rs (focused-pane resize via resize_event). User confirmed: split=
  next file; ctrl+alt = resize divider from focused pane.

**Runtime feedback round 4 — REVERTED a too-aggressive change:**
- split_next_file (#1 split-opens-file) REVERTED: it made EVERY split open
  an editor, which broke terminal splits ("terminal in split fucked").
  split_right/down are back to terminal split (known-good). split_editor
  still exists for tab tear-out.
- REAL fix kept: splits.rs split_panel now captures `restore_visible_leaf`
  (the visible stacked tab) before the focus-retarget-to-host, and restores
  that tab's active_stacked after splice — so splitting a stacked pane no
  longer reverts the source pane to the host terminal while its tab/
  breadcrumb still show the file (the tab/content desync).
- UNRESOLVED (NOT touched by my code — pane geometry only): "click a file
  in tree → tree goes invisible." My changes never touch file_tree
  visibility/width; suspect pre-existing or a separate render issue. Needs
  confirmation if it repros in a single-pane workspace (no splits).
- ctrl+alt+arrows divider keybinds kept (harmless).

**Runtime feedback round 5 — sub-agent research (no logs) + fixes:**
- EMPTY NVIM on closing a terminal tab in a split = MY double-remove bug.
  pane_tab_close TERMINAL branch already removes the pane via
  should_close_context_manager (RouteExitPlan::RemoveRoute -> remove_node);
  the collapse_empty_split_pane I added did a 2nd removal keyed by the strip
  route (which differs from terminal_route for a terminal-first pane that
  gained a stacked editor), tearing out the editor peer. FIX: removed that
  call from the terminal branch (kept it on the file branch), now just
  select_route_from_current_grid. (panes.rs pane_tab_close)
- split desync fix completed: split_panel now also `session_tree.focus_leaf
  (restore_visible_leaf)` so SessionTree Tabbed.active matches the restored
  active_stacked (3 "active tab" notions: Tabbed.active / active_stacked /
  pane_tabs strip — keep in sync). (splits.rs)
- TREE GOES INVISIBLE on file-open: TWO deep agents PROVED desktop code is
  consistent — editor pane provably cannot cover the tree (fixed-pass
  compositor, no depth test, tree draws unclipped at DEPTH 0/ORDER 6-9 on
  top in renderer.run which runs every frame; editor grid clipped to
  panel_left = scaled_margin.left which includes file_tree.width via
  chrome_x_offset; geometry single-pane override fires for stacked groups).
  NOT a layout/geometry/draw-order bug in the examined code. Needs ONE
  runtime observation to pin (likely first-post-open frame or pre-existing
  GPU/render). Highest-value inspect: chrome_geom.rs reapply_chrome_layout
  ~608-636 + layout.rs recompute_rects_from_tree first frame; trace editor
  panel_left/clip_rect on first frame after open.

**Runtime feedback round 6 — drag-drop split pane HIDDEN = structural bug (FIXED, root cause):**
A pane with a tab strip is a `Tabbed` group. `SessionTree::split_focused`
(tree.rs) wrapped the focused LEAF, so splitting a tab group produced
`Tabbed[Split[host,new], files]` — the new pane NESTED inside the host's
tab slot, only visible when that tab is active → appeared "hidden" (and the
earlier "split shows terminal" was the same nesting seen the other way).
FIX: split_focused now, when the focused leaf's parent is a `Tabbed`, wraps
the WHOLE Tabbed (`split_path = parent_path`) → `Split[Tabbed[...], new]`,
new pane is a visible SIBLING. Regression test
`split_focused_inside_tabbed_wraps_whole_group` (tree.rs). This made the
desktop `split_panel` host-retarget + restore_visible_leaf hacks
unnecessary — REMOVED them; split_panel now focuses the actual current leaf
and lets split_focused wrap the group. Applies to BOTH keyboard splits and
drag-drop tear-out (split_editor → grid.split_right/down → split_panel).
101 SessionTree tests + new test green.

**THE BIG REMAINING JOB (one cohesive rewrite, NOT micro-incrementable):**
Taffy is woven through geometry + borders (`get_panel_borders`/
`walk_separators` read Taffy `layout()` directly) + resize (writes Taffy
`flex_basis` via `set_panel_size`) + neighbors + the storage key
(`inner: FxHashMap<NodeId,...>`, `current/root: NodeId`). Deleting Taffy =
rewrite grid module to be SessionTreeLeafId-keyed: geometry from
`geometry::solve_with`, borders from solved `dividers`, resize via
`tree.set_ratio`/`resize_event`, neighbors via `visible_leaves`/visual
order; DELETE rebuild.rs, splice_rebuild, derive/sync_session_tree,
leaf_to_node/node_to_leaf, stacked_parents (→ Tabbed nodes). High-risk
blind (no geometry test net, no runtime in agent env) → do in a worktree
with USER runtime-testing each step.

**Still TODO (desktop, in priority order):**
1. Drag-input wiring: mouse grab a buffer tab/pane → drag → call
   `pane_drop_zone_at` each move for live overlay → on release drive
   `split_existing_*`/stack via `drop_placement_to_split_down`. Touches
   desktop event system (screen/mod.rs, selection.rs).
2. Draw the live drop-zone highlight overlay (chrome overlay, from
   `PaneDropTarget.highlight`).
3. (Optional) Decouple split-from-terminal in manager.rs::split (always
   create_context→shell today).
4. (Big, deferred) Re-key ContextGrid off Taffy NodeId → delete Taffy;
   then delete legacy.rs SessionLayout once web is moved off it.

Related: [[project_chrome_full_width_bands]] [[project_shared_side_panels]]
[[feedback_desktop_vs_web_paths]] [[project_workspace_root_model]]
Orphan shared files never wired: session_layout/{hit_test,taffy_bridge}.rs.
