---
name: "Bash git diff second-click collapse"
description: "Expanded Bash git diff dropped parent header target by mistakenly using live Edit-card behavior. Parent target retained; shared+desktop regressions passed."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

# Bash git diff could not collapse on the second click

Ordinary Bash tools whose output contains diff sections lost their parent header hit target once expanded. Shared view/tool_message/render.rs registered parent headers only when no diff sections existed or the tool was archived, incorrectly applying live Edit per-file behavior to all tools with diff output. Toggle state was correct in both desktop and shared panes; the second click had no parent target.

Fix: retain parent header hit rectangles for ordinary tools with diff output, including prepared sections. Only live edit-tool cards substitute per-file toggles for the parent. Styling, motion/reversal continuity, ordinary expansion anchor ownership and per-file independent toggles remain unchanged.

Regression: shared renderer decision + shared toggle lifecycle 2 tests passed using bash_diff filter; desktop bin toggle lifecycle 1 passed. The desktop library filter matches zero, so bin test is required. Native/shared and wasm32/web checks plus git diff whitespace check passed with concurrent scroll-disclosure fix. Files: shared view/tool_message/render.rs (production+decision test), shared state/hit_rects.rs and desktop pane/render_state.rs (tests only).
