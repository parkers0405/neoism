//! The host's system settings: appearance, accent, scroll bars, titlebar inset, keyboard access
//! and reduced motion.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

pub(super) struct ScrollBarHost {
    pub(super) state: crate::platform::scroll_bars::SystemScrollBars,
    pub(super) runtime_id: u64,
}

impl Default for ScrollBarHost {
    fn default() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            state: Default::default(),
            runtime_id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl<S: 'static> Runtime<S> {
    pub fn set_system_dark(&mut self, dark: bool) {
        if dark != self.env.system_dark {
            self.env.system_dark = dark;
            self.dirty = true;
        }
    }

    /// What [`Cx::titlebar_inset`] reports. Native macOS windows set it from AppKit's buttons on
    /// every resize, full screen included; a change re-runs the view.
    pub fn set_titlebar_inset(&mut self, points: f32) {
        if points != self.titlebar_inset {
            self.titlebar_inset = points;
            self.dirty = true;
        }
    }

    /// What [`Cx::accent`] and [`Cx::focus_color`] report, and the colour of default focus
    /// rings. Native windows set it from [`system_accent`](crate::system_accent) and again when
    /// the user picks another accent; a change re-runs the view. Headless runs keep
    /// [`SystemAccent::DEFAULT`](crate::SystemAccent::DEFAULT) unless a test sets one.
    pub(crate) fn system_accent(&self) -> crate::SystemAccent {
        self.system_accent
    }

    pub fn set_system_accent(&mut self, accent: crate::SystemAccent) {
        if accent != self.system_accent {
            self.system_accent = accent;
            self.dirty = true;
        }
    }

    /// Tab visits every control when on, else only text fields and lists, like macOS with
    /// Keyboard navigation (Full Keyboard Access) off. Native windows follow the system setting.
    pub fn set_full_keyboard_access(&mut self, on: bool) {
        self.full_keyboard_access = on;
    }
    /// Tab visits every control whatever [`Runtime::set_full_keyboard_access`] says
    /// (`App::tab_visits_controls`).
    pub fn set_tab_visits_controls(&mut self, on: bool) {
        self.tab_visits_controls = on;
    }
    pub fn set_system_reduced_motion(&mut self, reduced: bool) {
        if self.system_reduced_motion != reduced {
            self.system_reduced_motion = reduced;
            self.dirty = true;
        }
    }
    /// Host accessibility preference; false by default in a headless runtime.
    pub fn system_reduced_motion(&self) -> bool {
        self.system_reduced_motion
    }

    pub(crate) fn system_scroll_bars_state(&self) -> crate::platform::scroll_bars::SystemScrollBars {
        self.scroll_bars.state
    }

    pub(crate) fn set_system_scroll_bars_state(&mut self, next: crate::platform::scroll_bars::SystemScrollBars) {
        if next != self.scroll_bars.state {
            self.scroll_bars.state = next;
            self.dirty = true;
        }
    }

    /// What [`Cx::scroll_bars`] reports. Native macOS windows set it from `AppleShowScrollBars` and
    /// `NSPreferredScrollerStyleDidChangeNotification`; a change re-runs the view and its memos.
    /// Headless runs default to [`ScrollBars::WhenScrolling`](crate::ScrollBars::WhenScrolling).
    pub fn set_system_scroll_bars(&mut self, bars: crate::ScrollBars) {
        self.set_system_scroll_bars_state(crate::platform::scroll_bars::SystemScrollBars {
            bars,
            ..self.scroll_bars.state
        });
    }

    pub fn system_scroll_bars(&self) -> crate::ScrollBars {
        self.scroll_bars.state.bars
    }

    /// Whether the connected pointing device prefers overlay scroll bars under
    /// [`ScrollBars::Automatic`](crate::ScrollBars::Automatic) (`true` for a trackpad or touch
    /// surface, `false` for a mouse). Headless runs default to `true`.
    pub fn set_system_scroller_overlay(&mut self, overlay: bool) {
        self.set_system_scroll_bars_state(crate::platform::scroll_bars::SystemScrollBars {
            scroller_overlay: overlay,
            ..self.scroll_bars.state
        });
    }

    pub fn system_scroller_overlay(&self) -> bool {
        self.scroll_bars.state.scroller_overlay
    }

    /// Whether a click in a scroll bar's track jumps to the spot clicked (`true`) or pages toward
    /// it (`false`), as [`crate::system_scroller_jumps`] reports. Native macOS windows set it from
    /// `AppleScrollerPagingBehavior`; headless runs default to paging. A change re-runs the view.
    pub fn set_system_scroller_jumps(&mut self, jumps: bool) {
        self.set_system_scroll_bars_state(crate::platform::scroll_bars::SystemScrollBars {
            jump_to_click: jumps,
            ..self.scroll_bars.state
        });
    }

    pub fn system_scroller_jumps(&self) -> bool {
        self.scroll_bars.state.jump_to_click
    }

    /// Whether scroll bars currently overlay and fade out at rest (`true` for
    /// [`ScrollBars::WhenScrolling`](crate::ScrollBars::WhenScrolling), `false` for
    /// [`ScrollBars::Always`](crate::ScrollBars::Always), and [`Self::system_scroller_overlay`]
    /// for [`ScrollBars::Automatic`](crate::ScrollBars::Automatic); always `true` on iOS).
    pub fn overlay_scroll_bars(&self) -> bool {
        self.scroll_bars.state.is_overlay()
    }
}
