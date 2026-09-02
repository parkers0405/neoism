---
name: bug-undo-redo-415
description: /undo /redo gave HTTP 415 because Json extractor rejects empty-body POST; fix = Bytes-body handlers that compute target like opencode
metadata: 
  node_type: memory
  type: project
  originSessionId: 70ac95cc-bf03-40f8-8ab8-a09da2f75b18
---

`/undo` and `/redo` slash commands failed with **HTTP 415 Unsupported Media Type** (and would 400 from the daemon). Root cause: both clients POST with no/empty body —
- desktop `execute_session_history_command` → `api_request_json(.., None)` sends no `Content-Type` (neoism-frontend/desktop `agent/api.rs http_request`),
- workspace-daemon `handle_session_history` → `http_post_json(.., {})`.

The `/undo` and `/redo` routes were aliased to `session_revert`/`session_unrevert`. `session_revert` has a `Json<RevertRequest>` extractor that **requires `messageID`** and a JSON content-type → axum returns 415 (no content-type) or 400 (empty messageID).

**Fix (session_undo.rs):** dedicated `session_undo`/`session_redo` handlers take `body: Bytes` (never 415s, content-type-agnostic), parse an *optional* `RevertRequest`, and compute the target server-side like opencode's TUI:
- undo → most recent `user` message *before* the current `revert` marker (repeated undos walk backward); none left → no-op.
- redo → earliest `user` message *after* the marker → revert to it; none left → full `unrevert`.
- explicit `messageID` in body still honored (undo-tree UI + existing tests).
Refactored `revert_session`/`unrevert_session` cores out of the handlers; `/revert` `/unrevert` keep their `Json` handlers. neoism IDs are `{prefix}_{time:012x}{rand}` so lexicographic compare == chronological (matches opencode `x.id < revert`). Reverted messages are deleted from the store and stashed in `info.extra["revert"]`, so stepping reconstructs the full list first (`reconstruct_messages`).

Routes wired in app_router.rs (both `/api/session/:id/undo|redo` and `/session/:id/undo|redo`). Regression test: `session_undo_redo_step_without_request_body`. Not yet done (opencode parity): restoring the reverted user message's text back into the composer on undo. See [[bug-connectinfo-unix-daemon]].
