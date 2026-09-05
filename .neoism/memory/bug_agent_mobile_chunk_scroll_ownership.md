---
name: "Agent mobile chunk scrolling — FIXED"
description: "Fixed mobile Agent full-finger scrolling with sticky gesture owner and diff-bound wheel bubbling"
type: "bug"
scope: "project"
origin: "coding-session"
created: "2026-08-01"
updated: "2026-08-01"
---

Implemented robust Agent mobile gesture ownership. New pure TS `AgentTouchScrollOwnership` anchors at touch-down, axis-locks once, retains picker/side-panel vs outer timeline vs horizontal markdown ownership through crossing rendered chunks and release momentum, and resets on bounds/cancel/end/surface loss/new touch. `TerminalPanel` wires this into shared touch momentum. WASM `agent_drag_owned_at` accepts sticky owner codes; vertical touches over diff cards route outer timeline and return 0 when no timeline pixels were consumed. Agent wheel paths now bubble a diff `Some(false)` boundary into outer timeline. Horizontal code/table scrolling remains axis-specific; explicit scrollbar paths untouched. Tests cover chunk crossing, momentum ownership, hard bounds, horizontal axis lock, and resets. Checks: web typecheck; full 114 web tests; wasm cargo check; changed Rust rustfmt check; git diff --check. Workspace-wide cargo fmt check remains blocked by unrelated concurrent unformatted files.
