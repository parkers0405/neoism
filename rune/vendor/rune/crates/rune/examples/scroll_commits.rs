//! Scroll commits bench: 120 frames of wheel scrolling a 300-row list, for what a pure scroll
//! costs when the list holds a playing video or a view reads a row's layout. Headless, no
//! window, no GPU: it times the runtime's frame, not uploading or presenting it.
//!
//! - `video`: a looping, playing clip near the top of the list, scrolled out past the clip
//!   edge and back in.
//! - `layout`: the view reads row 20 with `Cx::layout`, and a memo reads row 10 with
//!   `memo::layout`; the list scrolls down 600 points and back.
//!
//! cargo build --profile perf -p rune --example scroll_commits --features frame-profile
//! target/perf/examples/scroll_commits [--samples 9]
//!
//! Prints `METRIC scenario=.. metric=.. median=.. p95=..` over samples: `commits` in the 120
//! frames, `cpu_p50_ms` and `cpu_mean_ms` (thread CPU per frame), and `allocations` per frame.
use rune::{Color, Cx, El, Env, Player, Runtime, ScrollSpec, SurfaceFit, TextStyle, col, el, memo, row, text};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

const SIZE: (f32, f32) = (800.0, 600.0);
const ROWS: usize = 300;
const FRAMES: usize = 120;

struct List {
    video: Option<Player>,
}

fn rows() -> El {
    (0..ROWS).fold(col().w_full(), |c, i| {
        c.child(row().key(format!("r{i}")).w_full().h(28.0).px(12.0).center_y().fill(Color::WHITE.alpha(0.04))
            .child(text(format!("row {i}: the quick brown fox jumps over the lazy dog")).font(TextStyle::new(12.0, 400.0))))
    })
}

fn view(s: &List, cx: &mut Cx<List>) -> El {
    let mut content = col().w_full();
    if let Some(v) = &s.video {
        content = content.child(el().h(100.0)).child(el().key("clip").size(320.0, 180.0).video(v, SurfaceFit::Fit));
    }
    let list = col().key("list").w_full().h(SIZE.1 - 40.0).scroll(ScrollSpec { stick_bottom: false }).child(content.child(rows()));
    let mut root = col().w_full().h_full().child(list);
    if s.video.is_none() {
        let r20 = cx.layout("r20").map_or(-1.0, |r| r[1]);
        root = root.child(
            row().w_full().h(40.0)
                .child(text(format!("r20 at {r20}")).font(TextStyle::new(12.0, 400.0)))
                .child(memo::memo("r10-label", 0, || text(format!("r10 {:?}", memo::layout("r10"))).font(TextStyle::new(12.0, 400.0)))),
        );
    }
    root
}

const METRICS: &[&str] = &["commits", "cpu_p50_ms", "cpu_mean_ms", "allocations"];

fn sample(video: bool) -> Vec<f64> {
    let video = video.then(|| {
        let v = Player::file(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rgbw.mp4"));
        v.set_looping(true);
        v.play();
        v
    });
    let mut rt = Runtime::new(List { video }, view, SIZE, Env::new(true, 2.0));
    for _ in 0..240 { if !rt.frame(1.0 / 120.0) { break; } }
    let commits0 = rt.stats.commits;
    let mut cpu = Vec::with_capacity(FRAMES);
    let mut allocations = 0.0;
    for i in 0..FRAMES {
        // Down 600 points, then back up: the clip leaves through the top edge and returns.
        let dy = if i < FRAMES / 2 { -10.0 } else { 10.0 };
        let (alloc0, cpu0) = (procwatch::alloc::thread_allocations(), rune::thread_cpu_ms());
        rt.wheel(400.0, 300.0, dy);
        rt.frame(1.0 / 120.0);
        cpu.push(rune::thread_cpu_ms() - cpu0);
        allocations += (procwatch::alloc::thread_allocations() - alloc0) as f64;
    }
    let mean = cpu.iter().sum::<f64>() / FRAMES as f64;
    cpu.sort_by(f64::total_cmp);
    vec![(rt.stats.commits - commits0) as f64, cpu[FRAMES / 2], mean, allocations / FRAMES as f64]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let samples: usize = value("--samples").map_or(9, |s| s.parse().expect("--samples N"));
    if cfg!(debug_assertions) { eprintln!("scroll_commits: debug build; time a --profile perf build"); }
    let mut rows = [Vec::new(), Vec::new()];
    // One warm-up of each, then alternate so heat and caches favour neither scenario.
    for s in 0..=samples {
        for video in if s % 2 == 0 { [false, true] } else { [true, false] } {
            let row = sample(video);
            if s > 0 { rows[video as usize].push(row); }
        }
    }
    for (video, rows) in rows.iter().enumerate() {
        let scenario = if video == 1 { "video" } else { "layout" };
        for (m, name) in METRICS.iter().enumerate() {
            let mut v: Vec<f64> = rows.iter().map(|r| r[m]).collect();
            v.sort_by(f64::total_cmp);
            let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
            println!("METRIC scenario={scenario} metric={name} median={:.4} p95={p95:.4} n={}", v[v.len() / 2], v.len());
        }
    }
}
