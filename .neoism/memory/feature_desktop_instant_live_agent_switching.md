---
name: "Desktop instant live agent switching"
description: "Desktop agent selection is local and instant, backed by continuously hydrated per-session caches, a root-tree event stream, guarded background repair, bounded batching/preloading, and reconnect reconciliation."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-08-17"
updated: "2026-08-17"
---

# Desktop instant agent session switching

## Root causes
- `NeoismAgentPane::activate_cached_session` synchronously fetched `/session/status` on the UI thread.
- Warm activation cloned active and cached transcripts, then merged them with repeated linear searches.
- Switching reset runtime and timeline layout state, forcing synchronous reconstruction and a full render rebuild.

## Implemented model
- Selection is immediate local UI state. Warm sessions move transcript ownership between active state and `session_cache`; network status is background repair only.
- `CachedAgentSession` preserves transcript, optimistic-prompt reconciliation, runtime, model context, timeline scroll/history/layout cache/dirty state, hydration, and LRU access time.
- Inactive root/child sessions continue ingesting transcript and runtime events into keyed cache entries.
- One root-tree SSE stream tracks descendants. Side-panel discoveries seed the live stream; reconnect reconciles root plus all known child statuses/transcripts before buffered live events are consumed.
- Runtime polls use generation plus per-session live revision guards. Late terminal-child text is retained but cannot resurrect terminal runtime.
- Prompt dispatch remains owned by its origin session across navigation. Success/failure and server echo reconciliation update the inactive cache correctly.
- Raw SSE ingestion is capped at 512 events/frame and background updates at 64/frame. Adjacent same-part deltas coalesce without bypassing the raw-event budget.
- Proactive preload is capped at 2 concurrent and 10 queued; selected/forced targets receive priority. Session cache is bounded at 40 with active/running/pending sessions pinned.
- Hidden panes drain live/background state but not potentially blocking outbound commands.
- Compaction events carry and respect their owner `session_id`.

## Key files
- `neoism-frontend/desktop/src/neoism/agent/commands.rs`
- `neoism-frontend/desktop/src/neoism/agent/pane.rs`
- `neoism-frontend/desktop/src/neoism/agent/pane/ingest.rs`
- `neoism-frontend/desktop/src/neoism/agent/pane/session.rs`
- `neoism-frontend/desktop/src/neoism/agent/updates.rs`
- `neoism-frontend/desktop/src/screen/bridges/agent.rs`
- `neoism-frontend/shared/src/panels/agent_pane/stream_events.rs`

## Validation baseline
- `cargo check -p neoism` passes.
- 18 focused desktop regression tests and shared descendant/compaction stream tests pass.
- Full desktop agent filter: 118 pass, 4 known pre-existing expectation failures (`apply_older_page_ignored_after_session_switch`, `model_change_queues_context_limit_refresh_for_runtime`, `setting_goal_during_a_run_queues_its_agent_turn`, `submit_prompt_while_streaming_queues_bottom_preview_without_transcript_echo`).
- Never use a release build to verify this work; debug checks/tests only.
