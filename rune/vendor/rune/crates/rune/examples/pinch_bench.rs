//! Pinch bench: a 120-frame pinch on a large design canvas, with and without `Tx::live_only`.
//! Headless, drawn offscreen, no window.
//!
//! The canvas holds `--cards N` cards (2,000 by default), each a frame with a title and a
//! subtitle, under one camera: a `live_offset` and `live_scale`. A toolbar floats over the
//! selected card, placed from its `on_drawn_layout` report. Each frame gets one pinch event,
//! which zooms about the pointer. `rebuild` is a canvas as apps build one today: the pinch
//! handler sets the camera's live values and the view re-runs anyway, and the toolbar's
//! report re-runs it again. `live` says `Tx::live_only` in both handlers, and the toolbar
//! follows by a live offset of its own.
//!
//! cargo build --profile perf -p rune --example pinch_bench --features frame-profile
//! target/perf/examples/pinch_bench [--samples 9] [--cards 2000]
//!
//! Prints `METRIC mode=.. metric=.. median=.. p95=..` over samples, each the mean of the 120
//! pinch frames: `commits` per frame, `cpu_ms` (thread CPU of the frame, then of preparing and
//! encoding its draw), `frame_ms` (the runtime's part), `encode_ms` (the draw's), `view_ms`
//! and `layout_ms` (phases of the commit), and `allocations`.
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Live, MagnifyEvent, Runtime, TextStyle, Tx, el, text};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

const SIZE: (f32, f32) = (1200.0, 800.0);
const COLUMNS: usize = 50;
const FRAMES: usize = 120;
const SELECTED: usize = 7;

struct Canvas {
    live: bool,
    cards: usize,
    zoom: f32,
    pan: [f32; 2],
    cam_pan: Live<[f32; 2]>,
    cam_zoom: Live<f32>,
    /// Where the toolbar shows: read by the view (`rebuild`) or set live (`live`).
    toolbar: [f32; 2],
    toolbar_live: Live<[f32; 2]>,
}

impl Canvas {
    /// Zoom by `1 + delta` about canvas point (x, y); the camera scales about the canvas centre.
    fn zoom_about(&mut self, x: f32, y: f32, delta: f32) {
        let (cx, cy) = (SIZE.0 / 2.0, SIZE.1 / 2.0);
        let z = (self.zoom * (1.0 + delta)).clamp(0.05, 8.0);
        let k = z / self.zoom;
        self.pan = [x - cx - (x - cx - self.pan[0]) * k, y - cy - (y - cy - self.pan[1]) * k];
        self.zoom = z;
        self.cam_pan.set(self.pan);
        self.cam_zoom.set(z);
    }
}

fn card(i: usize) -> El {
    let (x, y) = (24.0 + 220.0 * (i % COLUMNS) as f32, 24.0 + 140.0 * (i / COLUMNS) as f32);
    el().key(format!("card{i}")).pin(Some(x), Some(y), None, None).size(200.0, 120.0).radius(10.0)
        .fill(Color::WHITE.alpha(0.08)).padding(12.0)
        .child(text(format!("Frame {i}")).font(TextStyle::new(15.0, 600.0)))
        .child(text(format!("{} × {}", 200 + i % 7, 120 + i % 5)).font(TextStyle::new(12.0, 400.0)))
}

fn view(s: &Canvas, _: &mut Cx<Canvas>) -> El {
    let world = (0..s.cards).fold(el().key("world").size(SIZE.0, SIZE.1).live_offset(&s.cam_pan).live_scale(&s.cam_zoom), |w, i| {
        let c = card(i);
        w.child(if i != SELECTED { c } else {
            c.on_drawn_layout_tx(|s: &mut Canvas, r, tx| {
                let at = [r[0] + r[2] / 2.0 - 60.0, r[1] - 40.0];
                if s.live { s.toolbar_live.set(at); tx.live_only(); } else { s.toolbar = at; }
            })
        })
    });
    let at = if s.live { [0.0, 0.0] } else { s.toolbar };
    let toolbar = el().key("toolbar").pin(Some(at[0]), Some(at[1]), None, None).size(120.0, 32.0).radius(8.0).fill(Color::WHITE);
    let toolbar = if s.live { toolbar.live_offset(&s.toolbar_live) } else { toolbar };
    el().key("canvas").size(SIZE.0, SIZE.1).clip_soft(0.0).child(world).child(toolbar)
        .on_magnify(|s: &mut Canvas, e: MagnifyEvent, tx: &mut Tx| {
            s.zoom_about(e.x, e.y, e.delta);
            if s.live { tx.live_only(); }
            true
        })
}

struct Draw { gpu: Gpu, renderer: Renderer, out: Offscreen }

impl Draw {
    fn draw(&mut self, rt: &mut Runtime<Canvas>) {
        self.renderer.upload_images(&self.gpu.device, &self.gpu.queue, &mut rt.images);
        self.renderer.prepare(&self.gpu.device, &self.gpu.queue, &mut rt.text.atlas, &rt.instances, (self.out.width, self.out.height));
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.draw(&mut encoder, &self.out.view, rt.clear_color());
        self.renderer.submit(&self.gpu.queue, encoder);
    }
    fn wait(&self) { self.gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok(); }
}

const METRICS: &[&str] = &["commits", "cpu_ms", "frame_ms", "encode_ms", "view_ms", "layout_ms", "allocations", "live_only_events"];

fn sample(d: &mut Draw, live: bool, cards: usize) -> Vec<f64> {
    let state = Canvas {
        live, cards, zoom: 1.0, pan: [0.0, 0.0], cam_pan: Live::new([0.0, 0.0]), cam_zoom: Live::new(1.0),
        toolbar: [0.0, 0.0], toolbar_live: Live::new([0.0, 0.0]),
    };
    let mut rt = Runtime::new(state, view, SIZE, Env::new(true, 2.0));
    for _ in 0..240 { if !rt.frame(1.0 / 120.0) { break; } }
    d.draw(&mut rt);
    d.wait();
    rune::take_frame_phases();
    let (commits0, live0) = (rt.stats.commits, rt.live_only_events());
    let (mut cpu, mut frame, mut encode, mut allocations) = (0.0, 0.0, 0.0, 0.0);
    for i in 0..FRAMES {
        // In, then out: a pinch's deltas about a point off the canvas centre.
        let delta = if i < FRAMES / 2 { 0.01 } else { -0.01 };
        let (alloc0, cpu0) = (procwatch::alloc::thread_allocations(), rune::thread_cpu_ms());
        rt.magnify_event(MagnifyEvent::new(delta).at(400.0, 300.0));
        rt.frame(1.0 / 120.0);
        let cpu1 = rune::thread_cpu_ms();
        d.draw(&mut rt);
        let cpu2 = rune::thread_cpu_ms();
        allocations += (procwatch::alloc::thread_allocations() - alloc0) as f64;
        d.wait();
        frame += cpu1 - cpu0;
        encode += cpu2 - cpu1;
        cpu += cpu2 - cpu0;
    }
    let phases = rune::take_frame_phases();
    let p = rune::PHASES;
    let n = FRAMES as f64;
    let commits = (rt.stats.commits - commits0) as f64;
    vec![commits / n, cpu / n, frame / n, encode / n, phases[p] / n, phases[p + 2] / n, allocations / n,
        (rt.live_only_events() - live0) as f64 / n]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let samples: usize = value("--samples").map_or(9, |s| s.parse().expect("--samples N"));
    let cards: usize = value("--cards").map_or(2000, |s| s.parse().expect("--cards N"));
    if cfg!(debug_assertions) { eprintln!("pinch_bench: debug build; time a --profile perf build"); }
    let gpu = Gpu::headless();
    let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let out = Offscreen::new(&gpu.device, (SIZE.0 * 2.0) as u32, (SIZE.1 * 2.0) as u32);
    let mut d = Draw { gpu, renderer, out };
    let mut rows = [Vec::new(), Vec::new()];
    // One warm-up of each, then alternate so heat and caches favour neither mode.
    for s in 0..=samples {
        for live in if s % 2 == 0 { [false, true] } else { [true, false] } {
            let row = sample(&mut d, live, cards);
            if s > 0 { rows[live as usize].push(row); }
        }
    }
    for (live, rows) in rows.iter().enumerate() {
        let mode = if live == 1 { "live" } else { "rebuild" };
        for (m, name) in METRICS.iter().enumerate() {
            let mut v: Vec<f64> = rows.iter().map(|r| r[m]).collect();
            v.sort_by(f64::total_cmp);
            let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
            println!("METRIC mode={mode} cards={cards} metric={name} median={:.4} p95={p95:.4} n={}", v[v.len() / 2], v.len());
        }
    }
}
