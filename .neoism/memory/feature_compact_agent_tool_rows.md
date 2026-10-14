---
name: "Compact agent tool rows"
description: "Compact ordinary tool rows with status dots/icons and one expanded connector; live edits and patches are special and retain their visible diff cards."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

# Compact agent tool rows

Ordinary tool calls in the shared renderer collapse to one fixed 30px (scaled) header row, for live and archived calls. Header shows the existing status dot plus a per-tool symbol: arrow for Read, terminal for Bash/shell, magnifier for search, pencil for edit, gear fallback for generic/MCP tools. Titles omit the completed suffix and outer argument parentheses; long headers use shaped-width truncation. Pending/running/streaming/error labels reserve space on the right so they remain visible.

Expanded generic details draw exactly one curved connector. Generic details retain the existing 12-row preview cap. Todo output retains its dedicated checklist. Shared desktop/web paths use the same implementation.

User correction: edits/apply_patch/write/multiedit are SPECIAL. Live edit tools with actual diff data display the existing diff cards by default, without expanding the parent row. Cards retain their original full width, syntax highlighting, per-file header toggles, links, scrolling and diagnostics. They do not use the generic text-detail connector. Archived edit parents remain collapsed until clicked. Unsettled apply_patch parsing is still deferred. The shared `show_tool_diff_cards(tool, status, expanded, archived)` policy is used in measurement, rendering and timeline diff preparation.

Files: `neoism-frontend/shared/src/panels/agent_pane/view/tool_message.rs`, `tool_message/render.rs`, `tool_message/widgets.rs`, `tool_message/diff.rs`, `message_card.rs`, and `timeline/layout.rs`. Ordinary collapsed measurement skips header shaping and body wrapping; edit cards intentionally keep their specialized measurement. Removed the separate Code-output measurement branch that reserved height inconsistent with tool rendering. Lazy ordinary-tool estimates use the 30px constant. No runtime performance benchmark or GUI verification performed.

Read grouping requires special interaction handling; see [[bug_read_group_expand_coupling]] for the stable source/child IDs, animation remeasurement and click-target fix.
