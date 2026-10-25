---
name: "Interactive LSP hover and navigation menu"
description: "Selectable LSP hover; frozen capability-aware rounded navigation menu; explicit query outcomes, live snapshot synchronization, correct reference positions."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-21"
updated: "2026-10-21"
---

The native code editor's documentation hover uses shared `panels/hover_popup.rs::HoverPopupState`: rendered logical-pixel bounds, tapered source-to-popup gap corridor (above or below, without retaining neighboring symbols), measured Unicode byte caret stops via Sugarloaf's cached `measure_char_prefixes`, anchor/focus selection, and captured dragging outside popup bounds. Selection bands use theme accent behind glyphs. Each desktop `CodeHoverCard` and shared `LspHoverCard` owns its state. Hidden/occluded cards call `hide()` so stale geometry cannot intercept input. Desktop and shared Chrome/wasm pointer routing give the card first refusal before document selection; Ctrl/Cmd+C and global Copy prefer selected hover text.

Native editor right-click navigation uses the existing shared `ContextMenu`, with simple modern rounded styling, an empty title, and four entries: Go to Definition, Go to Declaration, Go to Type Definition, Show All References. `ContextMenuAction::CodeLsp` captures path, buffer revision, line and byte column when opening; execution validates document identity/revision and restores the captured position rather than trusting a mutable live cursor. Opening cancels code selection/scrollbar dragging; modal menu pointer motion runs before editor drag/hover routing. `ContextMenu::action_at` resolves the actual clicked enabled row, so clicking a disabled row cannot execute a previous selection. Locally negotiated unsupported actions are disabled with a muted `Not supported` hint; unknown/busy/remote capabilities remain enabled. Declaration/type-definition use real LSP methods and independently negotiated provider flags, never aliases to definition.

Follow-up on apparent universally empty results: direct LSP queries AND a live native menu test resolved `flake.nix` line 46's `msrv` reference to its definition on line 43. The initial caret was in indentation; valid empty results at whitespace/bindings/unused symbols must not be confused with failures. The installed real `nil` server supports definition and references but not declaration/type-definition; a real-server regression covers this. Do not invent nearest-symbol or definition fallbacks.

Explicit navigation now uses `engine::query_navigation` with captured live text: `Err` preserves a real failure, `Ok(None)` means unsupported/no route, and `Ok(Some(vec))` means a successful empty or populated result. Legacy Vec APIs remain wrappers. Snapshot synchronization and request run under the same client lock to avoid coalesced `Sync(A) -> Query(A) -> Sync(B)` skipping the required document state. Local reference locations are engine-one-based; convert to zero-based snippet rows/byte columns, then one-based finder display rows, and resolve relative paths against the captured root. Current-document reference previews prefer the captured live text rather than stale disk text. Tests include stateful fake-server document/position validation, real nil, frozen menu targets and disabled-row hit testing.
