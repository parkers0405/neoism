---
name: "Final answer followed by stuck Tinkering"
description: "A delayed completed task part after authoritative SessionIdle resurrected parent Working/Tinkering forever; fixed with a per-session terminal idle latch."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-20"
updated: "2026-08-19"
---

# Final answer followed by stuck Tinkering

Desktop could show a completed final assistant answer while the composer remained stoppable and the status row stayed `Tinkering · … · tools` indefinitely.

## Exact cause
`Tinkering/tools` is the viewed session's own `NeoismAgentStreamingState::Working`, not the aggregate sub-agent or background-task state. A rare event order was:

1. Parent receives authoritative `SessionIdle`; GUI clears streaming and renders final answer.
2. A delayed root `PartUpdated`/`PartDelta` for the just-completed `task` tool arrives.
3. Root part ingest unconditionally calls `note_streaming_from_part(Tool)` / `refresh_streaming_from_tail()`.
4. Parent returns to `Working`, but no later idle edge exists, so the label and stop button remain forever.

This is especially visible after a sub-agent task completes near the parent final answer.

## Fix
Desktop now tracks `terminal_idle_sessions`, an authoritative per-session idle latch.
- `SessionIdle` and an idle runtime snapshot set the latch.
- Late transcript parts are still ingested/reconciled but cannot derive activity while latched.
- Real `busy` queue status, retry, dequeued prompt, local fresh prompt, or authoritative busy hydration clears the latch.
- Applies to active and off-screen cached root transcripts.
- Cache eviction removes old latches.

Regression: `late_completed_tool_part_cannot_resurrect_idle_parent_status` injects Working → SessionIdle → completed task PartUpdated and verifies the task card lands while status remains Idle.

Changed desktop files include `pane.rs`, `pane/ingest.rs`, `pane/session.rs`, `pane/submit.rs`, `pane/render_state.rs`, and `pane/tests.rs`.

LSP diagnostics and `git diff --check` are clean. Focused Cargo test is blocked by unrelated missing `neoism-agent-server/src/workflow.rs`.
