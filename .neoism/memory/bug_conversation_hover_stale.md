---
name: "Conversation sidebar stale hover — FIXED"
description: "Side-panel hover now strictly follows current pointer presence and target"
type: "bug"
scope: "project"
origin: "session"
created: "2026-08-01"
updated: "2026-08-01"
---

2026-08-01: Conversation side-panel row hover could remain shaded after the pointer was no longer over that row, while the green active-session dot correctly moved. Root causes: hover-out used a fade that retained `hovered_session`, and desktop/shared hosts retained last pointer coordinates after `CursorLeft`/`PointerLeave`.

Fix: `NeoismAgentSidePanel::tick_pointer_animations(None, None)` immediately clears hovered identity/index and hover scale. Desktop `Mouse` tracks `inside_window`; cursor move sets it, cursor-left clears it and the renderer pointer, requests redraw, and the agent bridge only publishes pointer coordinates while inside. Shared `Chrome` similarly tracks `pointer_inside` before modal/file-browser early returns and passes `None` to side-panel rendering after `PointerLeave`. Added `side_panel_hover_clears_immediately_without_a_pointer_target` regression test.

Files: shared `panels/agent_pane/state/side_panel.rs`, `state/tests.rs`, `chrome.rs`, `chrome/config.rs`, `chrome/events.rs`, `chrome/draw.rs`; desktop `input/mouse/mod.rs`, `app/window_event/mouse.rs`, `screen/bridges/agent.rs`.

Verification: `cargo check -p neoism-ui -p neoism` passes; diff check passes. Focused lib test compilation is blocked by unrelated existing file-tree tests whose `PanelContext` literals lack the concurrently-added `plugins` field.
