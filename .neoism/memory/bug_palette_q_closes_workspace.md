---
name: "Alt+P q closed more than active tab"
description: "Alt+P q must close only the active buffer tab"
type: "bug"
scope: "project"
origin: "Neoism coding session 2026-08-01"
created: "2026-08-01"
updated: "2026-08-01"
---

## Bug

In the Alt+P command palette, `q`/`:q!` parsed correctly to `GlobalExCommandPlan::CloseFocusedBufferTab`, but `router/route.rs` dispatched that plan to `Screen::close_split_or_tab()`. That helper falls back from closing a focused split route to closing the entire workspace/context, so q could kill more than the active tab.

The normal editor ex-command path already used the correct operation: `Screen::close_focused_buffer_tab()`, which resolves the active pane tab first and otherwise the active workspace buffer tab. Its tab target handling includes Markdown, Code, and Agent.

## Fix

Changed the Alt+P palette dispatch for `CloseFocusedBufferTab` to call `close_focused_buffer_tab()` directly. Added `palette_q_targets_only_the_focused_buffer_tab` coverage for `q` and `:q!` parsing.

## Verification

- Targeted test passed.
- `cargo check -p neoism` passed.
- `rustfmt --check` for route.rs and `git diff --check` passed.
