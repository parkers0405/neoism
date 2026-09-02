---
name: bug-agent-double-stream-tail
description: Agent V2 stream froze then doubled the tail of the reply — v2_events SSE interleaved durable snapshots ahead of queued live deltas; FIXED via live-drain-before-replay
metadata: 
  node_type: memory
  type: project
  originSessionId: 39912326-ef2c-4184-a6b4-eec06c374bd3
  modified: 2026-08-26T21:16:03.384Z
---

Agent V2 GUI "freeze then double-stream" (2026-08-26, branch neoism_agent_v2) — FIXED.

**Symptom:** reply froze mid-stream, then the tail (~last 15 tokens) rendered twice, joined mid-line. Doubled segment began at a token boundary (missing its `4.` list marker) — the fingerprint of live deltas re-applied on top of a full snapshot, NOT a server retry or provider replay.

**Root cause:** `v2_routes.rs::v2_events` merged TWO channels with plain `tokio::select!`: `live_events` (transient deltas) + durable-tick→store-replay. `select!` picks randomly, so at end of turn the final `message.part.updated` full-text snapshot (durable, committed after the deltas were published) could be yielded while tail deltas still sat in the live channel. Client: applies deltas → accepts longer snapshot (merge: longer wins) → appends the stale queued tail deltas on top. The idle `Messages` refresh can't repair it: `merge_session_snapshot` keeps live text when stored is a prefix of it. Store stays CLEAN — only the pane doubles (session reload shows correct text).

**Fix v1 (superseded):** drain-live-before-replay + biased select. Fixed doubling but left the MIRROR skew: committed snapshots could LAG behind later live deltas → reasoning/tool cards rendered BELOW the streaming answer until idle refresh (screenshot bug 2026-08-26).

**Fix v2 (FINAL — opencode event model):** ONE ordered bus. `AppState::publish` now broadcasts to `inner.events` synchronously at CALL time (persistence rides alongside in the writer task — bus-first, project-after, exactly opencode's Bus + SyncEvent projection); `live_events`/`durable_events` channels deleted; `v2_events` consumes the single subscription — optional `since`/Last-Event-ID catch-up replay first (with replayed-id dedupe for the overlap window, capped 16k), then pure publish-order live streaming; `tail=true` (the GUI) is live-only like opencode's `/event`, reconciling over REST. events channel capacity 16384; Lagged → drop + idle-refresh reconciles. Tests: `v2_event_stream_flushes_live_deltas_before_durable_replay` (snapshot never early) + `v2_event_stream_keeps_publish_order_for_committed_parts_before_deltas` (snapshot never late). Divergence kept from opencode: we retain cursor catch-up replay (they have none) and client never-shrink merge guards (belt-and-braces for older/proxied servers).

**Key architecture facts:**
- `inner.events` (`state.subscribe()`) is the single TOTALLY-ORDERED merged channel (publish_live sends at emit; event_writer sends after commit). The v2 route ignores it and splits live/durable — that split created the race.
- `publish_committed`/`publish` deliver durable events via a spawned task / writer queue — never assume publish vs publish_live ordering.
- Client never-shrink guards (empty-assistant-snapshot drop in `upsert_part_message`, prefix-keep in `merge_part_message`) make any server-side "reset text" invisible — a retry reset (`reset_live_message_for_retry`) republishes the same part id with empty text and the client rightly drops it; if mid-stream retry doubling ever appears, fix identity (new part id + `message.part.removed`), not the guards.
- The "freeze" itself was genuine provider latency (codex gpt-5.6-sol: 55 tokens over 14.3s ≈ 3.8 tok/s), unrelated to the doubling.
- Debug server store: `~/.local/state/neoism-dev/default/agent/agent.turso.db` (Turso/libsql; readable via sqlite3 CLI on a /tmp COPY only). Prod: `~/.local/state/neoism/agent.turso.db`.
- Residual: joined workspaces proxy to the HOST daemon — hosts running older builds still have the racy route until updated.

Related: [[bug-agent-trace-flash-vanish]], [[bug-agent-silent-stop]], [[project-agent-user-part-sync]]
