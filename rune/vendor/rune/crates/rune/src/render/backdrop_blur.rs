//! In-window backdrop blur (`El::backdrop_blur`, `KIND_BACKDROP_BLUR` records). At each record
//! the pass splits (`ActivePass::split`) and copies what is below into the pooled backdrop, a
//! dual-filter pyramid blurs it at half resolution inside the record's rect plus the blur's reach,
//! and a composite quad draws the result inside the record's rounded rect and clip. The pyramid
//! covers only that region, not the window, and grows to the largest one seen. Built on the
//! first record; frames without one never touch it.
use super::*;

/// Backdrop-blurred elements per frame. Elements past this many draw only their fill.
pub const MAX_BACKDROP_BLURS: usize = 4;

/// Pyramid levels below the backdrop, the first at half resolution.
const MIPS: u32 = 7;
/// Uniform slot stride: the largest `min_uniform_buffer_offset_alignment` WebGPU allows.
const SLOT: u64 = 256;
/// Uniform slots per record: one per level for passes that read the pyramid, then the first
/// down pass, which reads the backdrop.
const SLOTS: usize = MIPS as usize + 1;
/// Pyramid sizes are multiples of this many half-resolution texels, so every level halves
/// exactly and small changes in a region's size don't reallocate.
const STEP: u32 = 1 << (MIPS - 1);
/// Standard deviation in device pixels of the pyramid's blur when its up chain starts at level
/// `n` at full weight (`n = 0`: the half-resolution downsample alone). Measured from the
/// filters' line spread; each level doubles it, and a crossfade mixes variances linearly.
const SIGMAS: [f32; MIPS as usize] = [1.58, 4.53, 9.62, 19.51, 39.15, 78.36, 156.2];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Level {
    /// x: share of the coarser level in an up pass.
    w: [f32; 4],
    /// The region's bounds in the source's uv, clamping every tap.
    region: [f32; 4],
    /// Source uv from target uv: `uv * xy + zw`.
    map: [f32; 4],
    /// Where the pyramid sits, in window device pixels: origin xy, extent zw. Level 0 of each
    /// slot carries it for the composite.
    place: [f32; 4],
}

/// One record's pyramid run, prepared with its uniforms.
#[derive(Clone, Copy)]
struct Record {
    at: u32,
    slot: usize,
    /// The up chain starts at this level (at least 1).
    levels: usize,
    /// Window device pixels blurred: the rect plus the blur's reach, inside the viewport.
    region: [f32; 4],
    /// The pyramid's window origin, aligned to a texel of level `levels`.
    origin: [f32; 2],
}

struct Pyramid {
    /// Level 0 in texels; level m is `size >> m`.
    size: [u32; 2],
    down: Vec<wgpu::TextureView>,
    up: Vec<wgpu::TextureView>,
    /// The down pass into level m + 1 reads level m.
    down_binds: Vec<wgpu::BindGroup>,
    /// The up pass into level m reads down level m + 1 (where the chain starts) or up level m + 1.
    up_binds: Vec<(wgpu::BindGroup, Option<wgpu::BindGroup>)>,
    /// Up level 0, what the composite samples.
    result: wgpu::BindGroup,
}

pub(super) struct BackdropBlur {
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    level_layout: wgpu::BindGroupLayout,
    result_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::PipelineLayout,
    composite_shader: wgpu::ShaderModule,
    composites: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    sampler: wgpu::Sampler,
    dummy: wgpu::TextureView,
    uniforms: wgpu::Buffer,
    pyramid: Option<Pyramid>,
    /// The down pass into level 0, reading the pooled backdrop of each format.
    first: Vec<(wgpu::TextureFormat, wgpu::BindGroup)>,
    records: Vec<Record>,
}

/// The up chain's start level `n` (1..=top) and the share of level `n` that gives `sigma`.
fn levels(sigma: f32, top: usize) -> (usize, f32) {
    let top = top.clamp(1, SIGMAS.len() - 1);
    let sigma = sigma.clamp(SIGMAS[0], SIGMAS[top]);
    let n = (1..=top).find(|&n| sigma <= SIGMAS[n]).unwrap_or(top);
    let (a, b) = (SIGMAS[n - 1], SIGMAS[n]);
    (n, ((sigma * sigma - a * a) / (b * b - a * a)).clamp(0.0, 1.0))
}

impl BackdropBlur {
    pub(super) fn new(device: &wgpu::Device, uber: &wgpu::BindGroupLayout) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let levels_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Level>() as u64),
            },
            count: None,
        };
        let level_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop blur level"),
            entries: &[texture(0), texture(1), sampler_entry(2), levels_entry(3, wgpu::ShaderStages::FRAGMENT)],
        });
        let result_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop blur result"),
            entries: &[texture(0), sampler_entry(1), levels_entry(2, wgpu::ShaderStages::FRAGMENT)],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../backdrop_blur.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop blur"),
            bind_group_layouts: &[Some(&level_layout)],
            immediate_size: 0,
        });
        let pass = |entry| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backdrop blur"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: layers::FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let (down, up) = (pass("fs_down"), pass("fs_up"));
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop blur composite"),
            source: wgpu::ShaderSource::Wgsl(format!("{CLIP_WGSL}\n{}", include_str!("../backdrop_composite.wgsl")).into()),
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop blur composite"),
            bind_group_layouts: &[Some(uber), Some(&result_layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("backdrop blur"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("backdrop blur stand-in"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: layers::FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }).create_view(&Default::default());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backdrop blur levels"),
            size: (MAX_BACKDROP_BLURS * SLOTS) as u64 * SLOT,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        BackdropBlur {
            down, up, level_layout, result_layout, composite_layout, composite_shader,
            composites: Vec::new(), sampler, dummy, uniforms, pyramid: None, first: Vec::new(), records: Vec::new(),
        }
    }

    fn levels_binding(&self) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.uniforms,
            offset: 0,
            size: wgpu::BufferSize::new(std::mem::size_of::<Level>() as u64),
        })
    }

    fn level_bind(&self, device: &wgpu::Device, src: &wgpu::TextureView, fine: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop blur level"),
            layout: &self.level_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(fine) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: self.levels_binding() },
            ],
        })
    }

    /// Points the first down pass at the pooled backdrop of `format` (new or resized).
    pub(super) fn bind_backdrop(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, view: &wgpu::TextureView) {
        let bind = self.level_bind(device, view, &self.dummy);
        match self.first.iter_mut().find(|(f, _)| *f == format) {
            Some(slot) => slot.1 = bind,
            None => self.first.push((format, bind)),
        }
    }

    pub(super) fn ensure_composite(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.composites.iter().any(|(f, _)| *f == format) { return; }
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backdrop blur composite"),
            layout: Some(&self.composite_layout),
            vertex: wgpu::VertexState {
                module: &self.composite_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(metallib::uber_vertex_layout())],
            },
            primitive: metallib::uber_primitive(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.composite_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(Blend::SourceOver.state()),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.composites.push((format, pipeline));
    }

    /// Makes the pyramid's level 0 at least `need` texels, grown in `STEP`s and never past `cap`.
    fn ensure_pyramid(&mut self, device: &wgpu::Device, need: [u32; 2], cap: [u32; 2]) {
        let have = self.pyramid.as_ref().map_or([0; 2], |p| p.size);
        let size = [0, 1].map(|k| have[k].min(cap[k]).max(need[k]));
        if size == have { return; }
        let mips = MIPS;
        let chain = |label| {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: layers::FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            (0..mips)
                .map(|m| t.create_view(&wgpu::TextureViewDescriptor { base_mip_level: m, mip_level_count: Some(1), ..Default::default() }))
                .collect::<Vec<_>>()
        };
        let (down, up) = (chain("backdrop blur down"), chain("backdrop blur up"));
        let m = down.len();
        let down_binds = (1..m).map(|k| self.level_bind(device, &down[k - 1], &self.dummy)).collect();
        let up_binds = (0..m.saturating_sub(1))
            .map(|k| (self.level_bind(device, &down[k + 1], &down[k]), (k + 2 < m).then(|| self.level_bind(device, &up[k + 1], &down[k]))))
            .collect();
        let result = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop blur result"),
            layout: &self.result_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&up[0]) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.levels_binding() },
            ],
        });
        self.pyramid = Some(Pyramid { size, down, up, down_binds, up_binds, result });
    }

    /// Records the pyramid passes for `record`, reading the backdrop of `format`.
    fn encode(&self, encoder: &mut wgpu::CommandEncoder, format: wgpu::TextureFormat, record: &Record) {
        let (Some(pyramid), Some((_, first))) = (&self.pyramid, self.first.iter().find(|(f, _)| *f == format)) else { return };
        let [pw, ph] = pyramid.size;
        let ([x0, y0, x1, y1], [ox, oy]) = (record.region, record.origin);
        // Each level covers the region plus two texels, the reach of its taps into the next.
        let scissor = |m: usize| {
            let k = (2u32 << m) as f32;
            let (w, h) = ((pw >> m).max(1), (ph >> m).max(1));
            let x = (((x0 - ox) / k).floor() as u32).saturating_sub(2).min(w - 1);
            let y = (((y0 - oy) / k).floor() as u32).saturating_sub(2).min(h - 1);
            let r = (((x1 - ox) / k).ceil() as u32 + 2).min(w);
            let b = (((y1 - oy) / k).ceil() as u32 + 2).min(h);
            (x, y, r.max(x + 1) - x, b.max(y + 1) - y)
        };
        let mut run = |pipeline: &wgpu::RenderPipeline, dst: &wgpu::TextureView, bind: &wgpu::BindGroup, m: usize, uniform: usize| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("backdrop blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
                    depth_slice: None,
                    resolve_target: None,
                    // Taps never read past the scissor, so nothing outside it needs loading.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let (x, y, w, h) = scissor(m);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[((record.slot * SLOTS + uniform) as u64 * SLOT) as u32]);
            pass.draw(0..3, 0..1);
        };
        let n = record.levels;
        for m in 0..=n {
            if m == 0 { run(&self.down, &pyramid.down[0], first, 0, MIPS as usize) } else { run(&self.down, &pyramid.down[m], &pyramid.down_binds[m - 1], m, m) }
        }
        for m in (0..n).rev() {
            let (from_down, from_up) = &pyramid.up_binds[m];
            run(&self.up, &pyramid.up[m], if m + 1 == n { from_down } else { from_up.as_ref().unwrap() }, m, m);
        }
    }
}

impl Renderer {
    pub(super) fn has_backdrop_blur(&self) -> bool {
        self.backdrop_blur.as_ref().is_some_and(|b| !b.records.is_empty())
    }

    /// Builds what this frame's backdrop blur records need and writes their pyramid uniforms.
    pub(super) fn prepare_backdrop_blur(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, (vw, vh): (u32, u32)) {
        let any = self.effects.iter().any(|(_, i)| i.params[1] == KIND_BACKDROP_BLUR);
        if let Some(b) = &mut self.backdrop_blur { b.records.clear(); }
        if !any || vw == 0 || vh == 0 { return; }
        let staging = self.uploads.staging(device, queue);
        if self.backdrop_blur.is_none() {
            let mut blur = BackdropBlur::new(device, &self.bind_layout);
            for bd in &self.backdrops { blur.bind_backdrop(device, bd.format, &bd.view); }
            self.backdrop_blur = Some(blur);
        }
        let Some(blur) = &mut self.backdrop_blur else { return };
        blur.ensure_composite(device, self.format);
        for (format, _) in &self.extra { blur.ensure_composite(device, *format); }
        if self.layers.targets.iter().any(Option::is_some) { blur.ensure_composite(device, layers::FORMAT); }
        let viewport = [vw as f32, vh as f32];
        let mut placed = Vec::new();
        let mut need = [STEP; 2];
        let records = self.effects.iter().filter(|(_, i)| i.params[1] == KIND_BACKDROP_BLUR).take(MAX_BACKDROP_BLURS);
        for (at, i) in records {
            let sigma = i.params[0];
            let (n, share) = levels(sigma, MIPS as usize - 1);
            let reach = sigma.min(SIGMAS[n]) * 3.0 + 2.0;
            let region = [
                (i.rect[0] - reach).max(0.0),
                (i.rect[1] - reach).max(0.0),
                (i.rect[0] + i.rect[2] + reach).min(viewport[0]),
                (i.rect[1] + i.rect[3] + reach).min(viewport[1]),
            ];
            if region[2] <= region[0] || region[3] <= region[1] { continue; }
            // On level n's texel grid, so the blur stays put while content scrolls under it.
            let align = (2u32 << n) as f32;
            let origin = [(region[0] / align).floor() * align, (region[1] / align).floor() * align];
            for k in 0..2 { need[k] = need[k].max((((region[k + 2] - origin[k]) / 2.0).ceil() as u32).next_multiple_of(STEP)); }
            placed.push((*at, n, share, region, origin));
        }
        if placed.is_empty() { return; }
        // The window plus the most an aligned origin can sit before it.
        let cap = [vw, vh].map(|v| (v.div_ceil(2) + STEP).next_multiple_of(STEP));
        blur.ensure_pyramid(device, need, cap);
        let Some(pyramid) = &blur.pyramid else { return };
        let extent = [pyramid.size[0] as f32 * 2.0, pyramid.size[1] as f32 * 2.0];
        let mut levels_data = vec![0u8; MAX_BACKDROP_BLURS * SLOTS * SLOT as usize];
        for (slot, (at, n, share, region, origin)) in placed.into_iter().enumerate() {
            let inside = [(region[0] - origin[0]) / extent[0], (region[1] - origin[1]) / extent[1],
                (region[2] - origin[0]) / extent[0], (region[3] - origin[1]) / extent[1]];
            let place = [origin[0], origin[1], extent[0], extent[1]];
            for m in 0..SLOTS {
                // The first down pass reads the window-sized backdrop; the rest read the pyramid.
                let (region, map) = if m == MIPS as usize {
                    ([region[0] / viewport[0], region[1] / viewport[1], region[2] / viewport[0], region[3] / viewport[1]],
                        [extent[0] / viewport[0], extent[1] / viewport[1], origin[0] / viewport[0], origin[1] / viewport[1]])
                } else {
                    (inside, [1.0, 1.0, 0.0, 0.0])
                };
                let level = Level { w: [if m + 1 == n { share } else { 1.0 }, 0.0, 0.0, 0.0], region, map, place };
                let at = (slot * SLOTS + m) * SLOT as usize;
                levels_data[at..at + std::mem::size_of::<Level>()].copy_from_slice(bytemuck::bytes_of(&level));
            }
            blur.records.push(Record { at, slot, levels: n, region, origin });
        }
        let used = blur.records.last().map_or(0, |r| (r.slot + 1) * SLOTS * SLOT as usize);
        staging.write_buffer(&blur.uniforms, 0, &levels_data[..used]);
    }

    /// Draws the backdrop blur record at `at`: splits the pass to copy and blur what is below,
    /// then composites the blur inside the record's rounded rect and clip. Draws nothing (the
    /// element shows only its fill) when the pass can't be split or the record wasn't prepared.
    pub(super) fn draw_backdrop_blur(&self, pass: &mut ActivePass<'_, '_>, format: wgpu::TextureFormat, at: u32, instance: &Instance, target: Option<usize>) {
        let Some(blur) = &self.backdrop_blur else { return };
        let Ok(k) = blur.records.binary_search_by_key(&at, |r| r.at) else { return };
        let record = &blur.records[k];
        let (Some((_, composite)), Some(pyramid)) = (blur.composites.iter().find(|(f, _)| *f == format), &blur.pyramid) else { return };
        let (origin, size) = self.target_frame(target);
        let (r, c) = (instance.rect, instance.clip);
        let edge = |v: f32, o: f32, max: u32| (v - o).clamp(0.0, max as f32) as u32;
        let (x0, y0) = (edge(r[0].max(c[0]).floor(), origin[0], size[0]), edge(r[1].max(c[1]).floor(), origin[1], size[1]));
        let (x1, y1) = (edge((r[0] + r[2]).min(c[2]).ceil(), origin[0], size[0]), edge((r[1] + r[3]).min(c[3]).ceil(), origin[1], size[1]));
        if x1 <= x0 || y1 <= y0 { return; }
        let split = pass.split(self, format, origin, size, target.is_some(), Some(record.region), |encoder, backdrop| {
            blur.encode(encoder, backdrop.format, record);
        });
        if split.is_none() { return; }
        let scope = pass.scissor();
        let [sx, sy, sw, sh] = scope.unwrap_or([0, 0, size[0], size[1]]);
        let (x0, y0, x1, y1) = (x0.max(sx), y0.max(sy), x1.min(sx + sw), y1.min(sy + sh));
        if x1 > x0 && y1 > y0 {
            let bind = match target {
                Some(index) => &self.layers.targets[index].as_ref().unwrap().base,
                None => &self.bind_group,
            };
            pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
            pass.set_pipeline(composite);
            pass.set_bind_group(0, bind, &[]);
            pass.set_bind_group(1, &pyramid.result, &[(record.slot * SLOTS) as u32 * SLOT as u32]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..4, at..at + 1);
        }
        pass.set_scissor(scope, size);
    }
}

#[cfg(test)]
#[path = "backdrop_blur_tests.rs"]
mod tests;
