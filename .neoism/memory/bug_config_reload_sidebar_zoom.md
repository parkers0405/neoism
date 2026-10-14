---
name: "Config reload closes sidebar and resets zoom - fixed"
description: "Desktop config reload discarded unified sidebar state and Ctrl +/- zoom; preserve live panels/host and only replace zoom when configured font size changes"
type: "bug"
scope: "project"
origin: "Config reload sidebar/zoom bug fix"
created: "2026-10-09"
updated: "2026-10-09"
---

## Cause
Desktop `Screen::update_config` rebuilt `Renderer` on every config/theme/Lua reload but only transferred file_tree and tab chrome. It discarded notes_sidebar, conversations_visible/conversations_pane, and left_sidebar_host, losing the unified active view, width, focus, and cached content. Sidebar placement reconciliation ran on the old renderer before replacement. Reload also unconditionally restored configured font size in the renderer, persistent rich-text default, and existing contexts, erasing Ctrl +/- live zoom.

## Fix
`desktop/src/host/state.rs`: `Renderer::preserve_left_sidebar_from` moves all sidebar models and the live host into the rebuilt renderer, then reapplies the new configured placements and reconciles visibility. `set_plugin_snapshot` only applies sidebar visibility defaults when their resolved value changes, so unchanged Lua styles do not override manual session toggles. `desktop/src/screen/chrome_geom.rs`: seed the rebuilt renderer with previous plugin defaults; resolve font size from old/new config baselines, preserving canonical window zoom if the baseline is unchanged. Keep Sugarloaf style font_size as Ctrl+0 baseline while using resolved live size for renderer zoom/chrome scale, persistent defaults, and all contexts. Conversation disable cleanup now runs after rebuilding; Screen plugin publication also respects conversations_panel_enabled.

## Verification
Added regression tests for each sidebar's visibility/focus/width, closed panels, actual placement changes, unchanged and changed Lua visibility defaults, preserved zoom and explicit font-size edits. `cargo check -p neoism --lib --tests` passed with existing warnings; GUI/runtime tests not executed. No release build.
