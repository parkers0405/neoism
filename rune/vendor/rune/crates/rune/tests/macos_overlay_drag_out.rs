//! An `El::drag_out` element in an overlay starts an AppKit drag session: the overlay takes
//! input only on its hit regions (`OverlayInput::Regions`), never activates the app, and sits
//! at window level 3 (`.floating`). A press in the region and a drag past the threshold start
//! the session, write the file to the drag pasteboard, and show the drag image over the element
//! (its window-server window, read with `CGWindowListCopyWindowInfo`, where
//! `El::on_screen_layout` says the element is). With Accessibility access the test then posts a
//! real button release where the pointer is, over the panel: the app's own windows take no
//! drops, so the end arrives once, `Cancelled`, and the next click is an ordinary press.
//! Without it, it checks the session start, pasteboard and image only.
//!
//! The press and drag are `postEvent:` NSEvents. The overlay also polls the real pointer, so the
//! test warps it to where the drag goes; that moves the cursor but posts no events. Opens
//! windows, so it only runs with RUNE_GUI_TESTS=1 in a logged-in GUI session.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod macos_support;

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
    use std::time::Duration;
    use objc2::{class, msg_send, msg_send_id, rc::Retained, runtime::{AnyObject, Bool}};
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSString};
    use rune::*;
    use crate::macos_support::{can_post_events, drag_pasteboard, on_main, release_real_button, wait, warp};

    static PRESSES: AtomicUsize = AtomicUsize::new(0);
    static STARTED: AtomicUsize = AtomicUsize::new(0);
    static ENDED: Mutex<Vec<DragOutcome>> = Mutex::new(Vec::new());
    static FILE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
    /// The card's rect in global top-left points, from `on_screen_layout`.
    static CARD: Mutex<Option<[f32; 4]>> = Mutex::new(None);

    /// Where the overlay goes, in points from the primary display's top-left.
    const PANEL: [f32; 4] = [300.0, 300.0, 240.0, 120.0];
    /// The draggable card inside it.
    const CARD_LOCAL: [f32; 4] = [20.0, 20.0, 120.0, 80.0];
    /// Where the press lands, in the card's points.
    const PRESS: (f64, f64) = (40.0, 40.0);
    /// Past the 4-point default threshold.
    const TRAVEL: f64 = 6.0;

    fn window_view(_: &(), _: &mut Cx<()>) -> El {
        el().w_full().h_full().fill(Color::WHITE)
    }

    fn overlay_view(_: &(), _: &mut Cx<()>) -> El {
        let [x, y, w, h] = CARD_LOCAL;
        el().w_full().h_full().child(
            el().key("card").absolute(x, y).size(w, h).radius(10.0)
                .fill(rune::color::oklch(0.6, 0.18, 255.0))
                .hit()
                .on_press(|_: &mut ()| { PRESSES.fetch_add(1, Ordering::SeqCst); })
                .drag_out(|_: &()| FILE.lock().unwrap().clone().map(|f| DragItem::new().file(f)))
                .on_drag_start(|_: &mut (), _| { STARTED.fetch_add(1, Ordering::SeqCst); })
                .on_drag_end(|_: &mut (), outcome, _| ENDED.lock().unwrap().push(outcome))
                .on_screen_layout(|_: &mut (), r| *CARD.lock().unwrap() = Some(r)),
        )
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *mut std::ffi::c_void;
        fn CGWindowLevelForKey(key: i32) -> i32;
        fn CFRelease(object: *mut std::ffi::c_void);
    }

    const DOWN: usize = 1;
    const UP: usize = 2;
    const DRAGGED: usize = 6;
    const NONACTIVATING_PANEL: usize = 1 << 7;
    const DRAGGING_LEVEL_KEY: i32 = 12;
    const ON_SCREEN_ONLY: u32 = 1;

    /// The panel with CGWindowID `number`: (level, style mask, key, frame in AppKit points).
    fn panel(number: u32) -> Option<(isize, usize, bool, NSRect)> {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: Retained<NSArray<AnyObject>> = msg_send_id![app, windows];
            windows.iter().find(|w| { let n: isize = msg_send![*w, windowNumber]; n == number as isize }).map(|w| {
                let key: Bool = msg_send![w, isKeyWindow];
                (msg_send![w, level], msg_send![w, styleMask], key.as_bool(), msg_send![w, frame])
            })
        }
    }

    fn primary_height() -> f64 {
        unsafe {
            let screens: Retained<NSArray<AnyObject>> = msg_send_id![class!(NSScreen), screens];
            let frame: NSRect = msg_send![&*screens.iter().next().expect("a screen"), frame];
            frame.size.height
        }
    }

    /// Posts mouse events to window `number`: (NSEventType, global top-left point).
    fn post(number: u32, events: &[(usize, (f64, f64))]) {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let (_, _, _, frame) = panel(number).expect("the overlay panel");
            let flipped = primary_height();
            let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            let now: f64 = msg_send![info, systemUptime];
            for &(kind, (x, y)) in events {
                let at = NSPoint::new(x - frame.origin.x, flipped - y - frame.origin.y);
                let event: *mut AnyObject = msg_send![class!(NSEvent), mouseEventWithType: kind, location: at, modifierFlags: 0usize,
                    timestamp: now, windowNumber: number as isize, context: std::ptr::null_mut::<AnyObject>(), eventNumber: 0isize, clickCount: 1isize, pressure: 1.0f32];
                let _: () = msg_send![app, postEvent: event, atStart: Bool::NO];
            }
        }
    }

    /// On-screen windows at the dragging level, `[x, y, w, h]` in global top-left points.
    fn drag_windows() -> Vec<[f64; 4]> {
        unsafe {
            let level = CGWindowLevelForKey(DRAGGING_LEVEL_KEY);
            let list = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY, 0);
            if list.is_null() { return Vec::new(); }
            let array = list.cast::<AnyObject>();
            let key = |s: &str| NSString::from_str(s);
            let count: usize = msg_send![array, count];
            let mut out = Vec::new();
            for i in 0..count {
                let info: *mut AnyObject = msg_send![array, objectAtIndex: i];
                let layer: *mut AnyObject = msg_send![info, objectForKey: &*key("kCGWindowLayer")];
                if layer.is_null() { continue; }
                let layer: i32 = msg_send![layer, intValue];
                if layer != level { continue; }
                let bounds: *mut AnyObject = msg_send![info, objectForKey: &*key("kCGWindowBounds")];
                if bounds.is_null() { continue; }
                let get = |k: &str| -> f64 {
                    let n: *mut AnyObject = msg_send![bounds, objectForKey: &*key(k)];
                    if n.is_null() { f64::NAN } else { msg_send![n, doubleValue] }
                };
                out.push([get("X"), get("Y"), get("Width"), get("Height")]);
            }
            CFRelease(list);
            out
        }
    }

    /// Whether the session ended (it can't without Accessibility access to post a release).
    fn drive(handle: AppHandle<()>, overlay: OverlayHandle<()>, file: std::path::PathBuf) -> bool {
        wait("the overlay's panel", || overlay.window_number().is_some());
        let number = overlay.window_number().unwrap();
        let expected = [PANEL[0] + CARD_LOCAL[0], PANEL[1] + CARD_LOCAL[1], CARD_LOCAL[2], CARD_LOCAL[3]];
        wait("the card's screen rect", || *CARD.lock().unwrap() == Some(expected));
        std::thread::sleep(Duration::from_millis(300));
        let (level, style, key, _) = on_main(&handle, move || panel(number)).expect("the overlay panel is one of the app's windows");
        assert_eq!(level, 3, "the overlay sits at the floating level");
        assert!(style & NONACTIVATING_PANEL != 0, "a non-activating panel");
        assert!(!key, "showing the overlay didn't make it key");

        let [cx, cy, cw, ch] = expected.map(f64::from);
        let press = (cx + PRESS.0, cy + PRESS.1);
        let past = (press.0 + TRAVEL, press.1);
        on_main(&handle, move || {
            // The polled pointer agrees with the posted drag; the session follows it.
            warp(past);
            post(number, &[(DOWN, press), (DRAGGED, (press.0 + TRAVEL / 2.0, press.1)), (DRAGGED, past)]);
        });
        wait("on_drag_start", || STARTED.load(Ordering::SeqCst) == 1);

        let name = file.file_name().unwrap().to_str().unwrap().to_owned();
        let mut url = None;
        for _ in 0..50 {
            url = on_main(&handle, || drag_pasteboard().0);
            if url.as_deref().is_some_and(|u| u.ends_with(&name)) { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        let url = url.expect("the session started and wrote the drag pasteboard");
        assert!(url.starts_with("file://") && url.ends_with(&name), "file URL {url}");

        // The image is the card's snapshot, laid exactly over the card when the session starts;
        // it then follows the pointer from the event that started it, at most `TRAVEL` away.
        let mut image = None;
        for _ in 0..100 {
            let windows = drag_windows();
            image = windows.into_iter().find(|w| (w[2] - cw).abs() <= 2.0 && (w[3] - ch).abs() <= 2.0);
            if image.is_some() { break; }
            std::thread::sleep(Duration::from_millis(20));
        }
        let image = image.unwrap_or_else(|| panic!("no card-sized window at the dragging level: {:?}", drag_windows()));
        assert!((image[0] - cx).abs() <= TRAVEL + 1.0 && (image[1] - cy).abs() <= 1.0,
            "the drag image {image:?} sits over the card {expected:?}");
        assert_eq!(PRESSES.load(Ordering::SeqCst), 0, "a drag-out isn't a press");

        if ENDED.lock().unwrap().is_empty() {
            if !can_post_events() {
                println!("macos_overlay_drag_out: session started, image {image:?}, pasteboard {url}; ending it needs a real button release, which this process may not post (no Accessibility access)");
                return false;
            }
            release_real_button();
        }
        // A drop the app refuses slides the image back before the session ends.
        wait("the drag to end", || !ENDED.lock().unwrap().is_empty());
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(*ENDED.lock().unwrap(), [DragOutcome::Cancelled], "dropped back on the panel: cancelled, reported once");
        on_main(&handle, move || post(number, &[(DOWN, press), (UP, press)]));
        wait("a click after the drag to press", || PRESSES.load(Ordering::SeqCst) == 1);
        println!("macos_overlay_drag_out: session ended {:?}, image {image:?}, pasteboard {url}", ENDED.lock().unwrap()[0]);
        true
    }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("macos_overlay_drag_out: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
        let file = std::env::temp_dir().join(format!("rune-overlay-drag-out-{}.txt", std::process::id()));
        std::fs::write(&file, "dragged out of a Rune overlay\n").unwrap();
        *FILE.lock().unwrap() = Some(file.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let mut app = App::new(AppSpec::new("overlay drag out", (200.0, 100.0), || (), window_view)).args(Vec::new())
            .on_ready(move |_| { let _ = ready_tx.send(()); });
        let [x, y, width, height] = PANEL;
        let options = OverlayOptions {
            placement: Some(OverlayPlacement::Rect { x, y, width, height }),
            display: DisplayTarget::Primary,
            input: OverlayInput::Regions,
            ..Default::default()
        };
        let overlay = app.overlay(OverlaySpec::new("overlay drag out", (width, height), overlay_view).options(options).macos_level(3), ());
        overlay.show();
        let handle = app.handle();
        let result = std::sync::Arc::new(Mutex::new(None));
        let out = result.clone();
        let f = file.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ready_rx.recv_timeout(Duration::from_secs(30)).expect("window ready");
                if !drive(handle.clone(), overlay.clone(), f.clone()) {
                    // The live session's tracking loop holds the event loop, so quit can't land.
                    let _ = std::fs::remove_file(&f);
                    println!("macos_overlay_drag_out: ok (session start, image and pasteboard only)");
                    std::process::exit(0);
                }
            }));
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        let _ = std::fs::remove_file(&file);
        assert_eq!(*result.lock().unwrap(), Some(true), "macos_overlay_drag_out failed");
        println!("macos_overlay_drag_out: ok");
    }
}
