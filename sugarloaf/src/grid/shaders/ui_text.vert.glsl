#version 450

// UI text vertex shader, ported from `text_vertex` in
// `sugarloaf/src/grid/shaders/grid.metal`. Positions glyphs in
// pixel space (not on a cell grid) — used by overlay code paths
// (tab titles, command palette, search overlay, assistant) via
// `sugarloaf::text::Text`.
//
// Per-instance vertex layout matches `TextInstance` in
// `sugarloaf/src/text.rs` (56 bytes):
//   loc 0  R32G32_SFLOAT    pos         (offset 0)   text-box top-left
//   loc 1  R32G32_UINT      glyph_pos   (offset 8)
//   loc 2  R32G32_UINT      glyph_size  (offset 16)
//   loc 3  R16G16_SINT      bearings    (offset 24)
//   loc 4  R8G8B8A8_UNORM   color       (offset 28)
//   loc 5  R8_UINT          atlas       (offset 32)
//   loc 6  R32G32B32A32_SFLOAT clip_rect (offset 36)
//   loc 7  R32_SFLOAT      raster_scale (offset 52; zero means unscaled)
//
// 4-vertex triangle strip per instance (`vkCmdDraw(4, N, ..)`).
//
// Uniform block: `vec2 viewport` + `vec2 _pad` for std140 16-byte
// alignment. We compute pixel→NDC inline (no projection matrix —
// uniforms are minimal, just the viewport size).

layout(set = 0, binding = 0, std140) uniform Uniforms {
    vec2 viewport;
    vec2 _pad;
} uniforms;

layout(location = 0) in vec2  in_pos;
layout(location = 1) in uvec2 in_glyph_pos;
layout(location = 2) in uvec2 in_glyph_size;
layout(location = 3) in ivec2 in_bearings;
layout(location = 4) in vec4  in_color;     // unorm8 → vec4 0..1
layout(location = 5) in uint  in_atlas;
layout(location = 6) in vec4  in_clip_rect;
layout(location = 7) in float in_raster_scale;

layout(location = 0) flat out uint out_atlas;
layout(location = 1) flat out vec4 out_color;
layout(location = 2)      out vec2 out_tex_coord;
layout(location = 3) flat out vec4 out_clip_rect;

void main() {
    // Quad corner 0..1 from vertex id (4-vertex TRIANGLE_STRIP).
    vec2 corner;
    corner.x = float(gl_VertexIndex == 1 || gl_VertexIndex == 3);
    corner.y = float(gl_VertexIndex == 2 || gl_VertexIndex == 3);

    vec2 size    = vec2(in_glyph_size);
    float scale = in_raster_scale > 0.0 ? in_raster_scale : 1.0;
    vec2 origin = in_pos + vec2(in_bearings) * scale;
    vec2 quad_px = origin + size * corner * scale;

    // Pixel → NDC (y-up convention). The Vulkan render pass uses a
    // negative-height viewport (set in `Sugarloaf::render_vulkan`)
    // so the rasterizer flips this back to top-left origin. Same
    // formula as the Metal `text_vertex` — see grid.metal.
    vec2 ndc = vec2(
        (quad_px.x / uniforms.viewport.x) * 2.0 - 1.0,
        1.0 - (quad_px.y / uniforms.viewport.y) * 2.0
    );

    gl_Position = vec4(ndc, 0.0, 1.0);

    // Atlas tex coord in PIXEL space — fragment shader uses
    // `texelFetch` (nearest filter, no normalization needed).
    out_tex_coord = vec2(in_glyph_pos) + size * corner;
    out_atlas = in_atlas | (in_raster_scale > 0.0 ? 2u : 0u);
    out_clip_rect = in_clip_rect;

    // Premultiplied RGBA. Color path's atlas already returns
    // premultiplied bytes; grayscale path's `color * mask_a` in the
    // fragment also stays premultiplied.
    vec4 color = in_color;
    color.rgb *= color.a;
    out_color = color;
}
