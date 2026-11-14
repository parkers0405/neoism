struct Uniforms {
    config: vec4<f32>,
    mapping: vec4<f32>,
    taps: array<vec4<f32>, 128>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@group(0) @binding(2) var<uniform> u: Uniforms;
override MODE: u32 = 0;
override ADDITIVE: bool = false;

@vertex
fn vs(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32((v << 1u) & 2u) * 2.0 - 1.0, f32(v & 2u) * 2.0 - 1.0, 0.0, 1.0);
}

fn sample_zero(p: vec2<f32>) -> vec4<f32> {
    if (ADDITIVE) {
        // Plus-lighter saturates each source texel BEFORE interpolation or convolution.
        let q = p - 0.5;
        let base = vec2<i32>(floor(q));
        let fraction = fract(q);
        let a = bounded_load(base);
        let b = bounded_load(base + vec2(1, 0));
        let c = bounded_load(base + vec2(0, 1));
        let d = bounded_load(base + vec2(1, 1));
        return mix(mix(a, b, fraction.x), mix(c, d, fraction.x), fraction.y);
    }
    // Explicit transparent border, independent of bucket padding. Clamping the sample to the
    // used texel centres then attenuating the edge implements bilinear zero extension exactly.
    let edge = clamp(p + 0.5, vec2(0.0), vec2(1.0)) * clamp(u.config.xy + 0.5 - p, vec2(0.0), vec2(1.0));
    let uv = clamp(p, vec2(0.5), u.config.xy - 0.5) / vec2<f32>(textureDimensions(source));
    return textureSampleLevel(source, linear, uv, 0.0) * edge.x * edge.y;
}

fn bounded_load(p: vec2<i32>) -> vec4<f32> {
    if (any(p < vec2(0)) || any(p >= vec2<i32>(u.config.xy))) { return vec4(0.0); }
    let color = textureLoad(source, p, 0);
    return select(color, clamp(color, vec4(0.0), vec4(1.0)), ADDITIVE);
}

@fragment
fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    if (MODE == 2u) {
        // Exact area prefilter for fractional reductions: no missed pixels or phase-dependent
        // brightness as the continuously animated sigma changes the intermediate dimensions.
        let lo = (p.xy - 0.5) * u.mapping.xy;
        let hi = (p.xy + 0.5) * u.mapping.xy;
        var sum = vec4(0.0);
        for (var y = i32(floor(lo.y)); y < i32(ceil(hi.y)); y++) {
            let wy = max(0.0, min(hi.y, f32(y + 1)) - max(lo.y, f32(y)));
            for (var x = i32(floor(lo.x)); x < i32(ceil(hi.x)); x++) {
                let wx = max(0.0, min(hi.x, f32(x + 1)) - max(lo.x, f32(x)));
                sum += bounded_load(vec2(x, y)) * wx * wy;
            }
        }
        return sum / (u.mapping.x * u.mapping.y);
    }
    if (MODE == 3u) { return sample_zero(p.xy * u.mapping.xy); }
    let axis = select(vec2(1.0, 0.0), vec2(0.0, 1.0), u.config.z > 0.5);
    var color = sample_zero(p.xy) * u.taps[0].y;
    for (var tap = 1u; tap < u32(u.config.w); tap++) {
        let offset = axis * u.taps[tap].x;
        color += (sample_zero(p.xy + offset) + sample_zero(p.xy - offset)) * u.taps[tap].y;
    }
    return color;
}
