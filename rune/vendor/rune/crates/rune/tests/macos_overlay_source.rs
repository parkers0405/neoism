//! An overlay or launcher panel keeps winit's view, and its clicks, when AppKit or another
//! framework restyles the hidden winit window behind it. ScreenCaptureKit does exactly that:
//! ReplayKit's `setWindowRotationLocked:` calls `setStyleMask:` on the first window in
//! `NSApp.windows`. Also checks that restyling leaves the launcher panel's frame and level
//! alone, that the source keeps its KVO class, and that a panel whose view was taken anyway
//! gets it back. Opens windows, so it only runs with RUNE_GUI_TESTS=1 in a
//! logged-in GUI session. Clicks are synthesized with `postEvent:`; the real pointer never moves.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::{Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
    use std::time::Duration;
    use objc2::{class, msg_send, sel, declare::ClassBuilder, runtime::{AnyClass, AnyObject, Bool, Sel}};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    #[link(name = "ReplayKit", kind = "framework")]
    unsafe extern "C" {}
    unsafe extern "C" {
        fn object_getClass(object: *const AnyObject) -> *const AnyClass;
        fn class_getSuperclass(class: *const AnyClass) -> *const AnyClass;
        fn class_copyMethodList(class: *const AnyClass, count: *mut u32) -> *mut *const std::ffi::c_void;
        fn method_getName(method: *const std::ffi::c_void) -> objc2::runtime::Sel;
        fn free(pointer: *mut std::ffi::c_void);
    }

    static CLICKS: AtomicUsize = AtomicUsize::new(0);
    static CREATED: AtomicBool = AtomicBool::new(false);
    static TITLE_CHANGES: AtomicUsize = AtomicUsize::new(0);
    static FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    const TITLED: usize = 1 << 0;
    const RESIZABLE: usize = 1 << 3;
    const FULL_SIZE_CONTENT: usize = 1 << 15;

    #[derive(Clone, Copy)]
    struct Pair { source: usize, panel: usize, view: usize, overlay: bool }

    fn fail(message: String) {
        eprintln!("FAIL {message}");
        FAILURES.lock().unwrap().push(message);
    }

    fn class_name(class: *const AnyClass) -> String {
        if class.is_null() { "NULL".into() } else { unsafe { (*class).name().to_string() } }
    }

    fn windows() -> Vec<*mut AnyObject> {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: *mut AnyObject = msg_send![app, windows];
            let count: usize = msg_send![windows, count];
            (0..count).map(|i| msg_send![windows, objectAtIndex: i]).collect()
        }
    }

    /// Hidden winit windows whose view Rune moved into a panel, found through winit's
    /// delegate (which owns the view) rather than any Rune internals.
    fn pairs() -> Vec<Pair> {
        let mut pairs = Vec::new();
        for window in windows() {
            unsafe {
                let name = class_name(object_getClass(window));
                let reported: *const AnyClass = msg_send![window, class];
                let mask: usize = msg_send![window, styleMask];
                let real: *mut AnyObject = msg_send![super(window, class!(NSWindow)), contentView];
                println!("window isa={name} class={} mask={mask:#x} realContentView={real:?}", class_name(reported));
                if reported.is_null() { fail(format!("-[{name} class] returned NULL")); }
                if name != "RuneOverlayPanel" { continue; }
                let view: *mut AnyObject = msg_send![window, contentView];
                let delegate: *mut AnyObject = msg_send![window, delegate];
                let source = windows().into_iter().find(|w| {
                    let d: *mut AnyObject = msg_send![*w, delegate];
                    *w != window && d == delegate
                });
                let Some(source) = source else { fail("panel without a winit source window".into()); continue };
                let level: isize = msg_send![window, level];
                pairs.push(Pair { source: source as usize, panel: window as usize, view: view as usize, overlay: level >= 25 });
            }
        }
        pairs
    }

    fn kvo_report(source: *mut AnyObject) {
        unsafe {
            let mut class = object_getClass(source);
            while !class.is_null() {
                let name = class_name(class);
                if name.starts_with("NSKVONotifying_") {
                    let mut count = 0;
                    let list = class_copyMethodList(class, &mut count);
                    let methods: Vec<String> = (0..count as usize).map(|i| method_getName(*list.add(i)).name().to_string()).collect();
                    free(list.cast());
                    println!("kvo class {name} overrides {methods:?}");
                }
                if name == "NSWindow" { break; }
                class = class_getSuperclass(class);
            }
        }
    }

    fn check(pair: Pair, path: &str) {
        unsafe {
            let (panel, view) = (pair.panel as *mut AnyObject, pair.view as *mut AnyObject);
            let content: *mut AnyObject = msg_send![panel, contentView];
            let window: *mut AnyObject = msg_send![view, window];
            if content != view || window != panel {
                fail(format!("{path}: {} panel lost its view (contentView={content:?} view.window={window:?})", if pair.overlay { "overlay" } else { "launcher" }));
            }
        }
    }

    fn click(panel: *mut AnyObject) {
        unsafe {
            let frame: NSRect = msg_send![panel, frame];
            let at = NSPoint::new(frame.size.width / 2.0, frame.size.height / 2.0);
            let number: isize = msg_send![panel, windowNumber];
            let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            let now: f64 = msg_send![info, systemUptime];
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            for kind in [1usize, 2] {
                let event: *mut AnyObject = msg_send![class!(NSEvent), mouseEventWithType: kind, location: at, modifierFlags: 0usize,
                    timestamp: now, windowNumber: number, context: std::ptr::null_mut::<AnyObject>(), eventNumber: 0isize, clickCount: 1isize, pressure: 1.0f32];
                let _: () = msg_send![app, postEvent: event, atStart: Bool::NO];
            }
        }
    }

    /// AppKit calls that restyle a window or replace its content, made on the hidden source
    /// window the way an unrelated framework would.
    fn paths() -> Vec<(&'static str, Box<dyn Fn(*mut AnyObject)>)> {
        unsafe fn mask(w: *mut AnyObject) -> usize { unsafe { msg_send![w, styleMask] } }
        unsafe fn set(w: *mut AnyObject, m: usize) { unsafe { let _: () = msg_send![w, setStyleMask: m]; } }
        vec![
            ("setStyleMask: (same)", Box::new(|w| unsafe { set(w, mask(w)) })),
            ("setStyleMask: (resizable)", Box::new(|w| unsafe { let m = mask(w); set(w, m ^ RESIZABLE); set(w, m) })),
            ("setStyleMask: (titled)", Box::new(|w| unsafe { let m = mask(w); set(w, m ^ TITLED); set(w, m) })),
            ("setStyleMask: (full size content)", Box::new(|w| unsafe { let m = mask(w); set(w, m ^ FULL_SIZE_CONTENT); set(w, m) })),
            ("setTitlebarAppearsTransparent:", Box::new(|w| unsafe {
                let t: Bool = msg_send![w, titlebarAppearsTransparent];
                let _: () = msg_send![w, setTitlebarAppearsTransparent: Bool::new(!t.as_bool())];
                let _: () = msg_send![w, setTitlebarAppearsTransparent: t];
            })),
            ("setTitleVisibility:", Box::new(|w| unsafe {
                let v: isize = msg_send![w, titleVisibility];
                let _: () = msg_send![w, setTitleVisibility: 1 - v];
                let _: () = msg_send![w, setTitleVisibility: v];
            })),
            ("setToolbar:", Box::new(|w| unsafe {
                let toolbar: *mut AnyObject = msg_send![class!(NSToolbar), new];
                let _: () = msg_send![w, setToolbar: toolbar];
                let _: () = msg_send![w, setToolbar: std::ptr::null_mut::<AnyObject>()];
                let _: () = msg_send![toolbar, release];
            })),
            ("setContentSize:", Box::new(|w| unsafe {
                let frame: NSRect = msg_send![w, frame];
                let _: () = msg_send![w, setContentSize: NSSize::new(frame.size.width + 7.0, frame.size.height + 5.0)];
                let _: () = msg_send![w, setContentSize: frame.size];
            })),
            ("setFrame:display:", Box::new(|w| unsafe {
                let frame: NSRect = msg_send![w, frame];
                let _: () = msg_send![w, setFrame: NSRect::new(frame.origin, NSSize::new(frame.size.width + 9.0, frame.size.height + 3.0)), display: Bool::YES];
                let _: () = msg_send![w, setFrame: frame, display: Bool::YES];
            })),
            ("setContentView:", Box::new(|w| unsafe {
                let view: *mut AnyObject = msg_send![class!(NSView), new];
                let _: () = msg_send![w, setContentView: view];
                let _: () = msg_send![w, setContentView: std::ptr::null_mut::<AnyObject>()];
                let _: () = msg_send![view, release];
            })),
            ("setAppearance:", Box::new(|w| unsafe {
                let name = objc2_foundation::NSString::from_str("NSAppearanceNameDarkAqua");
                let dark: *mut AnyObject = msg_send![class!(NSAppearance), appearanceNamed: &*name];
                let _: () = msg_send![w, setAppearance: dark];
                let _: () = msg_send![w, setAppearance: std::ptr::null_mut::<AnyObject>()];
            })),
            ("displayIfNeeded", Box::new(|w| unsafe { let _: () = msg_send![w, displayIfNeeded]; let _: () = msg_send![w, layoutIfNeeded]; })),
        ]
    }

    /// An NSObject that counts KVO notifications.
    fn observer_class() -> &'static AnyClass {
        static CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        extern "C" fn observe(_: *mut AnyObject, _: Sel, _: *mut AnyObject, _: *mut AnyObject, _: *mut AnyObject, _: *mut std::ffi::c_void) {
            TITLE_CHANGES.fetch_add(1, Ordering::SeqCst);
        }
        let class = *CLASS.get_or_init(|| {
            let mut builder = ClassBuilder::new("RuneOverlaySourceTitleObserver", class!(NSObject)).unwrap();
            unsafe {
                builder.add_method(sel!(observeValueForKeyPath:ofObject:change:context:),
                    observe as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject, *mut std::ffi::c_void));
            }
            builder.register() as *const AnyClass as usize
        });
        unsafe { &*(class as *const AnyClass) }
    }

    fn rotation_lock(lock: bool) -> bool {
        unsafe {
            let recorder: *mut AnyObject = msg_send![class!(RPScreenRecorder), sharedRecorder];
            let responds: Bool = msg_send![recorder, respondsToSelector: sel!(setWindowRotationLocked:)];
            if responds.as_bool() { let _: () = msg_send![recorder, setWindowRotationLocked: Bool::new(lock)]; }
            responds.as_bool()
        }
    }

    fn view(_: &(), _: &mut rune::Cx<()>) -> rune::El {
        rune::el().w_full().h_full().hit().on_press(|_: &mut ()| { CLICKS.fetch_add(1, Ordering::SeqCst); })
    }

    fn on_main(handle: &rune::AppHandle<()>, f: impl FnOnce() + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        handle.update(move |_, _| { f(); let _ = tx.send(()); });
        rx.recv_timeout(Duration::from_secs(10)).expect("event thread stalled");
    }

    fn expect_click(handle: &rune::AppHandle<()>, overlay: Pair, path: &str) {
        let before = CLICKS.load(Ordering::SeqCst);
        on_main(handle, move || click(overlay.panel as *mut AnyObject));
        for _ in 0..100 {
            if CLICKS.load(Ordering::SeqCst) > before { return; }
            std::thread::sleep(Duration::from_millis(10));
        }
        fail(format!("{path}: overlay click was dropped"));
    }

    fn drive(handle: rune::AppHandle<()>) {
        for _ in 0..200 {
            if CREATED.load(Ordering::SeqCst) { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(CREATED.load(Ordering::SeqCst), "overlay was not created");
        std::thread::sleep(Duration::from_millis(500));
        let (tx, rx) = std::sync::mpsc::channel();
        handle.update(move |_, _| { let _ = tx.send(pairs()); });
        let pairs = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(pairs.iter().any(|p| p.overlay) && pairs.iter().any(|p| !p.overlay), "expected an overlay and a launcher panel");
        let overlay = *pairs.iter().find(|p| p.overlay).unwrap();
        on_main(&handle, move || kvo_report(overlay.source as *mut AnyObject));
        expect_click(&handle, overlay, "baseline");
        // None of the restyles below may take the view: a panel quietly taking it back would
        // hide that.
        let reattached = rune::macos_panel_bridge::reattach_count();

        for (index, (path, _)) in paths().into_iter().enumerate() {
            let (pairs, again) = (pairs.clone(), pairs.clone());
            on_main(&handle, move || {
                let apply = &paths()[index].1;
                // These two are this binary resizing a launcher, which winit's calls do.
                let resizes = matches!(path, "setContentSize:" | "setFrame:display:");
                for pair in &pairs {
                    let panel = pair.panel as *mut AnyObject;
                    let (frame, level): (NSRect, isize) = unsafe { (msg_send![panel, frame], msg_send![panel, level]) };
                    apply(pair.source as *mut AnyObject);
                    check(*pair, path);
                    let (after, after_level): (NSRect, isize) = unsafe { (msg_send![panel, frame], msg_send![panel, level]) };
                    if !resizes && (after != frame || after_level != level) {
                        fail(format!("{path}: restyling the source moved the panel ({frame:?} level {level} -> {after:?} level {after_level})"));
                    }
                }
            });
            expect_click(&handle, overlay, path);
            on_main(&handle, move || unsafe {
                // Reattach for the next path so each failure is reported on its own.
                for pair in &again {
                    let content: *mut AnyObject = msg_send![pair.panel as *mut AnyObject, contentView];
                    if content != pair.view as *mut AnyObject { let _: () = msg_send![pair.panel as *mut AnyObject, setContentView: pair.view as *mut AnyObject]; }
                }
            });
        }

        let (tx, rx) = std::sync::mpsc::channel();
        handle.update(move |_, _| { let _ = tx.send(rotation_lock(true)); });
        if rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            // ReplayKit restyles on a later main-queue turn.
            std::thread::sleep(Duration::from_millis(300));
            on_main(&handle, move || { rotation_lock(false); });
            std::thread::sleep(Duration::from_millis(300));
            let pairs = pairs.clone();
            on_main(&handle, move || for pair in &pairs { check(*pair, "RPScreenRecorder setWindowRotationLocked:"); });
            expect_click(&handle, overlay, "RPScreenRecorder setWindowRotationLocked:");
        } else {
            println!("RPScreenRecorder has no setWindowRotationLocked: on this macOS");
        }
        let taken = rune::macos_panel_bridge::reattach_count() - reattached;
        if taken != 0 { fail(format!("restyles took the view {taken} times; the panels took it back")); }

        // KVO keeps working on the source: with a title observer attached, `-class` still
        // reports winit's class, and the setter override KVO adds still reaches the panel
        // and keeps the source's own title, which winit's `title()` reads.
        let pairs_kvo = pairs.clone();
        on_main(&handle, move || unsafe {
            let observer: *mut AnyObject = msg_send![observer_class(), new];
            let key = objc2_foundation::NSString::from_str("title");
            for pair in &pairs_kvo {
                let _: () = msg_send![pair.source as *mut AnyObject, addObserver: observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<std::ffi::c_void>()];
            }
            for pair in &pairs_kvo {
                let source = pair.source as *mut AnyObject;
                let reported: *const AnyClass = msg_send![source, class];
                if class_name(reported) != "WinitWindow" { fail(format!("-class with a KVO observer: {}", class_name(reported))); }
                if !pair.overlay {
                    let before = TITLE_CHANGES.load(Ordering::SeqCst);
                    let title = objc2_foundation::NSString::from_str("retitled");
                    let _: () = msg_send![source, setTitle: &*title];
                    for (window, name) in [(pair.panel as *mut AnyObject, "panel"), (source, "source")] {
                        let shown: *mut AnyObject = msg_send![window, title];
                        let equal: Bool = msg_send![shown, isEqualToString: &*title];
                        if !equal.as_bool() { fail(format!("launcher setTitle: under KVO didn't reach the {name}")); }
                    }
                    if TITLE_CHANGES.load(Ordering::SeqCst) == before { fail("the title observer was never notified".into()); }
                }
            }
            for pair in &pairs_kvo {
                let _: () = msg_send![pair.source as *mut AnyObject, removeObserver: observer, forKeyPath: &*key];
            }
            let _: () = msg_send![observer, release];
        });

        // Anything else that takes the view: the panel takes it back on the next event-loop
        // turn or input, and the click still lands.
        let before = CLICKS.load(Ordering::SeqCst);
        on_main(&handle, move || unsafe {
            let _: () = msg_send![overlay.view as *mut AnyObject, removeFromSuperview];
            click(overlay.panel as *mut AnyObject);
        });
        for _ in 0..100 {
            if CLICKS.load(Ordering::SeqCst) > before { break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        if CLICKS.load(Ordering::SeqCst) == before { fail("detached view: overlay click was dropped".into()); }
        on_main(&handle, move || check(overlay, "detached view"));
        expect_click(&handle, overlay, "after reattaching");
        let launcher = *pairs.iter().find(|p| !p.overlay).unwrap();
        on_main(&handle, move || unsafe { let _: () = msg_send![launcher.view as *mut AnyObject, removeFromSuperview]; });
        handle.open();
        std::thread::sleep(Duration::from_millis(300));
        on_main(&handle, move || check(launcher, "detached launcher view, then open"));
        // Synthesized clicks don't reach a launcher's view here, so only check that input
        // brings the view back.
        on_main(&handle, move || unsafe {
            let _: () = msg_send![launcher.view as *mut AnyObject, removeFromSuperview];
            click(launcher.panel as *mut AnyObject);
        });
        std::thread::sleep(Duration::from_millis(200));
        on_main(&handle, move || check(launcher, "detached launcher view, then click"));
        handle.hide();

        let failures = FAILURES.lock().unwrap();
        println!("clicks={} failures={}", CLICKS.load(Ordering::SeqCst), failures.len());
        std::process::exit(if failures.is_empty() { 0 } else { 1 });
    }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("macos_overlay_source: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
        let spec = rune::AppSpec::new("overlay source", (240.0, 80.0), || (), view);
        let mut app = rune::App::new(spec).args(Vec::new()).keep_running()
            .configured_options(rune::WindowOptions::launcher_panel());
        let handle = app.handle();
        let options = rune::OverlayOptions { input: rune::OverlayInput::Regions, ..Default::default() };
        let overlay = app.overlay(rune::OverlaySpec::new("overlay source overlay", (200.0, 80.0), view).options(options)
            .on_created(|_| CREATED.store(true, Ordering::SeqCst)), ());
        overlay.show();
        std::thread::spawn(move || drive(handle));
        app.run();
    }
}
