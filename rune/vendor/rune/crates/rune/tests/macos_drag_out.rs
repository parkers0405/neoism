//! An `El::drag_out` element starts an AppKit drag session from inside the mouse-dragged
//! event and writes the file to the drag pasteboard the way Finder and System Settings read it.
//! The press and drag are synthesized with `postEvent:`, but AppKit tracks a live session from
//! the window server, so ending it takes a real button release: with Accessibility access the
//! test posts one where the pointer already is, then checks the end callback ran once and the
//! next click is an ordinary press. Without it, it checks the session start and pasteboard only.
//! Opens a window, so it only runs with RUNE_GUI_TESTS=1 in a logged-in GUI session.

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
    use objc2_foundation::{NSArray, NSPoint, NSRect};
    use rune::*;
    use crate::macos_support::{can_post_events, drag_pasteboard, on_main, release_real_button, wait};

    static PRESSES: AtomicUsize = AtomicUsize::new(0);
    static ENDED: Mutex<Vec<DragOutcome>> = Mutex::new(Vec::new());
    static FILE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().w_full().h_full().fill(rune::color::oklch(0.6, 0.18, 255.0))
            .on_press(|_: &mut ()| { PRESSES.fetch_add(1, Ordering::SeqCst); })
            .drag_out(|_: &()| FILE.lock().unwrap().clone().map(|f| DragItem::new().file(f)))
            .on_drag_end(|_: &mut (), outcome, _| ENDED.lock().unwrap().push(outcome))
    }

    /// Posts mouse events to the app's visible window: (NSEventType, x, y) in points from the
    /// window's top-left.
    fn post(events: &[(usize, f64, f64)]) {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: Retained<NSArray<AnyObject>> = msg_send_id![app, windows];
            let window = windows.iter().find(|w| { let v: Bool = msg_send![*w, isVisible]; v.as_bool() }).expect("a visible window");
            let number: isize = msg_send![window, windowNumber];
            let content: NSRect = msg_send![window, contentLayoutRect];
            let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            let now: f64 = msg_send![info, systemUptime];
            for &(kind, x, y) in events {
                let at = NSPoint::new(content.origin.x + x, content.origin.y + content.size.height - y);
                let event: *mut AnyObject = msg_send![class!(NSEvent), mouseEventWithType: kind, location: at, modifierFlags: 0usize,
                    timestamp: now, windowNumber: number, context: std::ptr::null_mut::<AnyObject>(), eventNumber: 0isize, clickCount: 1isize, pressure: 1.0f32];
                let _: () = msg_send![app, postEvent: event, atStart: Bool::NO];
            }
        }
    }

    const DOWN: usize = 1;
    const UP: usize = 2;
    const DRAGGED: usize = 6;

    /// Whether the session ended (it can't without Accessibility access to post a release).
    fn drive(handle: AppHandle<()>, file: std::path::PathBuf) -> bool {
        std::thread::sleep(Duration::from_millis(500));
        on_main(&handle, || post(&[(DOWN, 40.0, 40.0), (DRAGGED, 42.0, 40.0), (DRAGGED, 52.0, 40.0), (DRAGGED, 80.0, 40.0), (UP, 80.0, 40.0)]));
        let name = file.file_name().unwrap().to_str().unwrap().to_owned();
        let mut board = (None, None);
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            board = on_main(&handle, drag_pasteboard);
            if board.0.as_deref().is_some_and(|u| u.ends_with(&name)) { break; }
        }
        let (url, names) = board;
        let url = url.expect("the session started and wrote the drag pasteboard");
        assert!(url.starts_with("file://") && url.ends_with(file.file_name().unwrap().to_str().unwrap()), "file URL {url}");
        assert_eq!(names.as_deref(), Some(&*file.to_string_lossy()), "AppKit derives the path list Finder reads");
        assert_eq!(PRESSES.load(Ordering::SeqCst), 0, "a drag-out isn't a press");
        if ENDED.lock().unwrap().is_empty() {
            if !can_post_events() {
                println!("macos_drag_out: session started, pasteboard {url}; ending it needs a real button release, which this process may not post (no Accessibility access)");
                return false;
            }
            release_real_button();
        }
        wait("the drag to end", || !ENDED.lock().unwrap().is_empty());
        assert_eq!(ENDED.lock().unwrap().len(), 1, "on_drag_end runs once");
        on_main(&handle, || post(&[(DOWN, 40.0, 40.0), (UP, 40.0, 40.0)]));
        wait("a click after the drag to press", || PRESSES.load(Ordering::SeqCst) == 1);
        println!("macos_drag_out: session ended {:?}, pasteboard {url}", ENDED.lock().unwrap()[0]);
        true
    }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("macos_drag_out: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
        let file = std::env::temp_dir().join(format!("rune-drag-out-{}.txt", std::process::id()));
        std::fs::write(&file, "dragged out of a Rune window\n").unwrap();
        *FILE.lock().unwrap() = Some(file.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let app = App::new(AppSpec::new("drag out", (240.0, 120.0), || (), view)).args(Vec::new())
            .on_ready(move |_| { let _ = ready_tx.send(()); });
        let handle = app.handle();
        let result = std::sync::Arc::new(Mutex::new(None));
        let out = result.clone();
        let f = file.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ready_rx.recv_timeout(Duration::from_secs(30)).expect("window ready");
                if !drive(handle.clone(), f.clone()) {
                    // The live session's tracking loop holds the event loop, so quit can't land.
                    let _ = std::fs::remove_file(&f);
                    println!("macos_drag_out: ok (session start and pasteboard only)");
                    std::process::exit(0);
                }
            }));
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        let _ = std::fs::remove_file(&file);
        assert_eq!(*result.lock().unwrap(), Some(true), "macos_drag_out failed");
        println!("macos_drag_out: ok");
    }
}
