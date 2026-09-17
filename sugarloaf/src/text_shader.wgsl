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
};

struct TextVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) atlas: u32,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) tex_coord: vec2<f32>,
    @location(3) @interpolate(flat) clip_rect: vec4<f32>,
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
    let quad_px = origin + size * corner * scale;

    // Pixel → NDC (y-flip).
    let vp = text_uniforms.viewport.xy;
    let ndc = vec2<f32>(
        (quad_px.x / vp.x) * 2.0 - 1.0,
        1.0 - (quad_px.y / vp.y) * 2.0,
    );

    var out: TextVsOut;
    out.position  = vec4<f32>(ndc, 0.0, 1.0);
    out.tex_coord = vec2<f32>(in.glyph_pos) + size * corner;
    out.atlas = in.atlas_pack.x | select(0u, 2u, in.raster_scale > 0.0);
    out.clip_rect = in.clip_rect;

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

    if ((in.atlas & 2u) != 0u) {
        if ((in.atlas & 1u) == 0u) { return in.color * sample_scaled(atlas_grayscale, in.tex_coord).r; }
        return sample_scaled(atlas_color, in.tex_coord);
    }
    let ic = vec2<i32>(in.tex_coord);
    if (in.atlas == ATLAS_GRAYSCALE) {
        let a = textureLoad(atlas_grayscale, ic, 0).r;
        return in.color * a;
    } else {
        return textureLoad(atlas_color, ic, 0);
    }
}
