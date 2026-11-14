//! Rounded corners and shadow re-sampling for transparent macOS windows.
//!
//! A transparent, borderless window has no frame to round it, so
//! `MacWindowOptions::corner_radius` rounds the content view's layer. wgpu's CAMetalLayer and
//! the native material views are its sublayers, so `masksToBounds` clips them all.
//!
//! The window server draws a transparent window's shadow from the window's pixels. It samples
//! them when the window orders in or changes size, usually before Rune's frame for that state
//! has landed, and keeps the outline of an empty or older frame. `-[NSWindow invalidateShadow]`
//! asks it to sample again. [`Resample`] decides when: once, after the first frame presented
//! since a show, resize, scale change or launcher prewarm has reached the screen, as Metal's
//! presented handler reports it (`present_time::metal`). Calling earlier would sample the old
//! pixels again; ordinary frames never call it.

use crate::frame::present_time::metal::{wake_main, Log};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSWindow;
use std::cell::Cell;
use std::sync::Arc;

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    static kCACornerCurveContinuous: &'static AnyObject;
    static kCACornerCurveCircular: &'static AnyObject;
}

/// Rounds the window's content layer when the options ask for it and the window is
/// transparent and borderless. Runs on each native creation, after wgpu adds its layer.
pub(crate) fn apply_corners(window: &winit::window::Window, options: &crate::MacWindowOptions, transparent: bool, borderless: bool) {
    let Some(radius) = options.corner_radius.filter(|_| transparent && borderless) else { return };
    let Some(view) = crate::platform::macos::content_view(window) else { return };
    // SAFETY: AppKit and Core Animation calls on the main thread; the layer is the view's own.
    unsafe {
        let layer: *mut AnyObject = msg_send![&*view, layer];
        let Some(layer) = layer.as_ref() else { return };
        let curve = if options.corner_curve_circular { kCACornerCurveCircular } else { kCACornerCurveContinuous };
        let _: () = msg_send![layer, setCornerRadius: f64::from(radius)];
        let _: () = msg_send![layer, setCornerCurve: curve];
        let _: () = msg_send![layer, setMasksToBounds: true];
    }
}

/// When to re-sample a transparent window's shadow. Drawables are numbered by [`Log`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Resample {
    #[default]
    Idle,
    /// Once the next frame presents.
    NextFrame,
    /// Once drawable `n` is on screen.
    OnScreen(u64),
}

impl Resample {
    /// Ordered in or revealed. The last presented drawable (0 for none) will do, since a
    /// reveal of cached pixels draws nothing new. A frame already awaited stays awaited.
    pub(crate) fn shown(&mut self, last: u64) {
        if *self == Resample::Idle { *self = if last > 0 { Resample::OnScreen(last) } else { Resample::NextFrame }; }
    }

    /// Resized, rescaled or prewarmed: only a frame drawn for the new state will do.
    pub(crate) fn redraw(&mut self) { *self = Resample::NextFrame; }

    /// Drawable `n` was presented.
    pub(crate) fn presented(&mut self, n: u64) {
        if *self == Resample::NextFrame { *self = Resample::OnScreen(n); }
    }

    pub(crate) fn hidden(&mut self) { *self = Resample::Idle; }

    /// A run loop turn, with the newest drawable on screen. True once: re-sample now.
    pub(crate) fn turn(&mut self, on_screen: u64) -> bool {
        let due = self.due(on_screen);
        if due { *self = Resample::Idle; }
        due
    }

    /// Whether the drawable it awaits is already on screen.
    fn due(self, on_screen: u64) -> bool { matches!(self, Resample::OnScreen(n) if on_screen >= n) }
}

/// Whether a window re-samples its shadow: transparent, with the system shadow on.
pub(crate) fn resamples(window: &winit::window::Window, transparent: bool) -> bool {
    transparent && crate::platform::macos::ns_window(window).is_some_and(|w| w.hasShadow())
}

/// One window's shadow re-sampling, fed by its hooked layer's [`Log`].
pub(crate) struct Shadow {
    window: Retained<NSWindow>,
    log: Arc<Log>,
    state: Cell<Resample>,
    /// The drawable the last frame presented.
    last: Cell<u64>,
}

impl Shadow {
    /// `first`: the window is on screen (or prewarming) and its first frame is coming.
    pub(crate) fn new(window: &winit::window::Window, log: Arc<Log>, first: bool) -> Option<Self> {
        let shadow = Shadow { window: crate::platform::macos::ns_window(window)?, log, state: Cell::default(), last: Cell::new(0) };
        if first { shadow.update(Resample::redraw); }
        Some(shadow)
    }

    pub(crate) fn shown(&self) { let last = self.last.get(); self.update(|r| r.shown(last)); }
    pub(crate) fn redraw(&self) { self.update(Resample::redraw); }
    pub(crate) fn hidden(&self) { self.update(Resample::hidden); }

    pub(crate) fn presented(&self) {
        let n = self.log.last_acquired();
        self.last.set(n);
        self.update(|r| r.presented(n));
    }

    /// At the start of each run loop turn.
    pub(crate) fn turn(&self) {
        let mut state = self.state.get();
        if state == Resample::Idle { return; }
        if state.turn(self.log.on_screen()) {
            // SAFETY: an AppKit call on the main thread, where run loop turns start.
            unsafe { self.window.invalidateShadow() };
        }
        self.state.set(state);
        self.log.watch(state != Resample::Idle);
    }

    fn update(&self, f: impl FnOnce(&mut Resample)) {
        let mut state = self.state.get();
        f(&mut state);
        self.state.set(state);
        self.log.watch(state != Resample::Idle);
        // Its drawable already landed, so no presented handler will wake the next turn.
        if state.due(self.log.on_screen()) { wake_main(); }
    }
}

#[cfg(test)]
#[path = "window_shape_tests.rs"]
mod tests;
