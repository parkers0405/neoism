---
name: "Joined workspace icon changed to folder after switch"
description: "Joined workspace chain icon persists across workspace switching"
type: "bug"
scope: "project"
origin: "Neoism coding session 2026-08-01"
created: "2026-08-01"
updated: "2026-08-01"
---

## Bug

A joined workspace's Island tab could switch from the chain icon to the default folder icon after navigating to another workspace. `workspace_icon_kind_for_index()` retained the correct durable adopted binding, but `IslandContexts::title()` returned `None` whenever the workspace's asynchronous terminal title entry was temporarily absent during switching. The Island renderer treats a missing title as a generic workspace and never sees `icon_kind`, producing the folder fallback.

## Fix

`desktop/src/bridges/island.rs` now always returns an `IslandTabTitle` for a valid context index, using `~`/no program while terminal-title metadata is absent, and independently asks `workspace_icon_kind_for_index(index)` for durable workspace identity. Added `joined_workspace_icon_survives_missing_terminal_title` regression coverage.

## Verification

Changed-file `rustfmt --check` and `git diff --check` pass. Full cargo check/test was blocked by unrelated concurrent worktree edits duplicating `AgentClientMessage::StopBackgroundTask` and imports in neoism-agent-server; those files were not touched.
