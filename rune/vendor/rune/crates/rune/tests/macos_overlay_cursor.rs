//! `Cursor::Grab` over an overlay's hit region shows the open hand while another app is
//! frontmost and the overlay isn't key, with the `macos-background-cursor` feature. The overlay
//! takes input only on its hit regions (`OverlayInput::Regions`); a helper app holds the focus
//! (handed to it with CuaDriver when nothing yields). The overlay polls the real pointer, so the
//! test warps it (which moves the cursor but posts no events) and reads the window server's
//! cursor with `[NSCursor currentSystemCursor]`. Off the hit region it is the arrow again.
//! Fading out, the panel clicks through at once, for the whole fade.
//! Opens windows, so it only runs with RUNE_GUI_TESTS=1 in a logged-in GUI session.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod macos_support;

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use objc2::{class, msg_send, msg_send_id, rc::Retained, runtime::{AnyObject, Bool}};
    use objc2_foundation::{NSArray, NSPoint, NSSize};
    use rune::*;
    use crate::macos_support::{activate_helper, app_active, frontmost_pid, on_main, wait, warp};

    const HELPER: &str = "RUNE_OVERLAY_CURSOR_HELPER";
    /// Where the overlay goes, in points from the primary display's top-left.
    const PANEL: [f32; 4] = [300.0, 300.0, 240.0, 120.0];
    /// The grabbable card inside it; the rest of the panel clicks through.
    const CARD: [f32; 4] = [20.0, 20.0, 120.0, 80.0];

    fn view<S>(_: &S, _: &mut Cx<S>) -> El { text("overlay cursor helper") }

    fn overlay_view(_: &(), _: &mut Cx<()>) -> El {
        let [x, y, w, h] = CARD;
        el().w_full().h_full().fill(Color::WHITE.alpha(0.3)).child(
            el().key("card").absolute(x, y).size(w, h).radius(10.0)
                .fill(rune::color::oklch(0.6, 0.18, 255.0))
                .hit()
                .cursor(Cursor::Grab),
        )
    }

    /// The panel with CGWindowID `number`: (key, ignores mouse events).
    fn panel(number: u32) -> Option<(bool, bool)> {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: Retained<NSArray<AnyObject>> = msg_send_id![app, windows];
            windows.iter().find(|w| { let n: isize = msg_send![*w, windowNumber]; n == number as isize }).map(|w| {
                let key: Bool = msg_send![w, isKeyWindow];
                let ignores: Bool = msg_send![w, ignoresMouseEvents];
                (key.as_bool(), ignores.as_bool())
            })
        }
    }

    /// A cursor's shape as far as the window server tells: (hot spot, image size).
    type Shape = ((f64, f64), (f64, f64));

    fn shape(cursor: *mut AnyObject) -> Option<Shape> {
        if cursor.is_null() { return None }
        unsafe {
            let hot: NSPoint = msg_send![cursor, hotSpot];
            let image: *mut AnyObject = msg_send![cursor, image];
            if image.is_null() { return None }
            let size: NSSize = msg_send![image, size];
            Some(((hot.x, hot.y), (size.width, size.height)))
        }
    }

    /// (the window server's current cursor, the open hand, the arrow).
    fn shapes() -> (Option<Shape>, Shape, Shape) {
        unsafe {
            let current: *mut AnyObject = msg_send![class!(NSCursor), currentSystemCursor];
            let hand: *mut AnyObject = msg_send![class!(NSCursor), openHandCursor];
            let arrow: *mut AnyObject = msg_send![class!(NSCursor), arrowCursor];
            (shape(current), shape(hand).expect("openHandCursor"), shape(arrow).expect("arrowCursor"))
        }
    }

    /// Waits for the window server's cursor to take `want`'s shape.
    fn expect_cursor(handle: &AppHandle<()>, what: &str, want: impl Fn(Shape, Shape) -> Shape + Send + Copy + 'static) {
        let start = Instant::now();
        loop {
            let (current, hand, arrow) = on_main(handle, shapes);
            let want = want(hand, arrow);
            if current == Some(want) { return }
            assert!(start.elapsed() < Duration::from_secs(3), "{what}: the cursor is {current:?}, want {want:?} (open hand {hand:?}, arrow {arrow:?})");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn drive(handle: &AppHandle<()>, overlay: &OverlayHandle<()>, pid: i32) {
        let (_, hand, arrow) = on_main(handle, shapes);
        assert_ne!(hand, arrow, "the open hand and the arrow need different shapes to tell them apart");
        wait("the overlay's panel", || overlay.window_number().is_some());
        let number = overlay.window_number().unwrap();
        activate_helper(handle, pid);
        assert!(!on_main(handle, app_active), "the helper holds the focus");

        let card = (f64::from(PANEL[0] + CARD[0] + CARD[2] / 2.0), f64::from(PANEL[1] + CARD[1] + CARD[3] / 2.0));
        let click_through = (f64::from(PANEL[0] + PANEL[2] - 20.0), f64::from(PANEL[1] + PANEL[3] - 10.0));
        let away = (f64::from(PANEL[0] + PANEL[2] + 200.0), f64::from(PANEL[1] + PANEL[3] + 200.0));

        on_main(handle, move || warp(away));
        std::thread::sleep(Duration::from_millis(300));
        let (before, _, _) = on_main(handle, shapes);
        assert_ne!(before, Some(hand), "the open hand before the pointer reaches the overlay");

        on_main(handle, move || warp(card));
        expect_cursor(handle, "over the card", |hand, _| hand);
        let (key, ignores) = on_main(handle, move || panel(number)).expect("the overlay panel is one of the app's windows");
        assert!(!key, "the overlay isn't key");
        assert!(!ignores, "the panel takes the pointer over its hit region");
        assert_eq!(on_main(handle, frontmost_pid), pid, "the helper is still frontmost");
        std::thread::sleep(Duration::from_millis(300));
        expect_cursor(handle, "still over the card", |hand, _| hand);

        on_main(handle, move || warp(click_through));
        expect_cursor(handle, "over the panel off its hit region", |_, arrow| arrow);
        on_main(handle, move || warp(card));
        expect_cursor(handle, "back over the card", |hand, _| hand);
        on_main(handle, move || warp(away));
        expect_cursor(handle, "off the overlay", |_, arrow| arrow);
        assert_eq!(on_main(handle, frontmost_pid), pid, "the helper kept the front throughout");

        // Fading out, the panel stops taking clicks at once, though it's still on screen.
        on_main(handle, move || warp(card));
        expect_cursor(handle, "over the card before the fade", |hand, _| hand);
        overlay.fade_out(Duration::from_secs(2));
        wait("the fading panel to click through", || on_main(handle, move || panel(number).is_some_and(|(_, ignores)| ignores)));
        assert!(on_main(handle, move || ordered_in(number)), "checked while the fade runs, before the panel orders out");
        overlay.show();
        wait("the shown panel to take the pointer again", || on_main(handle, move || panel(number).is_some_and(|(_, ignores)| !ignores)));
    }

    /// Whether the panel with CGWindowID `number` is still ordered in (a fade-out orders it out
    /// when it ends).
    fn ordered_in(number: u32) -> bool {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: Retained<NSArray<AnyObject>> = msg_send_id![app, windows];
            windows.iter().any(|w| {
                let n: isize = msg_send![&*w, windowNumber];
                let visible: Bool = msg_send![&*w, isVisible];
                n == number as isize && visible.as_bool()
            })
        }
    }

    fn helper() {
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); std::process::exit(0) });
        App::new(AppSpec::new("overlay cursor helper", (240.0, 80.0), || (), view)).args(Vec::new()).run();
    }

    pub fn main() {
        if std::env::var_os(HELPER).is_some() { return helper() }
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("macos_overlay_cursor: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
        let mut child = Command::new(std::env::current_exe().unwrap()).env(HELPER, "1").spawn().expect("helper");
        let pid = child.id() as i32;
        let mut app = App::new(AppSpec::new("overlay cursor", (200.0, 100.0), || (), view)).args(Vec::new())
            .accessory().windowless();
        let [x, y, width, height] = PANEL;
        let options = OverlayOptions {
            placement: Some(OverlayPlacement::Rect { x, y, width, height }),
            display: DisplayTarget::Primary,
            input: OverlayInput::Regions,
            ..Default::default()
        };
        let overlay = app.overlay(OverlaySpec::new("overlay cursor", (width, height), overlay_view).options(options).macos_level(3), ());
        overlay.show();
        let handle = app.handle();
        let result = std::sync::Arc::new(Mutex::new(None));
        let out = result.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                std::thread::sleep(Duration::from_millis(500));
                drive(&handle, &overlay, pid);
            }));
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(*result.lock().unwrap(), Some(true), "macos_overlay_cursor failed");
        println!("macos_overlay_cursor: ok");
    }
}
