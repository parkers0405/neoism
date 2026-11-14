//! Render passes. Bind groups are applied at the next draw, against the pipeline bound then, so
//! groups set before their pipeline land at the right slots; a pipeline with another layout
//! rebinds every group. The sizes buffer and immediates travel as inline bytes.

use std::ops::Range;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCounterDontSample, MTLIndexType, MTLLoadAction, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLScissorRect, MTLStoreAction, MTLViewport,
};
use objc2_video::rc::autoreleasepool;
use parking_lot::Mutex;
use wgpu::custom::*;

use super::encode::Rec;
use super::layout::{Counts, GroupRaw};
use super::pipeline::{PipelineRaw, RenderPipeline, vertex_slot};
use super::query::{self, QuerySet};
use super::resource::{BufferRaw, MetalBuffer, MetalView};
use super::store::{SWAPS, Store};
use super::submit::Keep;
use super::{Obj, send_sync};

const GROUPS: usize = 8;

#[derive(Debug)]
pub(crate) struct MetalPass {
    rec: Arc<Mutex<Rec>>,
    raw: Obj<dyn MTLRenderCommandEncoder>,
    keep: Keep,
    pipeline: Option<Arc<PipelineRaw>>,
    groups: [Option<(Arc<GroupRaw>, Vec<u32>)>; GROUPS],
    /// Groups to apply at the next draw, a bit each.
    dirty: u32,
    sizes_dirty: bool,
    immediates: Vec<u8>,
    immediates_dirty: bool,
    index: Option<(Arc<Store>, Arc<BufferRaw>, u64, MTLIndexType, u64)>,
    vertex: Vec<(u32, Arc<Store>, u64)>,
    /// `SWAPS` when buffers were last bound: a swap since may have moved a bound buffer.
    swaps: u64,
}
send_sync!(MetalPass);

fn view(view: &wgpu::TextureView) -> &MetalView { view.as_custom::<MetalView>().expect("a Metal texture view") }

impl MetalPass {
    pub(super) fn begin(rec: Arc<Mutex<Rec>>, desc: &wgpu::RenderPassDescriptor<'_>) -> Self {
        if desc.depth_stencil_attachment.is_some() { super::unsupported!("depth and stencil") }
        if desc.color_attachments.len() > 4 { super::unsupported!("more than four color attachments") }
        let d = MTLRenderPassDescriptor::new();
        let mut keep = rec.lock().shared().keep_list();
        let mut drawable = false;
        if let Some(ts) = &desc.timestamp_writes {
            let set = ts.query_set.as_custom::<QuerySet>().expect("a Metal query set").raw.clone();
            // SAFETY: attachment 0 exists on every descriptor; the indices are within the query set, as wgpu validated.
            unsafe {
                let attachment = d.sampleBufferAttachments().objectAtIndexedSubscript(0);
                attachment.setSampleBuffer(Some(&set.0));
                attachment.setStartOfVertexSampleIndex(query::index(ts.beginning_of_pass_write_index));
                attachment.setEndOfVertexSampleIndex(MTLCounterDontSample);
                attachment.setStartOfFragmentSampleIndex(MTLCounterDontSample);
                attachment.setEndOfFragmentSampleIndex(query::index(ts.end_of_pass_write_index));
            }
            keep.push(set);
        }
        for (i, attachment) in desc.color_attachments.iter().enumerate() {
            let Some(attachment) = attachment else { continue };
            if attachment.resolve_target.is_some() { super::unsupported!("multisample resolves") }
            let target = view(attachment.view).raw.clone();
            // SAFETY: at most four attachments, checked above.
            let color = unsafe { d.colorAttachments().objectAtIndexedSubscript(i) };
            color.setTexture(Some(&target.raw));
            if let Some(slice) = attachment.depth_slice { color.setDepthPlane(slice as usize); }
            match attachment.ops.load {
                wgpu::LoadOp::Clear(c) => {
                    color.setLoadAction(MTLLoadAction::Clear);
                    color.setClearColor(MTLClearColor { red: c.r, green: c.g, blue: c.b, alpha: c.a });
                }
                wgpu::LoadOp::Load => color.setLoadAction(MTLLoadAction::Load),
                wgpu::LoadOp::DontCare(_) => color.setLoadAction(MTLLoadAction::DontCare),
            }
            color.setStoreAction(match attachment.ops.store {
                wgpu::StoreOp::Store => MTLStoreAction::Store,
                wgpu::StoreOp::Discard => MTLStoreAction::DontCare,
            });
            drawable |= target.drawable;
            keep.push(target);
        }
        let raw = {
            let mut rec = rec.lock();
            rec.end_blit();
            rec.drawable |= drawable;
            let cb = rec.cb();
            autoreleasepool(|_| cb.renderCommandEncoderWithDescriptor(&d)).expect("a Metal render encoder")
        };
        MetalPass {
            rec, raw, keep, pipeline: None, groups: Default::default(), dirty: 0, sizes_dirty: false,
            immediates: Vec::new(), immediates_dirty: false, index: None, vertex: Vec::new(), swaps: SWAPS.load(Ordering::Relaxed),
        }
    }

    /// Rebinds bound buffers whose storage a write swapped since they were bound.
    fn rebind(&mut self) {
        let swaps = SWAPS.load(Ordering::Relaxed);
        if swaps == self.swaps { return }
        self.swaps = swaps;
        self.dirty = self.groups.iter().enumerate().filter(|(_, g)| g.is_some()).fold(0, |m, (i, _)| m | 1 << i);
        for (slot, store, offset) in &self.vertex {
            let raw = store.current();
            // SAFETY: as in `set_vertex_buffer`.
            unsafe { self.raw.setVertexBuffer_offset_atIndex(Some(&raw.raw), *offset as usize, vertex_slot(*slot)) };
            self.keep.push(raw);
        }
        if let Some((store, raw, ..)) = &mut self.index {
            *raw = store.current();
            self.keep.push(raw.clone());
        }
    }

    /// Applies what changed since the last draw against the bound pipeline.
    fn flush(&mut self) {
        self.rebind();
        let Some(pipeline) = &self.pipeline else { return };
        let layout = &pipeline.layout;
        while self.dirty != 0 {
            let index = self.dirty.trailing_zeros() as usize;
            self.dirty &= self.dirty - 1;
            let (Some((group, offsets)), Some(bases)) = (&self.groups[index], layout.bases.get(index)) else { continue };
            apply(&self.raw, group, offsets, bases, &mut self.keep);
            self.sizes_dirty = true;
        }
        if std::mem::take(&mut self.sizes_dirty) {
            for (s, sized) in pipeline.sized.iter().enumerate() {
                let Some(slot) = layout.stages[s].sizes_buffer else { continue };
                if sized.is_empty() { continue }
                let sizes: Vec<u32> = sized.iter().map(|b| {
                    let group = self.groups.get(b.group as usize).and_then(Option::as_ref);
                    group.and_then(|(g, _)| g.sizes.iter().find(|(at, _)| *at == b.binding)).map_or(0, |&(_, size)| size)
                }).collect();
                bytes(&self.raw, s, &sizes, slot);
            }
        }
        if std::mem::take(&mut self.immediates_dirty) && !self.immediates.is_empty() {
            for s in 0..2 {
                if let Some(slot) = layout.stages[s].immediates_buffer { bytes(&self.raw, s, &self.immediates, slot); }
            }
        }
    }
}

/// Sets `data` inline at buffer `slot` of stage `s` (0 vertex, 1 fragment).
fn bytes<T: Copy>(raw: &Obj<dyn MTLRenderCommandEncoder>, s: usize, data: &[T], slot: u8) {
    let ptr = NonNull::from(data).cast();
    let len = std::mem::size_of_val(data);
    // SAFETY: Metal copies `len` bytes from `ptr` before returning.
    unsafe {
        if s == 0 { raw.setVertexBytes_length_atIndex(ptr, len, slot as usize) } else { raw.setFragmentBytes_length_atIndex(ptr, len, slot as usize) }
    }
}

fn apply(raw: &Obj<dyn MTLRenderCommandEncoder>, group: &GroupRaw, offsets: &[u32], bases: &[Counts; 3], keep: &mut Keep) {
    for (s, binds) in group.stages[..2].iter().enumerate() {
        let base = bases[s];
        // SAFETY: slots come from the pipeline layout the shaders were compiled against, and the
        // resources stay alive in the pass's keep list.
        unsafe {
            for b in &binds.buffers {
                let offset = (b.offset + b.dynamic.map_or(0, |i| u64::from(offsets[i]))) as usize;
                let at = usize::from(base.buffers + b.slot);
                let buffer = b.store.current();
                if s == 0 { raw.setVertexBuffer_offset_atIndex(Some(&buffer.raw), offset, at) } else { raw.setFragmentBuffer_offset_atIndex(Some(&buffer.raw), offset, at) }
                keep.push(buffer);
            }
            for (slot, texture) in &binds.textures {
                let at = usize::from(base.textures + slot);
                if s == 0 { raw.setVertexTexture_atIndex(Some(texture), at) } else { raw.setFragmentTexture_atIndex(Some(texture), at) }
            }
            for (slot, sampler) in &binds.samplers {
                let at = usize::from(base.samplers + slot);
                if s == 0 { raw.setVertexSamplerState_atIndex(Some(sampler), at) } else { raw.setFragmentSamplerState_atIndex(Some(sampler), at) }
            }
        }
    }
}

impl Drop for MetalPass {
    fn drop(&mut self) {
        self.raw.endEncoding();
        let mut rec = self.rec.lock();
        rec.keep.append(&mut self.keep);
        rec.shared().recycle(std::mem::take(&mut self.keep));
    }
}

impl RenderPassInterface for MetalPass {
    fn set_pipeline(&mut self, pipeline: &DispatchRenderPipeline) {
        let next = &pipeline.as_custom::<RenderPipeline>().expect("a Metal render pipeline").raw;
        if self.pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, next)) { return }
        let relayout = self.pipeline.as_ref().is_none_or(|p| !Arc::ptr_eq(&p.layout, &next.layout));
        self.raw.setRenderPipelineState(&next.state);
        self.raw.setCullMode(next.cull);
        self.raw.setFrontFacingWinding(next.winding);
        self.raw.setTriangleFillMode(next.fill);
        if relayout {
            self.dirty = self.groups.iter().enumerate().filter(|(_, g)| g.is_some()).fold(0, |m, (i, _)| m | 1 << i);
            // Zeroed for a new layout, as wgpu-core does.
            self.immediates.clear();
            self.immediates.resize(next.layout.immediates as usize, 0);
            self.immediates_dirty = true;
        }
        self.sizes_dirty = true;
        self.keep.push(next.clone());
        self.pipeline = Some(next.clone());
    }
    fn set_bind_group(&mut self, index: u32, group: Option<&DispatchBindGroup>, offsets: &[wgpu::DynamicOffset]) {
        let index = index as usize;
        if index >= GROUPS { super::unsupported!("bind group {index}") }
        let Some(group) = group else { self.groups[index] = None; return };
        let group = &group.as_custom::<super::layout::BindGroup>().expect("a Metal bind group").raw;
        if let Some((bound, bound_offsets)) = &self.groups[index]
            && Arc::ptr_eq(bound, group) && bound_offsets[..] == *offsets { return }
        self.keep.push(group.clone());
        self.groups[index] = Some((group.clone(), offsets.to_vec()));
        self.dirty |= 1 << index;
    }
    fn set_index_buffer(&mut self, buffer: &DispatchBuffer, format: wgpu::IndexFormat, offset: wgpu::BufferAddress, _: Option<wgpu::BufferSize>) {
        let store = buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").store.clone();
        let raw = store.current();
        let (kind, stride) = match format {
            wgpu::IndexFormat::Uint16 => (MTLIndexType::UInt16, 2),
            wgpu::IndexFormat::Uint32 => (MTLIndexType::UInt32, 4),
        };
        self.keep.push(raw.clone());
        self.index = Some((store, raw, offset, kind, stride));
    }
    fn set_vertex_buffer(&mut self, slot: u32, buffer: Option<&DispatchBuffer>, offset: wgpu::BufferAddress, _: Option<wgpu::BufferSize>) {
        let store = buffer.map(|b| b.as_custom::<MetalBuffer>().expect("a Metal buffer").store.clone());
        let raw = store.as_ref().map(|s| s.current());
        // SAFETY: the slot is one the pipeline's vertex descriptor reads; the buffer stays alive
        // in the keep list.
        unsafe { self.raw.setVertexBuffer_offset_atIndex(raw.as_ref().map(|b| &*b.raw), offset as usize, vertex_slot(slot)) };
        if let Some(raw) = raw { self.keep.push(raw); }
        self.vertex.retain(|(at, ..)| *at != slot);
        if let Some(store) = store { self.vertex.push((slot, store, offset)); }
    }
    fn set_immediates(&mut self, offset: u32, data: &[u8]) {
        let at = offset as usize;
        if self.immediates.len() < at + data.len() { self.immediates.resize(at + data.len(), 0); }
        self.immediates[at..at + data.len()].copy_from_slice(data);
        self.immediates_dirty = true;
    }
    fn set_blend_constant(&mut self, c: wgpu::Color) {
        self.raw.setBlendColorRed_green_blue_alpha(c.r as f32, c.g as f32, c.b as f32, c.a as f32);
    }
    fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.raw.setScissorRect(MTLScissorRect { x: x as usize, y: y as usize, width: width as usize, height: height as usize });
    }
    fn set_viewport(&mut self, x: f32, y: f32, width: f32, height: f32, min_depth: f32, max_depth: f32) {
        self.raw.setViewport(MTLViewport {
            originX: x.into(), originY: y.into(), width: width.into(), height: height.into(), znear: min_depth.into(), zfar: max_depth.into(),
        });
    }
    fn set_stencil_reference(&mut self, reference: u32) { self.raw.setStencilReferenceValue(reference); }
    fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) {
        if vertices.is_empty() || instances.is_empty() { return }
        self.flush();
        let Some(pipeline) = &self.pipeline else { return };
        // SAFETY: wgpu validated the draw against the bound pipeline and buffers.
        unsafe {
            self.raw.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(pipeline.primitive, vertices.start as usize,
                vertices.len(), instances.len(), instances.start as usize)
        }
    }
    fn draw_indexed(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
        if indices.is_empty() || instances.is_empty() { return }
        self.flush();
        let (Some(pipeline), Some((_, buffer, offset, kind, stride))) = (&self.pipeline, &self.index) else { return };
        // SAFETY: wgpu validated the draw against the bound pipeline and buffers.
        unsafe {
            self.raw.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount_baseVertex_baseInstance(
                pipeline.primitive, indices.len(), *kind, &buffer.raw, (offset + u64::from(indices.start) * stride) as usize,
                instances.len(), base_vertex as isize, instances.start as usize)
        }
    }
    fn draw_mesh_tasks(&mut self, _: u32, _: u32, _: u32) { super::unsupported!("mesh shaders") }
    fn draw_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress) { super::unsupported!("indirect draws") }
    fn draw_indexed_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress) { super::unsupported!("indirect draws") }
    fn draw_mesh_tasks_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress) { super::unsupported!("mesh shaders") }
    fn multi_draw_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) { super::unsupported!("indirect draws") }
    fn multi_draw_indexed_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {
        super::unsupported!("indirect draws")
    }
    fn multi_draw_indirect_count(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {
        super::unsupported!("indirect draws")
    }
    fn multi_draw_mesh_tasks_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {
        super::unsupported!("mesh shaders")
    }
    fn multi_draw_indexed_indirect_count(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {
        super::unsupported!("indirect draws")
    }
    fn multi_draw_mesh_tasks_indirect_count(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchBuffer, _: wgpu::BufferAddress, _: u32) {
        super::unsupported!("mesh shaders")
    }
    fn insert_debug_marker(&mut self, _: &str) {}
    fn push_debug_group(&mut self, _: &str) {}
    fn pop_debug_group(&mut self) {}
    fn write_timestamp(&mut self, _: &DispatchQuerySet, _: u32) { super::unsupported!("timestamps") }
    fn begin_occlusion_query(&mut self, _: u32) { super::unsupported!("occlusion queries") }
    fn end_occlusion_query(&mut self) { super::unsupported!("occlusion queries") }
    fn begin_pipeline_statistics_query(&mut self, _: &DispatchQuerySet, _: u32) { super::unsupported!("query sets") }
    fn end_pipeline_statistics_query(&mut self) { super::unsupported!("query sets") }
    fn execute_bundles(&mut self, _: &mut dyn Iterator<Item = &DispatchRenderBundle>) { super::unsupported!("render bundles") }
}
