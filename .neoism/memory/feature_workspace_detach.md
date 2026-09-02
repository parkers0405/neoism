---
name: feature-workspace-detach
description: Drag a top-level workspace (Island tab) out into its own OS window. Implemented in-process (not via daemon) because desktop PTYs are local; live session preserved via Msg::RebindWindow.
metadata: 
  node_type: memory
  type: project
  originSessionId: 1b6ea9d8-d40d-43fb-bd7b-362511dac63d
---

Drag-to-detach a top-level workspace (an "Island" tab, created by
Ctrl+Shift+W) into its own OS window, keeping the live shell session.

**Key architecture fact:** desktop terminals are LOCAL in-process PTYs
(`neoism_terminal_pty::PtySession` per context). The workspace daemon
(`neoism-workspace-daemon`, `SessionRegistry`) is NOT wired to host
desktop terminals yet — `daemon_client/mod.rs` says "Desktop state is
not rewired to consume this yet" (the G4–G7 work). So:
- **Local detach = in-process** (one app, many winit windows via the
  router). Done as of 2026-06-03.
- **Cross-laptop over Tailscale** (the user's eventual goal) needs the
  desktop-PTY→daemon rewire first; the daemon + tailnet/pairing layer
  already exists. See [[desktop-vs-web-paths]].

**How local detach works (the moving parts):**
- The Island detach *gesture* was already half-built —
  `Screen::handle_island_drag_release` / `IslandDragRelease::Detach`
  in `desktop/src/screen/lifecycle.rs`; only the handoff was a stub.
- Re-home primitive: `Msg::RebindWindow(WindowId)` (neoism-backend
  `event/mod.rs`); the `Machine` IO thread applies it to
  `self.window_id` (`performer/mod.rs`) — lock-free, sole writer.
- `ContextManager::take_workspace(index, sugarloaf)` lifts the owned
  `ContextGrid` out (de-registers rich text); `adopt_workspace(grid,
  sugarloaf)` re-homes PTYs (`grid.rebind_window`), re-registers rich
  text (`sugarloaf.text(Some(id))`), appends + focuses it.
- App loop (`Application::finish_pending_workspace_detaches`, called
  after `handle_mouse_input`) spawns the new window via
  `router.create_window` and adopts the parked grid.

**Known follow-ups (not done):**
- DONE: throwaway default tab is now dropped — `adopt_workspace(..,
  discard_existing_default=true)` closes the fresh window's default
  shell so only the detached workspace remains.
- DONE: right-click menu on the workspace (Island) strip —
  `Screen::handle_workspace_tab_context_click` →
  `open_workspace_tab_context_menu` with "Detach to New Window" +
  "Close Workspace". New shared `ContextMenuAction::Workspace(
  WorkspaceContextAction::{Detach,Close})` (context_menu.rs),
  dispatched in `execute_context_menu_action`.
- REVERTED — editor (nvim) pane re-homing on detach. The attempt made
  `BridgeHandler.window_id` an `Arc<Mutex<WindowId>>` locked on every
  redraw, which **froze nvim** (user-reported regression). Reverted to
  the plain `window_id: WindowId`. `Context::rebind_window` now only
  rebinds the terminal PTY (messenger). Editor panes in a detached/moved
  workspace keep routing to the original window until re-attempted with
  a safer design (NOT a Mutex on the redraw hot path) + runtime testing.
- DONE: per-workspace chrome state travels with the detached window —
  `Screen::detach_workspace_at` bundles the grid + `workspace_roots` /
  `workspace_buffer_tabs` / `workspace_buf_enter_targets` /
  `workspace_editor_active_paths` entries (keyed by the grid's stable
  `workspace_route_id`) into `DetachedWorkspace`; `adopt_detached_
  workspace` re-seeds them so the new window restores the right
  buffer-tab strip + file-tree root.
- DONE (non-terminal): right-click a *buffer* tab → "Move to {workspace}"
  for nvim/markdown/file tabs (path-backed), via close-here +
  `select_top_level_workspace_at` + `open_path_in_editor` (routes
  markdown automatically). `WorkspaceContextAction::MoveBufferTab
  { tab_index, target_workspace }`; `Screen::handle_buffer_tab_context_
  click` (hit-tests the buffer-tab strip) → `open_buffer_tab_context_
  menu` (flattened per-workspace items). Wired into mouse.rs right-click
  below the workspace-strip check.
- DONE (live terminal): a non-root terminal buffer tab now moves
  between workspaces with its PTY intact. `ContextGrid::take_node` /
  `take_context_by_route` (non-destroying sibling of `remove_node` —
  extracts the `Context`, keeps the session + rich text registered);
  `ContextManager::{take_current_grid_context_by_route,
  add_stacked_context_to_current}`; `Screen::move_buffer_tab_to_workspace`
  extracts → drops source strip entry → `select_top_level_workspace_at`
  → `add_stacked_context` + `buffer_tabs.open_terminal(route_id)` in the
  target. Root terminal stays non-movable. Context-menu widget is a flat
  list (no submenus), so the workspace picker is flattened to N items.
- DONE (cross-window): a buffer tab can now move into a workspace that
  lives in a DIFFERENT OS window (e.g. a detached one). App is the
  registry: `Router::cross_window_workspaces(exclude)` lists all
  windows' workspaces; mouse.rs gathers them (before borrowing the
  route) and passes to `open_buffer_tab_context_menu` →
  `MoveBufferTabToWindow { tab_index, target_window(u64), target_workspace }`.
  Screen parks it; `Application::finish_pending_cross_window_tab_moves`
  → `Router::move_buffer_tab_across_windows` extracts via
  `extract_buffer_tab_for_cross_window` (source) and adopts via
  `accept_cross_window_tab` (target) — re-registers rich text in the
  target sugarloaf + rebinds window id (`add_stacked_context_to_current`
  now rebinds to its own window). Minor known leak: source sugarloaf
  keeps the moved terminal's now-unused rich-text object.
- The APP (not the daemon) is the cross-window workspace registry. The
  daemon does NOT own desktop terminals (local PTYs), so daemon-mediated
  / cross-laptop moves remain the separate migration (#6 / G4-G7).
- REVERTED (chrome layout reorder): the full-width-top-bar-above-tabs
  attempt made the bar + tabs paint OVER the file tree (user wanted the
  chrome "pushed in" like before). Reverted to the original: workspace
  (Island) tabs at top full-width (y=0), hamburger top bar scoped to the
  content column below them (`bar_top = island_chrome_top`), tree/git
  panel anchored at `island_chrome_top` (not pushed down). The
  `Island::top_offset` field + render offsets REMAIN but `set_top_offset`
  is called with 0.0, so they're inert (kept for a future, correct
  re-attempt). All island hit-tests reverted to the y=0 gates.
- DONE (workspace tabs inset): the Island now insets horizontally to the
  content column (right of the file tree / sidebars), aligned with the
  chrome top bar, instead of spanning full width over the tree. New
  `Island::set_left_offset` (`left_offset` field; render bg + tabs +
  available_width + x_position + progress bar offset by it). `run.rs`
  sets it to `file_tree.width() (+ notes_sidebar.width())` when visible.
  All 4 island hit-tests + `begin_drag` add `self.chrome_x_offset()` to
  their x-math. Top bar stays content-column at `island_chrome_top` y.
  Color picker + dragged-tab ghost x not yet offset (rare/transient).
- DONE (agent side panel insets tabs): `Renderer::right_chrome_edge`
  (host/tabs.rs) now also subtracts the current context's neoism agent
  side-panel width when shown (`!side_panel().user_hidden()`), so the
  workspace tabs push in for it like the git panel / tree. (alt+h is the
  user's custom binding for the agent side panel.)
- DONE (workspace tabs match buffer-tab look): ISLAND_HEIGHT 34→28,
  TITLE_FONT_SIZE 12→11.5; strip sits on `theme.surface`; active tab is a
  rounded `theme.bg` card (`sugarloaf.rounded_rect` radius ~6, top
  corners + squared bottom) instead of surface-fill + accent underline;
  inactive tabs blend into the strip. Mirrors BufferTabs (height 28, font
  11.5, surface strip, rounded bg active card). Test `test_island_
  constants` updated.
- DONE (workspace tab icon + label): each Island tab now draws the
  tree's workspace-root folder glyph (`FOLDER_CLOSED_ICON`) in neoism
  blue (`NEOISM_FOLDER_ICON_COLOR` [34,84,145]) before the label — new
  `file_tree::icons::workspace_tab_icon()`; island renders icon + 6px gap
  + title centered as a group. Label (`Island::get_title_for_tab`) now
  basenames path-like titles (full cwd → final component) so tabs read
  "neoism" not "/home/.../neoism"; non-path titles unchanged; still "~"
  when no title/program. Tab label source = `title.content` template
  (OSC title/cwd) → program → "~".
- DONE (stable workspace tab label): the tab title was unstable because
  `ContextManager::update_titles` set it from `grid.current()` (the
  ACTIVE pane) — terminal cwd vs nvim cwd flips it to "~". Now the TAB
  title uses `ContextGrid::root_context()` (new accessor: grid's `root`
  node context) so it's stable across inner-tab switches; the OS window
  title (`RioEvent::Title`) still follows `current()`.
- DONE (island zooms with Ctrl+/- + text matches buffer): the Island
  was NOT in the `chrome_scale` propagation. Added `Island::scale` +
  `set_scale`, and added `island.set_scale(clamped)` to
  `Renderer::set_chrome_scale` (state.rs) alongside file_tree/buffer_tabs.
  Island render now multiplies ALL geometry by `s = self.scale`: `h =
  ISLAND_HEIGHT*s`, font `TITLE_FONT_SIZE*s`, padding/gap/radius/cursor.
  `effective_height`/`height` also `* self.scale` so host layout +
  hit-tests follow. KEY FIX: the font must scale by `chrome_scale`, NOT
  the device `scale_factor` — sugarloaf applies the device scale on top
  (sugarloaf.rs:788 + rect/text auto-scale x/y by device). My earlier
  `* scale_factor` double-scaled → blew up the folder icon so it
  overlapped the first letter; switching to `* s` fixed BOTH the size
  mismatch and the icon overlap. `fit_title_to_width` now takes the
  scaled font_size.
- DONE (buffer-tab icon spacing, pre-existing): file/terminal icons used
  raw `measure(icon_glyph)` which is too tight for nerd glyphs (advance <
  visual, e.g. python). Now `icon_w = measure().max(icon_size)` for all
  non-agent icons (buffer_tabs.rs) so the label never crowds the icon.
- DONE (caret stays on workspace strip after switch): the real path is
  the island-focused key handler in panes.rs (NOT select_top_level_
  workspace, which is Alt+digit). Alt+←/→ on the focused Island moves
  the CARET only (no switch — `move_focus_cursor`); ENTER commits the
  switch (`select_top_level_workspace_at(cursor)`) and KEEPS the caret on
  the Island (`island.set_focused(true, target, ..)` + `buffer_tabs.
  set_focused(false)`, was: `clear_island_strip_focus()` + drop to buffer
  tabs). Escape/ArrowDown still exit into the workspace. MOUSE CLICK left unchanged — it
  intentionally clears island focus (`switch_to` arm in lifecycle.rs).
  Also `Screen::close_split_or_tab` now closes the focused workspace when
  `is_island_strip_focused()` (Cmd+W on the Island strip).
- DONE (drag ghost matches new geometry): `Island::render_dragged_tab`
  was drawing at old y=0 / un-inset / un-zoomed coords. Now uses
  `top_offset` (y), `left_offset` (strip_left x), and zoomed `h` for the
  source-slot dim, floating ghost, shadow, accent strip + detach border.
- island tab icon/text polish: ICON_GAP 9px; icon uses lighter
  `FOLDER_ICON_COLOR` ([126,186,228]); label font is `TITLE_FONT_SIZE *
  scale_factor` (PHYSICAL px — sugarloaf text expects pre-scaled font,
  same as buffer tabs; measured widths divided by scale to keep the
  logical-space centering). sugarloaf gotcha: `rect`/`text.draw` scale
  x/y/w/h by scale_factor internally, but `font_size` must be passed
  pre-multiplied by scale_factor (see sugarloaf.rs:788).
- DONE (final layout = VS-Code-style): side panels claim the top
  (`tree_top = 0`, `panel_top = 0` in run.rs; notes sidebar shares
  `tree_top`) → full-height left/right columns. Content column:
  hamburger chrome bar at y=0 (`tabs.rs bar_top = 0`, content_x/content_w)
  → workspace tabs below + inset (`Island::set_top_offset(top_bar_height)`
  + `set_left_offset(file_tree+notes width)`) → buffer tabs → terminal.
  All 4 island hit-tests carry BOTH offsets: `top_offset_px`
  (top_bar_strip_height) on the y-gate + `chrome_x_offset()` on the
  x-math; `update_drag` gets strip_top=top_bar_height; `begin_drag` gets
  left_margin+left_offset. Unverified visually. Color picker + dragged
  ghost x/y not offset (rare/transient) — refine if they look off.
- Detached-window + live-move *rendering* unverified at runtime (compiles
  + no test regressions only) — needs a dev-loop check.
