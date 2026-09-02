---
name: "Agent streaming freeze and catch-up"
description: "Desktop Agent token bursts were event-loop redraw starvation, not provider/SSE buffering; fixed with coalesced explicit winit wakes and independent animation phase."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-26"
updated: "2026-08-26"
---

## Fixed

Commit `8f1ad417917a862fb9aab31c38b068f97326b8ee` (`Restore live Agent streaming cadence`) pushed to `neoism_agent_v2` on 2026-08-26.

## Root causes

- Desktop SSE decoded and enqueued every provider delta immediately, but `std::sync::mpsc::Sender` did not wake winit.
- A transient `SessionIdle` removed the pane's old continuous redraw owner while the durable execution/branch remained active. Deltas accumulated until unrelated input caused a frame; the 512-update bounded drain then coalesced adjacent deltas and visibly caught up in a burst.
- `Crafting` and the bottom activity scanner incorrectly used cumulative `streaming_elapsed_seconds()` as animation phase. Model-time pauses froze them; later provider time made them jump.
- A new top-level prompt could inspect a stale unfinished execution before one last quiescence pass and temporarily inherit its previous total.

## Fix

- Every classified desktop SSE update now issues a window-specific `RioEvent::Render` after enqueue.
- Wake delivery is coalesced with an atomic pending gate; the pane clears it before draining so a racing delta schedules the next frame without flooding winit.
- The wake is attached before fresh-session prompt admission and retained on existing streams/window rendering.
- Durable unfinished execution, authoritative branch activity, and viewed-child activity remain continuous redraw owners across transient run-idle edges.
- Status word and scanner animation use the independent wrapped `now_seconds` animation clock; elapsed model time is display data only.
- New top-level admission runs `finish_if_quiescent` first and immediately publishes the new zeroed execution snapshot.
- Existing 512-update bound and adjacent-delta per-frame coalescing remain; they protect main-thread layout and no longer cause starvation.

## Verification

- Agent server full suite: 443 passed, 5 ignored.
- Shared UI full suite: 2106 passed.
- Focused desktop explicit-wake and redraw-owner tests passed.
- Desktop binary check passed.
- Real WASM target passed.
- Warning-denied Agent/shared check passed.
- Full desktop suite has unrelated pre-existing shared-global/environment failures; changed tests pass individually.
