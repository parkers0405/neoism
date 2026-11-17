// Trackpad scrolling as a hand does it, for frame-pacing checks (`NEOISM_RUNE_DRIVE`,
// `hand:<seconds>`): a finger phase that speeds up and eases off, lift-off, then the momentum
// phase macOS adds, decaying until it stops or the next touch catches it. Runs of flicks,
// slow reading drags, reversals, pauses.
//
// The events are real scroll `NSEvent`s (precise deltas, gesture and momentum phases) made
// on a thread with its own jittered ~120 Hz clock and handed to the window's view on the
// main thread, so they take the path a trackpad's do from the view down (winit's
// `scrollWheel:`, its event loop, Rune's wheel handling) and land between frames wherever
// they land. Unlike a system-wide event tap this needs no Accessibility permission.
import AppKit
import CoreGraphics

private struct Rng {
    var s: UInt64
    mutating func next() -> Double { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return Double(s % 1_000_000) / 1_000_000 }
    mutating func range(_ a: Double, _ b: Double) -> Double { a + (b - a) * next() }
}

private final class Hand {
    var rng: Rng
    let seconds: Double
    let profile: Int32
    let window: NSWindow
    let windowNumber: Int
    /// Over the transcript: right of centre, above the composer. Window points, bottom-left origin.
    let spot: NSPoint
    let screenTop: CGFloat
    let start = Date()
    var residual = 0.0
    /// Points above the newest message, as far as this run has scrolled.
    var travelled = 0.0
    var posted = 0

    init(seconds: Double, seed: UInt64, profile: Int32, window: NSWindow) {
        self.rng = Rng(s: seed == 0 ? 0x9E3779B97F4A7C15 : seed)
        self.seconds = seconds; self.profile = profile; self.window = window
        windowNumber = window.windowNumber
        let frame = window.frame
        spot = NSPoint(x: frame.width * 0.6, y: frame.height * 0.55)
        screenTop = NSScreen.screens.first?.frame.maxY ?? frame.maxY
    }

    func elapsed() -> Double { Date().timeIntervalSince(start) }

    /// The pointer rests over the transcript, wherever the real one is.
    func park() {
        if let e = NSEvent.mouseEvent(with: .mouseMoved, location: spot, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                      windowNumber: windowNumber, context: nil, eventNumber: 0, clickCount: 0, pressure: 0) {
            NSApp.postEvent(e, atStart: false)
        }
    }

    // Scroll phase: began 1, changed 2, ended 4. Momentum phase: begin 1, continue 2, end 3.
    func post(_ dy: Double, phase: Int64, momentum: Int64) {
        // Whole pixels, the remainder carried, as the system's own deltas are.
        residual += dy
        let whole = residual.rounded(.towardZero)
        residual -= whole
        travelled = max(0, travelled + whole)
        guard let c = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: Int32(whole), wheel2: 0, wheel3: 0) else { return }
        c.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
        c.setIntegerValueField(.scrollWheelEventPointDeltaAxis1, value: Int64(whole))
        c.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: whole / 10)
        c.setIntegerValueField(.scrollWheelEventScrollPhase, value: phase)
        c.setIntegerValueField(.scrollWheelEventMomentumPhase, value: momentum)
        // With no window, `locationInWindow` reads as screen points (bottom-left origin):
        // place the event so the view finds the pointer over the transcript.
        c.location = CGPoint(x: spot.x, y: screenTop - spot.y)
        guard let e = NSEvent(cgEvent: c) else { return }
        // An event that did not come through the window server names no window, so
        // NSApplication would drop it: hand it to the window's view (winit's), on the main
        // thread, as `-[NSWindow sendEvent:]` does.
        let window = self.window
        DispatchQueue.main.async { window.contentView?.scrollWheel(with: e) }
        posted += 1
    }

    /// One event interval: the trackpad's ~120 Hz reports with a little jitter, now and then late.
    func tick() -> Double {
        var dt = 1.0 / 120.0 + rng.range(-0.0009, 0.0009)
        if rng.next() < 0.02 { dt += rng.range(0.002, 0.007) }
        Thread.sleep(forTimeInterval: dt)
        return dt
    }

    /// A finger stroke: `distance` points over `duration`. Returns the speed at lift-off.
    func stroke(distance: Double, duration: Double, liftSpeed: Double) -> Double {
        park()
        post(0, phase: 1, momentum: 0)
        var t = 0.0, last = 0.0, speed = 0.0
        while t < duration && elapsed() < seconds {
            let dt = tick(); t = min(duration, t + dt)
            // Smoothstep for position, blended towards one that is still moving at lift-off.
            let x = t / duration
            let shape = (1 - liftSpeed) * (x * x * (3 - 2 * x)) + liftSpeed * (x * x * (2 - x))
            let now = distance * shape + rng.range(-0.4, 0.4)
            speed = (now - last) / dt
            post(now - last, phase: 2, momentum: 0)
            last = now
        }
        post(0, phase: 4, momentum: 0)
        return speed
    }

    /// The system's momentum after a flick: exponential decay, until slow or `cut` seconds in.
    func coast(from speed: Double, cut: Double) {
        var v = speed, t = 0.0, first = true
        while abs(v) > 22 && t < cut && elapsed() < seconds {
            let dt = tick(); t += dt
            v *= exp(-dt / 0.42)
            post(v * dt, phase: 0, momentum: first ? 1 : 2)
            first = false
        }
        if !first { post(0, phase: 0, momentum: 3) }
    }

    func rest(_ s: Double) { Thread.sleep(forTimeInterval: min(s, max(0, seconds - elapsed()))) }

    func run() {
        var direction = 1.0 // positive deltas scroll towards older messages
        while elapsed() < seconds {
            let kind: Int32
            if profile != 0 { kind = profile } else { let r = rng.next(); kind = r < 0.5 ? 1 : r < 0.8 ? 2 : 3 }
            if rng.next() < 0.3 { direction = -direction }
            // Stay off the newest edge, where there is nothing to scroll: a reader near the
            // bottom heads up into the history.
            if travelled < 1500 { direction = 1 }
            switch kind {
            case 2:
                // Reading: a slow drag with the fingers down, a pause, another.
                for _ in 0..<Int(rng.range(1, 4)) {
                    _ = stroke(distance: direction * rng.range(60, 260), duration: rng.range(0.35, 0.9), liftSpeed: 0.05)
                    rest(rng.range(0.15, 0.6))
                }
            case 3:
                // Flick, catch the momentum mid-flight, go the other way.
                for _ in 0..<Int(rng.range(2, 5)) {
                    let v = stroke(distance: direction * rng.range(150, 500), duration: rng.range(0.09, 0.2), liftSpeed: 0.9)
                    coast(from: v, cut: rng.range(0.1, 0.5))
                    direction = -direction
                }
            default:
                // A run of flicks the same way, each landing on the last one's momentum.
                for _ in 0..<Int(rng.range(1, 5)) {
                    let v = stroke(distance: direction * rng.range(120, 700), duration: rng.range(0.08, 0.22), liftSpeed: rng.range(0.6, 1.0))
                    coast(from: v, cut: rng.range(0.25, 2.5))
                }
                rest(rng.range(0.0, 0.4))
            }
        }
        post(0, phase: 0, momentum: 3)
        FileHandle.standardError.write("[hand] \(posted) events in \(String(format: "%.2f", elapsed()))s travelled=\(Int(travelled))\n".data(using: .utf8)!)
    }
}

/// Scroll the key window's transcript by hand for `seconds`. `profile`: 0 mixed, 1 flicks,
/// 2 reading, 3 reversals. Call on the main thread; returns at once.
@_cdecl("neoism_human_scroll")
public func neoismHumanScroll(_ seconds: Double, _ seed: UInt64, _ profile: Int32) {
    guard let window = NSApp.mainWindow ?? NSApp.keyWindow ?? NSApp.windows.first(where: { $0.isVisible }) else { return }
    let hand = Hand(seconds: seconds, seed: seed, profile: profile, window: window)
    Thread.detachNewThread { hand.run() }
}
