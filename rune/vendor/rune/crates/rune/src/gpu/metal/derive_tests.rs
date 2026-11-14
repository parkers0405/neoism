use wgpu::util::DeviceExt;

use super::bench::{clear, gpu};

fn wgpu_metal() -> (wgpu::Device, wgpu::Queue) {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::METAL;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    pollster::block_on(adapter.request_device(&Default::default())).unwrap()
}

/// Group 0 is unused; group 1 holds a uniform both stages read, a texture and a sampler.
const DRAW: &str = r#"
struct U { tint: vec4<f32>, shift: vec4<f32> };
@group(1) @binding(0) var<uniform> u: U;
@group(1) @binding(2) var tex: texture_2d<f32>;
@group(1) @binding(3) var smp: sampler;
struct O { @builtin(position) p: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> O {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: O;
    o.p = vec4<f32>(uv * 2.0 - 1.0 + u.shift.xy, 0.0, 1.0);
    o.uv = uv;
    return o;
}
@fragment fn fs(i: O) -> @location(0) vec4<f32> { return textureSample(tex, smp, i.uv) * u.tint; }
"#;

const SUM: &str = r#"
@group(0) @binding(0) var<storage, read> input: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;
@compute @workgroup_size(4) fn sum(@builtin(global_invocation_id) id: vec3<u32>) {
    output[id.x] = input[id.x] * 3u + 1u;
}
"#;

const SIZE: u32 = 64;

fn draw(device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("derived"), source: wgpu::ShaderSource::Wgsl(DRAW.into()) });
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("derived"),
        layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None, contents: bytemuck::cast_slice(&[1.0f32, 0.5, 0.25, 1.0, 0.25, -0.25, 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM,
    });
    let texels: Vec<u8> = (0..16u8).flat_map(|i| [i * 16, 255 - i * 16, 128, 255]).collect();
    let source = device.create_texture_with_data(queue, &wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
    }, Default::default(), &texels);
    let sampler = device.create_sampler(&Default::default());
    let view = source.create_view(&Default::default());
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let empty = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0), entries: &[] });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let out = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (SIZE * SIZE * 4) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let target_view = target.create_view(&Default::default());
        let mut pass = clear(&mut encoder, &target_view);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &empty, &[]);
        pass.set_bind_group(1, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(target.as_image_copy(), wgpu::TexelCopyBufferInfo {
        buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: Some(SIZE) },
    }, target.size());
    queue.submit([encoder.finish()]);
    read(device, &out)
}

fn sum(device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sum"), source: wgpu::ShaderSource::Wgsl(SUM.into()) });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("sum"), layout: None, module: &module, entry_point: None, compilation_options: Default::default(), cache: None,
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None, contents: bytemuck::cast_slice(&[1u32, 2, 3, 4, 5, 6, 7, 8]), usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(2, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 32);
    queue.submit([encoder.finish()]);
    read(device, &readback)
}

fn read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let out = slice.get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    out
}

/// A pipeline made with `layout: None` derives its layout from its shaders, as wgpu does, and
/// draws the same pixels through the groups `get_bind_group_layout` hands out.
#[test]
fn a_pipeline_without_a_layout_derives_one_as_wgpu_does() {
    let (device, queue) = gpu();
    let (wgpu_device, wgpu_queue) = wgpu_metal();
    let ours = draw(&device, &queue);
    assert!(ours.chunks(4).any(|p| p != [0, 0, 51, 102]), "the triangle drew");
    assert_eq!(ours, draw(&wgpu_device, &wgpu_queue));
    let sums = sum(&device, &queue);
    assert_eq!(bytemuck::cast_slice::<u8, u32>(&sums), [4, 7, 10, 13, 16, 19, 22, 25]);
    assert_eq!(sums, sum(&wgpu_device, &wgpu_queue));
}
