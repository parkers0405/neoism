//! `open()` activates an accessory app and makes its window key while another app is
//! frontmost: the first open of a window that doesn't exist yet, a reopen after hiding, and a
//! hidden `App::window` window. A launcher panel's open and an overlay's show stay
//! nonactivating. Starts a helper app to hold the focus (handed to it with CuaDriver when
//! nothing yields), so it only runs with RUNE_GUI_TESTS=1 in a logged-in macOS GUI session.

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
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::NSString;
    use rune::*;
    use crate::macos_support::{activate_helper, app_active, frontmost_pid, on_main, wait};

    const HELPER: &str = "RUNE_OPEN_ACTIVATES_HELPER";
    const MAIN_TITLE: &str = "open activates main";
    const SETTINGS_TITLE: &str = "open activates settings";

    fn view<S>(_: &S, _: &mut Cx<S>) -> El { text("open activates") }

    /// The key window's title, if this app has a key window.
    fn key_title() -> Option<String> {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let window: *mut AnyObject = msg_send![app, keyWindow];
            if window.is_null() { return None }
            let title: *mut NSString = msg_send![window, title];
            (!title.is_null()).then(|| (*title).to_string())
        }
    }

    /// The helper keeps the front. A key nonactivating panel can still make `isActive` report
    /// true, so that isn't checked.
    fn expect_inactive(handle: &AppHandle<()>, pid: i32, what: &str) {
        std::thread::sleep(Duration::from_millis(400));
        let front = on_main(handle, frontmost_pid);
        assert_eq!(front, pid, "{what} took the front from the helper");
    }

    fn expect_active(handle: &AppHandle<()>, title: &'static str, what: &str) {
        let me = std::process::id() as i32;
        let start = Instant::now();
        loop {
            let (active, front, key) = on_main(handle, || (app_active(), frontmost_pid(), key_title()));
            if active && front == me && key.as_deref() == Some(title) { return }
            assert!(start.elapsed() < Duration::from_secs(5),
                "{what}: active={active}, frontmost={front} (this app {me}), key window {key:?}, want {title:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn helper() {
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); std::process::exit(0) });
        App::new(AppSpec::new("open activates helper", (240.0, 80.0), || (), view)).args(Vec::new()).run();
    }

    pub fn main() {
        if std::env::var_os(HELPER).is_some() { return helper() }
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("open_activates: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(90)); eprintln!("timeout"); std::process::exit(2) });
        let mut child = Command::new(std::env::current_exe().unwrap()).env(HELPER, "1").spawn().expect("helper");
        let pid = child.id() as i32;
        let mut app = App::new(AppSpec::new(MAIN_TITLE, (240.0, 120.0), || (), view)).args(Vec::new())
            .accessory().windowless();
        let main = app.handle();
        let (_, settings) = app.window(WindowSpec::new("settings", AppSpec::new(SETTINGS_TITLE, (240.0, 120.0), || (), view))
            .options(WindowOptions { visible: false, ..Default::default() }));
        let (_, launcher) = app.window(WindowSpec::new("launcher", AppSpec::new("open activates launcher", (240.0, 80.0), || (), view))
            .configured_options(WindowOptions::launcher_panel()));
        let overlay = app.overlay(OverlaySpec::new("open activates overlay", (200.0, 60.0), view), ());
        let result = std::sync::Arc::new(Mutex::new(None));
        let out = result.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                std::thread::sleep(Duration::from_millis(500));
                assert!(!on_main(&main, app_active), "an accessory app doesn't activate at launch");
                activate_helper(&main, pid);
                assert!(!main.is_visible(), "windowless starts without a window");

                overlay.show();
                expect_inactive(&main, pid, "overlay show");
                overlay.hide();
                launcher.open();
                expect_inactive(&main, pid, "launcher open");
                launcher.hide();
                std::thread::sleep(Duration::from_millis(200));
                activate_helper(&main, pid);

                main.open();
                expect_active(&main, MAIN_TITLE, "the first open of a new window");
                println!("first open: active and key");

                main.hide();
                wait("hide", || !main.is_visible());
                activate_helper(&main, pid);
                main.open();
                expect_active(&main, MAIN_TITLE, "a reopen after hiding");
                println!("reopen: active and key");

                activate_helper(&main, pid);
                settings.open();
                expect_active(&main, SETTINGS_TITLE, "opening a hidden App::window window");
                println!("App::window open: active and key");
            });
            *out.lock().unwrap() = Some(run.is_ok());
            main.quit();
        });
        app.run();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(*result.lock().unwrap(), Some(true), "open_activates failed");
        println!("open_activates: ok");
    }
}
