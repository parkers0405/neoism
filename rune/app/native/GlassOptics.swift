import Foundation
import Metal

/// Clockwise, window-space corners, exactly as drawn by the ordinary Rune trail.
struct GlassQuad: Equatable {
    var a: SIMD2<Float>, b: SIMD2<Float>, c: SIMD2<Float>, d: SIMD2<Float>
    init(_ a: SIMD2<Float>, _ b: SIMD2<Float>, _ c: SIMD2<Float>, _ d: SIMD2<Float>) {
        self.a = a; self.b = b; self.c = c; self.d = d
    }
    init(rect: SIMD4<Float>) {
        self.init(SIMD2(rect.x, rect.y), SIMD2(rect.x + rect.z, rect.y),
                  SIMD2(rect.x + rect.z, rect.y + rect.w), SIMD2(rect.x, rect.y + rect.w))
    }
}

// Original optical shader. No source from LiquidGlassTabBar is incorporated.
// A lens-sized GPU blit breaks the render-target feedback loop. This is zero-copy
// interop (the actual Rune MTLTexture/command buffer), not zero GPU memory traffic.
final class GlassOptics {
    enum Failure: Error { case allocation }
    let device: MTLDevice
    let format: MTLPixelFormat
    let pipeline: MTLRenderPipelineState
    var backdrop: MTLTexture?
    struct Uniforms {
        var ab: SIMD4<Float>
        var cd: SIMD4<Float>
        var region: SIMD4<Float>
        var light: SIMD4<Float>
    }
    init(device: MTLDevice, format: MTLPixelFormat, shaderSource: String = GlassOptics.shader) throws {
        self.device = device
        self.format = format
        let library = try device.makeLibrary(source: shaderSource, options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "glassVertex")
        descriptor.fragmentFunction = library.makeFunction(name: "glassFragment")
        descriptor.colorAttachments[0].pixelFormat = format
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        assert(MemoryLayout<Uniforms>.stride == 64)
    }
    // Rect convenience for standalone optical tests; production always passes the live quad.
    func encode(_ cb: MTLCommandBuffer, _ target: MTLTexture, rect: SIMD4<Float>,
                clip: SIMD4<Float>, velocity: SIMD4<Float>, scale: Float) throws {
        try encode(cb, target, quad: GlassQuad(rect: rect), clip: clip, velocity: SIMD2(velocity.x, velocity.y), scale: scale)
    }
    func encode(_ cb: MTLCommandBuffer, _ target: MTLTexture, quad: GlassQuad,
                clip: SIMD4<Float>, velocity: SIMD2<Float>, scale: Float) throws {
        // A modest optical enlargement around the same caret center preserves each
        // corner's trail while giving the lens and its rim more room around the glyph.
        let center = (quad.a + quad.b + quad.c + quad.d) * 0.25
        let points = [quad.a, quad.b, quad.c, quad.d].map { (center + ($0 - center) * 1.12) * scale }
        guard scale.isFinite && scale > 0 && points.allSatisfy({ $0.x.isFinite && $0.y.isFinite }) else { return }
        // Clamp before integer conversion, including offscreen trails during resize.
        let minX = Int(max(0, min(Float(target.width), floor(points.map { $0.x }.min()! - 2))))
        let minY = Int(max(0, min(Float(target.height), floor(points.map { $0.y }.min()! - 2))))
        let maxX = Int(max(0, min(Float(target.width), ceil(points.map { $0.x }.max()! + 2))))
        let maxY = Int(max(0, min(Float(target.height), ceil(points.map { $0.y }.max()! + 2))))
        guard maxX > minX, maxY > minY else { return }
        let width = maxX - minX, height = maxY - minY
        // Capacity buckets avoid allocations as the droplet stretches. Same-queue accesses
        // serialize reuse; old allocations survive resize via the completion handler below.
        if backdrop == nil || backdrop!.width < width || backdrop!.height < height {
            let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: target.pixelFormat,
                width: (width + 63) / 64 * 64, height: (height + 63) / 64 * 64, mipmapped: false)
            d.storageMode = .private; d.usage = [.shaderRead]
            backdrop = device.makeTexture(descriptor: d)
        }
        guard let backdrop, let blit = cb.makeBlitCommandEncoder() else { throw Failure.allocation }
        // Register before encoding, so even a subsequent encoder allocation failure is safe.
        cb.addCompletedHandler { [backdrop, target, pipeline] _ in withExtendedLifetime((backdrop, target, pipeline)) {} }
        blit.copy(from: target, sourceSlice: 0, sourceLevel: 0,
                  sourceOrigin: MTLOrigin(x: minX, y: minY, z: 0), sourceSize: MTLSize(width: width, height: height, depth: 1),
                  to: backdrop, destinationSlice: 0, destinationLevel: 0, destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
        blit.endEncoding()
        let x0 = max(minX, Int(ceil(clip.x * scale)))
        let y0 = max(minY, Int(ceil(clip.y * scale)))
        let x1 = min(maxX, Int(floor((clip.x + clip.z) * scale)))
        let y1 = min(maxY, Int(floor((clip.y + clip.w) * scale)))
        guard x1 > x0, y1 > y0 else { return }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .load
        pass.colorAttachments[0].storeAction = .store
        guard let encoder = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure.allocation }
        var u = Uniforms(ab: SIMD4(points[0].x, points[0].y, points[1].x, points[1].y),
                         cd: SIMD4(points[2].x, points[2].y, points[3].x, points[3].y),
                         region: SIMD4(Float(minX), Float(minY), Float(backdrop.width), Float(backdrop.height)),
                         light: SIMD4(velocity.x / 1600, velocity.y / 1600, scale, 0))
        encoder.setRenderPipelineState(pipeline)
        encoder.setViewport(MTLViewport(originX: Double(minX), originY: Double(minY), width: Double(width), height: Double(height), znear: 0, zfar: 1))
        encoder.setScissorRect(MTLScissorRect(x: x0, y: y0, width: x1 - x0, height: y1 - y0))
        encoder.setFragmentTexture(backdrop, index: 0)
        encoder.setFragmentBytes(&u, length: MemoryLayout<Uniforms>.stride, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        encoder.endEncoding()
        // Rune command buffers have unretained references. Capture ALL our GPU resources,
        // not the command buffer, until completion, including across disable/resize/close.
    }
    static let shader = """
    #include <metal_stdlib>
    using namespace metal;
    struct Uniforms { float4 ab; float4 cd; float4 region; float4 light; };
    vertex float4 glassVertex(uint id [[vertex_id]]) {
        float2 p = float2(id == 1 ? 3 : -1, id == 2 ? 3 : -1);
        return float4(p, 0, 1);
    }
    // Winding + nearest segment supports even concave shapes during rapid turns. Round
    // each actual spring corner with a short quadratic, NOT its axis-aligned bounds.
    void edgeDistance(float2 p, float2 a, float2 b, thread float &best,
                      thread float2 &delta, thread bool &inside) {
        float2 e = b - a;
        float2 v = p - (a + e * clamp(dot(p - a, e) / max(dot(e, e), 1e-8), 0.0, 1.0));
        float dd = dot(v, v);
        if (dd < best) { best = dd; delta = v; }
        if ((a.y > p.y) != (b.y > p.y)) {
            if (p.x < a.x + (p.y - a.y) * e.x / e.y) inside = !inside;
        }
    }
    float quadDistance(float2 p, constant Uniforms &u, thread float2 &normal) {
        float2 q[4] = {u.ab.xy, u.ab.zw, u.cd.xy, u.cd.zw};
        float speed = length(u.light.xy);
        float fluid = smoothstep(0.20, 0.85, speed);
        float2 direction = u.light.xy / max(speed, 1e-5);
        float2 center = (q[0] + q[1] + q[2] + q[3]) * 0.25;
        float thickness = min(min(length(q[1]-q[0]), length(q[2]-q[1])),
                              min(length(q[3]-q[2]), length(q[0]-q[3])));
        float2 start[4], end[4];
        for (uint i = 0; i < 4; ++i) {
            float2 before = q[(i + 3) % 4] - q[i], after = q[(i + 1) % 4] - q[i];
            float lb = max(length(before), 1e-5), la = max(length(after), 1e-5);
            float r = min(4.0 * u.light.z, min(lb, la) * 0.25);
            start[i] = q[i] + before * (r / lb); end[i] = q[i] + after * (r / la);
        }
        float best = 1e30; float2 delta = float2(1, 0); bool inside = false;
        for (uint i = 0; i < 4; ++i) {
            float2 a = end[(i + 3) % 4], b = start[i];
            if (fluid == 0.0) {
                // Preserve the original settled/slow silhouette bit for bit.
                edgeDistance(p, a, b, best, delta, inside);
            } else {
                float2 side = b - a;
                float alignment = dot(side / max(length(side), 1e-5), direction);
                float2 inward = center - (a + b) * 0.5;
                // Inward-only, bounded by local thickness: no extra translation,
                // disconnected lobe or larger copy rectangle, even on a rapid turn.
                float amount = fluid * alignment * alignment * min(3.0 * u.light.z, thickness * 0.12);
                float2 bend = inward / max(length(inward), 1e-5) * amount;
                float2 previous = a;
                for (uint k = 1; k <= 12; ++k) {
                    float t = float(k) / 12.0;
                    // Zero endpoint slope joins the original corner tangents. Bias
                    // the waist toward the tail, leaving the leading lobe fuller.
                    float envelope = 16.0 * t*t * (1.0-t)*(1.0-t);
                    float bias = 1.0 - 0.45 * alignment * (2.0*t - 1.0);
                    float2 next = mix(a, b, t) + bend * envelope * bias;
                    edgeDistance(p, previous, next, best, delta, inside);
                    previous = next;
                }
            }
            float2 prev = start[i];
            for (uint j = 1; j <= 4; ++j) {
                float t = float(j) * 0.25;
                float2 next = mix(mix(start[i], q[i], t), mix(q[i], end[i], t), t);
                edgeDistance(p, prev, next, best, delta, inside); prev = next;
            }
        }
        float distance = sqrt(best);
        normal = delta / max(distance, 1e-5) * (inside ? -1.0 : 1.0);
        return distance * (inside ? -1.0 : 1.0);
    }
    fragment float4 glassFragment(float4 pos [[position]], texture2d<float> scene [[texture(0)]],
                                  constant Uniforms &u [[buffer(0)]]) {
        constexpr sampler s(coord::normalized, address::clamp_to_edge, filter::linear);
        float2 center = (u.ab.xy + u.ab.zw + u.cd.xy + u.cd.zw) * 0.25;
        float2 p = pos.xy - center, normal;
        float d = quadDistance(pos.xy, u, normal);
        if (d > 0.5) discard_fragment();
        float edge = exp(-max(-d, 0.0) / max(1.0, 2.2 * u.light.z));
        // The face magnifies 1.22x. The convex shoulder bends farther inward. This
        // smooth low-strength map keeps text legible, rather than folding strokes.
        float2 displacement = -p * 0.18 - normal * (0.65 * u.light.z * edge);
        float2 uv = (pos.xy - u.region.xy + displacement) / u.region.zw;
        float2 prism = normal * edge * 0.65 * u.light.z / u.region.zw;
        float3 color = float3(scene.sample(s, uv + prism).r, scene.sample(s, uv).g, scene.sample(s, uv - prism).b);
        float2 light = normalize(float2(-0.65, -0.85) + clamp(u.light.xy, -0.7, 0.7));
        float specular = pow(max(dot(normal, light), 0.0), 8.0) * edge;
        float counter = pow(max(dot(normal, -light), 0.0), 5.0) * edge;
        color = color * (1.0 - counter * 0.16) + 0.025 + specular * 0.48;
        float rim = exp(-abs(d) / max(0.5, 0.45 * u.light.z));
        float4 original = scene.sample(s, (pos.xy - u.region.xy) / u.region.zw);
        // Lift the silhouette over dark whitespace, not the transparent optical center.
        // The opposite reflection keeps the unlit edge readable without a neon outline.
        float luminance = dot(original.rgb, float3(0.2126, 0.7152, 0.0722));
        float dark = 1.0 - smoothstep(0.06, 0.40, luminance);
        float bright = smoothstep(0.55, 0.90, luminance);
        color += rim * (0.07 + dark * 0.48 + 0.12 * specular);
        color += dark * edge * (specular * 0.18 + counter * 0.14);
        // A wider graphite bevel survives white backgrounds where additive highlights
        // wash out. Keep it at the shoulder, never frost or dim the optical center.
        float bevel = exp(-abs(d) / max(0.75, 0.9 * u.light.z));
        color = mix(color, float3(0.16, 0.19, 0.23), bright * bevel * 0.72);
        return float4(mix(original.rgb, color, smoothstep(0.5, -0.5, d)), original.a);
    }
    """
}