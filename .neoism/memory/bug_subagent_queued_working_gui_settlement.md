---
name: "Subagent queued working GUI settlement"
description: "All GUI and server subagent completion surfaces now converge terminal"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-08-30"
---

Expanded subagent completion split-brain fix after GUI audit. Visible bug was stale queued/running/working (not merely hidden DB failed): parent received result but task card, sidebar, viewed child Tinkering, and footer remained active. Server correction: generic quiescence cannot terminalize outstanding; explicit finish returns errors and gates parent delivery; always republishes runtime snapshot and SESSION_SUBTASK_COMPLETED on retry. GUI corrections: apply_branch_lifecycle_snapshot must reconcile task message BEFORE pruning terminal branch evidence (previous after-prune call was ineffective); desktop calls reconcile_viewed_subagent_runtime for terminal snapshot; shared forces viewed child streaming Idle; task marker rewrite handles queued/working/busy in addition to running. Added desktop end-to-end tests asserting terminal runtime snapshot changes queued task card to completed, active count/footer zero, sidebar child absent, and viewed child Idle; added shared equivalent. All focused server/repeated-completion/desktop/shared tests, checks, LSP and diff pass. v0.7.72 remains affected; fix uncommitted/unreleased.
