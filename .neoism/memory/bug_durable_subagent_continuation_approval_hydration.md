---
name: "Durable subagent continuation + approval hydration"
description: "FIXED: terminal parent executions no longer strand resumed task_id work; E2 admission, repeated completion wakeups, and family-scoped durable approval hydration validated."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-31"
updated: "2026-08-01"
---

# Durable subagent continuation + approval hydration — FIXED

## Root cause
A parent execution E1 could become terminal before a background child completion notification reached the parent. The trusted runtime notification then ran against terminal E1, and later Task calls failed with `execution ... is no longer active`. Pending permissions/questions were also treated too much like transient events in direct desktop mode.

## Fix
- Trusted root runtime notifications are execution-admitting turns: join an active execution or mint E2; never reactivate terminal E1.
- Explicit `task_id` continuation uses `SubtaskAdmissionGuard::admit_continuation`, reusing the durable child session while attaching its reopened branch to the current execution.
- Ordinary duplicate admission still rejects terminal branches.
- Background admission occurs before the notify-on-idle marker is mutated.
- Every background completion generation durably queues a fresh parent runtime notification.
- Direct desktop activation and SSE reconnect fetch authoritative `/v2/interactions/permissions` and `/v2/interactions/questions` snapshots with runtime-revision race guards.
- Approval/question cards persist across main/child/sibling navigation within one root family, are cleared when switching unrelated families, and render standalone in subagent views because prompt-picker rendering is outside the composer visibility gate.
- Permission list payloads are enriched with child source session/title/agent metadata.
- Product settings paths use grouped kebab-case; stale `agent.dangerouslySkipPermissions` was removed from user config, retaining `agent.dangerously-skip-permissions`.

## Validation
- `task_tool_resumes_existing_child_session`: same child ID across E1→E2, E1 terminal, E2 branch ownership, distinct completion generations, fresh parent queue wake.
- `same_child_can_deliver_two_sequential_completions` passes.
- `cancelled_subtask_completion_keeps_admission_armed_for_retry` passes.
- Full `neoism-agent-server --lib`: 490 passed, 0 failed, 5 ignored.
- Shared permission/question policy tests pass.
- Desktop modified-file diagnostics are clean; full desktop check is blocked by unrelated `embedded_daemon.rs` calling `ensure_agent_server_started()` without its new `WorkspaceManager` argument.

## Important non-fix
The observed SKILL.md ApplyPatch hang was a permission/config issue before file write, not a formatter hang. Formatter timeout changes were intentionally removed from this patch.
