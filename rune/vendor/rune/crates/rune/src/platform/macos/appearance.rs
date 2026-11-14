//! winit 0.30 (and master) observes `effectiveAppearance` on each window and treats both KVO
//! values as `NSAppearance`. AppKit reports `NSNull` while a window's frame view is rebuilt
//! (ReplayKit, which ScreenCaptureKit starts, calls `setStyleMask:` on every app window), so
//! winit messages `NSNull` inside a callback that cannot unwind and the process aborts.
//! Wrap winit's observer once per process: nil transitions are held back, and the last real
//! appearance stands in for a nil old value, so real changes still reach winit's `ThemeChanged`.

use std::{ffi::c_void, sync::atomic::{AtomicUsize, Ordering}};
use objc2::{msg_send, sel, runtime::{AnyClass, AnyObject, Sel}};
use objc2_foundation::ns_string;

type Observe = unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject, *mut c_void);

unsafe extern "C" {
    fn class_getInstanceMethod(class: *const AnyClass, selector: Sel) -> *const c_void;
    fn method_getImplementation(method: *const c_void) -> *const c_void;
    fn method_setImplementation(method: *const c_void, imp: *const c_void) -> *const c_void;
    fn objc_getAssociatedObject(object: *const AnyObject, key: *const c_void) -> *mut AnyObject;
    fn objc_setAssociatedObject(object: *const AnyObject, key: *const c_void, value: *const AnyObject, policy: usize);
}
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSKeyValueChangeOldKey: *mut AnyObject;
    static NSKeyValueChangeNewKey: *mut AnyObject;
}

const RETAIN_NONATOMIC: usize = 1;
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static LAST_APPEARANCE: u8 = 0;

/// Main thread, after winit has created a window (its delegate class registers lazily).
#[doc(hidden)]
pub fn install() {
    if ORIGINAL.load(Ordering::Relaxed) != 0 { return; }
    let (Some(delegate), Some(object)) = (AnyClass::get("WinitWindowDelegate"), AnyClass::get("NSObject")) else { return };
    let selector = sel!(observeValueForKeyPath:ofObject:change:context:);
    unsafe {
        let method = class_getInstanceMethod(delegate, selector);
        // Only winit's own override; never NSObject's inherited implementation.
        if method.is_null() || method == class_getInstanceMethod(object, selector) { return; }
        ORIGINAL.store(method_getImplementation(method) as usize, Ordering::Relaxed);
        method_setImplementation(method, observe as *const c_void);
    }
}

unsafe fn appearance(value: *mut AnyObject) -> bool {
    let Some(class) = AnyClass::get("NSAppearance") else { return false };
    !value.is_null() && unsafe { msg_send![value, isKindOfClass: class] }
}

unsafe fn remember(window: *mut AnyObject, value: *mut AnyObject) {
    unsafe { objc_setAssociatedObject(window, (&LAST_APPEARANCE as *const u8).cast(), value, RETAIN_NONATOMIC) }
}

unsafe extern "C" fn observe(this: *mut AnyObject, cmd: Sel, key_path: *mut AnyObject, window: *mut AnyObject, change: *mut AnyObject, context: *mut c_void) {
    // SAFETY: set before this implementation was installed.
    let original: Observe = unsafe { std::mem::transmute(ORIGINAL.load(Ordering::Relaxed)) };
    let key: &AnyObject = ns_string!("effectiveAppearance");
    let watched = !key_path.is_null() && !window.is_null() && !change.is_null()
        && unsafe { msg_send![key_path, isEqualToString: key] };
    if !watched { return unsafe { original(this, cmd, key_path, window, change, context) } }
    unsafe {
        let old: *mut AnyObject = msg_send![change, objectForKey: NSKeyValueChangeOldKey];
        let new: *mut AnyObject = msg_send![change, objectForKey: NSKeyValueChangeNewKey];
        match (appearance(old), appearance(new)) {
            (true, true) => { remember(window, new); original(this, cmd, key_path, window, change, context) }
            (old_known, false) => if old_known { remember(window, old) },
            (false, true) => {
                let last = objc_getAssociatedObject(window, (&LAST_APPEARANCE as *const u8).cast());
                let patched: *mut AnyObject = msg_send![change, mutableCopy];
                let _: () = msg_send![patched, setObject: if last.is_null() { new } else { last }, forKey: NSKeyValueChangeOldKey];
                remember(window, new);
                original(this, cmd, key_path, window, patched, context);
                let _: () = msg_send![patched, release];
            }
        }
    }
}
