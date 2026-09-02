---
name: "Completed subagent sidebar resurrection"
description: "Unversioned busy/running events can no longer resurrect completed sidebar branches"
type: "bug"
scope: "project"
origin: "session implementation"
created: "2026-08-28"
updated: "2026-08-28"
---

Fixed a frontend branch lifecycle resurrection where completed subagents could later show `working`/spin again.

Root cause: delayed unversioned active evidence (`session.status busy/retry`, Task part `running`, generic `SubagentUpdate::Running`, status polls) was routed through `note_subagent_runtime`/`AuthoritativeRun`, clearing `terminal_locked`, restoring active IDs, and rewriting Task cards. These events have no execution ID or family revision and can replay after a newer authoritative completed runtime snapshot. Durable server `execution_subtasks` state was correct.

Fix:
- Added `note_subagent_observed_runtime` in shared and desktop panes. Terminal observations remain authoritative; active/waiting observations route through terminal-lock-aware part/ancillary activity.
- Desktop SubagentStatus ingestion only updates child runtime, Task card, and viewed state when the guarded transition is accepted.
- Desktop status hydration uses the guarded observed path and only writes running Task state when accepted.
- Shared/web `note_subagent_event` uses the same guarded path; stale generic Running no longer calls unguarded `set_branch_activity_tool` first.
- Shared task reconciliation gives latched branch state precedence over unversioned row status.
- Monotonic `apply_branch_lifecycle_snapshot` now applies accepted higher-family-revision rows authoritatively, so a real new execution can reopen the same child ID.

Regression sequence: family rev 8 completes child; delayed Busy/Running is rejected and branch remains locked/inactive; family rev 9 `outstanding` reopens it. Shared and desktop focused tests pass, prior permission-straggler tests pass, and `cargo check -p neoism-ui -p neoism` passes.
