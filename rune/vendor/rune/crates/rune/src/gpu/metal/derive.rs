//! Layouts derived from a pipeline's shaders, for a pipeline made with `layout: None`, as
//! wgpu-core derives them: each group holds every resource the pipeline's entry points use,
//! visible to the stages that use it and typed by its declaration, and the immediates are as
//! large as the largest a stage uses. `get_bind_group_layout` hands out its groups.

use std::collections::BTreeMap;

use wgpu::naga;

use super::layout::{BindGroupLayout, PipelineLayout};
use super::shader::ShaderModule;

/// An entry point a layout is derived from.
pub(super) struct Use<'a> {
    pub(super) module: &'a wgpu::ShaderModule,
    pub(super) entry: Option<&'a str>,
    pub(super) kind: naga::ShaderStage,
}

pub(super) fn layout(label: &str, uses: &[Use<'_>]) -> PipelineLayout {
    let mut groups: BTreeMap<u32, BTreeMap<u32, wgpu::BindGroupLayoutEntry>> = BTreeMap::new();
    let mut immediates = 0;
    for point in uses {
        let Some(ShaderModule::Naga(ir)) = point.module.as_custom::<ShaderModule>() else {
            super::unsupported!("{label}: a layout derived from a precompiled library's entry points")
        };
        let (module, info) = ir.parts();
        let index = entry(module, point).unwrap_or_else(|e| panic!("{label}: {e}"));
        let used = info.get_entry_point(index);
        let visibility = match point.kind {
            naga::ShaderStage::Vertex => wgpu::ShaderStages::VERTEX,
            naga::ShaderStage::Fragment => wgpu::ShaderStages::FRAGMENT,
            naga::ShaderStage::Compute => wgpu::ShaderStages::COMPUTE,
            other => super::unsupported!("{other:?} shaders"),
        };
        for (handle, var) in module.global_variables.iter() {
            if used[handle].is_empty() { continue }
            if var.space == naga::AddressSpace::Immediate {
                immediates = immediates.max(module.types[var.ty].inner.size(module.to_ctx()));
                continue;
            }
            let Some(binding) = var.binding else { continue };
            let ty = binding_type(module, var).unwrap_or_else(|what| super::unsupported!("{label}: {what}"));
            groups.entry(binding.group).or_default().entry(binding.binding)
                .and_modify(|e| e.visibility |= visibility)
                .or_insert(wgpu::BindGroupLayoutEntry { binding: binding.binding, visibility, ty, count: None });
        }
    }
    let count = groups.keys().next_back().map_or(0, |&g| g + 1);
    let layouts: Vec<BindGroupLayout> = (0..count).map(|g| {
        let entries: Vec<_> = groups.get(&g).map(|g| g.values().copied().collect()).unwrap_or_default();
        BindGroupLayout::new(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &entries })
    }).collect();
    let refs: Vec<_> = layouts.iter().map(Some).collect();
    PipelineLayout::from_groups(&refs, immediates)
}

/// The index of `point`'s entry point: the named one, or the module's only one of its kind.
fn entry(module: &naga::Module, point: &Use<'_>) -> Result<usize, String> {
    let mut of_kind = module.entry_points.iter().enumerate().filter(|(_, e)| e.stage == point.kind);
    match point.entry {
        Some(name) => of_kind.find(|(_, e)| e.name == name).map(|(i, _)| i).ok_or_else(|| format!("no entry point {name}")),
        None => match (of_kind.next(), of_kind.next()) {
            (Some((i, _)), None) => Ok(i),
            _ => Err(format!("no single {:?} entry point", point.kind)),
        },
    }
}

/// The binding type `var`'s declaration implies, or what this backend can't bind.
fn binding_type(module: &naga::Module, var: &naga::GlobalVariable) -> Result<wgpu::BindingType, String> {
    let buffer = |ty| Ok(wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None });
    match var.space {
        naga::AddressSpace::Uniform => return buffer(wgpu::BufferBindingType::Uniform),
        naga::AddressSpace::Storage { access } => {
            return buffer(wgpu::BufferBindingType::Storage { read_only: !access.contains(naga::StorageAccess::STORE) });
        }
        _ => {}
    }
    Ok(match module.types[var.ty].inner {
        naga::TypeInner::Sampler { comparison } => wgpu::BindingType::Sampler(if comparison {
            wgpu::SamplerBindingType::Comparison
        } else {
            wgpu::SamplerBindingType::Filtering
        }),
        naga::TypeInner::Image { dim, arrayed, class } => {
            let view_dimension = match dim {
                naga::ImageDimension::D1 => wgpu::TextureViewDimension::D1,
                naga::ImageDimension::D2 if arrayed => wgpu::TextureViewDimension::D2Array,
                naga::ImageDimension::D2 => wgpu::TextureViewDimension::D2,
                naga::ImageDimension::D3 => wgpu::TextureViewDimension::D3,
                naga::ImageDimension::Cube if arrayed => wgpu::TextureViewDimension::CubeArray,
                naga::ImageDimension::Cube => wgpu::TextureViewDimension::Cube,
            };
            match class {
                naga::ImageClass::Sampled { kind, multi } => wgpu::BindingType::Texture {
                    sample_type: match kind {
                        naga::ScalarKind::Sint => wgpu::TextureSampleType::Sint,
                        naga::ScalarKind::Uint => wgpu::TextureSampleType::Uint,
                        _ => wgpu::TextureSampleType::Float { filterable: true },
                    },
                    view_dimension,
                    multisampled: multi,
                },
                naga::ImageClass::Depth { multi } => {
                    wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension, multisampled: multi }
                }
                naga::ImageClass::Storage { format, access } => wgpu::BindingType::StorageTexture {
                    access: if access.contains(naga::StorageAccess::ATOMIC) {
                        wgpu::StorageTextureAccess::Atomic
                    } else if access.contains(naga::StorageAccess::LOAD | naga::StorageAccess::STORE) {
                        wgpu::StorageTextureAccess::ReadWrite
                    } else if access.contains(naga::StorageAccess::STORE) {
                        wgpu::StorageTextureAccess::WriteOnly
                    } else {
                        wgpu::StorageTextureAccess::ReadOnly
                    },
                    format: wgpu_naga_bridge::map_storage_format_from_naga(format),
                    view_dimension,
                },
                naga::ImageClass::External => return Err("external textures".into()),
            }
        }
        ref other => return Err(format!("a binding of type {other:?}")),
    })
}
