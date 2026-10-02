---
name: "Lua state polling FPS fixed"
description: "Expanded Lua platform regressed FPS by serializing full state at 10 Hz for style-only plugins; fixed by subscriber-gated polling."
type: "perf"
scope: "project"
origin: "agent"
created: "2026-09-01"
updated: "2026-09-01"
---

# Lua plugin FPS regression from unconditional state polling — FIXED

## Symptom
Enabling style-only Lua showcase packages reduced desktop rendering from roughly 144 FPS to about 70 FPS even though the packages registered no timers, state callbacks, jobs, or decorations.

## Root cause
`Application::pump_lua()` serialized and compared the complete editor/workspace/tab/panel/file-tree/notes/Agent/terminal/Git/plugin/config state every 100 ms whenever any Lua runtime was active. The expanded plugin platform made this snapshot substantially larger. Installed showcase packages were style-only and had no state autocmds, so the periodic work was entirely unnecessary but still ran on the application/frame path.

## Fix
Publish initial state once and continue forcing publication immediately before Lua commands and keys. Only arm the 100 ms periodic state snapshot when an active runtime actually subscribes to a state-change autocmd (`BufferChanged`, workspace/tab/panel/tree/notes/Agent/terminal/Git/theme/plugin/config/document/selection/pane events, or wildcard). Startup, command, async and other explicitly delivered events do not enable polling.

## Verification
`cargo check -p neoism` passes. Focused helper coverage verifies state events enable polling while Startup, Command, and AsyncResult do not.
