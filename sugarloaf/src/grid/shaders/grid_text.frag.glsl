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
layout(location = 4) flat in vec4 in_glyph_bounds;
layout(location = 5) flat in float in_blur_radius;

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

// Zero-extended bilinear reads: every fetch is checked against this glyph's
// atlas allocation, including the bilinear footprint. Never read a neighbor.
vec4 glyph_fetch(sampler2D atlas, ivec2 p) {
    if (any(lessThan(p, ivec2(in_glyph_bounds.xy))) ||
        any(greaterThanEqual(p, ivec2(in_glyph_bounds.zw)))) return vec4(0.0);
    return texelFetch(atlas, p, 0);
}
vec4 glyph_sample(sampler2D atlas, vec2 uv) {
    vec2 p = uv - vec2(0.5);
    ivec2 b = ivec2(floor(p));
    vec2 f = fract(p);
    return mix(mix(glyph_fetch(atlas, b), glyph_fetch(atlas, b + ivec2(1, 0)), f.x),
               mix(glyph_fetch(atlas, b + ivec2(0, 1)), glyph_fetch(atlas, b + ivec2(1, 1)), f.x), f.y);
}
// Dense 9x9 binomial cloud, paired into 5x5 bilinear taps. At maximum
// radius (4 physical px), weights are [1,8,28,56,70,56,28,8,1]/256
// per axis. Pairing adjacent texels gives offsets 28/9 and 4/3 with
// combined weights 9 and 84. Radius/4 scales support, NOT whole-glyph
// translations: source spacing never exceeds one physical pixel.
// Bounded 25 taps; settled glyphs bypass the kernel entirely.
vec4 glyph_blur(sampler2D atlas) {
    const float offsets[5] = float[5](-28.0/9.0, -4.0/3.0, 0.0, 4.0/3.0, 28.0/9.0);
    const float weights[5] = float[5](9.0, 84.0, 70.0, 84.0, 9.0);
    vec4 sum = vec4(0.0);
    float step_px = in_blur_radius / 4.0;
    for (int y = 0; y < 5; ++y) {
        for (int x = 0; x < 5; ++x) {
            vec2 offset = vec2(offsets[x], offsets[y]) * step_px;
            sum += glyph_sample(atlas, in_tex_coord + offset) * weights[x] * weights[y];
        }
    }
    return sum / 65536.0;
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

    if (in_blur_radius > 0.0) {
        out_color = (in_atlas & 1u) == 0u
            ? in_color * glyph_blur(atlas_grayscale).r
            : glyph_blur(atlas_color) * in_color.a;
        return;
    }
    if ((in_atlas & 2u) != 0u) {
        out_color = (in_atlas & 1u) == 0u
            ? in_color * sample_scaled(atlas_grayscale, in_tex_coord).r
            : sample_scaled(atlas_color, in_tex_coord) * in_color.a;
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
        out_color = texelFetch(atlas_color, uv, 0) * in_color.a;
    }
}
