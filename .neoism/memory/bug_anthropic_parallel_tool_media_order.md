---
name: "Anthropic parallel tool/media history rejection"
description: "Parallel tool-image results interleaved media, Anthropic 400 locked chat; shared ordering + wire grouping fixed, live chat recovered without deletion"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-11"
updated: "2026-10-11"
---

## Cause
Anthropic 400 saying a tool_use ID lacks an immediately following tool_result can come from media interleaving, even when every tool completed successfully. `message_model.rs::assistant_provider_messages` flattened each tool as `[Tool result, User media]`, so a two-image read batch became calls A+B, result A, image A, result B, image B. `provider_anthropic.rs::anthropic_body` converted messages one-to-one; Anthropic rejected B because ordinary image/text interrupted the tool-result prefix.

## Fix
Shared replay now emits all tool results before any media in an assistant batch. Anthropic serialization coalesces adjacent same-role messages and inserts every tool_result before ordinary text/media in the combined user message. Regression coverage exercises completed/error/interrupted results, interleaved images, normalization, replay, model-switch replay, and compaction. Do not replace successful tool states with errors; this bug is ordering, not lost execution.

## Live recovery on 2026-10-11
Recovered `Review Neoism embedded Rust browser versus T3 Code`, session `ses_ed30c7bd8ffeAT7zT0aGenyNOQ`, in `/home/parkersettle/projects/neoism`. Offending batch in message `msg_12d12beb9001uMXdRTzYe1Iukc` contained read calls `toolu_011YcfK1eQxR6DF49i8nSJAv` (chrome.png) and `toolu_01TWfGfTbgB8PhT4wYAy7fQL` (servo.png). Backed up session and all 42 messages at `~/.local/state/neoism/backups/anthropic-tool-order-ses_ed30c7bd8ffeAT7zT0aGenyNOQ/`. Through live API PATCH, moved first tool's state.metadata.attachments to the final tool's attachment list so both results precede both images even in the running old binary. Preserved every message, call, input, output, status, and both screenshot artifacts; verified all other transcript fields unchanged. Later observed user's `keep going` followed by numerous successful Anthropic turns and completed tools, no repeated 400.

## Recovery API details
Local Agent V2 API was reachable directly at `http://127.0.0.1:4096/v2/sessions`. `/session` and `/api/session` serve SPA and are not API endpoints. GET `sessions?search=...`; GET `sessions/{SID}/messages?order=asc&slim=false` returns `.items[]` with all messages when limit omitted. GET `sessions/{SID}/runtime` confirms idle. PATCH `sessions/{SID}/messages/{MID}/parts/{PID}` replaces a COMPLETE Part, not a partial merge, and broadcasts the update. Turso exclusively locks `~/.local/state/neoism/agent.turso.db`; do not bypass daemon by SQLite writes. New workspace-scoped daemon route `/agent/workspaces/{WS}/v2` returned empty lists for this legacy session due scope, while direct local API found it. Workspace discovery: POST loopback `:7878/agent-gui/workspaces` with Origin `http://127.0.0.1:4096` (read-only discovery).
