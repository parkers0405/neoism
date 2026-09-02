---
name: "Execution lifecycle + cumulative model timer"
description: "Execution-scoped subtask lifecycle and cumulative model-time timer"
type: "feature"
scope: "project"
origin: "session"
created: "2026-08-26"
updated: "2026-08-26"
---

Implemented 2026-08-?? (working tree, not committed): subtask lifecycle is execution-scoped and separate from transient SessionRun busy/idle. Turso tables `execution_activity`, `execution_activity_segments`, and `execution_subtasks` persist execution identity, provider segments, revisions, and exact branch terminal state. `/v2/sessions/:id/runtime` hydrates authoritative lifecycle + execution activity; protocol/daemon carry snapshots to desktop/web. Missing children in `/sessions/status` never imply terminal. Counts use unique-ID union and terminal locks/dedupe.

Timer semantics: each root top-level user work cycle gets execution_id + root_message_id; internal continuations, queues, children, retries, and background work preserve it until the full tree is quiescent. No SessionGoal/complete_goal coupling. Display is cumulative **model-seconds** (sum of concurrent provider stream segment durations); non-model waits freeze and UI says `model`. Active epochs remain integer u64; wasm uses web_time; animation phase remains separate. New settled top-level prompt replaces execution; final total remains visible.

Tasks sidebar renders every task using outer panel scroll only; no cap/footer. Hours format as `1h 0m`.

Focused server/protocol/daemon/shared/desktop/web checks and regressions pass with CARGO_BUILD_JOBS=2. Full workspace --all-targets has unrelated/pre-existing daemon workspace_demote missing lsp_runtime; global cargo fmt check is blocked by broad pre-existing formatting drift. git diff --check passes. Existing frontend markdown and Firecrawl working-tree changes were preserved.
