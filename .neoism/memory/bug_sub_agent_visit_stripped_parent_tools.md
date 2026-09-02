---
name: "Sub-agent visit stripped parent tools"
description: "Parent tools remain visible across parent/child/sibling navigation and settle only when leaving the whole conversation family."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-20"
updated: "2026-08-19"
---

# Sub-agent visit stripped parent tools

Desktop treated parent→child navigation like leaving the conversation entirely. `activate_cached_session` correctly computed `stays_in_family`, but only used it to retain the sub-agent roster. It still called `reset_timeline_navigation_for_session_switch()` unconditionally, and `CachedAgentSession` did not store `timeline_live_trace_start`/anchor. Returning to the parent therefore applied the settled prompt/answer-only mask and hid tools the user had just been viewing.

Fix:
- `CachedAgentSession` now parks the live-trace start and durable user-message anchor.
- Parent↔child↔sibling navigation within one conversation family caches/restores that trace.
- Switching to an unrelated conversation family caches no trace, so returning later still gets the intended settled clean view.
- Transient hover/selection/layout state remains reset; only visibility provenance is preserved.

Regressions:
- `subagent_visit_preserves_parent_live_tool_trace`
- `leaving_conversation_family_settles_live_tool_trace`

Relevant files: desktop `agent/commands.rs`, `agent/pane.rs`, `agent/pane/ingest.rs`, `agent/pane/tests.rs`. LSP diagnostics and `git diff --check` clean; full desktop test remains blocked by unrelated missing agent-server `workflow.rs`.
