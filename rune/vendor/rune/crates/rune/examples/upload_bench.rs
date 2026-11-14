//! Headless per-frame upload cost: `prepare`, `draw` and `submit` of a 2,000-instance frame,
//! with a few instances moving (`steady`) or all of them (`scroll`). Prints the median CPU
//! time to record and submit a frame (and on macOS the instructions the process ran meanwhile,
//! which a throttled or busy machine leaves unchanged), the median time until the GPU is done,
//! and the buffers created per frame, then the instructions each step ran (`prepare`, making the
//! encoder, `draw`, `submit`). Run with `--profile perf`; it leaves out presentation, and the GPU
//! copy is counted only in `frame_us`.

use std::time::Instant;

use bytemuck::Zeroable;
use rune::render::{Gpu, Instance, NO_CLIP, Offscreen, Renderer};

const SIZE: (u32, u32) = (1280, 800);
const COUNT: usize = 2000;
const FRAMES: usize = 600;
const WARMUP: usize = 60;

/// `UPLOAD_FRAMES` frames, default `FRAMES`: more to profile a run.
fn frames() -> usize { std::env::var("UPLOAD_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(FRAMES) }

fn scene(frame: usize, scroll: bool) -> Vec<Instance> {
    (0..COUNT)
        .map(|i| {
            let (mut x, mut y) = ((i % 50) as f32 * 25.0, (i / 50) as f32 * 20.0);
            if scroll {
                y -= (frame % 200) as f32;
            } else if i % 700 == 0 {
                x += (frame % 100) as f32;
            }
            Instance { rect: [x, y, 20.0, 14.0], fill: [0.2, 0.4, 0.8, 1.0], radii: [3.0; 4], clip: NO_CLIP, ..Zeroable::zeroed() }
        })
        .collect()
}

/// Instructions the process has retired, in thousands (0 off macOS).
fn kinstr() -> f64 {
    #[cfg(target_os = "macos")]
    // SAFETY: proc_pid_rusage fills the rusage_info_v4 it is handed for RUSAGE_INFO_V4.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, (&raw mut info).cast());
        info.ri_instructions as f64 / 1e3
    }
    #[cfg(not(target_os = "macos"))]
    0.0
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut text = rune::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    for (name, scroll) in [("steady", false), ("scroll", true)] {
        let mut renderer = Renderer::new(device, rune::render::OFFSCREEN_FORMAT);
        let (mut cpu, mut instructions, mut frame, mut created) = (Vec::new(), Vec::new(), Vec::new(), 0isize);
        let mut phases: Vec<[f64; 4]> = Vec::new();
        for i in 0..frames() + WARMUP {
            let instances = scene(i, scroll);
            let buffers = device.get_internal_counters().hal.buffers.read();
            let (start, start_kinstr) = (Instant::now(), kinstr());
            renderer.prepare(device, queue, &mut text.atlas, &instances, SIZE);
            let k1 = kinstr();
            let mut encoder = device.create_command_encoder(&Default::default());
            let k2 = kinstr();
            renderer.draw(&mut encoder, &target.view, [0.0; 4]);
            let k3 = kinstr();
            let made = device.get_internal_counters().hal.buffers.read() - buffers;
            renderer.submit(queue, encoder);
            if i >= WARMUP { phases.push([k1 - start_kinstr, k2 - k1, k3 - k2, kinstr() - k3]); }
            let (recorded, recorded_kinstr) = (start.elapsed(), kinstr() - start_kinstr);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            if i >= WARMUP {
                cpu.push(recorded.as_secs_f64() * 1e6);
                instructions.push(recorded_kinstr);
                frame.push(start.elapsed().as_secs_f64() * 1e6);
                created += made;
            }
        }
        println!(
            "scene={name} cpu_us={:.1} kinstr={:.1} frame_us={:.1} buffers_per_frame={:.2}",
            median(cpu),
            median(instructions),
            median(frame),
            created as f64 / frames() as f64
        );
        let ph = |j: usize| median(phases.iter().map(|p| p[j]).collect());
        println!("  phases kinstr: prepare={:.1} encoder={:.1} draw={:.1} submit={:.1}", ph(0), ph(1), ph(2), ph(3));
    }
}
