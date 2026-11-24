//! Finder's Open With is delivered to the AppKit delegate, not in the argument list.

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_void};
use std::path::PathBuf;

use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSApplication, NSApplicationDelegateReply};
use objc2_foundation::{NSArray, MainThreadMarker, NSString};

type Handler = fn(Vec<PathBuf>) -> bool;

thread_local! {
    static HANDLER: Cell<Option<Handler>> = const { Cell::new(None) };
    static WAKE: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Handle Finder document opens on the main thread. Set before running the app so the handler
/// also receives the request that launched it. Return whether the request was accepted.
pub fn set_open_documents_handler(handler: Handler) {
    HANDLER.set(Some(handler));
}

unsafe extern "C" {
    fn class_addMethod(class: *const AnyClass, selector: Sel, imp: *const c_void, types: *const c_char) -> Bool;
}

extern "C" fn open_files(_: &AnyObject, _: Sel, app: &NSApplication, paths: &NSArray<NSString>) {
    let accepted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let accepted = HANDLER.get().is_some_and(|handler| handler(paths.iter().map(|path| PathBuf::from(path.to_string())).collect()));
        if accepted { WAKE.with_borrow(|wake| { if let Some(wake) = wake { wake(); } }); }
        accepted
    })).unwrap_or(false);
    // SAFETY: this is the app's reply to the outstanding application:openFiles: request.
    unsafe { app.replyToOpenOrPrint(if accepted { NSApplicationDelegateReply::Success } else { NSApplicationDelegateReply::Failure }); }
}

pub(crate) fn install(wake: impl Fn() + 'static) {
    if HANDLER.get().is_none() { return; }
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let selector = sel!(application:openFiles:);
    // SAFETY: the event loop just installed its delegate, but AppKit has not started delivering
    // launch events yet. Add only a missing selector and preserve the delegate's other methods.
    unsafe {
        let delegate: *mut AnyObject = msg_send![&app, delegate];
        let Some(delegate) = delegate.as_ref() else { return };
        let answers: bool = msg_send![delegate, respondsToSelector: selector];
        if answers { return; }
        let imp = open_files as extern "C" fn(_, _, _, _);
        if class_addMethod(delegate.class(), selector, imp as *const c_void, c"v@:@@".as_ptr()).as_bool() {
            WAKE.with_borrow_mut(|slot| *slot = Some(Box::new(wake)));
            // Refresh AppKit's cache of optional delegate methods.
            let _: () = msg_send![&app, setDelegate: delegate];
        }
    }
}