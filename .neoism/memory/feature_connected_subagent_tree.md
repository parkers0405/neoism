---
name: "Connected subagent sidebar branches"
description: "Smooth white analytic 2px subagent branches; shared sibling spine; latest 4px-scaled gap below main/ancestor title instead of touching text"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-17"
updated: "2026-10-17"
---

Follow-up to [[feature_left_sidebar_subagents]]: user requested parent-connected branches, thicker joined siblings, then reported chipped curves and asked for white. FINAL geometry uses a continuous 2px UI-scaled device-aligned spine/arms with analytic anti-aliased quarter-ring elbows, not font glyphs or scanline bands. Connectors use theme.fg (white in user's theme). Latest tweak: `view/side_panel/draw.rs::branch_connector_top` adds a 4px UI-scaled gap BELOW parent title ink (23+FONT_SIZE+4) so line no longer touches main/ancestor text; sibling joins still meet prior elbow center and remain continuous. Tests assert gap across scales/nested rows.

Shared `branch_connector_geometry` computes fixed stem/arm tangents and bottom-left ring center/radius/stroke. `Sugarloaf::ring_clipped(center,radius,stroke,color,depth,order,clip)` evaluates analytic circle signed-distance coverage in GLSL/Metal/WGSL and CPU fallback, existing QuadInstance ABI (underline_style=-1; corner_radii payload centerxy/radius/halfstroke). Full bounds include half a physical pixel of AA padding. Quarter clip ends at tangent axes; viewport/modal visible_rect_fragments change clips only, NEVER ring geometry. Every piece uses ORDER_PANEL+2 and same weight. Intermediate siblings retain straight trunk through branch takeoff; last curves off. Do not approximate bends with thin rectangle bands again: user saw chipped edges.

Rendering happens before row visibility culling so a just-offscreen child can draw visible upper stem. Root/child row spacing, titles, click/keyboard focus, trailing running squares unchanged. Primitive extension in sugarloaf.rs, renderer/mod.rs + batch.rs, cpu.rs, quad.frag.glsl, renderer.wgsl, renderer.metal. Tests in renderer batch/cpu ring_tests cover transparent hole, fractional inner/outer AA, clip geometry preservation, translucent alpha, order/next-quad isolation, invalid inputs, and full WGSL naga validation. Renderer26tests and wasm32 --featuresweb check passed when implemented; Metal couldn't compile on Linux. Current gap update passes6branch tests. No live app/release builds.
