---
name: "Finished subagent sidebar resurrection"
description: "Prevent finished subagents from returning active in sidebar after reconnect or delayed snapshots"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-08-30"
---

Pre-existing finished-subagent sidebar resurrection fixed locally after v0.7.73. Three independent causes/guards: (1) Desktop and shared/web runtime execution+branches are now applied atomically through apply_runtime_lifecycle_snapshot; branches from a rejected stale execution snapshot cannot travel alone. Both wasm foreground and cache routes use it; non-authoritative snapshots still apply execution timing only. (2) Desktop recursive /children hydration no longer treats persisted session/externalAgent running status as live authority. Final /sessions/status omission forces historical children completed; only live-map status can mark running/blocked, while persisted terminal status remains usable. (3) execution_subtasks now stores owner_instance_id, migrated with ALTER for old DBs, using existing 5s execution owner heartbeat/15s stale lease. Stale owner terminalization condition is rechecked transactionally to avoid heartbeat TOCTOU, increments family revision, runs periodically and before runtime quiescence. Owner GC now retains owners referenced by outstanding subtasks. Explicit durable subtask completion records also repair outstanding immediately, but only when completedAt >= branch started_at to avoid old-generation false completion. Healthy owner protects legitimate no-work launch/between-step gaps. Regressions cover stale snapshot no resurrection desktop/shared, historical persisted running omission, terminal UI settlement, explicit evidence old/new generation, stale-vs-live owner, deletion cleanup. Targeted tests and cargo check for server/ui/desktop/wasm pass; git diff --check passes. Workspace contains unrelated untracked `updare`, untouched. Rust-analyzer showed stale old-arity diagnostics in execution_activity despite successful rustc tests/check; other LSP diagnostics clean. Uncommitted/unreleased.
