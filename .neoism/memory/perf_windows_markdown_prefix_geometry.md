---
name: "Windows markdown redraw: exact prefix geometry cache"
description: "Markdown warm redraw rebuilt quadratic prefix hit-test geometry; bounded exact cache + solo presence no-canonicalize; CPU microbenchmark 18.23ms→.022ms (not Windows FPS)"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-09-06"
updated: "2026-09-06"
---

Investigated Windows markdown ~40fps report. Fixed demonstrated redraw waste; no Windows GUI/FPS reproduction available in Linux session, so NOT a verified end-to-end FPS fix.

## Evidence and changes (only 3 production files)
1. `neoism-frontend/shared/src/editor/markdown/render/virtualized/inline_layout.rs::measured_stops_for_text` measured EVERY successive Unicode-scalar prefix each redraw. `render_data.rs::begin_block_layout` clears hit stops every frame; paragraph/heading/code/notebook draw registers them again. Warm Sugarloaf shape-cache hits still walk each prefix for font resolution, hash it, and clone shaped glyph vectors: O(n²) per visual row.
2. `sugarloaf/src/text.rs` adds `Text::measure_char_prefixes`, consumed only by markdown. Caches EXACT existing prefix measurements in 512-entry LRU, lines <=1024 UTF-8 bytes (~2.5 MiB maximum text/stops); oversized lines retain exact uncached behavior. Collision-safe full-text key includes font id, raw size bits, DPI bits, bold/italic; excludes paint color/clip. `clear_glyph_cache` clears it, including font-library replacement. No approximation of kerning, ligatures, fallback fonts or caret geometry. Cold measurement remains quadratic; warm work becomes linear copy/hash.
3. Desktop `screen/bridges/markdown/render.rs` previously canonicalized every visible markdown/notebook path each frame for presence even when solo. Windows `dunce::canonicalize` does file-handle queries. Gate identity collection using existing `remote_presence.has_any_peers()`; still clear pane cursor lists when peers leave. No change to canonical CRDT identities with peers present. Adds opt-in CPU timing using `RUST_LOG=neoism::markdown_perf=debug`: `cpu_us`, needs_redraw, remote_peers; excludes GPU submit/present.

## Platform findings / limits
Linux and Windows share Swash shaping and cache warm fallback resolution. Windows cold fallback discovery mmap/probes installed fonts. wgpu UI text is one instanced draw, not one per word. Full-document virtual parse is source-revision-gated. Cover resolution still does exists checks; draw embeds read files; left alone absent evidence special content explains report. Terminal scheduling, shaders, visual quality, and other agents' markdown scroll/input edits untouched.

## Verification
- PASS `cargo check -p sugarloaf -p neoism-ui -p neoism` (final integration, existing warnings).
- PASS `cargo test -p sugarloaf --features 'wgpu,neoism-window/x11' --lib markdown_prefix -- --include-ignored --nocapture`: 3 tests. Covers exact empty/ASCII/Unicode/combining/CJK/icon/emoji geometry, font/style/size/DPI, warm cache bypassing shaping, cache bounds/oversized bypass, glyph/font invalidation. Includes ignored-by-default microbenchmark.
- Linux unoptimized CPU microbenchmark: 48 lines x100 warm frames, old prefix loop 1.822916469s vs cached 2.209884ms (~18.23ms vs .0221ms per simulated frame FOR GEOMETRY ONLY, not actual FPS). Baseline uses slices rather than original growing String, so does not inflate allocation advantage.
- PASS `env -u CI cargo xwin check --target x86_64-pc-windows-msvc -p sugarloaf -p neoism-ui --features sugarloaf/wgpu`.
- PASS `env -u CI cargo xwin check --target x86_64-pc-windows-msvc -p neoism --features wgpu`. Initially blocked by Windows terminal missing base64 dependency; concurrent terminal agent corrected it, full retry passed. Did not modify terminal files.
- PASS `git diff --check`. No release build or commit. Logs `/tmp/neoism-markdown-{tests,check,windows-check,windows-shared-check}.log`.

Real Windows GPU/FPS validation remains required. Compare opt-in markdown CPU timing against frame duration to distinguish residual CPU work from presentation/pacing costs.
