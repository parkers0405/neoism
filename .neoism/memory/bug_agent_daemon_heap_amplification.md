---
name: "Agent daemon heap amplification: v2 streaming fixes"
description: "OpenCode v2 parity fixes: raw deltas, 8 MiB event bus, active context paging, capped ACP/spooled output, bounded WebSocket agent replies; 57+9 Rust and 29 GUI tests pass"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-25"
updated: "2026-10-25"
---

# Agent daemon heap amplification

Observed the embedded workspace daemon around 1-1.5 GiB RSS plus roughly 2.3 GiB swapped, with a 3.79 GiB resident high-water mark. UI around 150-200 MiB; rust-analyzer separately around 5.9 GiB. smaps showed mimalloc-backed anonymous heap, not GPU/file mappings; no heap profile established live-object versus allocator-retention split. The 42.3 GiB agent.turso.db and 660 MiB service log were disk sizes, not RAM attribution.

Compared actual OpenCode v2.0.32, pinned in neoism-agent/evals/public/opencode-v2/package.json; upstream commit 843a6c514e606b436a7fe713422ace0a93ad3d61. V2 uses incremental session.tool.input.delta and bounded subscriber delivery. Do not substitute old archived OpenCode.

Fixed native quadratic prefix amplification: provider_stream_processor emits MESSAGE_PART_DELTA field raw instead of 45 ms full growing input snapshots. Initial and semantic-boundary full parts remain authoritative. Live baseline, desktop/shared/cached/WASM and SDK GUI reducers append into pending state.raw and clear pending raw on semantic transitions.

AppState state/event_bus.rs is an ordered cursor-based broadcast with 16,384 event maximum and 8 MiB estimated retained-heap budget. Immediate eviction produces Lagged/reconnect; slow receivers retain cursors, not payloads. Consumed entries reclaimed; sparse JSON objects compacted. Not an allocator-level or whole-daemon cap.

SessionStore::list_context_messages reads backwards in 32-row pages until latest completed compaction plus protected tail. Agent request/compaction paths use it; UI/full persisted history remains untouched. Drop decoded histories before provider waits; skip redundant history read when auto-compaction disabled. Pending/failed compactions do not truncate history.

ACP external output uses managed exact-output disk spools, bounded previews, text deltas until 51,200 bytes, one truncation/path replacement at cap crossing, then suppresses further raw live text. Actual live reconnect baseline and durable completion snapshots are bounded; exact spool survives completion, interruption, process restart. Removed unbounded raw output copies from completion metadata. Files: external_agent/events.rs, helpers.rs, events_tests.rs.

Daemon agent/reply_queue.rs bounds per-WebSocket agent replies to 256 messages / 32 MiB estimated serialized payload, allowing one oversized response only when empty. Envelope dequeue/drop releases accounting. Overflow fails all sender clones and closes the socket, including stalled agent writes, forcing baseline reconciliation rather than silent delta loss. Generic non-agent WebSocket queues and already-dequeued/in-flight payloads are not covered.

Also fixed ExternalRunGuard::finish in external_agent/lifecycle.rs: preserve run_id through async finish_session_run, disarm afterward so cancellation Drop retries cleanup. Existing cancellation test previously timed out because take() happened before await.

Final verification: 57 backend memory/external-agent tests passed; 9 daemon reply-queue tests passed; GUI reducer tests 29/29; TypeScript, agent-server/daemon/desktop/shared UI checks and WASM check passed; git diff --check passed. No release build, runtime restart or live DB mutation. Installed running daemon remains old code until user normal build and full daemon restart; reopening just a window can reuse old daemon.

If memory remains high, measure synchronous durable event_writer queue depth/bytes (still an unbounded channel), Turso working memory and CRDT/LSP retention. Do not claim all original heap was conclusively attributed to these paths or claim live RAM reduction without testing the new binary.
