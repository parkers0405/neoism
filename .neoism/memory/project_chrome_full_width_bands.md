---
name: project_chrome_full_width_bands
description: Chrome layout model — full-width top bar+workspace strip and bottom status bar; tree/notes/git confined to the middle band; web vs desktop are SEPARATE geometry paths
metadata: 
  node_type: memory
  type: project
  originSessionId: c7c46f71-1f34-436c-8216-fb724e89feae
---

Current chrome layout (landed 2026-06-15): the **top bar + workspace
island strip** span the full window width pinned to the top, and the
**status bar** spans full width pinned to the bottom. **Buffer tabs +
breadcrumbs stay in the content column** (pushed inward by the tree).
The side panels — file tree (Alt+E), notes (Alt+N), git diff (Alt+G) —
are confined to the middle "band" between the top chrome and the status
bar (they no longer run full window height). On web the agent (Alt+H) is
a buffer tab, so it already lives in the content column below the top
chrome.

**Web and desktop are SEPARATE layout paths** — see [[feedback_desktop_vs_web_paths]]:
- **Web**: `Chrome::set_layout` + `Chrome::draw` in `neoism-frontend/shared/src/chrome.rs`. Only the wasm path calls these (desktop never calls `set_layout`). Band top = `buffer_tabs.y`; band bottom = `status_line.y`. Workspace strip drawn full-width from `wasm/src/lib.rs` (`set_left_offset(viewport.x)` + the strip bg rect).
- **Desktop**: its own geometry across `host/run.rs` (renders tree/notes/island/tabs/status/git), `screen/render/mod.rs` (TOP BAR LAST PASS), `screen/chrome_geom.rs`, and `screen/bridges/*`. Band top = `chrome_top(num_tabs)` = island_height + top_bar_strip; band bottom = `logical_height - status_line.scaled_height()`.

**Desktop hit-test gotcha (the crux):** several click handlers RECOMPUTE
geometry independently of the render pass and historically drifted (tree
render used `y=0` while clicks used `rio_island_height()`). Centralized
via `Screen::side_panel_band() -> (top, bottom)` in `chrome_geom.rs`;
file_tree (`file_tree_bounds`, scroll, `file_tree_row_under_mouse`) and
notes (`notes_sidebar_bounds`) all read it. The island left edge is now
`0.0` (full width) in render (`run.rs`), the 4 island hit sites
(`lifecycle.rs`), and `point_in_island_strip`. Widgets that store their
last-render geometry — `top_bar` (`pointer_down`), `status_line`,
`git_diff_panel`, `notes_sidebar` (`hit_test`) — auto-follow render, so
only file_tree + island needed manual hit-test edits.

Decision: dropped the desktop status-line composer width-snap (status is
full width even with the terminal composer visible) for web parity; the
composer floats above it via `render_status_join`. Related:
[[project_shared_side_panels]], [[feedback_chrome_panel_style]].

Follow-ups (same day):
- **Workspace/Island strip is full width.** Desktop `host/tabs.rs::right_chrome_edge` now just returns `logical_width` (despite the name) — the strip lives in the top chrome above the band, so Alt+G/Alt+H don't push it. The git/agent right-inset moved into `content_right_edge` (buffer tabs / breadcrumbs band still gets pushed). On web the island already used `viewport.w`.
- **Alt+H agent side panel** top aligns to the band top: desktop `screen/bridges/agent.rs` `sidebar_top = side_panel_band().0` (was `rio_island_height()`, one top-bar-row too high); web passes `panel_top_override = Some(layout.buffer_tabs.y)` (was `0.0`).
- **Top-bar toggle buttons show open state** via a two-tone glyph (no background wash): `chrome_topbar.rs::draw_icon_button` repaints one half of the split-pane codicon in `theme.accent` using `DrawOpts.clip_rect` — left half for the tree button, right half for the agent button. Hosts push `set_panel_open` / `set_right_panel_open` each frame.
