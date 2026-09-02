---
name: "Background shutdown and MCP status reconciliation"
description: "Shutdown settles background jobs and MCP hot reload refreshes visible status"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-08-30"
---

Fixed two lifecycle gaps after v0.7.71. Background tasks: graceful workspace/server shutdown previously sent cancellation, immediately cleared jobs, and finish_background_job returned early when runtime.closed, permanently leaving durable `status: running` cards unmatched; Stop after restart returned 404. Background runtime now tracks active job waiters, shutdown cancels then drains them (10s bound) through deterministic completion publication before clearing, and completion is never suppressed solely because runtime is closing. DELETE Stop for an unknown historical job in an existing session now idempotently publishes an `error`/interrupted completion card and returns status interrupted, so old stale cards can be cleared after restart. MCP: successful plugin generation publication now emits workspace-global mcp.tools.changed with directory/generation; shared event matching/classification accepts global MCP invalidation; daemon converts to typed McpChanged; wasm refreshes only a currently visible MCP/McpActions picker; desktop SSE does the same via show_mcp. Hidden pickers are not opened. Full stack cargo check, targeted server/shared/daemon tests, LSP, diff check pass.
