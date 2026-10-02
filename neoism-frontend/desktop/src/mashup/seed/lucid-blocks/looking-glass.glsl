void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord.xy / iResolution.xy;
    vec2 px = vec2(1.0) / iResolution.xy;
    vec4 base = sampleChannel0(uv);

    // Chromatic separation is confined to the outer glass, leaving editor and
    // chat text in the central 76% sampled exactly once.
    float frame = smoothstep(0.38, 0.69, distance(uv, vec2(0.5)));
    float split = frame * 0.75;
    vec3 color = base.rgb;
    color.r = sampleChannel0(uv + vec2(px.x * split, 0.0)).r;
    color.b = sampleChannel0(uv - vec2(px.x * split, 0.0)).b;

    vec2 cell = floor(uv * vec2(24.0, 15.0));
    float checker = mod(cell.x + cell.y, 2.0);
    float edge_x = smoothstep(0.985, 1.0, abs(uv.x * 2.0 - 1.0));
    float edge_y = smoothstep(0.975, 1.0, abs(uv.y * 2.0 - 1.0));
    float edge = max(edge_x, edge_y);
    vec3 card = mix(vec3(0.12, 0.72, 0.67), vec3(0.84, 0.24, 0.48), checker);
    color = mix(color, color + card * 0.12, edge);

    // A slow, narrow card dash crosses only the very top border.
    float dash = step(0.965, uv.y) * step(fract(uv.x * 18.0 - iTime * 0.18), 0.34);
    color += vec3(0.91, 0.74, 0.37) * dash * 0.055;
    fragColor = vec4(color, base.a);
}