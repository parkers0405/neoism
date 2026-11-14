//! When each window's next frame is asked for, and painted. A frame a window asks for while it
//! paints is asked for at the end of the loop's turn (`Windows::about_to_wait`); on Windows a
//! window whose asked-for paint the system keeps putting off is painted there.
//!
//! Windows makes up a `WM_PAINT` for one window at a time, the top one first, and winit ends its
//! message pass after each paint. A window in front that asks again inside its paint, or from a
//! timer or a wake each turn, is painted every turn, and one behind it, its frame asked for by
//! the same stream event, waited until the front one rested: a second and a half after a short
//! reply, the whole reply while one streams.

use crate::time::Instant;
use std::time::Duration;

/// How long a window's asked-for paint may wait while the loop turns before the loop paints it
/// itself: about a frame.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(super) const STARVE: Duration = Duration::from_millis(16);

/// One window's next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Redraw {
    /// No frame is on its way, and none is wanted.
    Idle,
    /// The system was asked for a paint at `at`, which hasn't come yet.
    Asked { at: Instant },
    /// The window paints, and nothing has asked for another frame since it started.
    Painting,
    /// The window paints, and wants the next frame.
    PaintingAgain,
    /// The window painted and wants the next frame, which the turn's end asks for.
    Wants,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum RedrawEvent {
    /// Something asks for a frame: an input, a wake, a timer, the frame that just drew.
    Ask(Instant),
    /// A paint starts.
    Paint,
    /// The paint ends.
    Painted,
    /// The loop's turn ends. `paints`: the window is on screen, and Windows puts its paint off
    /// behind the others'.
    TurnEnds { now: Instant, paints: bool },
}

/// What the window does for a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RedrawEffect {
    None,
    /// `Window::request_redraw`.
    Request,
    /// Paint now, and drop the system's paint still on its way.
    PaintNow,
}

impl Redraw {
    pub(super) fn step(self, event: RedrawEvent) -> (Redraw, RedrawEffect) {
        use {Redraw::*, RedrawEffect as E, RedrawEvent::*};
        match (self, event) {
            // Asked inside the paint: winit would ask the system again before the paint returns.
            (Painting | PaintingAgain, Ask(_)) => (PaintingAgain, E::None),
            // The first ask's time is how long it has waited.
            (Asked { at }, Ask(_)) => (Asked { at }, E::Request),
            (Idle | Wants, Ask(now)) => (Asked { at: now }, E::Request),
            (_, Paint) => (Painting, E::None),
            (Painting, Painted) => (Idle, E::None),
            (PaintingAgain, Painted) => (Wants, E::None),
            (state, Painted) => (state, E::None),
            (Wants, TurnEnds { now, .. }) => (Asked { at: now }, E::Request),
            (Asked { at }, TurnEnds { now, paints: true }) if now >= at + STARVE => (Painting, E::PaintNow),
            (state, TurnEnds { .. }) => (state, E::None),
        }
    }
}
/// How long a Windows window keeps presenting every refresh after its last change. With G-SYNC
/// or FreeSync in windowed mode the display refreshes when the foreground window presents, so a
/// hover's lone presents swing the refresh rate, and many panels' brightness with it (Hex #689).
/// A steady run at the display's rate, then rest, changes the rate once per interaction. Only the
/// focused window holds it: the display follows the foreground window, and a window behind that
/// presented every refresh would take frames from the others for nothing.
#[cfg(target_os = "windows")]
pub(super) const STEADY: Duration = Duration::from_secs(1);

/// A window's steady run (`STEADY`).
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Steady {
    /// No run: the window rests when its frames stop changing.
    Rest,
    /// Presenting every refresh until `until`.
    Hold { until: Instant },
    /// The run alone asked for the frame now drawing, which changes nothing unless it draws
    /// something new.
    HeldFrame { until: Instant },
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug)]
pub(super) enum SteadyEvent {
    /// A frame drew: `changed` when it showed something new, `focused` when the window has focus.
    Drew { changed: bool, focused: bool, now: Instant },
    /// The window rests after the frame: the run asks for the next one if it lasts.
    Rests { now: Instant },
    /// The window lost focus or hid.
    Ends,
}

#[cfg(target_os = "windows")]
impl Steady {
    /// The next state, and whether the window asks for another frame.
    pub(super) fn step(self, event: SteadyEvent) -> (Steady, bool) {
        use {Steady::*, SteadyEvent::*};
        match (self, event) {
            (_, Ends) | (_, Drew { focused: false, .. }) => (Rest, false),
            (_, Drew { changed: true, now, .. }) => (Hold { until: now + STEADY }, false),
            (HeldFrame { until }, Drew { changed: false, .. }) => (Hold { until }, false),
            (state, Drew { .. }) => (state, false),
            (Hold { until } | HeldFrame { until }, Rests { now }) if now < until => (HeldFrame { until }, true),
            (_, Rests { .. }) => (Rest, false),
        }
    }

    /// The frame now drawing was asked for by the run alone.
    pub(super) fn held_frame(self) -> bool { matches!(self, Steady::HeldFrame { .. }) }
}
