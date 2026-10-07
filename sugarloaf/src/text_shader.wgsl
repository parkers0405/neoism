// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

// WGSL shader for sugarloaf::text — immediate-mode UI text pass.
// Mirrors `text_vertex` + `grid_text_fragment` in grid.metal.
//
// Kept as its own module (not inlined into grid.wgsl) to sidestep the
// `@group(0) @binding(0)` collision that would happen if text used a
// different uniform struct than the grid's.

const ATLAS_GRAYSCALE: u32 = 0u;
const ATLAS_COLOR:     u32 = 1u;

// group(0): UI-text uniforms (just a viewport pair + 8 bytes of pad
// for WGSL's 16-byte min alignment).
struct TextUniforms {
    viewport: vec4<f32>,
};
@group(0) @binding(0) var<uniform> text_uniforms: TextUniforms;

// group(1): glyph atlases. `textureLoad` (no sampler) to match
// Metal's `coord::pixel + filter::nearest`.
@group(1) @binding(0) var atlas_grayscale: texture_2d<f32>;
@group(1) @binding(1) var atlas_color:     texture_2d<f32>;

struct TextInstanceIn {
    @location(0) pos:        vec2<f32>,
    @location(1) glyph_pos:  vec2<u32>,
    @location(2) glyph_size: vec2<u32>,
    @location(3) bearings:   vec2<i32>,   // Sint16x2, sign-ext to i32
    @location(4) color:      vec4<f32>,   // Unorm8x4 → 0..1
    @location(5) atlas_pack: vec4<u32>,   // Uint8x4; only .x used
    @location(6) clip_rect:  vec4<f32>,
    @location(7) raster_scale: f32,
    @location(8) blur_radius: f32,
};

struct TextVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) atlas: u32,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) tex_coord: vec2<f32>,
    @location(3) @interpolate(flat) clip_rect: vec4<f32>,
    @location(4) @interpolate(flat) glyph_bounds: vec4<f32>,
    @location(5) @interpolate(flat) blur_radius: f32,
};

@vertex
fn text_vertex(
    @builtin(vertex_index) vid: u32,
    in: TextInstanceIn,
) -> TextVsOut {
    // Quad corner 0..1 from vertex id (4-vertex triangle strip).
    var corner: vec2<f32>;
    corner.x = select(0.0, 1.0, vid == 1u || vid == 3u);
    corner.y = select(0.0, 1.0, vid == 2u || vid == 3u);

    let size    = vec2<f32>(in.glyph_size);
    let scale = select(1.0, in.raster_scale, in.raster_scale > 0.0);
    let origin = in.pos + vec2<f32>(in.bearings) * scale;
    let radius = clamp(in.blur_radius, 0.0, 4.0);
    let pad = select(0.0, radius + scale, radius > 0.0);
    let local = size * corner + (corner * 2.0 - vec2<f32>(1.0)) * pad / scale;
    let quad_px = origin + local * scale;

    // Pixel → NDC (y-flip).
    let vp = text_uniforms.viewport.xy;
    let ndc = vec2<f32>(
        (quad_px.x / vp.x) * 2.0 - 1.0,
        1.0 - (quad_px.y / vp.y) * 2.0,
    );

    var out: TextVsOut;
    out.position  = vec4<f32>(ndc, 0.0, 1.0);
    out.tex_coord = vec2<f32>(in.glyph_pos) + local;
    out.atlas = in.atlas_pack.x | select(0u, 2u, in.raster_scale > 0.0);
    out.clip_rect = in.clip_rect;
    out.glyph_bounds = vec4<f32>(vec2<f32>(in.glyph_pos), vec2<f32>(in.glyph_pos) + size);
    out.blur_radius = radius / scale;

    // Premultiply RGB by alpha. Blend state is
    // `One * src + OneMinusSrcAlpha * dst`.
    var color = in.color;
    color = vec4<f32>(color.rgb * color.a, color.a);
    out.color = color;
    return out;
}

fn sample_scaled(atlas: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let p = uv - vec2<f32>(0.5);
    let base = vec2<i32>(floor(p));
    let f = fract(p);
    let hi = vec2<i32>(textureDimensions(atlas)) - vec2<i32>(1);
    let a = textureLoad(atlas, clamp(base, vec2<i32>(0), hi), 0);
    let b = textureLoad(atlas, clamp(base + vec2<i32>(1, 0), vec2<i32>(0), hi), 0);
    let c = textureLoad(atlas, clamp(base + vec2<i32>(0, 1), vec2<i32>(0), hi), 0);
    let d = textureLoad(atlas, clamp(base + vec2<i32>(1, 1), vec2<i32>(0), hi), 0);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Zero extension, including every texel in each bilinear footprint.
fn glyph_fetch(atlas: texture_2d<f32>, p: vec2<i32>, bounds: vec4<f32>) -> vec4<f32> {
    if (any(p < vec2<i32>(bounds.xy)) || any(p >= vec2<i32>(bounds.zw))) {
        return vec4<f32>(0.0);
    }
    return textureLoad(atlas, p, 0);
}
fn glyph_sample(atlas: texture_2d<f32>, uv: vec2<f32>, bounds: vec4<f32>) -> vec4<f32> {
    let p = uv - vec2<f32>(0.5);
    let b = vec2<i32>(floor(p));
    let f = fract(p);
    return mix(mix(glyph_fetch(atlas, b, bounds), glyph_fetch(atlas, b + vec2<i32>(1, 0), bounds), f.x),
               mix(glyph_fetch(atlas, b + vec2<i32>(0, 1), bounds), glyph_fetch(atlas, b + vec2<i32>(1, 1), bounds), f.x), f.y);
}
// Dense 9x9 binomial kernel paired into 25 bounded bilinear taps, matching
// GLSL/Metal. Support <=4 physical px; unpaired source spacing <=1 px,
// preventing sparse translated glyph ghosts. Per-axis weights sum to 256.
fn glyph_blur(atlas: texture_2d<f32>, in: TextVsOut) -> vec4<f32> {
    let offsets = array<f32, 5>(-28.0/9.0, -4.0/3.0, 0.0, 4.0/3.0, 28.0/9.0);
    let weights = array<f32, 5>(9.0, 84.0, 70.0, 84.0, 9.0);
    var sum = vec4<f32>(0.0);
    let step_px = in.blur_radius / 4.0;
    for (var y = 0; y < 5; y++) {
        for (var x = 0; x < 5; x++) {
            let offset = vec2<f32>(offsets[x], offsets[y]) * step_px;
            sum += glyph_sample(atlas, in.tex_coord + offset, in.glyph_bounds) * weights[x] * weights[y];
        }
    }
    return sum / 65536.0;
}

@fragment
fn text_fragment(in: TextVsOut) -> @location(0) vec4<f32> {
    if (in.clip_rect.z > 0.0 && in.clip_rect.w > 0.0) {
        let px = in.position.x;
        let py = in.position.y;
        if (px < in.clip_rect.x
            || px >= in.clip_rect.x + in.clip_rect.z
            || py < in.clip_rect.y
            || py >= in.clip_rect.y + in.clip_rect.w) {
            discard;
        }
    }

    if (in.blur_radius > 0.0) {
        if ((in.atlas & 1u) == 0u) { return in.color * glyph_blur(atlas_grayscale, in).r; }
        return glyph_blur(atlas_color, in) * in.color.a;
    }
    if ((in.atlas & 2u) != 0u) {
        if ((in.atlas & 1u) == 0u) { return in.color * sample_scaled(atlas_grayscale, in.tex_coord).r; }
        return sample_scaled(atlas_color, in.tex_coord) * in.color.a;
    }
    let ic = vec2<i32>(in.tex_coord);
    if (in.atlas == ATLAS_GRAYSCALE) {
        let a = textureLoad(atlas_grayscale, ic, 0).r;
        return in.color * a;
    } else {
        return textureLoad(atlas_color, ic, 0) * in.color.a;
    }
}
