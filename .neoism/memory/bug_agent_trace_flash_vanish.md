---
name: bug-agent-trace-flash-vanish
description: Tool/task rows flashed into main chat then vanished live after visiting a sub-agent — FIXED 2026-08-19
metadata:
  type: project
---

Returning from a sub-agent chat, tool/task/background-completion blocks appeared
in the main timeline and then disappeared while being looked at. Nothing
cross-session was appended — desktop part ingest is correctly scoped. It was the
live-trace VISIBILITY MASK flipping (`timeline_message_visibility`: Reasoning /
Tool / Subtask / Compaction rows show only at `index >= live_trace_start`, so
settled turns are prompts+text only).

Two independent causes, both fixed in the desktop `pane/ingest.rs` AND its shared
twin `agent_pane/state/{ingest,timeline}.rs`:

1. **Flash** — `upsert_part_message` called `retain_current_turn_trace()` for ANY
   tool-ish part, so a LATE part landing in a settled session (background-task
   completion card, or a task card flipping to `completed`) un-hid that whole
   turn's trace. Fix = gate on `self.is_streaming()`. Safe because the live path
   calls `note_streaming_from_part` right after, which opens the window
   unconditionally; and the completion card is already exempt from the mask via
   `is_background_completion_result_card`.
2. **Vanish** — `rebase_current_turn_trace` (run after the idle refresh replaces
   the transcript with only the last 80 messages) fell back to re-anchoring at the
   LAST user message whenever the anchor id wasn't found, jumping the boundary to
   the tail and re-masking rows that were on screen a frame earlier. Fix = only
   the OPTIMISTIC anchor (empty id, no durable id yet) re-anchors; a durable
   anchor that is missing means its turn is older than everything loaded, so the
   window opens at 0. This restores the method's own documented contract: the
   trace collapses on leave/re-enter, never underneath a visit.

3. **The "Subagent finished" block itself vanishing** (the one the user
   screenshotted) is a THIRD, separate cause. Subtask completion mapped to
   `agent_message_system("Subagent", text)`, and
   `timeline_message_visibility` renders a System row ONLY when its tool is
   `location_notice` - so it was hidden outright on the next transcript
   rebuild. Background-task completion already had the right pattern: a
   durable Tool card (`background_task_result` / id `background-task-{job}`)
   exempted from the mask. Fix = give subagent completion the twin
   treatment - `subtask_completion_card` → tool `subagent_result`, id
   `subagent-task-{task_id}`, exempted via
   `is_subagent_completion_result_card`, produced identically by BOTH
   `part_block` (live) and `message_blocks` (history) so a refresh replaces
   in place instead of wiping.
   GOTCHA: both completion cards must `return` EARLY from `part_block` - its
   tail rewrites `message.id` to the parent message id for any user-role
   text part, which silently clobbers the durable card identity.

**How to apply:** when a timeline row appears then disappears, suspect the
live-trace window OR a kind that the visibility mask hides (System is hidden
unless `location_notice`), not cross-session part leakage. Live and history
mappings must agree on kind AND id.

OPEN (separate, web-only, NOT fixed): `wasm/src/rendered/catalog.rs`
`apply_agent_event_to_pane` applies parts with the event's session id discarded;
the only gate (`agent.rs:should_apply_agent_event`) compares against
`agent_state.session_id`/`requested_session_id`, which lag the pane's immediate
local switch, so events landing in that gap write into the wrong timeline and are
then erased by the next `HistoryChunk`. Daemon compounds it — `agent/events.rs`
re-stamps a session-less event with the bound (parent) session id.

Related: [[bug-subagent-recursion-timeline-wipe]], [[perf-agent-pagination]],
[[bug-agent-pane-web]]
