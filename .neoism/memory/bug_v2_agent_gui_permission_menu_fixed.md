---
name: "V2 agent GUI permission menu fixed"
description: "Shared/web v2 permission GUI now receives, hydrates, acknowledges, and retries permission interactions correctly."
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-08-30"
---

Fixed the shared/web agent permission picker for the v2 agent-server path. Workspace daemon now translates `permission.asked` using current v2 fields (`metadata.tool`, `metadata.input`) and translates `permission.replied` into an explicit `PermissionRemoved` protocol event. `ResumeStream` hydrates `/v2/interactions/permissions?sessionId=...`. Reply POSTs use `{ reply: once|always|reject }`, emit immediate removal on success, and `PermissionReplyFailed` on failure so the picker can retry. WASM clears by permission request ID and no longer incorrectly clears from tool completion IDs. Added daemon event translation tests. Checks: `cargo check -p neoism-protocol -p neoism-workspace-daemon`, `cargo check -p neoism-terminal-wasm`, `cargo check -p neoism`; `cargo test -p neoism-workspace-daemon --lib permission_` passes. Full daemon integration-test compilation remains pre-existing broken due missing `lsp_runtime` fields in test AppState initializers.
