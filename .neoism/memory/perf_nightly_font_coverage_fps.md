---
name: "Nightly FPS regression: preferred-font coverage"
description: "Nightly explicit-font shaping locked and parsed coverage per character per frame; cached by font and character"
type: "perf"
scope: "project"
origin: "2026-09-30 origin/main..nightly investigation"
created: "2026-09-30"
updated: "2026-09-30"
---

Local `main`, local `nightly`, and `origin/nightly` all point to `0b871210a`; there is no local main-vs-nightly diff (`0 0`). The useful stable comparison is `origin/main` (`f64b547da`), with nightly nine commits ahead. The first major render boundary is `ebe57b65e`; the Conversations/UI wave is `50e891ded`.

Primary FPS regression: `ebe57b65e` changed Sugarloaf explicit-font shaping so `Text::shape_with_preferred_font` calls `font_covers_char` for every character before reaching the existing shape cache. Each call acquired `font_library.inner.read()` and, on Linux, rebuilt a `swash::FontRef`/queried the charmap. Explicit fonts are used broadly by Markdown, Agent pixel headings, sidebar headings, icons, Notes, settings, and top chrome, so any animation forced this work every frame. Debug builds amplify it, but it is algorithmic and exists in release.

Fix: `sugarloaf/src/text.rs` now caches coverage by `(preferred_font_id, char)`. Common UI characters lock/parse once per Text instance rather than once per rendered character per frame. Preferred-font fallback behavior remains unchanged and the existing mixed icon/label test now verifies a warm second shape does not expand the coverage cache.

Secondary nightly costs: Conversations renders another complete visible catalog and Agent render bridging scans inactive workspaces/tab strips each Agent frame. The dirty tree already removed the committed full `sessions().to_vec()` clone. The required 600ms hover marquee intentionally owns continuous redraw while hovering an overflowing title; it can expose sidebar cost but should not be removed because it is specified behavior. GPU selection, present mode, shaders, and steady-state terminal rendering had no meaningful regression against `origin/main`.

Verification: `cargo check -p neoism`, wasm32 check, `cargo fmt --all -- --check`, and `git diff --check` pass. Standalone Sugarloaf unit-test compilation is blocked by pre-existing feature/cfg issues: without platform features `neoism-window` rejects the target; with `neoism-window/x11`, `shader_overlay` test code references cfg-absent `compile_shadertoy_glsl`. The application builds exercise the modified code successfully.
