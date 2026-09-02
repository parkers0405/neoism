---
name: "General Agent execution activity"
description: "General execution activity plus authoritative stable parent subagent indicator and per-child model timers; commits 673856695 and 415366162."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-08-26"
updated: "2026-08-26"
---

## Shipped

- Base activity implementation: `6738566958de187aa5954f9f1b3b52630e7a5e4f` (`Stabilize agent execution activity`).
- Correctness follow-up: `41536616295a148128ad25279e3902b08905dcea` (`Fix subagent activity status and timing`), pushed to `neoism_agent_v2` on 2026-08-26.

## Semantics

- Root/main view displays aggregate family provider/model-seconds.
- A viewed child displays only that child's cumulative provider/model-seconds.
- Each provider segment is atomically credited to both the family aggregate and its owning session before exact-once deletion.
- Turso migration backfills existing aggregate history and active segment owners; finalization self-heals missing per-session rows.
- Parent `Sub-agents working...` derives from the durable authoritative outstanding branch set and remains continuous from admission through terminal completion.
- Child SessionRun idle, status endpoint omission, stale tree recovery, tool-part completion, and transcript cache switching are nonterminal and cannot decrement the parent branch set.
- Admission and terminalization publish full authoritative family runtime snapshots, including independent family revision and per-session activity.
- Old-family runtime events are rejected after an out-of-family switch.
- Finished executions retain timing internally but do not reserve a permanent `Completed` status row.
- A completed viewed child also does not reserve blank status chrome while another family member remains active.
- Runtime snapshot SQL is one statement with linear UNION rows, avoiding segment x session x task Cartesian growth.

## Verification

- Agent server: 442 passed, 5 environment-only ignored.
- Shared UI: 2105 passed.
- Protocol: 92 passed.
- Workspace daemon: 175 passed.
- Desktop compilation and focused timer/status regressions passed.
- Actual WASM target passed.
- Final warning-denied scoped check passed.
- Migration and missing-row self-heal regression passed.

Unrelated frontend Markdown/drawing/input compatibility changes and Firecrawl artifacts were not staged.
