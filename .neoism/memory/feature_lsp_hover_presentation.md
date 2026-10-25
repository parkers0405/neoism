---
name: "Zed-style LSP hover presentation"
description: "Zed-like hover spacing/chips/raised surface; independent bundled Geist prose; source-language Nix highlighting and complete Markdown fences."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-21"
updated: "2026-10-21"
---

Hover presentation lives in `neoism-frontend/shared/src/panels/hover_popup.rs`. Match the Zed reference: theme-relative raised bg/surface blend with subtle border and 8px corners, 14px outer padding, comfortable line height, presentation-only gaps between Markdown blocks (not each wrapped row), and rounded inline-code chips with measured 4px horizontal padding. Fenced examples share the panel background instead of double-painted full-row/per-token stripes. Keep links, selection, canonical copying, wrapping, scrolling and clipping working together; chip padding participates in wrapping, piece widths, caret positions and link hit areas rather than being paint-only. Preserve actual source spaces, adjacent punctuation and code indentation.

Hover prose must NOT inherit `look::markdown_font_family()`; that is the notes/editor font and can be a pixel face. Use shared `primitives::prose_font::ui_prose_font_id(sugarloaf.font_library())`, backed by unmodified bundled proportional Geist, default upright weight 400. Code/signatures remain in the primary monospace cascade. Font bytes and OFL are under `sugarloaf/src/font/resources/Geist/`, exported as FONT_UI_PROSE; NOTICE records attribution. The helper caches only the asset's PostScript identity and resolves IDs per font library, lazily registering if needed, so browser registry resets and different font insertion orders do not reuse invalid numeric IDs. No user-font setting or profile is changed.

Nix highlighting already existed; hover's missing `nix` fence mapping was why # comments and examples were bland. HoverPopupLayout.document_language comes from the card's pinned document path in desktop host/run.rs and shared chrome/draw.rs. Recognized fence labels win; unlabelled/indented fences inherit the document language; explicit unknown/plaintext stays Other. Cache invalidation includes language and resolved prose-font/layout metrics. Shared editor/code/lsp_session.rs::hover_card_lines must preserve full source, with blank separators between servers: the old 40-line cutoff could cut a Markdown fence in half, and the popup now scrolls its viewport instead.

Focused checks: neoism-ui panels::hover_popup::tests; primitives::prose_font::tests; hover_card_keeps_complete_fences_and_server_separators. Standalone Sugarloaf bundled_ui_prose_is_regular_proportional_and_deduplicates verifies the actual font metadata, proportional advances and dedup without needing the whole frontend to compile. Use cargo check on native and wasm targets; do not drive/restart the live UI for repository fixes.
