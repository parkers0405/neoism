//! Filling atlas masks on the GPU (`text::atlas_fill`): each queued mask is a Windfoil glyph
//! instance drawn into its page, R8, replacing what was there, by the uber shader that draws
//! moving text from outlines, so a mask and its glyph in motion share one coverage function.

use super::*;
use crate::text::QueuedFill;

/// The fill pass's pipeline and what it draws with: the outline store, stand-ins for the atlas
/// and images (a page can't be read while it's drawn into), and a page-sized projection.
pub(super) struct FillPass {
    pipeline: wgpu::RenderPipeline,
    projection: wgpu::Buffer,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
}

/// What a fill pass binds besides its own buffers.
pub(super) struct FillBindings<'a> {
    pub layout: &'a wgpu::BindGroupLayout,
    pub globals: &'a wgpu::Buffer,
    pub outlines: &'a wgpu::Texture,
    pub stand_in: &'a wgpu::Texture,
}

impl FillPass {
    pub(super) fn new(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &dxbc::Shaders, bindings: FillBindings<'_>) -> FillPass {
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("atlas fill"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &shader.vertex,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(metallib::uber_vertex_layout())],
            },
            primitive: metallib::uber_primitive(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader.fragment,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let projection = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("atlas fill projection"), size: 16, usage: wgpu::BufferUsages::UNIFORM, mapped_at_creation: true,
        });
        let size = ATLAS_SIZE as f32;
        if let Ok(mut range) = projection.slice(..).get_mapped_range_mut() {
            range.copy_from_slice(bytemuck::cast_slice(&[0.0f32, 0.0, size, size]));
        }
        projection.unmap();
        let bind = Self::bind(device, &bindings, &projection);
        let capacity = 64;
        FillPass { pipeline, projection, bind, instances: Renderer::instance_buffer(device, capacity), capacity }
    }

    fn bind(device: &wgpu::Device, b: &FillBindings<'_>, projection: &wgpu::Buffer) -> wgpu::BindGroup {
        Renderer::bind_group(device, b.layout, b.globals, Some(b.outlines), b.stand_in, b.stand_in, Some(projection))
    }

    /// Binds a new outline texture.
    pub(super) fn rebind(&mut self, device: &wgpu::Device, bindings: FillBindings<'_>) {
        self.bind = Self::bind(device, &bindings, &self.projection);
    }

    /// Draws `fills` into `page`, ahead of anything submitted after.
    pub(super) fn run(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, pages: &[(&wgpu::Texture, Vec<QueuedFill>)]) {
        let count: usize = pages.iter().map(|(_, f)| f.len()).sum();
        if count == 0 {
            return;
        }
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            self.instances = Renderer::instance_buffer(device, self.capacity);
        }
        let instances: Vec<Instance> = pages.iter().flat_map(|(_, fills)| fills.iter().map(instance)).collect();
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("atlas fill") });
        let mut first = 0u32;
        for (texture, fills) in pages {
            let view = texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("atlas fill"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            let last = first + fills.len() as u32;
            pass.draw(0..4, first..last);
            first = last;
        }
        queue.submit([encoder.finish()]);
    }
}

/// A queued fill as the Windfoil glyph instance that draws it, over its mask and one-texel border
/// (which comes out clear), at zoom 1 with no clip.
fn instance(q: &QueuedFill) -> Instance {
    let [x, y, w, h] = q.rect.map(|v| v as f32);
    let f = q.fill;
    Instance {
        rect: [x - 1.0, y - 1.0, w + 2.0, h + 2.0],
        radii: [x + f.pen[0], y + f.pen[1], f.scale, 1.0],
        fill: [1.0; 4],
        border: [f.record as f32, f.hint as f32, f.radius, 0.0],
        params: [0.0, KIND_GLYPH, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

impl Renderer {
    /// A glyph atlas page; a Windfoil renderer also draws into its pages (`FillPass`).
    pub(super) fn atlas_texture(device: &wgpu::Device, filled: bool) -> wgpu::Texture {
        let render = if filled { wgpu::TextureUsages::RENDER_ATTACHMENT } else { wgpu::TextureUsages::empty() };
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | render,
            view_formats: &[],
        })
    }

    /// Runs the atlas's queued fills (after this frame's row uploads, which they draw over).
    pub(super) fn fill_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, staging: &rune_staging::Staging, atlas: &mut Atlas) {
        let variant::Uber::Windfoil { fill, .. } = &mut self.uber else { return };
        let fills = atlas.take_fills();
        if fills.is_empty() {
            return;
        }
        // Rows first: a fill draws over the zeros a row upload brings.
        staging.submit();
        let pages: Vec<(&wgpu::Texture, Vec<QueuedFill>)> = fills.into_iter().filter_map(|(page, f)| {
            let texture = if page == 0 { Some(&self.atlas) } else { self.atlas_pages.get(page - 1).map(|p| &p.0) };
            texture.map(|t| (t, f))
        }).collect();
        fill.run(device, queue, &pages);
    }
}
