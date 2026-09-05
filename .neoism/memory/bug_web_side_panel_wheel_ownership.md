---
name: "Web side-panel wheel ownership — FIXED"
description: "Web wheel ownership for shared side panels and no boundary chaining"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-01"
updated: "2026-08-01"
---

Web canvas wheel routing now has a single pointer-position owner in `TerminalPanel.handleWheel`: center modal/overlay, then shared chrome Tree/Notes/Git or horizontal buffer tabs through `chromeWheelScrollAt`, then active Markdown/editor/agent content, then terminal. Shared `Chrome::wheel_scroll_at` returns ownership based on geometry even when bounded scrolling does not move, preventing boundary scroll chaining. Tree and Notes normalize DOM positive-down to their shared positive-up pixel accumulator and convert line mode with each panel's own row height. Agent side-panel remains position-aware in `agent_scroll_wheel_at`. Regression policy tests in `web/src/terminal/wheelOwnership.test.mts` assert tree/notes ownership and no background page/terminal counters, including boundary. Verified npm test (102), npm typecheck, cargo check neoism-ui, cargo check neoism-terminal-wasm host + wasm32, chrome event tests, fmt check.
