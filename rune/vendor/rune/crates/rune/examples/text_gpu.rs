//! Text drawn from outlines on the GPU against the glyph atlas: stills, and GPU frame times.
//!
//! `cargo run --profile perf -p rune --example text_gpu -- snap <dir> [scale]` writes a paragraph
//! at rest (atlas) and mid-zoom (outlines) at the same zoom, and the zoom sweep's frames.
//! `cargo run --profile perf -p rune --example text_gpu -- perf [scale]` prints the GPU time of
//! the frame pass (timestamp queries, median of 200 frames) for a full screen of text at rest,
//! scrolling and zooming. `WGPU_ADAPTER_NAME` picks the adapter.

use rune::render::{Gpu, KIND_GLYPH, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{Color, Cx, El, Env, Live, Runtime, TextStyle, col, el, text};

const PARAGRAPH: &str = "Windfoil integrates each glyph's outline over the pixel box in closed form, so text that \
zooms, scrolls or turns stays as sharp as text at rest. Quick zephyrs blow, vexing daft Jim; \
0123456789 (){}[] -> => != <= Ag Wa To fi fl ff — “quotes” ‘and’ «more».";

const CJK: &str = "文字は輪郭から直接描かれ、拡大しても鮮明です。中文排版测试：汉字笔画清晰度。한국어 글꼴 테스트입니다. نص عربي للاختبار والتجربة";

struct State {
    content: &'static str,
    zoom: Live<f32>,
    scroll: f32,
    lines: usize,
    size: f32,
    dark: bool,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let grey = |v: u8| { let c = rune::color::srgb_decode(v as f32 / 255.0); Color { r: c, g: c, b: c, a: 1.0 } };
    let (bg, fg) = if s.dark { (grey(0x16), grey(0xed)) } else { (grey(0xff), grey(0x16)) };
    let mut body = col().gap(4.0).w_full();
    for i in 0..s.lines {
        let style = match i % 4 {
            0 => TextStyle::new(s.size, 440.0),
            1 => TextStyle::new(s.size, 400.0).mono(),
            2 => TextStyle::new(s.size * 1.25, 600.0),
            _ => TextStyle::new(s.size * 0.85, 400.0),
        };
        body = body.child(text(s.content).font(style).color(fg).w_full());
    }
    el().w_full().h_full().fill(bg).padding(16.0).child(
        el().key("canvas").w_full().live_scale(&s.zoom).offset(0.0, -s.scroll).child(body),
    )
}

fn frame(gpu: &Gpu, renderer: &mut Renderer, rt: &mut Runtime<State>, out: &Offscreen, timestamps: Option<&wgpu::QuerySet>) {
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (out.width, out.height));
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    let writes = timestamps.map(|q| wgpu::RenderPassTimestampWrites { query_set: q, beginning_of_pass_write_index: Some(0), end_of_pass_write_index: Some(1) });
    renderer.draw_timed(&mut enc, &out.view, rt.clear_color(), writes);
    if timestamps.is_none() { out.copy(&mut enc); }
    renderer.submit(&gpu.queue, enc);
}

fn snap(gpu: &Gpu, renderer: &mut Renderer, rt: &mut Runtime<State>, out: &Offscreen, path: &std::path::Path) {
    frame(gpu, renderer, rt, out, None);
    let rgba = out.read(&gpu.device);
    rune::render::write_png(path, out.width, out.height, &rgba).expect("write png");
}

fn runtime(scale: f32, size: (f32, f32), lines: usize, dark: bool) -> Runtime<State> {
    let content = if std::env::args().any(|a| a == "cjk") { CJK } else { PARAGRAPH };
    let state = State { content, zoom: Live::new(1.0), scroll: 0.0, lines, size: 13.0, dark };
    Runtime::with_text(state, view, size, Env::new(dark, scale), TextSystem::default())
}

fn outlined(rt: &Runtime<State>) -> usize {
    rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH && i.radii[3] > 0.0).count()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("snap") => {
            let dir = std::path::PathBuf::from(args.get(1).expect("snap <dir> [scale]"));
            let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
            std::fs::create_dir_all(&dir).unwrap();
            let gpu = Gpu::headless();
            for dark in [false, true] {
                let size = (520.0, 300.0);
                let out = Offscreen::new(&gpu.device, (size.0 * scale) as u32, (size.1 * scale) as u32);
                let mut renderer = if args.get(4).is_some_and(|a| a == "classic") { Renderer::new_classic(&gpu.device, OFFSCREEN_FORMAT) } else { Renderer::new(&gpu.device, OFFSCREEN_FORMAT) };
                let mut rt = runtime(scale, size, 4, dark);
                if let Some(mode) = args.get(3) { rt.set_font_smoothing(match mode.as_str() { "on" => rune::FontSmoothing::On, "windfoil" => rune::FontSmoothing::Windfoil, "off" => rune::FontSmoothing::Off, _ => rune::FontSmoothing::DirectWrite }); }
                let zoom = rt.state.zoom.clone();
                let tag = if dark { "dark" } else { "light" };
                // `gpu`: the renderer says first that it fills masks, so the first frame's are GPU-filled.
                if args.get(5).is_some_and(|a| a == "gpu") {
                    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &[], (out.width, out.height));
                }
                let t0 = std::time::Instant::now();
                rt.frame(0.0);
                println!("first frame {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0);
                snap(&gpu, &mut renderer, &mut rt, &out, &dir.join(format!("rest-{tag}.png")));
                println!("at rest: shader {}", renderer.uber_shader());
                for (i, k) in [1.0f32, 1.06, 1.3, 1.6, 2f32.powf(0.25), std::f32::consts::SQRT_2].iter().enumerate() {
                    zoom.set(*k);
                    rt.frame(1.0 / 120.0);
                    let moving = outlined(&rt);
                    snap(&gpu, &mut renderer, &mut rt, &out, &dir.join(format!("zoom{i}-moving-{tag}.png")));
                    for _ in 0..40 { rt.frame(1.0 / 120.0); }
                    let resting = outlined(&rt);
                    snap(&gpu, &mut renderer, &mut rt, &out, &dir.join(format!("zoom{i}-rest-{tag}.png")));
                    println!("zoom {k}: {moving} outline glyphs moving, {resting} at rest, shader {}", renderer.uber_shader());
                }
            }
        }
        Some("perf") => perf(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1.5), args.get(2).map(String::as_str)),
        _ => eprintln!("usage: text_gpu snap <dir> [scale] | perf [scale]"),
    }
}

fn perf(scale: f32, mode: Option<&str>) {
    let gpu = Gpu::try_with_features(Gpu::instance(), None, wgpu::Features::TIMESTAMP_QUERY).expect("a GPU with timestamp queries");
    let info = gpu.adapter.get_info();
    let size = (2560.0 / scale, 1440.0 / scale);
    let out = Offscreen::new(&gpu.device, 2560, 1440);
    let queries = gpu.device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: 2 });
    let resolve = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let read = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let period = gpu.queue.get_timestamp_period() as f64;
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    // `empty`: the same frame without its text, so the text's own cost is the difference.
    let mut rt = runtime(scale, size, if mode == Some("empty") { 0 } else { 48 }, false);
    if let Some(mode) = mode { rt.set_font_smoothing(match mode { "on" => rune::FontSmoothing::On, "directwrite" => rune::FontSmoothing::DirectWrite, "off" => rune::FontSmoothing::Off, _ => rune::FontSmoothing::Windfoil }); }
    let zoom = rt.state.zoom.clone();
    rt.frame(0.0);
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    let glyphs = rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count();
    let measure = |rt: &mut Runtime<State>, renderer: &mut Renderer, step: &mut dyn FnMut(&mut Runtime<State>, usize)| {
        let mut ms = Vec::new();
        for f in 0..220 {
            step(rt, f);
            rt.frame(1.0 / 120.0);
            frame(&gpu, renderer, rt, &out, Some(&queries));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            enc.resolve_query_set(&queries, 0..2, &resolve, 0);
            enc.copy_buffer_to_buffer(&resolve, 0, &read, 0, 16);
            gpu.queue.submit([enc.finish()]);
            read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
            let t: [u64; 2] = bytemuck::cast_slice::<u8, u64>(&read.slice(..).get_mapped_range().expect("mapped")).try_into().unwrap();
            read.unmap();
            if f >= 20 { ms.push((t[1].saturating_sub(t[0])) as f64 * period / 1e6); }
        }
        ms.sort_by(f64::total_cmp);
        (ms[ms.len() / 2], ms[ms.len() * 9 / 10], outlined(rt))
    };
    let still = measure(&mut rt, &mut renderer, &mut |_, _| {});
    let scroll = measure(&mut rt, &mut renderer, &mut |rt, f| { rt.state.scroll = (f % 60) as f32 * 3.0; rt.invalidate(); });
    rt.state.scroll = 0.0;
    rt.invalidate();
    let zooming = measure(&mut rt, &mut renderer, &mut |_, f| { zoom.set(1.0 + 0.25 * ((f as f32) * 0.05).sin().abs()); });
    println!("adapter={} backend={:?} scale={scale} mode={mode:?} glyphs={glyphs} shader={}", info.name, info.backend, renderer.uber_shader());
    for (name, (median, p90, outlines)) in [("still", still), ("scroll", scroll), ("zoom", zooming)] {
        println!("METRIC case={name} scale={scale} mode={} median_ms={median:.4} p90_ms={p90:.4} outline_glyphs={outlines}", mode.unwrap_or("default"));
    }
}
