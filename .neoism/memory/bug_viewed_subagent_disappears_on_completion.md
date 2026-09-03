---
name: "Viewed subagent disappears on completion — FIXED"
description: "Live viewed subagent retained through completion until navigation; historical dead sessions remain filtered"
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-01"
updated: "2026-09-01"
---

2026-08 sidebar viewed subagent disappearing on completion FIXED. History: original viewed_session_id retention came in e30b6e9 (v0.7.21); execution-family pruning in 6738566 and later recovery/pruning changes af8f8e6/936070c bypassed it through retain_authoritative_branches, deleting an open completed child when the parent continuation replaced the execution. With only root left, Branches section hid, stranding user in child. Fix in shared agent pane side_panel.rs: added non-persisted retained_viewed_subagent_id latch. It arms only when currently viewed child is observed Active/WaitingPermission (on navigation into known-live row or live/recovery active edge), preserves that existing row/activity through authoritative snapshots and set_subagents omissions after terminal transition, and clears on viewed-session change. subagent_hidden now uses latch rather than raw viewed id, so merely opening/loading a historical completed session cannot retain/resurrect it. Tests cover live viewed completion surviving empty authoritative reconciliation until leaving, and viewed historical completion remaining filtered. Verified cargo check -p neoism-ui -p neoism --tests and both focused tests pass; warnings pre-existing.
