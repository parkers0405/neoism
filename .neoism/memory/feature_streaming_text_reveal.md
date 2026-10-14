---
name: "Streaming text cloud reveal"
description: "180ms live text blur+fade; paint-only canonical identity, coalesced updates, 25-tap GPU cloud; agent.streaming-text-animation defaults true"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

# Streaming text cloud reveal

Implemented a 180 ms paint-only blur-to-sharp and opacity reveal for live assistant/reasoning text, including prose, headings/lists/quotes, tables, inline code, fenced code and raw Mermaid. Tokens still append and lay out immediately. Generated controls/markers stay sharp. Both desktop and shared pane types own separate TextRevealState and live ingestion hooks; both expose set_text_reveal_enabled(bool).

## Implementation

Shared helper: neoism-frontend/shared/src/panels/agent_pane/text_reveal.rs. Recent session-scoped bodies only; offscreen ages expire, history sharp, identical live snapshots do not replay. Canonical DISPLAYED text excludes whitespace to preserve identity across wrapping. Per-line pointer offsets refer to retained Rc Markdown layouts; run maps validate pointer content. Prefix/suffix matching handles append fast; bounded interior Unicode LCS preserves unchanged letters when Markdown completion removes multiple delimiters (<=512 scalars/side, <=65536 DP cells /128 KiB; conservative fallback). Paint scope is synchronous/nest-safe thread-local, without held pane/shaper locks.

Important performance fix: ingestion coalesces to <=2 pending snapshots (optional initial sharp baseline + latest live revision), and paint uses the CURRENT already-prepared Markdown blocks for latest projection. Never lay out intermediate token snapshots. Only first update to existing history may lay out its old source baseline. Animation progress never dirties content or layout caches. Visibility/occlusion gates reveal-owned redraws, including the final 180 ms after streaming stops.

Sugarloaf API: sugarloaf::text::TextReveal { range: Range<usize>, opacity: f32, blur_radius: f32 } and Text::draw_revealed(x,y,text,&DrawOpts,&[TextReveal]). DrawOpts unchanged. Full styled-run shaping and advance preserved. Cached glyphs include exclusive cluster_end; fallback clones rebase bounds to full-input UTF-8. Overlapping birth ranges affect ligatures/combining clusters; lowest opacity wins, larger radius breaks ties. Normal draw retains no-reveal specialization.

TextInstance is now 60 bytes: raster_scale offset52, blur_radius offset56. Vulkan, Metal and WGSL match layouts. Actual localized dense 9x9 binomial cloud paired into25 bilinear taps, support capped4 PHYSICAL pixels; glyph atlas isolation and expanded clip geometry prevent neighbors/occlusion leaks. Settled glyphs keep one-fetch path. Color emoji obey reveal alpha without tint. CPU fallback sharp + alpha only. Shared draw.rs halo matches physical kernel cap + bilinear footprint, not an oversized logical halo.

## Configuration and validation

agent.streaming-text-animation defaults true, grouped typed config and Settings > Agent descriptor "Streaming text reveal"; false is manual reduced-motion option. Desktop initial/hot reload/hidden-pane draining and shared Chrome install/draw plus wasm config snapshots wire the flag. No automatic OS reduced-motion detection added.

Verification: shared reveal tests48passed, Sugarloaf text tests29passed1ignored, new config toggle testpassed; native desktop/shared and wasm32 web cargo checks passed before final interior-diff change, shared rechecked after it. Vulkan shader compile/link/SPIR-V and WGSL Naga validation passed. Metal shader source updated but not compiled on Linux. No release build, GPU visual tuning, or runtime frame-time benchmark performed. Broader backend config suite89passed1failed on unrelated agent.enabled-providers schema assertion; left untouched.
