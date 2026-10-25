---
name: "Agent code/diff warm-frame preparation"
description: "Verified code/diff warm-frame caches: shared shaping/selection, exact token/gutter advances, bounded admission, nested prep reduction; native/web compile-only checks passed"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-24"
updated: "2026-10-24"
---

# Agent code/diff warm-frame preparation

After the whole-app performance pass, ordinary fenced code, Markdown diff fences and tool-edit diff cards still incurred recurring CPU preparation while visible/scrolled. Never disable, throttle, simplify or idle-pause animations/effects; user explicitly wants continuous animation. This pass was compile-only: no tests execution, application/release build, GUI manipulation or relaunch.

## Implemented

- `sugarloaf/src/text.rs` and `text/{cache,font_stamp}.rs`: immutable `Arc<[ShapedGlyph]>`, source-byte offsets instead of copied/rebased glyph vectors, whole-text/style cache ahead of character/font segmentation, exact retained run advances. Both shape caches have incremental bounded LRU bookkeeping (2048 entries/8 MiB each, 256 KiB entry cap). Hash hits verify exact source/style equality.
- `Text::font_cache_generation()` synchronously observes pending library/DPI changes and returns process-globally unique opaque tokens, not renderer-local counters or raw pointers. Observation is allocation-free O(faces + symbol ranges), because library maps are publicly mutable without a mutation epoch. Appended fallback discovery preserves existing emitted instances/atlas slots; destructive invalidation resets outer caches AND Swash shape/scale contexts.
- `draw_scaled` completes shaping/invalidation before its instance checkpoint, then emits via observer-free `draw_shaped` so the checkpoint cannot be invalidated below it. `font/mod.rs` removes redundant size-only library primary-metrics caching and uses the current primary face's existing natural-metrics cache, preventing stale baselines after face replacement.
- Shared/desktop selection paths retain immutable text/caret stops and width; code rows keep local stops plus translation. Inline Markdown preserves the old screen-origin accumulation and keys by exact origin bits to avoid floating-point pixel changes. Borrowed warm keys avoid owned source/translated-vector allocations; history clones share storage. Both caret caches have 8 MiB retained-byte budgets plus entry/point limits, share Arc source keys with FIFO, and skip oversized entries including huge hidden Markdown URLs.
- `widgets/diff_card/measurement_cache.rs`: exact token advances keyed by pinned syntax spans and synchronized font identity; exact gutter maximum over the same ten repeated-digit probes. Font allocations are weak-pinned, not retained strongly. Translated positions reproduce sequential `x += advance` from actual screen x; nowrap seeking uses retained endpoints and never substring shaping. Separate 16 KiB entry admission and 4 MiB aggregate budget prevent two giant rows evicting/repreparing one another. Oversized rows keep the old viewport-limited measurement path.
- Tool-message rendering shares one child diff-section lookup across collapsed/expanded height endpoints and paint, including explicit negative preparation, while sampling live expansion/status progress every frame. Parent clipping precedes child preparation; offscreen file sections skip gutters/width/scroll setup; iteration stops once subsequent sections are below the viewport. Height additions preserve original order. Retained full content width validates font generation.

## Review and verification

Independent source reviews covered selection translation/UTF-8/empty-row behavior, diff token/gutter/culling semantics, glyph sharing/source rebasing, widths, reveal clusters, global font identity and bounded LRU bookkeeping. All confirmed findings were corrected and follow-up reviews found no further concrete issues. Findings fixed: hidden raw-source retention, measurement admission thrashing, draw_scaled invalidation checkpoint, Swash same-key replacement state, shared primary metrics.

Final commands passed after all corrections: `cargo check --locked -p neoism -p neoism-ui -p sugarloaf --tests --message-format=short`; `cargo check --locked -p neoism-terminal-wasm --target wasm32-unknown-unknown --features web --tests --message-format=short`; `git diff --check`. Shaping-specific `cargo check -p sugarloaf --tests --features neoism-window/x11` also passed. Existing warnings remain. Regression tests were compiled, not run, including float-bit positions, pointer sharing, combining/UTF-8/tabs/ligatures, clipping/reveal, cache byte/admission bounds and multi-renderer/font-replacement CPU-pixel comparisons.

## Residuals and limits

Do not claim measured FPS or runtime/pixel equivalence from compile checks. Full-text hashing/byte comparison, font metadata observation, cold shaping, reveal work and glyph emission still exist. Pre-atlas glyph culling was not added: exact raster ink/bearings and later instance transforms make speculative clipping unsafe. Nested compositions still allocate child metadata and hash payload once per child; work was reduced, not eliminated. Mutation observation is at API boundaries, not an atomic font snapshot against arbitrary concurrent mid-call edits. Animations and effect quality were not changed. Previous Vulkan CPU-wait/barrier and image-retirement fixes remain untouched; WSI presentation-lifetime redesign remains separate.
