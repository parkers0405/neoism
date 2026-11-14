use super::*;

const SIZE: (u32, u32) = (64, 32);
const RED: [u8; 4] = [255, 0, 0, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn square(x: f32, y: f32) -> Instance {
    Instance { rect: [x, y, 8.0, 8.0], fill: [1.0, 0.0, 0.0, 1.0], clip: NO_CLIP, ..Zeroable::zeroed() }
}

/// A moving square over a still grid of small ones.
fn scene(frame: usize) -> Vec<Instance> {
    let grid = (0..48).map(|i| Instance { rect: [(i % 16) as f32 * 4.0, (i / 16) as f32 * 4.0, 1.0, 1.0], ..square(0.0, 0.0) });
    std::iter::once(square(frame as f32, 20.0)).chain(grid).collect()
}

fn live_buffers(device: &wgpu::Device) -> isize {
    device.get_internal_counters().hal.buffers.read()
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE.0 + x) * 4) as usize;
    pixels[at..at + 4].try_into().unwrap()
}

#[test]
fn a_steady_animating_frame_creates_no_buffers_and_skips_unchanged_globals() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    let mut pixels = Vec::new();
    for frame in 0..24 {
        let (buffers, globals, stats) = (live_buffers(device), renderer.uploads.globals_writes, renderer.uploads.stats());
        renderer.prepare(device, queue, &mut text.atlas, &scene(frame), SIZE);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        // Before the submit, while a queue write's staging buffer would still be alive.
        let created = live_buffers(device) - buffers;
        if frame == 23 { target.copy(&mut encoder); }
        renderer.submit(queue, encoder);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        if frame < 4 { continue; }
        let now = renderer.uploads.stats();
        assert_eq!(created, 0, "frame {frame} created buffers");
        assert_eq!(now.chunks, stats.chunks, "frame {frame} made a staging chunk");
        assert_eq!(renderer.uploads.globals_writes, globals, "frame {frame} rewrote unchanged globals");
        assert_eq!((now.writes - stats.writes, now.bytes - stats.bytes), (1, size_of::<Instance>() as u64),
            "frame {frame} writes only the instance that moved");
        // Written in place where the backend can (`rune_staging::set_direct`), else staged.
        let staged = u64::from(now.direct == stats.direct);
        assert_eq!((now.flushes - stats.flushes, now.submits - stats.submits), (staged, 0), "recorded into the frame");
    }
    pixels.extend(target.read(device));
    assert_eq!(pixel(&pixels, 27, 24), RED, "the square reached its last position");
    assert_eq!(pixel(&pixels, 2, 24), CLEAR, "and left its first");
}

/// Instances that moved far apart are written alone, ones close together as one span, and every
/// write lands: the frame draws each square where it is now.
#[test]
fn instances_are_written_in_the_spans_that_changed() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    let hidden = square(-100.0, -100.0);
    // Two short of a power of two, so the instances added below fit the buffer.
    let (far, last) = (2 * BLOCK + 10, 4 * BLOCK - 3);
    let mut instances = vec![hidden; last + 1];
    let mut frame = |instances: &[Instance]| {
        let stats = renderer.uploads.stats();
        renderer.prepare(device, queue, &mut text.atlas, instances, SIZE);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        target.copy(&mut encoder);
        renderer.submit(queue, encoder);
        let now = renderer.uploads.stats();
        ((now.writes - stats.writes, (now.bytes - stats.bytes) / size_of::<Instance>() as u64), target.read(device))
    };
    frame(&instances);
    (instances[3], instances[far], instances[last]) = (square(0.0, 0.0), square(20.0, 0.0), square(40.0, 0.0));
    let (written, pixels) = frame(&instances);
    assert_eq!(written, (3, 3), "three far apart instances, written alone");
    for x in [4, 24, 44] { assert_eq!(pixel(&pixels, x, 4), RED, "the square at {x} landed"); }
    (instances[far], instances[far + 10], instances[far + 25]) = (hidden, square(20.0, 20.0), square(40.0, 20.0));
    let (written, pixels) = frame(&instances);
    assert_eq!(written, (1, 26), "close together, one span from the first to the last");
    assert_eq!(pixel(&pixels, 24, 4), CLEAR, "the square that left");
    for x in [24, 44] { assert_eq!(pixel(&pixels, x, 24), RED, "the square at {x}, 24 landed"); }
    instances.extend([square(56.0, 0.0), hidden]);
    let (written, pixels) = frame(&instances);
    assert_eq!(written, (1, 2), "added instances");
    assert_eq!(pixel(&pixels, 60, 4), RED);
    instances.iter_mut().for_each(|i| i.rect[1] += 1.0);
    assert_eq!(frame(&instances).0, (1, last as u64 + 3), "everything moved: one span");
}

/// The upload has no encoder to go into when the first draw is into a caller's pass, and a
/// caller may submit without `Renderer::submit`.
#[test]
fn uploads_land_before_a_draw_into_a_callers_pass() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    for x in [0.0, 30.0] {
        renderer.prepare(device, queue, &mut text.atlas, &[square(x, 4.0)], SIZE);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            renderer.draw_range(&mut pass, OFFSCREEN_FORMAT, 0..1);
        }
        target.copy(&mut encoder);
        queue.submit([encoder.finish()]);
        let pixels = target.read(device);
        assert_eq!(pixel(&pixels, x as u32 + 4, 8), RED);
    }
}

/// Writes recorded into an encoder that is then dropped never land; the next frame sends what
/// it would otherwise skip as unchanged.
#[test]
fn a_dropped_encoder_makes_the_next_frame_resend_everything() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    let instances = [square(10.0, 10.0)];
    renderer.prepare(device, queue, &mut text.atlas, &instances, SIZE);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    drop(encoder);
    renderer.prepare(device, queue, &mut text.atlas, &instances, SIZE);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    renderer.submit(queue, encoder);
    assert_eq!(pixel(&target.read(device), 14, 14), RED);
}

/// `Frame::write_buffer`: each write lands ahead of the commands recorded after it, and a frame's
/// writes share one reused staging chunk rather than making a buffer each, as a queue write would.
#[test]
fn compositor_buffer_writes_land_in_order_and_reuse_staging() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: None, size: 16, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None, size: 32, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    for frame in 0..12u32 {
        let buffers = live_buffers(device);
        let mut encoder = device.create_command_encoder(&Default::default());
        let allocs = crate::tooling::test_alloc::allocs();
        renderer.write_buffer(device, queue, &mut encoder, &uniforms, 0, bytemuck::cast_slice(&[frame; 4]));
        let first = crate::tooling::test_alloc::allocs() - allocs;
        encoder.copy_buffer_to_buffer(&uniforms, 0, &readback, 0, 16);
        let allocs = crate::tooling::test_alloc::allocs();
        renderer.write_buffer(device, queue, &mut encoder, &uniforms, 0, bytemuck::cast_slice(&[frame + 100; 4]));
        let second = crate::tooling::test_alloc::allocs() - allocs;
        encoder.copy_buffer_to_buffer(&uniforms, 0, &readback, 16, 16);
        let created = live_buffers(device) - buffers;
        renderer.submit(queue, encoder);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let words: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
        readback.unmap();
        assert_eq!(words, [[frame; 4], [frame + 100; 4]].concat(), "frame {frame}: each write before the copy after it");
        let stats = renderer.uploads.frame_write_stats();
        let staged = u64::from(stats.direct == 0);
        assert_eq!(stats.chunks, staged, "frame {frame}: the writes share one chunk, reused once the GPU is done");
        if frame >= 2 {
            assert_eq!(created, 0, "frame {frame} created buffers");
            // A queue write allocates 6 or 7 times; the first command in a new encoder grows its list once.
            assert!(first <= 1 && second == 0, "frame {frame}: the writes allocated {first} and {second} times");
        }
    }
}

#[test]
fn a_frame_with_nothing_to_upload_never_touches_the_staging() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    renderer.upload_images(device, queue, &mut ImageAtlas::default());
    assert!(renderer.uploads.staging.is_none(), "no images, no staging");
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: None, size: 16, usage: wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.write_buffer(device, queue, &mut encoder, &uniforms, 0, bytemuck::cast_slice(&[1u32; 4]));
    let writes = renderer.uploads.frame_writes.clone().unwrap();
    assert!(writes.busy() || writes.stats().direct == 1, "recorded, its chunk still mapped, or written in place");
    renderer.before_submit(&mut encoder);
    assert!(!writes.busy(), "sealed into the frame");
    let flushes = writes.stats().flushes;
    renderer.before_submit(&mut encoder);
    assert_eq!(writes.stats().flushes, flushes);
    queue.submit([encoder.finish()]);
}

/// A compositor asking for `IMMEDIATES` gets the adapter's immediate size and can colour a
/// draw with `set_immediates`, no buffer written.
#[test]
fn a_device_asked_for_immediates_draws_with_them() {
    let Ok(gpu) = Gpu::try_with_features(Gpu::instance(), None, wgpu::Features::IMMEDIATES) else { return };
    let device = &gpu.device;
    if !device.features().contains(wgpu::Features::IMMEDIATES) { return; }
    assert!(device.limits().max_immediate_size >= 16, "the limit came with the feature");
    let plain = Gpu::headless();
    if crate::gpu::backend::native(&plain.device) {
        assert!(plain.device.features().contains(wgpu::Features::IMMEDIATES), "free on the native backend, so always there");
    } else {
        assert_eq!(plain.device.limits().max_immediate_size, 0, "only when asked");
    }
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl("
            // A struct: naga's HLSL output has no immediates of other types (wgpu#5683).
            struct Fill { colour: vec4<f32> }
            var<immediate> fill: Fill;
            @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                return vec4<f32>(vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
            }
            @fragment fn fs() -> @location(0) vec4<f32> { return fill.colour; }
        ".into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[], immediate_size: 16 });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&layout),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState {
            module: &shader, entry_point: Some("fs"), compilation_options: Default::default(),
            targets: &[Some(OFFSCREEN_FORMAT.into())],
        }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let target = Offscreen::new(device, SIZE.0, SIZE.1);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_immediates(0, bytemuck::cast_slice(&[1.0f32, 0.0, 0.0, 1.0]));
        pass.draw(0..3, 0..1);
    }
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    assert_eq!(pixel(&target.read(device), 10, 10), RED);
}

#[test]
fn same_finds_one_changed_bit_in_any_word() {
    let a = scene(0);
    assert!(same(&a, &a.clone()));
    for word in 0..size_of::<Instance>() / 4 {
        let mut b = a.clone();
        let words: &mut [u32] = bytemuck::cast_slice_mut(&mut b[a.len() - 1..]);
        words[word] ^= 1 << 31;
        assert!(!same(&a, &b), "missed a change in word {word}");
    }
}

/// Lists at any 4-byte offset compare, not only 8-aligned ones: a stack array or a Vec may land
/// either way.
#[test]
fn same_takes_lists_at_any_word_offset() {
    let n = size_of::<Instance>() / 4;
    let words: Vec<u32> = (0..3 * n as u32 + 1).collect();
    let mut other = words.clone();
    for skip in [0, 1] {
        let a: &[Instance] = bytemuck::cast_slice(&words[skip..skip + 3 * n]);
        assert!(same(a, a), "offset {skip}");
        other[skip + 2 * n] ^= 1;
        assert!(!same(a, bytemuck::cast_slice(&other[skip..skip + 3 * n])), "offset {skip}");
        other[skip + 2 * n] ^= 1;
    }
}

/// Plain rects between images and glyphs keep the batch they're in; only image and glyph pages
/// start one.
#[test]
fn batches_break_only_where_the_bound_page_changes() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let with = |kind: f32, page: f32| Instance { params: [page, kind, 0.0, 0.0], ..square(0.0, 0.0) };
    let glyph = |page: f32| Instance { params: [0.0, KIND_GLYPH, page, 0.0], ..square(0.0, 0.0) };
    let instances = [
        square(0.0, 0.0), with(KIND_IMAGE, 1.0), square(0.0, 0.0), with(KIND_SHADOW, 0.0),
        glyph(2.0), with(KIND_GRADIENT, 0.0), glyph(0.0), square(0.0, 0.0), with(KIND_IMAGE, 1.0),
    ];
    renderer.prepare(device, queue, &mut text.atlas, &instances, SIZE);
    let batches: Vec<_> = renderer.image_batches.iter().map(|(range, bind)| (range.clone(), match bind {
        BatchBind::Image(page) => ('i', *page),
        BatchBind::Glyph(page) => ('g', *page),
    })).collect();
    assert_eq!(batches, [(0..1, ('i', 0)), (1..4, ('i', 1)), (4..6, ('g', 1)), (6..8, ('i', 0)), (8..9, ('i', 1))]);
}

/// Frames whose instances only move keep their batches, and their effects see where they moved;
/// a changed `params`, a longer list or a lost staging batches again.
#[test]
fn only_a_changed_params_or_length_batches_again() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let frame = |x: f32, page: f32, extra: usize| {
        let mut list = vec![square(x, 0.0), Instance { params: [page, KIND_IMAGE, 0.0, 0.0], ..square(x, 8.0) },
            Instance { params: [0.0, KIND_EFFECT, 0.0, 0.0], ..square(x, 16.0) }];
        list.extend((0..extra).map(|i| square(i as f32, 24.0)));
        list
    };
    let mut prepare = |renderer: &mut Renderer, list: &[Instance]| {
        let runs = renderer.uploads.batch_runs;
        renderer.prepare(device, queue, &mut text.atlas, list, SIZE);
        let pages: Vec<_> = renderer.image_batches.iter().map(|(r, b)| (r.clone(), matches!(b, BatchBind::Image(1)))).collect();
        (renderer.uploads.batch_runs - runs, pages, renderer.effects.iter().map(|(at, i)| (*at, i.rect[0])).collect::<Vec<_>>())
    };
    assert_eq!(prepare(&mut renderer, &frame(0.0, 1.0, 0)), (1, vec![(0..1, false), (1..3, true)], vec![(2, 0.0)]));
    assert_eq!(prepare(&mut renderer, &frame(5.0, 1.0, 0)), (0, vec![(0..1, false), (1..3, true)], vec![(2, 5.0)]), "moved only");
    assert_eq!(prepare(&mut renderer, &frame(5.0, 0.0, 0)), (1, vec![(0..3, false)], vec![(2, 5.0)]), "image page changed");
    assert_eq!(prepare(&mut renderer, &frame(5.0, 0.0, 2)).0, 1, "longer");
    renderer.uploads.forget_instances();
    assert_eq!(prepare(&mut renderer, &frame(5.0, 0.0, 2)).0, 1, "forgotten");
    assert_eq!(prepare(&mut renderer, &[]), (1, vec![], vec![]), "emptied");
}
