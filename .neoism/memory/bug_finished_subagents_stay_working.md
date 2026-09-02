---
name: "Finished subagents stay working"
description: "Finished sub-agents stayed Active in the parent GUI because omitted `/session/status` entries were treated as unknown instead of idle."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-19"
updated: "2026-08-19"
---

Finished sub-agents can stay marked `working` in the parent GUI even though the children themselves are idle.

Cause: `/session/status` is the live run set (idle children are omitted). The GUI treated omission as unknown and kept the last Active. After a missed terminal SSE, `active_subagent_count()` stayed > 0, so the footer stayed on `Sub-agents working`, the sidebar dots stayed `working`, the 975m clock never cleared, and the composer kept the stop square. The parent transcript saying "Stopped. No subagents are running." was telling the truth.

Fix:
- Successful status snapshot: tracked children omitted from the map settle to Completed.
- Abort/force-stop: locally settle every tracked child to Stopped and clear the waiting clock.
- Reconnect + recovery snapshot: listed/known children omitted from a successful `/session/status` are idle, not unknown.

A failed snapshot still preserves live rows. A genuinely busy child still present in the map stays Active.

Tests: `abort_settles_stale_working_subagents`, `omitted_child_from_runtime_status_snapshot_settles_working_latch`, `status_omission_for_present_child_settles_live_activity`.

Not USB-specific. Same "usb agent" = sub-agent naming.
