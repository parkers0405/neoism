//! Per-frame sampling cost. `cargo run --profile perf --example bench`
//!
//! Every scenario keeps all tracks active for the whole measurement (long, bouncy springs that
//! can't settle in time) and reports the best of several runs (robust to a busy machine).

use std::hint::black_box;
use std::time::Instant;

use rune_motion::prelude::*;
use rune_motion::{Prop, PropClass};

const X: Prop<f32> = Prop::custom(64, PropClass::Spatial);
const FRAMES: usize = 120;
const RUNS: usize = 9;

fn best(v: Vec<f64>) -> f64 {
    v.into_iter().fold(f64::INFINITY, f64::min)
}

/// Builds a store, then times `FRAMES` ticks at 120 Hz. Returns ns per track per tick.
fn bench_store(label: &str, tracks: usize, setup: impl Fn(&mut AnimationStore, NodeId, usize)) -> f64 {
    let mut runs = Vec::new();
    for _ in 0..RUNS {
        let mut st = AnimationStore::with_capacity(tracks, 4);
        for i in 0..tracks {
            setup(&mut st, NodeId(i as u64), i);
        }
        st.tick_at(0.0);
        let t0 = Instant::now();
        for f in 1..=FRAMES {
            st.tick_at(f as f64 / 120.0);
            black_box(st.events());
        }
        let el = t0.elapsed().as_nanos() as f64;
        assert_eq!(st.active_len(), tracks, "{label}: every track must still be active");
        runs.push(el / (FRAMES * tracks) as f64);
    }
    let ns = best(runs);
    println!("{label:<44} {tracks:>7} tracks  {ns:>7.2} ns/track  {:>7.3} ms/frame", ns * tracks as f64 / 1e6);
    ns
}

fn main() {
    println!("rune-motion bench ({FRAMES} frames at 120 Hz, best of {RUNS})\n");
    for &n in &[10_000usize, 100_000] {
        bench_store("scalar springs, underdamped (bounce .3)", n, |st, id, i| {
            st.set(id, X, 0.0);
            st.animate(id, X, 100.0 + (i % 13) as f32, Spring::new(3.0, 0.3));
        });
        bench_store("scalar springs, critical (bounce 0)", n, |st, id, i| {
            st.set(id, X, 0.0);
            st.animate(id, X, 100.0 + (i % 13) as f32, Spring::new(3.0, 0.0));
        });
        bench_store("scalar tweens (ease-in-out bezier)", n, |st, id, i| {
            st.set(id, X, 0.0);
            st.animate(id, X, 100.0 + (i % 13) as f32, Curve::ease_in_out(5.0));
        });
        bench_store("rect frames (4 edge lanes, MORPH)", n, |st, id, i| {
            st.set(id, prop::FRAME, Rect::new(0.0, 0.0, 10.0, 10.0));
            let f = (i % 17) as f32;
            st.animate(id, prop::FRAME, Rect::new(100.0 + f, 50.0, 300.0, 200.0), Spring::new(3.0, 0.16));
        });
        bench_store("colors (4 OKLab lanes)", n, |st, id, _| {
            st.set(id, prop::FILL, Color::hex(0xff3b30));
            st.animate(id, prop::FILL, Color::hex(0x007aff), Spring::new(3.0, 0.0));
        });
        bench_store("mixed: 1/3 spring, 1/3 tween, 1/3 additive x2", n, |st, id, i| {
            st.set(id, X, 0.0);
            match i % 3 {
                0 => {
                    st.animate(id, X, 50.0, Spring::new(3.0, 0.2));
                }
                1 => {
                    st.animate(id, X, 50.0, Curve::ease_in_out(5.0));
                }
                _ => {
                    st.animate(id, X, 50.0, Spring::new(3.0, 0.0));
                    st.animate(id, X, 80.0, Spring::new(3.0, 0.0).additive());
                }
            }
        });
        println!();
    }

    // Scene: every node's frame animating, presented values out.
    for &n in &[1_000usize, 10_000] {
        let mut runs = Vec::new();
        for _ in 0..RUNS {
            let mut s = Scene::new();
            for i in 0..n {
                s.insert(NodeId(i as u64), NodeSpec::new(Rect::new(0.0, i as f32 * 20.0, 300.0, 20.0)), Mount::Initial);
            }
            s.frame_at(0.0);
            for i in 0..n {
                s.set_frame(NodeId(i as u64), Rect::new(10.0, i as f32 * 24.0, 300.0, 24.0), Spring::new(3.0, 0.0));
            }
            s.frame_at(1.0 / 120.0);
            let t0 = Instant::now();
            for f in 2..FRAMES + 2 {
                let out = s.frame_at(f as f64 / 120.0);
                black_box(out.changed.len());
            }
            runs.push(t0.elapsed().as_nanos() as f64 / (FRAMES * n) as f64);
        }
        let ns = best(runs);
        println!("{:<44} {n:>7} nodes   {ns:>7.2} ns/node   {:>7.3} ms/frame", "scene: layout reflow → Presented", ns * n as f64 / 1e6);
    }

    // Raw closed-form sample, no store.
    let s = Spring::new(0.5, 0.3);
    let iters = 2_000_000;
    let runs = (0..RUNS)
        .map(|_| {
            let t0 = Instant::now();
            let mut acc = 0.0;
            for i in 0..iters {
                let (x, v) = s.sample(0.0, 0.0, 1.0, black_box((i % 1000) as f32 * 1e-3));
                acc += x + v;
            }
            black_box(acc);
            t0.elapsed().as_nanos() as f64 / iters as f64
        })
        .collect();
    println!("\nSpring::sample (closed form, standalone)   {:>7.2} ns/call", best(runs));
}
