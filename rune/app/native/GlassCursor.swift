import AppKit
import Metal

// Installed once for the app, including when the optical keyboard cursor is disabled.
// The local monitor observes event types only and never consumes or records input.
private final class KeyboardMousePointer {
    static let shared = KeyboardMousePointer()
    private let events: Any?
    private let focus: NSObjectProtocol

    private init() {
        events = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .leftMouseDown, .rightMouseDown, .otherMouseDown]) { event in
            guard NSApp.isActive, NSApp.keyWindow != nil else { return event }
            NSCursor.setHiddenUntilMouseMoves(event.type == .keyDown)
            return event
        }
        focus = NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification,
                                                        object: nil, queue: .main) { _ in
            NSCursor.setHiddenUntilMouseMoves(false)
        }
    }
}

// The bridge has no NSView overlay: input, accessibility, safe areas and presentation all
// belong to Rune. Main-thread target updates and render-thread encoding share this lock.
final class CursorMotion: @unchecked Sendable {
    let lock = NSLock()
    var renderer: GlassOptics?
    var failed = false
    var dirty = false
    // Renderer readiness/failure must reach the UI even if another encoded frame
    // consumes dirty before the next poll. Cleared only by a UI acknowledgement.
    var needsPoll = false
    var enabled = false
    var active = false
    var visible = false
    var placed = false
    var quad = GlassQuad(rect: .zero)
    var velocity = SIMD2<Float>.zero
    var clip = SIMD4<Float>.zero

    // Geometry arrives already animated (and scroll-carried) by Rust's original Trail.
    // A second native spring would erase its independent leading/trailing corners.
    func update(_ next: GlassQuad, velocity: SIMD2<Float>, clip region: SIMD4<Float>,
                enabled: Bool, placed: Bool, visible: Bool, ready: Bool = false) {
        let wasEnabled = self.enabled, wasVisible = self.visible, wasPlaced = self.placed
        let wasActive = active
        let wasClip = clip
        self.enabled = enabled; self.visible = visible; clip = region
        let moved = next != quad || velocity != self.velocity
        quad = next; self.velocity = velocity; self.placed = placed && enabled
        // A temporarily absent tab/workspace layout is not backend failure.
        active = enabled && ready
        needsPoll = false
        dirty = dirty || moved || wasEnabled != enabled || wasVisible != visible
            || wasClip != clip || wasPlaced != self.placed || wasActive != active
    }
}
private let cursor = CursorMotion()

@_cdecl("neoism_glass_attach")
public func glassAttach(_ pointer: UnsafeMutableRawPointer) {
    if Thread.isMainThread && NSApp != nil { _ = KeyboardMousePointer.shared }
    cursor.lock.lock(); defer { cursor.lock.unlock() }
    cursor.enabled = false; cursor.active = false; cursor.placed = false
    cursor.renderer = nil; cursor.failed = false
    cursor.dirty = false; cursor.needsPoll = true
}

@_cdecl("neoism_glass_update")
public func glassUpdate(_ quad: UnsafePointer<Float>, _ velocity: UnsafePointer<Float>, _ clip: UnsafePointer<Float>,
                        _ enabled: Bool, _ placed: Bool, _ visible: Bool) -> Bool {
    guard Thread.isMainThread else { return false }
    let workspace = NSWorkspace.shared
    cursor.lock.lock(); defer { cursor.lock.unlock() }
    let allowed = enabled && !workspace.accessibilityDisplayShouldReduceTransparency
        && !workspace.accessibilityDisplayShouldIncreaseContrast
    cursor.update(GlassQuad(SIMD2(quad[0], quad[1]), SIMD2(quad[2], quad[3]),
                            SIMD2(quad[4], quad[5]), SIMD2(quad[6], quad[7])),
                  velocity: SIMD2(velocity[0], velocity[1]),
                  clip: SIMD4(clip[0], clip[1], clip[2], clip[3]), enabled: allowed,
                  placed: placed, visible: visible, ready: cursor.renderer != nil && !cursor.failed)
    return cursor.active
}

@_cdecl("neoism_glass_animating")
public func glassAnimating() -> Bool {
    cursor.lock.lock(); defer { cursor.lock.unlock() }
    return cursor.needsPoll || (cursor.dirty && cursor.renderer != nil && !cursor.failed)
}

@_cdecl("neoism_glass_reduce_motion")
public func glassReduceMotion() -> Bool {
    Thread.isMainThread && NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
}

// Shared accessibility policy only; selection never reads or changes cursor enablement.
@_cdecl("neoism_glass_selection_allowed")
public func glassSelectionAllowed() -> Bool {
    guard Thread.isMainThread else { return false }
    let workspace = NSWorkspace.shared
    return !workspace.accessibilityDisplayShouldReduceTransparency
        && !workspace.accessibilityDisplayShouldIncreaseContrast
}

@_cdecl("neoism_glass_encode")
public func glassEncode(_ buffer: UnsafeMutableRawPointer, _ texture: UnsafeMutableRawPointer, _ scale: Double) {
    autoreleasepool {
        let cb = Unmanaged<AnyObject>.fromOpaque(buffer).takeUnretainedValue() as! MTLCommandBuffer
        let drawable = Unmanaged<AnyObject>.fromOpaque(texture).takeUnretainedValue() as! MTLTexture
        cursor.lock.lock(); defer { cursor.lock.unlock() }
        cursor.dirty = false
        guard cursor.enabled && !cursor.failed else { return }
        do {
            if cursor.renderer == nil || cursor.renderer!.format != drawable.pixelFormat
                || cursor.renderer!.device.registryID != cb.device.registryID {
                let first = cursor.renderer == nil
                cursor.renderer = try GlassOptics(device: cb.device, format: drawable.pixelFormat)
                cursor.dirty = true
                cursor.needsPoll = true
                // The current scene still contains the fallback caret. Main's next update
                // acknowledges readiness and removes it before we render the native lens.
                if first { return }
            }
            guard cursor.active && cursor.placed && cursor.visible, let renderer = cursor.renderer else { return }
            try renderer.encode(cb, drawable, quad: cursor.quad, clip: cursor.clip,
                                velocity: cursor.velocity, scale: Float(scale))
        } catch {
            cursor.failed = true
            cursor.needsPoll = true
            NSLog("Neoism glass disabled: %@", String(describing: error))
        }
    }
}