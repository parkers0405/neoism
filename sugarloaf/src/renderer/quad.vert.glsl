#version 450

// Per-instance quad vertex shader, ported from `vs_instanced` in
// `sugarloaf/src/renderer/renderer.metal`. One `QuadInstance` per
// instance, vertex_id picks the corner. 4 vertices per instance,
// drawn as `TRIANGLE_STRIP`.
//
// Per-instance vertex layout matches `QuadInstance` in
// `sugarloaf/src/renderer/batch.rs` (96 bytes):
//   loc 0  R32G32B32_SFLOAT     pos[3]            (offset 0)
//   loc 1  R32G32B32A32_SFLOAT  color[4]          (offset 12)
//   loc 2  R32G32B32A32_SFLOAT  uv_rect[4]        (offset 28)
//   loc 3  R32G32_SINT          layers[2]         (offset 44)
//   loc 4  R32G32_SFLOAT        size[2]           (offset 52)
//   loc 5  R32G32B32A32_SFLOAT  corner_radii[4]   (offset 60)
//   loc 6  R32_SINT             underline_style   (offset 76)
//   loc 7  R32G32B32A32_SFLOAT  clip_rect[4]      (offset 80)
//
// This Vulkan port renders only the SDF rounded-rect fill path; the
// glyph atlas sampling and underline pattern paths from
// renderer.metal are not yet ported (grid text + UI text overlay
// have their own pipelines for those).

layout(set = 0, binding = 0, std140) uniform Globals {
    mat4 transform;
    uint input_colorspace;
    uint _pad0;
    uint _pad1;
    uint _pad2;
} globals;

layout(location = 0) in vec3  in_pos;
layout(location = 1) in vec4  in_color;
layout(location = 2) in vec4  in_uv_rect;
layout(location = 3) in ivec2 in_layers;
layout(location = 4) in vec2  in_size;
layout(location = 5) in vec4  in_corner_radii;
layout(location = 6) in int   in_underline_style;
layout(location = 7) in vec4  in_clip_rect;

layout(location = 0)      out vec4 out_color;
layout(location = 1)      out vec2 out_uv;
layout(location = 2) flat out ivec2 out_layers;
layout(location = 3)      out vec4 out_corner_radii;
layout(location = 4)      out vec2 out_rect_size;
layout(location = 5) flat out int  out_underline_style;
layout(location = 6) flat out vec4 out_clip_rect;

// Unit quad corners for triangle strip: TL, BL, TR, BR.
const vec2 UNIT_QUAD[4] = vec2[](
    vec2(0.0, 0.0),
    vec2(0.0, 1.0),
    vec2(1.0, 0.0),
    vec2(1.0, 1.0)
);

void main() {
    vec2 unit = UNIT_QUAD[gl_VertexIndex];
    // Rasterize only the clipped part of the quad. The fragment shader's
    // clip test is a per-pixel `discard`, so a large card emitted once per
    // occlusion fragment (often zero-height slivers) used to shade its whole
    // area every time: hundreds of megapixels per frame, GPU-bound scrolling
    // past diff cards. UVs stay relative to the full quad, so SDF corners and
    // underline patterns are unchanged.
    vec2 lo = in_pos.xy;
    vec2 hi = in_pos.xy + in_size;
    if (in_clip_rect.z > 0.0) {
        lo = max(lo, in_clip_rect.xy);
        hi = max(lo, min(hi, in_clip_rect.xy + in_clip_rect.zw));
    }
    vec2 pos = mix(lo, hi, unit);
    vec2 full_unit = (pos - in_pos.xy) / max(in_size, vec2(1e-6));
    vec2 uv = mix(in_uv_rect.xy, in_uv_rect.zw, full_unit);

    gl_Position = globals.transform * vec4(pos, in_pos.z, 1.0);

    out_color = in_color;
    out_uv = uv;
    out_layers = in_layers;
    out_corner_radii = in_corner_radii;
    out_rect_size = in_size;
    out_underline_style = in_underline_style;
    out_clip_rect = in_clip_rect;
}
