//! Shows and hides two overlays that give their surface back while hidden (a HUD and a live-text
//! plate, as Hex shows on every dictation) a few thousand times, and checks that the process's
//! memory stays flat and that every show draws on the one GPU device the first show made
//! (hex#1268: memory grew by gigabytes over an hour of dictation on a hybrid laptop). Prints the
//! memory every few hundred cycles. Opens overlays, so it only runs with RUNE_GUI_TESTS=1 in a
//! desktop session; `RUNE_SOAK_CYCLES` sets the count (2000 by default).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use rune::*;

static STARTS: AtomicUsize = AtomicUsize::new(0);

/// Memory may grow this much from the warmed-up baseline to the last cycle.
const ALLOWED_GROWTH_MB: f64 = 48.0;
/// Cycles before the baseline: allocators, caches and the driver settle.
const WARM_UP: usize = 100;

fn hud(_: &(), _: &mut Cx<()>) -> El { el().size(200.0, 40.0).fill(oklcha(0.2, 0.0, 0.0, 0.6)).radius(20.0) }
fn plate(_: &(), _: &mut Cx<()>) -> El {
    el().w_full().h_full().fill(oklcha(0.25, 0.0, 0.0, 0.8)).radius(12.0).child(text("live words"))
}

fn wait(limit: Duration, f: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while !f() {
        if start.elapsed() > limit { return false; }
        std::thread::sleep(Duration::from_micros(200));
    }
    true
}

/// Shows `overlay` until it has presented a frame.
fn show<T>(name: &str, overlay: &OverlayHandle<T>) {
    let presents = overlay.surface_stats().totals.presents;
    overlay.show();
    assert!(wait(Duration::from_secs(5), || overlay.surface_stats().totals.presents > presents && overlay.surface_description().is_some()),
        "{name} presented nothing: {:?}", overlay.failure());
}

/// Hides `overlay` until its surface is gone.
fn hide<T>(name: &str, overlay: &OverlayHandle<T>) {
    overlay.hide();
    assert!(wait(Duration::from_secs(5), || overlay.surface_description().is_none()), "{name} kept its surface");
}

#[cfg(target_os = "windows")]
fn memory_mb() -> f64 {
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

/// Resident memory, from `/proc/self/statm` (pages).
#[cfg(target_os = "linux")]
fn memory_mb() -> f64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: f64 = statm.split_whitespace().nth(1).and_then(|p| p.parse().ok()).unwrap_or(0.0);
    pages * 4096.0 / 1e6
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn memory_mb() -> f64 { 0.0 }

fn main() {
    if std::env::var_os("RUNE_GUI_TESTS").is_none() {
        println!("overlay_soak: skipped (set RUNE_GUI_TESTS=1 in a desktop session)");
        return;
    }
    let cycles: usize = std::env::var("RUNE_SOAK_CYCLES").ok().and_then(|n| n.parse().ok()).unwrap_or(2000).max(WARM_UP + 1);
    std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(3600)); eprintln!("timeout"); std::process::exit(2) });
    on_gpu(|event| if event == GpuEvent::Starting { STARTS.fetch_add(1, Ordering::SeqCst); });
    let mut app = App::new(AppSpec::new("overlay soak", (100.0, 100.0), || (), hud)).args(Vec::new()).windowless();
    let hud = app.overlay(OverlaySpec::new("soak hud", (200.0, 40.0), hud)
        .options(OverlayOptions { prewarm: true, ..Default::default() })
        .release_gpu_when_hidden(), ());
    let plate = app.overlay(OverlaySpec::new("soak live text", (420.0, 44.0), plate)
        .options(OverlayOptions { placement: Some(OverlayPlacement::Edge { edge: ScreenEdge::Top, margin: 60.0, align: Align::Center, inset: 0.0 }), ..Default::default() })
        .release_gpu_when_hidden(), ());
    let handle = app.handle();
    let result = Arc::new(Mutex::new(None));
    let out = result.clone();
    std::thread::spawn(move || {
        let run = std::panic::catch_unwind(move || {
            let started = Instant::now();
            let mut baseline = 0.0;
            for cycle in 0..cycles {
                show("hud", &hud);
                show("live text", &plate);
                hide("live text", &plate);
                hide("hud", &hud);
                if cycle + 1 == WARM_UP { baseline = memory_mb(); }
                if (cycle + 1) % 250 == 0 {
                    println!("cycle {:>5}: {:.0} MB, {} GPU device(s) made, {:.0} s", cycle + 1, memory_mb(), STARTS.load(Ordering::SeqCst), started.elapsed().as_secs_f64());
                }
            }
            let end = memory_mb();
            let starts = STARTS.load(Ordering::SeqCst);
            println!("overlay_soak: {cycles} cycles, {baseline:.0} MB after {WARM_UP}, {end:.0} MB at the end, {starts} GPU device(s) made");
            assert!(hud.failure().is_none() && plate.failure().is_none(), "{:?} {:?}", hud.failure(), plate.failure());
            assert_eq!(starts, 1, "every show draws on the first show's device");
            assert!(end - baseline < ALLOWED_GROWTH_MB, "memory grew {:.0} MB over {} cycles", end - baseline, cycles - WARM_UP);
        });
        *out.lock().unwrap() = Some(run.is_ok());
        handle.quit();
    });
    app.run();
    assert_eq!(*result.lock().unwrap(), Some(true), "overlay_soak failed");
    println!("overlay_soak: ok");
}
