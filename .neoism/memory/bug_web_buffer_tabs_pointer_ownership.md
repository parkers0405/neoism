---
name: "Web BufferTabs pointer ownership"
description: "Tab-strip gestures are exclusively chrome-owned through up/cancel so activation cannot click newly active content"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-09-01"
updated: "2026-09-01"
---

---
name: "Web BufferTabs pointer ownership"
description: "Tab-strip gestures are exclusively chrome-owned through up/cancel so activation cannot click newly active content"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-09-01"
updated: "2026-09-01"
---

Root cause: web routed each pointer phase against the currently active surface. BufferTabs activates on pointerdown, while TS applies the activation during the next render-intent drain; sub-threshold move/pointerup could therefore reach the newly active code/Markdown/terminal/agent surface. Ownership was also inferred only from draggable tab bodies, leaving close/new/background chrome unowned, and split pane-tab down was consumed without retaining ownership through release.

Fix: `TabGestureOwnership` records workspace/pane strip ownership by pointer/touch id from down until up/cancel. TerminalPanel checks workspace strip geometry before content routing, consumes all owned move/up phases, retains shared drag/reorder/tear-out behavior, restores canonical order on cancel, and claims split pane-tab interactions. Touches beginning in BufferTabs use the dedicated horizontal pan/tap path on all touch layouts; touchcancel never synthesizes a click and touchend prevents compatibility mouse fall-through. Existing focusSurface behavior is retained (no new focus suppression).

Regression: `tabGestureOwnership.test.mts` uses coordinates that are a tab before activation and a code-line coordinate afterward; pointer down/up and touch simulated click assert editor handlers do not run and caret/selection remain unchanged.

Checks: web `npm run typecheck`, all 105 tests; `cargo check -p neoism-ui`; `cargo check -p neoism-terminal-wasm --features web --target wasm32-unknown-unknown`; diff check. No release build.
