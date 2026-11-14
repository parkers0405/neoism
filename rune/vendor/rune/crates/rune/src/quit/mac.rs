//! AppKit asks the app delegate `applicationShouldTerminate:` before every quit: ⌘Q, the Dock's
//! Quit, logout, restart and shutdown. winit's delegate doesn't implement it, so Rune adds the
//! method to the delegate's class once (as `platform::reopen` does for reopening). It marks the
//! app quitting and lets the quit go ahead; `applicationWillTerminate:` follows at once, where
//! winit ends the loop and `Windows::exiting` runs every window's `App::on_quit`, then its
//! `on_close`.

use std::cell::Cell;
use std::ffi::{c_char, c_void};

use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;

thread_local! {
    /// The delegate class Rune added the method to.
    static HOOKED: Cell<*const AnyClass> = const { Cell::new(std::ptr::null()) };
}

unsafe extern "C" {
    fn class_addMethod(class: *const AnyClass, selector: Sel, implementation: *const c_void, types: *const c_char) -> Bool;
}

/// `NSTerminateNow`.
const TERMINATE_NOW: usize = 1;

extern "C" fn should_terminate(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    // Windows::exiting carries out the quit's effects.
    let _ = super::send(super::Event::Start { ending: false });
    TERMINATE_NOW
}

pub(super) fn install() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let selector = sel!(applicationShouldTerminate:);
    // SAFETY: AppKit calls and runtime changes on the main thread. The added function has the
    // selector's ABI (`NSApplicationTerminateReply` is an `NSUInteger`, `Q` on 64-bit), and
    // adding a method the class lacks leaves its other methods and instances alone.
    unsafe {
        let delegate: *mut AnyObject = msg_send![&app, delegate];
        let Some(delegate) = delegate.as_ref() else { return };
        let class = delegate.class();
        if HOOKED.with(|hooked| std::ptr::eq(hooked.get(), class)) {
            return;
        }
        // A later winit, or an app's own delegate, decides itself.
        let answers: bool = msg_send![delegate, respondsToSelector: selector];
        if answers {
            return;
        }
        let imp = should_terminate as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize;
        if !class_addMethod(class, selector, imp as *const c_void, c"Q@:@".as_ptr()).as_bool() {
            eprintln!("rune: could not add the terminate method to the app delegate");
            return;
        }
        HOOKED.with(|hooked| hooked.set(class));
        // AppKit caches which optional methods its delegate answers when it is set.
        let _: () = msg_send![&app, setDelegate: delegate];
    }
}
