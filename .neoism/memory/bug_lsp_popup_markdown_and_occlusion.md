---
name: "LSP popup Markdown and precise menu compositing"
description: "LSP hover renders linked Markdown with measured wrapping/scrolling; right-click menu no longer erases whole editor text runs. Shared late-overlay compositing and precise clip-fragment regressions."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-21"
updated: "2026-10-21"
---

`neoism-frontend/shared/src/panels/hover_popup.rs` now parses rich inline Markdown with pulldown-cmark: hyperlink labels, nested emphasis and inline code omit Markdown delimiters; fenced code retains cached syntax highlighting. Unsupported HTML-like literals (including `Option<T>` diagnostic text) remain inert text; standalone `<br>` inserts a break. Only safe HTTP(S) links are activated. Layout caches measured styled runs, wraps at words/graphemes using overlay text metrics, caps the OUTER card width and viewport height, and replaces the former per-line ellipsis/18-source-line cutoff with bounded vertical scrolling plus a small thumb. Canonical display-text byte offsets preserve selection and copied text across soft wrapping, scrolling and reflow.

HoverPopupState exposes `link_at`, `take_activated_link`, and `scroll(x,y,delta_y)` (positive down). Click activation requires the same link and a latched 4-logical-pixel drag threshold; dragging back or scrolling while pressed cannot activate a link. Desktop code bridge updates the release position, opens through background_process::open_url, and routes wheels before underlying panes. Wasm editor release opens in a new browser tab with noopener/noreferrer; createTerminal.editorPointerUp optionally refreshes x/y before the old wasm release call. Both hosts revalidate HTTP schemes case-insensitively. Shared Chrome has read-only/mutable hover accessors sharing visibility gates; browser/native pointer cursors show hand versus text appropriately. Do not drive/restart the live UI to test code changes; use tests/checks.

Right-click menu disappearing-text root cause was whole syntax-run culling in shared editor/code/render.rs: any overlap with an occlusion rectangle discarded the entire source run, including text outside the popup. Native menus already use Sugarloaf's late overlay pass, so desktop host/tabs.rs must NOT add context_menu.rect() to active_text_occlusion_rects. Shared Chrome now renders context menus in the same late pass. Remaining rectangular occlusions use disjoint clip fragments with identical glyph shaping/origins, preserving fractional partial glyphs rather than culling runs. Do not remove all occlusion globally or restore bounding-box run rejection.

Regressions cover Markdown links/literal code, styled grapheme wrapping, soft-wrap copy, reflow/scroll selection, link click/drag/scroll cancellation, clipped link hits, viewport/outer-cap/scroll-thumb bounds, partial-run/glyph clipping, and actual CPU text plus opaque rounded menu compositing at 100/125/150/200% scale. Focused tests: `cargo test -p neoism-ui --lib panels::hover_popup::tests`, `cargo test -p neoism-ui --lib text_occlusion_tests`, and Sugarloaf `opaque_rounded_menu_preserves_surrounding_editor_pixels_at_all_scales` (use `--features neoism-window/x11` for its standalone test). Native and wasm-target checks and web `npm run typecheck` validate the host paths.
