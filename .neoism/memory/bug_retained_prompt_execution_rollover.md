---
name: "Retained prompt admission blocked next execution - fixed"
description: "FIXED self-blocking retained admission after first reply; SQL admitting-session exemptions + dequeue at real handoff; 21 queue tests pass"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Retained queue admission blocked execution rollover

2026-10-19 follow-up regression from startup hardening. User images: FIRST prompt during generation had `queued item` despite no followup; after first reply second prompt emitted global notice `Neoism: execution <root-session> still has active work`, no model progress. Root cause: worker now retained prompt_queue row until durable run handoff, but state.admit_execution_activity rollover SQL + mark_execution_finished settlement SQL considered that same incoming session's queue competing old work. Rust finish_if_quiescent_impl already exempted admitting_session; SQL did not. Fresh first execution insertion worked, next turn after finished execution failed/parked indefinitely. Original19 queue tests missed distinct second prompt in used session.

Fix in state.rs + execution_activity.rs: explicit admitting_session optional queue exemption for settlement AND rollover, preserving default strict wrappers for other callers. Ignore only admitting session's pending queue, including queued successors; real runs/provider segments/outstanding subtasks/other family queue still block. ensure_for_prompt passes session ID; finish_if_quiescent_impl forwards its existing admitting_session.

Separate display bug: drain_prompt_queue published dequeue only AFTER append_prompt_admission returned full reply. Moved dequeue/status to durable handoff in session_prompt before title/model/provider work, and to acknowledgement boundary for noReply/idempotent branches. Carry delivery in admission tuple(&id,saved,&delivery), preserve original request for event identity. Queue post-return updates now action snapshot, not duplicate dequeue. Recovery token remains durable until run handoff, not deleted early.

Tests added public v2 stub-provider first-then-second prompt same session +two-prompt burst after settlement, asserting real replies, empty queue, released run/worker, new executionroot, dequeue0 BEFORE first streamed reply. Both pass. All21 queue tests pass on fixed compiled binary. Execution group16/17 pass; reopen-quiescence fixture races startup auto recovery (finished before expected unfinished assertion), not marked fullygreen. Strengthened existing new_top_level_prompt_reconciles_settled_execution_before_admission with pending own queue to exercise unfinished settlement; final targeted test tracked parent session. Server --tests check passed. No paid provider calls/live state mutation/release build. Normal rebuild/restart required; do not kill user's running server from agent (same server can host current chat).
