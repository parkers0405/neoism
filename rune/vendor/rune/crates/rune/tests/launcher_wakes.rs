//! A retained launcher panel (`WindowOptions::launcher_panel`) runs `poll` on a plain
//! `AppHandle::wake`, hidden or shown, and never reveals cached pixels older than an update
//! in the same turn. With RUNE_LATENCY=1 it repeats 12 times and prints the median reveal
//! latency of both paths. Opens a native panel, so it only runs with RUNE_GUI_TESTS=1 in a
//! logged-in macOS GUI session.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() { mac::main() }

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::{Mutex, OnceLock, atomic::{AtomicUsize, Ordering}};
    use std::time::{Duration, Instant};
    use rune::*;

    struct State { label: String }

    static POLLS: AtomicUsize = AtomicUsize::new(0);
    static INBOX: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// Each rendered label, with whether the window was already revealed when it rendered.
    static RENDERS: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());
    static HANDLE: OnceLock<AppHandle<State>> = OnceLock::new();

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let visible = HANDLE.get().is_some_and(|h| h.is_visible());
        RENDERS.lock().unwrap().push((s.label.clone(), visible));
        text(&s.label)
    }

    fn poll(s: &mut State, _: f64, _: &mut Tx) -> bool {
        if BLOCK.swap(false, Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
            *UNBLOCKED.lock().unwrap() = Some(Instant::now());
            return false;
        }
        POLLS.fetch_add(1, Ordering::SeqCst);
        let Some(label) = INBOX.lock().unwrap().pop() else { return false };
        s.label = label;
        true
    }

    fn wait(what: &str, f: impl Fn() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn rendered(label: &str) -> Option<bool> {
        RENDERS.lock().unwrap().iter().find(|(l, _)| l == label).map(|(_, visible)| *visible)
    }

    fn wake_with(handle: &AppHandle<State>, label: &str) {
        let before = POLLS.load(Ordering::SeqCst);
        INBOX.lock().unwrap().push(label.into());
        handle.wake();
        wait(&format!("poll for {label}"), || POLLS.load(Ordering::SeqCst) > before);
        wait(&format!("a render of {label}"), || rendered(label).is_some());
    }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("launcher_wakes: skipped (set RUNE_GUI_TESTS=1 in a GUI session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(60)); eprintln!("timeout"); std::process::exit(2) });
        let spec = AppSpec { poll: Some(poll), ..AppSpec::new("launcher wakes", (240.0, 80.0), || State { label: "initial".into() }, view) };
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let app = App::new(spec).args(Vec::new()).keep_running()
            .configured_options(WindowOptions::launcher_panel())
            .on_ready(move |_| { let _ = ready_tx.send(()); });
        let handle = app.handle();
        let _ = HANDLE.set(handle.clone());
        let result = std::sync::Arc::new(Mutex::new(None));
        let out = result.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                ready_rx.recv_timeout(Duration::from_secs(10)).expect("launcher ready");
                hidden_and_shown_wakes(&handle);
                reveal_after_update(&handle);
            });
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        assert_eq!(*result.lock().unwrap(), Some(true), "launcher_wakes failed");
        println!("launcher_wakes: ok");
    }

    fn hidden_and_shown_wakes(handle: &AppHandle<State>) {
        assert!(!handle.is_visible());
        wake_with(handle, "hidden");
        assert_eq!(rendered("hidden"), Some(false), "a hidden poll change renders the cached frame hidden");
        let polls = POLLS.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        // A loop would poll every frame; AppKit may still ask for a stray hidden redraw.
        assert!(POLLS.load(Ordering::SeqCst) <= polls + 2, "a hidden launcher must not run a frame loop");
        assert!(!handle.is_visible());

        handle.open();
        wait("reveal", || handle.is_visible());
        std::thread::sleep(Duration::from_millis(100));
        wake_with(handle, "shown");
        assert_eq!(rendered("shown"), Some(true));
        handle.hide();
        wait("hide", || !handle.is_visible());
    }

    static BLOCK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static UNBLOCKED: Mutex<Option<Instant>> = Mutex::new(None);

    /// Queues `requests` while the event thread is busy in a hidden poll, so one drain takes
    /// them all, and returns the time from that drain's start until the window is shown.
    fn reveal_in_one_drain(handle: &AppHandle<State>, requests: impl FnOnce()) -> Duration {
        BLOCK.store(true, Ordering::SeqCst);
        handle.wake();
        std::thread::sleep(Duration::from_millis(30));
        requests();
        let start = Instant::now();
        while !handle.is_visible() { assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for reveal"); }
        let shown = Instant::now();
        shown - UNBLOCKED.lock().unwrap().take().expect("blocking update ran")
    }

    fn hide(handle: &AppHandle<State>) {
        handle.hide();
        wait("hide", || !handle.is_visible());
        std::thread::sleep(Duration::from_millis(150));
    }

    fn reveal_after_update(handle: &AppHandle<State>) {
        let latency = std::env::var("RUNE_LATENCY").as_deref() == Ok("1");
        let (mut cached, mut fresh) = (Vec::new(), Vec::new());
        for i in 0..if latency { 12 } else { 1 } {
            cached.push(reveal_in_one_drain(handle, || handle.open()));
            hide(handle);
            let label = format!("fresh {i}");
            let update = label.clone();
            fresh.push(reveal_in_one_drain(handle, || {
                handle.update(move |s, _| s.label = update);
                handle.open();
            }));
            assert_eq!(rendered(&label), Some(false), "the update rendered hidden before the reveal");
            hide(handle);
        }
        INBOX.lock().unwrap().clear();
        reveal_in_one_drain(handle, || {
            INBOX.lock().unwrap().push("polled".into());
            handle.wake();
            handle.open();
        });
        assert_eq!(rendered("polled"), Some(false), "a wake's poll ran and rendered before the reveal");
        hide(handle);
        if latency {
            let median = |v: &mut Vec<Duration>| { v.sort(); v[v.len() / 2].as_secs_f64() * 1000.0 };
            let (cached, fresh) = (median(&mut cached), median(&mut fresh));
            println!("launcher_wakes: reveal median {cached:.2} ms cached, {fresh:.2} ms after an update (+{:.2} ms)", fresh - cached);
        }
    }
}
