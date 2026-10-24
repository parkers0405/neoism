---
name: "Queued steers misplaced only during live Rust GUI stream - fixed"
description: "FIXED Rust live timeline sorting queued human steers by send-time client IDs; append at admission instead, preserve runtime notice ordering"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Queued steer messages jumped above assistant content during streaming

User screenshot: initial prompt then two admitted steers shown adjacent above assistant commentary that should precede them; finished conversation history refreshed into correct order. Rust desktop live-only bug, not server storage/history sorting.

Root: desktop pane.rs chronological_live_insert_index sorted incoming User rows against canonical parent message IDs. Queued prompts have client IDs allocated at SEND time, earlier than some assistant steps that are already visible when those steers finally dequeue. SSE user-part upsert used this helper and inserted steers before those assistant groups. Dequeue then deduped by ID without correcting slot. History uses authoritative snapshot positions, correcting itself at finish.

Fix: early return None for incoming.kind==User in desktop chronological_live_insert_index; human turns append in live admission order, existing optimistic/server echo merges keep position. Preserve ID-based delayed System/runtime notification ordering and last-human hard boundary for assistant output. Only one caller in desktop pane/ingest.rs. Desktop inactive-cache/shared live/dequeue paths already append/dedupe in place; no shared/server/history changes required.

Added desktop tests queued_steer_echo_follows_streamed_output_despite_older_client_id + queued_steer_dequeue_before_echo_preserves_live_and_snapshot_order, covering two older-ID steers, echo/dequeue permutations, continued commentary/reasoning, dedupe and snapshot agreement. Both PASS with cargo test -p neoism --bin neoism queued_steer_. Existing dequeued_prompt_ tests2, delayed_subagent_completion_stays_before_its_live_assistant_response1, history_refresh_keeps_server_order_for_late_user_and_reasoning1 also PASS via current compiled test binary. No release build/live UI/provider calls. Changes uncommitted. Test fixture server echo carries same author as dequeue; unrelated merge semantics for missing author were not changed.
