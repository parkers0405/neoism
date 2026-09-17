#version 450

// Text fragment shader. Two atlases bound — grayscale (R8) for outline
// glyphs, RGBA8 for color emoji. The vertex shader's `out_atlas` flag
// picks which one to read.
//
// Sampling is `texelFetch` (nearest, no filtering) at integer pixel
// coordinates — matches Metal's `coord::pixel` + `filter::nearest`.

layout(set = 1, binding = 0) uniform sampler2D atlas_grayscale;
layout(set = 1, binding = 1) uniform sampler2D atlas_color;

layout(location = 0) flat in uint in_atlas;
layout(location = 1) flat in vec4 in_color;
layout(location = 2)      in vec2 in_tex_coord;
layout(location = 3) flat in vec4 in_clip_rect;

layout(location = 0) out vec4 out_color;

const uint ATLAS_GRAYSCALE = 0u;

vec4 sample_scaled(sampler2D atlas, vec2 uv) {
    vec2 p = uv - vec2(0.5);
    ivec2 base = ivec2(floor(p));
    vec2 f = fract(p);
    ivec2 hi = textureSize(atlas, 0) - ivec2(1);
    vec4 a = texelFetch(atlas, clamp(base, ivec2(0), hi), 0);
    vec4 b = texelFetch(atlas, clamp(base + ivec2(1, 0), ivec2(0), hi), 0);
    vec4 c = texelFetch(atlas, clamp(base + ivec2(0, 1), ivec2(0), hi), 0);
    vec4 d = texelFetch(atlas, clamp(base + ivec2(1, 1), ivec2(0), hi), 0);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

void main() {
    if (in_clip_rect.z > 0.0 && in_clip_rect.w > 0.0) {
        float px = gl_FragCoord.x;
        float py = gl_FragCoord.y;
        if (px < in_clip_rect.x
            || px >= in_clip_rect.x + in_clip_rect.z
            || py < in_clip_rect.y
            || py >= in_clip_rect.y + in_clip_rect.w) {
            discard;
        }
    }

    if ((in_atlas & 2u) != 0u) {
        out_color = (in_atlas & 1u) == 0u
            ? in_color * sample_scaled(atlas_grayscale, in_tex_coord).r
            : sample_scaled(atlas_color, in_tex_coord);
        return;
    }
    ivec2 uv = ivec2(in_tex_coord);
    if (in_atlas == ATLAS_GRAYSCALE) {
        // Grayscale: sample alpha mask, multiply by per-glyph color.
        // Colour is already premultiplied (in_color.rgb *= in_color.a
        // in the vertex shader), so the result is also premultiplied.
        float a = texelFetch(atlas_grayscale, uv, 0).r;
        out_color = in_color * a;
    } else {
        // Color atlas: sample RGBA premultiplied directly.
        out_color = texelFetch(atlas_color, uv, 0);
    }
}
