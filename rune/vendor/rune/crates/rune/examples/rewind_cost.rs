//! Rewind cost bench: what recording a window's frames (`Tx::record_rewind`) adds to a frame,
//! and what a minute of it holds. Headless, no window, no GPU. A chat-like page: a 400-row
//! transcript of keyed rows, each with keyed children, that streams a row in and grows the last
//! one every frame, scrolls, and opens and closes a fold.
//!
//! cargo build --profile perf -p rune --example rewind_cost
//! target/perf/examples/rewind_cost [--samples 9]
//!
//! Each sample runs every scenario with the recorder off and on, alternated. Prints
//! `METRIC scenario=.. rewind=off|on metric=.. median=..` over samples: `p50_ms` and `p99_ms`
//! (wall time per frame, input included) and `allocations` per frame, then the bytes a recorder
//! holds after 60 s of streaming at 60 Hz.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use rune::rewind::Rewind;
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

const SIZE: (f32, f32) = (1000.0, 800.0);
const ROWS: usize = 400;
const FRAMES: usize = 240;

struct Page {
    rows: usize,
    tail: usize,
    open: bool,
    rewind: Rewind,
}

fn view(s: &Page, _: &mut Cx<Page>) -> El {
    let style = TextStyle::new(13.0, 400.0);
    let rows = (0..s.rows).fold(col().w_full().gap(6.0), |c, i| {
        let body = if i + 1 == s.rows { "streaming words ".repeat(1 + s.tail / 4) } else { format!("message {i}: the quick brown fox jumps over the lazy dog") };
        c.child(col().key(format!("m{i}")).w_full().px(16.0).py(8.0).gap(4.0).fill(Color::WHITE.alpha(0.04))
            .child(row().key(format!("m{i}-head")).gap(8.0)
                .child(el().key(format!("m{i}-face")).size(18.0, 18.0).radius(9.0).fill(Color::WHITE.alpha(0.2)))
                .child(text(format!("agent {}", i % 7)).font(style)))
            .child(text(body).key(format!("m{i}-body")).font(style)))
    });
    let fold = col().key("fold").w_full().h(if s.open { 240.0 } else { 0.0 }).clip_rounded(0.0)
        .children((0..12).map(|i| row().key(format!("fold-{i}")).h(20.0).child(text(format!("tool output line {i}")).font(style))));
    col().key("page").w_full().h_full()
        .child(row().key("header").w_full().h(40.0).px(16.0).center_y().child(text("Session").font(style)))
        .child(fold)
        .child(col().key("transcript").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: true }).child(rows))
}

const SCENARIOS: &[&str] = &["stream", "scroll", "fold", "idle"];

struct Run {
    times: Vec<f64>,
    allocs: u64,
}

fn run(scenario: usize, record: bool) -> Run {
    let page = Page { rows: ROWS, tail: 0, open: false, rewind: Rewind::new() };
    let mut rt = Runtime::new(page, view, SIZE, Env::new(true, 2.0));
    if record { rt.update(|s, tx| tx.record_rewind(Some(&s.rewind))); }
    for _ in 0..240 { if !rt.frame(1.0 / 120.0) { break; } }
    let mut times = Vec::with_capacity(FRAMES);
    let mut allocs = 0;
    for i in 0..FRAMES {
        let (a0, t0) = (allocations(), Instant::now());
        match scenario {
            0 => rt.update(|s, _| { s.tail += 1; if s.tail % 8 == 0 { s.rows += 1; } }),
            1 => rt.wheel(500.0, 400.0, if (i / 40) % 2 == 0 { -24.0 } else { 24.0 }),
            2 => if i % 30 == 0 { rt.update(|s, _| s.open = !s.open) },
            _ => {}
        }
        rt.frame(1.0 / 120.0);
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
        allocs += allocations() - a0;
    }
    assert_eq!(rt.state.rewind.frames() > 0, record);
    Run { times, allocs: allocs / FRAMES as u64 }
}

fn pct(v: &[f64], p: f64) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Bytes held after a minute of streaming with the recorder on, and the frames held.
fn minute() -> (usize, usize) {
    let page = Page { rows: ROWS, tail: 0, open: false, rewind: Rewind::new() };
    let mut rt = Runtime::new(page, view, SIZE, Env::new(true, 2.0));
    rt.update(|s, tx| tx.record_rewind(Some(&s.rewind)));
    for i in 0..3600 {
        rt.update(|s, _| { s.tail += 1; if s.tail % 8 == 0 { s.rows += 1; } if i % 300 == 0 { s.open = !s.open; } });
        if i % 600 < 60 { rt.wheel(500.0, 400.0, -24.0); }
        rt.frame(1.0 / 60.0);
    }
    (rt.state.rewind.bytes(), rt.state.rewind.frames())
}

fn main() {
    let samples = std::env::args().skip_while(|a| a != "--samples").nth(1).and_then(|n| n.parse().ok()).unwrap_or(5);
    for (k, name) in SCENARIOS.iter().enumerate() {
        let mut stats: [[Vec<f64>; 3]; 2] = Default::default();
        for n in 0..samples {
            // Alternate which goes first so warm caches favour neither.
            for record in if n % 2 == 0 { [false, true] } else { [true, false] } {
                let r = run(k, record);
                let s = &mut stats[record as usize];
                s[0].push(pct(&r.times, 0.5));
                s[1].push(pct(&r.times, 0.99));
                s[2].push(r.allocs as f64);
            }
        }
        for (record, s) in stats.iter().enumerate() {
            for (m, metric) in ["p50_ms", "p99_ms", "allocations"].iter().enumerate() {
                println!("METRIC scenario={name} rewind={} metric={metric} median={:.4}", ["off", "on"][record], pct(&s[m], 0.5));
            }
        }
    }
    let (bytes, frames) = minute();
    println!("METRIC scenario=minute rewind=on metric=bytes median={bytes} frames={frames}");
}
