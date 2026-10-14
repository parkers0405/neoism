---
name: "Splash shortcut grid experiment"
description: "Reversible two-column splash: workspace actions left, Notes/agent/terminal right; wider logo and smaller text"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

The current splash implementation is shared at `neoism-frontend/shared/src/panels/splash_overlay.rs` (desktop re-exports it). User requested a reversible two-column experiment with eight entries. Final requested column order: left = Change Directory, Open file tree, Search, Command palette; right = Notes, Neoism Agent, Agent panel, New Terminal. `MENU` stores these interleaved row-major. `MENU_COLUMNS = 2` controls the experiment; set it to `1` to restore stacked layout while retaining the new labels/action. Narrow panes automatically stack when fitting both measured columns would shrink below 75% of their initial scale. Drawing, hover, and cached click rectangles use `menu_button_rect` so both columns agree. The existing 16-row terminal reservation is unchanged. Follow-up sizing: wordmark pane-width cap increased from 42% to 60% (preserves aspect ratio and reserved height); menu label/key/icon base sizes reduced to 15/14/15px.

`OpenAgent` still opens the agent tab (`Alt+A`), now labelled `Neoism Agent`. New typed `OpenAgentPanel` / wire `open-agent-panel` is labelled `Agent panel`, displays `Alt+C`, and uses the existing conversations-sidebar action: desktop `screen/lifecycle/splash_island.rs` calls `toggle_conversations_sidebar`; WASM `rendered/terminal_input.rs` calls `chrome.toggle_conversations`. Do not confuse this with `ToggleAgentSidePanel`, which controls the separate agent-details panel. Mobile `SplashAction` includes the new wire action; it does not request a keyboard. Existing layout/hit-test Rust tests and mobile intent coverage were updated.
