---
name: "History-query and FTS no-op fixes; replay/live safety"
description: "Scoped history/no-op FTS fixes pass35 tests; candidate not activated; preserve live switching; event retention requires cursor/HWM compatibility and FTS engine amplification remains"
type: "bug"
scope: "project"
origin: "Omarchy lag investigation and focused regression verification"
created: "2026-10-25"
updated: "2026-10-25"
---

## Findings

Omarchy Framework16 app uses AMD native Vulkan; Windows uses wgpu. Live PID4867 workspace daemon showed multi-TiB disk reads over hours. Checkpointed main-file metadata suggested ~43.5 GiB agent.turso.db, ~3,580 sessions (791 roots),179,122 messages,4.22M events; counts approximate because live WAL was excluded. Stock SQLite needs unsupported Turso schema parsing bypass; do not treat this as a validated live snapshot or repeat lock-bypass diagnostics. Recent log confirmed SELECT info_json FROM sessions ORDER BY updated DESC taking up to2.72s. Embeddings disabled; list/search backfills complete.

## Implemented, not yet activated

`SessionStore::list_child_sessions` uses existing idx_sessions_catalog_parent and preserves canonical extras/hosting. v2 children, task-tree traversal, pending parent completion checks scoped to family. Cycles terminate without returning root. Permission/question polling only authorizes distinct pending-session IDs; empty snapshots read no archive; authenticated failures fail closed. `state/conversation_search.rs` joins existing projected fields, compares complete projection, and emits only transactionally guarded dirty cleanup for unchanged docs. No-op cleanup revalidates revision, canonical source, and every projection field. ProjectionTick separates work from native index changes so cleanup does not re-arm optimization. No schema/data cleanup changes.

35 focused regressions PASS on final test binary: family4 + search21 + interaction3 + pending completion1 + live baseline5 + raw late-baseline reconstruction1. Includes legacy/helper parity, poisoned unrelated history, source-byte bounds, rollback, race, concurrent tokens, completion during replay, live child runtime and reconnect. Four broader session_actions lifecycle tests fail (abort wrapper, teardown extra snapshot, persisted completion reply, startup idle deferred); attribution unresolved, NOT established as preexisting. Independent review found no actionable diff issues. Desktop candidate build is ongoing; no install/restart and no live DB mutation performed. See docs/agent-database-activity.md.

## Remaining work / strict live-safety requirement

User requires preserving click-through into active/completed subagents and clicking away/back during live generation. Never disable live baselines, remove navigation, or delete subagent transcripts to save storage. Raw token deltas are ALREADY transient. The events journal supports explicit since/Last-Event-ID replay, including SDK/ACP/older hosted clients; no persistence-off setting exists. Global wire counter seeds from MAX(events.seq), so pruning requires independent durable high-water metadata and explicit replay floor, plus negotiated expired-cursor snapshot reconciliation. Conversation/queue/recovery state is separate, but blanket persistence-off or pruning is NOT safe yet.

Turso0.6.1 native FTS invalidates directory cache after real writes; cache miss enumerates blob-backed records and eagerly reads cold payloads just for length. No-op suppression removes unnecessary triggers, NOT engine amplification on genuine edits. Removing cache invalidation is unsafe (rollback/snapshots). 0.7.x still has problem;0.8.x uses incompatible FTS segment layout requiring deliberate rebuild. Potential format-preserving engine fix: inspect local B-tree record prefix/serial-type blob length for cold files, full-record fallback/hot capture; requires reproducible dependency patch and counting-I/O/correctness tests.

OpenCode v2.0.32 keeps completed child sessions/transcripts, hides children from Home, preserves parent→child navigation, permits same-parent resume with sessionID, recursively removes children on explicit parent deletion; no completion TTL found. Its raw event payload persistence defaults false, unlike Neoism's current replay contract. Pinned upstream843a6c514e606b436a7fe713422ace0a93ad3d61.
