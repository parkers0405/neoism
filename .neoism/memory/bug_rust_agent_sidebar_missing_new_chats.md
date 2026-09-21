---
name: "Rust agent sidebar missing new chats — FIXED"
description: "Rust GUI sidebar catalogue stayed Ready and active-family SSE excluded new root chats; fixed with authorized directory-scoped catalogue SSE and in-place deltas"
type: "bug"
scope: "project"
origin: "agent investigation and implementation"
created: "2026-08-01"
updated: "2026-08-01"
---

# Rust agent sidebar missing newly created chats — fixed

## Symptom
After another user joined a workspace, later root chats sometimes did not appear live in the Rust GUI agent side panel, but appeared after relaunch.

## Root cause
The side-panel catalogue became terminally `SessionCatalogState::Ready` after its first successful hydration. `should_refresh_sessions()` never fetched it again. The only existing SSE was `/v2/events?sessionId=...`, intentionally scoped to the active conversation family, so `session.created` for an independent root was filtered out. A pane on agent home had no transcript stream at all. Relaunch reconstructed the catalogue in `Initial`, explaining why persisted chats then appeared.

## Fix
Added authorized, directory-scoped `GET /v2/session-catalog/events` in `neoism-agent-server`, carrying only root `session.created`, `session.updated`, and `session.deleted` events. It applies hosted association hydration before authorization and delivery. Deletion events now retain the deleted `SessionInfo` so directory/workspace authorization remains possible after the row is gone.

The Rust desktop starts one catalogue watcher per agent pane/server/directory, independent of the selected conversation. Live deltas upsert/remove sidebar rows in place, preserving pagination, scroll, and selection by session ID. Stream connect/reconnect triggers an authoritative newest-page reconciliation. Reconciliation merges with deltas so a create event racing an older HTTP snapshot cannot be erased.

## Key files
- `neoism-agent/crates/neoism-agent-server/src/v2_routes.rs`: `v2_session_catalog_events`
- `neoism-agent/crates/neoism-agent-server/src/session_routes.rs`: deleted event includes `info`
- `neoism-frontend/desktop/src/neoism/agent/updates.rs`: `AgentSessionCatalogStream`
- `neoism-frontend/desktop/src/neoism/agent/pane/session.rs`: watcher lifecycle
- `neoism-frontend/desktop/src/neoism/agent/pane/ingest.rs`: apply deltas/reconcile
- `neoism-frontend/shared/src/panels/agent_pane/state/side_panel.rs`: in-place catalogue mutations

## Verification
- `cargo check -p neoism` passes (existing unrelated warnings only).
- `cargo test -p neoism-ui live_session_catalog_mutations_preserve_pages_and_selected_session` passes.
- `cargo test -p neoism-agent-server v2_session_catalog_stream_only_forwards_roots_in_the_requested_directory` passes.
- `cargo test -p neoism-agent-server every_operation_has_a_unique_id_and_success_response` passes.
- `git diff --check` passes.
