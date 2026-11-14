//! In a macOS window, `El::help` is AppKit's own tooltip: the pointer resting on the element
//! shows a system tooltip window, Rune still sees the hover, and a press hides the tooltip.
//! Mouse events are synthetic NSEvents on the app's own queue. Opens a window, so it only runs
//! with RUNE_GUI_TESTS=1 in a logged-in GUI session. With RUNE_HELP_HOLD=<seconds> it keeps the
//! tooltip up that long after printing `TOOLTIP`, for an outside screenshot.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::{NSPoint, NSRect};
    use rune::*;

    static HOVERED: AtomicBool = AtomicBool::new(false);
    static PRESSES: AtomicUsize = AtomicUsize::new(0);

    const ICON: (f64, f64) = (54.0, 54.0);

    #[repr(C)]
    struct CGPoint { x: f64, y: f64 }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
    }

    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().fill(Color::WHITE)
            .child(el().key("copy").size(28.0, 28.0).pin(Some(40.0), Some(40.0), None, None)
                .fill(oklch(0.45, 0.02, 260.0)).radius(6.0)
                .help("Copy command")
                .on_hover_change(|_: &mut (), on, _| HOVERED.store(on, Ordering::SeqCst))
                .on_press(|_: &mut ()| { PRESSES.fetch_add(1, Ordering::SeqCst); }))
    }

    fn wait(limit: Duration, f: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while !f() {
            if start.elapsed() > limit { return false; }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    fn on_main<T: Send + 'static>(handle: &AppHandle<()>, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        handle.update(move |_, _| { let _ = tx.send(f()); });
        rx.recv_timeout(Duration::from_secs(5)).expect("an update runs on the main thread")
    }

    /// Posts a mouse event of `kind` at view point (x, y) (top-left origin) on the app's queue.
    fn mouse(handle: &AppHandle<()>, kind: usize, (x, y): (f64, f64)) {
        on_main(handle, move || {
            // SAFETY: main thread (an update); the app's only window. postEvent:atStart: retains
            // the autoreleased event.
            unsafe {
                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                let windows: *mut AnyObject = msg_send![app, windows];
                let window: *mut AnyObject = msg_send![windows, firstObject];
                let view: *mut AnyObject = msg_send![window, contentView];
                let at: NSPoint = msg_send![view, convertPoint: NSPoint::new(x, y), toView: std::ptr::null::<AnyObject>()];
                if kind == 5 {
                    // AppKit's tooltip timer follows the real cursor, so move it there too.
                    let s: NSPoint = msg_send![window, convertPointToScreen: at];
                    let screen: *mut AnyObject = msg_send![window, screen];
                    let frame: NSRect = msg_send![screen, frame];
                    CGWarpMouseCursorPosition(CGPoint { x: s.x, y: frame.size.height - s.y });
                }
                let number: isize = msg_send![window, windowNumber];
                let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
                let now: f64 = msg_send![info, systemUptime];
                let event: *mut AnyObject = msg_send![class!(NSEvent), mouseEventWithType: kind, location: at,
                    modifierFlags: 0usize, timestamp: now, windowNumber: number, context: std::ptr::null::<AnyObject>(),
                    eventNumber: 0isize, clickCount: 1isize, pressure: if kind == 1 { 1.0f32 } else { 0.0f32 }];
                assert!(!event.is_null());
                let _: () = msg_send![app, postEvent: event, atStart: false];
            }
        });
    }

    /// The screen frame of a visible AppKit tooltip window, if one shows.
    fn tooltip(handle: &AppHandle<()>) -> Option<[f64; 4]> {
        on_main(handle, || {
            // SAFETY: main thread; read-only queries on the app's windows.
            unsafe {
                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                let windows: *mut AnyObject = msg_send![app, windows];
                let count: usize = msg_send![windows, count];
                for i in 0..count {
                    let w: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                    let name = (*w).class().name().to_string();
                    let visible: bool = msg_send![w, isVisible];
                    if visible && name.contains("ToolTip") {
                        let f: NSRect = msg_send![w, frame];
                        return Some([f.origin.x, f.origin.y, f.size.width, f.size.height]);
                    }
                }
                None
            }
        })
    }

    fn rest_on_icon(handle: &AppHandle<()>) {
        for k in (0..=6).rev() {
            mouse(handle, 5, (ICON.0 - 4.0 * k as f64, ICON.1 + 3.0 * k as f64));
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("help_macos: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(120)); eprintln!("timeout"); std::process::exit(2) });
        let hold = std::env::var("RUNE_HELP_HOLD").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let (ready_tx, ready_rx) = mpsc::channel();
        let app = App::new(AppSpec::new("help", (320.0, 160.0), || (), view)).args(Vec::new()).on_ready(move |_| { let _ = ready_tx.send(()); });
        let handle = app.handle();
        let worker = std::thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                ready_rx.recv_timeout(Duration::from_secs(60)).expect("window ready");
                on_main(&handle, || unsafe {
                    // SAFETY: main thread.
                    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                    let _: () = msg_send![app, activateIgnoringOtherApps: true];
                    // Lower left, clear of anything centred on screen.
                    let windows: *mut AnyObject = msg_send![app, windows];
                    let window: *mut AnyObject = msg_send![windows, firstObject];
                    let _: () = msg_send![window, setFrameTopLeftPoint: NSPoint::new(40.0, 400.0)];
                    let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
                });
                std::thread::sleep(Duration::from_millis(500));
                assert!(tooltip(&handle).is_none(), "no tooltip before the pointer arrives");

                rest_on_icon(&handle);
                assert!(wait(Duration::from_secs(2), || HOVERED.load(Ordering::SeqCst)), "Rune sees the hover under the tooltip rect");
                assert!(wait(Duration::from_secs(5), || tooltip(&handle).is_some()), "AppKit shows the tooltip");
                println!("TOOLTIP {:?}", tooltip(&handle).unwrap());
                if hold > 0.0 { std::thread::sleep(Duration::from_secs_f64(hold)); }

                mouse(&handle, 1, ICON);
                mouse(&handle, 2, ICON);
                assert!(wait(Duration::from_secs(2), || PRESSES.load(Ordering::SeqCst) == 1), "the press reaches the element");
                assert!(wait(Duration::from_secs(2), || tooltip(&handle).is_none()), "a press hides the tooltip");
            });
            handle.quit();
            run.is_ok()
        });
        app.run();
        assert!(worker.join().unwrap(), "help_macos failed");
        println!("help_macos: ok");
    }
}
