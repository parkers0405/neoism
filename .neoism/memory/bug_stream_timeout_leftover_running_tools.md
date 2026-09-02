---
name: "Stream timeout leftover running tools"
description: "OpenCode does not retry stream timeouts; it settles leftover running tools and open reasoning. Neoism now does the same on every fatal stream error."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-20"
updated: "2026-08-20"
---

OpenCode v2 does **not** retry or re-prompt a mid-turn provider stream timeout. Recovery is halt + cleanup + stop.

OpenCode `SessionProcessor.cleanup()` always:
1. closes open text/reasoning (`time.end`)
2. waits 250ms for in-flight tools
3. force-settles leftover pending/running tools to `error` + "Tool execution aborted" + `interrupted: true`

Retry is only for classified transient API errors (5xx / ECONNRESET / rate-limit). A timeout is `UnknownError` and is not retryable. Parent continues after a settled child result, not by retrying the dead child stream.

Neoism gap: `finish_provider_stream_with_error` only called `mark_interrupted_tool_parts` on user abort (`"Session aborted"`). A 120s idle timeout / provider error left tools `running` and reasoning without `time.end`. That is the stale sub-agent chat.

Fix: every fatal/abort path now settles leftover tools + open reasoning, matching OpenCode cleanup. Do not add retry/re-prompt for this class.

Test: `fatal_stream_cleanup_settles_running_tool_and_open_reasoning`.
