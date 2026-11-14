//! Reopening a running app: on macOS, a click on its Dock icon, or opening it again from Finder,
//! Spotlight or `open` while it runs. Without this an app whose windows Rune hid (⌘W with
//! `keep_running`, `hide`) stays invisible, because AppKit only brings back minimized windows.
//!
//! AppKit answers the reopen Apple event itself and asks the app delegate
//! `applicationShouldHandleReopen:hasVisibleWindows:`. winit's delegate doesn't implement it,
//! so Rune adds the method to the delegate's class once (`install`). The method only sends an
//! event to the loop; `App::on_reopen` hooks and the default (`target`) run from it, like any
//! other wake, never inside the AppKit callback.

use std::sync::atomic::{AtomicU64, Ordering};

/// One window as reopening sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Candidate {
    /// An app window that still exists and can come back: not a launcher or other
    /// nonactivating panel, an overlay, or a window that closed for good.
    pub reopenable: bool,
    /// On screen and not minimized.
    pub visible: bool,
    pub minimized: bool,
    /// When Rune last hid it (a close with `keep_running`, `hide`), as `stamp` counted.
    pub hidden_at: Option<u64>,
    /// A window of the app itself, not one `App::window` added.
    pub main: bool,
}

/// Whether any window reopening could bring back already shows.
pub(crate) fn has_visible(windows: &[Candidate]) -> bool {
    windows.iter().any(|w| w.reopenable && w.visible)
}

/// The window the default reopen shows, when none shows: a minimized one, as AppKit restores
/// them; else the one Rune hid last; else the first main window, which may never have shown.
pub(crate) fn target(windows: &[Candidate]) -> Option<usize> {
    if has_visible(windows) { return None; }
    let ok = |w: &&Candidate| w.reopenable;
    let indexed = || windows.iter().enumerate().filter(|(_, w)| ok(w));
    indexed().find(|(_, w)| w.minimized).map(|(i, _)| i)
        .or_else(|| indexed().filter_map(|(i, w)| Some((w.hidden_at?, i))).max().map(|(_, i)| i))
        .or_else(|| indexed().find(|(_, w)| w.main).map(|(i, _)| i))
}

static HIDES: AtomicU64 = AtomicU64::new(0);

/// A number larger than every earlier one, for the order windows hid in.
pub(crate) fn stamp() -> u64 { HIDES.fetch_add(1, Ordering::Relaxed) + 1 }

/// What the reopen method answers AppKit: whether AppKit should run its own reopen handling.
/// When AppKit sees a visible window its handling changes nothing, and the activation that
/// came with the reopen already brought the app forward, to the window's Space; so it goes
/// ahead. Otherwise Rune brings the window back itself, unless the event never reached Rune's
/// loop (it has exited), when AppKit still restores what it can.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn appkit_handles(sent: bool, appkit_sees_visible: bool) -> bool { !sent || appkit_sees_visible }

#[cfg(target_os = "macos")]
pub(crate) use mac::{accessory, front, install, unhide};

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::{Cell, RefCell};
    use std::ffi::{c_char, c_void};
    use objc2::{msg_send, sel};
    use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;

    thread_local! {
        /// Sends the reopen to Rune's loop; false once the loop is gone.
        static REOPEN: RefCell<Option<Box<dyn Fn() -> bool>>> = const { RefCell::new(None) };
        /// The delegate class Rune added the method to.
        static HOOKED: Cell<*const AnyClass> = const { Cell::new(std::ptr::null()) };
    }

    unsafe extern "C" {
        fn class_addMethod(class: *const AnyClass, selector: Sel, implementation: *const c_void, types: *const c_char) -> Bool;
    }

    extern "C" fn should_handle_reopen(_: *mut AnyObject, _: Sel, _: *mut AnyObject, visible: Bool) -> Bool {
        let sent = std::panic::catch_unwind(|| {
            REOPEN.with(|reopen| reopen.try_borrow().ok().and_then(|f| f.as_ref().map(|f| f())).unwrap_or(false))
        }).unwrap_or_else(|_| { eprintln!("rune: reopen callback panicked"); false });
        Bool::new(super::appkit_handles(sent, visible.as_bool()))
    }

    /// Calls `reopen` on the main thread each time the app is reopened; it returns whether the
    /// event reached Rune's loop. Adds the reopen method to the app delegate's class unless the
    /// class already answers it (a later winit, or an app's own delegate), in which case
    /// reopening stays with that delegate. Main thread only; a later call replaces `reopen`.
    pub(crate) fn install(reopen: impl Fn() -> bool + 'static) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let selector = sel!(applicationShouldHandleReopen:hasVisibleWindows:);
        // SAFETY: AppKit calls and runtime changes on the main thread. The added function has
        // the selector's ABI (`types`: BOOL is `bool` on Apple silicon, `signed char` on Intel),
        // and adding a method the class lacks leaves its other methods and instances alone.
        unsafe {
            let delegate: *mut AnyObject = msg_send![&app, delegate];
            let Some(delegate) = delegate.as_ref() else { return };
            let class = delegate.class();
            if !HOOKED.with(|hooked| std::ptr::eq(hooked.get(), class)) {
                let answers: bool = msg_send![delegate, respondsToSelector: selector];
                if answers { return; }
                let types = if cfg!(target_arch = "aarch64") { c"B@:@B" } else { c"c@:@c" };
                let imp = should_handle_reopen as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, Bool) -> Bool;
                if !class_addMethod(class, selector, imp as *const c_void, types.as_ptr()).as_bool() {
                    eprintln!("rune: could not add the reopen method to the app delegate");
                    return;
                }
                HOOKED.with(|hooked| hooked.set(class));
                // AppKit caches which optional methods its delegate answers when it is set.
                let _: () = msg_send![&app, setDelegate: delegate];
            }
        }
        REOPEN.with(|r| if let Ok(mut r) = r.try_borrow_mut() { *r = Some(Box::new(reopen)); });
    }

    /// The app has no Dock icon now (`App::accessory`, `AppHandle::set_dock_icon(false)`).
    pub(crate) fn accessory() -> bool {
        let Some(mtm) = MainThreadMarker::new() else { return false };
        // SAFETY: reading the activation policy on the main thread.
        unsafe { NSApplication::sharedApplication(mtm).activationPolicy() != NSApplicationActivationPolicy::Regular }
    }

    /// Unhides the app if it is hidden (⌘H, Hide Others), which also activates it.
    pub(crate) fn unhide() {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: AppKit calls on the main thread.
        unsafe { if app.isHidden() { app.unhide(None); } }
    }

    /// Activates the app with `window` key and in front. Unlike winit's `focus_window`, this
    /// also works while the window is still deminiaturizing.
    pub(crate) fn front(window: &winit::window::Window) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        if let Some(window) = crate::platform::macos::ns_window(window) { window.makeKeyAndOrderFront(None); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(visible: bool, hidden_at: Option<u64>) -> Candidate {
        Candidate { reopenable: true, visible, minimized: false, hidden_at, main: false }
    }

    #[test]
    fn reopening_shows_the_window_hidden_last() {
        let windows = [Candidate { main: true, ..window(false, Some(1)) }, window(false, Some(3)), window(false, Some(2))];
        assert!(!has_visible(&windows));
        assert_eq!(target(&windows), Some(1));
    }

    #[test]
    fn a_visible_window_leaves_reopening_alone() {
        let windows = [window(true, None), window(false, Some(5))];
        assert!(has_visible(&windows));
        assert_eq!(target(&windows), None);
    }

    #[test]
    fn a_minimized_window_comes_back_first() {
        let windows = [window(false, Some(9)), Candidate { minimized: true, ..window(false, None) }];
        assert_eq!(target(&windows), Some(1));
    }

    #[test]
    fn without_a_hidden_window_the_main_window_shows() {
        let windows = [window(false, None), Candidate { main: true, ..window(false, None) }];
        assert_eq!(target(&windows), Some(1));
    }

    #[test]
    fn panels_and_closed_windows_never_come_back_or_count_as_visible() {
        let panel = Candidate { reopenable: false, ..window(true, Some(7)) };
        let closed = Candidate { reopenable: false, main: true, ..window(false, Some(8)) };
        assert!(!has_visible(&[panel, closed]));
        assert_eq!(target(&[panel, closed]), None);
        assert_eq!(target(&[panel, closed, window(false, Some(1))]), Some(2));
    }

    #[test]
    fn appkit_handles_a_reopen_unless_rune_brings_a_window_back() {
        assert!(appkit_handles(true, true), "a window shows: AppKit's handling, which changes nothing");
        assert!(!appkit_handles(true, false), "none shows: Rune's");
        assert!(appkit_handles(false, false), "the loop is gone: AppKit's");
    }

    #[test]
    fn stamps_count_up() {
        let (a, b) = (stamp(), stamp());
        assert!(b > a);
    }
}
