// Ordered optical material: the backdrop is copied BEFORE the original rail icons.
struct Uniform { rect: vec4f, screen: vec4f, style: vec4f }
@group(0) @binding(0) var<uniform> u: Uniform;
@group(0) @binding(1) var backdrop: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let corners = array<vec2f, 6>(vec2f(0,0), vec2f(1,0), vec2f(0,1), vec2f(0,1), vec2f(1,0), vec2f(1,1));
    let p = u.rect.xy - vec2f(4 * u.screen.z) + corners[i] * (u.rect.zw + vec2f(8 * u.screen.z));
    return vec4f(p / u.screen.xy * vec2f(2,-2) + vec2f(-1,1), 0, 1);
}
fn sample_at(p: vec2f) -> vec3f {
    let size = vec2i(textureDimensions(backdrop));
    return textureLoad(backdrop, clamp(vec2i(p), vec2i(0), size - vec2i(1)), 0).rgb;
}
@fragment fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let scale = u.screen.z;
    let center = u.rect.xy + u.rect.zw * 0.5;
    let p = (pos.xy - center) / scale;
    let stretch = u.screen.w;
    let halfsize = u.rect.zw / scale * 0.5 + vec2f(-stretch, 2.5 * stretch);
    let waist = stretch * 0.8 * pow(max(0.0, 1.0 - abs(p.y) / halfsize.y), 2.0);
    let q = abs(p) - halfsize + vec2f(14.0 + waist, 14.0);
    let distance = length(max(q, vec2f(0))) + min(max(q.x, q.y), 0.0) - 14.0;
    let coverage = 1.0 - smoothstep(-0.7 / scale, 0.7 / scale, distance);
    if coverage <= 0.0 { discard; }
    let normal = normalize(p + vec2f(0.001));
    let shoulder = exp(-abs(distance) / 2.2);
    let source = center + p * scale / 1.045 - normal * shoulder * scale * 1.5;
    let prism = normal * shoulder * scale * 0.35;
    var color = vec3f(sample_at(source + prism).r, sample_at(source).g, sample_at(source - prism).b);
    let light = u.style.x;
    // Neutral tint and opposite-polarity rim retain contrast on white and black rails.
    color = mix(color, vec3f(select(1.0, 0.0, light > 0.5)), select(0.018, 0.025, light > 0.5));
    let rim = exp(-abs(distance + 0.65) / 0.55);
    let shine = clamp(0.5 - normal.y * 0.35 - normal.x * 0.15, 0.0, 1.0);
    color = mix(color, vec3f(select(0.38, 0.35, light > 0.5)), rim * (0.16 + shine * 0.2));
    return vec4f(color * coverage, coverage);
}