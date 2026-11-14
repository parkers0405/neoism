// Standalone GPU integration checks; readback exists ONLY in this test executable.
// xcrun swiftc native/GlassOptics.swift native/GlassOpticsTests.swift -o /tmp/neoism-glass-tests
import Foundation
import Metal

@main struct GlassOpticsTests {
    static func main() throws {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            fatalError("Metal is required for optical cursor integration tests")
        }
        let optics = try GlassOptics(device: device, format: .rgba8Unorm)
        let original = try GlassOptics(device: device, format: .rgba8Unorm,
            shaderSource: GlassOptics.shader.replacingOccurrences(of: "float fluid = smoothstep(0.20, 0.85, speed);", with: "float fluid = 0.0;"))
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: 96, height: 96, mipmapped: false)
        d.storageMode = .shared; d.usage = [.renderTarget, .shaderRead]
        let target = device.makeTexture(descriptor: d)!
        func render(_ source: [UInt8], clip: SIMD4<Float> = SIMD4(0, 0, 96, 96), velocity: SIMD4<Float> = .zero,
                    quad: GlassQuad = GlassQuad(rect: SIMD4(32, 24, 28, 48)),
                    renderer: GlassOptics? = nil) throws -> [UInt8] {
            source.withUnsafeBytes { target.replace(region: MTLRegionMake2D(0, 0, 96, 96), mipmapLevel: 0, withBytes: $0.baseAddress!, bytesPerRow: 96 * 4) }
            let cb = queue.makeCommandBufferWithUnretainedReferences()!
            try (renderer ?? optics).encode(cb, target, quad: quad, clip: clip, velocity: SIMD2(velocity.x, velocity.y), scale: 1)
            cb.commit(); cb.waitUntilCompleted()
            precondition(cb.status == .completed, "GPU error: \(String(describing: cb.error))")
            var pixels = [UInt8](repeating: 0, count: source.count)
            pixels.withUnsafeMutableBytes { target.getBytes($0.baseAddress!, bytesPerRow: 96 * 4, from: MTLRegionMake2D(0, 0, 96, 96), mipmapLevel: 0) }
            return pixels
        }
        func solid(_ r: UInt8, _ g: UInt8, _ b: UInt8) -> [UInt8] {
            (0..<96*96).flatMap { _ in [r, g, b, 255] }
        }
        func pixel(_ data: [UInt8], _ x: Int, _ y: Int, _ c: Int = 0) -> Int { Int(data[(y * 96 + x) * 4 + c]) }
        let red = try render(solid(180, 0, 0))
        let blue = try render(solid(0, 0, 180))
        precondition(pixel(red, 46, 48) > 170 && pixel(blue, 46, 48) < 20)
        precondition(pixel(blue, 46, 48, 2) > 170, "must sample THIS frame, not a cached snapshot")
        precondition(pixel(blue, 10, 10) == 0 && pixel(blue, 10, 10, 2) == 180, "outside silhouette unchanged")
        let black = solid(0, 0, 0)
        let clipped = try render(black, clip: SIMD4(40, 30, 10, 20))
        for y in 0..<96 { for x in 0..<96 where x < 40 || x >= 50 || y < 30 || y >= 50 {
            precondition(pixel(clipped, x, y) == 0, "clip must contain highlights and deformation")
        } }
        let still = try render(black)
        precondition(pixel(still, 61, 48) >= 40, "dark background needs a visible resting rim even on the unlit side")
        precondition(pixel(still, 31, 48) > 0 && pixel(still, 61, 48) > 0,
                     "lens must enlarge beyond both original sides around the same center")
        precondition(pixel(still, 46, 48) < 15, "contrast treatment must not frost the clear center")
        let white = try render(solid(255, 255, 255))
        precondition(pixel(white, 61, 48) < 190 && pixel(white, 30, 48) < 190,
                     "both lit and unlit edges must remain distinct on white")
        precondition(pixel(white, 46, 48) > 245, "light-mode contrast must preserve the transparent center")
        let paper = try render(solid(235, 235, 235))
        precondition(pixel(paper, 61, 48) < 185, "off-white themes also need a visible bevel")
        let moving = try render(black, velocity: SIMD4(500, 0, 0, 0))
        precondition(still != moving, "motion must affect silhouette/specular reflection")
        let originalStill = try render(black, renderer: original)
        precondition(still == originalStill, "exact original settled rendering")
        let slow = SIMD4<Float>(120, 0, 0, 0)
        let slowNew = try render(black, velocity: slow)
        let slowOriginal = try render(black, velocity: slow, renderer: original)
        precondition(slowNew == slowOriginal, "small movements retain original optics")
        let fast = SIMD4<Float>(0, 1600, 0, 0)
        let droplet = try render(black, velocity: fast)
        let rigid = try render(black, velocity: fast, renderer: original)
        precondition(pixel(droplet, 31, 48) == 0 && pixel(rigid, 31, 48) > 0, "speed must curve silhouette inward, not only change lighting")
        precondition(pixel(droplet, 33, 48) > 0, "neck stays connected")
        for y in 27..<69 { precondition(pixel(droplet, 46, y) > 0, "continuous droplet centerline") }
        let reversed = try render(black, velocity: -fast)
        let mask = stride(from: 0, to: black.count, by: 4).map { droplet[$0] > 0 }
        let reverseMask = stride(from: 0, to: black.count, by: 4).map { reversed[$0] > 0 }
        precondition(mask != reverseMask, "tail bias must follow travel direction, independently of lighting")
        let settledAgain = try render(black)
        precondition(settledAgain == originalStill, "motion leaves no residual distortion")
        let fastClip = try render(black, clip: SIMD4(40, 30, 10, 20), velocity: fast)
        for y in 0..<96 { for x in 0..<96 where x < 40 || x >= 50 || y < 30 || y >= 50 {
            precondition(pixel(fastClip, x, y) == 0, "moving curvature stays scissored")
        } }
        var ramp = black
        for y in 0..<96 { for x in 0..<96 { for c in 0..<3 { ramp[(y * 96 + x) * 4 + c] = UInt8(x * 2) } } }
        let refracted = try render(ramp)
        let curvedRamp = try render(ramp, velocity: fast)
        precondition(pixel(curvedRamp, 33, 40, 0) != pixel(curvedRamp, 33, 40, 2), "curved shoulder normals disperse RGB")
        precondition(abs(pixel(curvedRamp, 46, 48) - pixel(refracted, 46, 48)) <= 1, "curvature preserves glyph center")
        var glyph = black
        for y in 38..<59 { for x in 44..<49 { for c in 0..<3 { glyph[(y * 96 + x) * 4 + c] = 220 } } }
        let glyphMoving = try render(glyph, velocity: fast)
        let glyphOriginal = try render(glyph, velocity: fast, renderer: original)
        for y in 42..<55 { for x in 43..<50 {
            precondition(abs(pixel(glyphMoving, x, y) - pixel(glyphOriginal, x, y)) <= 1, "central glyph remains legible and registered")
        } }
        precondition(pixel(refracted, 35, 48) > pixel(ramp, 35, 48) + 4, "face must sample inward (magnification)")
        precondition(pixel(refracted, 31, 48, 0) != pixel(refracted, 31, 48, 2), "prism edge must disperse RGB")
        let diagonal = GlassQuad(SIMD2(20, 20), SIMD2(65, 40), SIMD2(70, 75), SIMD2(40, 60))
        let deformed = try render(black, quad: diagonal)
        precondition(pixel(deformed, 22, 70) == 0, "quad bounds are not the lens silhouette")
        precondition(pixel(deformed, 48, 48) > 0, "interior of asymmetric lens renders")
        let skewRamp = try render(ramp, quad: diagonal)
        let boundsRamp = try render(ramp, quad: GlassQuad(rect: SIMD4(20, 20, 50, 55)))
        precondition(skewRamp != boundsRamp, "deformation must reach optics, not just the copy bounds")
        // On the slanted right edge (x~67 at y55), RGB normals must tilt with
        // the real edge rather than the axis-aligned bounding rectangle at x70.
        precondition(pixel(skewRamp, 66, 55, 0) != pixel(skewRamp, 66, 55, 2), "deformed shoulder refracts/disperses")
        let turn = GlassQuad(SIMD2(60, 20), SIMD2(75, 28), SIMD2(30, 75), SIMD2(20, 50))
        let turned = try render(black, quad: turn)
        precondition(turned != deformed && pixel(turned, 72, 70) == 0, "turn changes independent corner geometry")
        // Resizing releases old texture only once all prior command buffers complete.
        var pending: [MTLCommandBuffer] = []
        for i in 1...12 {
            let cb = queue.makeCommandBufferWithUnretainedReferences()!
            try optics.encode(cb, target, rect: SIMD4(5, 5, Float(i * 6), 65), clip: SIMD4(0, 0, 96, 96), velocity: .zero, scale: 1)
            cb.commit(); pending.append(cb)
        }
        pending.last!.waitUntilCompleted()
        precondition(pending.allSatisfy { $0.status == .completed })
        for scale: Float in [1, 2] {
            for rect: SIMD4<Float> in [SIMD4(-20, -10, 28, 48), SIMD4(90, 90, 28, 48), SIMD4(40, 30, 1, 24)] {
                let cb = queue.makeCommandBufferWithUnretainedReferences()!
                try optics.encode(cb, target, rect: rect, clip: SIMD4(0, 0, 48, 48), velocity: SIMD4(1600, -1200, 0, 0), scale: scale)
                cb.commit(); cb.waitUntilCompleted(); precondition(cb.status == .completed)
            }
        }
        print("PASS: live source, exact settled/slow optics, curved moving silhouette/normals, glyph center, magnification, RGB dispersion, clipping, resize and unretained-resource lifetime")
    }
}