// Backdrop blur pyramid (dual filter): a 13-tap downsample chain (Jimenez) from the backdrop
// copy at half resolution, over the region being blurred (`u.map` places level 0 in the copy),
// then a 9-tap tent upsample chain back to half resolution. The level the up chain starts from
// sets the blur width; `u.w.x` crossfades it with the next finer level so the width is
// continuous. Every tap is clamped to the copied region, so texels outside it never leak in and
// edges repeat instead of darkening.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var fine: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

struct Level {
    // x: share of the coarser level in an up pass.
    w: vec4<f32>,
    // The region's bounds in the source's uv: min xy, max xy.
    region: vec4<f32>,
    // Source uv from target uv: uv * xy + zw.
    map: vec4<f32>,
    place: vec4<f32>,
};
@group(0) @binding(3) var<uniform> u: Level;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> V {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: V;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

fn s(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(src, smp, clamp(uv, u.region.xy, u.region.zw), 0.0);
}

@fragment
fn fs_down(in: V) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src, 0));
    let uv = in.uv * u.map.xy + u.map.zw;
    let a = s(uv + t * vec2<f32>(-2.0, -2.0));
    let b = s(uv + t * vec2<f32>(0.0, -2.0));
    let c = s(uv + t * vec2<f32>(2.0, -2.0));
    let d = s(uv + t * vec2<f32>(-1.0, -1.0));
    let e = s(uv + t * vec2<f32>(1.0, -1.0));
    let f = s(uv + t * vec2<f32>(-2.0, 0.0));
    let g = s(uv);
    let h = s(uv + t * vec2<f32>(2.0, 0.0));
    let i = s(uv + t * vec2<f32>(-1.0, 1.0));
    let j = s(uv + t * vec2<f32>(1.0, 1.0));
    let k = s(uv + t * vec2<f32>(-2.0, 2.0));
    let l = s(uv + t * vec2<f32>(0.0, 2.0));
    let m = s(uv + t * vec2<f32>(2.0, 2.0));
    return (d + e + i + j) * 0.125 + (a + b + g + f) * 0.03125 + (b + c + h + g) * 0.03125
         + (f + g + l + k) * 0.03125 + (g + h + m + l) * 0.03125;
}

@fragment
fn fs_up(in: V) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src, 0));
    let uv = in.uv * u.map.xy + u.map.zw;
    var c = s(uv) * 4.0;
    c += (s(uv + vec2<f32>(t.x, 0.0)) + s(uv - vec2<f32>(t.x, 0.0)) + s(uv + vec2<f32>(0.0, t.y)) + s(uv - vec2<f32>(0.0, t.y))) * 2.0;
    c += s(uv + t) + s(uv - t) + s(uv + vec2<f32>(t.x, -t.y)) + s(uv + vec2<f32>(-t.x, t.y));
    c /= 16.0;
    let f = textureSampleLevel(fine, smp, clamp(uv, u.region.xy, u.region.zw), 0.0);
    return mix(f, c, u.w.x);
}
