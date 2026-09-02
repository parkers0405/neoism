---
name: "web missing letters glyph atlas"
description: "Web UI text dropped letters while keeping advances because wgpu R8 atlas uploads used tight bytes_per_row; WebGL UNPACK_ALIGNMENT=4 and WebGPU 256-byte pitch reject those rows."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-20"
updated: "2026-08-20"
---

Web canvas text (file tree, agent, sidebar) can render as Swiss-cheese letters (`Documents` → `ocumens`) while HTML overlays like "Ask anything" stay intact. Desktop Metal/Vulkan is fine.

Cause: `WgpuGlyphAtlas::insert` uploaded tightly packed swash bitmaps (`bytes_per_row = width`). WebGL2 `texSubImage2D` still uses default UNPACK_ALIGNMENT=4 (wgpu-hal only sets it to 1 on native GLES). WebGPU also requires bytesPerRow % 256 == 0. Slot + advance are recorded anyway, so the shader samples empty texels.

Fix (2026-04-18):
- `sugarloaf/src/grid/webgpu.rs` pads each row to `COPY_BYTES_PER_ROW_ALIGNMENT` (256) before `write_texture`.
- `WgpuGlyphAtlas::grow` + bind-group rebuild so the shared UI atlas can double like Metal.
- Workspace `wgpu` pin is `default-features = false` plus native backends; wasm only adds `webgl` so `Instance::new` cannot pick ContextWebGpu when `navigator.gpu` exists.

Do not revert the workspace wgpu default-features change without re-checking wasm Instance backend selection.
