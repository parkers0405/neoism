---
name: "File tree grabs files outside its bounds - fixed"
description: "Desktop tree grabbed files from adjacent pane: hit test used natural not rendered width; stale drag ownership also leaked. Fixed resolved bounds, gesture cancellation, outside-drop policy; 56 tests and desktop check passed."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-14"
updated: "2026-10-14"
---

## Root causes

Desktop `file_tree_row_under_mouse()` used the tree's preferred/natural width (`hit_test`) rather than the left-sidebar host's resolved rendered width. A row could be returned for clicks in the adjacent pane and trusted even when `in_tree_bounds` was false. Drag motion also used window x without the tree's horizontal offset. Stale armed file drags survived lost releases/focus loss; motion consumed them without checking button ownership.

## Fix

`neoism-frontend/desktop/src/screen/bridges/file_tree/mouse.rs` now uses `hit_test_in_bounds` with `file_tree_bounds()` for press/context click/motion/release. Drag move requires the left button down and a hosted tree. `cancel_file_tree_drag()` clears gesture state and opened-on-press bookkeeping, wired into fresh press, button-up motion safeguards, and window focus loss. Shared `set_visible(false)` cancels drag and `toggle_visible()` delegates to it.

Shared `update_file_drag` now takes `inside_panel`: blank space inside the tree remains a workspace-root target, but leaving the panel clears the drop target and dwell clock while preserving an active drag for re-entry. Desktop release refreshes its target at the release point so stale hover cannot move files on outside drops.

## Verification

`cargo test -p neoism-ui --lib panels::file_tree -- --nocapture`: 56 passed, 0 failed. New tests cover rendered widths smaller/larger than preferred width, nonzero panel offsets and all four outside edges, leaving/re-entering the panel, and cancelled/hidden armed or live drags not reviving on motion. `cargo check -p neoism`: passed. `git diff --check`: passed. No interactive GUI validation performed.
