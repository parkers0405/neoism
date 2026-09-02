---
name: "Repeated subagent completion notifications fixed"
description: "Reused child sessions can now notify the parent after every completion via unique durable completion records acknowledged after delivery."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-23"
updated: "2026-08-22"
---

Fixed 2026-08-22. A child session reused through `task(task_id=...)` could notify its parent only once because runtime notification IDs were derived solely from the child session ID. Later completions collided in parent history. New completions are persisted as append-only `subtaskCompletions` records, each with a unique stable MessageId used by the parent prompt. Records remain pending through durable queue admission and are acknowledged only after `append_prompt` succeeds; stable IDs make recovery retries idempotent. Legacy singular `subtaskCompletion` records remain recoverable. Focused regression proves two sequential completions from one child deliver distinct notifications. Files: server `session_actions.rs`, `session_queue.rs`, `tests_session_queue.rs`.
