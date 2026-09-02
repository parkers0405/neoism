---
name: "Agent tool card click-through tab chrome"
description: "Chrome gestures now exclusively own press/release and tool hit rects match visible area"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-08-30"
---

Fixed desktop/web agent tool-card click-through under tab chrome locally. Root cause: tab activation occurs on pointer-down, but sub-threshold Island/buffer-tab drag release fell through to newly active agent pane; desktop handle_neoism_agent_mouse_release then independently toggle_tool_at release coords. Fix: desktop Mouse has chrome_gesture_owned bit set for Island/top-bar/buffer chrome consumed presses; every armed Island/buffer gesture and every other chrome-owned press consumes/clears release, including cross-window source releases. Web beginBufferTabDrag now returns ownership, forwards press to shared Chrome then returns before agent dispatch; endBufferTabDrag consumes plain-click and legacy non-drag releases. Defensive layer: tool card, child, diff-card, link, and diff-scroll semantic rects are intersected with visible message/viewport clip before registration. Verified cargo check neoism-ui/neoism/wasm web, 29 shared tool tests, npm typecheck, all 71 web tests, LSP clean for mouse/render, git diff --check. Uncommitted/unreleased.
