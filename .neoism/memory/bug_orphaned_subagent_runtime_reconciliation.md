---
name: "Orphaned subagent runtime reconciliation"
description: "Killed historical subagents no longer resurrect from stale runtime ledger"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-29"
updated: "2026-08-29"
---

Fixed old killed/interrupted subagents resurrecting in the sidebar. Root cause: durable execution_subtasks rows could remain status='outstanding' after process/interrupted teardown; finish_if_quiescent treated that stale row as a veto even after every live authority (provider segments, coordinator runs/workers, queued prompts, background jobs) was gone. /runtime then authoritatively re-announced the dead child as active. Quiescence now terminalizes such orphan rows to failed under the root keyed lock, increments family revision through the existing store method, interrupts leaked run rows, and settles execution. /v2/sessions/:id/runtime defensively reconciles before reading. Real work remains protected: regression holds an active provider segment and verifies child stays outstanding, then removes it and verifies failed+finished. Desktop/shared recovery only upsert active branches; terminal recovery snapshots lock and hide existing rows immediately (no 7s live-completion linger), then prune. Live completions still use the existing linger path. Shared, desktop, server tests and cargo checks including wasm pass.
