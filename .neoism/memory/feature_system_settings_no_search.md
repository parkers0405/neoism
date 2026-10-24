---
name: "Searchless system settings"
description: "Per-system settings have no search field, caret, input hits, blank band or hidden query filtering; main list search remains."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

System settings details are intentionally searchless. Main Systems/Servers lists retain their search bars and returning from details restores the previous main-list query/selection. CommandPalette::has_query_input() is false for SystemSettings; rendering reserves header height only, paints no query text/caret, and text_entry_at returns false. Settings always return all rows; set_query/insert_query_text ignore input so there is no invisible filter. Scale changes and refresh keep the header-only band and empty query. Back arrow/Escape/empty Backspace continue navigation. Regression tests cover no search geometry/hits/hidden filtering, preserved list search, row alignment at several scales, and old query-aware tests updated. 125 palette tests, native/shared check --tests and wasm32 check passed. See feature_system_settings_and_ssh_workspace.md for the broader settings/SSH architecture.
