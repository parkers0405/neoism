---
name: "Agent primary text grey across dark themes"
description: "Agent prose and bold now same bright foreground; native/web real bold face verified, wasm fallback now registers four styles"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

Agent chat prose previously used `IdeTheme.fg` at full opacity, but several dark palettes set it to grey. `white` is not a brighter-primary token: Pastel Dark's is darker than fg; custom themes can set white=fg. Shared `panels/agent_pane/view/mod.rs::primary_text_color` now floors each dark-theme RGB channel at 0xee, preserving already-brighter channels and light-theme fg unchanged. `markdown.rs::render_markdown_blocks` uses it for final-answer paragraphs/lists/headings (not reasoning); `user_input.rs::render_user_message` uses it for sent user copy. Inline bold/bold-italic now inherit foreground rather than switching to white, so emphasized copy cannot get darker. Secondary labels, reasoning dim/alpha, quotes, syntax highlights, and global palettes remain unchanged. Desktop and web both use these shared renderers. Three primary_text_tests cover built-ins, custom white/fg mismatch, already-bright fg, opacity, and light backgrounds; all passed. Native and web cargo checks passed. No release build or visual validation performed.

Bold weight traced through DrawOpts.bold -> Weight::BOLD -> styled font resolution -> real bold face or native synthetic emboldening (Swash .embolden(0.5), CoreText fill+stroke). Desktop defaults load real bold. Full web frontend (`neoism-frontend/wasm/src/rendered/mod.rs`) resets the library and registers Geist regular/italic/bold/bold-italic, so active web already supports real emphasis. However sugarloaf's wasm fallback FontLibraryData::load previously registered only regular, silently losing bold/italic in fallback use; it now calls load_bundled_styles to register all four default bundled variants. Font-resolution regression added in sugarloaf/src/font/mod.rs, asserting bold >=700 and heavier than regular. Standalone sugarloaf lib tests need --features neoism-window/x11,wgpu (otherwise no platform backend, then unrelated compile_shadertoy_glsl test failure).
