// Draws a blurred backdrop (`backdrop_blur.wgsl`'s half-resolution result) inside a backdrop
// blur record's rounded rect, cut by its clip, at its opacity (`fill.a`). Group 0 is the uber
// shader's, so the record lands in whichever target (frame or isolated layer) is being drawn.
// Prepended with `clip.wgsl`.

struct Globals {
    viewport: vec2<f32>,
    pad: vec2<f32>,
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(3) var<uniform> projection: vec4<f32>;
@group(1) @binding(0) var blurred: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;

struct Level {
    w: vec4<f32>,
    region: vec4<f32>,
    map: vec4<f32>,
    // The pyramid's window origin xy and extent zw, in device pixels.
    place: vec4<f32>,
};
@group(1) @binding(2) var<uniform> u: Level;

struct In {
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) radii: vec4<f32>,
    @location(2) fill: vec4<f32>,
    @location(6) clip: vec4<f32>,
    @location(7) fade: vec4<u32>,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) alpha: f32,
    @location(4) @interpolate(flat) clip: vec4<f32>,
    @location(5) @interpolate(flat) fade: vec4<u32>,
};

@vertex
fn vs(i: In) -> Out {
    let corner = vec2<f32>(f32(i.vi & 1u), f32(i.vi >> 1u));
    let p = i.rect.xy - vec2<f32>(1.0) + corner * (i.rect.zw + vec2<f32>(2.0));
    let size_px = select(g.viewport, projection.zw, projection.z > 0.0);
    let local = p - projection.xy;
    var o: Out;
    o.pos = vec4<f32>(local.x / size_px.x * 2.0 - 1.0, 1.0 - local.y / size_px.y * 2.0, 0.0, 1.0);
    o.p = p;
    o.rect = i.rect;
    o.radii = i.radii;
    o.alpha = i.fill.a;
    o.clip = i.clip;
    o.fade = i.fade;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let half = i.rect.zw * 0.5;
    let d = sd_rounded(i.p - (i.rect.xy + half), half, i.radii);
    let cov = clamp(0.5 - d, 0.0, 1.0) * clip_alpha(i.p, i.clip, i.fade) * i.alpha;
    let col = textureSampleLevel(blurred, smp, (i.p - u.place.xy) / u.place.zw, 0.0);
    return col * cov;
}
