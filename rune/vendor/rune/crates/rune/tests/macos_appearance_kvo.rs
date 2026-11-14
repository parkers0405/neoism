//! winit's `effectiveAppearance` observer receives `NSNull` when ReplayKit/ScreenCaptureKit
//! calls `setStyleMask:` on app windows. Opens windows, so it only runs with RUNE_GUI_TESTS=1
//! in a logged-in GUI session. Each case runs in a child process because the bug aborts.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use std::{os::unix::process::ExitStatusExt, process::Command, sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}};
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::{NSString, ns_string};
    use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, ControlFlow, EventLoop}, window::{Theme, Window, WindowId}};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const CASE: &str = "RUNE_APPEARANCE_CASE";
    const SIGABRT: i32 = 6;

    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {
        static NSKeyValueChangeOldKey: *mut AnyObject;
        static NSKeyValueChangeNewKey: *mut AnyObject;
    }

    pub fn main() {
        if std::env::var_os(CASE).is_some() {
            std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
        }
        match std::env::var(CASE).as_deref() {
            Ok("unguarded") => winit_case(false),
            Ok("guarded") => winit_case(true),
            Ok("rune") => rune_case(),
            _ => parent(),
        }
    }

    fn parent() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("macos_appearance_kvo: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        let run = |case: &str| {
            let out = Command::new(std::env::current_exe().unwrap()).env(CASE, case).env("RUST_BACKTRACE", "0").output().unwrap();
            let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let lines: Vec<&str> = log.lines().filter(|l| !l.starts_with(' ')).collect();
            println!("--- {case}: {:?}\n{}", out.status, lines[lines.len().saturating_sub(4)..].join("\n"));
            (out.status, log)
        };
        let (status, log) = run("unguarded");
        assert_eq!(status.signal(), Some(SIGABRT), "plain winit must abort on an NSNull appearance");
        assert!(log.contains("NSNull"), "the abort should be winit messaging NSNull");
        let (status, log) = run("guarded");
        assert!(status.success() && log.contains("guarded ok"), "guarded winit window must survive and keep ThemeChanged");
        let (status, log) = run("rune");
        assert!(status.success() && log.contains("rune ok"), "Rune window and overlay panel must survive");
        println!("macos_appearance_kvo: ok");
    }

    fn appearance(name: &str) -> *mut AnyObject {
        let name = NSString::from_str(name);
        unsafe { msg_send![class!(NSAppearance), appearanceNamed: &*name] }
    }

    fn null() -> *mut AnyObject { unsafe { msg_send![class!(NSNull), null] } }

    /// Exactly what AppKit's KVO delivers to winit: `old`/`new` may be `NSNull`.
    fn notify(window: *mut AnyObject, old: *mut AnyObject, new: *mut AnyObject) {
        unsafe {
            let delegate: *mut AnyObject = msg_send![window, delegate];
            let change: *mut AnyObject = msg_send![class!(NSMutableDictionary), dictionary];
            let _: () = msg_send![change, setObject: old, forKey: NSKeyValueChangeOldKey];
            let _: () = msg_send![change, setObject: new, forKey: NSKeyValueChangeNewKey];
            let key: &AnyObject = ns_string!("effectiveAppearance");
            let _: () = msg_send![delegate, observeValueForKeyPath: key, ofObject: window, change: change, context: std::ptr::null_mut::<std::ffi::c_void>()];
        }
    }

    fn toggle_style_mask(window: *mut AnyObject, times: usize) {
        const RESIZABLE: usize = 1 << 3;
        unsafe {
            let mask: usize = msg_send![window, styleMask];
            for i in 0..times {
                let _: () = msg_send![window, setStyleMask: if i % 2 == 0 { mask ^ RESIZABLE } else { mask }];
            }
            let _: () = msg_send![window, setStyleMask: mask];
        }
    }

    fn ns_window(window: &Window) -> *mut AnyObject {
        let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else { unreachable!() };
        unsafe { msg_send![handle.ns_view.as_ptr().cast::<AnyObject>(), window] }
    }

    struct Probe { guarded: bool, window: Option<Window>, themes: Vec<Theme>, step: u32 }

    impl ApplicationHandler for Probe {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() { return; }
            self.window = Some(event_loop.create_window(Window::default_attributes().with_visible(false)).unwrap());
            if self.guarded { rune::macos_appearance::install(); }
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(20)));
        }
        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            if let WindowEvent::ThemeChanged(theme) = event { self.themes.push(theme); }
        }
        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            let Some(window) = &self.window else { return };
            let window = ns_window(window);
            let (aqua, dark) = (appearance("NSAppearanceNameAqua"), appearance("NSAppearanceNameDarkAqua"));
            self.step += 1;
            match self.step {
                1 => {
                    notify(window, null(), null());
                    println!("survived NSNull -> NSNull");
                    // A transient nil between equal appearances is not a theme change.
                    notify(window, aqua, null());
                    notify(window, null(), aqua);
                    toggle_style_mask(window, 100);
                }
                // A real change that straddles a nil transition still reaches winit.
                2 => { notify(window, aqua, null()); notify(window, null(), dark); }
                // Ordinary appearance changes pass through untouched.
                3 => notify(window, dark, aqua),
                5 => {
                    assert_eq!(self.themes, [Theme::Dark, Theme::Light]);
                    println!("guarded ok themes={:?}", self.themes);
                    event_loop.exit();
                    return;
                }
                _ => {}
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(20)));
        }
    }

    fn winit_case(guarded: bool) {
        let event_loop = EventLoop::new().unwrap();
        event_loop.run_app(&mut Probe { guarded, window: None, themes: Vec::new(), step: 0 }).unwrap();
    }

    static OVERLAY_CREATED: AtomicBool = AtomicBool::new(false);

    fn view<T>(_: &T, _: &mut rune::Cx<T>) -> rune::El { rune::el().w_full().h_full() }

    fn stress() {
        let (aqua, dark) = (appearance("NSAppearanceNameAqua"), appearance("NSAppearanceNameDarkAqua"));
        let mut observed = Vec::new();
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: *mut AnyObject = msg_send![app, windows];
            let count: usize = msg_send![windows, count];
            for i in 0..count {
                let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                let delegate: *mut AnyObject = msg_send![window, delegate];
                // The real isa, which KVO subclasses at runtime.
                let class = (*window).class().name().to_string();
                let winit = !delegate.is_null() && (*delegate).class().name() == "WinitWindowDelegate";
                for _ in 0..50 {
                    toggle_style_mask(window, 2);
                    if winit {
                        notify(window, null(), null());
                        notify(window, aqua, null());
                        notify(window, null(), dark);
                        notify(window, dark, aqua);
                    }
                }
                if winit { observed.push(class); }
            }
        }
        observed.sort();
        println!("rune ok winit windows={observed:?}");
        assert!(observed.iter().any(|c| c == "RuneOverlayPanel"), "overlay panel missing");
        // The ordinary window and the overlay's hidden source keep winit's (KVO) class.
        assert_eq!(observed.iter().filter(|c| c.ends_with("WinitWindow")).count(), 2, "ordinary or overlay source window missing");
        std::process::exit(0);
    }

    fn rune_case() {
        let spec = rune::AppSpec::new("appearance-kvo", (320.0, 200.0), || (), view::<()>);
        let mut app = rune::App::new(spec).args(["appearance-kvo".to_string()]);
        let handle = app.handle();
        // Launch can take seconds on a busy machine: stress once the overlay's panel exists.
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(45);
            while !OVERLAY_CREATED.load(Ordering::Acquire) {
                if Instant::now() > deadline { eprintln!("overlay was not created within 45 s"); std::process::exit(3) }
                std::thread::sleep(Duration::from_millis(20));
            }
            handle.update(|_, _| stress());
        });
        let overlay = app.overlay(rune::OverlaySpec::new("appearance-kvo overlay", (200.0, 80.0), view::<()>).options(rune::OverlayOptions::default())
            .on_created(|_| OVERLAY_CREATED.store(true, Ordering::Release)), ());
        overlay.show();
        app.run();
    }
}
