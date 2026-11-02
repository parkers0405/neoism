---
name: "GPUiX /sessions empty and missing pagination fixed"
description: "GPUiX slash sessions now shares sidebar catalog capability, pinned/recent sections and lazy 50-row pagination"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-11-02"
updated: "2026-11-02"
---

GPUiX `/sessions` and `/session` with no argument were using `scope.sdk.sessions.list({directory:scope.directory})`, which bypassed the sidebar's daemon-granted master catalog and could show no legacy/cross-project chats. Fixed in `neoism-frontend/gpuix/src/agent/SlashController.ts` via `sessionsPicker`: same `AgentSdk.catalogProjects` pinned summary list and `catalogSessions({roots:true,limit:50})` capability-aware reader as the sidebar; Pinned then Recent sorted by updated time, current/pin marks, root sessions only, selection fetch routes through `forSession(workspaceId, selectedId)`. Existing InlinePicker remains anchored above composer. Continuation callback single-flights and dedupes by ID; appended options preserve shared Rust picker scroll/selection/query. `InlinePicker` requests pages only on downward wheel/keyboard navigation near loaded tail, using max(animated top, committed selection) to handle spring lag; never requests during paint/frame/appends, adds no polling or SSE, and suppresses cancelled/stale workspace results. Failure keeps rows/cursor for explicit retry. Tests: `bun test ./tests/slash-controller.test.ts ./tests/sdk.test.ts ./tests/pickers.test.ts ./tests/agent-sidepanel-catalog.test.ts ./tests/agent-sidebar-actions.test.ts` (44 passing) and `bun run typecheck` from gpuix. Only TS/TSX edited; no native rebuild needed.
