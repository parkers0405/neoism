//! Headless benchmark for `El::cache()`: 25 text-heavy cards under a live pan and zoom.
//!
//! cargo build --profile perf -p rune --example cache_pan_zoom --features frame-profile
//! target/perf/examples/cache_pan_zoom [--samples 15] [--cards 25] [--frames 60]
//!
//! Prints `METRIC mode=.. case=.. metric=.. median=.. p95=.. n=..` per measurement.
use std::time::Instant;

use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{Color, Cx, El, Env, Live, Runtime, TextStyle, col, el, row, text};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

const SIZE: (f32, f32) = (1280.0, 820.0);
const COLS: usize = 5;

struct CardData {
    key: String,
    title: String,
    badge: String,
    version: String,
    line1: String,
    line2: String,
    line3: String,
    footer: String,
}

struct State {
    cached: bool,
    pan: Live<[f32; 2]>,
    zoom: Live<f32>,
    cards: Vec<CardData>,
}

impl State {
    fn new(cached: bool, count: usize) -> Self {
        let cards = (0..count)
            .map(|i| CardData {
                key: format!("card-{i:02}"),
                title: format!("Subsystem Pipeline #{i:02}"),
                badge: if i % 2 == 0 { "READY".into() } else { "SYNCED".into() },
                version: format!("v2.{}.{}", i % 9, (i * 3) % 14),
                line1: format!("Offscreen tile cache node {i:02} rasterizes glyphs and borders once."),
                line2: format!("Ancestor live_offset and live_scale composite without reshaping ({i} shards)."),
                line3: format!("Latency p50 {:.2}ms · p99 {:.2}ms · throughput {}k ops/s", 0.42 + (i as f32) * 0.03, 1.15 + (i as f32) * 0.05, 120 + i * 7),
                footer: format!("region-us-east-{} · replica {} · 120 Hz", (i % 4) + 1, (i % 3) + 1),
            })
            .collect();
        State {
            cached,
            pan: Live::new([0.0, 0.0]),
            zoom: Live::new(1.0),
            cards,
        }
    }
}

const BG: Color = Color { r: 0.012, g: 0.014, b: 0.018, a: 1.0 };
const CARD_BG: Color = Color { r: 0.032, g: 0.036, b: 0.046, a: 1.0 };
const BORDER: Color = Color { r: 0.12, g: 0.14, b: 0.18, a: 1.0 };
const BADGE_BG: Color = Color { r: 0.08, g: 0.22, b: 0.16, a: 1.0 };
const BADGE_FG: Color = Color { r: 0.35, g: 0.88, b: 0.62, a: 1.0 };
const TEXT: Color = Color { r: 0.88, g: 0.90, b: 0.94, a: 1.0 };
const BODY: Color = Color { r: 0.58, g: 0.62, b: 0.68, a: 1.0 };
const MUTED: Color = Color { r: 0.36, g: 0.40, b: 0.46, a: 1.0 };
const ACCENT: Color = Color { r: 0.24, g: 0.52, b: 0.96, a: 1.0 };

fn card_view(c: &CardData, cached: bool) -> El {
    let mut node = col()
        .key(c.key.clone())
        .size(228.0, 136.0)
        .padding(10.0)
        .gap(5.0)
        .radius(8.0)
        .fill(CARD_BG)
        .border(1.0, BORDER)
        .child(
            row()
                .w_full()
                .gap(6.0)
                .center_y()
                .child(
                    el().px(6.0)
                        .py(2.0)
                        .radius(4.0)
                        .fill(BADGE_BG)
                        .child(text(c.badge.clone()).font(TextStyle::new(10.0, 600.0)).color(BADGE_FG)),
                )
                .child(text(c.title.clone()).font(TextStyle::new(13.0, 600.0)).color(TEXT).grow(1.0))
                .child(text(c.version.clone()).font(TextStyle::new(11.0, 500.0)).color(MUTED)),
        )
        .child(text(c.line1.clone()).font(TextStyle::new(11.0, 400.0)).color(BODY))
        .child(text(c.line2.clone()).font(TextStyle::new(11.0, 400.0)).color(BODY))
        .child(text(c.line3.clone()).font(TextStyle::new(11.0, 400.0)).color(BODY))
        .child(
            row()
                .w_full()
                .gap(6.0)
                .center_y()
                .child(el().size(8.0, 8.0).radius(4.0).fill(ACCENT))
                .child(text(c.footer.clone()).font(TextStyle::new(10.0, 500.0)).color(MUTED).grow(1.0))
                .child(el().size(36.0, 4.0).radius(2.0).fill(ACCENT)),
        );
    if cached {
        node = node.cache();
    }
    node
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut grid = col().gap(10.0);
    for (row_idx, chunk) in s.cards.chunks(COLS).enumerate() {
        let mut line = row().key(format!("row-{row_idx}")).gap(10.0);
        for c in chunk {
            line = line.child(card_view(c, s.cached));
        }
        grid = grid.child(line);
    }
    el().w_full()
        .h_full()
        .fill(BG)
        .padding(32.0)
        .child(
            el().key("canvas")
                .size(1200.0, 740.0)
                .live_offset(&s.pan)
                .live_scale(&s.zoom)
                .child(grid),
        )
}

const METRICS: &[&str] = &[
    "cpu_ms",
    "paint_ms",
    "encode_ms",
    "instances",
    "extra_passes",
    "targets",
    "cache_kb",
    "allocations",
];

fn run_sample(
    gpu: &Gpu,
    out: &Offscreen,
    cached: bool,
    case: &str,
    cards: usize,
    frames: usize,
) -> Vec<f64> {
    let mut text = TextSystem::default();
    for sz in [10.0, 11.0, 13.0] {
        text.layout_rich("Warm benchmark fonts 0123456789", &[], TextStyle::new(sz, 400.0), 2.0, None);
    }
    let state = State::new(cached, cards);
    let pan = state.pan.clone();
    let zoom = state.zoom.clone();
    let mut rt = Runtime::with_text(state, view, SIZE, Env::new(true, 2.0), text);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);

    // Frame 0: initial build and rasterize.
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (out.width, out.height));
    {
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &out.view, rt.clear_color());
        renderer.submit(&gpu.queue, enc);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    }
    // Settle any mount animations.
    for _ in 0..60 {
        if !rt.frame(1.0 / 120.0) {
            break;
        }
    }
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (out.width, out.height));
    {
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &out.view, rt.clear_color());
        renderer.submit(&gpu.queue, enc);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    }

    let mut total_cpu = 0.0;
    let mut total_paint = 0.0;
    let mut total_encode = 0.0;
    let mut total_instances = 0.0;
    let mut total_extra_passes = 0.0;
    let mut total_allocs = 0.0;

    for f in 0..frames {
        let t = (f + 1) as f32 / frames as f32;
        if case == "pan" {
            let angle = t * std::f32::consts::TAU;
            pan.set([angle.cos() * 24.0, angle.sin() * 18.0]);
        } else {
            let angle = t * std::f32::consts::TAU;
            pan.set([angle.cos() * 12.0, angle.sin() * 10.0]);
            zoom.set(1.0 + 0.25 * t);
        }
        rune::take_frame_phases();
        let alloc0 = procwatch::alloc::thread_allocations();
        let cpu0 = rune::thread_cpu_ms();
        rt.frame(1.0 / 120.0);
        let cpu = rune::thread_cpu_ms() - cpu0;
        let allocs = (procwatch::alloc::thread_allocations() - alloc0) as f64;
        let phases = rune::take_frame_phases();
        let paint = phases[rune::PHASES + 4];

        let enc0 = Instant::now();
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (out.width, out.height));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &out.view, rt.clear_color());
        renderer.submit(&gpu.queue, enc);
        let encode = enc0.elapsed().as_secs_f64() * 1000.0;
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();

        let stats = renderer.layer_stats();
        total_cpu += cpu;
        total_paint += paint;
        total_encode += encode;
        total_instances += rt.instances.len() as f64;
        total_extra_passes += stats.extra_passes as f64;
        total_allocs += allocs;
    }

    let n = frames as f64;
    let stats = renderer.layer_stats();
    vec![
        total_cpu / n,
        total_paint / n,
        total_encode / n,
        total_instances / n,
        total_extra_passes / n,
        stats.targets as f64,
        stats.bytes as f64 / 1024.0,
        total_allocs / n,
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let samples: usize = value("--samples").map_or(15, |s| s.parse().expect("--samples N"));
    let cards: usize = value("--cards").map_or(25, |s| s.parse().expect("--cards N"));
    let frames: usize = value("--frames").map_or(60, |s| s.parse().expect("--frames N"));
    if cfg!(debug_assertions) {
        eprintln!("cache_pan_zoom: debug build; time a --profile perf build");
    }
    let gpu = Gpu::headless();
    let out = Offscreen::new(&gpu.device, (SIZE.0 * 2.0) as u32, (SIZE.1 * 2.0) as u32);
    for (mode, cached) in [("uncached", false), ("cached", true)] {
        for case in ["pan", "pan_zoom"] {
            let rows: Vec<Vec<f64>> = (0..=samples)
                .map(|_| run_sample(&gpu, &out, cached, case, cards, frames))
                .skip(1)
                .collect();
            for (m, name) in METRICS.iter().enumerate() {
                let mut v: Vec<f64> = rows.iter().map(|r| r[m]).collect();
                v.sort_by(f64::total_cmp);
                let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
                println!(
                    "METRIC mode={mode} case={case} metric={name} median={:.4} p95={p95:.4} n={}",
                    v[v.len() / 2],
                    v.len()
                );
            }
        }
    }
}
