//! GPU backdrop, rounded-window, and blurred-shadow compositing for `--desktop`.

use std::path::Path;

use crate::render::{Gpu, OFFSCREEN_FORMAT};
use crate::tooling::die;

/// Shadows of the key window and of the others: blur sigma, downward offset (points), opacity.
pub(super) const KEY_SHADOW: (f32, f32, f32) = (14.0, 14.0, 0.5);
pub(super) const SHADOW: (f32, f32, f32) = (6.0, 5.0, 0.32);
/// Shadow masks are this many window pixels to a texel.
pub(super) const DOWN: u32 = 4;
/// Largest backdrop image, in pixels per side, decoded.
pub(super) const MAX_BACKDROP: u32 = 16384;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Uniform {
    /// Window top-left and size, in output pixels.
    pub(super) origin: [f32; 2],
    pub(super) size: [f32; 2],
    pub(super) out_size: [f32; 2],
    pub(super) shadow_offset: [f32; 2],
    pub(super) mask_size: [f32; 2],
    pub(super) radius: f32,
    /// Window pixels of shadow mask around the window.
    pub(super) pad: f32,
    /// Blur sigma in mask texels.
    pub(super) sigma: f32,
    pub(super) opacity: f32,
    pub(super) _unused: [f32; 2],
}

const SHADER: &str = r#"
struct W {
    origin: vec2<f32>,
    size: vec2<f32>,
    out_size: vec2<f32>,
    shadow_offset: vec2<f32>,
    mask_size: vec2<f32>,
    radius: f32,
    pad: f32,
    sigma: f32,
    opacity: f32,
    unused: vec2<f32>,
};
@group(0) @binding(0) var<uniform> w: W;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

const DOWN: f32 = 4.0;

fn corner(i: u32) -> vec2<f32> {
    let c = array(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    return c[i];
}
fn clip(p: vec2<f32>) -> vec4<f32> {
    return vec4(p.x / w.out_size.x * 2.0 - 1.0, 1.0 - p.y / w.out_size.y * 2.0, 0.0, 1.0);
}
@vertex fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let points = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(points[i], 0.0, 1.0);
}
@vertex fn vs_window(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return clip(w.origin + corner(i) * w.size);
}
@vertex fn vs_shadow(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return clip(w.origin + w.shadow_offset - vec2(w.pad) + corner(i) * (w.size + 2.0 * w.pad));
}

// Signed distance to the window's rounded rect; p in window pixels.
fn rounded(p: vec2<f32>) -> f32 {
    let half = w.size * 0.5;
    let r = min(w.radius, min(half.x, half.y));
    let q = abs(p - half) - half + vec2(r);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment fn fs_backdrop(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(tex, vec2<i32>(p.xy), 0);
}

// Premultiplied window pixels, cut to the rounded rect.
@fragment fn fs_window(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let local = p.xy - w.origin;
    let color = textureLoad(tex, vec2<i32>(floor(local)), 0);
    return color * clamp(0.5 - rounded(local), 0.0, 1.0);
}

// The window's alpha inside its corners, DOWN x DOWN pixels to a texel.
@fragment fn fs_mask(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let base = floor(p.xy) * DOWN - vec2(w.pad);
    var sum = 0.0;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            let q = base + vec2(f32(x), f32(y));
            if all(q >= vec2(0.0)) && all(q < w.size) {
                sum += textureLoad(tex, vec2<i32>(q), 0).a * clamp(0.5 - rounded(q + 0.5), 0.0, 1.0);
            }
        }
    }
    return vec4(sum / 16.0, 0.0, 0.0, 1.0);
}

fn blur(p: vec2<f32>, axis: vec2<i32>) -> vec4<f32> {
    let c = vec2<i32>(p);
    let dims = vec2<i32>(textureDimensions(tex));
    let sigma = max(w.sigma, 0.5);
    let r = i32(ceil(sigma * 3.0));
    var sum = 0.0;
    var total = 0.0;
    for (var k = -r; k <= r; k++) {
        let q = c + axis * k;
        let weight = exp(-f32(k * k) / (2.0 * sigma * sigma));
        total += weight;
        if all(q >= vec2(0)) && all(q < dims) {
            sum += weight * textureLoad(tex, q, 0).r;
        }
    }
    return vec4(sum / total, 0.0, 0.0, 1.0);
}
@fragment fn fs_blur_x(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return blur(p.xy, vec2(1, 0)); }
@fragment fn fs_blur_y(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return blur(p.xy, vec2(0, 1)); }

@fragment fn fs_shadow(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let m = (p.xy - w.origin - w.shadow_offset + vec2(w.pad)) / DOWN;
    let a = textureSample(tex, samp, m / w.mask_size).r * w.opacity;
    return vec4(0.0, 0.0, 0.0, a);
}
"#;

const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// The backdrop and the pipelines that draw windows and their shadows over it.
pub(super) struct Composite {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    backdrop: wgpu::BindGroup,
    full: wgpu::RenderPipeline,
    window: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    mask: wgpu::RenderPipeline,
    blur: [wgpu::RenderPipeline; 2],
}

impl Composite {
    pub(super) fn new(gpu: &Gpu, backdrop: &Path, size: (u32, u32)) -> Composite {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("desktop"), source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("desktop"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0, visibility: wgpu::ShaderStages::VERTEX | fragment, count: None,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, visibility: fragment, count: None,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: fragment, count: None, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering) },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("desktop"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipeline = |vs: &str, fs: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs), layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None, cache: None,
            })
        };
        let over = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("desktop shadow"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default()
        });
        let pixels = fit_backdrop(backdrop, size).unwrap_or_else(|e| die(&e));
        let (texture, view) = new_texture(device, "desktop backdrop", size, OFFSCREEN_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0 * 4), rows_per_image: Some(size.1) },
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
        let uniform = uniform_buffer(device);
        let backdrop = bind(device, &layout, &uniform, &view, &sampler);
        Composite {
            full: pipeline("vs_full", "fs_backdrop", OFFSCREEN_FORMAT, None),
            window: pipeline("vs_window", "fs_window", OFFSCREEN_FORMAT, over),
            shadow: pipeline("vs_shadow", "fs_shadow", OFFSCREEN_FORMAT, over),
            mask: pipeline("vs_full", "fs_mask", MASK_FORMAT, None),
            blur: [pipeline("vs_full", "fs_blur_x", MASK_FORMAT, None), pipeline("vs_full", "fs_blur_y", MASK_FORMAT, None)],
            layout, sampler, backdrop,
        }
    }

    /// The backdrop, then each window's shadow and pixels, bottom first.
    pub(super) fn draw<'a>(&self, enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, windows: impl Iterator<Item = (&'a Slot, bool)> + Clone) {
        for (slot, _) in windows.clone().filter(|(_, shadow)| *shadow) {
            let passes = [(&self.mask, &slot.window, 0), (&self.blur[0], &slot.masks[0].2, 1), (&self.blur[1], &slot.masks[1].2, 0)];
            for (pipeline, source, into) in passes {
                let mut pass = begin(enc, &slot.masks[into].1, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, source, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        let mut pass = begin(enc, target, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
        pass.set_pipeline(&self.full);
        pass.set_bind_group(0, &self.backdrop, &[]);
        pass.draw(0..3, 0..1);
        for (slot, shadow) in windows {
            if shadow {
                pass.set_pipeline(&self.shadow);
                pass.set_bind_group(0, &slot.masks[0].2, &[]);
                pass.draw(0..6, 0..1);
            }
            pass.set_pipeline(&self.window);
            pass.set_bind_group(0, &slot.window, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

/// One window's texture, its shadow masks and what binds them.
pub(crate) struct Slot {
    pub(super) px: (u32, u32),
    pub(super) view: wgpu::TextureView,
    pub(super) srgb_blend_view: wgpu::TextureView,
    _texture: wgpu::Texture,
    /// Ping-pong masks: texture, view, and a bind group that reads it.
    masks: [(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup); 2],
    pub(super) mask: (u32, u32),
    pub(super) pad: u32,
    uniform: wgpu::Buffer,
    window: wgpu::BindGroup,
}

impl Slot {
    pub(super) fn new(device: &wgpu::Device, comp: &Composite, px: (u32, u32), scale: f32) -> Slot {
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let non_srgb = OFFSCREEN_FORMAT.remove_srgb_suffix();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("desktop window"),
            size: wgpu::Extent3d { width: px.0.max(1), height: px.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT, usage, view_formats: &[non_srgb],
        });
        let view = texture.create_view(&Default::default());
        let srgb_blend_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(non_srgb),
            ..Default::default()
        });
        // Room for the key window's blur, whole mask texels on each side.
        let pad = ((KEY_SHADOW.0 * scale * 3.0) / DOWN as f32).ceil() as u32 * DOWN;
        let mask = ((px.0 + 2 * pad).div_ceil(DOWN), (px.1 + 2 * pad).div_ceil(DOWN));
        let uniform = uniform_buffer(device);
        let masks = [0, 1].map(|_| {
            let (t, v) = new_texture(device, "desktop shadow", mask, MASK_FORMAT, usage);
            let b = bind(device, &comp.layout, &uniform, &v, &comp.sampler);
            (t, v, b)
        });
        let window = bind(device, &comp.layout, &uniform, &view, &comp.sampler);
        Slot { px, view, srgb_blend_view, _texture: texture, masks, mask, pad, uniform, window }
    }

    pub(super) fn write(&self, queue: &wgpu::Queue, uniform: Uniform) {
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
    }
}

fn begin<'e>(enc: &'e mut wgpu::CommandEncoder, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'e> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("desktop"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
    })
}

fn new_texture(device: &wgpu::Device, label: &str, size: (u32, u32), format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label), size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn uniform_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("desktop window"), size: std::mem::size_of::<Uniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    })
}

fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniform: &wgpu::Buffer, view: &wgpu::TextureView, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("desktop"), layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

/// The backdrop scaled to cover `size` pixels, centred and cropped, as opaque sRGB RGBA.
pub(super) fn fit_backdrop(path: &Path, (w, h): (u32, u32)) -> Result<Vec<u8>, String> {
    let fail = |e: ::image::ImageError| format!("{}: {e}", path.display());
    let (iw, ih) = ::image::image_dimensions(path).map_err(fail)?;
    if iw.max(ih) > MAX_BACKDROP {
        return Err(format!("{}: {iw}x{ih} pixels; a backdrop is at most {MAX_BACKDROP} pixels a side", path.display()));
    }
    let image = ::image::open(path).map_err(fail)?.into_rgba8();
    let (iw, ih) = image.dimensions();
    // The centred part of the source with the output's aspect, then scaled to the output.
    let (cw, ch) = if u64::from(iw) * u64::from(h) > u64::from(ih) * u64::from(w) {
        (((f64::from(ih) * f64::from(w) / f64::from(h)).round() as u32).clamp(1, iw), ih)
    } else {
        (iw, ((f64::from(iw) * f64::from(h) / f64::from(w)).round() as u32).clamp(1, ih))
    };
    let crop = ::image::imageops::crop_imm(&image, (iw - cw) / 2, (ih - ch) / 2, cw, ch);
    let image = if (cw, ch) == (w, h) { crop.to_image() } else { ::image::imageops::resize(&*crop, w, h, ::image::imageops::FilterType::CatmullRom) };
    let len = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)).ok_or_else(|| format!("{w}x{h} pixels is too large"))?;
    let mut out = Vec::with_capacity(len);
    for p in image.pixels() {
        let [r, g, b, a] = p.0;
        let over = |c: u8| (u32::from(c) * u32::from(a) / 255) as u8;
        out.extend([over(r), over(g), over(b), 255]);
    }
    Ok(out)
}

/// `size` points at `scale` in whole pixels, when a texture of at most `max` a side holds them.
pub(super) fn pixels(size: (f32, f32), scale: f32, max: u32) -> Option<(u32, u32)> {
    let side = |v: f32| {
        let px = (v * scale).round();
        (px >= 1.0 && px <= max as f32).then_some(px as u32)
    };
    Some((side(size.0)?, side(size.1)?))
}

