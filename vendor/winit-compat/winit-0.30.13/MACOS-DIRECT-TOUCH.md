# macOS direct touch bridge

The macOS view installs `WinitDirectTouchRecognizer`, a custom `NSGestureRecognizer` restricted to direct contacts. macOS 27 does not deliver direct display touches through `NSView` responder callbacks or event monitors. A view-only `touchesBeganWithEvent:` bridge therefore never receives them; AppKit instead emulates mouse down/drag/up, which starts text selection. See Apple's [Adopting gesture recognizers in your app](https://developer.apple.com/documentation/appkit/adopting-gesture-recognizers-in-your-app) and [Handling AppKit gestures](https://developer.apple.com/documentation/appkit/handling-appkit-gestures).

The recognizer caches each touch event and phase, updates its state, and forwards contacts as `WindowEvent::Touch` from its target-action callback. It implements all four touch callbacks, terminates when the last contact ends or cancels, supplies `locationInView:`, and clears cached state in `reset`. Rune needs raw touch-down timing for taps, scroll-versus-drag policy, and hold-to-drag, so a standard pan recognizer that begins only after movement is insufficient. Each action queries only the matching phase and view and additionally checks `NSTouch.type == NSTouchTypeDirect`.

Direct positions come from `NSTouch.locationInView:` (macOS 10.12.2+), not trackpad `normalizedPosition`, mouse coordinates, or device-model guesses. The flipped NSView supplies top-left logical coordinates, which are multiplied by the window backing scale just like cursor positions. Runtime selector checks gate APIs unavailable on older macOS releases. The pinned objc2-app-kit 0.2.2 bindings expose these APIs with the `NSTouch` feature; objc2-foundation 0.2.2 needs `NSSet` for the event's touch collection.

Apple documents that `NSTouch` is a snapshot, and that identities may be reused after a contact ends. The bridge retains active identity objects, compares them using `isEqual:` as Apple requires, and allocates per-view monotonically increasing IDs rather than using snapshot pointers or potentially colliding hashes. End and cancellation remove the identity. Orphaned move/end/cancel callbacks are ignored rather than inventing a start. Force is unavailable and is reported as `None`.

The recognizer's delegate rejects pointer-event recognition and accepts only direct touches. Indirect trackpad contacts are never converted to touchscreen input; native mouse selection and trackpad scrolling keep their existing paths. No mouse subtype, HID device name, or coordinate/timestamp heuristic classifies input. A driver that exposes only mouse events remains mouse-only here.

The recognizer delays primary mouse-button responder events; accepting the direct-touch gesture lets AppKit arbitrate instead of delivering a parallel emulated text-selection drag. The delegate declines real pointer streams, so they do not wait for a touch gesture to fail. Rune's host also currently ignores cursor/button events while `Runtime::touching()` is true; that alone cannot fix an input path that never receives the touch start.

## API references

- [NSTouch.location(in:)](https://developer.apple.com/documentation/appkit/nstouch/location(in:)) — view-coordinate location, introduced in macOS 10.12.2.
- [NSTouch.identity](https://developer.apple.com/documentation/appkit/nstouch/identity) — lifetime uniqueness, reuse and `isEqual:` comparison.
- [NSTouch.TouchType.direct](https://developer.apple.com/documentation/appkit/nstouch/touchtype/direct) — direct finger contact on a screen.
- [NSGestureRecognizer.allowedTouchTypes](https://developer.apple.com/documentation/appkit/nsgesturerecognizer/allowedtouchtypes) — restricts recognition to direct contacts.
- [Handling Trackpad Events](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/HandlingTouchEvents/HandlingTouchEvents.html) — trackpads generate touch, gesture and mouse events; indirect normalized touch coordinates are not screen locations.

Hardware verification must confirm stable IDs across movement, distinct simultaneous contacts, physical coordinates at both backing scales, end/cancel delivery, and unchanged trackpad scroll/momentum and real mouse input. Compilation alone cannot verify an unknown driver's delivery or compatibility mouse behavior.

## Native delivery diagnostics

Set `NEOISM_TOUCH_TRACE` to a private output file before launching a new app instance to record up to 1024 native pointer callbacks. The opt-in trace records process ID, event type/subtype, time, mouse coordinates, direct/indirect touch counts for touch events, view association, coordinate-selector availability, and active direct contacts. It never records keyboard events, typed text, or message contents. No touch set is inferred from a mouse event: Apple explicitly does not associate an `NSTouch` collection with those events. With the variable unset, the trace performs no I/O or touch inspection.

A successful simulated `Runtime::touch` test cannot establish correct native device delivery. Verify actual recognizer `touch-started`, `touch-moved`, and terminal callbacks in this trace, observable content movement without selection, and unchanged real mouse/trackpad behavior. The old failure was captured as repeated `mouse-dragged subtype=0 active=0`, with no touch callbacks, despite finger input directly on the display.