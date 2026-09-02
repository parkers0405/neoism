---
name: bug_agent_silent_stop
description: "Agent runs \"silently stop\" (no error) — fatal errors go through ONE ephemeral session.error broadcast that's lost on SSE drop; retry-widening delayed errors into that window"
metadata: 
  node_type: memory
  type: project
  originSessionId: 7f62545c-19db-444c-86e4-3b6a6574bcc7
  modified: 2026-07-29T20:45:07.749Z
---

Symptom: agent runs stop mid-generation with NO error modal; user comes back and
it "just stopped". Also: sidebar context/% flashes to 0 on a stop/fail.

Root cause (traced): a failed run surfaces ONLY via an ephemeral fire-and-forget
`session.error` broadcast (the only SESSION_ERROR emitters are session_queue.rs
:359/:388, from append_prompt's Err). Three gaps make a missed one vanish:
1. frontend has NO `message.updated` arm (stream_events.rs:614 → `_=>Vec::new()`)
   — the durable `assistant.error` copy is dropped;
2. `api_mapping::message_blocks` never rendered assistant `info.error` (only
   role/id/system/parts) — reloaded snapshot hid the failure;
3. client SSE disables replay: open_event_stream uses `since=i64::MAX&limit=1`
   (desktop agent/api.rs:1050), so reconnect replays nothing (events ARE
   persisted w/ ids — event_routes.rs:73).
Why it started after [[bug_codex_limit_drift_compaction]]-era retry widening
([[bug_compaction_loop_and_fallback]]/session_retry.rs pattern add): transient
provider errors that USED to fire session.error immediately (SSE up) now retry
3× w/ 2/4/8s backoff (session_prompt.rs:1976), so the terminal error lands tens
of seconds later — exactly when a flaky SSE ("Connection reset without closing
handshake") has dropped. DB evidence: recent runs all finish='stop' err=None
(server not eating errors) → it's the ephemeral-channel-lost path.

Fixes applied (cargo-check clean, UNCOMMITTED, no build/push):
- Render persisted error: message_blocks pushes `agent_message_system("Agent
  error", ...)` when assistant `info.error` present (+ new `assistant_error_message`
  extractor tolerating string/{message}/{data:{message}}). Client re-fetches
  messages on every reconnect, so a failed turn is now visible on come-back.
  (api_mapping.rs ~470)
- Context %-flash-to-0: `timeline.rs latest_usage` now skips zeroed usages
  (Some but input/output/total all 0 — an aborted turn) and falls back to the
  last turn that really consumed context.

Retry overhaul (2026-07-29, uncommitted, cargo-check clean) — "retry never
did shit" because `session_retry::retryable_error` returned early on
`provider_error.retryable` (HTTP STATUS flag only) and NEVER consulted the
message patterns → OpenAI 200-with-error-body / worded-5xx were classed
non-retryable. FIXED: retryable_error now also matches provider_error.message +
.body (except context_overflow). Ceiling DEFAULT_MAX_RETRIES 3→8, initial delay
1.5s. TRUE MID-RESPONSE retry: removed the `!saw_progress` gate at
provider_stream_processor.rs (both error sites) so retryable errors retry even
after tokens streamed; run_provider_stream_step_with_retry now calls new
`reset_live_message_for_retry` (provider_stream_message.rs — wipes partial reply
to a clean step_start+empty-text part, clears error/finish/tokens, update_message
+ republish) before each retry so it re-streams clean instead of doubling.
Inline "Retrying…" indicator (opencode/codex style): new NeoismAgentStreamingState::
Retrying + SessionEventUpdate::Retrying + AgentSessionUpdate::Retrying; the
session.status type:"retry" event (was dropped at stream_events.rs else=>Vec::new)
now drives note_streaming(Retrying) shown where "Thinking…" renders.

NOT done (residual follow-ups, both from the trace): (A) restore SSE replay —
track last event id in updates.rs read_event_stream, pass as `since` instead of
i64::MAX (makes errors visible LIVE, not just on reconnect); (B) empty-success
hole — provider_stream_processor.rs:157 End on a truncated/empty stream →
finish_provider_stream_success synthesizes finish:"stop" and returns Ok w/ no
content, no error (skipped: mis-detection risk vs legit tool-only turns).

**2026-08-27 addendum — frozen WATCHED pane after provider-overload retry (FIXED, 5b009b4a2):** user repro: overload 529 → retry → a NEW tab of the same session streamed fine, the ORIGINAL tab showed nothing until reopened. Root cause: half-dead SSE socket — server keep-alives every 10s (v2_routes Sse KeepAlive), but desktop `read_event_stream` treated WouldBlock/TimedOut as benign FOREVER, so a dead connection spun silently. Fix: track last-bytes Instant; >45s silence = stale → break → existing reconnect path (which already refetches statuses + messages page + idle recovery). Secondary fix: `reset_live_message_for_retry` re-seeds the SAME text part with empty text; the pane's empty-snapshot guard ignored the wipe → retried tokens doubled onto the partial. `retry_reset_pending` flag (set on Retrying update, cleared at idle) lets exactly one wipe through. Tests: silent_event_stream_goes_stale_and_returns_for_reconnect (real TcpListener), retry_reset_wipes_partial_text, late_empty_snapshot_without_retry_never_regresses. Diagnostic key: status pill can update via polling while SSE is dead — 'timer moves but no tokens' = dead event stream, not a render bug.
