---
name: "Agent tool-card collapse hit target — FIXED"
description: "Agent tool cards now toggle reliably from selectable title/body text; diff filename links use exact visible geometry and todos have no inert toggle."
type: "bug"
scope: "project"
origin: "session"
created: "2026-08-31"
updated: "2026-08-31"
---

Agent tool cards were easy to expand but hard to collapse on desktop because selectable text wins pointer-down and `handle_neoism_agent_mouse_release` replayed links only, never `toggle_tool_at`; expanded body/title text swallowed plain clicks, leaving narrow whitespace as the only collapse target. Fix: add `selection_is_plain_click` to desktop/shared selection state and on release replay tool toggle only for a stationary text click after link priority; drag selections still copy and do not toggle. Diff cards also registered almost the full header as a file link; `diff_card::CardLayout.header_link_rect` now reports the actual rendered filename width plus 3px-scale padding, and tool rendering registers only that rect, leaving badges/blank header as toggle space. Todo outputs no longer register inert tool toggles. Files: desktop screen/bridges/agent.rs, desktop/shared selection.rs, shared widgets/diff_card.rs, tool_message/render.rs. Verified cargo check -p neoism-ui -p neoism, focused desktop test, rustfmt and diff checks.
