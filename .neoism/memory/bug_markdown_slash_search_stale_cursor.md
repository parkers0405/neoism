---
name: "markdown-slash-search-stale-cursor-crash"
description: "Normal-mode / or ? could panic on first query character because CommandPalette query.clear() did not reset query_cursor"
type: "bug"
scope: "project"
origin: "agent investigation and coredump analysis"
created: "2026-10-02"
updated: "2026-10-02"
---

---
name: markdown-slash-search-stale-cursor-crash
description: Normal-mode / or ? could panic on first query character because CommandPalette query.clear() did not reset query_cursor
type: bug
scope: project
status: fixed
updated: 2026-10-02
---

# Markdown `/` search stale cursor crash — FIXED

## Symptom

In a Markdown document, typing `/` in Normal mode and then the first search character could completely crash Neoism. The coredump assertion was `assertion failed: self.is_char_boundary(idx)`.

## Root cause

Markdown Normal-mode `/` enters the shared `CommandPalette` Search mode. `enter_search_mode_with_direction` cleared `query` but did not reset the byte-based `query_cursor`. If a prior command-palette query left `query_cursor > 0`, the next search character called `String::insert_str` at that stale index against an empty string and panicked.

The Markdown insert-mode slash block menu was not involved.

## Fix

In `neoism-frontend/shared/src/panels/command_palette/state.rs`, added `clear_query()`, which clears text and resets `query_cursor` to zero. All palette paths that clear the query now use it: normal open, Ex, Search, Fonts, Themes, Mashups, Shaders, Buffers, Servers, and Workspaces.

Regression test `search_modes_reset_stale_query_cursor_before_typing` in `command_palette/tests.rs` covers both `/` and `?` after a stale prior query.

## Verification

- `cargo check -p neoism-ui` passes.
- `cargo test -p neoism-ui --lib search_modes_reset_stale_query_cursor_before_typing` passes.
- Default package test remains blocked by unrelated `shared/tests/buffer_tabs.rs` missing the newer `PanelContext.plugins` field.
- Formatter checks expose unrelated pre-existing formatting drift; changed code itself follows rustfmt output.
