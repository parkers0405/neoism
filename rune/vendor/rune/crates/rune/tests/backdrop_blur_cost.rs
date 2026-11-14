//! Manual cost fixture for `El::backdrop_blur`: a full-width 60 pt frosted bar at 2x over a
//! scrolling list, against the same bar with a plain fill.
//!
//! `cargo test -p rune --profile perf --test backdrop_blur_cost -- --ignored --nocapture`
use std::time::Instant;

use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, col, el, oklch, row};

#[derive(Default)]
struct Bar {
    blur: bool,
}

fn view(s: &Bar, _: &mut Cx<Bar>) -> El {
    let rows = (0..200).map(|i| row().w_full().h(56.0).px(16.0).gap(12.0).center_y()
        .child(el().size(40.0, 40.0).radius(12.0).fill(oklch(0.7, 0.16, (i as f32 * 47.0) % 360.0)))
        .child(el().size(180.0, 12.0).radius(6.0).fill(oklch(0.4, 0.0, 0.0))));
    let bar = el().pin(Some(0.0), None, Some(0.0), Some(0.0)).h(60.0).fill(Color::WHITE.alpha(0.6));
    el().w_full().h_full().fill(Color::WHITE)
        .child(col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(col().w_full().children(rows)))
        .child(if s.blur { bar.backdrop_blur(20.0) } else { bar })
}

/// GPU milliseconds per frame: `K` frames encoded into one submission, timed on the wall
/// clock from submit to completion, so the GPU stays busy and clocked up and the encode isn't
/// counted.
fn gpu_time(gpu: &Gpu, mut draw: impl FnMut(&mut wgpu::CommandEncoder)) -> f64 {
    const K: u32 = 24;
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    for _ in 0..K { draw(&mut enc); }
    let commands = enc.finish();
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let t0 = Instant::now();
    gpu.queue.submit([commands]);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    t0.elapsed().as_secs_f64() * 1000.0 / f64::from(K)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
#[ignore = "manual GPU and CPU cost of a full-width 60 pt frosted bar at 2x"]
fn frosted_bar_cost() {
    let gpu = Gpu::headless();
    eprintln!("adapter: {:?}", gpu.adapter.get_info().name);
    for (w, h) in [(390.0f32, 844.0f32), (1280.0, 800.0)] {
        let size = ((w * 2.0) as u32, (h * 2.0) as u32);
        let target = Offscreen::new(&gpu.device, size.0, size.1);
        let mut sides: Vec<_> = [false, true].into_iter().map(|blur| {
            let mut rt = Runtime::new(Bar { blur }, view, (w, h), Env::new(false, 2.0));
            rt.frame(0.0);
            for _ in 0..4 { rt.frame(1.0 / 120.0); }
            let idle = !rt.frame(1.0 / 120.0);
            let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
            (blur, rt, renderer, idle, Vec::new(), Vec::new(), Vec::new(), Vec::new())
        }).collect();
        // Alternate the two sides so clock and thermal drift hit both alike.
        for round in 0..12 {
            for (_, rt, renderer, _, still, scroll, frame_ms, encode_ms) in &mut sides {
                renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
                let ms = gpu_time(&gpu, |enc| renderer.draw(enc, &target.view, [1.0; 4]));
                // Scrolling: a 3 pt wheel step each 120 Hz frame, rebuilt, prepared and encoded.
                let dir = if round % 2 == 0 { -3.0 } else { 3.0 };
                let mut frames = Vec::new();
                let ms_scroll = gpu_time(&gpu, |enc| {
                    rt.wheel(w * 0.5, h * 0.5, dir);
                    let t0 = Instant::now();
                    rt.frame(1.0 / 120.0);
                    let t1 = Instant::now();
                    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
                    renderer.draw(enc, &target.view, [1.0; 4]);
                    frames.push(((t1 - t0).as_secs_f64() * 1000.0, t1.elapsed().as_secs_f64() * 1000.0));
                });
                if round > 1 {
                    still.push(ms);
                    scroll.push(ms_scroll);
                    frame_ms.extend(frames.iter().map(|f| f.0));
                    encode_ms.extend(frames.iter().map(|f| f.1));
                }
            }
        }
        for (blur, _, _, idle, still, scroll, frame_ms, encode_ms) in sides {
            eprintln!("{w}x{h} pt @2x blur={blur}: idle frame asks for redraw: {}; gpu still {:.4} ms, scrolling {:.4} ms; cpu frame {:.4} ms, prepare+encode {:.4} ms",
                !idle, median(still), median(scroll), median(frame_ms), median(encode_ms));
        }
    }
}
