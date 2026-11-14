//! Headless spike: CPU reference, vertex-stage differential readback, retained-frame timings.
use bytemuck::{Pod, Zeroable};
use rune::{
    gpu_springs::{Node, Track, split_time},
    render::{Gpu, Instance, NO_CLIP, OFFSCREEN_FORMAT, Offscreen, Renderer},
    text::TextSystem,
};
use rune_motion::{AnimationStore, NodeId, Prop, PropClass, Spring};
use std::time::Instant;
use wgpu::util::DeviceExt;

fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage, mapped_at_creation: false })
}
fn read(device: &wgpu::Device, b: &wgpu::Buffer) -> Vec<u8> {
    b.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = b.slice(..).get_mapped_range().unwrap().to_vec();
    b.unmap();
    bytes
}
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}
fn rng(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    ((*seed >> 40) as f32) / 16777216.0
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Query {
    track: Track,
    time: [f32; 4],
}

fn differential(gpu: &Gpu) {
    let d = &gpu.device;
    let mut queries = Vec::new();
    let mut expected = Vec::new();
    let mut seed = 71;
    // Critical, both sides of the near-critical branch, overdamped, and highly bouncy.
    let bounces = [0.0, 0.00005, 0.0002, -0.0002, -0.9, -0.4, 0.16, 0.8, 0.99];
    for i in 0..4096 {
        let s = Spring::new(0.08 + rng(&mut seed) * 1.92, bounces[i % bounces.len()]);
        let opacity = i % 2 == 0;
        let scale = if opacity { 1.0 } else { 2048.0 };
        let mut from = rng(&mut seed) * scale;
        let mut target = rng(&mut seed) * scale;
        let mut velocity = (rng(&mut seed) - 0.5) * scale * 4.0;
        let mut start = if i % 3 == 0 { 86400.123456 } else { 0.123456 };
        for _ in 0..4 {
            let track = Track::new(s, start, from, target, velocity, if opacity { 0.00001 } else { 0.001 });
            let duration = track.settle_at() - (split_time(start)[0] as f64 + split_time(start)[1] as f64);
            for dt in [0.0, 0.0001, rng(&mut seed) as f64 * s.duration as f64, s.duration as f64 * 2.0, duration + 0.01]
            {
                let now = start + dt;
                let (x, v) = if dt >= duration { (target, 0.0) } else { s.sample(from, velocity, target, dt as f32) };
                let t = split_time(now);
                queries.push(Query { track, time: [t[0], t[1], if opacity { 1.0 } else { 0.0 }, 0.0] });
                expected.push((x, v, opacity));
            }
            let dt = s.duration * (0.05 + rng(&mut seed) * 0.7);
            (from, velocity) = if dt as f64 >= duration { (target, 0.0) } else { s.sample(from, velocity, target, dt) };
            start += dt as f64;
            target = rng(&mut seed) * scale;
        }
    }
    let width = 256;
    let height = queries.len() as u32 / width;
    assert_eq!(queries.len() as u32, width * height);
    let prefix = rune::gpu_springs::SHADER.split("struct SpringNode").next().unwrap();
    let source = format!(
        "{prefix}\n{}",
        r#"
struct Query { track: SpringTrack, time: vec4<f32> };
@group(0) @binding(0) var<storage, read> queries: array<Query>;
struct O { @builtin(position) p: vec4<f32>, @location(0) @interpolate(flat) result: vec4<f32> };
@vertex fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) id: u32) -> O {
    let q = queries[id];
    let value = spring_sample(q.track, q.time.xy);
    let x = f32(id % 256u) + f32(vi & 1u);
    let y = f32(id / 256u) + f32(vi >> 1u);
    var o: O;
    o.p = vec4<f32>(x / 256.0 * 2.0 - 1.0, 1.0 - y / 320.0 * 2.0, 0.0, 1.0);
    o.result = vec4<f32>(value, clamp(value.x, 0.0, 1.0), 1.0);
    return o;
}
@fragment fn fs(i: O) -> @location(0) vec4<f32> { return i.result; }
"#
    );
    assert_eq!(height, 320);
    let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spring differential"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let input = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&queries),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() }],
    });
    let texture = d.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let out = buffer(d, width as u64 * height as u64 * 16, wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
    let mut enc = d.create_command_encoder(&Default::default());
    {
        let view = texture.create_view(&Default::default());
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..4, 0..queries.len() as u32);
    }
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &out,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 16), rows_per_image: None },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([enc.finish()]);
    let data = read(d, &out);
    let mut max_position = 0.0f32;
    let mut max_opacity = 0.0f32;
    let mut max_velocity_relative = 0.0f32;
    let mut max_velocity_absolute = 0.0f32;
    let mut failures = 0;
    for (i, (&(x, v, opacity), bytes)) in expected.iter().zip(data.chunks_exact(16)).enumerate() {
        let got: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        let error = (x - got[0]).abs();
        let limit = if opacity { 0.0001 } else { 0.01 };
        assert!(got.iter().all(|v| v.is_finite()), "nonfinite {i}");
        if error > limit {
            if failures < 8 { eprintln!("sample {i}: {x} vs {} error {error} limit {limit}", got[0]); }
            failures += 1;
        }
        let velocity_error = (v - got[1]).abs() / v.abs().max(1.0);
        max_velocity_absolute = max_velocity_absolute.max((v - got[1]).abs());
        if opacity {
            max_opacity = max_opacity.max((x.clamp(0.0, 1.0) - got[2]).abs());
        } else {
            max_position = max_position.max(error);
        }
        max_velocity_relative = max_velocity_relative.max(velocity_error);
    }
    println!(
        "DIFFERENTIAL samples={} retargets={} max_position={} max_opacity={} max_velocity_relative={} max_velocity_absolute={} failures={}",
        queries.len(),
        4096 * 3,
        max_position,
        max_opacity,
        max_velocity_relative,
        max_velocity_absolute,
        failures
    );
    assert_eq!(failures, 0, "GPU position/opacity differential");
}

fn bench(gpu: &Gpu, timing: bool) {
    let d = &gpu.device;
    let q = &gpu.queue;
    let target = Offscreen::new(d, 512, 512);
    let queries =
        d.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: 2 });
    let resolved = buffer(d, 16, wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC);
    let timestamps = buffer(d, 16, wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
    for n in [1, 100, 1000, 10000] {
        let s = Spring::new(3.0, 0.16);
        let nodes: Vec<_> = (0..n)
            .map(|i| Node {
                x: Track::new(s, 0.0, 0.0, 3.0 + (i % 7) as f32, 0.0, 0.001),
                y: Track::new(s, 0.0, 0.0, 2.0, 0.0, 0.001),
                opacity: Track::new(Spring::new(3.0, 0.0), 0.0, 0.2, 0.9, 0.0, 0.00001),
            })
            .collect();
        let ids: Vec<_> = (0..n as u32).collect();
        let mut store = AnimationStore::with_capacity(n * 3, 1);
        const X: Prop<f32> = Prop::custom(64, PropClass::Spatial);
        const Y: Prop<f32> = Prop::custom(65, PropClass::Spatial);
        const A: Prop<f32> = Prop::custom(66, PropClass::Opacity);
        let tracks: Vec<_> = (0..n)
            .map(|i| {
                let id = NodeId(i as u64);
                let x = store.set(id, X, 0.0);
                let y = store.set(id, Y, 0.0);
                let a = store.set(id, A, 0.2);
                store.animate(id, X, 3.0 + (i % 7) as f32, s);
                store.animate(id, Y, 2.0, s);
                store.animate(id, A, 0.9, Spring::new(3.0, 0.0));
                [x, y, a]
            })
            .collect();
        let base: Vec<_> = (0..n)
            .map(|i| Instance {
                rect: [12.0 + (i % 100) as f32 * 4.8, 12.0 + (i / 100) as f32 * 4.8, 2.0, 2.0],
                fill: [0.3, 0.5, 0.7, 1.0],
                clip: NO_CLIP,
                ..Zeroable::zeroed()
            })
            .collect();
        let mut instances = base.clone();
        let mut cpu = Renderer::new(d, OFFSCREEN_FORMAT);
        let mut fast = Renderer::with_springs(d, OFFSCREEN_FORMAT, &nodes, &ids);
        let mut atlas = TextSystem::default().atlas;
        fast.prepare(d, q, &mut atlas, &base, (512, 512));
        let mut results: [Vec<(f64, f64)>; 2] = [vec![], vec![]];
        for run in 0..if timing { 8 } else { 0 } {
            for k in 0..2 {
                let path = (k + run) % 2;
                let mut cpu_times = Vec::new();
                let mut gpu_times = Vec::new();
                for frame in 0..120 {
                    let now = frame as f64 / 120.0;
                    let start = Instant::now();
                    let renderer = if path == 0 {
                        store.tick_at(now);
                        for (i, inst) in instances.iter_mut().enumerate() {
                            let x = store.values_of(tracks[i][0]).unwrap()[0];
                            let y = store.values_of(tracks[i][1]).unwrap()[0];
                            let a = store.values_of(tracks[i][2]).unwrap()[0].clamp(0.0, 1.0);
                            *inst = base[i];
                            inst.rect[0] += x;
                            inst.rect[1] += y;
                            inst.fill = inst.fill.map(|c| c * a);
                        }
                        cpu.prepare(d, q, &mut atlas, &instances, (512, 512));
                        &cpu
                    } else {
                        fast.present_at(q, (512, 512), now);
                        &fast
                    };
                    let mut enc = d.create_command_encoder(&Default::default());
                    renderer.draw_timed(
                        &mut enc,
                        &target.view,
                        [0.0; 4],
                        Some(wgpu::RenderPassTimestampWrites {
                            query_set: &queries,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: Some(1),
                        }),
                    );
                    q.submit([enc.finish()]);
                    cpu_times.push(start.elapsed().as_secs_f64() * 1e6);
                    // Resolve only after this pass completed. Metal counter-sample resolves
                    // in the same command buffer returned stale/reversed values on M5 Max.
                    d.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                    let mut enc = d.create_command_encoder(&Default::default());
                    enc.resolve_query_set(&queries, 0..2, &resolved, 0);
                    enc.copy_buffer_to_buffer(&resolved, 0, &timestamps, 0, 16);
                    q.submit([enc.finish()]);
                    let bytes = read(d, &timestamps);
                    let t0 = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                    let t1 = u64::from_le_bytes(bytes[8..].try_into().unwrap());
                    assert!(t1 >= t0, "reversed GPU timestamps: {t0} -> {t1}");
                    gpu_times.push((t1 - t0) as f64 * q.get_timestamp_period() as f64 / 1000.0);
                }
                if run > 0 {
                    let c = median(&mut cpu_times);
                    let g = median(&mut gpu_times);
                    println!("RUN nodes={n} path={path} run={run} cpu_us={c:.3} gpu_us={g:.3}");
                    results[path].push((c, g));
                }
            }
        }
        for (path, runs) in results.iter().enumerate() {
            if runs.is_empty() { continue; }
            let mut c: Vec<_> = runs.iter().map(|x| x.0).collect();
            let mut g: Vec<_> = runs.iter().map(|x| x.1).collect();
            let cm = median(&mut c);
            let gm = median(&mut g);
            let mad = median(&mut c.iter().map(|x| (x - cm).abs()).collect::<Vec<_>>());
            println!(
                "BENCH nodes={n} path={} cpu_us={cm:.3} cpu_mad_us={mad:.3} gpu_us={gm:.3}",
                if path == 0 { "cpu" } else { "gpu" }
            );
        }
        // Same uber fragment path, compare actual pixels including alpha and antialiasing.
        for now in [0.0, 0.07, 0.31, 0.99] {
            for (i, inst) in instances.iter_mut().enumerate() {
                *inst = base[i];
                inst.rect[0] += s.sample(0.0, 0.0, 3.0 + (i % 7) as f32, now).0;
                inst.rect[1] += s.sample(0.0, 0.0, 2.0, now).0;
                let a = Spring::new(3.0, 0.0).sample(0.2, 0.0, 0.9, now).0.clamp(0.0, 1.0);
                inst.fill = inst.fill.map(|c| c * a);
            }
            cpu.prepare(d, q, &mut atlas, &instances, (512, 512));
            fast.present_at(q, (512, 512), now as f64);
            // Uploading changed geometry after latching time must not rewind the tracks.
            fast.prepare(d, q, &mut atlas, &base, (512, 512));
            let mut images = Vec::new();
            for r in [&cpu, &fast] {
                let mut enc = d.create_command_encoder(&Default::default());
                r.draw(&mut enc, &target.view, [0.0; 4]);
                target.copy(&mut enc);
                q.submit([enc.finish()]);
                images.push(target.read(d));
            }
            let max = images[0].iter().zip(&images[1]).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            let changed = images[0].iter().zip(&images[1]).filter(|(a, b)| a != b).count();
            assert!(max <= 1, "pixel delta {max}");
            println!("PIXELS nodes={n} time={now} changed_channels={changed} max_delta={max}");
        }
        assert!(fast.springs_animating(0.5));
        assert!(!fast.springs_animating(120.0));
        if n == 1 {
            // Replacing the last moving node must cancel its old wake deadline.
            fast.write_spring_node(q, 0, &Node {
                x: Track::constant(0.0), y: Track::constant(0.0), opacity: Track::constant(1.0),
            });
            assert!(!fast.springs_animating(0.5));
        }
    }
}

fn primitive_pixels(gpu: &Gpu) {
    let (d, q) = (&gpu.device, &gpu.queue);
    let target = Offscreen::new(d, 512, 96);
    let mut cpu = Renderer::new(d, OFFSCREEN_FORMAT);
    let mut image_atlas = Default::default();
    cpu.upload_images(d, q, &mut image_atlas);
    let pixels = rune::Pixels::new();
    pixels.set(2, 2, vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 192, 255, 255, 255, 255]);
    let uv = image_atlas.place(&pixels, [2.0, 2.0]).unwrap().rect.map(|v| v as f32);
    let s = Spring::new(0.4, 0.16);
    let fade = Spring::new(0.4, 0.0);
    let node = Node {
        x: Track::new(s, 0.0, 0.0, 6.0, 0.0, 0.001),
        y: Track::new(s, 0.0, 0.0, 3.0, 0.0, 0.001),
        opacity: Track::new(fade, 0.0, 0.2, 0.9, 0.0, 0.00001),
    };
    let mut fast = Renderer::with_springs(d, OFFSCREEN_FORMAT, &[node], &[0; 8]);
    cpu.upload_images(d, q, &mut image_atlas);
    image_atlas.invalidate();
    fast.upload_images(d, q, &mut image_atlas);
    let mut atlas = TextSystem::default().atlas;
    // A real opaque glyph footprint, independently uploaded to both renderers.
    atlas.data[0] = 255;
    atlas.data[1] = 192;
    atlas.data[rune::text::ATLAS_SIZE as usize] = 128;
    atlas.data[rune::text::ATLAS_SIZE as usize + 1] = 255;
    let base: Vec<_> = (0..8).map(|kind| {
        let x = 8.0 + kind as f32 * 60.0;
        let mut i = Instance {
            rect: [x, 24.0, 40.0, 30.0], radii: [4.0; 4], fill: [0.2, 0.4, 0.6, 0.8],
            params: [0.0, kind as f32, 0.0, 1.0], clip: NO_CLIP, ..Zeroable::zeroed()
        };
        match kind {
            1 => i.params[2] = 2.0,
            2 => i.uv = [0.0, 0.0, 2.0, 2.0],
            3 => { i.params[0] = 2.0; i.params[2] = 1.0; }
            4 => i.uv = [0.7, 0.2, 0.1, 0.8],
            5 => { i.radii = [x, 24.0, x + 40.0, 54.0]; i.params[0] = 2.0; i.params[2] = 1.0; }
            6 => { i.radii = [x, 24.0, x + 40.0, 24.0]; i.uv = [x + 20.0, 54.0, 0.0, 0.0]; i.params[0] = 7.0; }
            7 => i.uv = uv,
            _ => {}
        }
        i
    }).collect();
    for now in [0.0, 0.07, 0.31, 0.99] {
        let dx = s.sample(0.0, 0.0, 6.0, now).0;
        let dy = s.sample(0.0, 0.0, 3.0, now).0;
        let alpha = fade.sample(0.2, 0.0, 0.9, now).0.clamp(0.0, 1.0);
        let mut expected = base.clone();
        for (kind, i) in expected.iter_mut().enumerate() {
            i.rect[0] += dx;
            i.rect[1] += dy;
            i.fill = i.fill.map(|v| v * alpha);
            if kind == 4 { i.uv = i.uv.map(|v| v * alpha); }
            if kind == 5 || kind == 6 {
                for (v, offset) in i.radii.iter_mut().zip([dx, dy, dx, dy]) { *v += offset; }
            }
            if kind == 6 { i.uv[0] += dx; i.uv[1] += dy; }
        }
        atlas.dirty = Some((0, 2));
        cpu.prepare(d, q, &mut atlas, &expected, (512, 96));
        atlas.dirty = Some((0, 2));
        fast.present_at(q, (512, 96), now as f64);
        fast.prepare(d, q, &mut atlas, &base, (512, 96));
        let mut images = Vec::new();
        for renderer in [&cpu, &fast] {
            let mut enc = d.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0; 4]);
            target.copy(&mut enc);
            q.submit([enc.finish()]);
            images.push(target.read(d));
        }
        for kind in 0..8 {
            assert!((0..96).any(|y| (kind * 60..kind * 60 + 60).any(|x| images[0][(y * 512 + x) * 4 + 3] != 0)), "blank primitive {kind}");
        }
        let max = images[0].iter().zip(&images[1]).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        let changed = images[0].iter().zip(&images[1]).filter(|(a, b)| a != b).count();
        assert!(max <= 1, "primitive pixel delta {max} at {now}");
        println!("PRIMITIVES kinds=8 time={now} changed_channels={changed} max_delta={max}");
    }
}

fn main() {
    let instance = Gpu::instance();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: wgpu::Features::TIMESTAMP_QUERY,
        ..Default::default()
    }))
    .unwrap();
    println!("ADAPTER {:?}", adapter.get_info());
    let gpu = Gpu { instance, adapter, device, queue };
    differential(&gpu);
    bench(&gpu, !std::env::args().any(|arg| arg == "--verify-only"));
    primitive_pixels(&gpu);
}
