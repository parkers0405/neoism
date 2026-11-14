//! Separable premultiplied Gaussian. Wider kernels use continuously scaled intermediate
//! resolution; the final reconstruction returns to the layer so composite bindings stay stable.
use bytemuck::{Pod, Zeroable};

pub(crate) const MAX_SIGMA: f32 = 64.0;
pub(crate) fn sigma(value: f32) -> f32 { value.max(0.0).min(MAX_SIGMA) }
const TAPS: usize = 128; // centre + ceil(3 * MAX_SIGMA / 2) pairs; padded for Pod arrays

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    meta: [f32; 4], // used width, height, axis (0 horizontal), tap count
    mapping: [f32; 4], // output->input scale xy
    taps: [[f32; 4]; TAPS], // offset, weight; vec4 stride matches WGSL uniform arrays
}

pub(super) struct Pipeline {
    pipelines: [wgpu::RenderPipeline; 5],
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

pub(super) struct Scratch {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    work: Option<(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup)>,
    uniforms: [wgpu::Buffer; 4],
    bindings: [wgpu::BindGroup; 4],
    size: [f32; 2],
    reduced: [f32; 2],
    additive: bool,
    sigma: f32,
    #[cfg(test)]
    pub uniform_writes: usize,
}

impl Pipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer gaussian"), source: wgpu::ShaderSource::Wgsl(include_str!("../layer_blur.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gaussian inputs"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gaussian"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipelines = [(0.0, 0.0), (0.0, 1.0), (2.0, 0.0), (2.0, 1.0), (3.0, 0.0)].map(|(mode, additive)| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("layer gaussian"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &[("MODE", mode), ("ADDITIVE", additive)], ..Default::default() },
                targets: &[Some(wgpu::ColorTargetState { format: super::layers::FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None, cache: None,
        }));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gaussian linear taps"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipelines, layout, sampler }
    }

    pub fn scratch(&self, device: &wgpu::Device, source: &wgpu::Texture) -> Scratch {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer gaussian scratch"), size: source.size(), mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: super::layers::FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let input = source.create_view(&Default::default());
        let uniforms = std::array::from_fn(|_| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gaussian taps"), size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        }));
        let bindings = std::array::from_fn(|axis| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gaussian input"), layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(if axis == 0 { &input } else { &view }) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: uniforms[axis].as_entire_binding() },
            ],
        }));
        Scratch { texture, view, uniforms, bindings, work: None, size: [0.0; 2], reduced: [0.0; 2], additive: false,
            sigma: 0.0, #[cfg(test)] uniform_writes: 0 }
    }

    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, scratch: &Scratch, destination: &wgpu::TextureView) {
        let reduced = scratch.reduced != scratch.size;
        for axis in 0..if reduced { 4 } else { 2 } {
            let (view, bind, size) = if reduced {
                let (_, work, bind) = scratch.work.as_ref().unwrap();
                match axis {
                    0 => (&scratch.view, &scratch.bindings[0], scratch.reduced),
                    1 => (work, &scratch.bindings[1], scratch.reduced),
                    2 => (&scratch.view, bind, scratch.reduced),
                    _ => (destination, &scratch.bindings[3], scratch.size),
                }
            } else { (if axis == 0 { &scratch.view } else { destination }, &scratch.bindings[axis], scratch.size) };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("layer gaussian axis"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            pass.set_viewport(0.0, 0.0, size[0], size[1], 0.0, 1.0);
            let pipeline = if reduced && axis == 3 { 4 }
                else if reduced && axis == 0 { 2 + usize::from(scratch.additive) }
                else { usize::from(axis == 0 && scratch.additive) };
            pass.set_pipeline(&self.pipelines[pipeline]);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

impl Scratch {
    pub fn bytes(&self) -> u64 {
        u64::from(self.texture.width()) * u64::from(self.texture.height()) * 8
            + self.work.as_ref().map_or(0, |(t, _, _)| u64::from(t.width()) * u64::from(t.height()) * 8)
    }
    pub fn passes(&self) -> usize { if self.reduced != self.size { 4 } else { 2 } }

    /// Makes the next `prepare` write the uniforms again.
    pub fn forget_uniforms(&mut self) { self.sigma = f32::NAN; }

    pub fn prepare(&mut self, device: &wgpu::Device, pipeline: &Pipeline, staging: &rune_staging::Staging, size: [f32; 2], sigma: f32, additive: bool) -> bool {
        let sigma = sigma.clamp(0.001, MAX_SIGMA);
        if self.size == size && self.sigma == sigma && self.additive == additive { return false; }
        let reduction = (sigma / 3.0).clamp(1.0, 8.0);
        self.size = size;
        self.sigma = sigma;
        self.additive = additive;
        self.reduced = size.map(|v| (v / reduction).ceil());
        let reduced = self.reduced != size;
        let allocated = reduced && self.work.is_none();
        if allocated {
            // Allocate against the stable primary bucket, not each animated reduced size.
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gaussian reduced work"), size: self.texture.size(), mip_level_count: 1, sample_count: 1,
                dimension: wgpu::TextureDimension::D2, format: super::layers::FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gaussian reduced input"), layout: &pipeline.layout, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&pipeline.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.uniforms[2].as_entire_binding() },
                ],
            });
            self.work = Some((texture, view, bind));
        }
        let scale = [size[0] / self.reduced[0], size[1] / self.reduced[1]];
        for pass in 0usize..if reduced { 4 } else { 2 } {
            let axis = if reduced { pass.saturating_sub(1).min(1) } else { pass };
            let input = if !reduced || pass == 0 { size } else { self.reduced };
            let mode = if !reduced { axis } else { [2, 0, 1, 3][pass] };
            // Subtract the box prefilter + linear reconstruction variance from the Gaussian.
            let variance = (scale[axis] * scale[axis] - 1.0) * 0.25;
            let mut u = kernel((sigma * sigma - variance).sqrt() / scale[axis]);
            u.meta = [input[0], input[1], mode as f32, u.meta[3]];
            u.mapping = match mode {
                2 => [scale[0], scale[1], 0.0, 0.0],
                3 => [1.0 / scale[0], 1.0 / scale[1], 0.0, 0.0],
                _ => [1.0, 1.0, 0.0, 0.0],
            };
            staging.write_buffer(&self.uniforms[pass], 0, bytemuck::bytes_of(&u));
            #[cfg(test)] { self.uniform_writes += 1; }
        }
        allocated
    }
}

fn kernel(sigma: f32) -> Uniforms {
    let radius = (3.0 * sigma).ceil() as usize;
    let weight = |x: usize| (-0.5 * (x as f32 / sigma).powi(2)).exp();
    let sum = 1.0 + 2.0 * (1..=radius).map(weight).sum::<f32>();
    let mut u = Uniforms::zeroed();
    u.taps[0][1] = 1.0 / sum;
    let mut count = 1;
    for x in (1..=radius).step_by(2) {
        let a = weight(x);
        let b = if x < radius { weight(x + 1) } else { 0.0 };
        let w = a + b;
        if w > 0.0 {
            u.taps[count] = [x as f32 + b / w, w / sum, 0.0, 0.0];
            count += 1;
        }
    }
    u.meta[3] = count as f32;
    u
}
