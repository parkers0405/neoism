---
name: "Virtual Markdown spelling diagnostics"
description: "Active virtual Markdown spell diagnostics, shared menu integration, global custom dictionary, session ignore, table hit-testing, and stale-edit safety"
type: "feature"
scope: "project"
origin: "Implemented during golden-standard Markdown spelling/LSP menu work"
created: "2026-08-17"
updated: "2026-08-17"
---

The active Markdown renderer is `neoism-frontend/shared/src/editor/markdown/render/virtualized.rs` plus included `virtualized/inline_layout.rs`; legacy spell squiggles in `render/inline.rs` do not automatically appear there. Active prose squiggles are emitted per measured inline run, checking Normal/Bold/Italic and excluding Code/Link/Tag. Tables use `render/table.rs` and require their own decoration call plus `table_cell_rects` hit-testing in `MarkdownPane::spelling_word_at`. Desktop spelling suggestions use the shared `ContextMenu`, which already owns mouse hover/click, wheel scrolling, keyboard navigation, clipping, and scrollbar. `ContextMenuAction::MarkdownSpellingReplace` carries the expected source word so stale menu actions are rejected before edit application.

Global overrides live in `spellcheck.rs`: `Ignore` inserts a normalized word into a process-wide session set; `Add to Dictionary` appends the normalized word once to `${XDG_CONFIG_HOME}/neoism/dictionary.txt` (via `dirs::config_dir`) and updates the in-memory set immediately. Both are represented as Markdown spelling context-menu actions, and the menu still opens when there are no replacement candidates.
