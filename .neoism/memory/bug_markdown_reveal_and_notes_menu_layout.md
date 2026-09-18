---
name: "Markdown reveal overlap and Notes/menu layout fixes"
description: "Fixed raw/rendered reveal height mismatch, table hover wash, Notes partial glyph clipping/background menu/direct notebook creation, slash trigger/dismissal/bounds/scroll."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-16"
updated: "2026-09-16"
---

# Markdown reveal overlap, Notes clipping, slash menus

User screenshots: linked-image source wraps three rows when Insert reveals source, following paragraph overlaps last row; normal/other-line wrapping inconsistent. Root causes in shared renderer: measured_cursor_line only invalidated when caret line moved, not mode change; hold-arrow suppression measured rendered text but still drew raw source.

Fixes:
- MarkdownVirtualRenderState.measured_reveal_line tracks effective raw-source reveal. surface.rs invalidates old/new nodes on every effective reveal transition (including Insert/Normal on same line).
- reveals_source_line includes cursor_reveal_active; cursor_reveal_active now checks suppression flag only, not elapsed time midway through a frame. tick_scroll lifts suppression and marks dirty at frame boundary. Measurement and drawing must ALWAYS use same reveal state. This supersedes old advice that drawing should still show raw while measured height stays frozen.
- widgets/markdown.rs parse_markdown_link balances nested/escaped label brackets; linked images `[![alt](image)](url)` consume outer link correctly. source_map + virtual inline_runs project image alt labels consistently. `![Neoism](GitHub repo URL)` is image syntax pointing at HTML, not a normal hyperlink; normal syntax is `[Neoism](...)`.
- Table renderer no longer calls whole-block background chrome on hover/caret; block action handles and cell/selection affordances remain.

Notes fixes:
- shared NotesSidebar row text/icons use list_clip = list band (not whole panel). Sugarloaf glyph clip preserves partial rows in scroll spring; no painted masks/whole-row hiding. Row/icon hit rects clipped too, so hidden parts can't steal header/footer input. Cursor rect clipped to list; tiny list heights max(0), not forced rowheight.
- NotesSidebar stores panel_rect and exposes contains_point. Desktop right-click accepts empty panel background and targets viewed vault; no-linked-vault falls back to vault menu.
- NotesNewNotebook replaces NotesPromptNewNotebook. It directly calls create_documentation_notebook_in(clicked dir), shared NotebookBinding::create_untitled_in reserves Untitled Notebook / numbered folder with atomic create_dir, seeds/opens it, no path modal. Palette create-from-folder still legitimately asks a path. docs updated.

Slash menu fixes:
- slash_block_query_before_cursor only allows slash at an otherwise blank/indented paragraph, at EOL, ASCII word query; excludes prose, links/paths, backticks/code fences, suffix text, whitespace/punctuation. Desktop and shared dispatch use it before opening; remove_slash_trigger uses same guard.
- Unmatched query closes popup (removed disabled No matches fallback). Escape preserves typed source. Desktop caret-navigation/modified shortcuts dismiss+pass through, outside click dismisses+passes through, outside wheel dismisses. Web caret-nav dismissal updated too.
- ContextMenu accent side stripe removed, rounded neutral border added. set_viewport/layout_in_bounds reserves actual editor bounds, shrinks visible rows above/below caret, closes if no room. Desktop uses Markdown viewport + pane rect each frame; shared chrome uses terminal content rect. Cannot spill into top tab/workspace chrome.
- Menu::scroll_pixels previously called ensure_selection_visible and immediately undid wheel movement. Now clamps selection to visible rows after wheel. set_max_visible no-ops when unchanged so render doesn't undo scroll.
- Added tests: mode-only reveal invalidation/suppression consistency, nested images/source-map carets, ordinary slash exclusions, menu dismissal/viewport/wheel stability, empty Notes panel hit bounds, untitled notebook collision handling.

Verification: cargo check -p neoism-ui --tests, cargo check -p neoism --tests, and cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown --features web all passed. Tests compiled, NOT executed; no release build or manual GUI check. Preserve concurrent/unrelated dirty changes.
