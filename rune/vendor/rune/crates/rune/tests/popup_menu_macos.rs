//! A native pop-up menu opens from a press or an update, Rune keeps drawing while it tracks,
//! and its pick or dismissal reaches the callback exactly once. Clicks and keys are synthetic
//! NSEvents on the app's own queue. Where the menu ignores them, it says so and drives the open
//! menu through AppKit instead. Opens a window, so it only runs with RUNE_GUI_TESTS=1 in a
//! logged-in GUI session.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::{Duration, Instant};

    use objc2::{class, msg_send, msg_send_id, rc::Retained, runtime::AnyObject};
    use objc2_foundation::{NSPoint, NSString};
    use rune::*;

    #[derive(Default)]
    struct State { theme: String }

    /// Frames drawn: `poll` runs once a frame, and the spinner keeps frames coming.
    static FRAMES: AtomicUsize = AtomicUsize::new(0);
    static PRESSES: AtomicUsize = AtomicUsize::new(0);
    static TRACKING: AtomicBool = AtomicBool::new(false);
    static PICKS: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());

    thread_local! {
        static MENU: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
        static OBSERVERS: RefCell<Vec<Retained<AnyObject>>> = const { RefCell::new(Vec::new()) };
    }

    fn menu() -> PopupMenu {
        PopupMenu::picker([("system", "System"), ("light", "Light"), ("dark", "Dark")], "system")
    }

    fn record(s: &mut State, picked: Option<&str>, _: &mut Tx) {
        PICKS.lock().unwrap().push(picked.map(str::to_owned));
        if let Some(id) = picked { s.theme = id.into(); }
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        el()
            .child(el().key("theme").size(120.0, 24.0).pin(Some(40.0), Some(40.0), None, None)
                .fill(oklch(0.45, 0.02, 260.0)).radius(6.0)
                .on_press_tx(|_: &mut State, tx: &mut Tx| {
                    PRESSES.fetch_add(1, Ordering::SeqCst);
                    tx.popup_menu(menu(), "theme", record);
                }))
            .child(el().key("spinner").size(24.0, 24.0).pin(Some(40.0), Some(100.0), None, None)
                .fill(oklch(0.7, 0.15, 50.0)).looping(LoopSpec::rotation(1.0)))
    }

    fn poll(_: &mut State, _: f64, _: &mut Tx) -> bool {
        FRAMES.fetch_add(1, Ordering::SeqCst);
        false
    }

    fn wait(what: &str, limit: Duration, f: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while !f() {
            if start.elapsed() > limit { println!("popup_menu_macos: no {what} within {limit:?}"); return false; }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    /// Runs `f` on the main thread through an update and waits for it.
    fn on_main<T: Send + 'static>(handle: &AppHandle<State>, f: impl FnOnce(&mut State, &mut Tx) -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        handle.update(move |s, t| { let _ = tx.send(f(s, t)); });
        rx.recv_timeout(Duration::from_secs(5)).expect("an update runs on the main thread")
    }

    /// The app's window, its number, and a view point (top-left origin) in window coordinates.
    unsafe fn window_point(x: f64, y: f64) -> (*mut AnyObject, isize, NSPoint) {
        // SAFETY (caller): main thread. Queries on the app's only window.
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: *mut AnyObject = msg_send![app, windows];
            let window: *mut AnyObject = msg_send![windows, firstObject];
            let view: *mut AnyObject = msg_send![window, contentView];
            let at: NSPoint = msg_send![view, convertPoint: NSPoint::new(x, y), toView: std::ptr::null::<AnyObject>()];
            let number: isize = msg_send![window, windowNumber];
            (window, number, at)
        }
    }

    unsafe fn now() -> f64 {
        // SAFETY (caller): a plain query.
        unsafe {
            let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            msg_send![info, systemUptime]
        }
    }

    /// Queues synthetic events on the app's own event queue, as the window server would
    /// deliver them: no Accessibility permission, and the real AppKit and winit dispatch.
    fn post(handle: &AppHandle<State>, events: Vec<Synthetic>) {
        on_main(handle, move |_, _| {
            // SAFETY: main thread (an update). The NSEvent factories return autoreleased events
            // that postEvent:atStart: retains.
            unsafe {
                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                for event in events {
                    let (_, number, at) = window_point(event.at.0, event.at.1);
                    let made: *mut AnyObject = match event.kind {
                        Kind::Mouse(kind) => msg_send![class!(NSEvent), mouseEventWithType: kind, location: at,
                            modifierFlags: 0usize, timestamp: now(), windowNumber: number, context: std::ptr::null::<AnyObject>(),
                            eventNumber: 0isize, clickCount: 1isize, pressure: if kind == 1 { 1.0f32 } else { 0.0f32 }],
                        Kind::Key(kind, chars, code) => {
                            let chars = NSString::from_str(chars);
                            msg_send![class!(NSEvent), keyEventWithType: kind, location: at, modifierFlags: 0usize,
                                timestamp: now(), windowNumber: number, context: std::ptr::null::<AnyObject>(),
                                characters: &*chars, charactersIgnoringModifiers: &*chars, isARepeat: false, keyCode: code]
                        }
                    };
                    assert!(!made.is_null(), "NSEvent made the synthetic event");
                    let _: () = msg_send![app, postEvent: made, atStart: false];
                }
            }
        });
    }

    #[derive(Clone, Copy)]
    enum Kind { Mouse(usize), Key(usize, &'static str, u16) }
    #[derive(Clone, Copy)]
    struct Synthetic { kind: Kind, at: (f64, f64) }

    const BUTTON: (f64, f64) = (100.0, 52.0);

    fn click(handle: &AppHandle<State>) {
        post(handle, vec![Synthetic { kind: Kind::Mouse(1), at: BUTTON }, Synthetic { kind: Kind::Mouse(2), at: BUTTON }]);
    }

    fn key(handle: &AppHandle<State>, chars: &'static str, code: u16) {
        post(handle, vec![Synthetic { kind: Kind::Key(10, chars, code), at: BUTTON }, Synthetic { kind: Kind::Key(11, chars, code), at: BUTTON }]);
    }

    /// Notes when a menu starts and stops tracking, keeping the open one for `drive`.
    fn observe_menus() {
        // SAFETY: main thread; the observer tokens are kept for the process.
        unsafe {
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            for (name, begin) in [("NSMenuDidBeginTrackingNotification", true), ("NSMenuDidEndTrackingNotification", false)] {
                let block = block2::RcBlock::new(move |note: *mut AnyObject| {
                    let menu: *mut AnyObject = msg_send![note, object];
                    MENU.with(|slot| *slot.borrow_mut() = if begin { Retained::retain(menu) } else { None });
                    TRACKING.store(begin, Ordering::SeqCst);
                });
                let token: Option<Retained<AnyObject>> = msg_send_id![center, addObserverForName: &*NSString::from_str(name),
                    object: std::ptr::null::<AnyObject>(), queue: std::ptr::null::<AnyObject>(), usingBlock: &*block];
                OBSERVERS.with(|o| o.borrow_mut().extend(token));
            }
        }
    }

    /// Picks item `index` of the tracking menu (or dismisses it) through AppKit, from an update
    /// that runs while the menu tracks.
    fn drive(handle: &AppHandle<State>, index: Option<isize>) {
        on_main(handle, move |_, _| MENU.with(|slot| {
            let Some(menu) = slot.borrow().clone() else { return };
            // SAFETY: the tracking NSMenu, on the main thread.
            unsafe {
                if let Some(index) = index { let _: () = msg_send![&*menu, performActionForItemAtIndex: index]; }
                let _: () = msg_send![&*menu, cancelTracking];
            }
        }));
    }

    fn picks() -> Vec<Option<String>> { PICKS.lock().unwrap().clone() }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("popup_menu_macos: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(150)); eprintln!("timeout"); std::process::exit(2) });
        assert!(popup_menu_supported());
        let spec = AppSpec { poll: Some(poll), ..AppSpec::new("popup menu", (320.0, 160.0), || State { theme: "system".into() }, view) };
        let (ready_tx, ready_rx) = mpsc::channel();
        let app = App::new(spec).args(Vec::new()).on_ready(move |_| { let _ = ready_tx.send(()); });
        let handle = app.handle();
        let worker = std::thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                let started = Instant::now();
                ready_rx.recv_timeout(Duration::from_secs(60)).expect("window ready");
                println!("popup_menu_macos: ready after {:?}", started.elapsed());
                std::thread::sleep(Duration::from_millis(300));
                run(&handle);
            });
            handle.quit();
            run.is_ok()
        });
        app.run();
        // The last step closes the window while its menu tracks: `run` returning means the
        // tracking loop ended with it.
        assert!(!TRACKING.load(Ordering::SeqCst), "the menu still tracked after the app ended");
        assert!(worker.join().unwrap(), "popup_menu_macos failed");
        assert_eq!(picks().len(), 4, "every menu answered once: {:?}", picks());
        println!("popup_menu_macos: ok");
    }

    /// Opens the menu over the button, by a click if clicks reach the window.
    fn open_menu(handle: &AppHandle<State>, clicks: bool) {
        if clicks { click(handle); } else { handle.update(|_, tx| tx.popup_menu(menu(), "theme", record)); }
        assert!(wait("the menu", Duration::from_secs(3), || TRACKING.load(Ordering::SeqCst)), "the menu never opened");
    }

    fn run(handle: &AppHandle<State>) {
        on_main(handle, |_, _| observe_menus());
        let before = FRAMES.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(500));
        println!("popup_menu_macos: {} frames in 500 ms before the menu", FRAMES.load(Ordering::SeqCst) - before);

        // 1. A click on the button opens the menu over it.
        click(handle);
        let clicked = wait("menu from a synthetic click", Duration::from_secs(2), || TRACKING.load(Ordering::SeqCst));
        if !clicked {
            println!("popup_menu_macos: the synthetic click didn't reach the window; opening from an update");
            handle.update(|_, tx| tx.popup_menu(menu(), "theme", record));
            assert!(wait("menu from an update", Duration::from_secs(3), || TRACKING.load(Ordering::SeqCst)), "the menu never opened");
        } else {
            assert_eq!(PRESSES.load(Ordering::SeqCst), 1, "one press, one menu");
        }

        // 2. Rune keeps drawing, and updates still land, while the menu tracks.
        let before = FRAMES.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(500));
        let during = FRAMES.load(Ordering::SeqCst) - before;
        println!("popup_menu_macos: {during} frames in 500 ms while the menu tracked");
        assert!(during >= 10, "frames stalled while the menu tracked ({during} in 500 ms)");
        let theme = on_main(handle, |s, _| s.theme.clone());
        assert_eq!(theme, "system", "an update runs mid-tracking and nothing is picked yet");
        assert!(TRACKING.load(Ordering::SeqCst));

        // 3. Type-select "Dark" and pick it with Return.
        key(handle, "d", 2);
        key(handle, "\r", 36);
        let typed = wait("pick from synthetic keys", Duration::from_secs(2), || !picks().is_empty());
        if !typed {
            println!("popup_menu_macos: synthetic keys didn't reach the menu; picking through AppKit");
            drive(handle, Some(2));
        }
        assert!(wait("the pick", Duration::from_secs(3), || !picks().is_empty()), "the pick never arrived");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(picks(), [Some("dark".to_owned())], "the pick reaches the callback once");
        assert_eq!(on_main(handle, |s, _| s.theme.clone()), "dark");
        assert!(!TRACKING.load(Ordering::SeqCst));
        if clicked { assert_eq!(PRESSES.load(Ordering::SeqCst), 1, "the menu's mouse-up doesn't press the button again"); }

        // 4. The button presses again (no pointer capture left behind), and Escape dismisses.
        if clicked {
            click(handle);
            assert!(wait("a second menu from a click", Duration::from_secs(2), || TRACKING.load(Ordering::SeqCst)), "the button didn't press after the menu");
            assert_eq!(PRESSES.load(Ordering::SeqCst), 2);
        } else {
            handle.update(|_, tx| tx.popup_menu(menu(), "theme", record));
            assert!(wait("a second menu", Duration::from_secs(3), || TRACKING.load(Ordering::SeqCst)), "the second menu never opened");
        }
        key(handle, "\u{1b}", 53);
        if !wait("dismissal from Escape", Duration::from_secs(2), || picks().len() == 2) {
            println!("popup_menu_macos: Escape didn't reach the menu; dismissing through AppKit");
            drive(handle, None);
        }
        assert!(wait("the dismissal", Duration::from_secs(3), || picks().len() == 2), "the dismissal never arrived");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(picks(), [Some("dark".to_owned()), None]);
        let before = FRAMES.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        assert!(FRAMES.load(Ordering::SeqCst) > before, "frames keep running after the menu");
        println!("popup_menu_macos: click {}, keys {}", if clicked { "delivered" } else { "dropped" }, if typed { "delivered" } else { "dropped" });

        // 5. Hiding the window while its menu tracks ends the menu and answers None.
        open_menu(handle, clicked);
        handle.hide();
        assert!(wait("the menu to end with the hide", Duration::from_secs(3), || !TRACKING.load(Ordering::SeqCst)), "the menu outlived its hidden window");
        assert!(wait("the hide's answer", Duration::from_secs(3), || picks().len() == 3));
        assert_eq!(picks()[2], None);
        assert!(!handle.is_visible());
        handle.open();
        assert!(wait("the window back", Duration::from_secs(3), || handle.is_visible()));
        std::thread::sleep(Duration::from_millis(300));

        // 6. Closing it while its menu tracks answers None while the state lives, and the app
        // (which doesn't keep running) ends; `main` checks the rest.
        open_menu(handle, clicked);
        handle.close();
        assert!(wait("the close's answer", Duration::from_secs(3), || picks().len() == 4));
        assert_eq!(picks()[3], None);
    }
}
