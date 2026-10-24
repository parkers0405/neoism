---
name: "Agent-pane steady frames + image inspector"
description: "Broad agent jitter fixes: cached timeline vector sharing, inverse-clip fast path, shared draft geometry, virtual table rows, original-quality image inspector, fair window-wide ingestion and exact no-empty-wake yielding; verification and residual risks"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Agent-pane steady frames and image inspector

User reported general agent-pane lag, not just images, and wants consistent frame times without any visual-quality reduction. Do not attribute everything to images or promise a locked refresh rate without live p95/p99 measurements.

## Implemented

- Shared timeline: source-revision/length gate skips artifact reconciliation rows and body bytes on unchanged frames. State/render aliases transfer the same row/page vectors instead of reconstructing history. Tail visibility evaluates requested sources rather than allocating a full-history mask. Right rail drawing is bounded (33 binary searches, <=11 ticks); desktop/shared hover invalidates only tick/geometry/entry/exit transitions. Browser host redraw scheduling is separate.
- Sugarloaf `occlusion.rs`: prepared aggregate bounds reject primitives far from the composer's rounded scanline holes. `filter_tail` compacts in place with extra storage only for actual splits. Preserve disabled clip sentinels, original geometry/UVs, fractional DPI and nested scopes. Debug microbenchmark for 100k distant glyphs: old ~802ms, prepared ~15ms. This is one CPU path, not whole-frame FPS.
- Composer: shared `Rc<Vec<InputWrapRow>>` across cache/render/pane geometry; retained `Rc<str>` draft; reusable content-aware font fingerprint. Exact draft comparison/font-face validation remain. Oversized >512KiB wraps still bypass cache.
- Agent Markdown tables: cached scale-independent row offsets; O(1) total height; binary-search first visible row; only visible rows painted; borrowed display lines rather than owned strings. Preserve row heights, cell wrapping and reveal/selection projection.
- Attachments: one `Arc<ImagePayload>` retains original URL, precomputed SHA-256 identity, shared decode-failure flag and `OnceLock<Option<u32>>` renderer ID. Warm painting neither hashes/copies base64 nor locks the global ID registry. Collision-free allocation within the attachment namespace; negative decode cache and culling before registration. Original decoded resolution/pixels retained, no thumbnail downsampling. URLs convert to strings only at wire boundaries, including workspace-daemon history attachments.
- Image inspector: shared `UniversalModal::open_image`, dark full-window backdrop, spacious aspect-fit full-quality image and minimal close control. Escape/backdrop/close dismissal. Exact 2D thumbnail hit fragments exclude clipping/occlusion. Dedicated late-overlay image owner; chrome clears retained placements even after last overlay closes. Desktop/WASM pointer and wheel routing block background chat motion.
- Desktop wakes: persistent per-window AgentEventWake and same-receiver short circuit avoid allocating/reinstalling locks each paint. Wake callbacks run outside wake mutexes.
- Ingestion: `neoism/agent/ingestion_budget.rs` provides cooperative thread-local nested deadlines. Screen bridge applies one 2ms window budget; visible jobs share at most1400us, remaining time serves stable hidden jobs with a rotating first consumer. Sidebar is a distinct None job, not route0 dispatch. No drains remain in the later paint loop. Background/catalog slices take one fifth of remaining time capped400us, preserving live-stream time in small split allotments. Queues retain unapplied FIFO suffixes/lookahead across yields and request continuation only when work is confirmed. Empty expired queues must not self-sustain redraws. Readiness probes rearm before checking so racing enqueues aren't stranded.

## Verification

Passed desktop bin cargo check and wasm web cargo check. Targeted shared groups passed: timeline117, image52, Markdown39, composer49 (one ignored) plus viewport5; clipping8 plus manual microbenchmark. Desktop ingestion13, transport/coalescing/wake18, nested/fractional budget3 passed. Independent read-only reviews found no confirmed regressions in timeline/hover, clipping/table geometry or window-budget scheduling. No release build or live UI manipulation.

Full suites are not clean: shared3457 passed/14 failed/1 ignored; Sugarloaf174 passed/11 virtual_surface failed/2 ignored. Do not claim all-green or verified live144Hz.

## Remaining profiling targets

Changed-source artifact reconciliation can still scan candidate text across history. Page bookkeeping and anchor construction can remain history-wide. Growing live Markdown can reparse the full answer. Exact draft/font validation remains linear in its input. Cold Vulkan glyph atlas uploads wait on outstanding fences; never remove this synchronization blindly. Cooperative budgets cannot preempt one expensive snapshot, catalog rebuild, refresh helper or GPU wait. Live frame distributions still need measurement on the user's running build.
