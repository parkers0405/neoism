//! Keeps a window off screen until its first frame lands. Windows shows a window before
//! anything presents into it, and DWM fills that empty client area white for a frame or more
//! (Hex #850). Cloaked, DWM composes the window and it takes paints and presents, but nothing
//! of it reaches the screen.

use std::cell::Cell;
use std::time::Instant;

use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};

/// A cloaked window's give-up time: one whose frame never comes still shows by then.
#[derive(Default)]
pub(crate) struct Cloak {
    until: Cell<Option<Instant>>,
}

impl Cloak {
    /// Cloaks the window until `lift`, or `until` at the latest.
    pub(crate) fn hold(&self, window: &winit::window::Window, until: Instant) {
        if set(window, true) { self.until.set(Some(until)); }
    }

    pub(crate) fn held(&self) -> bool { self.until.get().is_some() }

    /// Moves a held cloak's deadline, as when making the GPU took part of it.
    pub(crate) fn extend(&self, until: Instant) {
        if self.held() { self.until.set(Some(until)); }
    }

    /// When the cloak gives up on a frame.
    pub(crate) fn deadline(&self) -> Option<Instant> { self.until.get() }

    /// Uncloaks: its frame landed, or `now` is past the deadline.
    pub(crate) fn lift(&self, window: &winit::window::Window) {
        if self.until.take().is_some() { set(window, false); }
    }

    pub(crate) fn lift_if_due(&self, window: &winit::window::Window, now: Instant) {
        if self.until.get().is_some_and(|until| until <= now) { self.lift(window); }
    }
}

fn set(window: &winit::window::Window, cloaked: bool) -> bool {
    let Some(hwnd) = super::hwnd(window) else { return false };
    let value = i32::from(cloaked);
    // SAFETY: DWMWA_CLOAK reads one BOOL from `value` for a window this thread owns.
    unsafe { DwmSetWindowAttribute(hwnd, DWMWA_CLOAK as u32, (&raw const value).cast(), 4) >= 0 }
}
