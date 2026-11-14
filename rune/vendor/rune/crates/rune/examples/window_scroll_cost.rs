//! A real window scrolling a long list of text rows at 600 points a second, for the per-frame
//! CPU of drawing, submitting and presenting a window frame: the main thread's, and the whole
//! process's (main plus driver threads). The scroll moves each frame from `poll` without
//! rebuilding the view, as a wheel scroll does. Runs `SCROLL_SECONDS` (default 12) after a
//! 2 s warmup, then prints frames, CPU and instructions per frame, and the Metal calls a frame
//! makes (command buffers committed, render and blit encoders, presents), on macOS.
//!
//! `SCROLL_PRESUME_VISIBLE=1` makes the window report itself on screen, so a Mac whose screen is
//! locked still draws: only for profiling. Compare backends with `RUNE_GPU_BACKEND=metal` on a
//! `metal-backend` build:
//!
//! cargo build --profile perf -p rune --example window_scroll_cost --features metal-backend

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[path = "hud_probe/mod.rs"]
mod probe;
use probe::{process_cpu_ms, thread_cpu_ms};
use rune::{App, AppSpec, Color, Cx, El, ScrollSpec, TextStyle, Tx, col, oklch, row, text};

const SECTIONS: usize = 60;
const ROWS: usize = 6;
const SPEED: f64 = 600.0;
const WARMUP: f64 = 2.0;

fn view(_: &(), _: &mut Cx<()>) -> El {
    let mut list = col().key("list").w_full();
    for i in 0..SECTIONS {
        let mut section = col().key(format!("sec-{i}")).w_full().pb(8.0).child(
            row().w_full().h(32.0).px(12.0).center_y().fill(oklch(0.23, 0.018, 265.0))
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
    let range = (SECTIONS * (32 + 8 + ROWS * 28)) as f64 - 600.0;
    let at = (t * SPEED) % (2.0 * range);
    tx.scroll_to_y("scroll", (if at > range { 2.0 * range - at } else { at }) as f32);
    tx.wake_after(Duration::ZERO);
    false
}

/// Process instructions, main-thread instructions, and Metal calls, so far.
fn counts() -> ([u64; 3], [u64; 2], [u64; 5]) {
    (probe::process_counts(), probe::thread_counts(), probe::metal_counts())
}

struct Start { main: f64, process: f64, wall: Instant, frames: u64, counts: ([u64; 3], [u64; 2], [u64; 5]) }

fn main() {
    if std::env::var_os("SCROLL_PRESUME_VISIBLE").is_some() { probe::presume_visible(); }
    probe::count_metal();
    let seconds: f64 = std::env::var("SCROLL_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(12.0);
    let began = Instant::now();
    let frames = Rc::new(RefCell::new((0u64, None::<Start>)));
    let app = App::new(AppSpec { poll: Some(poll), ..AppSpec::new("Window scroll cost", (800.0, 600.0), || (), view) })
        .on_frame_drawn(move |_| {
            let (count, from) = &mut *frames.borrow_mut();
            *count += 1;
            let elapsed = began.elapsed().as_secs_f64();
            if from.is_none() && elapsed >= WARMUP {
                *from = Some(Start { main: thread_cpu_ms(), process: process_cpu_ms(), wall: Instant::now(), frames: *count, counts: counts() });
            }
            let Some(s) = from.as_ref().filter(|_| elapsed >= WARMUP + seconds) else { return };
            let n = (*count - s.frames) as f64;
            let wall = s.wall.elapsed().as_secs_f64();
            let (process, main, metal) = counts();
            println!("frames={n} fps={:.1} main_us_per_frame={:.1} process_us_per_frame={:.1}",
                n / wall, (thread_cpu_ms() - s.main) * 1e3 / n, (process_cpu_ms() - s.process) * 1e3 / n);
            println!("per_frame: kinstructions={:.1} main_kinstructions={:.1} wakeups={:.2}",
                (process[0] - s.counts.0[0]) as f64 / n / 1e3, (main[0] - s.counts.1[0]) as f64 / n / 1e3, (process[2] - s.counts.0[2]) as f64 / n);
            let per = |i: usize| (metal[i] - s.counts.2[i]) as f64 / n;
            println!("per_frame: command_buffers={:.3} render_encoders={:.3} blit_encoders={:.3} presents={:.3} completed_handlers={:.3}",
                per(0), per(1), per(2), per(3), per(4));
            std::process::exit(0);
        });
    let handle = app.handle();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(WARMUP + seconds + 20.0));
        eprintln!("window_scroll_cost: no frames came; quitting");
        handle.quit();
    });
    app.run();
}
