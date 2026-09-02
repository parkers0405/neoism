---
name: bug-execution-timer-inherited-new-run
description: "New Agent runs inherited the previous run's execution timer — TWO root causes: worker-context quiescence (fixed) + leaked 'running' session_runs rows vetoing mark_execution_finished (fixed)"
metadata: 
  node_type: memory
  type: project
  originSessionId: 39912326-ef2c-4184-a6b4-eec06c374bd3
  modified: 2026-08-27T02:45:40.667Z
---

Agent V2 execution timer carried over into brand-new runs (2026-08-26) — FIXED.

**Symptom:** follow-ups and steered prompts correctly continued the total timer (by design), but a genuinely new run — including the first run after a daemon restart ("same timer as before launch") — also showed the accumulated old timer. Dev store proof: one `execution_activity` row (finished=0) spanned an hour of turns.

**Root cause (two holes, both from worker context):**
1. `append_prompt` runs the ENTIRE generation inline on the queue worker's stack (`drain_prompt_queue` → `append_prompt` → `start_session_run` … `finish_session_run`). So the run-end settle (`try_finish_session_run` → `finish_if_quiescent`) always sees `worker_active == true` and bails — deterministically, not a race. Nothing settled after the worker exited.
2. `ensure_for_prompt(allow_new)`'s pre-admission reconcile is ALSO called from inside the worker, so its `finish_if_quiescent` bails on the admitting prompt's own worker flag → Case 1 (`!finished && inherited id matches`) inherits the stale execution. This is also why restarts didn't help: the stale finished=0 row survives, and the first post-restart prompt still can't settle it.

**Fix:**
- `session_queue.rs::drain_prompt_queue`: call `finish_if_quiescent` after the worker loop exits (mirrors the deferred-subtask-completion placement there).
- `execution_activity.rs`: `finish_if_quiescent_impl(state, session, admitting_session)` — admission path (`ensure_for_prompt` allow_new) exempts the admitting session's OWN worker/queue from the quiescence guards (they belong to the new prompt, not the old execution). Children's workers/queues/runs/segments/branches still block settle.
- Regression test `new_top_level_prompt_settles_prior_execution_despite_own_worker` (worker flag set + session extra carrying old executionID — verified red pre-fix).

**Key invariants:** executions settle at (a) worker exit, (b) new-admission reconcile, (c) run end when no worker (direct appends), (d) bg-job end / session delete / compaction release. Follow-ups+steers inherit via Case 1 on an unfinished execution — that's the desired "total timer" behavior.

**Streaming latency audit (same session, MEASURED):** token path is genuinely real-time at every hop — provider per-chunk → publish_live per delta → SSE per event (post [[bug-agent-double-stream-tail]] fix) → client thread + immediate winit wake → pane drains 512/frame. Live probe against the debug server's /v2/events (create session via API + prompt + timestamped SSE reader, script pattern /tmp/sse_cadence.py) proved deltas are token-sized (1-15 chars) and relayed sub-ms, BUT they ARRIVE in OpenAI edge-side batches: typically 1-4 tokens per flush every 75-160ms, with 10-15-token mega-flushes after ~500ms stalls — and the FIRST flush after a reasoning phase can carry a whole buffered preamble sentence, which then "jumps in". So whole-sentence jumps = codex SSE batching, NOT our stack. Only remedy would be a client-side typewriter reveal (paced revealed_len per message) — a UX feature, not a bug fix. Debug agent server = ./target/debug/neoism embedded, loopback port from `ss` (was 37695), auth = Bearer <daemon-token file>; probe sessions: create/DELETE via /v2/sessions.

**SECOND ROOT CAUSE (regressed 2026-08-27, commit a85af70c5) — leaked store run rows:** timer inheritance came back with the worker fixes intact. Debug-store forensics: `session_runs` row stuck `status='running'` (created==updated, never finalized) → `mark_execution_finished`'s SQL guard (`NOT EXISTS session_runs status='running' for family`) vetoed settling FOREVER → execution stayed finished=0 → every prompt joined via Case 1. Leak source: `session_context.rs::release_owned_compaction_run` finished the run in the COORDINATOR only, never `store.finish_run` — while manual compaction's start path DOES `store.start_run`. Fix: (1) release now durably finishes the store row first (try_finish_session_run ordering); (2) `finish_if_quiescent_impl`, after all live-authority guards pass under the root keyed lock, calls `store.interrupt_abandoned_runs(family)` — sound because run starts take `admission_guard` (same keyed lock) before `store.start_run`, so a 'running' row with no coordinator run is provably dead; self-heals ANY leak path. Tests: `quiescence_reconciles_leaked_running_run_row_and_settles`, `releasing_owned_compaction_run_finalizes_the_store_row`. Startup already sweeps via `interrupt_stale_runs` (process-kill leaks heal on boot). Forensics recipe: copy debug turso db, check `execution_activity.finished`, `session_runs` where created==updated status='running', `prompt_queue` stragglers.

Related: [[bug-agent-double-stream-tail]], [[bug-agent-silent-stop]]
