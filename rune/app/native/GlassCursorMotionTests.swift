import Foundation
import Metal

@main struct GlassCursorMotionTests {
    static func main() {
        let motion = CursorMotion()
        let region = SIMD4<Float>(0, 0, 1200, 800)
        var quad = GlassQuad(rect: SIMD4(200, 180, 10, 22))
        func update(enabled: Bool = true, placed: Bool = true, visible: Bool = true,
                    velocity: SIMD2<Float> = .zero, ready: Bool = true) {
            motion.update(quad, velocity: velocity, clip: region, enabled: enabled,
                          placed: placed, visible: visible, ready: ready)
        }
        update()
        precondition(motion.quad == quad, "first appearance is exact")
        // A diagonal/turning Rust Trail sample must not become a bounding rectangle,
        // nor be interpolated a second time by the native renderer.
        quad = GlassQuad(SIMD2(201, 181), SIMD2(270, 130), SIMD2(275, 150), SIMD2(220, 195))
        update(velocity: SIMD2(500, -200))
        precondition(motion.quad == quad && motion.velocity == SIMD2(500, -200) && motion.dirty)
        motion.dirty = false
        update(velocity: SIMD2(500, -200))
        precondition(!motion.dirty, "identical polls do not request more frames")
        let before = quad
        let delta = SIMD2<Float>(0, -0.333333)
        quad = GlassQuad(quad.a + delta, quad.b + delta, quad.c + delta, quad.d + delta)
        update(velocity: SIMD2(500, -200))
        precondition(motion.quad == quad && motion.quad.a == before.a + delta,
                     "fractional scroll transports the entire asymmetric shape immediately")
        motion.dirty = false
        update(visible: false)
        precondition(motion.dirty, "resting blink invalidates native frame")
        update(placed: false)
        precondition(motion.active, "missing layout does not revoke native ownership")
        quad = GlassQuad(rect: SIMD4(50, 30, 3, 24))
        update()
        precondition(motion.quad == quad, "reappearance uses current Rust geometry")
        update(enabled: false)
        quad = GlassQuad(rect: SIMD4(500, 400, 9, 20))
        update()
        precondition(motion.quad == quad && motion.velocity == .zero,
                     "reenable/reduced-motion snap has no native lag")
        precondition(motion.clip == region)
        motion.dirty = false
        update(ready: false)
        precondition(!motion.active && motion.dirty, "backend loss invalidates a settled cursor")
        motion.dirty = false
        update()
        precondition(motion.active && motion.dirty, "readiness acknowledgement requests a frame")
        nativeHandoff()
        print("PASS: exact independent corners, scroll transport, invalidation, visibility and reenable")
    }

    static func nativeHandoff() {
        guard glassSelectionAllowed() else { print("SKIP: native handoff blocked by accessibility policy"); return }
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            fatalError("Native handoff test requires Metal")
        }
        let desc = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm,
                                                           width: 128, height: 128, mipmapped: false)
        desc.usage = [.renderTarget, .shaderRead]
        let texture = device.makeTexture(descriptor: desc)!
        let q: [Float] = [30, 30, 40, 30, 40, 52, 30, 52]
        let v: [Float] = [0, 0], clip: [Float] = [0, 0, 128, 128]
        func update(_ enabled: Bool = true, _ placed: Bool = true) -> Bool {
            q.withUnsafeBufferPointer { q in v.withUnsafeBufferPointer { v in clip.withUnsafeBufferPointer { c in
                glassUpdate(q.baseAddress!, v.baseAddress!, c.baseAddress!, enabled, placed, placed)
            } } }
        }
        func frame() {
            let cb = queue.makeCommandBuffer()!
            glassEncode(Unmanaged.passUnretained(cb as AnyObject).toOpaque(),
                        Unmanaged.passUnretained(texture as AnyObject).toOpaque(), 1)
            cb.commit(); cb.waitUntilCompleted()
            precondition(cb.status == .completed)
        }
        // No installed window or config is needed: exercise the actual exported bridge
        // against the same command-buffer/texture objects used by Rune's native hook.
        glassAttach(Unmanaged.passUnretained(texture as AnyObject).toOpaque())
        precondition(!update(), "ordinary cursor remains until a native backend is proven")
        frame()
        frame() // extra frame must not lose the readiness notification
        precondition(glassAnimating(), "readiness survives encoded frames until UI acknowledgement")
        precondition(update())
        frame()
        precondition(!glassAnimating(), "acknowledged settled native frame must stop waking")
        precondition(update(true, false), "tab/workspace layout gap retains renderer ownership")
        frame()
        precondition(update())
        frame()
        precondition(!update(false), "true popup/disabled policy restores fallback")
        frame()
        precondition(update(), "reenabling a ready backend does not flash fallback")
        frame()
        print("PASS: real Metal bridge readiness, layout gap, disable and reenable")
    }
}