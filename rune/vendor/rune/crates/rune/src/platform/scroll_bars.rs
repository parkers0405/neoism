//! The user's "Show scroll bars" and "Click in the scroll bar to" preferences, and the
//! pointing-device scroller style.
//!
//! macOS reads `AppleShowScrollBars`, `AppleScrollerPagingBehavior` and
//! `NSScroller.preferredScrollerStyle`, and follows
//! `NSPreferredScrollerStyleDidChangeNotification` while windows are open; iOS always uses
//! overlay scroll bars; other platforms and headless runs default to [`ScrollBars::WhenScrolling`].

/// The system's "Show scroll bars" preference (System Settings › Appearance on macOS).
///
/// Read it with [`system_scroll_bars`], or in a view with [`Cx::scroll_bars`](crate::Cx::scroll_bars)
/// and [`Cx::overlay_scroll_bars`](crate::Cx::overlay_scroll_bars), which re-run the view and its
/// memos when the preference or connected pointing device changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollBars {
    /// Follow the connected pointing device (`NSScroller.preferredScrollerStyle` on macOS):
    /// overlay with a trackpad or touch surface, always shown with a mouse.
    Automatic,
    /// Overlay over the content while scrolling and fade out after a short rest.
    #[default]
    WhenScrolling,
    /// Show whenever content overflows.
    Always,
}

impl ScrollBars {
    /// Whether scroll bars overlay and fade out at rest, given whether the pointing device prefers
    /// overlay scrollers (`NSScroller.preferredScrollerStyle == .overlay` on macOS). iOS always
    /// uses overlay scroll bars.
    pub const fn is_overlay(self, scroller_overlay: bool) -> bool {
        #[cfg(target_os = "ios")]
        {
            let _ = scroller_overlay;
            return true;
        }
        #[cfg(not(target_os = "ios"))]
        match self {
            Self::Automatic => scroller_overlay,
            Self::WhenScrolling => true,
            Self::Always => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SystemScrollBars {
    pub bars: ScrollBars,
    pub scroller_overlay: bool,
    /// A click in the track jumps to the spot clicked instead of paging toward it.
    pub jump_to_click: bool,
}

impl SystemScrollBars {
    pub const DEFAULT: Self = Self { bars: ScrollBars::WhenScrolling, scroller_overlay: true, jump_to_click: false };

    pub const fn is_overlay(self) -> bool {
        self.bars.is_overlay(self.scroller_overlay)
    }

    pub const fn with_override(self, bars: Option<ScrollBars>) -> Self {
        match bars {
            Some(bars) => Self { bars, ..self },
            None => self,
        }
    }
}

impl Default for SystemScrollBars {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(any(target_os = "macos", test))]
fn parse_show_scroll_bars(raw: Option<&str>) -> ScrollBars {
    match raw {
        Some("Always") => ScrollBars::Always,
        Some("WhenScrolling") => ScrollBars::WhenScrolling,
        _ => ScrollBars::Automatic,
    }
}

thread_local! {
    static VIEW_SCOPE: std::cell::Cell<Option<(SystemScrollBars, u64)>> = const { std::cell::Cell::new(None) };
}

/// Holds a runtime's scroll-bar preference and id for [`system_scroll_bars`] during a view build.
#[must_use]
pub(crate) struct Scope(Option<(SystemScrollBars, u64)>);

pub(crate) fn scope(bars: SystemScrollBars, runtime_id: u64) -> Scope {
    Scope(VIEW_SCOPE.with(|c| c.replace(Some((bars, runtime_id)))))
}

impl Drop for Scope {
    fn drop(&mut self) {
        VIEW_SCOPE.with(|c| c.set(self.0));
    }
}

pub(crate) fn scoped() -> Option<SystemScrollBars> {
    VIEW_SCOPE.with(std::cell::Cell::get).map(|(bars, _)| bars)
}

#[doc(hidden)]
pub fn current_runtime_id() -> u64 {
    VIEW_SCOPE.with(std::cell::Cell::get).map_or(0, |(_, id)| id)
}

/// The user's "Show scroll bars" preference: the building window's setting inside a view, or the
/// host setting outside one (`AppleShowScrollBars` on macOS, [`ScrollBars::WhenScrolling`]
/// elsewhere and headless).
pub fn system_scroll_bars() -> ScrollBars {
    scoped().unwrap_or_else(current).bars
}

/// Whether the connected pointing device prefers overlay scroll bars: the building window's
/// setting inside a view, or `NSScroller.preferredScrollerStyle == .overlay` on macOS (`true`
/// elsewhere and headless).
pub fn system_scroller_overlay() -> bool {
    scoped().unwrap_or_else(current).scroller_overlay
}

/// Whether a click in a scroll bar's track jumps to the spot clicked rather than paging toward
/// it (System Settings › Appearance › "Click in the scroll bar to"): the building window's setting
/// inside a view, or `AppleScrollerPagingBehavior` on macOS (`false`, paging, elsewhere and
/// headless). Option-click does the other.
pub fn system_scroller_jumps() -> bool {
    scoped().unwrap_or_else(current).jump_to_click
}

pub(crate) fn current() -> SystemScrollBars {
    #[cfg(target_os = "macos")]
    return mac::current();
    #[cfg(not(target_os = "macos"))]
    SystemScrollBars::DEFAULT
}

pub(crate) fn reread() -> SystemScrollBars {
    #[cfg(target_os = "macos")]
    return mac::reread();
    #[cfg(not(target_os = "macos"))]
    SystemScrollBars::DEFAULT
}

/// Starts following scroller-style changes (`NSPreferredScrollerStyleDidChangeNotification`), once
/// per process: each change rereads the preference and asks every window to redraw, which hands it
/// to its runtime. Main thread.
pub(crate) fn watch() {
    #[cfg(target_os = "macos")]
    mac::watch();
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{SystemScrollBars, parse_show_scroll_bars};
    use objc2::{class, msg_send, msg_send_id, rc::Retained, runtime::{AnyObject, Bool}};
    use objc2_foundation::{MainThreadMarker, NSString, NSUserDefaults};
    use std::{cell::Cell, sync::Mutex};

    const OVERLAY_STYLE: isize = 1;

    static CURRENT: Mutex<Option<SystemScrollBars>> = Mutex::new(None);

    pub(super) fn current() -> SystemScrollBars {
        let mut cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        if cur.is_none() && MainThreadMarker::new().is_some() {
            *cur = Some(read());
        }
        cur.unwrap_or_default()
    }

    pub(super) fn reread() -> SystemScrollBars {
        let mut cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        if MainThreadMarker::new().is_some() {
            *cur = Some(read());
        }
        cur.unwrap_or_default()
    }

    fn read() -> SystemScrollBars {
        // SAFETY: main thread (`MainThreadMarker` checked by callers); `standardUserDefaults`
        // returns a valid shared instance and `NSScroller.preferredScrollerStyle` returns NSInteger.
        unsafe {
            let pref = NSUserDefaults::standardUserDefaults()
                .stringForKey(&NSString::from_str("AppleShowScrollBars"))
                .map(|s| s.to_string());
            let style: isize = msg_send![class!(NSScroller), preferredScrollerStyle];
            let jump_to_click = NSUserDefaults::standardUserDefaults()
                .boolForKey(&NSString::from_str("AppleScrollerPagingBehavior"));
            SystemScrollBars {
                bars: parse_show_scroll_bars(pref.as_deref()),
                scroller_overlay: style == OVERLAY_STYLE,
                jump_to_click,
            }
        }
    }

    pub(super) fn watch() {
        thread_local! { static WATCHING: Cell<bool> = const { Cell::new(false) }; }
        if MainThreadMarker::new().is_none() || WATCHING.replace(true) {
            return;
        }
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(read());
        let block = block2::RcBlock::new(|_: *mut AnyObject| {
            *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(read());
            // SAFETY: main thread (the observer has no queue, and AppKit posts this there);
            // `windows` is an NSArray of NSWindow, each with a content view or nil.
            unsafe {
                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                let windows: *mut AnyObject = msg_send![app, windows];
                let count: usize = msg_send![windows, count];
                for i in 0..count {
                    let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                    let view: *mut AnyObject = msg_send![window, contentView];
                    if !view.is_null() {
                        let _: () = msg_send![view, setNeedsDisplay: Bool::YES];
                    }
                }
            }
        });
        // SAFETY: main thread; the observer lives for the process, so its token is leaked.
        unsafe {
            let center: Retained<AnyObject> = msg_send_id![class!(NSNotificationCenter), defaultCenter];
            let name = NSString::from_str("NSPreferredScrollerStyleDidChangeNotification");
            let token: Option<Retained<AnyObject>> = msg_send_id![
                &*center,
                addObserverForName: &*name,
                object: std::ptr::null::<AnyObject>(),
                queue: std::ptr::null::<AnyObject>(),
                usingBlock: &*block
            ];
            std::mem::forget(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_resolution_match_system_modes() {
        assert_eq!(ScrollBars::default(), ScrollBars::WhenScrolling);
        assert!(SystemScrollBars::DEFAULT.is_overlay());
        assert!(ScrollBars::WhenScrolling.is_overlay(false));
        assert!(ScrollBars::WhenScrolling.is_overlay(true));
        #[cfg(not(target_os = "ios"))]
        {
            assert!(!ScrollBars::Always.is_overlay(true));
            assert!(!ScrollBars::Always.is_overlay(false));
            assert!(ScrollBars::Automatic.is_overlay(true));
            assert!(!ScrollBars::Automatic.is_overlay(false));
        }
    }

    #[test]
    fn parses_macos_show_scroll_bars_values() {
        assert_eq!(parse_show_scroll_bars(None), ScrollBars::Automatic);
        assert_eq!(parse_show_scroll_bars(Some("Automatic")), ScrollBars::Automatic);
        assert_eq!(parse_show_scroll_bars(Some("WhenScrolling")), ScrollBars::WhenScrolling);
        assert_eq!(parse_show_scroll_bars(Some("Always")), ScrollBars::Always);
        assert_eq!(parse_show_scroll_bars(Some("Unexpected")), ScrollBars::Automatic);
    }
}
