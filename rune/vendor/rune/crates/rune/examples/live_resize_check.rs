//! A live window resize against real AppKit. Posts the mouse events of a person dragging each
//! of the window's edges out and back, and checks that each size AppKit gives the window has a
//! frame drawn for it before AppKit carries on, so in the same Core Animation transaction,
//! rather than the last frame stretched to it. Then that a frame after the drag presents on its
//! own again, that the layer keeps late frames at the window's top left, that no drawable wait
//! stalls the drag, and that `Tx::resize_window` and full screen still end at the right size.
//!
//! `cargo run --release -p rune --example live_resize_check`
//!
//! Prints a line per size, PASS/FAIL lines, and exits nonzero on a failure. It needs a screen
//! and moves no real pointer. `RUNE_LIVE_RESIZE_SLOW_MS=10` makes each frame cost that much.
#[cfg(not(target_os = "macos"))]
fn main() { eprintln!("macOS only"); }

#[cfg(target_os = "macos")]
fn main() { mac::run(); }

#[cfg(target_os = "macos")]
mod mac {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_video::rc::{Retained, autoreleasepool};
    use objc2_video::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2_video::{msg_send, sel};
    use rune::*;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct State { n: u32 }

    fn bar(label: &str, color: Color) -> El {
        row().w_full().h(44.0).center().fill(color).child(text(label).color(Color::WHITE))
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().h_full().fill(oklch(0.22, 0.02, 260.0)).border(2.0, oklch(0.75, 0.18, 145.0))
            .child(bar("top edge", oklch(0.5, 0.15, 260.0)))
            .child(col().w_full().grow(1.0).center().gap(8.0)
                .child(text("Drag any edge").font(tok::type_::TITLE).color(Color::WHITE))
                .child(text(format!("frame {}", s.n)).color(Color::WHITE)))
            .child(bar("bottom edge", oklch(0.55, 0.2, 25.0)))
    }

    /// Draws Rune's frame after spending `slow` on the CPU, as a heavy app's frame does.
    struct Slow(Duration);
    impl Compositor<State> for Slow {
        fn render(&mut self, f: &mut Frame<'_, State>) {
            let start = Instant::now();
            while start.elapsed() < self.0 { std::hint::spin_loop(); }
            f.draw_ui();
        }
    }

    #[derive(Debug)]
    enum Ev {
        Mark(&'static str),
        /// The content view's frame changed, after winit's own observer of it handled the change:
        /// its size in pixels, and whether the user is resizing.
        Resized { at: Instant, px: (f64, f64), live: bool },
        /// `nextDrawable`: the drawable's size, whether it presents with the transaction, and
        /// how long the call blocked.
        Acquire { at: Instant, px: (f64, f64), sync: bool, wait: Duration },
    }

    static LOG: Mutex<Vec<Ev>> = Mutex::new(Vec::new());
    static LAYER_BASE: OnceLock<&'static AnyClass> = OnceLock::new();
    /// The window's frame in screen points, kept on the main thread for the thread posting events.
    static FRAME: Mutex<[f64; 4]> = Mutex::new([0.0; 4]);
    static FAILED: Mutex<bool> = Mutex::new(false);

    fn log(ev: Ev) { LOG.lock().unwrap().push(ev); }

    unsafe extern "C-unwind" fn next_drawable(this: &AnyObject, _: Sel) -> *mut AnyObject {
        // SAFETY: CAMetalLayer property reads and its own nextDrawable, on the main thread.
        unsafe {
            let size: CGSize = msg_send![this, drawableSize];
            let sync: Bool = msg_send![this, presentsWithTransaction];
            let start = Instant::now();
            let drawable: *mut AnyObject = msg_send![super(this, *LAYER_BASE.get().unwrap()), nextDrawable];
            log(Ev::Acquire { at: Instant::now(), px: (size.width, size.height), sync: sync.as_bool(), wait: start.elapsed() });
            drawable
        }
    }

    /// Makes the layer an instance of a subclass of its class that logs each `nextDrawable`.
    fn hook_layer(layer: &AnyObject) {
        let class = layer.class();
        let name = std::ffi::CString::new(format!("LiveResizeProbe_{}", class.name().to_str().unwrap())).unwrap();
        let mut builder = ClassBuilder::new(&name, class).expect("a fresh probe class");
        // SAFETY: the signature matches -[CAMetalLayer nextDrawable].
        unsafe { builder.add_method(sel!(nextDrawable), next_drawable as unsafe extern "C-unwind" fn(_, _) -> _) };
        let probe = builder.register();
        LAYER_BASE.set(class).unwrap();
        // SAFETY: the subclass adds no ivars, so the instance layout is unchanged.
        unsafe { AnyObject::set_class(layer, probe) };
    }

    fn store_frame(window: &AnyObject) {
        // SAFETY: a property read on the main thread.
        let f: CGRect = unsafe { msg_send![window, frame] };
        *FRAME.lock().unwrap() = [f.origin.x, f.origin.y, f.size.width, f.size.height];
    }

    /// Logs each change of the content view's frame. winit observes the same notification, and
    /// observers run in the order they were added, so Rune has handled the change by then.
    fn watch_resizes(window: &AnyObject, view: &AnyObject) {
        let (w, v) = (window as *const AnyObject as usize, view as *const AnyObject as usize);
        let block = block2_metal::RcBlock::new(move |_: *mut AnyObject| {
            // SAFETY: the window and its view outlive the run; reads on the main thread.
            unsafe {
                let (window, view) = (&*(w as *const AnyObject), &*(v as *const AnyObject));
                let frame: CGRect = msg_send![view, frame];
                let scale: f64 = msg_send![window, backingScaleFactor];
                let live: Bool = msg_send![window, inLiveResize];
                log(Ev::Resized { at: Instant::now(), px: (frame.size.width * scale, frame.size.height * scale), live: live.as_bool() });
                store_frame(window);
            }
        });
        // SAFETY: main thread; the center copies the block and the token lives for the run.
        unsafe {
            let center: Retained<AnyObject> = msg_send![AnyClass::get(c"NSNotificationCenter").unwrap(), defaultCenter];
            let name = objc2_foundation_video::NSString::from_str("NSViewFrameDidChangeNotification");
            let token: Option<Retained<AnyObject>> = msg_send![&*center, addObserverForName: &*name,
                object: view, queue: std::ptr::null::<AnyObject>(), usingBlock: &*block];
            std::mem::forget(token);
        }
    }

    fn check(name: &str, ok: bool, detail: impl std::fmt::Display) {
        if ok { println!("PASS {name}"); } else { println!("FAIL {name}: {detail}"); *FAILED.lock().unwrap() = true; }
    }

    fn class_name(o: &AnyObject) -> String { o.class().name().to_string_lossy().into_owned() }

    fn metal_layer(layer: &AnyObject) -> Option<Retained<AnyObject>> {
        // SAFETY: CALayer reads on the main thread.
        unsafe {
            if class_name(layer).contains("MetalLayer") { return Retained::retain(layer as *const AnyObject as *mut AnyObject); }
            let subs: Option<Retained<AnyObject>> = msg_send![layer, sublayers];
            let subs = subs?;
            let count: usize = msg_send![&*subs, count];
            (0..count).find_map(|i| { let s: Retained<AnyObject> = msg_send![&*subs, objectAtIndex: i]; metal_layer(&s) })
        }
    }

    /// Rune's window, its content view and its Metal layer.
    fn window() -> Option<(Retained<AnyObject>, Retained<AnyObject>, Retained<AnyObject>)> {
        // SAFETY: AppKit reads on the main thread.
        unsafe {
            let app: Retained<AnyObject> = msg_send![AnyClass::get(c"NSApplication")?, sharedApplication];
            let windows: Retained<AnyObject> = msg_send![&*app, windows];
            let count: usize = msg_send![&*windows, count];
            (0..count).find_map(|i| {
                let w: Retained<AnyObject> = msg_send![&*windows, objectAtIndex: i];
                let visible: Bool = msg_send![&*w, isVisible];
                if !visible.as_bool() { return None; }
                let view: Retained<AnyObject> = msg_send![&*w, contentView];
                let layer: Option<Retained<AnyObject>> = msg_send![&*view, layer];
                let layer = layer?;
                let layer = metal_layer(&layer)?;
                Some((w, view, layer))
            })
        }
    }

    const DOWN: usize = 1;
    const UP: usize = 2;
    const DRAGGED: usize = 6;

    /// Posts a left mouse event at `at`, in the window's coordinates, to the app's queue.
    fn post(kind: usize, at: CGPoint, window_number: isize) {
        autoreleasepool(|_| unsafe {
            // SAFETY: NSEvent and NSApplication class methods; postEvent:atStart: may be called
            // from any thread.
            let info: Retained<AnyObject> = msg_send![AnyClass::get(c"NSProcessInfo").unwrap(), processInfo];
            let now: f64 = msg_send![&*info, systemUptime];
            let event: Option<Retained<AnyObject>> = msg_send![AnyClass::get(c"NSEvent").unwrap(),
                mouseEventWithType: kind, location: at, modifierFlags: 0usize, timestamp: now,
                windowNumber: window_number, context: std::ptr::null_mut::<AnyObject>(), eventNumber: 0isize,
                clickCount: 1isize, pressure: if kind == UP { 0.0f32 } else { 1.0f32 }];
            let app: Retained<AnyObject> = msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication];
            let _: () = msg_send![&*app, postEvent: &*event.unwrap(), atStart: false];
        });
    }

    /// Drags one edge out by 25 steps of 6 points and back, an event every 12 ms. `edge` maps
    /// the window's frame at the start, its frame now (both `[x, y, w, h]` in screen points) and
    /// the drag so far to the pointer in window coordinates.
    fn drag(number: isize, edge: impl Fn([f64; 4], [f64; 4], f64) -> CGPoint) {
        let frame = || *FRAME.lock().unwrap();
        let start = frame();
        let at = |d: f64| edge(start, frame(), d);
        post(DOWN, at(0.0), number);
        let steps = 25;
        for d in (1..=steps).chain((0..steps).rev()).map(|k| k as f64 * 6.0) {
            std::thread::sleep(Duration::from_millis(12));
            post(DRAGGED, at(d), number);
        }
        std::thread::sleep(Duration::from_millis(12));
        post(UP, at(0.0), number);
        std::thread::sleep(Duration::from_millis(400));
    }

    fn on_main(handle: &AppHandle<State>, f: impl FnOnce(&mut State, &mut Tx) + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        handle.update(move |s, t| { f(s, t); let _ = tx.send(()); });
        rx.recv_timeout(Duration::from_secs(5)).expect("the app answers");
    }

    fn near(a: (f64, f64), b: (f64, f64)) -> bool { (a.0 - b.0).abs() <= 1.0 && (a.1 - b.1).abs() <= 1.0 }

    fn ms(d: Duration) -> f64 { d.as_secs_f64() * 1000.0 }

    /// One drag's sizes: each line says whether its frame was drawn before AppKit carried on.
    fn report(name: &str, events: &[Ev]) {
        let (mut sizes, mut drawn, mut wait) = (0, 0, Duration::ZERO);
        let (mut late, mut gap, mut last) = (Vec::new(), Duration::ZERO, None::<Instant>);
        let mut since = Vec::new();
        for (i, ev) in events.iter().enumerate() {
            match ev {
                Ev::Acquire { px, sync, wait: w, .. } => { since.push((*px, *sync)); wait = wait.max(*w); }
                Ev::Resized { at, px, live } => {
                    let drew = std::mem::take(&mut since);
                    if !live { continue; }
                    if let Some(last) = last { gap = gap.max(*at - last); }
                    last = Some(*at);
                    sizes += 1;
                    if drew.iter().any(|&(p, sync)| sync && near(p, *px)) {
                        drawn += 1;
                        println!("  {name} {:>4}x{:<4} px  drawn with the resize", px.0, px.1);
                        continue;
                    }
                    // How long the window showed an older frame at this size: until a drawable
                    // this size, or the next resize.
                    let next = events[i + 1..].iter().find_map(|e| match e {
                        Ev::Acquire { at: a, px: p, .. } if near(*p, *px) => Some(Some(*a - *at)),
                        Ev::Resized { .. } => Some(None),
                        _ => None,
                    }).flatten();
                    let shown = match next {
                        Some(d) => { late.push(d); format!("an older frame stretched to it for {:.1} ms", ms(d)) }
                        None => "never drawn: an older frame stretched to it until the next size".into(),
                    };
                    println!("  {name} {:>4}x{:<4} px  {shown}", px.0, px.1);
                }
                Ev::Mark(_) => {}
            }
        }
        let late_note = if late.is_empty() { String::new() } else {
            format!("; {} drawn late, by {:.1} ms on average", late.len(), ms(late.iter().sum::<Duration>()) / late.len() as f64)
        };
        check(&format!("{name}: every size the drag reached is drawn with its resize"), sizes > 0 && drawn == sizes,
            format!("{drawn} of {sizes} drawn{late_note}"));
        check(&format!("{name}: no drawable wait stalls the drag"), wait < Duration::from_millis(50), format!("waited {:.1} ms", ms(wait)));
        println!("  {name}: {sizes} sizes, longest between sizes {:.1} ms, longest drawable wait {:.1} ms", ms(gap), ms(wait));
    }

    /// A resize the user didn't drag: it ends with a frame at the window's final size, and no
    /// drawable wait stalls it.
    fn settles(name: &str, events: &[Ev]) {
        let size = events.iter().rev().find_map(|e| match e { Ev::Resized { px, .. } => Some(*px), _ => None });
        let drawn = events.iter().rev().find_map(|e| match e { Ev::Acquire { px, .. } => Some(*px), _ => None });
        let wait = events.iter().filter_map(|e| match e { Ev::Acquire { wait, .. } => Some(*wait), _ => None }).max().unwrap_or_default();
        let sizes = events.iter().filter(|e| matches!(e, Ev::Resized { .. })).count();
        let live = events.iter().filter(|e| matches!(e, Ev::Resized { live: true, .. })).count();
        check(&format!("{name}: ends drawn at the window's size"), size.is_some() && size.zip(drawn).is_some_and(|(a, b)| near(a, b)),
            format!("window {size:?}, last frame {drawn:?}"));
        check(&format!("{name}: no drawable wait stalls it"), wait < Duration::from_millis(50), format!("waited {:.1} ms", ms(wait)));
        println!("  {name}: {sizes} sizes, {live} of them in a live resize, longest drawable wait {:.1} ms", ms(wait));
    }

    pub fn run() {
        let slow = std::env::var("RUNE_LIVE_RESIZE_SLOW_MS").ok().and_then(|ms| ms.parse().ok()).map(Duration::from_millis);
        let spec = AppSpec { min_size: Some((200.0, 200.0)), ..AppSpec::new("Live resize check", (480.0, 400.0), State::default, view) };
        let mut app = App::new(spec).args(vec![]);
        if let Some(slow) = slow { app = app.compositor(Slow(slow)); }
        let handle = app.handle();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            let (tx, rx) = std::sync::mpsc::channel();
            on_main(&handle, move |_, _| {
                let (window, view, layer) = window().expect("Rune's window and its Metal layer");
                // SAFETY: CALayer and NSWindow reads and calls on the main thread.
                unsafe {
                    let gravity: Retained<AnyObject> = msg_send![&*layer, contentsGravity];
                    let gravity: *const std::ffi::c_char = msg_send![&*gravity, UTF8String];
                    let gravity = std::ffi::CStr::from_ptr(gravity).to_string_lossy().into_owned();
                    let flipped: Bool = msg_send![&*layer, contentsAreFlipped];
                    let scale: f64 = msg_send![&*layer, contentsScale];
                    let backing: f64 = msg_send![&*window, backingScaleFactor];
                    let want = if flipped.as_bool() { "bottomLeft" } else { "topLeft" };
                    check("late frames stay at the window's top left", gravity == want, format!("contentsGravity {gravity}, contents flipped: {}", flipped.as_bool()));
                    check("the layer's scale is the window's", scale == backing, format!("{scale} vs {backing}"));
                    let number: isize = msg_send![&*window, windowNumber];
                    let app: Retained<AnyObject> = msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication];
                    let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
                    let _: () = msg_send![&*window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
                    store_frame(&window);
                    hook_layer(&layer);
                    watch_resizes(&window, &view);
                    let _ = tx.send(number);
                    // Held for the run: the observer reads the window and its view.
                    std::mem::forget((window, view, layer));
                }
            });
            let number = rx.recv().unwrap();
            std::thread::sleep(Duration::from_millis(300));
            log(Ev::Mark("right"));
            // The right edge: the window's origin stays put.
            drag(number, |start, _, d| CGPoint { x: start[2] + 2.0 + d, y: start[3] / 2.0 });
            log(Ev::Mark("bottom"));
            // The bottom edge: AppKit's origin is the bottom left, so it moves with the edge.
            drag(number, |start, now, d| CGPoint { x: start[2] / 2.0, y: start[1] - 2.0 - d - now[1] });
            log(Ev::Mark("left"));
            // The left edge, the right one pinned: the origin moves with it.
            drag(number, |start, now, d| CGPoint { x: start[0] - 2.0 - d - now[0], y: start[3] / 2.0 });
            log(Ev::Mark("top"));
            // The top edge, the bottom one pinned.
            drag(number, |start, now, d| CGPoint { x: start[2] / 2.0, y: start[1] + start[3] + 2.0 + d - now[1] });
            log(Ev::Mark("after"));
            on_main(&handle, |s, _| {
                let (_, _, layer) = window().expect("Rune's window and its Metal layer");
                let sync: Bool = unsafe { msg_send![&*layer, presentsWithTransaction] };
                check("ending the drag clears presentsWithTransaction while idle", !sync.as_bool(), format!("presentsWithTransaction: {}", sync.as_bool()));
                s.n += 1;
            });
            std::thread::sleep(Duration::from_millis(300));
            // Rune's own animated resize, out and back.
            log(Ev::Mark("animated"));
            on_main(&handle, |_, t| t.resize_window((560.0, 460.0)));
            std::thread::sleep(Duration::from_millis(1200));
            on_main(&handle, |_, t| t.resize_window((480.0, 400.0)));
            std::thread::sleep(Duration::from_millis(1200));
            // Into full screen and out.
            log(Ev::Mark("full screen"));
            let toggle = || on_main(&handle, |_, _| {
                let (window, ..) = window().expect("Rune's window");
                // SAFETY: an NSWindow action on the main thread.
                unsafe { let _: () = msg_send![&*window, toggleFullScreen: std::ptr::null::<AnyObject>()]; }
            });
            toggle();
            std::thread::sleep(Duration::from_millis(2500));
            toggle();
            std::thread::sleep(Duration::from_millis(2500));
            log(Ev::Mark("end"));
            on_main(&handle, |s, _| s.n += 1);
            std::thread::sleep(Duration::from_millis(300));
            on_main(&handle, |_, _| {
                let events = std::mem::take(&mut *LOG.lock().unwrap());
                if std::env::var_os("RUNE_LIVE_RESIZE_EVENTS").is_some() { eprintln!("{events:#?}"); }
                let at = |name| events.iter().position(|e| matches!(e, Ev::Mark(m) if *m == name)).unwrap();
                let after = at("after");
                report("right edge", &events[at("right")..at("bottom")]);
                report("bottom edge", &events[at("bottom")..at("left")]);
                report("left edge", &events[at("left")..at("top")]);
                report("top edge", &events[at("top")..after]);
                let next = events[after..].iter().find_map(|e| match e { Ev::Acquire { sync, .. } => Some(*sync), _ => None });
                check("a frame after the drag presents on its own", next == Some(false), format!("{next:?}"));
                settles("resize_window", &events[at("animated")..at("full screen")]);
                settles("full screen", &events[at("full screen")..at("end")]);
                let next = events[at("end")..].iter().find_map(|e| match e { Ev::Acquire { sync, .. } => Some(*sync), _ => None });
                check("a frame after full screen presents on its own", next == Some(false), format!("{next:?}"));
                std::process::exit(i32::from(*FAILED.lock().unwrap()));
            });
        });
        app.run();
    }
}
