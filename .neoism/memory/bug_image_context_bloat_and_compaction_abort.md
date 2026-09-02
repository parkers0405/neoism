---
name: bug_image_context_bloat_and_compaction_abort
description: "Image attach ballooned context (base64 inlined as text); compaction/undo couldn't be ESC-cancelled — both fixed 2026-07-08"
metadata: 
  node_type: memory
  type: project
  originSessionId: edf84f9f-cc77-480f-80c8-0dd8200b614f
---

Two agent-runtime fixes (2026-07-08):

**Image context bloat.** `message_model.rs::visible_part_text` `Part::File` arm emitted `[file: {mime}] {url}` — inlining the *entire base64 `data:` URL* into the message `content` string, on top of the correct structured image attachment. So every pasted screenshot was sent twice (once tokenized as text), re-sent every turn from history, and never stripped by compaction (`include_attachments:false` only drops the structured block). Fix: `file_part_placeholder()` emits a compact `[image: <filename>]` / `[file: <label>]` and never inlines a `data:` URL (real http URLs still inline, they're small). Structured `attachments` path already carried the real bytes to every provider. This is why neoism used far more context per image than opencode/codex.

**Compaction/undo not ESC-cancellable.** `compact_session_context` only *checked* `state.inner.runs` for a conflict but never *registered* a run, so `abort_session_run` (which cancels by looking up `runs[session_id]`) found nothing to cancel — compaction always ran to completion. Fix in `session_context.rs`: wrapper `compact_session_context_inner` registers a transient cancellable `SessionRun` (or reuses the active run for auto-compaction), always releases it (register→`run_compaction`→release, leak-safe against `?`), and the stream loop in `generate_model_compaction_summary` races `wait_for_cancellation(cancel)` so abort is ≤50ms. Undo/redo (`execute_session_history_command`) moved OFF the UI thread (was blocking `api_request_json`+`fetch_session_messages` on the huge sqlite, freezing the pane so ESC couldn't be processed) → background thread + `SessionHistoryApplied`/`Failed` updates. Also: `clear_or_abort` now aborts on a **single** ESC while `is_streaming()` (incl. Compacting), keeping the double-press guard only when idle. See [[bug_compaction_loop_and_fallback]] [[bug_undo_redo_415]] [[perf_agent_pagination]].
