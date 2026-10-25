---
name: "Background cover/contain + positioning"
description: "Centered cover default + contain/normalized positioning across Metal/Vulkan/wgpu; Settings/picker preserved; standalone Linux tests need neoism-window/x11."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Background images: aspect-preserving cover/contain

Desktop backgrounds now default to centered `cover`, replacing independent width/height stretch. Existing path/opacity-only configs deserialize to cover with position-x/y 0.5. `sugarloaf::BackgroundImageFit` (Cover/Contain) and ImageProperties fit/position_x/position_y serialize as `fit`, `position-x`, `position-y` with normalized alignment 0..1. Cover fills viewport and crops UVs; contain shows whole image and aligns spare space over underlying background color.

One pure `BackgroundImageLayout::instance` in sugarloaf/src/renderer/mod.rs computes ImageInstance for Metal, Vulkan and wgpu on each frame/resize. UV source_rect is [u0,v0,u1,v1], NOT origin/size (shader comments fixed). Renderer retains background dimensions separately from GPU texture. Fit/position-only hot reload skips image decode/upload while preserving file-identity checks.

Config intelligence supplies desktop Settings fit closed select and numeric positions for ui.window plus platform overrides. Background normalization/catalog/picker preserve framing. Pathless ImageProperties can adjust inherited pack fit/positions while retaining its path/opacity; disabled wins. Web hides background-image subtree in Settings because Sugarloaf still cannot load backgrounds on wasm. Picker preview intentionally remains full-opacity centered contain.

Docs: neoism-workspace-index/src/welcome/Neoism/Appearance.md. Regression coverage: renderer background_image_tests; backend config::background tests and config descriptor bounds; palette background selection property preservation.

Verification: desktop/backend --tests cargo check and wasm --features web check passed. 5 renderer geometry tests, 10 scoped backend background/descriptor tests, and 51 shared UI background tests passed. Standalone sugarloaf tests need `--features neoism-window/x11` on Linux or the dev dependency emits unsupported-platform compile_error. Existing whole-Config serialization/Lua-patch background test fails `invalid type: map, expected a sequence`; excluded only that test from scoped backend rerun. New framing roundtrip test uses Window rather than unrelated whole-Config roundtrip. Background palette geometry comparison now uses settled palette_rect instead of two wall-clock active_visual_rect samples (previously flaky animation differences).

No personal config changed; user requested local debug evaluation, no commit/push.
