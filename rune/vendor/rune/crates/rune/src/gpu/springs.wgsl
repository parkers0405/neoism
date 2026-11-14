// Coefficients are prepared once at retarget using rune-motion's damping branches.
struct SpringTrack {
    abcd: vec4<f32>,
    rates: vec2<f32>,
    goal: f32,
    kind: u32,
    start: vec2<f32>,
    duration: f32,
    pad: f32,
};

// Match rune_motion::math::exp for overdamped roots: near-critical coefficients
// nearly cancel, amplifying native-exp differences by four orders of magnitude.
fn spring_exp(x: f32) -> f32 {
    if (x < -87.0) { return 0.0; }
    let xc = clamp(x, -87.0, 88.0);
    let k = round(xc * 1.4426950408889634);
    let r = fma(-k, 0.0000014286068, fma(-k, 0.69314575, xc));
    let r2 = r * r;
    let a = 1.0 + r;
    let b = fma(r, 1.0 / 6.0, 0.5);
    let c = fma(r, 1.0 / 120.0, 1.0 / 24.0);
    let p = fma(r2, fma(r2, fma(r2, 1.0 / 720.0, c), b), a);
    return p * bitcast<f32>(u32(i32(k) + 127) << 23u);
}

// Split seconds preserve sub-frame precision after days of application uptime.
fn spring_sample(s: SpringTrack, time: vec2<f32>) -> vec2<f32> {
    // Integer subtraction is an optimization barrier against fast-math reassociating
    // (hi-hi)+(lo-lo) into lossy absolute f32 timestamps on Metal.
    let whole = f32(i32(time.x) - i32(s.start.x));
    let t = max(whole + (time.y - s.start.y), 0.0);
    if (s.kind == 0u || t >= s.duration) { return vec2<f32>(s.goal, 0.0); }
    let c = s.abcd;
    if (s.kind == 1u) {
        let e = exp(-s.rates.x * t);
        let co = cos(s.rates.y * t);
        let si = sin(s.rates.y * t);
        return vec2<f32>(s.goal + e * (c.x * co + c.y * si), e * (c.z * co + c.w * si));
    }
    if (s.kind == 2u) {
        let e = exp(-s.rates.x * t);
        return vec2<f32>(s.goal + (c.x + c.y * t) * e, (c.z + c.w * t) * e);
    }
    let e1 = spring_exp(c.z * t);
    let e2 = spring_exp(c.w * t);
    return vec2<f32>(s.goal + (c.x * e1 + c.y * e2), c.x * c.z * e1 + c.y * c.w * e2);
}

struct SpringNode { x: SpringTrack, y: SpringTrack, opacity: SpringTrack };
@group(1) @binding(0) var<storage, read> spring_nodes: array<SpringNode>;
@group(1) @binding(1) var<storage, read> spring_ids: array<u32>;

@vertex
fn vs_springs(input: In, @builtin(instance_index) instance: u32) -> Out {
    let node = spring_nodes[spring_ids[instance]];
    let time = g.pad;
    let offset = vec2<f32>(spring_sample(node.x, time).x, spring_sample(node.y, time).x);
    let alpha = clamp(spring_sample(node.opacity, time).x, 0.0, 1.0);
    var i = input;
    // This spike translates geometry under a fixed screen-space clip. No text reflow or scaling.
    i.rect = vec4<f32>(i.rect.xy + offset, i.rect.zw);
    i.fill *= alpha;
    i.border *= alpha;
    if (i.params.y > 3.5 && i.params.y < 4.5) { i.uv *= alpha; }
    // Vector endpoints are screen-space geometry, unlike image/glyph atlas UVs.
    if (i.params.y > 4.5 && i.params.y < 6.5) {
        i.radii += vec4<f32>(offset, offset);
        if (i.params.y > 5.5) { i.uv = vec4<f32>(i.uv.xy + offset, i.uv.zw); }
    }
    return base_vs(i);
}
