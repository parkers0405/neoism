---
name: "Top-right server menu"
description: "Correct top-chrome placement for separate server control"
type: "feature"
scope: "project"
origin: "user correction to top chrome server placement"
created: "2026-05-08"
updated: "2026-05-08"
---

Correction on 2026-05-08: hamburger must remain in its original left position immediately after the file-tree toggle. Servers is NOT a hamburger dropdown item. Added a distinct far-right top-chrome server button modeled after OpenCode: custom-drawn three-tier rack glyph in a hoverable square plus small green status dot at upper-right. Clicking it emits OpenServers directly. Optional right agent-panel toggle sits immediately left of the server button. Shared geometry/hit-tests updated and desktop/wasm/web verified.
