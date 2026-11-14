//! A transparent, borderless window with `MacWindowOptions::corner_radius(16)` rounds its
//! content layer with a continuous curve and keeps its system shadow, and re-samples that
//! shadow once after each show and resize, never on ordinary frames. Reads the layer and
//! counts `-[NSWindow invalidateShadow]` calls without looking at the screen. Opens a native
//! window, so it only runs with RUNE_GUI_TESTS=1 in a logged-in macOS GUI session.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
    use objc2::{msg_send, sel};
    use rune::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    struct State { frame: u32 }

    static INVALIDATIONS: AtomicUsize = AtomicUsize::new(0);
    static ORIGINAL: OnceLock<usize> = OnceLock::new();
    static WINDOW: OnceLock<WindowHandle> = OnceLock::new();

    #[link(name = "QuartzCore", kind = "framework")]
    unsafe extern "C" {
        static kCACornerCurveContinuous: &'static AnyObject;
    }

    unsafe extern "C" fn counted(this: *mut AnyObject, sel: Sel) {
        INVALIDATIONS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the original -[NSWindow invalidateShadow] implementation.
        let original: unsafe extern "C" fn(*mut AnyObject, Sel) = unsafe { std::mem::transmute(*ORIGINAL.get().unwrap()) };
        unsafe { original(this, sel) }
    }

    /// Counts every `invalidateShadow` on any window that doesn't override it.
    fn count_invalidations() {
        let method = AnyClass::get("NSWindow").and_then(|c| c.instance_method(sel!(invalidateShadow))).expect("NSWindow.invalidateShadow");
        // SAFETY: the replacement has the method's signature and calls through to the original.
        let original = unsafe { method.set_implementation(std::mem::transmute::<unsafe extern "C" fn(*mut AnyObject, Sel), Imp>(counted)) };
        let _ = ORIGINAL.set(original as usize);
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().fill(Color { r: 0.2, g: 0.3, b: 0.5, a: 1.0 }).grow(1.0).child(text(format!("frame {}", s.frame)))
    }

    fn wait(what: &str, f: impl Fn() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn count() -> usize { INVALIDATIONS.load(Ordering::SeqCst) }

    /// Waits for a resample beyond `before`, then for things to settle, and returns the count.
    fn resampled(what: &str, before: usize) -> usize {
        wait(what, || count() > before);
        std::thread::sleep(Duration::from_millis(400));
        count()
    }

    #[derive(Debug)]
    struct Readback { radius: f64, continuous: bool, masks: bool, shadow: bool }

    fn read(window: &WindowHandle) -> Readback {
        use winit::raw_window_handle::RawWindowHandle;
        window.with_raw_handle(|raw| {
            let RawWindowHandle::AppKit(handle) = raw.as_raw() else { panic!("not AppKit") };
            let view = handle.ns_view.as_ptr().cast::<AnyObject>();
            // SAFETY: readbacks on the main thread from a live view.
            unsafe {
                let layer: *mut AnyObject = msg_send![view, layer];
                let window: *mut AnyObject = msg_send![view, window];
                let curve: *mut AnyObject = msg_send![layer, cornerCurve];
                let continuous: Bool = msg_send![curve, isEqualToString: kCACornerCurveContinuous];
                let masks: Bool = msg_send![layer, masksToBounds];
                let shadow: Bool = msg_send![window, hasShadow];
                Readback { radius: msg_send![layer, cornerRadius], continuous: continuous.as_bool(), masks: masks.as_bool(), shadow: shadow.as_bool() }
            }
        }).expect("window handle")
    }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("macos_window_corners: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
        count_invalidations();
        let spec = AppSpec::new("corners", (240.0, 160.0), || State { frame: 0 }, view);
        let options = WindowOptions { transparent: true, borderless: true, min_size: Some((0.0, 0.0)), ..Default::default() }
            .macos(MacWindowOptions::default().shadow(true).corner_radius(16.0));
        let window = &WINDOW;
        let (ready_tx, ready_rx) = mpsc::channel();
        let app = App::new(spec).args(Vec::new()).keep_running().configured_options(options)
            .on_ready(move |w| { let _ = window.set(w.clone()); let _ = ready_tx.send(()); });
        let handle = app.handle();
        let result = std::sync::Arc::new(Mutex::new(None));
        let out = result.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                ready_rx.recv_timeout(Duration::from_secs(10)).expect("window ready");
                let (tx, rx) = mpsc::channel();
                handle.update(move |_, _| { let _ = tx.send(read(window.get().unwrap())); });
                let readback = rx.recv_timeout(Duration::from_secs(5)).expect("readback");
                println!("macos_window_corners: layer {readback:?}");
                assert_eq!(readback.radius, 16.0);
                assert!(readback.continuous && readback.masks && readback.shadow, "{readback:?}");

                let shown = resampled("a resample after the first frame", 0);
                println!("macos_window_corners: show -> frame: {shown} invalidateShadow");
                for _ in 0..8 {
                    handle.update(|s, _| s.frame += 1);
                    std::thread::sleep(Duration::from_millis(30));
                }
                std::thread::sleep(Duration::from_millis(300));
                assert_eq!(count(), shown, "ordinary frames must not resample");

                window.get().unwrap().resize_instant((320.0, 200.0));
                let resized = resampled("a resample after the resize", shown);
                println!("macos_window_corners: resize -> frame: {resized} invalidateShadow");
                assert_eq!(resized, shown + 1, "one resample per resize");

                handle.hide();
                wait("hide", || !handle.is_visible());
                std::thread::sleep(Duration::from_millis(200));
                let hidden = count();
                handle.show();
                let reshown = resampled("a resample after showing again", hidden);
                println!("macos_window_corners: hide, show -> frame: {reshown} invalidateShadow");
                assert_eq!(reshown, hidden + 1, "one resample per show");
            });
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        assert_eq!(*result.lock().unwrap(), Some(true), "macos_window_corners failed");
        println!("macos_window_corners: ok");
    }
}
