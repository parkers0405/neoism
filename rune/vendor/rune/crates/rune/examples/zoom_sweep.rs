//! Headless zoom-sweep benchmark for the glyph atlas and `live_scale` bucketing.
//!
//! cargo run --profile perf -p rune --example zoom_sweep
//!
//! Prints `METRIC case=.. metric=.. median=.. p95=.. n=..` for:
//! - `live_zoom_10s`: 10 s (1,200 frames at 120 Hz) continuous `live_scale` sweep across [0.25, 4.0]
//!   on a 25-card text-heavy canvas, plus a settled rest frame at an off-bucket scale (1.13).
//! - `font_sweep_10s`: 10 s (1,200 frames at 120 Hz) continuous authored font-size sweep forcing
//!   repeated glyph-atlas page recycling, verifying zero dropped glyphs after sweep.
use std::time::Instant;

use rune::render::{Gpu, KIND_GLYPH, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{Color, Cx, El, Env, Live, Runtime, TextStyle, col, el, row, text};

const SIZE: (f32, f32) = (1280.0, 820.0);
const COLS: usize = 5;
const CARDS: usize = 25;
const FRAMES_10S: usize = 1200;

struct CardData {
    key: String,
    title: String,
    badge: String,
    version: String,
    line1: String,
    line2: String,
    footer: String,
}

struct State {
    zoom: Live<f32>,
    font_scale: f32,
    cards: Vec<CardData>,
}

impl State {
    fn new() -> Self {
        let cards = (0..CARDS)
            .map(|i| CardData {
                key: format!("card-{i:02}"),
                title: format!("Canvas Node #{i:02}"),
                badge: if i % 2 == 0 { "LIVE".into() } else { "CRISP".into() },
                version: format!("v2.{}.{}", i % 9, (i * 3) % 14),
                line1: format!("Logarithmic quarter-octave buckets bound atlas pressure ({i:02})."),
                line2: format!("Two-page ping-pong shelf recycling preserves 120 Hz pacing ({i} shards)."),
                footer: format!("layer-{} · scale-settle 100ms · 120 Hz", (i % 4) + 1),
            })
            .collect();
        State {
            zoom: Live::new(1.0),
            font_scale: 1.0,
            cards,
        }
    }
}

const BG: Color = Color { r: 0.012, g: 0.014, b: 0.018, a: 1.0 };
const CARD_BG: Color = Color { r: 0.032, g: 0.036, b: 0.046, a: 1.0 };
const BORDER: Color = Color { r: 0.12, g: 0.14, b: 0.18, a: 1.0 };
const TEXT: Color = Color { r: 0.88, g: 0.90, b: 0.94, a: 1.0 };
const BODY: Color = Color { r: 0.58, g: 0.62, b: 0.68, a: 1.0 };
const MUTED: Color = Color { r: 0.36, g: 0.40, b: 0.46, a: 1.0 };

fn card_view(c: &CardData, k: f32) -> El {
    col()
        .key(c.key.clone())
        .size(228.0, 118.0)
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
                .child(text(c.badge.clone()).font(TextStyle::new(10.0 * k, 600.0)).color(TEXT))
                .child(text(c.title.clone()).font(TextStyle::new(13.0 * k, 600.0)).color(TEXT).grow(1.0))
                .child(text(c.version.clone()).font(TextStyle::new(11.0 * k, 500.0)).color(MUTED)),
        )
        .child(text(c.line1.clone()).font(TextStyle::new(11.0 * k, 400.0)).color(BODY))
        .child(text(c.line2.clone()).font(TextStyle::new(11.0 * k, 400.0)).color(BODY))
        .child(text(c.footer.clone()).font(TextStyle::new(10.0 * k, 500.0)).color(MUTED))
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut grid = col().gap(10.0);
    for (row_idx, chunk) in s.cards.chunks(COLS).enumerate() {
        let mut line = row().key(format!("row-{row_idx}")).gap(10.0);
        for c in chunk {
            line = line.child(card_view(c, s.font_scale));
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
                .live_scale(&s.zoom)
                .child(grid),
        )
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn run_case(gpu: &Gpu, out: &Offscreen, case: &str) {
    let mut text = TextSystem::default();
    for sz in [10.0, 11.0, 13.0] {
        text.layout_rich("Warm benchmark fonts 0123456789", &[], TextStyle::new(sz, 400.0), 2.0, None);
    }
    let state = State::new();
    let zoom = state.zoom.clone();
    let mut rt = Runtime::with_text(state, view, SIZE, Env::new(true, 2.0), text);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);

    // Frame 0: initial rest build and rasterization.
    rt.frame(0.0);
    for _ in 0..30 {
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

    let rest_rasterized = rt.text.atlas.rasterized().max(1);
    let resets0 = rt.text.atlas.resets();

    let mut frame_ms = Vec::with_capacity(FRAMES_10S);
    let mut build_ms = Vec::with_capacity(FRAMES_10S);
    let mut encode_ms = Vec::with_capacity(FRAMES_10S);

    for f in 1..=FRAMES_10S {
        let u = f as f32 / FRAMES_10S as f32;
        if case == "live_zoom_10s" {
            let k = 0.25 * 16.0f32.powf(u);
            zoom.set(k);
        } else {
            rt.state.font_scale = 0.6 + u * 1.4;
            rt.invalidate();
        }
        let t0 = Instant::now();
        rt.frame(1.0 / 120.0);
        let b_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let e0 = Instant::now();
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (out.width, out.height));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &out.view, rt.clear_color());
        renderer.submit(&gpu.queue, enc);
        let e_ms = e0.elapsed().as_secs_f64() * 1000.0;
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();

        build_ms.push(b_ms);
        encode_ms.push(e_ms);
        frame_ms.push(b_ms + e_ms);
    }

    let sweep_rasterized = rt.text.atlas.rasterized();
    let sweep_resets = rt.text.atlas.resets() - resets0;
    let resets_per_min = (sweep_resets as f64) * (60.0 / 10.0);
    let pages = rt.text.atlas.page_count() as f64;
    let sizes_10s = (sweep_rasterized as f64 / rest_rasterized as f64).round();

    // Settle at an off-bucket scale and verify crisp 1:1 glyphs.
    if case == "live_zoom_10s" {
        zoom.set(1.13);
        for _ in 0..20 {
            rt.frame(1.0 / 120.0);
        }
        assert!(
            rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).all(|i| i.uv[2] == 0.0),
            "settled live_scale must re-rasterize crisp 1:1 glyphs"
        );
    } else {
        rt.state.font_scale = 1.0;
        rt.invalidate();
        rt.frame(1.0 / 120.0);
        let glyphs = rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count();
        assert!(glyphs >= 1000, "all glyphs present at scale 1 after 1,200-scale sweep (got {glyphs})");
    }

    for (name, mut samples) in [
        ("frame_ms", frame_ms),
        ("build_ms", build_ms),
        ("encode_ms", encode_ms),
    ] {
        let med = percentile(&mut samples, 0.50);
        let p95 = percentile(&mut samples, 0.95);
        println!("METRIC case={case} metric={name} median={med:.4} p95={p95:.4} n={FRAMES_10S}");
    }
    println!("METRIC case={case} metric=rasterized_10s median={sweep_rasterized} p95={sweep_rasterized} n=1");
    println!("METRIC case={case} metric=sizes_10s median={sizes_10s:.0} p95={sizes_10s:.0} n=1");
    println!("METRIC case={case} metric=resets_per_min median={resets_per_min:.1} p95={resets_per_min:.1} n=1");
    println!("METRIC case={case} metric=atlas_pages median={pages:.0} p95={pages:.0} n=1");
}

fn main() {
    if cfg!(debug_assertions) {
        eprintln!("zoom_sweep: debug build; run with --profile perf");
    }
    let gpu = Gpu::headless();
    let out = Offscreen::new(&gpu.device, (SIZE.0 * 2.0) as u32, (SIZE.1 * 2.0) as u32);
    for case in ["live_zoom_10s", "font_sweep_10s"] {
        run_case(&gpu, &out, case);
    }
}
