---
name: "Alt+S initial file list and input chevron"
description: "Alt+S lists files on empty open; removed file-input chevron; regression matrix added"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

Alt+S file picker uses shared `neoism-frontend/shared/src/panels/finder`. Empty-query opening/reopening cleared both query and `last_executed_query`, so `tick` skipped refreshing because both keys were empty. Fixed Files branch to refresh on every due dirty tick (still no idle-frame searching), and empty Files queries bypass the 75ms debounce. `render.rs` no longer prepends the decorative `› ` to Files input; other mode prefixes remain. Empty-project hint is now `No files found`, not `Start typing to search files…`. Added unit regression matrix in `search.rs` for fuzzy/exact and service/fallback paths: open, cached reopen, clear query, symbols-to-files switch, no duplicate idle search, no empty grep dispatch. Test run initially blocked by unrelated existing agent-pane test referencing missing ConfigChipTransition::Settling at state/tests.rs:2341.
