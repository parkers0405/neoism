//! Convert Rune's premultiplied composition to the straight alpha required by
//! wgpu's straight-alpha surfaces and PNG exports. Metal's CAMetalLayer stays premultiplied.
pub(crate) struct AlphaSurface {
    pub view: wgpu::TextureView,
    texture: wgpu::Texture,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    bind: wgpu::BindGroup,
    format: wgpu::TextureFormat,
}

#[cfg(test)]
mod tests {
    #[test]
    fn gpu_straight_alpha_matches_cpu_reference() {
        use crate::render::{Gpu, Offscreen, OFFSCREEN_FORMAT};
        let gpu = Gpu::new(Gpu::instance(), None);
        let alpha = super::AlphaSurface::new(&gpu.device, OFFSCREEN_FORMAT, (4, 4));
        let target = Offscreen::new(&gpu.device, 4, 4);
        for color in [[0.125_f32, 0.25, 0.375, 0.5], [0.0; 4], [0.4, 0.6, 0.1, 1.0]] {
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            {
                let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None, color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &alpha.view, depth_slice: None, resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: color[0] as f64, g: color[1] as f64, b: color[2] as f64, a: color[3] as f64,
                        }), store: wgpu::StoreOp::Store },
                    })], depth_stencil_attachment: None, timestamp_writes: None,
                    occlusion_query_set: None, multiview_mask: None,
                });
            }
            alpha.draw(&mut enc, &target.view);
            target.copy(&mut enc);
            gpu.queue.submit([enc.finish()]);
            let pixels = target.read(&gpu.device);
            let mut reference = [0_u8; 4];
            for i in 0..3 { reference[i] = (crate::color::srgb_encode(color[i]) * 255.0).round() as u8; }
            reference[3] = (color[3] * 255.0).round() as u8;
            crate::tooling::straight_alpha(&mut reference);
            for pixel in pixels.chunks_exact(4) {
                assert!(pixel.iter().zip(reference).all(|(a, b)| a.abs_diff(b) <= 1), "GPU {pixel:?}, CPU {reference:?}");
            }
        }
    }
}
impl AlphaSurface {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("straight alpha"),
            source: wgpu::ShaderSource::Wgsl(r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(points[index], 0.0, 1.0);
}
@fragment fn fs(@builtin(position) point: vec4<f32>) -> @location(0) vec4<f32> {
    let color = textureLoad(source, vec2<i32>(point.xy), 0);
    if color.a > 0.0 { return vec4(color.rgb / color.a, color.a); }
    return vec4(0.0);
}
"#.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("alpha source"), entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("alpha conversion"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("straight alpha"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None, cache: None,
        });
        let (texture, view, bind) = Self::target(device, &layout, format, size);
        Self { texture, view, bind, pipeline, layout, format }
    }
    fn target(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, size: (u32, u32))
        -> (wgpu::Texture, wgpu::TextureView, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("premultiplied composition"),
            size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("alpha source"), layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        });
        (texture, view, bind)
    }
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if (self.texture.width(), self.texture.height()) == (size.0.max(1), size.1.max(1)) { return; }
        (self.texture, self.view, self.bind) = Self::target(device, &self.layout, self.format, size);
    }
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("straight alpha"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
