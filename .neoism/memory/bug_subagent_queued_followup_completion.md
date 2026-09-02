---
name: "Subagent queued follow-up completion race"
description: "Model subagent follow-ups now use human-style steer and final completion waits for true child idle"
type: "bug"
scope: "project"
origin: "user report and direct fix"
created: "2026-08-21"
updated: "2026-08-21"
---

Fixed 2026-08-21 (working tree after v0.7.53). Symptom: main model sends `task(task_id=..., prompt=...)` while a child is running; prompt queues, child current turn finishes, UI may terminalize it, queued follow-up drains but parent never receives final `Subagent finished` notification.

Root cause: subagent completion used a whole-SessionInfo `subtaskNotifyOnIdle` marker as the only deferred worker-exit obligation. Concurrent completion/marker read-modify-write could erase it. Initial background wrapper also emitted `session.subtask.completed` without checking child queue/worker activity, terminal-locking the frontend too early.

Fix is deliberately subagent-lifecycle-only; regular main-agent queue semantics are unchanged. `publish_background_subtask_finished` now publishes only at true child quiescence (no run, worker, or queued prompts), and clears the marker in the same write that persists completion. The generic drain worker only records whether it consumed a child prompt; at worker exit this in-memory obligation forces child completion even if the durable marker was lost. Startup reconciliation scans marker-only idle children as well as pending completion outbox entries.

User requested model follow-ups behave exactly like human-to-main steering. `task(task_id=...)` continuation delivery changed from `queue` to `steer`: the active child absorbs it at its next provider-step boundary via the existing in-run steering path. If the run ends before absorption, the durable row still falls back to the unchanged worker as a new turn. No changes to queue ordering, wake/coalescing, conflict requeue, or append_prompt.

Files: `neoism-agent/crates/neoism-agent-server/src/session_actions.rs`, `session_queue.rs`, `tool_runtime.rs`, `tests_session_queue.rs`.

Regressions: `queued_child_prompt_defers_terminal_event_and_survives_lost_marker` simulates stale whole-session overwrite and verifies no premature completion plus exactly one parent notification after drain; marker-only startup recovery test. Existing active-run steer, deferred completion, parent busy queue ordering, sibling batching/abort, and subtask tests pass.
