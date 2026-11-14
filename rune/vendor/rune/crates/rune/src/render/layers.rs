//! Bounded painter scopes. The ordinary path retains its instance buffer, batches and pass;
//! only scopes requiring isolation acquire a target. Scope records are also compositor fences.
use super::*;
use std::ops::Range;

// Linear premultiplied intermediates preserve HDR colours and avoid accumulating 8-bit
// quantization through nested layers. The destination format is applied only at composition.
pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Default offscreen texture budget for [`El::cache`](crate::El::cache) subtrees (64 MiB).
pub const DEFAULT_CACHE_BUDGET: u64 = 64 * 1024 * 1024;
/// Maximum width or height of a cached subtree's offscreen texture in device pixels.
const MAX_CACHE_DIM: u32 = 4096;

#[cfg(test)]
#[path = "../runtime/layer_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "../runtime/layer_cache_tests.rs"]
mod cache_tests;
#[cfg(test)]
#[path = "layer_filter_tests.rs"]
mod filter_tests;
#[cfg(test)]
#[path = "../runtime/rounded_clip_tests.rs"]
mod rounded_clip_tests;
#[cfg(test)]
#[path = "continuous_corner_tests.rs"]
mod continuous_corner_tests;

pub(super) struct Layers {
    plan: Vec<Layer>,
    pub targets: Vec<Option<Target>>,
    retained: Vec<Target>,
    stack: Vec<usize>,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    pub additive: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>,
    blur: Option<layer_blur::Pipeline>,
    viewport: (u32, u32),
    image_generation: u64,
    allocations: u64,
    cache_budget: u64,
    frame_stamp: u64,
}

impl Layers {
    /// Makes every blurred layer write its blur uniforms again on its next prepare.
    pub(super) fn forget_blur_uniforms(&mut self) {
        let targets = self.targets.iter_mut().flatten().chain(&mut self.retained);
        for blur in targets.filter_map(|t| t.blur.as_mut()) { blur.forget_uniforms(); }
    }
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            plan: Vec::new(),
            targets: Vec::new(),
            retained: Vec::new(),
            stack: Vec::new(),
            pipelines: Vec::new(),
            additive: None,
            blur: None,
            viewport: (0, 0),
            image_generation: 0,
            allocations: 0,
            cache_budget: DEFAULT_CACHE_BUDGET,
            frame_stamp: 0,
        }
    }
}

struct Layer {
    begin: u32,
    end: u32,
    parent: Option<usize>,
    bounds: [f32; 4], // edges while planning, xywh after preparation
    /// The part of `bounds` that can show (edges), which a cache that doesn't fit falls back to.
    vis: [f32; 4],
    clip: [f32; 4],
    limit: [f32; 4], // ancestor output needed by their filters; never a replacement rounded clip
    fade: [u32; 4],
    opacity: f32,
    blend: Blend,
    blur: f32,
    support: f32,
    cache: Option<(u64, u64)>,
    needs_render: bool,
    /// `Instance::turned`: the composite turns `angle` radians about `pivot`.
    turn: Option<(f32, [f32; 2])>,
    /// Turned, or inside a turned layer: the window, clips and filter limits above don't line
    /// up with this layer's pixels, so `frame` bounds it instead.
    free: bool,
    /// For a free layer, the edges of its contents that can reach the window.
    frame: [f32; 4],
    /// The round hole its composite cuts (`El::cutout_circle`): centre, then the radii it's clear
    /// within and whole from, in window device pixels; zeros for none.
    hole: [f32; 4],
}

/// The upright bounds of edges `r` turned `angle` radians clockwise about `p`.
fn turned_bounds(r: [f32; 4], angle: f32, p: [f32; 2]) -> [f32; 4] {
    let (sn, c) = angle.sin_cos();
    let mut out = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (dx, dy) = (x - p[0], y - p[1]);
        let (qx, qy) = (p[0] + c * dx - sn * dy, p[1] + sn * dx + c * dy);
        out = [out[0].min(qx), out[1].min(qy), out[2].max(qx), out[3].max(qy)];
    }
    out
}

pub(super) struct Target {
    identity: u64,
    composite_parent: Option<u64>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    projection: wgpu::Buffer,
    quad: wgpu::Buffer,
    composite: wgpu::BindGroup,
    pub base: wgpu::BindGroup,
    pub images: Vec<Option<wgpu::BindGroup>>,
    pub glyphs: Vec<wgpu::BindGroup>,
    image_generation: u64,
    blur: Option<layer_blur::Scratch>,
    cache_node: Option<u64>,
    cache_sig: u64,
    last_used: u64,
    content_size: [f32; 2],
}

/// Resource/pass counters for diagnostics. The ordinary path creates no layer resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerStats {
    pub targets: usize,
    pub bytes: u64,
    pub extra_passes: usize,
    pub allocations: u64,
}

fn union(a: &mut [f32; 4], b: [f32; 4]) {
    if b[2] <= b[0] || b[3] <= b[1] { return; }
    a[0] = a[0].min(b[0]); a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]); a[3] = a[3].max(b[3]);
}

impl Renderer {
    fn target_bytes(t: &Target) -> u64 {
        let mut bytes = u64::from(t.texture.width()) * u64::from(t.texture.height())
            * u64::from(FORMAT.block_copy_size(None).unwrap());
        if let Some(blur) = &t.blur {
            bytes += blur.bytes();
        }
        bytes
    }

    /// Maximum bytes of offscreen textures retained for [`El::cache`](crate::El::cache) subtrees
    /// before least-recently-used entries are evicted. Defaults to 64 MiB.
    pub fn set_cache_budget(&mut self, bytes: u64) {
        self.layers.cache_budget = bytes;
        let mut retained_bytes: u64 = self.layers.retained.iter().map(Self::target_bytes).sum();
        while !self.layers.retained.is_empty() && retained_bytes > bytes {
            let oldest = self.layers.retained.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            retained_bytes -= Self::target_bytes(&self.layers.retained[oldest]);
            self.layers.retained.swap_remove(oldest);
        }
    }

    /// Current [`El::cache`](crate::El::cache) texture memory budget in bytes.
    pub fn cache_budget(&self) -> u64 {
        self.layers.cache_budget
    }

    pub(super) fn sync_resident_caches(&self, out: &mut std::collections::HashMap<u64, u64>) {
        if !out.is_empty() {
            out.clear();
        }
        for t in self.layers.targets.iter().flatten().chain(self.layers.retained.iter()) {
            if let Some(node) = t.cache_node {
                out.insert(node, t.cache_sig);
            }
        }
    }

    pub fn layer_stats(&self) -> LayerStats {
        let mut stats = LayerStats { allocations: self.layers.allocations, ..Default::default() };
        for (index, t) in self.layers.targets.iter().enumerate() {
            let Some(t) = t else { continue };
            stats.targets += 1;
            stats.bytes += Self::target_bytes(t);
            if let Some(blur) = &t.blur {
                stats.extra_passes += blur.passes();
            }
            if self.layers.plan.get(index).is_none_or(|l| l.needs_render) {
                stats.extra_passes += 1;
            }
        }
        for t in &self.layers.retained {
            stats.targets += 1;
            stats.bytes += Self::target_bytes(t);
        }
        stats
    }

    pub(super) fn prepare_layers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance], viewport: (u32, u32)) {
        let staging = self.uploads.staging(device, queue);
        self.layers.viewport = viewport;
        let had_layers = !self.layers.plan.is_empty();
        for slot in &mut self.layers.targets {
            if slot.as_ref().is_some_and(|t| t.cache_node.is_some()) {
                self.layers.retained.push(slot.take().unwrap());
            }
        }
        self.layers.plan.clear();
        self.layers.stack.clear();
        for (index, i) in instances.iter().enumerate() {
            if i.params[1] == KIND_LAYER {
                let parent = self.layers.stack.last().copied();
                let parent_cached_hit = parent.is_some_and(|p| {
                    self.layers.plan[p].cache.is_some_and(|(n, s)| {
                        self.layers.retained.iter().any(|t| t.cache_node == Some(n) && t.cache_sig == s)
                    })
                });
                let hidden = parent.is_some_and(|p| self.layers.plan[p].opacity == 0.0) || parent_cached_hit;
                // Bound storage by the ancestor's required input, including filter support.
                // Keep the actual clip separate so rounded corners never move.
                let limit = parent.map_or(NO_CLIP, |p| {
                    let p = &self.layers.plan[p];
                    let grow = (3.0 * p.blur).ceil();
                    [p.clip[0].max(p.limit[0]) - grow, p.clip[1].max(p.limit[1]) - grow,
                        p.clip[2].min(p.limit[2]) + grow, p.clip[3].min(p.limit[3]) + grow]
                });
                let blur = layer_blur::sigma(i.params[2]);
                let cache = (i.params[3] == 1.0).then(|| (unpack_u64_f32(i.radii), unpack_u64_f32(i.uv)));
                let turn = (i.border[1] != 0.0).then_some((i.border[1], [i.border[2], i.border[3]]));
                let parent_free = parent.is_some_and(|p| self.layers.plan[p].free);
                let free = turn.is_some() || parent_free;
                let frame = if free {
                    let outer = match parent {
                        Some(p) if parent_free => self.layers.plan[p].frame,
                        _ => {
                            let support = parent.map_or(0.0, |p| self.layers.plan[p].support);
                            [-support, -support, viewport.0 as f32 + support, viewport.1 as f32 + support]
                        }
                    };
                    let shown = [outer[0].max(i.clip[0]), outer[1].max(i.clip[1]), outer[2].min(i.clip[2]), outer[3].min(i.clip[3])];
                    match turn {
                        Some((angle, pivot)) if shown[2] > shown[0] && shown[3] > shown[1] => turned_bounds(shown, -angle, pivot),
                        _ => shown,
                    }
                } else { [0.0; 4] };
                let limit = if parent_free { NO_CLIP } else { limit };
                self.layers.stack.push(self.layers.plan.len());
                self.layers.plan.push(Layer {
                    begin: index as u32, end: 0, parent,
                    bounds: [i.rect[0], i.rect[1], i.rect[0] + i.rect[2], i.rect[1] + i.rect[3]],
                    vis: [0.0; 4],
                    clip: i.clip,
                    limit,
                    fade: i.fade,
                    opacity: if hidden { 0.0 } else { i.fill[3].clamp(0.0, 1.0) },
                    blend: if i.params[0] == 1.0 { Blend::PlusLighter } else { Blend::SourceOver },
                    blur,
                    support: parent.map_or(0.0, |p| self.layers.plan[p].support) + (3.0 * blur).ceil(),
                    cache,
                    needs_render: true,
                    turn,
                    free,
                    frame,
                    // A cached scope's `radii` name its node instead.
                    hole: if cache.is_some() { [0.0; 4] } else { i.radii },
                });
            } else if i.params[1] == KIND_LAYER_END {
                let at = self.layers.stack.pop().expect("unbalanced layer end");
                let l = &mut self.layers.plan[at];
                l.end = index as u32;
                let grow = (3.0 * l.blur).ceil();
                // Retain off-viewport sources that can contribute through this or an ancestor's
                // filter. Clipping to the destination viewport before filtering loses those taps.
                let vis = if l.free {
                    [(l.bounds[0] - grow).max(l.frame[0] - grow).floor(), (l.bounds[1] - grow).max(l.frame[1] - grow).floor(),
                        (l.bounds[2] + grow).min(l.frame[2] + grow).ceil(), (l.bounds[3] + grow).min(l.frame[3] + grow).ceil()]
                } else {
                    [(l.bounds[0] - grow).max(l.clip[0].max(l.limit[0]) - grow).max(-l.support).floor(),
                        (l.bounds[1] - grow).max(l.clip[1].max(l.limit[1]) - grow).max(-l.support).floor(),
                        (l.bounds[2] + grow).min(l.clip[2].min(l.limit[2]) + grow).min(viewport.0 as f32 + l.support).ceil(),
                        (l.bounds[3] + grow).min(l.clip[3].min(l.limit[3]) + grow).min(viewport.1 as f32 + l.support).ceil()]
                };
                l.vis = vis;
                if l.cache.is_some() {
                    if vis[2] <= vis[0] || vis[3] <= vis[1] {
                        l.opacity = 0.0;
                    }
                    l.bounds = [l.bounds[0].floor(), l.bounds[1].floor(),
                        l.bounds[2].ceil(), l.bounds[3].ceil()];
                } else {
                    l.bounds = vis;
                }
                let bounds = match l.turn {
                    Some((angle, pivot)) if l.bounds[2] > l.bounds[0] && l.bounds[3] > l.bounds[1] => turned_bounds(l.bounds, angle, pivot),
                    _ => l.bounds,
                };
                if let Some(parent) = l.parent.filter(|&p| self.layers.plan[p].cache.is_none()) {
                    union(&mut self.layers.plan[parent].bounds, bounds);
                }
            } else if let Some(&at) = self.layers.stack.last() {
                if self.layers.plan[at].cache.is_some() { continue; }
                let mut rect = i.rect;
                if i.params[1] == KIND_GLYPH && i.params[0] != 0.0 {
                    let (sin, cos) = i.params[0].sin_cos();
                    let width = cos.abs() * rect[2] + sin.abs() * rect[3];
                    let height = sin.abs() * rect[2] + cos.abs() * rect[3];
                    rect = [rect[0] + (rect[2] - width) * 0.5, rect[1] + (rect[3] - height) * 0.5, width, height];
                }
                let grow = match i.params[1] {
                    KIND_GLYPH => 0.0,
                    KIND_SHADOW => i.params[2] * 3.0 + 1.0,
                    KIND_EFFECT => i.params[2] + 1.0,
                    KIND_RECT | KIND_GRADIENT | KIND_DASHED | KIND_RADIAL => 1.0 + i.params[2] * 2.0,
                    _ => 1.0,
                };
                union(&mut self.layers.plan[at].bounds, [
                    (rect[0] - grow).max(i.clip[0]), (rect[1] - grow).max(i.clip[1]),
                    (rect[0] + rect[2] + grow).min(i.clip[2]),
                    (rect[1] + rect[3] + grow).min(i.clip[3]),
                ]);
            }
        }
        assert!(self.layers.stack.is_empty(), "unclosed layer scope");
        self.layers.targets.resize_with(self.layers.plan.len(), || None);
        if !self.layers.plan.is_empty() || !self.layers.retained.is_empty() {
            self.layers.frame_stamp += 1;
        }
        let stamp = self.layers.frame_stamp;
        let max_dim = device.limits().max_texture_dimension_2d.min(MAX_CACHE_DIM);
        let bpp = u64::from(FORMAT.block_copy_size(None).unwrap());
        let mut active_cache_bytes = 0u64;

        // First claim resident cache hits so existing on-screen caches never thrash under a tight budget.
        for index in 0..self.layers.plan.len() {
            let l = &mut self.layers.plan[index];
            l.bounds[2] = (l.bounds[2] - l.bounds[0]).max(0.0);
            l.bounds[3] = (l.bounds[3] - l.bounds[1]).max(0.0);
            if l.bounds[2] == 0.0 || l.bounds[3] == 0.0 { l.opacity = 0.0; }
            let Some((node_id, sig)) = l.cache.filter(|_| l.opacity > 0.0) else { continue };
            let Some(pos) = self.layers.retained.iter().position(|t| t.cache_node == Some(node_id) && t.cache_sig == sig) else { continue };
            let t_bytes = Self::target_bytes(&self.layers.retained[pos]);
            let over_budget = active_cache_bytes + t_bytes > self.layers.cache_budget;
            if over_budget && l.begin + 1 < l.end { continue; }
            let mut t = self.layers.retained.swap_remove(pos);
            if over_budget {
                t.cache_node = None;
            } else {
                t.last_used = stamp;
                active_cache_bytes += t_bytes;
            }
            l.needs_render = false;
            let mut quad = Instance::begin_layer(l.bounds, l.clip, LayerSpec {
                opacity: l.opacity, blend: l.blend, ..Default::default() });
            quad.fade = l.fade;
            if (t.content_size[0] - l.bounds[2]).abs() > 0.5 || (t.content_size[1] - l.bounds[3]).abs() > 0.5 {
                quad.uv = [0.0, 0.0, t.content_size[0], t.content_size[1]];
            }
            staging.write_buffer(&t.quad, 0, bytemuck::bytes_of(&quad));
            self.layers.targets[index] = Some(t);
        }

        for index in 0..self.layers.plan.len() {
            if !self.layers.plan[index].needs_render { continue; }
            let additive_parent = self.layers.plan[index].parent.is_some_and(|p| self.layers.plan[p].blend == Blend::PlusLighter);
            let l = &mut self.layers.plan[index];
            if l.opacity == 0.0 {
                self.layers.targets[index] = None;
                continue;
            }
            let mut bounds = l.bounds;
            let (mut width, mut height) = (bounds[2] as u32, bounds[3] as u32);
            let mut bucket = (width.div_ceil(64) * 64, height.div_ceil(64) * 64);
            let mut cache_target = None;
            if let Some((node_id, sig)) = l.cache {
                let needed_bytes = u64::from(bucket.0) * u64::from(bucket.1) * bpp;
                if bucket.0 <= max_dim && bucket.1 <= max_dim && active_cache_bytes + needed_bytes <= self.layers.cache_budget {
                    active_cache_bytes += needed_bytes;
                    cache_target = Some((node_id, sig));
                    if let Some(pos) = self.layers.retained.iter().position(|t| t.cache_node == Some(node_id)) {
                        self.layers.targets[index] = Some(self.layers.retained.swap_remove(pos));
                    } else {
                        let mut retained_bytes: u64 = self.layers.retained.iter().map(Self::target_bytes).sum();
                        while !self.layers.retained.is_empty() && active_cache_bytes + retained_bytes > self.layers.cache_budget {
                            let oldest = self.layers.retained.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
                            retained_bytes -= Self::target_bytes(&self.layers.retained[oldest]);
                            self.layers.retained.swap_remove(oldest);
                        }
                    }
                } else {
                    if let Some(pos) = self.layers.retained.iter().position(|t| t.cache_node == Some(node_id)) {
                        self.layers.retained.swap_remove(pos);
                    }
                    l.cache = None;
                    // Drawn like an uncached group, it only needs the part that shows.
                    let v = l.vis;
                    bounds = [v[0], v[1], (v[2] - v[0]).max(0.0), (v[3] - v[1]).max(0.0)];
                    l.bounds = bounds;
                    (width, height) = (bounds[2] as u32, bounds[3] as u32);
                    bucket = (width.div_ceil(64) * 64, height.div_ceil(64) * 64);
                }
            }
            let l = &self.layers.plan[index];
            if cache_target.is_none() && l.opacity == 1.0 && l.blend == Blend::SourceOver && !additive_parent
                && l.blur == 0.0 && l.fade[3] == 0 && l.clip == NO_CLIP && l.turn.is_none() && l.hole[3] == 0.0 {
                self.layers.targets[index] = None;
                continue;
            }
            if self.layers.targets[index].as_ref().is_none_or(|t| {
                let size = (t.texture.width(), t.texture.height());
                // The minimum bucket is unavoidable for tiny controls. Do not recreate that
                // same bucket every frame when rounding alone exceeds the 2x area limit.
                size.0 < width || size.1 < height || (size != bucket
                    && u64::from(size.0) * u64::from(size.1) > 2 * u64::from(width) * u64::from(height))
            }) {
                let (width, height) = bucket;
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("isolated group"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                    format: FORMAT, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let projection = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("layer projection"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let base = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, &self.atlas, Some(&projection));
                let composite = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, &texture, None);
                self.layers.targets[index] = Some(Target { texture, view, projection,
                    identity: self.layers.allocations + 1, composite_parent: None,
                    quad: Self::instance_buffer(device, 1), base, composite, images: Vec::new(), glyphs: Vec::new(), image_generation: u64::MAX, blur: None,
                    cache_node: None, cache_sig: 0, last_used: 0, content_size: [0.0; 2] });
                self.layers.allocations += 1;
            }
            let sigma = self.layers.plan[index].blur;
            if sigma > 0.0 && self.layers.blur.is_none() { self.layers.blur = Some(layer_blur::Pipeline::new(device)); }
            let t = self.layers.targets[index].as_mut().unwrap();
            t.cache_node = cache_target.map(|(n, _)| n);
            t.cache_sig = cache_target.map_or(0, |(_, s)| s);
            t.last_used = stamp;
            t.content_size = [bounds[2], bounds[3]];
            if sigma > 0.0 {
                if t.blur.is_none() {
                    t.blur = Some(self.layers.blur.as_ref().unwrap().scratch(device, &t.texture));
                    self.layers.allocations += 1;
                }
                if t.blur.as_mut().unwrap().prepare(device, self.layers.blur.as_ref().unwrap(), &staging,
                    [bounds[2], bounds[3]], sigma, self.layers.plan[index].blend == Blend::PlusLighter) {
                    self.layers.allocations += 1;
                }
            } else { t.blur = None; }
            staging.write_buffer(&t.projection, 0, bytemuck::cast_slice(&bounds));
            let l = &self.layers.plan[index];
            let mut quad = Instance::begin_layer(bounds, l.clip, LayerSpec {
                opacity: l.opacity, blend: l.blend, ..Default::default() });
            if let Some((angle, pivot)) = l.turn { quad = quad.turned(angle, pivot); }
            quad.fade = l.fade;
            quad.radii = l.hole;
            staging.write_buffer(&t.quad, 0, bytemuck::bytes_of(&quad));
        }
        if !self.layers.retained.is_empty() {
            let mut retained_bytes: u64 = self.layers.retained.iter().map(Self::target_bytes).sum();
            while !self.layers.retained.is_empty() && active_cache_bytes + retained_bytes > self.layers.cache_budget {
                let oldest = self.layers.retained.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
                retained_bytes -= Self::target_bytes(&self.layers.retained[oldest]);
                self.layers.retained.swap_remove(oldest);
            }
        }
        // Rebind child composites to their nearest materialized parent (opaque scopes flatten).
        for index in 0..self.layers.plan.len() {
            if self.layers.targets[index].is_none() { continue; }
            let parent = self.layer_target(self.layers.plan[index].parent);
            let t = self.layers.targets[index].as_ref().unwrap();
            let parent_identity = parent.map(|p| self.layers.targets[p].as_ref().unwrap().identity);
            if t.composite_parent == parent_identity { continue; }
            let composite = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, &t.texture,
                parent.map(|p| &self.layers.targets[p].as_ref().unwrap().projection));
            let t = self.layers.targets[index].as_mut().unwrap();
            t.composite = composite;
            t.composite_parent = parent_identity;
        }
        if self.layers.targets.iter().any(Option::is_some) {
            self.add_format(device, FORMAT);
            self.add_layer_format(device, self.format);
            for index in 0..self.extra.len() { self.add_layer_format(device, self.extra[index].0); }
        }
        if self.layers.plan.iter().any(|l| l.blend == Blend::PlusLighter) {
            if self.layers.additive.is_none() {
                self.layers.additive = Some((
                    Self::pipeline(device, &self.layout, &self.shader, FORMAT, self.vertex_entry, Blend::PlusLighter),
                    Self::pipeline(device, &self.layout, &self.shader, FORMAT, "vs", Blend::PlusLighter)));
            }
            if let Some(s) = &mut self.surfaces { s.add_plus_lighter(device, FORMAT); }
        }
        self.refresh_layer_bindings(device);
        if had_layers || !self.layers.plan.is_empty() { self.position_layer_surfaces(queue); }
    }

    /// The layer pipelines again from the renderer's shader (a new uber variant).
    pub(super) fn rebuild_layer_pipelines(&mut self, device: &wgpu::Device) {
        for index in 0..self.layers.pipelines.len() {
            let format = self.layers.pipelines[index].0;
            self.layers.pipelines[index].1 = Self::pipeline(device, &self.layout, &self.shader, format, "vs", Blend::SourceOver);
        }
        if self.layers.additive.is_some() {
            self.layers.additive = Some((
                Self::pipeline(device, &self.layout, &self.shader, FORMAT, self.vertex_entry, Blend::PlusLighter),
                Self::pipeline(device, &self.layout, &self.shader, FORMAT, "vs", Blend::PlusLighter)));
        }
    }

    pub(super) fn add_layer_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if !self.layers.pipelines.iter().any(|(f, _)| *f == format) {
            self.layers.pipelines.push((format, Self::pipeline(device, &self.layout, &self.shader, format, "vs", Blend::SourceOver)));
        }
    }

    fn layer_target(&self, mut layer: Option<usize>) -> Option<usize> {
        while let Some(index) = layer {
            if self.layers.targets[index].is_some() { return Some(index); }
            layer = self.layers.plan[index].parent;
        }
        None
    }

    pub(super) fn layer_blend(&self, target: Option<usize>) -> Blend {
        target.map_or(Blend::SourceOver, |p| self.layers.plan[p].blend)
    }

    pub(super) fn images_changed(&mut self) { self.layers.image_generation += 1; }

    /// Makes every target's bind groups again (a new outline texture): the base one here, its
    /// composite and image and glyph pages at the next `prepare_layers`.
    pub(super) fn rebind_layers(&mut self, device: &wgpu::Device) {
        let (layout, globals, outlines, atlas) = (&self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas);
        for t in self.layers.targets.iter_mut().flatten().chain(&mut self.layers.retained) {
            t.base = Self::bind_group(device, layout, globals, outlines, atlas, atlas, Some(&t.projection));
            // No target's identity is 0, so the composite is made again.
            t.composite_parent = Some(0);
            t.image_generation = u64::MAX;
        }
    }

    pub(super) fn refresh_layer_bindings(&mut self, device: &wgpu::Device) {
        for t in self.layers.targets.iter_mut().flatten() {
            if t.image_generation == self.layers.image_generation { continue; }
            t.images.clear();
            t.images.extend(self.images.iter().map(|page| page.as_ref().map(|(texture, _)|
                Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, texture, Some(&t.projection)))));
            t.glyphs.clear();
            t.glyphs.extend(self.atlas_pages.iter().map(|(texture, _)|
                Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), texture, texture, Some(&t.projection))));
            t.image_generation = self.layers.image_generation;
        }
    }

    pub(super) fn position_layer_surfaces(&self, queue: &wgpu::Queue) {
        let Some(s) = &self.surfaces else { return };
        for index in 0..s.len() {
            let before = s.before_instance(index);
            let layer = self.layers.plan.iter().rposition(|l| before > l.begin && before <= l.end);
            let bounds = self.layer_target(layer).map_or(
                [0.0, 0.0, self.layers.viewport.0 as f32, self.layers.viewport.1 as f32],
                |p| self.layers.plan[p].bounds);
            // prepare_surfaces may precede prepare on the first frame.
            if bounds[2] > 0.0 && bounds[3] > 0.0 { s.target(queue, index, bounds); }
        }
    }

    /// Encode isolated scopes before opening a compositor render pass that calls `draw_range`.
    /// `draw`/`draw_timed` do this automatically. Contents are redrawn each presentation so GPU
    /// springs and newly latched native surfaces stay live; textures, not content, are pooled.
    pub fn encode_layers(&self, encoder: &mut wgpu::CommandEncoder) {
        self.encode_layers_timed(encoder, None, &mut |_, _| {});
    }

    /// `encode_layers` that calls `hook` at each effect slot inside an isolated layer, in painter
    /// order within that layer's pass. The layer's opacity, blur, dissolve and clip then apply to
    /// whatever the hook drew when the layer composites. Slots outside any layer are reached by
    /// `draw_range_with` or `draw_with`.
    pub fn encode_layers_with(&self, encoder: &mut wgpu::CommandEncoder, hook: &mut EffectHook<'_>) {
        self.encode_layers_timed(encoder, None, &mut |phase, site| {
            if let EffectPhase::Draw(pass) = phase { hook(pass, site); }
        });
    }

    /// `encode_layers_with` that also calls `hook(EffectPhase::Backdrop(encoder), site)` outside
    /// the render pass at each backdrop slot (`El::effect_backdrop(true)`) inside an isolated
    /// layer, right after copying the layer's target up to that slot into `EffectSite::backdrop`
    /// and before re-opening the layer's pass for `EffectPhase::Draw(pass)`.
    pub fn encode_layers_with_backdrop(&self, encoder: &mut wgpu::CommandEncoder, hook: &mut EffectBackdropHook<'_>) {
        self.encode_layers_timed(encoder, None, hook);
    }

    /// Origin and size of a pass's target: a layer's bounds, or the frame viewport.
    pub(super) fn target_frame(&self, target: Option<usize>) -> ([f32; 2], [u32; 2]) {
        match target {
            Some(index) => {
                let b = self.layers.plan[index].bounds;
                ([b[0], b[1]], [b[2] as u32, b[3] as u32])
            }
            None => ([0.0, 0.0], [self.layers.viewport.0, self.layers.viewport.1]),
        }
    }

    pub(super) fn encode_layers_timed(&self, encoder: &mut wgpu::CommandEncoder, timestamps: Option<&wgpu::RenderPassTimestampWrites<'_>>, hook: &mut EffectBackdropHook<'_>) {
        self.flush_uploads(encoder);
        let mut first = true;
        for (index, l) in self.layers.plan.iter().enumerate().rev() {
            if !l.needs_render { continue; }
            let Some(target) = &self.layers.targets[index] else { continue };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("isolated group"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })], depth_stencil_attachment: None,
                timestamp_writes: if first { timestamps.map(|t| wgpu::RenderPassTimestampWrites {
                    query_set: t.query_set, beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                    end_of_pass_write_index: None,
                }) } else { None },
                occlusion_query_set: None, multiview_mask: None,
            }).forget_lifetime();
            first = false;
            pass.set_viewport(0.0, 0.0, l.bounds[2], l.bounds[3], 0.0, 1.0);
            let mut active = ActivePass::owned(pass, encoder, &target.view);
            self.draw_layer_range(&mut active, FORMAT, l.begin + 1..l.end, Some(index), hook);
            drop(active);
            if let Some(blur) = &target.blur {
                self.layers.blur.as_ref().unwrap().encode(encoder, blur, &target.view);
            }
        }
    }

    pub(super) fn draw_layer_range(&self, pass: &mut ActivePass<'_, '_>, format: wgpu::TextureFormat, range: Range<u32>, target: Option<usize>, hook: &mut EffectBackdropHook<'_>) {
        let mut cursor = range.start;
        for (index, l) in self.layers.plan.iter().enumerate() {
            if l.begin < cursor || l.begin >= range.end { continue; }
            assert!(l.end < range.end, "draw_range must not split a layer scope");
            self.draw_plain(pass, format, cursor..l.begin, target, true, hook);
            if let Some(t) = &self.layers.targets[index] {
                let pipeline = if self.layer_blend(target) == Blend::PlusLighter {
                    &self.layers.additive.as_ref().expect("prepared dissolve").1
                } else { &self.layers.pipelines.iter().find(|(f, _)| *f == format).expect("Renderer::add_format first").1 };
                pass.set_pipeline(pipeline);
                if let Some(properties) = &self.springs { pass.set_bind_group(1, &properties.group, &[]); }
                pass.set_bind_group(0, &t.composite, &[]);
                pass.set_vertex_buffer(0, t.quad.slice(..));
                pass.draw(0..4, 0..1);
            } else if l.opacity == 1.0 {
                if l.clip != NO_CLIP {
                    let (origin, size) = self.target_frame(target);
                    let edge = |v: f32, o: f32, max: u32| (v - o).clamp(0.0, max as f32) as u32;
                    let (x0, y0) = (edge(l.clip[0].floor(), origin[0], size[0]), edge(l.clip[1].floor(), origin[1], size[1]));
                    let (x1, y1) = (edge(l.clip[2].ceil(), origin[0], size[0]), edge(l.clip[3].ceil(), origin[1], size[1]));
                    if x1 > x0 && y1 > y0 {
                        let outer = pass.scissor();
                        pass.set_scissor(Some([x0, y0, x1 - x0, y1 - y0]), size);
                        self.draw_layer_range(pass, format, l.begin + 1..l.end, target, hook);
                        pass.set_scissor(outer, size);
                    }
                } else {
                    self.draw_layer_range(pass, format, l.begin + 1..l.end, target, hook);
                }
            }
            cursor = l.end + 1;
        }
        let include_end = range.end == self.count || self.layers.plan.iter().any(|l| l.end == range.end);
        self.draw_plain(pass, format, cursor..range.end, target, include_end, hook);
    }

    pub(super) fn validate_layer_range(&self, range: &Range<u32>) {
        for l in &self.layers.plan {
            assert!(!(l.begin < range.start && range.start <= l.end)
                && !(l.begin < range.end && range.end <= l.end), "draw_range must not split a layer scope");
        }
    }

    fn draw_plain(&self, pass: &mut ActivePass<'_, '_>, format: wgpu::TextureFormat, range: Range<u32>, target: Option<usize>, include_end: bool, hook: &mut EffectBackdropHook<'_>) {
        let mut cursor = range.start;
        if let Some(s) = &self.surfaces {
            for index in 0..s.len() {
                let before = s.before_instance(index);
                if before < range.start || before > range.end || (before == range.end && !include_end) { continue; }
                self.draw_with_effects(pass, format, cursor..before, target, hook);
                s.draw_blended(pass, index, format, self.layer_blend(target));
                cursor = before;
            }
        }
        self.draw_with_effects(pass, format, cursor..range.end, target, hook);
    }
}
