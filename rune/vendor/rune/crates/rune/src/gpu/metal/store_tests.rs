use super::bench::{clear, gpu};
use super::resource::MetalBuffer;
use super::*;

fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage, mapped_at_creation: false })
}

fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let out = slice.get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    out
}

/// Spares `buffer` holds and the swaps it made.
fn swaps(buffer: &wgpu::Buffer) -> (usize, u64) { buffer.as_custom::<MetalBuffer>().unwrap().store.swaps() }

fn live(device: &wgpu::Device) -> isize { device.get_internal_counters().hal.buffers.read() }

/// Each frame writes parts of a buffer while the last frame's copy of it is recorded but not yet
/// submitted, so every frame swaps storage: each copy still reads what was written before it,
/// and once warm a frame makes no buffers.
#[test]
fn writes_while_the_gpu_reads_swap_storage_and_land_in_order() {
    use wgpu::BufferUsages as B;
    const WORDS: usize = 64;
    const FRAMES: usize = 24;
    let (device, queue) = gpu();
    let source = buffer(&device, (WORDS * 4) as u64, B::COPY_SRC | B::COPY_DST);
    let out = buffer(&device, (WORDS * 4 * FRAMES) as u64, B::COPY_DST | B::MAP_READ);
    let mut model = [7u32; WORDS];
    assert!(write_in_place(&source, 0, bytemuck::cast_slice(&model)));
    let (mut want, mut recorded, mut buffers) = (Vec::new(), None, 0);
    for frame in 0..FRAMES {
        if frame == 4 { buffers = live(&device); }
        // Up to more writes than the store's log remembers, over different spans.
        for k in 0..1 + frame % 11 {
            let at = (frame * 5 + k * 13) % (WORDS - 4);
            let words = [(frame * 100 + k) as u32; 4];
            model[at..at + 4].copy_from_slice(&words);
            assert!(write_in_place(&source, (at * 4) as u64, bytemuck::cast_slice(&words)));
        }
        want.extend_from_slice(&model);
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&source, 0, &out, (frame * WORDS * 4) as u64, (WORDS * 4) as u64);
        if let Some(last) = recorded.replace(encoder.finish()) {
            let index = queue.submit([last]);
            device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).unwrap();
        }
    }
    queue.submit(recorded);
    assert_eq!(live(&device), buffers, "warm frames made no buffers");
    let (spares, swapped) = swaps(&source);
    assert_eq!(swapped, FRAMES as u64 - 1, "each frame but the first swapped");
    assert!(spares <= 2, "{spares} spares");
    let got = read(&device, &out);
    for (frame, (got, want)) in got.chunks(WORDS * 4).zip(want.chunks(WORDS)).enumerate() {
        assert_eq!(got, bytemuck::cast_slice::<u32, u8>(want), "frame {frame}");
    }
}

/// A frame that writes the whole buffer while the GPU reads it swaps in a spare and copies
/// nothing forward, and a partial write after it still copies what it leaves: every frame reads
/// exactly what was written by then.
#[test]
fn a_partial_write_after_a_whole_one_reads_the_right_bytes() {
    use wgpu::BufferUsages as B;
    const WORDS: usize = 64;
    let (device, queue) = gpu();
    let source = buffer(&device, (WORDS * 4) as u64, B::COPY_SRC | B::COPY_DST);
    let frames: [&[(usize, usize)]; 6] = [&[(0, WORDS)], &[(0, WORDS)], &[(8, 12)], &[(0, WORDS)], &[(60, 64), (0, 2)], &[(30, 31)]];
    let out = buffer(&device, (WORDS * 4 * frames.len()) as u64, B::COPY_DST | B::MAP_READ);
    let store = &source.as_custom::<MetalBuffer>().unwrap().store;
    let (mut model, mut want, mut recorded) = ([0u32; WORDS], Vec::new(), None);
    for (frame, writes) in frames.iter().enumerate() {
        let copied = store.copied();
        for (k, &(a, b)) in writes.iter().enumerate() {
            let words = vec![(frame * 100 + k + 1) as u32; b - a];
            model[a..b].copy_from_slice(&words);
            assert!(write_in_place(&source, (a * 4) as u64, bytemuck::cast_slice(&words)));
        }
        if writes[..] == [(0, WORDS)] { assert_eq!(store.copied(), copied, "frame {frame} wrote everything, so copied nothing"); }
        want.extend_from_slice(&model);
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&source, 0, &out, (frame * WORDS * 4) as u64, (WORDS * 4) as u64);
        if let Some(last) = recorded.replace(encoder.finish()) {
            let index = queue.submit([last]);
            device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).unwrap();
        }
    }
    queue.submit(recorded);
    assert_eq!(swaps(&source).1, frames.len() as u64 - 1, "each frame but the first swapped");
    assert!(store.copied() > 0, "partial writes copied what they left");
    let got = read(&device, &out);
    for (frame, (got, want)) in got.chunks(WORDS * 4).zip(want.chunks(WORDS)).enumerate() {
        assert_eq!(got, bytemuck::cast_slice::<u32, u8>(want), "frame {frame}");
    }
}

const FILL: &str = "
@group(0) @binding(0) var<uniform> color: vec4<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return color; }
";

/// A write that swaps a buffer a pass has bound reaches the pass's later draws, though the
/// group isn't set again: the draw before it reads the old color, the one after the new.
#[test]
fn a_pass_rebinds_a_buffer_swapped_under_it() {
    let (device, queue) = gpu();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(FILL.into()) });
    let groups = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[wgpu::BindGroupLayoutEntry {
        binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, count: None,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
    }] });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&groups)], immediate_size: 0 });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())] }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let color = buffer(&device, 16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &groups,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: color.as_entire_binding() }] });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: 64, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    assert!(write_in_place(&color, 0, bytemuck::cast_slice(&[1.0f32, 0.0, 0.0, 1.0])));
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = clear(&mut encoder, &view);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_scissor_rect(0, 0, 32, 1);
        pass.draw(0..3, 0..1);
        assert!(write_in_place(&color, 0, bytemuck::cast_slice(&[0.0f32, 0.0, 1.0, 1.0])));
        assert_eq!(swaps(&color).1, 1, "the pass held the storage, so the write swapped it");
        pass.set_scissor_rect(32, 0, 32, 1);
        pass.draw(0..3, 0..1);
    }
    let out = buffer(&device, 256, wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
    encoder.copy_texture_to_buffer(target.as_image_copy(), wgpu::TexelCopyBufferInfo {
        buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: None },
    }, target.size());
    queue.submit([encoder.finish()]);
    let pixels = read(&device, &out);
    assert_eq!(pixels[..4], [255, 0, 0, 255], "the first draw read the old color");
    assert_eq!(pixels[32 * 4..32 * 4 + 4], [0, 0, 255, 255], "the second the new");
}

/// A queue write lands ahead of the next submit, so an encoder recorded before it and submitted
/// after reads it, as on wgpu: written in place only while nothing recorded uses the buffer.
#[test]
fn a_queue_write_reaches_an_encoder_recorded_before_it() {
    use wgpu::BufferUsages as B;
    let (device, queue) = gpu();
    let source = buffer(&device, 16, B::COPY_SRC | B::COPY_DST);
    let out = buffer(&device, 32, B::COPY_DST | B::MAP_READ);
    queue.write_buffer(&source, 0, bytemuck::cast_slice(&[1u32; 4]));
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&source, 0, &out, 0, 16);
    let early = encoder.finish();
    queue.write_buffer(&source, 0, bytemuck::cast_slice(&[2u32; 4]));
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&source, 0, &out, 16, 16);
    queue.submit([early, encoder.finish()]);
    let words: Vec<u32> = bytemuck::cast_slice(&read(&device, &out)).to_vec();
    assert_eq!(words, [[2u32; 4], [2; 4]].concat());
}

/// A queue write to a buffer only submitted work reads swaps in a spare instead of staging a copy
/// of its own, so the frame after it stays one command buffer, and the submitted work still
/// reads the old bytes.
#[test]
fn a_queue_write_behind_submitted_work_swaps_rather_than_stages() {
    use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLEvent, MTLSharedEvent};
    use wgpu::BufferUsages as B;
    let (device, queue) = gpu();
    let shared = device.as_custom::<MetalDevice>().unwrap().shared.clone();
    // Holds the GPU until the end, so the first copy is still waiting when the write comes.
    let gate = shared.device.newSharedEvent().unwrap();
    let wait = shared.command_buffer();
    wait.encodeWaitForEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&*gate), 1);
    wait.commit();
    let source = buffer(&device, 16, B::COPY_SRC | B::COPY_DST);
    let out = buffer(&device, 32, B::COPY_DST | B::MAP_READ);
    let copy = |at| {
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&source, 0, &out, at, 16);
        encoder.finish()
    };
    queue.write_buffer(&source, 0, bytemuck::cast_slice(&[1u32; 4]));
    queue.submit([copy(0)]);
    let before = shared.state.lock().last;
    queue.write_buffer(&source, 0, bytemuck::cast_slice(&[2u32; 4]));
    queue.submit([copy(16)]);
    assert_eq!(shared.state.lock().last, before + 1, "no staged copy of its own");
    assert_eq!(swaps(&source).1, 1, "the write swapped storage under the waiting copy");
    gate.setSignaledValue(1);
    let words: Vec<u32> = bytemuck::cast_slice(&read(&device, &out)).to_vec();
    assert_eq!(words, [[1u32; 4], [2; 4]].concat());
}
