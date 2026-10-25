---
name: "Manual compact queued like steering - FIXED"
description: "FIXED: durable manual compact control at safe boundaries; desktop/web/CLI aligned, auto untouched; restart/Stop/clear/pop/lifecycle regression coverage"
type: "bug"
scope: "project"
origin: "implementation and verification in current session"
created: "2026-10-18"
updated: "2026-10-18"
---

FIXED: manual /compact is backend-owned durable pending control; auto-compaction triggers/call sites/lifecycle intentionally preserved at user's request. Former bug: manual backend rejected occupied run slot; desktop polled300x1s, web sent once, CLI aborted first. OpenCodeV2 reference anomalyco/opencode dev501b15af26915ba52682e20e220e7e137e89e92f (2.0.32): typed durable compact control defaults steer, prioritized at step boundary.

Implementation: state.rs prompt_queue kind TEXT NOT NULL DEFAULT 'prompt' migration; partial unique index one pending 'compact' per session, separate typed control methods (never PromptRequest magic). session_queue::enqueue_manual_compaction persists+wakes existing worker even while busy; both HTTP compact/summarize admit promptly. Active session consumes via session_context::drain_manual_compaction_for_run at initial and after-step boundaries before steers; refresh info before constructing provider context; compact not included in steer count, so no extra reply. Idle worker compact_queued_session_context claims own run BEFORE authoritative atomic delete. Run conflicts leave pending row; final-boundary late enqueue reaches worker. Worker terminal error cleanup targets original control ID and parks on store cleanup failure, avoiding hotloops. Pending controls survive restart and session transfer (SessionBundle VERSION3; old2 accepts default compactionQueued=false; old hosts reject3 rather than silently drop controls). Stop clears pending compact only, shared admission lock serializes claim/abort/visible Clear/Pop; Pop removes compact before ordinary prompts; internal clear-all remains lock-free for locked callers.

run_manual_compaction wrapper closes lifecycle on any returned storage/provider error, clears time.compacting best effort only for matching attempt timestamp, emits fallback ended once. Manual in-run error does not end owning run or emit run-terminating session.error. Auto bypasses new wrapper and retains previous behavior. Ended/compacted runContinues:true ONLY manual inside existing run; false/absent automatic/standalone unchanged.

Contract: session.compaction.queued {sessionID}; session.queue.updated properties.queue {count,preview,compactionQueued,items}; compact item {type:'compact',preview:'Compaction queued'}. Busy status/reconnect queue includes same metadata. Consume/cancel refresh stored status to prevent stale badge. Existing started/delta/ended/compacted lifecycle; optional runContinues on ended+compacted. Canonical OpenAPI/v2.json+sha and TS generated contract updated.

Clients: desktop one scoped HTTP admission call, no retry and no false finished/idle; scoped async admission failure cached to origin. Queue snapshots are side-effect-free streaming-wise; normal dequeue prompt echo BEFORE authoritative snapshot (desktop speculative decrement otherwise clears remaining compact). Web daemon forwards same lifecycle and session-scoped admission errors; wasm optional protocol run_continues. CLI no abort-first, says Compaction queued; completion SSE driven. SDK GUI narrows compact queue union and labels control separately; welcome Compaction.md updated.

Verification: cargo check --tests passed server, CLI(package neoism-agent), UI, protocol, desktop; production --lib --bins check passed daemon + wasm; actual wasm32-unknown-unknown check passed. SDK npm run typecheck and GUI tsc --noEmit passed. Vitest useSessionActivity.test.tsx9/9 passed. Rust regressions added/typechecked, NOT executed (user wants cargo check only, own build loop). Wider daemon --tests blocked by unrelated remote_pty_io.rs:268 exhaustive match missing TerminalDirectoryChanged/TerminalDirectoryChangeFailed; left untouched. git diff --check clean. Existing unrelated dirty changes preserved.

Regressions: tests_manual_compaction.rs has local stub tests dedupe/busy endpoints/priority/ownership/Stop/Clear/Pop/restart+transfer/late wake/first-step/cancellation/terminal errors; test-only DB failpoint for post-start assistant-insert failure and exactly-once error-ended. UI tests cover acceptance preserving stream, late session-scoped errors, continued run lifecycle and authoritative snapshot ordering.
