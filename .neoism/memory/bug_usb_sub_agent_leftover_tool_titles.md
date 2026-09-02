---
name: "USB/sub-agent leftover tool titles"
description: "Leave-and-return from a sub-agent chat collapsed live-trace but restored the parked parent layout, so leftover tool titles stayed clickable until the next click."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-19"
updated: "2026-08-19"
---

When you enter a sub-agent chat and come back, the parent transcript is supposed to settle to prompt+answer only. Tool/reasoning rows are hidden by `timeline_message_visibility` once `timeline_live_trace_start` is cleared on session switch.

What actually happened: `activate_cached_session` restored the parked parent `timeline_layout_cache` from the previous visit. That cache still contained the live tool rows. The renderer painted those leftover titles (header-only archived cards). Clicking one hit the stale tool hit-rect / expand path, which invalidated layout and made the remnant disappear.

Fix: after restoring the parked parent transcript, drop the parked layout (`invalidate_timeline_layout`) instead of reinstalling it. Live-trace stays cleared, so the next layout pass re-masks settled tools.

Files:
- `neoism-frontend/shared/src/panels/agent_pane/state/session_cache.rs`
- `neoism-frontend/desktop/src/neoism/agent/commands.rs`
- test: `settled_tool_titles_hide_after_subagent_round_trip`

Related: [[bug_subagent_recursion_timeline_wipe]] (the hide-on-leave design). Not USB-specific — "usb agent" was the sub-agent chat.
