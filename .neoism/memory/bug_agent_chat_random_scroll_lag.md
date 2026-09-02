---
name: "Agent GUI random per-chat scroll lag — FIXED"
description: "Hidden persisted running background-task rows caused chat-specific full-transcript scans and perpetual redraw; fixed with cached, versioned authoritative runtime state plus subtask lease ownership corrections."
type: "bug"
scope: "project"
origin: "2026-08-30 investigation"
created: "2026-08-31"
updated: "2026-08-30"
---

# Agent GUI random per-chat scroll lag — root cause and fix

## Root cause
Some persisted conversations contain a hidden `background_task` tool row with `status: running`. Frontend hydration intentionally clears process-liveness clocks, but commit `af8f8e668` changed desktop/shared `running_background_task_count()` getters to rescan the transcript directly. A stale row therefore resurrected as live only in affected chats, explaining why shorter chats could lag while longer clean chats stayed smooth.

The stale marker caused multiple full-transcript scans/allocations per frame and refreshed `streaming_status_hold` every frame, creating continuous redraw. Generic transcript length, pagination, and layout virtualization were not the differentiator.

## Fix (2026-08-30)
- Desktop/shared getters return cached O(1) runtime count; historical hydration does not seed it.
- Relevant live message upserts refresh the cache; summaries are gated by cached count.
- Cold reconnect gets authoritative process-local jobs in `SessionRuntimeSnapshot` / protocol.
- Background job start/finish publishes a full family-filtered `session.background_tasks.updated` list.
- Every job transition has a server epoch plus monotonic revision. UI rejects stale/same-revision reconnect/SSE updates; a new server epoch permits revision reset.
- Server snapshots use a seqlock-style revision-before/list/revision-after capture so list and revision cannot mismatch.
- Execution-only runtime events omit background authority and cannot clear/resurrect job state.

## Related lifecycle fix
The execution lease loop previously reaped its own current-owner subtasks after a 15s heartbeat stall. Reconciliation now excludes the current server owner and only cleans foreign/previous-server leases. Durable failed lifecycle overrides zombie in-memory `running`. Explicit resume reopens a terminal subtask only through a direct task invocation on an active execution, matching OpenCode v2 instance-owned/explicit-cancellation semantics without automatic resurrection.

## Verification
Native `cargo check` passes for agent server, protocol, daemon, shared UI, and desktop. Focused desktop/shared background tests, protocol roundtrip, server background tests, lease test, daemon event test, and all OpenAPI contract tests pass. WASM reaches one unrelated pre-existing missing `tracing` dependency after the new exhaustive event handling was fixed.
