//! When the input a handler runs for arrived (`Tx::input_time`). A host dispatches each input
//! event inside a scope holding its arrival time; the runtime stamps the actions that event
//! queues with it, and each handler sees its own action's stamp.

use std::cell::Cell;

use crate::time::Instant;

thread_local! {
    static CURRENT: Cell<Option<Instant>> = const { Cell::new(None) };
    static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// When this thread's windows last took an input event (a key, the pointer, a wheel or a
/// gesture) from the platform: the moment it was dispatched, not its `Tx::input_time`, so
/// input dispatched after a frame is always later than that frame. `None` before any.
pub fn last_input() -> Option<Instant> { LAST.with(Cell::get) }

/// The arrival time of the input being dispatched on this thread, if any.
pub(crate) fn current() -> Option<Instant> { CURRENT.with(Cell::get) }

/// Holds an arrival time until dropped, then restores the one before it.
#[must_use]
pub(crate) struct Scope(Option<Instant>);

/// Dispatch with `at` as the input's arrival time (`None` for work no input caused).
pub(crate) fn scope(at: Option<Instant>) -> Scope { Scope(CURRENT.with(|c| c.replace(at))) }

impl Drop for Scope {
    fn drop(&mut self) { CURRENT.with(|c| c.set(self.0)); }
}

/// The input events whose handlers see an arrival time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Key,
    Mouse,
    Wheel,
    Magnify,
    Rotate,
    SmartMagnify,
}

impl Kind {
    fn of(event: &winit::event::WindowEvent) -> Option<Self> {
        use winit::event::WindowEvent as E;
        Some(match event {
            E::KeyboardInput { .. } | E::ModifiersChanged(_) | E::Ime(_) => Kind::Key,
            E::MouseInput { .. } | E::CursorMoved { .. } | E::CursorLeft { .. } => Kind::Mouse,
            E::MouseWheel { .. } => Kind::Wheel,
            E::PinchGesture { .. } => Kind::Magnify,
            E::RotationGesture { .. } => Kind::Rotate,
            E::DoubleTapGesture { .. } => Kind::SmartMagnify,
            _ => return None,
        })
    }
}

/// A scope for dispatching `event` when it is input, holding when it arrived: on macOS the
/// timestamp of the `NSEvent` AppKit is dispatching, when it is that kind of event; else now.
pub(crate) fn arrived(event: &winit::event::WindowEvent) -> Option<Scope> {
    let kind = Kind::of(event)?;
    LAST.with(|c| c.set(Some(Instant::now())));
    Some(scope(Some(native(kind).unwrap_or_else(Instant::now))))
}

#[cfg(target_os = "macos")]
fn native(kind: Kind) -> Option<Instant> {
    use objc2_app_kit::{NSApplication, NSEventType as T};
    use objc2_foundation::{MainThreadMarker, NSProcessInfo};
    let mtm = MainThreadMarker::new()?;
    let event = NSApplication::sharedApplication(mtm).currentEvent()?;
    // SAFETY: reading the current event's properties on the main thread.
    let (ty, timestamp) = unsafe { (event.r#type(), event.timestamp()) };
    let matches = match kind {
        Kind::Key => matches!(ty, T::KeyDown | T::KeyUp | T::FlagsChanged),
        Kind::Mouse => matches!(ty, T::LeftMouseDown | T::LeftMouseUp | T::RightMouseDown | T::RightMouseUp
            | T::OtherMouseDown | T::OtherMouseUp | T::MouseMoved | T::LeftMouseDragged | T::RightMouseDragged
            | T::OtherMouseDragged | T::MouseExited),
        Kind::Wheel => ty == T::ScrollWheel,
        Kind::Magnify => ty == T::Magnify,
        Kind::Rotate => ty == T::Rotate,
        Kind::SmartMagnify => ty == T::SmartMagnify,
    };
    // SAFETY: as above. The uptime is read after the event, so never before its timestamp.
    matches.then(|| since_boot(Instant::now(), unsafe { NSProcessInfo::processInfo().systemUptime() }, timestamp))
}

#[cfg(not(target_os = "macos"))]
fn native(_: Kind) -> Option<Instant> { None }

/// The `Instant` of an `NSEvent` timestamp, in seconds since boot like `uptime`, the system
/// uptime read at `now`. Both count `mach_absolute_time`, as `Instant` does on macOS. An
/// event stamped after `uptime` (or unreadable) arrived `now`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn since_boot(now: Instant, uptime: f64, timestamp: f64) -> Instant {
    let age = uptime - timestamp;
    if !age.is_finite() || age <= 0.0 { return now; }
    now.checked_sub(std::time::Duration::from_secs_f64(age)).unwrap_or(now)
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
