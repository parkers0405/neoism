---
name: "Native tool status indicators and Task navigation"
description: "Native task rows: consistent dots, white stepped square-chase spinner, click Task to sidebar-equivalent subagent session"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

Native compact tool rows in neoism-frontend/shared/src/panels/agent_pane/view/tool_message/render.rs previously gave only completed (green) status dots a 1.65x halo, making them look larger than running/pending yellow dots. Removed the completed-only halo. Running/streaming rows now replace the gutter dot with four white 2px squares chasing around eight discrete square-perimeter stops, using the existing precision-safe now_seconds clock threaded through the message-card tool delegate. visible_running_tool_active is reset at the beginning of timeline render and marked only when a spinner square is actually visible; shared and desktop animation_reason use it so off-screen rows do not own continuous redraws. Both shared state/hit_rects.rs and desktop pane/render_state.rs toggle_tool_at resolve exact Task-row IDs via message_policy::task_id_from_text(detail, text) and call switch_session, same navigation as sidebar; missing child IDs retain normal expansion, diff and group child clicks unchanged. Added geometry, animation-owner, navigation, fallback and child round-trip regression tests. cargo check -p neoism-ui -p neoism passes. --tests check is blocked by unrelated existing shared integration test fixtures: missing PanelContext.plugins in shared/tests/file_tree.rs and git_diff.rs, and missing GitDiffIo fetch/pull/push in host_git_panel.rs. Unit test targets type-check; no tests executed and no release build.
