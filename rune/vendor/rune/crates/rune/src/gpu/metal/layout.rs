//! Bind group layouts, pipeline layouts and bind groups. A pipeline layout hands each stage's
//! buffers, textures and samplers slots counted up group by group, as wgpu-hal does: immediates
//! first, then every group's entries in binding order, then the sizes buffer when a stage sees
//! a storage buffer. A bind group records its resources per stage at slots relative to its
//! group's first, so binding it is adding the layout's base.

use std::sync::Arc;

use objc2_metal::{MTLSamplerState, MTLTexture};
use wgpu::custom::*;
use wgpu::naga::{self, back::msl};

use super::resource::{Live, MetalBuffer, MetalSampler, MetalView};
use super::store::Store;
use super::{Obj, send_sync};

/// The stages a pipeline binds resources for: vertex and fragment for render pipelines, then compute.
pub(super) const STAGES: [wgpu::ShaderStages; 3] = [wgpu::ShaderStages::VERTEX, wgpu::ShaderStages::FRAGMENT, wgpu::ShaderStages::COMPUTE];

/// The compute stage's index in `STAGES`.
pub(super) const COMPUTE: usize = 2;

/// Slots of each kind a stage has handed out.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Counts {
    pub(super) buffers: u8,
    pub(super) textures: u8,
    pub(super) samplers: u8,
}

#[derive(Debug)]
pub(crate) struct BindGroupLayout {
    /// Sorted by binding.
    entries: Arc<[wgpu::BindGroupLayoutEntry]>,
}

#[derive(Debug)]
pub(crate) struct PipelineLayout {
    pub(super) raw: Arc<LayoutRaw>,
}

#[derive(Debug)]
pub(crate) struct LayoutRaw {
    /// Each group's first slots, per stage.
    pub(super) bases: Vec<[Counts; 3]>,
    pub(super) stages: [msl::EntryPointResources; 3],
    pub(super) immediates: u32,
    /// Each group's entries, for `get_bind_group_layout`.
    groups: Vec<Arc<[wgpu::BindGroupLayoutEntry]>>,
}

impl LayoutRaw {
    /// Group `index`'s layout, as a pipeline made with this layout hands it out.
    pub(super) fn group(&self, index: u32) -> DispatchBindGroupLayout {
        let entries = self.groups.get(index as usize).unwrap_or_else(|| panic!("no bind group {index} in the pipeline's layout")).clone();
        DispatchBindGroupLayout::custom(BindGroupLayout { entries })
    }
}

#[derive(Debug)]
pub(crate) struct BindGroup {
    pub(super) raw: Arc<GroupRaw>,
}

#[derive(Debug, Default)]
pub(crate) struct GroupRaw {
    pub(super) stages: [StageBinds; 3],
    /// Storage buffers' bound lengths by binding, which the sizes buffer reports.
    pub(super) sizes: Vec<(u32, u32)>,
    /// Owners of the textures it binds that this backend didn't make.
    #[allow(dead_code, reason = "held, never read")]
    owners: Vec<super::Owner>,
    #[allow(dead_code, reason = "held, never read")]
    live: Option<Live>,
}

#[derive(Debug, Default)]
pub(super) struct StageBinds {
    pub(super) buffers: Vec<BufferBind>,
    pub(super) textures: Vec<(u8, Obj<dyn MTLTexture>)>,
    pub(super) samplers: Vec<(u8, Obj<dyn MTLSamplerState>)>,
}

#[derive(Debug)]
pub(super) struct BufferBind {
    pub(super) slot: u8,
    /// Resolved to its current storage when the group is applied.
    pub(super) store: Arc<Store>,
    pub(super) offset: u64,
    /// Its index among the group's dynamic offsets.
    pub(super) dynamic: Option<usize>,
}

send_sync!(GroupRaw);

/// The bind target an entry takes in a stage, counting its slot.
fn target(ty: &wgpu::BindingType, counts: &mut Counts) -> msl::BindTarget {
    let mut target = msl::BindTarget::default();
    match *ty {
        wgpu::BindingType::Buffer { ty, .. } => {
            target.buffer = Some(counts.buffers);
            target.mutable = matches!(ty, wgpu::BufferBindingType::Storage { read_only: false });
            counts.buffers += 1;
        }
        wgpu::BindingType::Sampler(_) => {
            target.sampler = Some(msl::BindSamplerTarget::Resource(counts.samplers));
            counts.samplers += 1;
        }
        wgpu::BindingType::Texture { .. } => {
            target.texture = Some(counts.textures);
            counts.textures += 1;
        }
        wgpu::BindingType::StorageTexture { access, .. } => {
            target.texture = Some(counts.textures);
            target.mutable = access != wgpu::StorageTextureAccess::ReadOnly;
            counts.textures += 1;
        }
        ref other => super::unsupported!("binding {other:?}"),
    }
    target
}

impl BindGroupLayout {
    pub(crate) fn new(desc: &wgpu::BindGroupLayoutDescriptor<'_>) -> Self {
        let mut entries = desc.entries.to_vec();
        entries.sort_by_key(|e| e.binding);
        if entries.iter().any(|e| e.count.is_some()) { super::unsupported!("binding arrays") }
        BindGroupLayout { entries: entries.into() }
    }
}

impl BindGroupLayoutInterface for BindGroupLayout {}

impl PipelineLayout {
    pub(crate) fn new(desc: &wgpu::PipelineLayoutDescriptor<'_>) -> Self {
        let groups: Vec<_> = desc.bind_group_layouts.iter()
            .map(|layout| layout.map(|l| l.as_custom::<BindGroupLayout>().expect("a Metal bind group layout")))
            .collect();
        Self::from_groups(&groups, desc.immediate_size)
    }

    pub(super) fn from_groups(groups: &[Option<&BindGroupLayout>], immediate_size: u32) -> Self {
        let mut counts = [Counts::default(); 3];
        let mut stages: [msl::EntryPointResources; 3] = Default::default();
        let mut storage = [false; 3];
        if immediate_size > 0 {
            for (s, resources) in stages.iter_mut().enumerate() {
                resources.immediates_buffer = Some(counts[s].buffers);
                counts[s].buffers += 1;
            }
        }
        let mut bases = Vec::new();
        let mut entries = Vec::new();
        for (group, layout) in groups.iter().enumerate() {
            bases.push(counts);
            entries.push(layout.map_or_else(|| Arc::from([]), |l| l.entries.clone()));
            let Some(layout) = layout else { continue };
            for entry in layout.entries.iter() {
                let binding = naga::ResourceBinding { group: group as u32, binding: entry.binding };
                for (s, stage) in STAGES.iter().enumerate() {
                    if !entry.visibility.contains(*stage) { continue }
                    storage[s] |= matches!(entry.ty, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { .. }, .. });
                    stages[s].resources.insert(binding, target(&entry.ty, &mut counts[s]));
                }
            }
        }
        for s in 0..3 {
            if storage[s] { stages[s].sizes_buffer = Some(counts[s].buffers); }
        }
        PipelineLayout { raw: Arc::new(LayoutRaw { bases, stages, immediates: immediate_size, groups: entries }) }
    }
}

impl PipelineLayoutInterface for PipelineLayout {}

impl BindGroup {
    pub(crate) fn new(desc: &wgpu::BindGroupDescriptor<'_>, live: Live) -> Self {
        let layout = desc.layout.as_custom::<BindGroupLayout>().expect("a Metal bind group layout");
        let mut group = GroupRaw { live: Some(live), ..GroupRaw::default() };
        let mut counts = [Counts::default(); 3];
        let mut dynamic = 0;
        for entry in layout.entries.iter() {
            let resource = &desc.entries.iter().find(|e| e.binding == entry.binding).expect("a bind group entry for every layout entry").resource;
            let dynamic_index = match entry.ty {
                wgpu::BindingType::Buffer { has_dynamic_offset: true, .. } => { dynamic += 1; Some(dynamic - 1) }
                _ => None,
            };
            if let (wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { .. }, .. }, wgpu::BindingResource::Buffer(b)) = (&entry.ty, resource) {
                let size = b.size.map_or_else(|| b.buffer.size() - b.offset, |s| s.get());
                group.sizes.push((entry.binding, u32::try_from(size).unwrap_or(u32::MAX)));
            }
            for (s, stage) in STAGES.iter().enumerate() {
                if !entry.visibility.contains(*stage) { continue }
                let slot = target(&entry.ty, &mut counts[s]);
                let binds = &mut group.stages[s];
                match resource {
                    wgpu::BindingResource::Buffer(b) => {
                        let store = b.buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").store.clone();
                        if slot.mutable { store.gpu_write(); }
                        binds.buffers.push(BufferBind { slot: slot.buffer.unwrap(), store, offset: b.offset, dynamic: dynamic_index });
                    }
                    wgpu::BindingResource::TextureView(v) => {
                        let view = &v.as_custom::<MetalView>().expect("a Metal texture view").raw;
                        binds.textures.push((slot.texture.unwrap(), view.raw.clone()));
                        group.owners.extend(view.owner.clone());
                    }
                    wgpu::BindingResource::Sampler(sampler) => {
                        let Some(msl::BindSamplerTarget::Resource(at)) = slot.sampler else { unreachable!() };
                        binds.samplers.push((at, sampler.as_custom::<MetalSampler>().expect("a Metal sampler").raw.0.clone()));
                    }
                    other => super::unsupported!("bind group resource {other:?}"),
                }
            }
        }
        BindGroup { raw: Arc::new(group) }
    }
}

impl BindGroupInterface for BindGroup {}
