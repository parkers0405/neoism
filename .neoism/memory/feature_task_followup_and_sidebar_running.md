---
name: "Task follow-ups and running chat indicators"
description: "Task continuations labeled Follow-up with validated child identity; Alt+C running chats use timeline's exact white stepped squares, viewport-owned redraws and authoritative web catalog busy state."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

# Task follow-up presentation and running Conversations indicator

A task follow-up normally resumes or queues work in the same child session (`input.task_id`), but shared API projection formerly displayed it just like a new Task launch. `api_mapping.rs` now produces `Task(@agent · description)` for launches and `Follow-up(@agent · description)` for continuations. Nonempty input task_id is provisional before resolution; authoritative state/part metadata sessionId (then output task_id) wins. If nonexistent requested ID falls back to a different new child, label Task, not Follow-up. Tool remains task; distinct invocation/part IDs, chronological positions, output markers, status lifecycle and click navigation are preserved. Existing title_text compaction provides consistent spacing. Do not merge message identities just to visually group follow-ups; same-task cache equivalence has separate lifecycle risks.

Alt+C Conversations sidebar and child-detail rows now use timeline's literal white square-chase: four unrounded 2*s squares with alphas [1,.65,.4,.2], eight stepped positions around 3*s orbit. Shared geometry/opacity lives in view/draw.rs; timeline tool render and sidebar draw both consume it. Replaced sidebar's pastel rounded-dot/glow spinner. Only BranchStatus::Active runs; waiting/blocked/terminal retain existing treatment. Sidebar owns redraws via an independent visible_running_indicator flag, reset per paint/hiding/layout removal and set only when visible nonoccluded squares are painted. Timeline's visible_running_tool_active must not be shared because timeline resets it independently. Preserve other loading/waiting/scroll/hover owners.

Web ThreadSummary.busy previously always false. Daemon agent/handlers.rs now performs one bounded 2-second /v2/sessions/status fetch per catalog request and derives busy from authoritative snapshot, reused by selected-session hydration. Wasm rendered/agent.rs refreshes catalog on MessageStart, SessionIdle and StreamingState lifecycle edges (plus existing catalog mutations), not token/tool-delta events. Desktop uses shared sidebar rendering and existing status catalog mapping. Terminal-lock logic and wrapped epoch clocks unchanged.

Verification: task_projection 4 passed, compact_task_followup 1 passed, running_ 26 passed, square_chase 1 passed, view sidebar suite with historical-return test skipped 48 passed. Full side_panel filter 58 passed/1 failed in viewed_historical_child_keeps_return_navigation_until_leaving (expected stopped, normalized error); no terminal logic changed here. Daemon busy-mapping unit test blocked by E0425 in existing agent/tests.rs due inaccessible todo_items_from_response helper. Native/shared/daemon cargo check, wasm32/web cargo check, git diff --check pass. No release build or live GPU visual validation. Related: [[feature_agent_tool_task_indicators]], [[feature_multi_agent_conversation_sidebar]], [[bug_viewed_subagent_disappears_on_completion]].
