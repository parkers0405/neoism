float unit_hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord.xy / iResolution.xy;
    vec2 px = vec2(1.0) / iResolution.xy;
    vec4 base = sampleChannel0(uv);

    // Keep the center optically stable; color separation grows only near
    // the frame, where it reads as energized glass rather than blurry text.
    float edge = smoothstep(0.34, 0.76, distance(uv, vec2(0.5)));
    float split = edge * (0.55 + 0.25 * sin(iTime * 0.65));
    vec3 color;
    color.r = sampleChannel0(uv + vec2(px.x * split, 0.0)).r;
    color.g = base.g;
    color.b = sampleChannel0(uv - vec2(px.x * split, 0.0)).b;

    float top_bar = 1.0 - smoothstep(0.0, 0.004, abs(uv.y - 0.055));
    float bottom_bar = 1.0 - smoothstep(0.0, 0.0025, abs(uv.y - 0.945));
    float sweep_y = fract(iTime * 0.055);
    float sweep = 1.0 - smoothstep(0.0, 0.012, abs(uv.y - sweep_y));
    float scan = 0.985 + 0.015 * sin(fragCoord.y * 3.14159265);
    float vignette = smoothstep(0.88, 0.30, distance(uv, vec2(0.5)));
    float noise = (unit_hash(fragCoord + floor(iTime * 30.0)) - 0.5) * 0.006;

    color *= scan * mix(0.82, 1.0, vignette);
    color += vec3(0.34, 0.08, 0.52) * top_bar * 0.09;
    color += vec3(0.72, 1.0, 0.16) * bottom_bar * 0.08;
    color += vec3(0.72, 1.0, 0.16) * sweep * 0.018;
    color += noise;

    fragColor = vec4(color, base.a);
}