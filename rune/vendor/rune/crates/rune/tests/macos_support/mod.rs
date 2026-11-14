#![allow(dead_code)]

use std::process::Command;
use std::time::{Duration, Instant};
use objc2::{class, msg_send, msg_send_id, rc::Retained, runtime::{AnyObject, Bool}};
use objc2_foundation::{NSArray, NSPoint, NSString};
use rune::AppHandle;

pub const CUA_DRIVER: &str = "/Applications/CuaDriver.app/Contents/MacOS/cua-driver";

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWarpMouseCursorPosition(point: NSPoint) -> i32;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> NSPoint;
    fn CGEventCreateMouseEvent(
        source: *const std::ffi::c_void,
        kind: u32,
        at: NSPoint,
        button: u32,
    ) -> *mut std::ffi::c_void;
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
    fn CGPreflightPostEventAccess() -> bool;
    fn CFRelease(object: *mut std::ffi::c_void);
}

pub fn on_main<S: 'static, T: Send + 'static>(handle: &AppHandle<S>, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    handle.update(move |_, _| {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(10)).expect("event thread stalled")
}

pub fn wait(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn warp((x, y): (f64, f64)) {
    unsafe {
        CGWarpMouseCursorPosition(NSPoint::new(x, y));
    }
}

pub fn can_post_events() -> bool {
    unsafe { CGPreflightPostEventAccess() }
}

/// AppKit tracks a running session from the window server, which never saw the posted press; a
/// real left-button release where the pointer is ends it.
pub fn release_real_button() {
    unsafe {
        let here = CGEventCreate(std::ptr::null());
        let at = CGEventGetLocation(here);
        CFRelease(here);
        let up = CGEventCreateMouseEvent(std::ptr::null(), 2, at, 0);
        CGEventPost(0, up);
        CFRelease(up);
    }
}

/// The drag pasteboard's first item: its file URL, and the path list Finder also reads.
pub fn drag_pasteboard() -> (Option<String>, Option<String>) {
    unsafe {
        let name = NSString::from_str("Apple CFPasteboard drag");
        let board: *mut AnyObject = msg_send![class!(NSPasteboard), pasteboardWithName: &*name];
        let url: Option<Retained<NSString>> = msg_send_id![board, stringForType: &*NSString::from_str("public.file-url")];
        let names: Option<Retained<NSArray<NSString>>> =
            msg_send_id![board, propertyListForType: &*NSString::from_str("NSFilenamesPboardType")];
        (url.map(|u| u.to_string()), names.and_then(|n| n.iter().next().map(|s| s.to_string())))
    }
}

pub fn app_active() -> bool {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let active: Bool = msg_send![app, isActive];
        active.as_bool()
    }
}

pub fn frontmost_pid() -> i32 {
    unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return 0;
        }
        msg_send![app, processIdentifier]
    }
}

/// Hands the focus to the helper. macOS only lets the active app pass activation on, so when
/// nobody yields this falls back to CuaDriver's `bring_to_front`.
pub fn activate_helper(handle: &AppHandle<()>, pid: i32) {
    wait("the helper to launch", || {
        on_main(handle, move || unsafe {
            let app: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
            !app.is_null() && {
                let done: Bool = msg_send![app, isFinishedLaunching];
                done.as_bool()
            }
        })
    });
    on_main(handle, move || unsafe {
        let app: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
        // NSApplicationActivateIgnoringOtherApps
        let _: Bool = msg_send![app, activateWithOptions: 1usize << 1];
    });
    let helper_front = || on_main(handle, move || frontmost_pid() == pid);
    let start = Instant::now();
    while !helper_front() && start.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(20));
    }
    if !helper_front() {
        let status = Command::new(CUA_DRIVER)
            .arg("bring_to_front")
            .arg(format!(r#"{{"pid":{pid}}}"#))
            .stdout(std::process::Stdio::null())
            .status();
        assert!(status.is_ok_and(|s| s.success()), "couldn't hand the focus to the helper (needs {CUA_DRIVER})");
    }
    wait("the helper to be frontmost", helper_front);
}
