---
name: "Primary queue flash and Esc handoff - FIXED"
description: "FIXED old-chat primary queued flash + graceful Esc: empty queue immediate idle; real queued turn neutral footer handoff, abort HTTP off UI thread"
type: "bug"
scope: "project"
origin: "current session implementation and verification"
created: "2026-10-18"
updated: "2026-10-18"
---

FIXED two related frontend issues using same general subagent per user request. (1) Reopen old conversation then send idle ordinary prompt: Crafting correct but briefly 'queued item' for primary. Root server admission queue count1 before worker; desktop admission correction was Windows-only and new canonical QueueSnapshot bypassed it. Existing chat attaches SSE before POST so intermediate frame visible; fresh chat attaches after POST tends to batch. No one-second UI timer. (2) Esc while ordinary message queued dropped footer then recreated and froze: local unconditional Idle plus synchronous desktop POST /abort on UI thread.

Shared status_policy::PromptAdmission tracks ONLY locally submitted idle primary ID; project count platform independently, closes on running/start evidence, retains ID until matching dequeue to avoid consuming true followup. project_preview clears unknown wire-head preview while hiding primary (server preview would be primary A even if count2); explicit Compaction queued preserved. Running/action metadata survives canonical QueueSnapshot and full daemon session.queue.updated SessionEvent. Desktop/cache QueueStatus, snapshot and hydration all project consistently. Local submit revision boundary generalized to reject pre-Enter hydration on all platforms; Windows-only clocks/retired-execution policy unchanged. Shared/web reservation + cache parity added.

Strict user Esc contract: NO ordinary queued work => immediate Idle / Crafting/Pondering/tool indicator gone. Compact-only or optimistic pendingcompact => cancel + immediate Idle; don't count it as successor. Real ordinary queue => QueueHandoff holds neutral Generating/Crafting mounted with no stale reasoning/tool label until next dequeue/distinct start promotes; abort guard cleared only for successor. Clear/failure/real idle settles. Empty busy snapshot matching old run is ambiguous; canonical, QueueStatus, shared apply_queue, cached updates AND hydration all defer settlement consistently. Don't hide every queue during Crafting, debounce, sleep or reorder backend scheduler.

Desktop commands::execute_abort_session_command now background thread; success silent; failure origin scoped and can't set successor Idle. Session-scoped StoppedParts identity fence retains seen/stopped part+assistant-parent IDs across dequeue/promotion/cache/hydration; retired live parts rejected before transcript/revision mutations and retired saved tail cannot drive activity, saved history kept. Added optional parent_message_id to protocol ContentDelta+MessageUpdated, daemon producers, wasm active/cache consumers and TS mirror. No backend Stop/scheduler/auto-compaction semantic changes. Existing manual /compact durable queue + optimistic request overlay preserved.

Main files: desktop pane/status_timing.rs, submit/session/ingest + commands/api/updates, new pane/admission_tests.rs +admission_tests/review.rs; shared status_policy.rs, state/queue.rs, queue_tests.rs+queue_tests/review.rs, ingest/streaming/cache/session, stream_events; daemon events.rs; wasm catalog.rs; protocol agent.rs +tests/part_parent_identity.rs. Tests cover canonical split/batched events both OS policies, old hydration, real followups, compact interleavings, cache, strict emptyqueueStop for allstates, queued handoff, oldstartzeroqueue ambiguity everypath, late cancelled liveparts before/after successor, abort failure scope.

Final verification PASSED: cargo check -p neoism -p neoism-ui -p neoism-protocol --tests; daemon+wasm --lib --bins; actual wasm32-unknown-unknown --tests. git diff --check clean; unrelated warnings remain, new overwritten changed assignments cleaned. Rust tests TYPECHECKED NOT EXECUTED per user workflow. No paid provider calls/live UI/build/restart. Changes require user's regular build; don't claim runtime animation was visually verified.
