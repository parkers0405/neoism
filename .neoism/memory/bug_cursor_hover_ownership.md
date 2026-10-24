---
name: "Desktop cursor ownership: chrome, agent tools and markdown/tree"
description: "Cursor ownership fixed across agent logos/tools, markdown/tree, clickable top chrome; shared hit tests and post-click hand restoration"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-15"
updated: "2026-10-15"
---

Desktop cursor routing is in neoism-frontend/desktop/src/app/window_event/mouse.rs. Agent wordmark hover was absent, causing Text fallback; native/shared NeoismAgentPane::wordmark_hit is reused by clicks and native Screen::neoism_agent_wordmark_hovered. Both hover branches and mouse-release cursor selection include it. Terminal splash already had Pointer in handle_splash_overlay_hover.

Expandable tools: native pane/render_state.rs and shared state/hit_rects.rs expose pure tool_hit using exact frame-local tool_hit_rects. Native Screen::neoism_agent_tool_hovered and all agent Pointer branches (changed hover, steady hover, release) include tool targets. Header/group-child/per-file targets are already registered only when actionable; do not broaden to entire bodies.

Markdown: shared editor/markdown/interactions.rs handle_hovered now takes x,y and checks actual handle_rect minus convert_rect plus scrollbar geometry. Previously hovered_line covered the entire prose block and made every paragraph Grab. Keep hovered_line broad for paragraph highlighting. Native screen/bridges/markdown/input.rs markdown_pane_contains_mouse uses current_grid.scaled_margin + current_item.layout_rect divided by DPI, matching render.rs. handle_markdown_hover clears stale state outside pane. mouse.rs only assigns Markdown cursor/returns inside pane; outside it falls through to tree/sidebar cursor routing. Links/actions Pointer, actual handles Grab, prose Text.

Top chrome: shared panels/chrome_topbar.rs pointer_hovered reports visible clickable icon/menu-row hover, not inert peer tooltips. Native screen/bridges/top_bar.rs handle_top_chrome_hover updates topbar, Island workspace tabs, and buffer/pane tabs together before returning (changed, clickable), clearing old strip hover on transitions. mouse.rs uses Pointer for all clickable tab bodies, closes, plus buttons, workspace tabs and topbar icons. Previously only buffer-tab close glyph was Pointer; activation/NewTab/workspaces were Default and icons had no cursor hook. All three local chrome-gesture release paths (Island, buffer tabs, general chrome) refresh the same predicate to retain Pointer after mouse-up. Active drag routing still precedes hover.

Tests: shared tool_hover_uses_only_current_click_targets_without_expanding, markdown prose_hover_does_not_claim_the_drag_handle_cursor, wordmark bounds/NaN tests, chrome_topbar pointer_hover_tracks_only_visible_clickable_controls. Verified desktop/shared cargo check; wasm check; shared hover25, chrome_topbar14, buffer_tabs88, wordmark12 and read-only Markdown1 tests pass. Native binary unit tests currently blocked by unrelated stale context/manager/test.rs:174-183 referencing removed apply_server_message and FullSnapshot.panes. Do not fix unrelated concurrent work. Earlier transient file_browser.rs missing constants resolved by owner.
