//! Native material checks against real AppKit (macOS 26+ for glass): a window keeps
//! `presentsWithTransaction` through a resize and drops it once its materials unmount, an
//! inserted material leaves the existing view in place, a prewarmed overlay has its glass
//! before it shows, and a fading-in overlay reads as visible. Prints PASS/FAIL lines and exits
//! nonzero on a failure.
#[cfg(not(target_os = "macos"))]
fn main() { eprintln!("macOS only"); }

#[cfg(target_os = "macos")]
fn main() { mac::run(); }

#[cfg(target_os = "macos")]
mod mac {
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id, rc::Retained};
    use objc2_app_kit::{NSApplication, NSView, NSWindow};
    use objc2_foundation::{MainThreadMarker, NSArray, NSString};
    use rune::{App, AppHandle, AppSpec, Cx, DisplayTarget, El, KeyPolicy, OverlayInput, OverlayOptions, OverlayPlacement,
        OverlaySpec, RunOptions, SystemMaterial, WindowOptions, WindowSpec, el};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const PREWARMED: f64 = 300.0;
    const FADING: f64 = 260.0;
    static FAILED: Mutex<bool> = Mutex::new(false);

    #[derive(Default)]
    struct State { glass: bool, popover: bool, glass_view: usize }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let card = |x: f32, m| el().absolute(x, 20.0).size(120.0, 80.0).radius(20.0).system_material(m);
        let mut root = el().w_full().h_full();
        if s.popover { root = root.child(card(20.0, SystemMaterial::Popover)); }
        if s.glass { root = root.child(card(160.0, SystemMaterial::Glass)); }
        root
    }

    fn overlay_view(_: &(), _: &mut Cx<()>) -> El {
        el().w_full().h_full().child(el().absolute(20.0, 20.0).size(120.0, 80.0).radius(20.0).system_material(SystemMaterial::Glass))
    }

    fn check(name: &str, ok: bool, detail: impl std::fmt::Display) {
        if ok { println!("PASS {name}"); } else { println!("FAIL {name}: {detail}"); *FAILED.lock().unwrap() = true; }
    }

    fn class_name(o: &AnyObject) -> String {
        // SAFETY: -class and -description on a live object.
        unsafe { let c: *const AnyObject = msg_send![o, class]; let n: Retained<NSString> = msg_send_id![c, description]; n.to_string() }
    }

    /// The overlay panel `width` wide, or else the main window.
    fn window(width: Option<f64>) -> Option<Retained<NSWindow>> {
        let windows = NSApplication::sharedApplication(MainThreadMarker::new()?).windows();
        let wide = |w: &NSWindow, v: f64| (w.frame().size.width - v).abs() < 0.5;
        let pick = |w: &NSWindow| match width {
            Some(width) => wide(w, width) && class_name(w) == "RuneOverlayPanel",
            None => !wide(w, PREWARMED) && !wide(w, FADING) && w.isVisible(),
        };
        windows.iter().find(|w| pick(w)).map(objc2::ClassType::retain)
    }

    /// The material views under `window`'s content view: (class, pointer, alpha).
    fn materials(window: &NSWindow) -> Vec<(String, usize, f64)> {
        fn walk(v: &NSView, out: &mut Vec<(String, usize, f64)>) {
            let name = class_name(v);
            if name.contains("GlassEffect") || name.contains("VisualEffect") {
                out.push((name, v as *const NSView as usize, unsafe { v.alphaValue() }));
                return;
            }
            for s in unsafe { v.subviews() }.iter() { walk(&s, out); }
        }
        let mut out = Vec::new();
        if let Some(v) = window.contentView() { walk(&v, &mut out); }
        out
    }

    fn shown(window: &NSWindow, class: &str) -> Vec<usize> {
        materials(window).into_iter().filter(|m| m.0.contains(class) && m.2 > 0.0).map(|m| m.1).collect()
    }

    /// The Metal layer's `presentsWithTransaction`.
    fn transactions(window: &NSWindow) -> Option<bool> {
        fn find(layer: &AnyObject) -> Option<bool> {
            if class_name(layer).contains("MetalLayer") { return Some(unsafe { msg_send![layer, presentsWithTransaction] }); }
            let subs: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send_id![layer, sublayers] };
            subs?.iter().find_map(|l| find(&l))
        }
        let layer: Option<Retained<AnyObject>> = unsafe { msg_send_id![&*window.contentView()?, layer] };
        find(&*layer?)
    }

    fn occluded(window: &NSWindow) -> bool {
        let state: usize = unsafe { msg_send![window, occlusionState] };
        state & 2 == 0
    }

    fn on_main(handle: &AppHandle<State>, f: impl FnOnce(&mut State, &mut rune::Tx) + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        handle.update(move |s, t| { f(s, t); let _ = tx.send(()); });
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }

    pub fn run() {
        rune::run_windows_with(RunOptions { accessory: true }, |_| {
            let init = || State { glass: true, ..Default::default() };
            let mut app = App::new(AppSpec::new("Material check", (400.0, 300.0), init, view)).args(vec![]);
            let options = |x: f32, size: (f64, f64), prewarm| OverlayOptions {
                display: DisplayTarget::Primary, input: OverlayInput::None, keys: KeyPolicy::None, prewarm,
                placement: Some(OverlayPlacement::Rect { x, y: 400.0, width: size.0 as f32, height: size.1 as f32 }), ..Default::default()
            };
            let size = |w: f64| (w, 180.0);
            let prewarmed = app.overlay(OverlaySpec::new("Prewarmed", (PREWARMED as f32, 180.0), overlay_view)
                .options(options(500.0, size(PREWARMED), true)), ());
            let fading = app.overlay(OverlaySpec::new("Fading", (FADING as f32, 180.0), overlay_view)
                .options(options(100.0, size(FADING), false)), ());
            let handle = app.handle();
            std::thread::spawn(move || {
                let sleep = |ms| std::thread::sleep(Duration::from_millis(ms));
                sleep(1500);
                on_main(&handle, |s, _| {
                    let w = window(None).expect("main window");
                    let glass = shown(&w, "Glass");
                    check("window shows its glass", glass.len() == 1, format!("{:?}", materials(&w)));
                    check("materials present with transactions", transactions(&w) == Some(true), format!("{:?}", transactions(&w)));
                    s.glass_view = glass.first().copied().unwrap_or(0);
                    let p = window(Some(PREWARMED)).expect("prewarmed overlay");
                    check("a prewarmed overlay has its glass before it shows", shown(&p, "Glass").len() == 1, format!("{:?}", materials(&p)));
                });
                on_main(&handle, |s, _| s.popover = true);
                sleep(500);
                on_main(&handle, |s, _| {
                    let w = window(None).unwrap();
                    check("an inserted popover keeps the glass view", shown(&w, "Glass") == [s.glass_view], format!("{:?} vs {:x}", materials(&w), s.glass_view));
                    check("the inserted popover shows", shown(&w, "VisualEffect").len() == 1, format!("{:?}", materials(&w)));
                });
                on_main(&handle, |s, _| (s.popover, s.glass) = (false, false));
                sleep(500);
                on_main(&handle, |_, _| {
                    let w = window(None).unwrap();
                    check("unmounting every material turns transactions off", transactions(&w) == Some(false), format!("{:?}", transactions(&w)));
                    check("and hides the views", materials(&w).iter().all(|m| m.2 == 0.0), format!("{:?}", materials(&w)));
                });
                on_main(&handle, |s, _| s.glass = true);
                sleep(500);
                on_main(&handle, |_, _| {
                    let w = window(None).unwrap();
                    check("a remounted material shows its view again", shown(&w, "Glass").len() == 1, format!("{:?}", materials(&w)));
                    check("and turns transactions back on", transactions(&w) == Some(true), format!("{:?}", transactions(&w)));
                });
                on_main(&handle, |_, tx| tx.resize_window((520.0, 360.0)));
                sleep(80);
                on_main(&handle, |_, _| { let t = transactions(&window(None).unwrap()); check("mid-resize", t == Some(true), format!("{t:?}")); });
                sleep(1200);
                on_main(&handle, |_, _| { let t = transactions(&window(None).unwrap()); check("a landed resize leaves transactions on", t == Some(true), format!("{t:?}")); });
                for (name, overlay, width) in [("fading-in overlay", &fading, FADING), ("fading-in prewarmed overlay", &prewarmed, PREWARMED)] {
                    overlay.fade_in(Duration::from_millis(300));
                    let start = Instant::now();
                    let mut seen = Vec::new();
                    while start.elapsed() < Duration::from_millis(120) {
                        let (tx, rx) = std::sync::mpsc::channel();
                        handle.update(move |_, _| { let _ = tx.send(window(Some(width)).map(|w| occluded(&w))); });
                        if let Ok(Some(o)) = rx.recv_timeout(Duration::from_secs(1)) { seen.push((start.elapsed().as_millis(), o)); }
                        sleep(4);
                    }
                    check(&format!("a {name} reads as visible"), !seen.is_empty() && seen.iter().all(|s| !s.1), format!("{seen:?}"));
                }
                sleep(500);
                on_main(&handle, |_, _| std::process::exit(i32::from(*FAILED.lock().unwrap())));
            });
            let at = WindowOptions { borderless: true, transparent: true, position: Some((200, 200)), ..Default::default() };
            vec![WindowSpec::from_app("check", app).options(at)]
        });
    }
}
