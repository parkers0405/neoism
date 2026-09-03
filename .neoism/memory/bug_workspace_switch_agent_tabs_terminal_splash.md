---
name: "Workspace switch Agent tabs become terminal splash — FIXED"
description: "Stale workspace switch ack caused Agent tabs to render terminal splash; fixed by treating ack as ack-only"
type: "bug"
scope: "project"
origin: "session"
created: "2026-08-01"
updated: "2026-08-01"
---

2026-08 workspace-switch agent tabs becoming terminal splash FIXED. Root cause: every UI workspace selection sends async SwitchHostWorkspace; daemon returns HostWorkspaceChanged only to that sender. ContextManager ingest treated this acknowledgement as a fresh selection and called select_tab directly. Reordered A -> B -> A acknowledgements could change current_index behind Screen, bypassing save/load workspace chrome and Sugarloaf visibility. Live buffer tabs from A were then paired with B's grid; Agent route lookup failed in B and the remaining root terminal rendered its splash. Fix: HostWorkspaceChanged ingest is acknowledgement-only and refreshes tree, never changes local selected grid; removed switch_local_context_to_daemon_workspace; corrected protocol comment. Regression test simulates A -> B -> A with stale B ack. Verified cargo check -p neoism --tests and focused test pass. Files: desktop context/manager/ingest.rs, daemon_link.rs, test.rs; protocol workspace/server_message.rs.
