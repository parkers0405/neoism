//! A prewarmed overlay with `release_gpu_when_hidden` makes its window at startup on X11, and
//! nothing on Windows, and no GPU surface, nor device, until it shows; each hide gives the
//! surface back (and on Windows the window), and `release_gpu_after` does so once the overlay
//! stayed hidden that long. The device waits for the next show, which takes it rather than
//! making another (hex#1268). Prints
//! how long each show waits for its first present, and the process's private memory along the
//! way. Opens overlays, so it only runs with RUNE_GUI_TESTS=1 in a desktop session.

#[cfg(target_os = "macos")]
fn main() {}

#[cfg(not(target_os = "macos"))]
fn main() { lazy::main() }

#[cfg(not(target_os = "macos"))]
mod lazy {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use rune::*;

    static STARTS: AtomicUsize = AtomicUsize::new(0);

    fn view(_: &(), _: &mut Cx<()>) -> El { el().w_full().h_full().fill(oklcha(0.6, 0.2, 250.0, 0.2)) }
    fn hud(_: &(), _: &mut Cx<()>) -> El { el().size(200.0, 40.0).fill(oklcha(0.2, 0.0, 0.0, 0.6)).radius(20.0) }

    fn wait(limit: Duration, f: impl Fn() -> bool) -> Option<Duration> {
        let start = Instant::now();
        while !f() {
            if start.elapsed() > limit { return None; }
            std::thread::sleep(Duration::from_micros(200));
        }
        Some(start.elapsed())
    }

    /// The time from `show` to the overlay's first present; a locked session may present
    /// nothing, and then the surface is what's checked.
    fn show<T>(name: &str, overlay: &OverlayHandle<T>, results: &mut Vec<(String, Option<f64>)>) {
        let presents = overlay.surface_stats().totals.presents;
        overlay.show();
        let presented = wait(Duration::from_secs(5), || overlay.surface_stats().totals.presents > presents);
        assert!(wait(Duration::from_secs(5), || overlay.surface_description().is_some()).is_some(), "{name} made no surface: {:?}", overlay.failure());
        let ms = presented.map(|d| d.as_secs_f64() * 1000.0);
        println!("{name}: first present {}", ms.map_or("none".into(), |ms| format!("{ms:.1} ms")));
        results.push((name.to_owned(), ms));
    }

    fn hide<T>(name: &str, overlay: &OverlayHandle<T>, released: bool) {
        overlay.hide();
        if released {
            assert!(wait(Duration::from_secs(2), || overlay.surface_description().is_none()).is_some(), "{name}: hidden, its surface is released");
        } else {
            std::thread::sleep(Duration::from_millis(100));
            assert!(overlay.surface_description().is_some(), "{name}: hidden, its surface is kept");
        }
    }

    #[cfg(target_os = "windows")]
    fn private_mb() -> f64 {
        #[repr(C)]
        #[derive(Default)]
        struct Counters { cb: u32, faults: u32, peak_ws: usize, ws: usize, qpp: usize, qp: usize, qnpp: usize, qnp: usize, pagefile: usize, peak_pagefile: usize, private: usize }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
        }
        let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
        // SAFETY: fills `c`, whose `cb` names PROCESS_MEMORY_COUNTERS_EX.
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        c.private as f64 / 1e6
    }

    #[cfg(not(target_os = "windows"))]
    fn private_mb() -> f64 { 0.0 }

    pub fn main() {
        if std::env::var_os("RUNE_GUI_TESTS").is_none() {
            println!("overlay_gpu_lifecycle: skipped (set RUNE_GUI_TESTS=1 in a desktop session)");
            return;
        }
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(120)); eprintln!("timeout"); std::process::exit(2) });
        on_gpu(|event| if event == GpuEvent::Starting { STARTS.fetch_add(1, Ordering::SeqCst); });
        let mut app = App::new(AppSpec::new("overlay gpu lifecycle", (100.0, 100.0), || (), view)).args(Vec::new()).windowless();
        let created = Arc::new(AtomicUsize::new(0));
        let seen = created.clone();
        let full = app.overlay(OverlaySpec::new("lazy full screen", (100.0, 100.0), view)
            .options(OverlayOptions { placement: Some(OverlayPlacement::FullScreen), prewarm: true, ..Default::default() })
            .release_gpu_when_hidden()
            .on_created(move |_| { seen.fetch_add(1, Ordering::SeqCst); }), ());
        let idle_release = Duration::from_millis(800);
        let small = app.overlay(OverlaySpec::new("hud", (200.0, 40.0), hud).options(OverlayOptions { prewarm: true, ..Default::default() }).release_gpu_after(idle_release), ());
        let handle = app.handle();
        let result = Arc::new(Mutex::new(None));
        let out = result.clone();
        std::thread::spawn(move || {
            let run = std::panic::catch_unwind(move || {
                let mut r = Vec::new();
                if cfg!(target_os = "windows") {
                    std::thread::sleep(Duration::from_millis(1500));
                    assert_eq!(created.load(Ordering::SeqCst), 0, "a hidden overlay has no window on Windows");
                } else {
                    wait(Duration::from_secs(10), || full.availability() == Availability::Available).expect("the prewarmed window");
                    // Let startup make the prewarmed window.
                    std::thread::sleep(Duration::from_millis(1500));
                    assert_eq!(created.load(Ordering::SeqCst), 1, "on_created runs at startup");
                }
                assert!(full.surface_description().is_none(), "a hidden prewarmed overlay has no surface");
                assert_eq!(STARTS.load(Ordering::SeqCst), 0, "nothing shown, no GPU device");
                let idle = private_mb();

                show("full screen, first show of the process", &full, &mut r);
                assert_eq!(STARTS.load(Ordering::SeqCst), 1);
                println!("  {}", full.surface_description().unwrap());
                let shown = private_mb();
                hide("full screen", &full, true);
                std::thread::sleep(Duration::from_millis(500));
                let released = private_mb();

                show("full screen, on the parked device", &full, &mut r);
                assert_eq!(STARTS.load(Ordering::SeqCst), 1, "the next show takes the parked device");
                hide("full screen", &full, true);
                std::thread::sleep(Duration::from_millis(300));

                warm_gpu(wgpu::Features::empty());
                std::thread::sleep(Duration::from_millis(800));
                show("full screen, warmed 800 ms ahead", &full, &mut r);
                assert_eq!(STARTS.load(Ordering::SeqCst), 1, "warm_gpu makes nothing while a device waits");
                hide("full screen", &full, true);
                std::thread::sleep(Duration::from_millis(300));

                warm_gpu(wgpu::Features::empty());
                show("full screen, warmed at the show", &full, &mut r);
                assert_eq!(STARTS.load(Ordering::SeqCst), 1);
                hide("full screen", &full, true);
                std::thread::sleep(Duration::from_millis(300));

                show("hud, no device", &small, &mut r);
                hide("hud", &small, false);
                show("hud, shown again within release_gpu_after", &small, &mut r);
                hide("hud", &small, false);
                warm_gpu(wgpu::Features::empty());
                assert_eq!(STARTS.load(Ordering::SeqCst), 1, "warm_gpu makes nothing while the hud holds the device");
                show("full screen, on the hud's device", &full, &mut r);
                assert_eq!(STARTS.load(Ordering::SeqCst), 1, "the full-screen overlay shares the hud's device");
                hide("full screen", &full, true);
                std::thread::sleep(idle_release + Duration::from_millis(500));
                assert!(small.surface_description().is_none(), "the hud gave its surface back after release_gpu_after");
                show("hud, after release_gpu_after", &small, &mut r);
                assert_eq!(STARTS.load(Ordering::SeqCst), 1, "the hud takes the device it parked");
                hide("hud", &small, false);
                assert!(full.failure().is_none() && small.failure().is_none(), "{:?} {:?}", full.failure(), small.failure());
                println!("private memory: hidden at startup {idle:.0} MB, full screen shown {shown:.0} MB, hidden again {released:.0} MB");
                println!("| show | first present |");
                println!("| --- | --- |");
                for (name, ms) in r { println!("| {name} | {} |", ms.map_or("none".into(), |ms| format!("{ms:.0} ms"))); }
            });
            *out.lock().unwrap() = Some(run.is_ok());
            handle.quit();
        });
        app.run();
        assert_eq!(*result.lock().unwrap(), Some(true), "overlay_gpu_lifecycle failed");
        println!("overlay_gpu_lifecycle: ok");
    }
}
