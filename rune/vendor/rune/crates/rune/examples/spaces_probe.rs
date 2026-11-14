//! Opt-in native ordering regression: `cargo run -p rune --example spaces_probe` on a Mac
//! with a desktop session. Reads only this process's WindowServer metadata; no screen capture,
//! microphone or keyboard input. This checks hidden panel ordering, not Mission Control pixels.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
    use objc2::{msg_send, runtime::AnyObject};
    use objc2_foundation::NSString;
    use rune::*;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" { fn CGWindowListCopyWindowInfo(options: u32, relative: u32) -> *mut AnyObject; }
    fn snapshot(label: &str, hidden: bool) -> bool {
        let mut clean = true;
        let mut on_screen = 0;
        // SAFETY: CoreGraphics returns an owned array, released below; each value is read
        // while that array lives, on the AppKit main thread.
        unsafe {
            let all = CGWindowListCopyWindowInfo(0, 0);
            let count: usize = msg_send![all, count];
            let get = |row: *mut AnyObject, key: &str| -> *mut AnyObject { msg_send![row, objectForKey: &*NSString::from_str(key)] };
            for i in 0..count {
                let row: *mut AnyObject = msg_send![all, objectAtIndex: i];
                let pid: u32 = msg_send![get(row, "kCGWindowOwnerPID"), unsignedIntValue];
                if pid != std::process::id() { continue; }
                let level: isize = msg_send![get(row, "kCGWindowLayer"), integerValue];
                if level < 25 { continue; }
                let sharing: usize = msg_send![get(row, "kCGWindowSharingState"), unsignedIntegerValue];
                let alpha: f64 = msg_send![get(row, "kCGWindowAlpha"), doubleValue];
                let visible = get(row, "kCGWindowIsOnscreen");
                let onscreen = !visible.is_null() && msg_send![visible, boolValue];
                println!("{label}: level={level} alpha={alpha} sharing={sharing} onscreen={onscreen}");
                if onscreen { on_screen += 1; }
                if hidden && onscreen { clean = false; }
            }
            let _: () = msg_send![all, release];
        }
        clean && (hidden || on_screen == 1)
    }
    fn view(_: &(), _: &mut Cx<()>) -> El { el() }
    let mut app = App::new(AppSpec::new("Spaces probe", (32.0, 32.0), || (), view))
        .args(Vec::new()).keep_running().windowless();
    let handle = app.handle();
    let (created, ready) = std::sync::mpsc::channel();
    let overlay = app.overlay(OverlaySpec::new("Excluded panel", (32.0, 32.0), view)
        .options(OverlayOptions { prewarm: true, exclude_from_capture: true, input: OverlayInput::None, ..Default::default() })
        .on_created(move |_| { let _ = created.send(()); }), ());
    let passed = Arc::new(AtomicBool::new(true));
    let result = passed.clone();
    std::thread::spawn(move || {
        ready.recv_timeout(Duration::from_secs(30)).unwrap();
        let check = |label: &'static str, hidden| {
            std::thread::sleep(Duration::from_millis(600));
            let (tx, rx) = std::sync::mpsc::channel();
            handle.update(move |_, _| { let _ = tx.send(snapshot(label, hidden)); });
            if !rx.recv_timeout(Duration::from_secs(10)).unwrap() { result.store(false, Ordering::SeqCst); }
        };
        check("prewarm", true);
        overlay.show(); check("shown", false);
        overlay.hide(); check("hidden", true);
        overlay.show(); check("reshown", false);
        overlay.set_exclude_from_capture(false); check("include-shown", false);
        overlay.hide(); check("include-hidden", true);
        overlay.set_exclude_from_capture(true);
        overlay.fade_in(Duration::from_millis(100)); check("fade-in", false);
        overlay.fade_out(Duration::from_millis(100)); check("fade-out", true);
        overlay.show(); overlay.resize((64.0, 64.0)); check("resize-shown", false);
        overlay.hide(); check("resize-hidden", true);
        overlay.fade_in(Duration::from_millis(100));
        overlay.fade_out(Duration::from_millis(100)); overlay.show(); check("reshow-during-fade", false);
        overlay.hide(); check("final-hidden", true);
        handle.quit();
    });
    app.run();
    assert!(passed.load(Ordering::SeqCst), "hidden native panels remain on Spaces");
}
