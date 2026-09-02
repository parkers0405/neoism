---
name: "Subagent live token SSE broken — FIXED"
description: "Root-scoped /v2/events excluded descendants and discarded publish_live deltas; fixed with family-scoped durable replay plus separate transient SSE bus and route-level tests (1db33c446)."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-25"
updated: "2026-08-25"
---

# Subagent live token SSE broken — FIXED

Fixed on `neoism_agent_v2` in commit `1db33c446`.

## Root causes

- Desktop intentionally holds one root-session SSE stream, but `/v2/events?sessionId=<root>` queried durable events with exact `session_id = root`, excluding all child events.
- Provider and ACP token deltas use `AppState::publish_live`, but `/v2/events` treated every broadcast only as a wakeup for durable replay and never yielded the transient payload.

Completion still worked because durable parent lifecycle events and later snapshots arrived.

## Fix

- Split durable commit notifications and transient live payloads into dedicated internal broadcast channels while preserving the existing all-events subscriber.
- `/v2/events` now replays durable events across the root and all descendants, forwards transient family deltas immediately without advancing the durable cursor, dynamically admits children created after connection, and excludes unrelated sessions.
- Added HTTP route-level tests for live child delta delivery and durable child replay with unrelated-session exclusion.

## Verification

- Strict `RUSTFLAGS='-D warnings' cargo check -p neoism-agent-server` passed.
- Both new SSE tests passed.
- Full suite: 401 passed, 5 ignored; one unrelated timing-dependent LSP crash-restart test failed waiting for diagnostics.
