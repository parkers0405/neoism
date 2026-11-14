//! Interaction timing cost bench: what `RUNE_PERF` timing adds to a frame. Headless, no window,
//! no GPU. A settings page with a 300-row list takes 120 frames each of presses (a commit per
//! frame), wheel scrolling, and idle frames with no input.
//!
//! cargo build --profile perf -p rune --example interaction_cost
//! RUNE_PERF=0 target/perf/examples/interaction_cost [--samples 9]
//! RUNE_PERF=1 target/perf/examples/interaction_cost [--samples 9]
//!
//! The gate is read once per process, so compare runs with it off and on, alternated. Prints
//! `METRIC scenario=.. metric=.. median=.. p95=..` over samples: `p50_ms` and `mean_ms`
//! (wall time per frame, input included) and `allocations` per frame.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, TextStyle, col, el, row, text};

struct Counting;

thread_local! { static ALLOCATIONS: Cell<u64> = const { Cell::new(0) }; }

// SAFETY: forwards every call to `System` unchanged; only counts allocations on this thread.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `System` through `alloc` above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn allocations() -> u64 { ALLOCATIONS.with(Cell::get) }

const SIZE: (f32, f32) = (800.0, 600.0);
const ROWS: usize = 300;
const FRAMES: usize = 120;

#[derive(Default)]
struct Page {
    saves: u32,
}

fn view(s: &Page, _: &mut Cx<Page>) -> El {
    let rows = (0..ROWS).fold(col().w_full(), |c, i| {
        c.child(row().key(format!("r{i}")).w_full().h(28.0).px(12.0).center_y().fill(Color::WHITE.alpha(0.04))
            .child(text(format!("row {i}: the quick brown fox jumps over the lazy dog")).font(TextStyle::new(12.0, 400.0))))
    });
    col().key("settings").w_full().h_full()
        .child(row().w_full().h(40.0)
            .child(el().key("save").size(80.0, 30.0).on_press(|s: &mut Page| s.saves += 1))
            .child(text(format!("saved {}", s.saves)).font(TextStyle::new(12.0, 400.0))))
        .child(col().key("list").w_full().h(SIZE.1 - 40.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
}

const SCENARIOS: &[&str] = &["press", "wheel", "idle"];
const METRICS: &[&str] = &["p50_ms", "mean_ms", "allocations"];

fn sample(scenario: usize) -> Vec<f64> {
    let mut rt = Runtime::new(Page::default(), view, SIZE, Env::new(true, 2.0));
    for _ in 0..240 { if !rt.frame(1.0 / 120.0) { break; } }
    let (x, y) = rt.find("save").expect("save button");
    let mut times = Vec::with_capacity(FRAMES);
    let mut allocs = 0;
    for i in 0..FRAMES {
        let (a0, t0) = (allocations(), Instant::now());
        match scenario {
            0 => {
                rt.pointer_down(x, y);
                rt.pointer_up(x, y);
            }
            1 => rt.wheel(400.0, 300.0, if i < FRAMES / 2 { -10.0 } else { 10.0 }),
            _ => {}
        }
        rt.frame(1.0 / 120.0);
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
        allocs += allocations() - a0;
    }
    let mean = times.iter().sum::<f64>() / FRAMES as f64;
    times.sort_by(f64::total_cmp);
    vec![times[FRAMES / 2], mean, allocs as f64 / FRAMES as f64]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let samples: usize = value("--samples").map_or(9, |s| s.parse().expect("--samples N"));
    if cfg!(debug_assertions) { eprintln!("interaction_cost: debug build; time a --profile perf build"); }
    let mut rows = vec![Vec::new(); SCENARIOS.len()];
    // One warm-up round, then rotate the order so heat and caches favour no scenario.
    for s in 0..=samples {
        for k in 0..SCENARIOS.len() {
            let scenario = (s + k) % SCENARIOS.len();
            let row = sample(scenario);
            if s > 0 { rows[scenario].push(row); }
        }
    }
    let perf = std::env::var("RUNE_PERF").unwrap_or_default();
    for (scenario, rows) in rows.iter().enumerate() {
        for (m, name) in METRICS.iter().enumerate() {
            let mut v: Vec<f64> = rows.iter().map(|r| r[m]).collect();
            v.sort_by(f64::total_cmp);
            let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
            println!("METRIC perf={perf} scenario={} metric={name} median={:.4} p95={p95:.4} n={}", SCENARIOS[scenario], v[v.len() / 2], v.len());
        }
    }
}
