---
name: "Theme picker typing panic — FIXED"
description: "Theme input panic from stale byte cursor after enter_themes_mode cleared query; reproduced and reset cursor, shared all-platform fix"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-06"
updated: "2026-09-06"
---

## Cause and fix
Desktop opens theme selector in `screen/bridges/palette.rs::open_theme_picker` via shared `CommandPalette::enter_themes_mode`. That cleared `query` but retained `query_cursor` from typed ThemePicker/Themes or an earlier closed picker session. Desktop `router/route.rs:1646` calls `insert_query_text`, whose `String::insert_str(at, text)` panics with `assertion failed: self.is_char_boundary(idx)` when at > empty query length. Reproduced before fix with native neoism-ui unit test at state.rs:730. Arrows only manipulate row selection, so work.
Fix scoped to themes: reset query_cursor=0 alongside query.clear in shared panels/command_palette/state.rs. Tests in command_palette/tests.rs cover prior command, reopen, first char, Unicode/grapheme backspace, empty backspace, no results, incremental query, filtered selection/navigation, and IdeTheme resolution for preview.
Preview render.rs uses get_selected_theme (safe filtered_rows().get(selected_index)) and IdeTheme::by_name; typing resets selection/scroll and requests redraw only. Desktop apply_unified_theme in screen/chrome_geom.rs executes on Enter/click, persists config and updates terminal colors; not on failing input path. No mutex/hot-reload fix needed.
Pre-fix regression failed 0/1; after fix palette suite passed 98/98; Windows desktop cargo-xwin check passed. No release/commit/tag/push. Existing local Windows modal-anchor changes in host/run.rs and shared chrome_policy.rs left untouched.
