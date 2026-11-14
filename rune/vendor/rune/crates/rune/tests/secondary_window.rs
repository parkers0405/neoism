//! A window added with `App::window` starts hidden, opens later with its own state, closes
//! by hiding without quitting the app, reopens with that state, and never keeps the app
//! running once the main window closes, closing with it. Opens native windows, so it only runs with
//! RUNE_GUI_TESTS=1 in a logged-in GUI session.

use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::{Duration, Instant};
use rune::*;

struct Main;
struct Settings { label: String }

static RENDERS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CLOSES: AtomicUsize = AtomicUsize::new(0);

fn main_view(_: &Main, _: &mut Cx<Main>) -> El { text("main") }

fn settings_view(s: &Settings, _: &mut Cx<Settings>) -> El {
    RENDERS.lock().unwrap().push(s.label.clone());
    text(&s.label)
}

fn wait(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn state<T: Send + 'static>(handle: &AppHandle<Settings>, f: impl FnOnce(&Settings) -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    handle.update(move |s, _| { let _ = tx.send(f(s)); });
    rx.recv_timeout(Duration::from_secs(5)).expect("update ran")
}

fn main() {
    if std::env::var_os("RUNE_GUI_TESTS").is_none() {
        println!("secondary_window: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
        return;
    }
    std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
    let mut app = App::new(AppSpec::new("secondary main", (240.0, 120.0), || Main, main_view)).args(Vec::new());
    let main = app.handle();
    let settings = App::new(AppSpec::new("secondary settings", (240.0, 120.0), || Settings { label: "initial".into() }, settings_view))
        .on_close(|_, _| { CLOSES.fetch_add(1, Ordering::SeqCst); });
    let (settings, window) = app.window(WindowSpec::from_app("settings", settings)
        .options(WindowOptions { visible: false, ..Default::default() }));
    let result = std::sync::Arc::new(Mutex::new(None));
    let out = result.clone();
    std::thread::spawn(move || {
        let run = std::panic::catch_unwind(|| {
            wait("main window", || main.is_visible());
            std::thread::sleep(Duration::from_millis(200));
            assert!(!settings.is_visible(), "visible: false starts hidden");

            window.open();
            wait("settings open", || settings.is_visible());
            assert!(main.is_visible());
            settings.update(|s, _| s.label = "opened".into());
            wait("settings render", || RENDERS.lock().unwrap().iter().any(|l| l == "opened"));

            window.close();
            wait("settings close", || !settings.is_visible());
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(CLOSES.load(Ordering::SeqCst), 1, "close runs on_close once");
            assert!(main.is_visible(), "closing the secondary window keeps the app and main window");

            settings.open();
            wait("settings reopen", || settings.is_visible());
            assert_eq!(state(&settings, |s| s.label.clone()), "opened", "reopening keeps the state");
            assert!(window.is_ready());
        });
        *out.lock().unwrap() = Some(run.is_ok());
        // The settings window is open and retained, yet the app ends with its main window.
        main.close();
    });
    app.run();
    assert_eq!(*result.lock().unwrap(), Some(true), "secondary_window failed");
    assert_eq!(CLOSES.load(Ordering::SeqCst), 2, "the open settings window closes with the app");
    println!("secondary_window: ok");
}
