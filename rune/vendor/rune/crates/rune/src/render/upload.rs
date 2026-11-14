//! Per-frame uploads: instances, globals, glyph and image atlas rows, and GPU spring nodes.
//!
//! Writes go through the renderer's `Staging`, so a steady frame allocates no buffers. They are
//! recorded into the frame's encoder where the renderer first uses it (`flush_uploads`, at the
//! start of `encode_layers*` and `draw_target_range_with`), or submitted ahead of a draw into a
//! caller's pass (`draw_range*`) and of `Renderer::submit`. Globals are written only when they
//! change, and instances only in the spans that changed.

use std::sync::atomic::{AtomicU64, Ordering};

use rune_staging::Staging;

use super::*;

/// Upload state: the staging, and what the instance and globals buffers hold.
pub(super) struct Uploads {
    /// Made on the first upload, which brings the queue.
    staging: Option<Staging>,
    /// A compositor's `Frame::write_buffer`s, recorded into the frame's encoder as they come and
    /// sealed when it's submitted. Apart from `staging`, whose `submit` would seal them early.
    frame_writes: Option<Staging>,
    /// The instance buffer's contents, so a frame writes only the span that changed.
    instances: Vec<Instance>,
    /// Whether `image_batches`, `effects` and `has_layers` were made from `instances`: false
    /// once the list's length or an instance's `params` change, which can move a batch.
    batched: bool,
    /// Whether `instances` holds a layer.
    has_layers: bool,
    /// The viewport last written to globals (`width << 32 | height`, `u64::MAX` before any).
    viewport: AtomicU64,
    /// The sRGB flag last written to globals.
    srgb: Option<bool>,
    /// `Staging::lost` as last seen by `prepare` and by `upload_images`.
    lost: (u64, u64),
    #[cfg(test)]
    pub(super) globals_writes: u64,
    #[cfg(test)]
    pub(super) batch_runs: u64,
}

impl Default for Uploads {
    fn default() -> Uploads {
        Uploads { staging: None, frame_writes: None, instances: Vec::new(), batched: false, has_layers: false, viewport: AtomicU64::new(u64::MAX), srgb: None, lost: (0, 0),
            #[cfg(test)] globals_writes: 0, #[cfg(test)] batch_runs: 0 }
    }
}

impl Uploads {
    /// Forgets what the instance buffer holds, so the next frame writes and batches it all.
    fn forget_instances(&mut self) {
        self.instances.clear();
        self.batched = false;
    }

    pub(super) fn staging(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Staging {
        self.staging.get_or_insert_with(|| Staging::new(device, queue)).clone()
    }

    #[cfg(test)]
    pub(super) fn stats(&self) -> rune_staging::Stats {
        self.staging.as_ref().map(Staging::stats).unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) fn frame_write_stats(&self) -> rune_staging::Stats {
        self.frame_writes.as_ref().map(Staging::stats).unwrap_or_default()
    }
}

/// Bits for the kinds the batching loop in `prepare` looks at; every other kind leaves the batch
/// as it is.
const BATCHED_KINDS: u32 = 1 << KIND_GLYPH as u32 | 1 << KIND_IMAGE as u32 | 1 << KIND_LAYER as u32
    | 1 << KIND_EFFECT as u32 | 1 << KIND_BACKDROP_BLUR as u32;

/// Instances `write_instances` compares at once.
const BLOCK: usize = 64;

/// Whether two equally long instance lists hold the same bytes. Folds over words into one
/// accumulator per word of an instance, which vectorizes, at about half the cost of a `memcmp`.
/// Words are `u32`, `Instance`'s own alignment, so any list casts.
fn same(a: &[Instance], b: &[Instance]) -> bool {
    let words = |l| bytemuck::cast_slice::<Instance, [u32; 32]>(l);
    let mut acc = [0u32; 32];
    for (x, y) in words(a).iter().zip(words(b)) {
        for j in 0..32 { acc[j] |= x[j] ^ y[j]; }
    }
    acc.iter().fold(0, |s, w| s | w) == 0
}

fn pack((w, h): (u32, u32)) -> u64 { u64::from(w) << 32 | u64::from(h) }

impl Renderer {
    /// Records pending uploads into `encoder`, ahead of the renderer's commands in it.
    pub(super) fn flush_uploads(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(s) = &self.uploads.staging { s.flush(encoder); }
    }

    /// Submits pending uploads ahead of commands already recorded.
    pub(super) fn submit_uploads(&self) {
        if let Some(s) = &self.uploads.staging { s.submit(); }
    }

    /// Readies the frame's `encoder` for submission: submits pending uploads ahead of it, and
    /// seals the compositor's writes recorded into it.
    pub(crate) fn before_submit(&self, encoder: &mut wgpu::CommandEncoder) {
        self.submit_uploads();
        if let Some(s) = &self.uploads.frame_writes { s.flush(encoder); }
    }

    /// `Queue::write_buffer` through staging chunks, recorded into `encoder` here: ahead of the
    /// commands recorded after it (`Frame::write_buffer`). A frame's writes share a chunk, which
    /// `before_submit` hands back to the GPU.
    pub(crate) fn write_buffer(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        let staging = self.uploads.frame_writes.get_or_insert_with(|| Staging::new(device, queue));
        staging.write_buffer(buffer, offset, data);
        staging.record(encoder);
    }

    /// Writes rows `y0..y1` of `texture` from its tightly packed CPU copy `data`.
    fn write_rows(staging: &Staging, texture: &wgpu::Texture, data: &[u8], (y0, y1): (u32, u32)) {
        if y1 <= y0 { return; }
        let row = texture.width() * texture.format().block_copy_size(None).unwrap();
        staging.write_texture_rows(texture, y0, &data[(y0 * row) as usize..(y1 * row) as usize]);
    }

    /// Uploads changed rows of the runtime's image atlas (`El::pixels`), creating its texture on
    /// first use.
    pub fn upload_images(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, images: &mut ImageAtlas) {
        if images.pages.is_empty() && self.images.is_empty() { return self.refresh_layer_bindings(device); }
        let staging = self.uploads.staging(device, queue);
        if staging.lost() != self.uploads.lost.1 {
            self.uploads.lost.1 = staging.lost();
            for page in &mut images.pages { page.dirty = Some((0, page.height)); }
        }
        let mut changed = self.images.len() != images.pages.len();
        self.images.resize_with(images.pages.len(), || None);
        for (index, page) in images.pages.iter_mut().enumerate() {
            if page.data.is_empty() { changed |= self.images[index].is_some(); self.images[index] = None; continue; }
            if self.images[index].as_ref().is_none_or(|(t, _)| t.width() != page.width || t.height() != page.height) {
                let texture = Self::image_texture(device, page.width, page.height);
                let bind = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, &texture, None);
                self.images[index] = Some((texture, bind));
                changed = true;
                page.dirty = Some((0, page.height));
            }
            let Some(rows) = page.dirty.take() else { continue };
            Self::write_rows(&staging, &self.images[index].as_ref().unwrap().0, &page.data, rows);
        }
        if changed { self.images_changed(); }
        self.refresh_layer_bindings(device);
    }

    /// Uploads several instance lists back to back (one buffer, one glyph upload) and returns
    /// each list's instance range for `draw_range`. A compositor's version of `prepare`.
    pub fn prepare_lists(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut Atlas,
        lists: &[&[Instance]],
        viewport: (u32, u32),
    ) -> Vec<std::ops::Range<u32>> {
        let mut all = Vec::with_capacity(lists.iter().map(|l| l.len()).sum());
        let mut ranges = Vec::with_capacity(lists.len());
        for l in lists {
            let a = all.len() as u32;
            all.extend_from_slice(l);
            ranges.push(a..all.len() as u32);
        }
        self.prepare(device, queue, atlas, &all, viewport);
        ranges
    }

    pub(super) fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads instances, globals and any new glyphs. They land ahead of the renderer's next draw:
    /// recorded into the encoder it draws into, which must then be submitted, or submitted ahead
    /// of a `draw_range` and of `submit`.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut Atlas,
        instances: &[Instance],
        viewport: (u32, u32),
    ) {
        if self.springs.is_some() { assert_eq!(instances.len(), self.spring_instances); }
        let staging = self.uploads.staging(device, queue);
        if staging.lost() != self.uploads.lost.0 {
            self.uploads.lost.0 = staging.lost();
            self.resend_uploads(atlas);
        }
        // The Windfoil shader, built the first time text draws from outlines (render/variant.rs),
        // fills atlas pages on the GPU, so the pages are made again as ones it can draw into.
        if atlas.outlines.has_records() && self.select(device, variant::Event::OutlinesWanted) {
            self.atlas = Self::atlas_texture(device, true);
            for page in &mut self.atlas_pages { page.0 = Self::atlas_texture(device, true); }
            self.rebind_all(device);
            atlas.invalidate();
        }
        atlas.outlines.set_support(self.uber.support());
        if let Some(rows) = atlas.dirty.take() {
            atlas.rows_uploaded(0, rows);
            Self::write_rows(&staging, &self.atlas, &atlas.data, rows);
        }
        let mut atlas_changed = self.atlas_pages.len() > atlas.pages.len();
        self.atlas_pages.truncate(atlas.pages.len());
        let filled = self.uber.outlines().is_some();
        for index in 0..atlas.pages.len() {
            if index >= self.atlas_pages.len() {
                let texture = Self::atlas_texture(device, filled);
                let bind = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &texture, &texture, None);
                self.atlas_pages.push((texture, bind));
                atlas_changed = true;
            }
            if let Some(rows) = atlas.pages[index].dirty.take() {
                atlas.rows_uploaded(index + 1, rows);
                Self::write_rows(&staging, &self.atlas_pages[index].0, &atlas.pages[index].data, rows);
            }
        }
        if atlas_changed { self.images_changed(); }
        self.upload_outlines(device, &staging, atlas);
        self.fill_atlas(device, queue, &staging, atlas);
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.instances = Self::instance_buffer(device, self.capacity);
            self.uploads.forget_instances();
        }
        self.write_instances(&staging, instances);
        // Geometry uploads must preserve a presentation timestamp already latched this frame.
        if self.uploads.viewport.swap(pack(viewport), Ordering::Relaxed) != pack(viewport) {
            staging.write_buffer(&self.globals, 0, bytemuck::cast_slice(&[viewport.0 as f32, viewport.1 as f32]));
            #[cfg(test)] { self.uploads.globals_writes += 1; }
        }
        if self.uploads.srgb.replace(self.srgb_blend) != Some(self.srgb_blend) {
            let flags: [f32; 4] = [if self.srgb_blend { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            staging.write_buffer(&self.globals, 16, bytemuck::cast_slice(&flags));
            #[cfg(test)] { self.uploads.globals_writes += 1; }
        }
        self.count = instances.len() as u32;
        if !self.uploads.batched {
            self.batch(instances);
        } else {
            // Same batches: only the effects' copies of their instances can be out of date.
            for (at, effect) in &mut self.effects { *effect = instances[*at as usize]; }
        }
        let has_layers = self.uploads.has_layers;
        self.prepare_layers(device, queue, if has_layers { instances } else { &[] }, viewport);
        self.sync_resident_caches(&mut atlas.resident_caches);
        if self.effects.iter().any(|(_, i)| i.params[0] != 0.0 || i.params[1] == KIND_BACKDROP_BLUR) {
            self.ensure_backdrop(device, self.format, viewport);
            for index in 0..self.extra.len() {
                self.ensure_backdrop(device, self.extra[index].0, viewport);
            }
            if self.layers.targets.iter().any(Option::is_some) {
                self.ensure_backdrop(device, layers::FORMAT, viewport);
            }
        }
        self.prepare_backdrop_blur(device, queue, viewport);
    }

    pub fn prepare_surfaces(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, draws: &[crate::native_surface::SurfaceDraw], viewport: (u32,u32)) {
        if !draws.is_empty() && self.surfaces.is_none() {
            let mut surfaces = crate::native_surface::SurfaceRenderer::new(device, self.format);
            surfaces.set_srgb_blend(self.srgb_blend);
            surfaces.set_staging(self.uploads.staging(device, queue));
            for (format, _) in &self.extra { surfaces.add_format(device, *format); }
            self.surfaces = Some(surfaces);
        }
        if let Some(s) = &mut self.surfaces {
            s.set_srgb_blend(self.srgb_blend);
            if self.layers.additive.is_some() { s.add_plus_lighter(device, layers::FORMAT); }
            s.prepare(device,queue,draws,viewport);
        }
        if self.layer_stats().targets > 0 { self.position_layer_surfaces(queue); }
    }

    /// One uniform write per presentation-only frame; use the display's target time.
    pub fn present_at(&self, queue: &wgpu::Queue, viewport: (u32, u32), time: f64) {
        let t = crate::gpu::springs::split_time(time);
        let data = [viewport.0 as f32, viewport.1 as f32, t[0], t[1]];
        self.uploads.viewport.store(pack(viewport), Ordering::Relaxed);
        match &self.uploads.staging {
            Some(s) => s.write_buffer(&self.globals, 0, bytemuck::cast_slice(&data)),
            None => queue.write_buffer(&self.globals, 0, bytemuck::cast_slice(&data)),
        }
    }

    /// A retarget touches one node record; caller computes inherited position/velocity on CPU.
    pub fn write_spring_node(&mut self, queue: &wgpu::Queue, id: usize, node: &crate::gpu::springs::Node) {
        let properties = self.springs.as_mut().expect("renderer has no GPU springs");
        assert!(id < properties.deadlines.len());
        properties.deadlines[id] = node.settle_at();
        properties.settle_at = properties.deadlines.iter().copied().fold(0.0, f64::max);
        let offset = (id * std::mem::size_of::<crate::gpu::springs::Node>()) as u64;
        match &self.uploads.staging {
            Some(s) => s.write_buffer(&properties.nodes, offset, bytemuck::bytes_of(node)),
            None => queue.write_buffer(&properties.nodes, offset, bytemuck::bytes_of(node)),
        }
    }

    /// Splits the instances into runs that bind the same atlas pages, and notes their effects
    /// and whether they hold a layer.
    fn batch(&mut self, instances: &[Instance]) {
        self.image_batches.clear();
        let (mut start, mut bind) = (0, BatchBind::Image(0));
        let mut has_layers = false;
        self.effects.clear();
        for (i, instance) in instances.iter().enumerate() {
            let kind = instance.params[1];
            if kind >= 0.0 && kind < 16.0 && BATCHED_KINDS & (1 << kind as u32) == 0 { continue }
            has_layers |= instance.params[1] == KIND_LAYER;
            if instance.params[1] == KIND_EFFECT || instance.params[1] == KIND_BACKDROP_BLUR { self.effects.push((i as u32, *instance)); }
            let next_bind = if instance.params[1] == KIND_IMAGE {
                Some(BatchBind::Image(instance.params[0] as usize))
            } else if instance.params[1] == KIND_GLYPH {
                let g = instance.params[2] as usize;
                if g == 0 {
                    match bind {
                        BatchBind::Image(_) => None,
                        BatchBind::Glyph(_) => Some(BatchBind::Image(0)),
                    }
                } else {
                    Some(BatchBind::Glyph(g - 1))
                }
            } else {
                None
            };
            if let Some(next) = next_bind && next != bind {
                if start < i as u32 { self.image_batches.push((start..i as u32, bind)); }
                start = i as u32;
                bind = next;
            }
        }
        if start < instances.len() as u32 { self.image_batches.push((start..instances.len() as u32, bind)); }
        self.uploads.has_layers = has_layers;
        self.uploads.batched = true;
        #[cfg(test)] { self.uploads.batch_runs += 1; }
    }

    /// Writes the spans of instances that differ from what the buffer holds, comparing a block
    /// at a time. Spans less than a block apart merge, so a scroll writes one span and a frame
    /// where a few instances move writes only those. A span whose `params` changed asks `prepare`
    /// to batch again.
    fn write_instances(&mut self, staging: &Staging, instances: &[Instance]) {
        let held = &mut self.uploads.instances;
        let shared = instances.len().min(held.len());
        let mut rebatch = instances.len() != held.len();
        held.resize(instances.len(), Instance::zeroed());
        let mut open: Option<std::ops::Range<usize>> = None;
        for start in (0..shared).step_by(BLOCK) {
            let end = (start + BLOCK).min(shared);
            let differs = |i: &usize| !same(&instances[*i..=*i], &held[*i..=*i]);
            // The first instance alone, so a block that all moved costs one compare to find.
            let first = if differs(&start) { start } else if same(&instances[start..end], &held[start..end]) { continue } else {
                (start + 1..end).find(differs).unwrap_or(start)
            };
            let last = (first..end).rfind(differs).map_or(end, |i| i + 1);
            rebatch |= Self::join(&self.instances, held, staging, instances, &mut open, first..last);
        }
        if instances.len() > shared { rebatch |= Self::join(&self.instances, held, staging, instances, &mut open, shared..instances.len()); }
        if let Some(span) = open { rebatch |= Self::write_span(&self.instances, held, staging, instances, span); }
        self.uploads.batched &= !rebatch;
    }

    /// Adds `span` to the open span if it is less than a block past it, or writes the open span
    /// and opens `span`. True if it wrote a span whose `params` changed.
    fn join(buffer: &wgpu::Buffer, held: &mut [Instance], staging: &Staging, instances: &[Instance],
        open: &mut Option<std::ops::Range<usize>>, span: std::ops::Range<usize>) -> bool {
        match open {
            Some(o) if span.start <= o.end + BLOCK => { o.end = span.end; false }
            _ => open.replace(span).is_some_and(|o| Self::write_span(buffer, held, staging, instances, o)),
        }
    }

    /// Writes `span`, and says whether any of its instances' `params`, which pick their batch,
    /// changed.
    fn write_span(buffer: &wgpu::Buffer, held: &mut [Instance], staging: &Staging, instances: &[Instance], span: std::ops::Range<usize>) -> bool {
        let params = |i: &Instance| bytemuck::cast::<[f32; 4], u128>(i.params);
        let rebatch = held[span.clone()].iter().zip(&instances[span.clone()]).any(|(h, i)| params(h) != params(i));
        held[span.clone()].copy_from_slice(&instances[span.clone()]);
        let offset = (span.start * std::mem::size_of::<Instance>()) as u64;
        staging.write_buffer(buffer, offset, bytemuck::cast_slice(&instances[span]));
        rebatch
    }

    /// The staging lost writes (an encoder holding them was dropped unsubmitted): forget what
    /// the buffers are thought to hold, so this frame writes it all again.
    fn resend_uploads(&mut self, atlas: &mut Atlas) {
        self.uploads.forget_instances();
        self.uploads.viewport.store(u64::MAX, Ordering::Relaxed);
        self.uploads.srgb = None;
        atlas.dirty = Some((0, ATLAS_SIZE));
        for page in &mut atlas.pages { page.dirty = Some((0, ATLAS_SIZE)); }
        atlas.outlines.invalidate();
        self.layers.forget_blur_uniforms();
    }
}

#[cfg(test)]
mod tests;
