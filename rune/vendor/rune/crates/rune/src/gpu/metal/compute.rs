//! Compute pipelines and passes. As in render passes, bind groups apply at the next dispatch
//! against the pipeline bound then, and the sizes buffer and immediates travel as inline bytes.
//! A pass's timestamp writes sample at its start and end.

use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use objc2_metal::{MTLCommandBuffer, MTLCommandEncoder, MTLComputeCommandEncoder, MTLComputePassDescriptor, MTLComputePipelineState, MTLDevice, MTLSize};
use objc2_video::rc::autoreleasepool;
use parking_lot::Mutex;
use wgpu::custom::*;
use wgpu::naga;

use super::derive::{self, Use};
use super::encode::Rec;
use super::layout::{COMPUTE, GroupRaw, LayoutRaw, PipelineLayout};
use super::shader::{Stage, compile};
use super::store::SWAPS;
use super::submit::Keep;
use super::{Obj, Shared, query, send_sync};

const GROUPS: usize = 8;

#[derive(Debug)]
pub(crate) struct ComputePipeline {
    pub(super) raw: Arc<ComputeRaw>,
}

#[derive(Debug)]
pub(crate) struct ComputeRaw {
    state: Obj<dyn MTLComputePipelineState>,
    layout: Arc<LayoutRaw>,
    /// The storage bindings whose sizes the shader reads, in order.
    sized: Vec<naga::ResourceBinding>,
    workgroup: MTLSize,
}
send_sync!(ComputeRaw);

pub(super) fn create(shared: &Shared, desc: &wgpu::ComputePipelineDescriptor<'_>) -> ComputePipeline {
    let label = desc.label.unwrap_or("compute pipeline");
    let layout = match desc.layout {
        Some(layout) => layout.as_custom::<PipelineLayout>().expect("a Metal pipeline layout").raw.clone(),
        None => derive::layout(label, &[Use { module: desc.module, entry: desc.entry_point, kind: naga::ShaderStage::Compute }]).raw,
    };
    let stage = Stage { module: desc.module, entry: desc.entry_point, constants: desc.compilation_options.constants,
        kind: naga::ShaderStage::Compute };
    let cs = compile(shared, &stage, &layout.stages[COMPUTE], false).unwrap_or_else(|e| panic!("{label}: {e}"));
    let state = shared.device.newComputePipelineStateWithFunction_error(&cs.function).unwrap_or_else(|e| panic!("{label}: {}", e.localizedDescription()));
    let [x, y, z] = cs.workgroup.map(|n| n as usize);
    ComputePipeline { raw: Arc::new(ComputeRaw { state, layout, sized: cs.sized, workgroup: MTLSize { width: x, height: y, depth: z } }) }
}

impl ComputePipelineInterface for ComputePipeline {
    fn get_bind_group_layout(&self, index: u32) -> DispatchBindGroupLayout {
        self.raw.layout.group(index)
    }
}

#[derive(Debug)]
pub(crate) struct MetalComputePass {
    rec: Arc<Mutex<Rec>>,
    raw: Obj<dyn MTLComputeCommandEncoder>,
    keep: Keep,
    pipeline: Option<Arc<ComputeRaw>>,
    groups: [Option<(Arc<GroupRaw>, Vec<u32>)>; GROUPS],
    /// Groups to apply at the next dispatch, a bit each.
    dirty: u32,
    sizes_dirty: bool,
    immediates: Vec<u8>,
    immediates_dirty: bool,
    /// `SWAPS` when buffers were last bound: a swap since may have moved a bound buffer.
    swaps: u64,
}
send_sync!(MetalComputePass);

impl MetalComputePass {
    pub(super) fn begin(rec: Arc<Mutex<Rec>>, desc: &wgpu::ComputePassDescriptor<'_>) -> Self {
        let d = MTLComputePassDescriptor::new();
        let mut keep = rec.lock().shared().keep_list();
        if let Some(ts) = &desc.timestamp_writes {
            let set = ts.query_set.as_custom::<query::QuerySet>().expect("a Metal query set").raw.clone();
            // SAFETY: attachment 0 exists on every descriptor; the indices are within the query set, as wgpu validated.
            unsafe {
                let attachment = d.sampleBufferAttachments().objectAtIndexedSubscript(0);
                attachment.setSampleBuffer(Some(&set.0));
                attachment.setStartOfEncoderSampleIndex(query::index(ts.beginning_of_pass_write_index));
                attachment.setEndOfEncoderSampleIndex(query::index(ts.end_of_pass_write_index));
            }
            keep.push(set);
        }
        let raw = {
            let mut rec = rec.lock();
            rec.end_blit();
            let cb = rec.cb();
            autoreleasepool(|_| cb.computeCommandEncoderWithDescriptor(&d)).expect("a Metal compute encoder")
        };
        MetalComputePass {
            rec, raw, keep, pipeline: None, groups: Default::default(), dirty: 0, sizes_dirty: false,
            immediates: Vec::new(), immediates_dirty: false, swaps: SWAPS.load(Ordering::Relaxed),
        }
    }

    fn bound(&self) -> u32 { self.groups.iter().enumerate().filter(|(_, g)| g.is_some()).fold(0, |m, (i, _)| m | 1 << i) }

    /// Applies what changed since the last dispatch against the bound pipeline.
    fn flush(&mut self) {
        let swaps = SWAPS.load(Ordering::Relaxed);
        if swaps != self.swaps {
            self.swaps = swaps;
            self.dirty = self.bound();
        }
        let Some(pipeline) = &self.pipeline else { return };
        let layout = &pipeline.layout;
        while self.dirty != 0 {
            let index = self.dirty.trailing_zeros() as usize;
            self.dirty &= self.dirty - 1;
            let (Some((group, offsets)), Some(bases)) = (&self.groups[index], layout.bases.get(index)) else { continue };
            let base = bases[COMPUTE];
            let binds = &group.stages[COMPUTE];
            // SAFETY: slots come from the pipeline layout the shader was compiled against, and the
            // resources stay alive in the pass's keep list.
            unsafe {
                for b in &binds.buffers {
                    let offset = (b.offset + b.dynamic.map_or(0, |i| u64::from(offsets[i]))) as usize;
                    let buffer = b.store.current();
                    self.raw.setBuffer_offset_atIndex(Some(&buffer.raw), offset, usize::from(base.buffers + b.slot));
                    self.keep.push(buffer);
                }
                for (slot, texture) in &binds.textures { self.raw.setTexture_atIndex(Some(texture), usize::from(base.textures + slot)); }
                for (slot, sampler) in &binds.samplers { self.raw.setSamplerState_atIndex(Some(sampler), usize::from(base.samplers + slot)); }
            }
            self.sizes_dirty = true;
        }
        if std::mem::take(&mut self.sizes_dirty) && let Some(slot) = layout.stages[COMPUTE].sizes_buffer && !pipeline.sized.is_empty() {
            let sizes: Vec<u32> = pipeline.sized.iter().map(|b| {
                let group = self.groups.get(b.group as usize).and_then(Option::as_ref);
                group.and_then(|(g, _)| g.sizes.iter().find(|(at, _)| *at == b.binding)).map_or(0, |&(_, size)| size)
            }).collect();
            bytes(&self.raw, &sizes, slot);
        }
        if std::mem::take(&mut self.immediates_dirty) && !self.immediates.is_empty()
            && let Some(slot) = layout.stages[COMPUTE].immediates_buffer {
            bytes(&self.raw, &self.immediates, slot);
        }
    }
}

/// Sets `data` inline at buffer `slot`.
fn bytes<T: Copy>(raw: &Obj<dyn MTLComputeCommandEncoder>, data: &[T], slot: u8) {
    // SAFETY: Metal copies the bytes before returning.
    unsafe { raw.setBytes_length_atIndex(NonNull::from(data).cast(), std::mem::size_of_val(data), slot as usize) }
}

impl Drop for MetalComputePass {
    fn drop(&mut self) {
        self.raw.endEncoding();
        let mut rec = self.rec.lock();
        rec.keep.append(&mut self.keep);
        rec.shared().recycle(std::mem::take(&mut self.keep));
    }
}

impl ComputePassInterface for MetalComputePass {
    fn set_pipeline(&mut self, pipeline: &DispatchComputePipeline) {
        let next = &pipeline.as_custom::<ComputePipeline>().expect("a Metal compute pipeline").raw;
        if self.pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, next)) { return }
        if self.pipeline.as_ref().is_none_or(|p| !Arc::ptr_eq(&p.layout, &next.layout)) {
            self.dirty = self.bound();
            // Zeroed for a new layout, as wgpu-core does.
            self.immediates.clear();
            self.immediates.resize(next.layout.immediates as usize, 0);
            self.immediates_dirty = true;
        }
        self.raw.setComputePipelineState(&next.state);
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
    fn set_immediates(&mut self, offset: u32, data: &[u8]) {
        let at = offset as usize;
        if self.immediates.len() < at + data.len() { self.immediates.resize(at + data.len(), 0); }
        self.immediates[at..at + data.len()].copy_from_slice(data);
        self.immediates_dirty = true;
    }
    fn insert_debug_marker(&mut self, _: &str) {}
    fn push_debug_group(&mut self, _: &str) {}
    fn pop_debug_group(&mut self) {}
    fn write_timestamp(&mut self, _: &DispatchQuerySet, _: u32) { super::unsupported!("timestamps inside passes") }
    fn begin_pipeline_statistics_query(&mut self, _: &DispatchQuerySet, _: u32) { super::unsupported!("query sets") }
    fn end_pipeline_statistics_query(&mut self) { super::unsupported!("query sets") }
    fn dispatch_workgroups(&mut self, x: u32, y: u32, z: u32) {
        if x == 0 || y == 0 || z == 0 { return }
        self.flush();
        let Some(pipeline) = &self.pipeline else { return };
        let groups = MTLSize { width: x as usize, height: y as usize, depth: z as usize };
        self.raw.dispatchThreadgroups_threadsPerThreadgroup(groups, pipeline.workgroup);
    }
    fn dispatch_workgroups_indirect(&mut self, _: &DispatchBuffer, _: wgpu::BufferAddress) {
        super::unsupported!("indirect dispatches")
    }
    fn transition_resources<'a>(&mut self, _: &mut dyn Iterator<Item = wgpu::wgt::BufferTransition<&'a DispatchBuffer>>,
        _: &mut dyn Iterator<Item = wgpu::wgt::TextureTransition<&'a DispatchTextureView>>) {}
}
