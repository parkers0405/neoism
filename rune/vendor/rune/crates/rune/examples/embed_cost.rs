//! What an embedded `LayerHost`, the way iOS apps host Rune, costs on each GPU backend: wgpu's
//! Metal backend or Rune's own (`metal-backend`). Pick one with `RUNE_GPU_BACKEND=wgpu|metal`
//! (`SIMCTL_CHILD_RUNE_GPU_BACKEND` through `xcrun simctl spawn`).
//!
//! With no arguments it hosts a long list on a `CAMetalLayer` in no window and scrolls it a
//! little each frame, then prints the time to the first frame, CPU and instructions per frame, and
//! memory footprint. With Rune's tooling arguments (`--snap out.png`) it renders the same view
//! headless instead, for comparing the backends' pixels.
//!
//! cargo build --profile perf -p rune --example embed_cost --features metal-backend --target aarch64-apple-ios-sim

use rune::{AppSpec, Color, Cx, El, ScrollSpec, TextStyle, Tx, col, oklch, row, text};

const SECTIONS: usize = 60;
const ROWS: usize = 6;
const SPEED: f64 = 600.0;

fn view(_: &(), _: &mut Cx<()>) -> El {
    let mut list = col().key("list").w_full().px(12.0).gap(8.0);
    for i in 0..SECTIONS {
        let mut section = col().key(format!("sec-{i}")).w_full().radius(12.0).fill(oklch(0.2, 0.015, 265.0))
            .border(1.0, oklch(0.32, 0.02, 265.0)).shadow(4.0, 12.0, Color::BLACK.alpha(0.5)).child(
                row().w_full().h(32.0).px(12.0).center_y().gradient(oklch(0.3, 0.06, 265.0), oklch(0.24, 0.03, 265.0))
                    .child(text(format!("Section {i:02} · {ROWS} rows")).font(TextStyle::new(13.0, 600.0))),
            );
        for r in 0..ROWS {
            section = section.child(row().w_full().h(28.0).px(12.0).center_y()
                .child(text(format!("row {r}: the quick brown fox jumps over the lazy dog {i}")).font(TextStyle::new(12.0, 400.0))));
        }
        list = list.child(section);
    }
    col().w_full().h_full().fill(Color::BLACK).child(col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(list))
}

/// Scrolls down and back up the list, a little each frame.
fn poll(_: &mut (), t: f64, tx: &mut Tx) -> bool {
    let range = (SECTIONS * (32 + 8 + ROWS * 28)) as f64 - 800.0;
    let at = (t * SPEED) % (2.0 * range);
    tx.scroll_to_y("scroll", (if at > range { 2.0 * range - at } else { at }) as f32);
    false
}

fn spec() -> AppSpec<()> {
    AppSpec { poll: Some(poll), ..AppSpec::new("Embed cost", (430.0, 932.0), || (), view) }
}

fn main() {
    if std::env::args().len() > 1 {
        return rune::App::new(spec()).run();
    }
    #[cfg(target_vendor = "apple")]
    embedded::run();
}

#[cfg(target_vendor = "apple")]
mod embedded {
    use std::ffi::{c_char, c_void};
    use std::sync::Arc;
    use std::time::Instant;

    use rune::embed::LayerHost;

    type Id = *mut c_void;

    #[link(name = "QuartzCore", kind = "framework")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Id;
        fn objc_msgSend();
        // libc declares it for macOS only; iOS's libSystem has it too.
        fn proc_pid_rusage(pid: libc::c_int, flavor: libc::c_int, buffer: *mut c_void) -> libc::c_int;
    }

    fn usage() -> libc::rusage_info_v4 {
        // SAFETY: proc_pid_rusage fills the rusage_info_v4 it is handed for RUSAGE_INFO_V4.
        unsafe {
            let mut info: libc::rusage_info_v4 = std::mem::zeroed();
            proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, (&raw mut info).cast());
            info
        }
    }

    fn process_cpu_us() -> f64 {
        // SAFETY: a zeroed rusage is valid, and getrusage only writes it.
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        let us = |t: libc::timeval| t.tv_sec as f64 * 1e6 + t.tv_usec as f64;
        us(u.ru_utime) + us(u.ru_stime)
    }

    fn thread_cpu_us() -> f64 {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a live, writable timespec; the clock needs no handle.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        t.tv_sec as f64 * 1e6 + t.tv_nsec as f64 / 1e3
    }

    /// A new `CAMetalLayer`, in no window.
    fn layer() -> Id {
        // SAFETY: `+[CAMetalLayer new]` takes no arguments and returns a retained layer.
        unsafe {
            let new: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            new(objc_getClass(c"CAMetalLayer".as_ptr()), sel_registerName(c"new".as_ptr()))
        }
    }

    pub fn run() {
        let began = Instant::now();
        let warmup: usize = std::env::var("EMBED_WARMUP").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
        let frames: usize = std::env::var("EMBED_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
        // SAFETY: a live CAMetalLayer that outlives the host, used only on this thread.
        let mut host = unsafe { LayerHost::from_core_animation_layer(layer(), &super::spec(), (), (430.0, 932.0), 3.0, true, Arc::new(|| {})) }
            .unwrap_or_else(|e| panic!("embed_cost: {e}"));
        let host_ms = began.elapsed().as_secs_f64() * 1e3;
        while !host.frame().drew {}
        let first_ms = began.elapsed().as_secs_f64() * 1e3;
        let first_footprint = usage().ri_phys_footprint;
        for _ in 0..warmup {
            host.frame();
        }
        let (cpu, process, before, wall) = (thread_cpu_us(), process_cpu_us(), usage(), Instant::now());
        let mut drew = 0;
        for _ in 0..frames {
            drew += usize::from(host.frame().drew);
        }
        let (cpu, process, after, wall) = (thread_cpu_us() - cpu, process_cpu_us() - process, usage(), wall.elapsed().as_secs_f64());
        let n = frames as f64;
        println!("backend={} host_ms={host_ms:.1} first_frame_ms={first_ms:.1} frames={frames} drew={drew} fps={:.1}",
            std::env::var("RUNE_GPU_BACKEND").unwrap_or_else(|_| "wgpu".into()), n / wall);
        println!("per_frame: main_cpu_us={:.1} process_cpu_us={:.1} kinstructions={:.1}", cpu / n, process / n,
            (after.ri_instructions - before.ri_instructions) as f64 / n / 1e3);
        println!("footprint_mb: first_frame={:.1} steady={:.1} peak={:.1}", first_footprint as f64 / 1048576.0,
            after.ri_phys_footprint as f64 / 1048576.0, after.ri_lifetime_max_phys_footprint as f64 / 1048576.0);
    }
}
