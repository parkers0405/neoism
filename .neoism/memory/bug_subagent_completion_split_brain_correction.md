---
name: "Subagent completion split-brain correction"
description: "Subagent completion split brain fixed; generic quiescence cannot kill outstanding branches"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-08-30"
---

Critical correction after v0.7.72: v0.7.69 generic quiescence orphan reconciliation was invalid. It terminalized any durable outstanding branch when no run/queue/worker/provider segment/job was visible, but legitimate subagent launch and between-step windows have exactly that shape. This could mutate branch to failed between durable admission and child run; later real completion saw changed=false, published no runtime snapshot, and parent completion delivery still succeeded, creating split brain: parent finished while task card/sidebar/child/footer stayed running. Fix: generic finish_if_quiescent once again treats outstanding branch as authority and returns; only explicit completion/abort terminalizes. finish_subtask_for_child now returns errors; parent completion publication aborts if lifecycle commit fails; terminal completion always republishes authoritative family snapshot even on idempotent retry and re-emits SESSION_SUBTASK_COMPLETED regardless of completion-record creation. Desktop/shared apply_branch_lifecycle_snapshot now reconcile parent task message statuses too. Updated regression asserts quiescence never terminalizes outstanding after provider segment ends. Server repeated delivery, shared/desktop recovery tests, checks pass. This supersedes memory bug_orphaned_subagent_runtime_reconciliation's server-side generic orphan terminalization; frontend recovery defense remains valid.
