// Shared clip maths: the uber shader and compositor shaders (`rune::render::CLIP_WGSL`) use these.
// `clip_alpha(p, clip, fade)` takes a window device-pixel position, an instance's `clip` and its
// `fade` words (`vec4<u32>`), and returns the coverage of its rectangular, soft-edged and rounded
// (circular or continuous) clip.

// Apple's continuous corner of radius r: its curve leaves each edge K·r from the corner, and
// from the point (K·r, K·r) inside the corner it lies K·r·g(u) away, u running -1..1 across
// the quarter turn (0 on the diagonal; within 0.32% of r). Each quadrant takes its own
// corner's whole curve, as Core Animation does when K·r passes half the shorter side.
const CONTINUOUS_K: f32 = 1.52866483;
fn sd_continuous(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let e = CONTINUOUS_K * r;
    let q = half - abs(p);
    let v = vec2<f32>(e) - q;
    if (v.x <= 0.0 || v.y <= 0.0) { return max(-q.x, -q.y); }
    let u = atan2(v.y, v.x) * (4.0 / 3.14159265) - 1.0;
    let u2 = u * u;
    return length(v) - e * (1.14602295 + u2 * (-0.37573161 + u2 * (0.30796609 - 0.07724354 * u2)));
}

fn sd_rounded(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    // radii: top-left, top-right, bottom-right, bottom-left (y down). Negative is continuous.
    var r = radii.x;
    if (p.x > 0.0) {
        r = select(radii.y, radii.z, p.y > 0.0);
    } else {
        r = select(radii.x, radii.w, p.y > 0.0);
    }
    if (r < 0.0) { return sd_continuous(p, half, min(-r, min(half.x, half.y))); }
    r = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

// A soft edge's coverage `d` in from it: smoothstep over `fade`, linear over `-fade` when it's
// negative (`El::clip_fade_linear`), and a hard, anti-aliased edge at zero.
fn ramp(d: f32, fade: f32) -> f32 {
    if (fade == 0.0) {
        return clamp(d + 0.5, 0.0, 1.0);
    }
    let x = clamp(d / abs(fade), 0.0, 1.0);
    if (fade < 0.0) {
        return x;
    }
    return x * x * (3.0 - 2.0 * x);
}

fn unpack_clip_radii(w: u32) -> vec4<f32> {
    if (w == 0u) { return vec4<f32>(0.0); }
    if ((w & 0x60000000u) != 0x60000000u) {
        return vec4<f32>(bitcast<f32>(w));
    }
    let step = select(0.5, 2.0, (w & 0x10000000u) != 0u);
    let sgn = select(1.0, -1.0, (w & 0x80000000u) != 0u);
    return vec4<f32>(
        f32(w & 0x7fu),
        f32((w >> 7u) & 0x7fu),
        f32((w >> 14u) & 0x7fu),
        f32((w >> 21u) & 0x7fu),
    ) * (step * sgn);
}

fn clip_alpha(p: vec2<f32>, c: vec4<f32>, packed: vec4<u32>) -> f32 {
    let f = vec4<f32>(unpack2x16float(packed.x), unpack2x16float(packed.y));
    let offset = unpack4x8unorm(packed.z);
    let hard = vec4<f32>(p - c.xy, c.zw - p);
    var alpha = 1.0;
    for (var edge = 0u; edge < 4u; edge++) {
        // Keep the original sub-pixel ramp exactly when its origin is the hard edge.
        var a = ramp(hard[edge], f[edge]);
        if offset[edge] >= 1.0 {
            a = ramp(hard[edge], 0.0);
        } else if offset[edge] > 0.0 {
            a = min(ramp(hard[edge], 0.0), ramp(hard[edge] + offset[edge] * abs(f[edge]), f[edge]));
        }
        alpha *= a;
    }
    if packed.w != 0u {
        let half = (c.zw - c.xy) * 0.5;
        alpha *= clamp(0.5 - sd_rounded(p - c.xy - half, half, unpack_clip_radii(packed.w)), 0.0, 1.0);
    }
    return alpha;
}
