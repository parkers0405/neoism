---
name: "Rounded thinking/code cards lose corners while scrolling"
description: "FIXED: rounded shapes keep original bounds under GPU clipping; header-only code slices no longer disappear; 5 regressions + native/shared check pass"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

Fixed October 9 2026: native agent thinking/reasoning containers lose remaining rounded corners as they cross viewport; code areas can drop entire visible header/cap while scrolling. Source roots: shared widgets/quad.rs deliberately drew partial top/side clips as square rects and bottom clips as a newly rounded visible slice + square fill (comment incorrectly claimed rounded quads cannot be scissored). Agent view/draw.rs top-rounded header same all-or-nothing fallback. Markdown/code_block renderers returned before painting chrome when no code lines/body text clip visible.

Implementation: new Sugarloaf::quad_clipped logical-pixel immediate primitive scales ORIGINAL rect/radii and SEPARATE viewport clip, routes normal/late overlay compositor appropriately. Renderer::quad_clipped delegates BatchManager::quad_clipped with per-instance clip; no mutable global clip state changes/leaks. Existing BatchManager::quad delegates with its inherited clip. GPU backend shaders and CPU already honor these instance bounds. Shared uniform rounded helper now uses exact original shape with viewport clipping, preserves cached id behavior via physical set_bounds, removes obsolete tolerance parameter from all four callers (agent draw, Markdown editor, extensions page, stock card). Top-only header uses per-corner [radius,radius,0,0] original bounds instead of square fallback/fill patch.

Code chrome: whole block intersection controls border/background/header. Body text clipping and zero visible-line ranges only gate text, not chrome. Tool output includes its original 4px top extension when culling; header label culling no longer skips visible body. Tool-code scroll hit region intersects viewport. No layout, scroll offset, timeline follow, activity docking/pinning, or input positioning change. Other agents' existing primary text color/image/trace changes preserved.

Tests: 3 renderer regressions passed (all edges/thin slices/top-only corners/multiple scales preserve full geometry and radii; clip isolation/inherited clip; CPU rendered pixels equal original shape masked by viewport). 2 code-chrome tests passed (header-only/padding-only/body/offscreen at scales .75/1/1.5/2). cargo check -p sugarloaf -p neoism-ui -p neoism passed. Test invocation requires --features neoism-window/wayland,neoism-window/wayland-dlopen for standalone Sugarloaf Linux test dev dependency; default has no window platform. Also corrected pre-existing shader_overlay test gating: module cfg(all(test, any(wgpu,macos))) and bundled WGSL compiler test cfg(wgpu), matching actual function availability, no runtime shader changes. Final renderer tests rerun clean; scoped diff whitespace clean.

Read-only native screenshot captured installed Neoism; no input events, app restart, release build, paid requests, or post-rebuild visual verification performed. A straight cut through middle of an original rounded card remains a natural viewport edge; do not re-round cropped fragment or move cards/headers to follow viewport. Previous always-docked activity attempt was reverted at user request (bug_reasoning_pushes_bottom_activity.md); do not reintroduce it.
