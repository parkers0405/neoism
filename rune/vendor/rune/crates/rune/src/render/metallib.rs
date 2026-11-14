//! Precompiled Metal library (`.metallib`) for Rune's uber shader on macOS.
//!
//! `wgpu` 30's Metal backend otherwise translates `shader.wgsl` through Naga to MSL and calls
//! `[MTLDevice newLibraryWithSource:options:error:]` on the XPC `MTLCompilerService` during
//! `Renderer::new`. Loading precompiled AIR bytecode via `Device::create_shader_module_passthrough`
//! skips WGSL parsing, validation, MSL codegen, and the Metal front-end compile on every launch.
//!
//! On wgpu's backend, other shaders stay on WGSL (Rune's own backend precompiles them too, in
//! `gpu/metal/builtin.rs`):
//! - `Renderer::with_springs`, `rune-fx`, and `rune-light` bind runtime-sized `var<storage>`
//!   arrays (`_mslBufferSizes`), and `wgpu-hal` 30's `ShaderModuleSource::Passthrough` leaves
//!   `CompiledShader::sized_bindings` empty.
//! - `layer_blur` specializes WGSL `override` constants (`MODE`, `ADDITIVE`) per pipeline, which
//!   `ShaderModuleSource::Passthrough` does not evaluate.
//! - iOS (`target_os = "ios"`) uses a distinct `MTLB` platform tag (`ios` vs `ios-simulator` vs
//!   `macos`) and stays on WGSL for now.
//!
//! Note on `MTLBinaryArchive`: `wgpu-hal` 30's Metal `PipelineCache` implementation is a no-op, so
//! precompiling `.metallib` eliminates front-end MSL->AIR compilation while the final AIR->GPU ISA
//! step in `newRenderPipelineStateWithDescriptor` remains cached by the macOS Metal shader cache.
//! Supporting `MTLBinaryArchive` without forking `wgpu` would require `wgpu-hal` to wire
//! `MTLRenderPipelineDescriptor.binaryArchives` through `RenderPipelineDescriptor::cache`.
//!
//! Rune's own Metal backend (`metal-backend`) loads `uber_native.metallib` instead: the MSL that
//! backend emits (vertex descriptors rather than wgpu-hal's vertex pulling), in MSL 3.0.

include!("uber_metallib_hash.rs");

#[cfg(any(test, target_os = "macos"))]
const UBER_METALLIB: &[u8] = include_bytes!("../uber.metallib");
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
const UBER_NATIVE_METALLIB: &[u8] = include_bytes!("../uber_native.metallib");

pub(super) const UBER_VERTEX_ATTRS: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
    4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Uint32x4
];

#[cfg(all(test, target_os = "macos"))]
pub(super) const METAL_COMPILE_FLAGS: &str =
    "-std=macos-metal2.3 -mmacosx-version-min=11.0 -fpreserve-invariance -Wno-unused-const-variable -O2";

/// Rune's Metal backend picks MSL 3.0 or later, so needs macOS 13.
#[cfg(all(test, target_os = "macos", feature = "metal-backend"))]
pub(super) const NATIVE_METAL_COMPILE_FLAGS: &str =
    "-std=metal3.0 -mmacosx-version-min=13.0 -fpreserve-invariance -Wno-unused-const-variable -O2";
#[cfg(all(test, target_os = "macos", feature = "metal-backend"))]
const NATIVE_MSL_VERSION: (u8, u8) = (3, 0);

#[cfg(target_os = "macos")]
std::thread_local! {
    static FORCE_METALLIB: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    static METALLIB_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn uber_wgsl() -> String {
    format!("{}\n{}", super::CLIP_WGSL, include_str!("../shader.wgsl"))
}

#[cfg(any(test, target_os = "macos"))]
pub(super) fn uber_wgsl_hash(wgsl: &str) -> blake3::Hash {
    blake3::hash(wgsl.as_bytes())
}

pub(super) const fn uber_bind_entries() -> [wgpu::BindGroupLayoutEntry; 4] {
    let uniform = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let texture = wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: false },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    [
        wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: uniform, count: None },
        wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: texture, count: None },
        wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: texture, count: None },
        wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: uniform, count: None },
    ]
}

pub(super) fn uber_pipeline_layout<'a>(layouts: &'a [Option<&'a wgpu::BindGroupLayout>; 2], springs: bool) -> wgpu::PipelineLayoutDescriptor<'a> {
    wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &layouts[..if springs { 2 } else { 1 }],
        immediate_size: 0,
    }
}

pub(super) fn uber_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::Instance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &UBER_VERTEX_ATTRS,
    }
}

pub(super) fn uber_primitive() -> wgpu::PrimitiveState {
    wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() }
}

// Descriptor values shared with Renderer::create, not their source spelling or labels.
// WGSL, generated MSL, and the actual Naga options have separate fingerprints below.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub(super) fn uber_abi_hash() -> u64 {
    layout_hash(&uber_bind_entries(), &uber_vertex_layout(), uber_primitive())
}

/// [`uber_abi_hash`] for another set of uber bindings (the Windfoil variant's).
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub(super) fn abi_hash_of(entries: &[wgpu::BindGroupLayoutEntry]) -> u64 {
    layout_hash(entries, &uber_vertex_layout(), uber_primitive())
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
fn layout_hash(entries: &[wgpu::BindGroupLayoutEntry], vertex: &wgpu::VertexBufferLayout<'_>, primitive: wgpu::PrimitiveState) -> u64 {
    // The normal uber pipeline has one group; use a placeholder instead of hashing opaque GPU
    // handle IDs. The springs pipeline has a second group and never uses this library.
    let groups = [None, None];
    let layout = uber_pipeline_layout(&groups, false);
    descriptor_hash(entries, vertex, primitive, &layout)
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
fn descriptor_hash(entries: &[wgpu::BindGroupLayoutEntry], vertex: &wgpu::VertexBufferLayout<'_>, primitive: wgpu::PrimitiveState, layout: &wgpu::PipelineLayoutDescriptor<'_>) -> u64 {
    fnv1a64(format!("{entries:?}|{}|{}|{vertex:?}|{primitive:?}",
        layout.bind_group_layouts.len(), layout.immediate_size).as_bytes())
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    h
}

fn wgsl_shader(device: &wgpu::Device, wgsl: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("uber"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    })
}

pub(super) fn uber_shader_and_pipeline(
    device: &wgpu::Device,
    mut make_pipeline: impl FnMut(&super::dxbc::Shaders) -> wgpu::RenderPipeline,
) -> (super::dxbc::Shaders, wgpu::RenderPipeline) {
    let wgsl = uber_wgsl();
    #[cfg(target_os = "macos")]
    if metallib_enabled()
        && let Some((shader, pipeline)) = try_uber_metallib(device, &wgsl, uber_metallib(device), UBER_WGSL_BLAKE3, |shader| make_pipeline(&super::dxbc::Shaders::one(shader.clone())))
    {
        METALLIB_LOADS.with(|n| n.set(n.get() + 1));
        return (super::dxbc::Shaders::one(shader), pipeline);
    }
    #[cfg(target_os = "windows")]
    if let Some(result) = super::dxbc::uber(device, &wgsl, uber_abi_hash(), &mut make_pipeline) {
        return result;
    }
    let shaders = super::dxbc::Shaders::one(wgsl_shader(device, &wgsl));
    let pipeline = make_pipeline(&shaders);
    (shaders, pipeline)
}

#[cfg(target_os = "macos")]
pub(super) fn metallib_enabled() -> bool {
    if let Some(forced) = FORCE_METALLIB.with(std::cell::Cell::get) {
        return forced;
    }
    !matches!(
        std::env::var("RUNE_DISABLE_METALLIB").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes")
    )
}

#[cfg(all(test, target_os = "macos"))]
pub(super) fn thread_loads() -> usize {
    METALLIB_LOADS.with(std::cell::Cell::get)
}

/// Whether `device` is Rune's own Metal backend's.
#[cfg(target_os = "macos")]
fn native(device: &wgpu::Device) -> bool {
    #[cfg(feature = "metal-backend")]
    { crate::gpu::metal::raw_device(device).is_some() }
    #[cfg(not(feature = "metal-backend"))]
    { let _ = device; false }
}

/// The library built for `device`'s backend.
#[cfg(target_os = "macos")]
fn uber_metallib(device: &wgpu::Device) -> &'static [u8] {
    #[cfg(feature = "metal-backend")]
    if native(device) { return UBER_NATIVE_METALLIB; }
    #[cfg(not(feature = "metal-backend"))]
    let _ = device;
    UBER_METALLIB
}

#[cfg(target_os = "macos")]
fn try_uber_metallib<T>(
    device: &wgpu::Device,
    wgsl: &str,
    metallib: &[u8],
    expected_wgsl_hash: &str,
    make_pipeline: impl FnOnce(&wgpu::ShaderModule) -> T,
) -> Option<(wgpu::ShaderModule, T)> {
    // Rune's backend takes a metallib without the feature, which promises arbitrary passthrough.
    if !native(device) && !device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS) {
        return None;
    }
    if metallib != uber_metallib(device) || uber_abi_hash() != UBER_ABI_FNV64 {
        return None;
    }
    if uber_wgsl_hash(wgsl).to_hex().as_str() != expected_wgsl_hash {
        return None;
    }
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
    // SAFETY: only our embedded macOS library is accepted, never caller-provided shader code.
    // Its validated Naga MSL retains bounds checks and loop bounding; the ABI fingerprint above
    // covers the renderer's bindings, immediates and vertex/primitive layout. Freshness tests
    // cover the generator's actual MSL/options; pixel equivalence guards backend patch updates.
    // Vertex pulling uses buffer(30), and Metal supplies its sizes even for passthrough.
    // No storage/workgroup arrays or specialization constants are used (HAL cannot reflect those
    // from passthrough). Unsupported OS/GPU libraries and pipeline linkage failures are caught
    // before any draw. Pixel equivalence and regeneration tests check the committed artifact.
    let shader = unsafe {
        device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
            label: Some("uber"),
            entry_points: std::borrow::Cow::Borrowed(&[
                wgpu::PassthroughShaderEntryPoint {
                    name: std::borrow::Cow::Borrowed("vs"),
                    workgroup_size: (0, 0, 0),
                },
                wgpu::PassthroughShaderEntryPoint {
                    name: std::borrow::Cow::Borrowed("fs"),
                    workgroup_size: (0, 0, 0),
                },
            ]),
            metallib: Some(std::borrow::Cow::Borrowed(metallib)),
            ..Default::default()
        })
    };
    // Rune's backend reports no errors to scopes, so check it loaded and has both functions.
    #[cfg(feature = "metal-backend")]
    if native(device) && !crate::gpu::metal::module_ready(&shader) {
        let _ = (pollster::block_on(internal_scope.pop()), pollster::block_on(validation_scope.pop()));
        return None;
    }
    // Metal may load the library but reject an entry point or its AIR at pipeline creation.
    // Keep both scopes active through that operation, before retaining the shader for later use.
    let pipeline = make_pipeline(&shader);
    let internal_err = pollster::block_on(internal_scope.pop());
    let validation_err = pollster::block_on(validation_scope.pop());
    if internal_err.is_some() || validation_err.is_some() {
        return None;
    }
    Some((shader, pipeline))
}

#[cfg(all(test, target_os = "macos"))]
pub(super) fn with_metallib_enabled<R>(enabled: bool, f: impl FnOnce() -> R) -> R {
    struct Guard(Option<bool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            FORCE_METALLIB.with(|c| c.set(self.0));
        }
    }
    let prev = FORCE_METALLIB.with(|c| c.replace(Some(enabled)));
    let _guard = Guard(prev);
    f()
}

/// Translates `uber_wgsl()` to MSL for the `vs` and `fs` entry points with the exact resource
/// bindings, vertex-pulling transform (`buffer(30)`), and runtime check policies that `wgpu-hal`
/// uses on Metal for `Renderer::new`.
#[cfg(all(test, target_os = "macos"))]
fn uber_msl(wgsl_src: &str, attrs: &[wgpu::VertexAttribute], stride: u32) -> (String, String, u64) {
    uber_msl_with(wgsl_src, &uber_bind_entries(), attrs, stride)
}

/// [`uber_msl`] for another uber variant's bindings (render/variant.rs).
#[cfg(all(test, target_os = "macos"))]
fn uber_msl_with(wgsl_src: &str, bind_entries: &[wgpu::BindGroupLayoutEntry], attrs: &[wgpu::VertexAttribute], stride: u32) -> (String, String, u64) {
    use wgpu::naga::{
        self,
        back::{msl, pipeline_constants::process_overrides},
        front::wgsl,
        proc::{BoundsCheckPolicies, BoundsCheckPolicy},
        valid::{Capabilities, SubgroupOperationSet, ValidationFlags, Validator},
    };

    let module = wgsl::parse_str(wgsl_src).expect("parse uber WGSL");
    let info = Validator::new(
        ValidationFlags::all(),
        Capabilities::default() | Capabilities::SHADER_FLOAT16_IN_FLOAT32,
    )
    .subgroup_stages(naga::valid::ShaderStages::all())
    .subgroup_operations(SubgroupOperationSet::all())
    .validate(&module)
    .expect("validate uber WGSL");

    // Follow HAL's per-stage allocation, using the renderer's actual entries. Fail closed if
    // the shader gains storage/arrays: passthrough cannot provide their sized-binding metadata.
    let resources = |stage: wgpu::ShaderStages| {
        let groups = [None, None];
        let layout = uber_pipeline_layout(&groups, false);
        assert_eq!(layout.immediate_size, 0, "audit Metal immediate-data mapping before precompiling");
        let mut result = msl::EntryPointResources::default();
        let (mut buffer, mut texture) = (0, 0);
        let mut entries = bind_entries.to_vec();
        entries.sort_by_key(|entry| entry.binding);
        for entry in entries {
            if !entry.visibility.contains(stage) { continue; }
            assert!(entry.count.is_none(), "passthrough binding arrays need an audit");
            let mut target = msl::BindTarget::default();
            match entry.ty {
                wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, .. } => {
                    target.buffer = Some(buffer);
                    buffer += 1;
                }
                wgpu::BindingType::Texture { .. } => {
                    target.texture = Some(texture);
                    texture += 1;
                }
                _ => panic!("audit new uber binding type before precompiling"),
            }
            result.resources.insert(naga::ResourceBinding { group: 0, binding: entry.binding }, target);
        }
        if stage == wgpu::ShaderStages::VERTEX { result.sizes_buffer = Some(buffer); }
        result
    };
    let vs_resources = resources(wgpu::ShaderStages::VERTEX);
    let fs_resources = resources(wgpu::ShaderStages::FRAGMENT);

    let attributes = attrs
        .iter()
        .map(|attr| msl::AttributeMapping {
            shader_location: attr.shader_location,
            offset: attr.offset as u32,
            format: attr.format,
        })
        .collect();
    let vbm = msl::VertexBufferMapping {
        id: 30,
        stride,
        step_mode: match uber_vertex_layout().step_mode {
            wgpu::VertexStepMode::Instance => msl::VertexBufferStepMode::ByInstance,
            wgpu::VertexStepMode::Vertex => msl::VertexBufferStepMode::ByVertex,
        },
        attributes,
    };

    let limits = wgpu::Limits::default();
    let translate = |stage: naga::ShaderStage,
                     ep: &str,
                     resources: msl::EntryPointResources,
                     vertex_buffer_mappings: Vec<msl::VertexBufferMapping>| {
        let (ep_module, ep_info) =
            process_overrides(&module, &info, Some((stage, ep)), &Default::default())
                .expect("process overrides");
        let options = msl::Options {
            lang_version: (2, 3),
            inline_samplers: Default::default(),
            spirv_cross_compatibility: false,
            fake_missing_bindings: false,
            per_entry_point_map: msl::EntryPointResourceMap::from([(ep.to_owned(), resources)]),
            bounds_check_policies: BoundsCheckPolicies {
                index: BoundsCheckPolicy::Restrict,
                buffer: BoundsCheckPolicy::Restrict,
                image_load: BoundsCheckPolicy::Restrict,
                binding_array: BoundsCheckPolicy::Unchecked,
            },
            zero_initialize_workgroup_memory: true,
            force_loop_bounding: true,
            task_dispatch_limits: Some(naga::back::TaskDispatchLimits {
                max_mesh_workgroups_per_dim: limits.max_mesh_workgroups_per_dimension,
                max_mesh_workgroups_total: limits.max_mesh_workgroup_total_count,
            }),
            mesh_shader_primitive_indices_clamp: true,
            emit_int_div_checks: true,
            ray_query_initialization_tracking: true,
        };
        let pipeline_options = msl::PipelineOptions {
            entry_point: Some((stage, ep.to_owned())),
            allow_and_force_point_size: uber_primitive().topology == wgpu::PrimitiveTopology::PointList,
            vertex_pulling_transform: true,
            vertex_buffer_mappings,
            binding_array_length_map: Default::default(),
        };
        let (msl, translation_info) =
            msl::write_string(&ep_module, &ep_info, &options, &pipeline_options)
                .expect("translate uber WGSL to MSL");
        assert_eq!(
            translation_info.entry_point_names[0].as_deref(),
            Ok(ep),
            "MSL entry point name must match WGSL entry point"
        );
        // Resource maps are ordered; the binding-array map is empty (arrays rejected above).
        (msl, format!("{options:?}\n{pipeline_options:?}"))
    };

    let (vs_msl, vs_options) = translate(naga::ShaderStage::Vertex, "vs", vs_resources, vec![vbm]);
    let (fs_msl, fs_options) = translate(naga::ShaderStage::Fragment, "fs", fs_resources, Vec::new());
    (vs_msl, fs_msl, fnv1a64(format!("{vs_options}\n{fs_options}").as_bytes()))
}

#[cfg(all(test, target_os = "macos"))]
fn uber_msl_hash(vs_msl: &str, fs_msl: &str) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(METAL_COMPILE_FLAGS.as_bytes());
    hasher.update(&[0]);
    hasher.update(vs_msl.as_bytes());
    hasher.update(&[0]);
    hasher.update(fs_msl.as_bytes());
    hasher.finalize()
}

/// The MSL Rune's Metal backend compiles the uber shader to, which `uber_native.metallib` holds.
#[cfg(all(test, target_os = "macos", feature = "metal-backend"))]
fn native_uber_msl(wgsl: &str) -> [String; 2] {
    crate::gpu::metal::render_msl(wgsl, &uber_bind_entries(), NATIVE_MSL_VERSION)
}

#[cfg(all(test, target_os = "macos", feature = "metal-backend"))]
fn native_uber_msl_hash([vs, fs]: &[String; 2]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    for part in [NATIVE_METAL_COMPILE_FLAGS, vs, fs] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Called by `scripts/precompile-metal-shaders.sh` (`--ignored --exact --nocapture`) to emit
    /// the Naga-translated MSL and BLAKE3 digests for the current `shader.wgsl` + `clip.wgsl`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn dump_uber_msl() {
        let wgsl = uber_wgsl();
        let wgsl_hash = uber_wgsl_hash(&wgsl);
        let (vs_msl, fs_msl, options_hash) =
            uber_msl(&wgsl, &UBER_VERTEX_ATTRS, std::mem::size_of::<super::super::Instance>() as u32);
        let msl_hash = uber_msl_hash(&vs_msl, &fs_msl);
        println!("---BEGIN_WGSL_BLAKE3---\n{}\n---END_WGSL_BLAKE3---", wgsl_hash.to_hex());
        println!("---BEGIN_MSL_BLAKE3---\n{}\n---END_MSL_BLAKE3---", msl_hash.to_hex());
        println!("---BEGIN_METAL_FLAGS---\n{METAL_COMPILE_FLAGS}\n---END_METAL_FLAGS---");
        println!("---BEGIN_ABI_FNV64---\n{:016x}\n---END_ABI_FNV64---", uber_abi_hash());
        println!("---BEGIN_OPTIONS_FNV64---\n{options_hash:016x}\n---END_OPTIONS_FNV64---");
        println!("---BEGIN_VS_MSL---\n{vs_msl}---END_VS_MSL---");
        println!("---BEGIN_FS_MSL---\n{fs_msl}---END_FS_MSL---");
        #[cfg(feature = "metal-backend")]
        {
            let native = native_uber_msl(&wgsl);
            println!("---BEGIN_NATIVE_MSL_BLAKE3---\n{}\n---END_NATIVE_MSL_BLAKE3---", native_uber_msl_hash(&native).to_hex());
            println!("---BEGIN_NATIVE_METAL_FLAGS---\n{NATIVE_METAL_COMPILE_FLAGS}\n---END_NATIVE_METAL_FLAGS---");
            println!("---BEGIN_NATIVE_VS_MSL---\n{}---END_NATIVE_VS_MSL---", native[0]);
            println!("---BEGIN_NATIVE_FS_MSL---\n{}---END_NATIVE_FS_MSL---", native[1]);
        }
        // The Windfoil variant (render/variant.rs), for uber_windfoil{,_native}.metallib.
        let windfoil = super::super::variant::windfoil_wgsl().expect("the Windfoil uber shader");
        let entries = super::super::variant::windfoil_bind_entries();
        let (vs_msl, fs_msl, _) = uber_msl_with(&windfoil, &entries, &UBER_VERTEX_ATTRS, std::mem::size_of::<super::super::Instance>() as u32);
        println!("---BEGIN_WINDFOIL_WGSL_BLAKE3---\n{}\n---END_WINDFOIL_WGSL_BLAKE3---", uber_wgsl_hash(&windfoil).to_hex());
        println!("---BEGIN_WINDFOIL_ABI_FNV64---\n{:016x}\n---END_WINDFOIL_ABI_FNV64---", super::super::variant::windfoil_abi_hash());
        println!("---BEGIN_WINDFOIL_VS_MSL---\n{vs_msl}---END_WINDFOIL_VS_MSL---");
        println!("---BEGIN_WINDFOIL_FS_MSL---\n{fs_msl}---END_WINDFOIL_FS_MSL---");
        #[cfg(feature = "metal-backend")]
        {
            let native = crate::gpu::metal::render_msl(&windfoil, &entries, NATIVE_MSL_VERSION);
            println!("---BEGIN_WINDFOIL_NATIVE_VS_MSL---\n{}---END_WINDFOIL_NATIVE_VS_MSL---", native[0]);
            println!("---BEGIN_WINDFOIL_NATIVE_FS_MSL---\n{}---END_WINDFOIL_NATIVE_FS_MSL---", native[1]);
        }
    }

    #[test]
    fn precompiled_uber_metallib_is_fresh() {
        let wgsl = uber_wgsl();
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            uber_wgsl_hash(&wgsl).to_hex().as_str(),
            UBER_WGSL_BLAKE3,
            "WGSL changed; run this freshness test on macOS to check whether MSL needs regeneration"
        );
        assert!(
            UBER_METALLIB.starts_with(b"MTLB") && UBER_METALLIB.len() > 1024,
            "uber.metallib must be a valid Metal library binary"
        );
        assert_eq!(
            fnv1a64(UBER_METALLIB),
            UBER_METALLIB_FNV64,
            "crates/rune/src/uber.metallib does not match recorded hash; run scripts/precompile-metal-shaders.sh"
        );

        #[cfg(target_os = "macos")]
        {
            let stride = std::mem::size_of::<super::super::Instance>() as u32;
            let (vs_msl, fs_msl, options_hash) = uber_msl(&wgsl, &UBER_VERTEX_ATTRS, stride);
            assert_eq!(
                uber_msl_hash(&vs_msl, &fs_msl).to_hex().as_str(),
                UBER_MSL_BLAKE3,
                "generated MSL or Metal compile flags changed; run scripts/precompile-metal-shaders.sh"
            );

            let wgsl_hash = uber_wgsl_hash(&wgsl).to_hex().to_string();
            assert!(uber_abi_hash() == UBER_ABI_FNV64
                && options_hash == UBER_MSL_OPTIONS_FNV64
                && wgsl_hash == UBER_WGSL_BLAKE3,
                "MSL is byte-identical; no Xcode or metallib regeneration needed. Paste these lines into crates/rune/src/render/uber_metallib_hash.rs:\n\
pub(super) const UBER_ABI_FNV64: u64 = 0x{:016x};\n\
pub(super) const UBER_MSL_OPTIONS_FNV64: u64 = 0x{options_hash:016x};\n\
pub(super) const UBER_WGSL_BLAKE3: &str = \"{wgsl_hash}\";", uber_abi_hash());

            // Perturbing WGSL or vertex layout must change the hash so staleness cannot hide.
            let mut altered_wgsl = wgsl.clone();
            altered_wgsl.push_str("\n// perturbed\n");
            assert_ne!(uber_wgsl_hash(&altered_wgsl).to_hex().as_str(), UBER_WGSL_BLAKE3);

            let (vs_stride_changed, _, changed_options) = uber_msl(&wgsl, &UBER_VERTEX_ATTRS, stride + 16);
            assert_ne!(changed_options, options_hash);
            assert_ne!(
                uber_msl_hash(&vs_stride_changed, &fs_msl).to_hex().as_str(),
                UBER_MSL_BLAKE3
            );
        }
        #[cfg(all(target_os = "macos", feature = "metal-backend"))]
        {
            assert!(UBER_NATIVE_METALLIB.starts_with(b"MTLB") && UBER_NATIVE_METALLIB.len() > 1024);
            assert_eq!(fnv1a64(UBER_NATIVE_METALLIB), UBER_NATIVE_METALLIB_FNV64,
                "crates/rune/src/uber_native.metallib does not match recorded hash; run scripts/precompile-metal-shaders.sh");
            assert_eq!(native_uber_msl_hash(&native_uber_msl(&wgsl)).to_hex().as_str(), UBER_NATIVE_MSL_BLAKE3,
                "the Metal backend's MSL or its compile flags changed; run scripts/precompile-metal-shaders.sh");
        }
        #[cfg(not(target_os = "macos"))]
        assert_eq!(uber_abi_hash(), UBER_ABI_FNV64,
            "descriptor fingerprint changed; run this test on macOS for exact replacement hash lines (no Xcode if MSL is unchanged)");
    }

    #[test]
    fn semantic_fingerprint_tracks_descriptors_not_source_text() {
        let entries = uber_bind_entries();
        let vertex = uber_vertex_layout();
        let primitive = uber_primitive();
        let baseline = layout_hash(&entries, &vertex, primitive);
        let mut changed = entries;
        changed[0].visibility = wgpu::ShaderStages::VERTEX;
        assert_ne!(layout_hash(&changed, &vertex, primitive), baseline);
        changed = entries;
        changed[0].binding = 4;
        assert_ne!(layout_hash(&changed, &vertex, primitive), baseline);
        let mut changed_vertex = uber_vertex_layout();
        changed_vertex.array_stride += 16;
        assert_ne!(layout_hash(&entries, &changed_vertex, primitive), baseline);
        changed_vertex = uber_vertex_layout();
        changed_vertex.step_mode = wgpu::VertexStepMode::Vertex;
        assert_ne!(layout_hash(&entries, &changed_vertex, primitive), baseline);
        let mut attrs = UBER_VERTEX_ATTRS;
        attrs[0].offset += 4;
        changed_vertex = uber_vertex_layout();
        changed_vertex.attributes = &attrs;
        assert_ne!(layout_hash(&entries, &changed_vertex, primitive), baseline);
        let mut changed_primitive = primitive;
        changed_primitive.strip_index_format = Some(wgpu::IndexFormat::Uint32);
        assert_ne!(layout_hash(&entries, &vertex, changed_primitive), baseline);
        changed_primitive = primitive;
        changed_primitive.topology = wgpu::PrimitiveTopology::TriangleList;
        assert_ne!(layout_hash(&entries, &vertex, changed_primitive), baseline);
        let groups = [None, None];
        let mut layout = uber_pipeline_layout(&groups, false);
        layout.immediate_size = 16;
        assert_ne!(descriptor_hash(&entries, &vertex, primitive, &layout), baseline);
        let layout = uber_pipeline_layout(&groups, true);
        assert_ne!(descriptor_hash(&entries, &vertex, primitive, &layout), baseline);
        // Labels, comments, and unrelated Rust source are deliberately not inputs.
        assert_eq!(layout_hash(&entries, &vertex, primitive), baseline);
        #[cfg(target_os = "macos")]
        {
            let wgsl = uber_wgsl();
            let stride = vertex.array_stride as u32;
            let original = uber_msl(&wgsl, vertex.attributes, stride);
            let comment_only = uber_msl(&format!("{wgsl}\n// unrelated comment\n"), vertex.attributes, stride);
            assert_eq!(original, comment_only, "comment edits must not require a Metal compiler");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn uber_shader_loads_precompiled_metallib_and_falls_back_cleanly() {
        use super::super::{Gpu, OFFSCREEN_FORMAT, Renderer};

        let gpu = Gpu::headless_wgpu();
        assert!(
            gpu.device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS),
            "Metal device must enable PASSTHROUGH_SHADERS"
        );

        let before = METALLIB_LOADS.with(std::cell::Cell::get);
        let _r_on = with_metallib_enabled(true, || Renderer::new(&gpu.device, OFFSCREEN_FORMAT));
        let after_on = METALLIB_LOADS.with(std::cell::Cell::get);
        assert_eq!(after_on, before + 1, "Renderer::new must load precompiled uber.metallib on macOS");

        let _r_off = with_metallib_enabled(false, || Renderer::new(&gpu.device, OFFSCREEN_FORMAT));
        let after_off = METALLIB_LOADS.with(std::cell::Cell::get);
        assert_eq!(after_off, after_on, "disabled metallib must fall back to WGSL");

        let wgsl = uber_wgsl();
        assert!(
            try_uber_metallib(&gpu.device, &wgsl, UBER_METALLIB, "stale-hash", |_| ()).is_none(),
            "hash mismatch must fall back"
        );
        assert!(
            try_uber_metallib(&gpu.device, &wgsl, b"not-mtlb", UBER_WGSL_BLAKE3, |_| ()).is_none(),
            "missing MTLB magic must fall back"
        );
        assert!(
            try_uber_metallib(&gpu.device, &wgsl, b"MTLBcorrupt-payload", UBER_WGSL_BLAKE3, |_| ()).is_none(),
            "foreign or corrupt metallib must be rejected before passing bytes to the driver"
        );

        // Device created without PASSTHROUGH_SHADERS must fall back cleanly to WGSL.
        let (no_pt_device, _no_pt_queue) = pollster::block_on(gpu.adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("no-passthrough"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: Default::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: Default::default(),
            },
        ))
        .expect("device without passthrough");
        assert!(
            try_uber_metallib(&no_pt_device, &wgsl, UBER_METALLIB, UBER_WGSL_BLAKE3, |_| ()).is_none(),
            "device without PASSTHROUGH_SHADERS must fall back"
        );
        let _r_fallback = Renderer::new(&no_pt_device, OFFSCREEN_FORMAT);
    }

    #[cfg(all(target_os = "macos", feature = "metal-backend"))]
    #[test]
    fn native_backend_loads_its_metallib_and_falls_back_cleanly() {
        use super::super::{Gpu, OFFSCREEN_FORMAT, Renderer};

        let gpu = Gpu::new(crate::gpu::metal::instance(), None);
        let before = METALLIB_LOADS.with(std::cell::Cell::get);
        let _on = with_metallib_enabled(true, || Renderer::new(&gpu.device, OFFSCREEN_FORMAT));
        assert_eq!(METALLIB_LOADS.with(std::cell::Cell::get), before + 1, "Renderer::new must load uber_native.metallib");

        let wgsl = uber_wgsl();
        assert!(try_uber_metallib(&gpu.device, &wgsl, UBER_METALLIB, UBER_WGSL_BLAKE3, |_| ()).is_none(),
            "wgpu-hal's vertex-pulling library must not load on Rune's backend");
        let module = |bytes: &'static [u8], names: &'static [&'static str]| {
            let entry_points: Vec<_> = names.iter()
                .map(|n| wgpu::PassthroughShaderEntryPoint { name: std::borrow::Cow::Borrowed(*n), workgroup_size: (0, 0, 0) })
                .collect();
            // SAFETY: the backend only hands the bytes to Metal, which checks them.
            let module = unsafe {
                gpu.device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                    entry_points: entry_points.into(),
                    metallib: Some(std::borrow::Cow::Borrowed(bytes)),
                    ..Default::default()
                })
            };
            crate::gpu::metal::module_ready(&module)
        };
        assert!(module(UBER_NATIVE_METALLIB, &["vs", "fs"]));
        assert!(!module(UBER_NATIVE_METALLIB, &["vs", "missing"]), "a missing function must fall back");
        assert!(!module(b"MTLBcorrupt-payload", &["vs", "fs"]), "a library Metal refuses must fall back");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pipeline_linkage_failure_retries_wgsl_without_uncaptured_error() {
        use super::super::{Blend, Gpu, OFFSCREEN_FORMAT, Renderer};

        let gpu = Gpu::headless_wgpu();
        let renderer = with_metallib_enabled(false, || Renderer::new(&gpu.device, OFFSCREEN_FORMAT));
        let before = METALLIB_LOADS.with(std::cell::Cell::get);
        let mut attempts = 0;
        let (_shader, _pipeline) = with_metallib_enabled(true, || {
            uber_shader_and_pipeline(&gpu.device, |shader| {
                attempts += 1;
                // Loading the library succeeds; resolving this function fails in HAL's
                // load_shader at pipeline creation. The default uncaptured handler panics
                // if the pipeline is outside the fallback's error scopes.
                let entry = if attempts == 1 { "missing_vertex_entry" } else { "vs" };
                Renderer::pipeline(&gpu.device, &renderer.layout, shader, OFFSCREEN_FORMAT,
                    entry, Blend::SourceOver)
            })
        });
        assert_eq!(attempts, 2, "a loaded library can still fail at pipeline creation");
        assert_eq!(METALLIB_LOADS.with(std::cell::Cell::get), before,
            "a failed pipeline must not retain the passthrough shader");
    }

    /// The precompiled uber shader (a metallib on macOS, DXBC on Windows) and WGSL draw the same pixels.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn metallib_and_wgsl_render_identical_pixels() {
        #[cfg(target_os = "macos")]
        use super::with_metallib_enabled as with_precompiled;
        #[cfg(target_os = "windows")]
        use super::super::dxbc::with_dxbc_enabled as with_precompiled;
        use super::super::{
            Blend, Gpu, Instance, KIND_GLYPH, KIND_GRADIENT, KIND_IMAGE, KIND_INSET, KIND_RECT,
            KIND_SEGMENT, KIND_SHADOW, KIND_TRIANGLE, LayerSpec, NO_CLIP, OFFSCREEN_FORMAT,
            Offscreen, Renderer, pack_fade,
        };
        use crate::{
            pixels::ImageAtlas,
            text::{ATLAS_SIZE, TextSystem},
        };
        use bytemuck::Zeroable;

        let gpu = Gpu::headless_wgpu();
        #[cfg(all(target_os = "macos", feature = "metal-backend"))]
        let native = Gpu::new(crate::gpu::metal::instance(), None);
        let (width, height) = (160u32, 120u32);

        let render_scene = |gpu: &Gpu, use_metallib: bool, format: wgpu::TextureFormat, srgb_blend: bool| {
            with_precompiled(use_metallib, || {
                let mut renderer = Renderer::new(&gpu.device, format);
                renderer.set_srgb_blend(srgb_blend);

                let mut text = TextSystem::default();
                for y in 0..16u32 {
                    for x in 0..16u32 {
                        text.atlas.data[(y * ATLAS_SIZE + x) as usize] =
                            ((x * 15 + y * 9) & 0xff) as u8;
                    }
                }
                text.atlas.dirty = Some((0, 16));

                let mut images = ImageAtlas::default();
                let px = crate::pixels::Pixels::new();
                px.set(
                    4,
                    4,
                    (0..16)
                        .flat_map(|i| [200u8, (i * 12) as u8, 90, 255])
                        .collect(),
                );
                let placed = images.place(&px, [24.0, 24.0]).expect("place test pixels");
                renderer.upload_images(&gpu.device, &gpu.queue, &mut images);

                let mut instances = vec![
                    // Rounded rect with border
                    Instance {
                        rect: [8.0, 8.0, 44.0, 32.0],
                        radii: [8.0, 8.0, 8.0, 8.0],
                        fill: [0.2, 0.45, 0.85, 1.0],
                        border: [0.9, 0.95, 1.0, 1.0],
                        params: [2.0, KIND_RECT, 0.0, 0.0],
                        clip: NO_CLIP,
                        ..Instance::zeroed()
                    },
                    // Continuous-corner rect (negative radii) with edge fade
                    Instance {
                        rect: [60.0, 8.0, 44.0, 32.0],
                        radii: [-10.0; 4],
                        fill: [0.85, 0.3, 0.4, 0.9],
                        params: [0.0, KIND_RECT, 0.0, 0.0],
                        clip: [58.0, 6.0, 106.0, 42.0],
                        fade: pack_fade([6.0, 6.0, 6.0, 6.0], [58.0, 6.0, 106.0, 42.0], [58.0, 6.0, 106.0, 42.0]),
                        ..Instance::zeroed()
                    },
                    // Drop shadow + inset shadow
                    Instance {
                        rect: [112.0, 10.0, 36.0, 28.0],
                        radii: [6.0; 4],
                        fill: [0.0, 0.0, 0.0, 0.55],
                        params: [0.0, KIND_SHADOW, 6.0, 0.0],
                        clip: NO_CLIP,
                        ..Instance::zeroed()
                    },
                    Instance {
                        rect: [112.0, 10.0, 36.0, 28.0],
                        radii: [6.0; 4],
                        fill: [0.1, 0.1, 0.15, 0.5],
                        params: [2.0, KIND_INSET, 4.0, 0.0],
                        uv: [1.0, 2.0, 0.0, 0.0],
                        clip: NO_CLIP,
                        ..Instance::zeroed()
                    },
                    // Vertical gradient
                    Instance {
                        rect: [8.0, 48.0, 44.0, 30.0],
                        radii: [6.0; 4],
                        fill: [0.15, 0.7, 0.55, 1.0],
                        border: [0.8, 0.8, 0.2, 1.0],
                        params: [1.5, KIND_GRADIENT, 0.0, 0.0],
                        uv: [0.05, 0.2, 0.35, 1.0],
                        clip: NO_CLIP,
                        ..Instance::zeroed()
                    },
                    // Stroke segment (round caps) + triangle
                    Instance {
                        rect: [56.0, 48.0, 48.0, 30.0],
                        radii: [60.0, 52.0, 100.0, 74.0],
                        fill: [0.95, 0.6, 0.2, 1.0],
                        params: [2.5, KIND_SEGMENT, 1.0, 0.0],
                        clip: NO_CLIP,
                        ..Instance::zeroed()
                    },
                    Instance {
                        rect: [108.0, 48.0, 44.0, 30.0],
                        radii: [112.0, 74.0, 130.0, 50.0],
                        fill: [0.6, 0.35, 0.9, 0.95],
                        params: [7.0, KIND_TRIANGLE, 0.0, 0.0],
                        uv: [148.0, 74.0, 0.0, 0.0],
                        clip: NO_CLIP,
                        ..Instance::zeroed()
                    },
                    // Glyph + image
                    Instance {
                        rect: [12.0, 86.0, 16.0, 16.0],
                        fill: [1.0, 1.0, 1.0, 1.0],
                        params: [0.0, KIND_GLYPH, 0.0, 1.45],
                        uv: [0.0, 0.0, 16.0, 16.0],
                        clip: NO_CLIP,
                        ..Instance::zeroed()
                    },
                    Instance {
                        rect: [36.0, 84.0, 24.0, 24.0],
                        radii: [4.0; 4],
                        fill: [0.0, 0.0, 0.0, 0.9],
                        params: [placed.page as f32, KIND_IMAGE, 0.0, 0.0],
                        uv: placed.rect.map(|v| v as f32),
                        clip: NO_CLIP,
                        ..Instance::zeroed()
                    },
                ];

                // Isolated layer with PlusLighter blend and rounded clip
                instances.push(Instance::begin_layer(
                    [70.0, 82.0, 76.0, 30.0],
                    [70.0, 82.0, 146.0, 112.0],
                    LayerSpec {
                        opacity: 0.8,
                        blend: Blend::PlusLighter,
                        blur: 0.0,
                        clip_radius: 6.0,
                    },
                ));
                instances.push(Instance {
                    rect: [74.0, 86.0, 40.0, 22.0],
                    radii: [5.0; 4],
                    fill: [0.3, 0.5, 0.7, 0.85],
                    params: [0.0, KIND_RECT, 0.0, 0.0],
                    clip: NO_CLIP,
                    ..Instance::zeroed()
                });
                instances.push(Instance {
                    rect: [96.0, 86.0, 40.0, 22.0],
                    radii: [5.0; 4],
                    fill: [0.7, 0.3, 0.4, 0.85],
                    params: [0.0, KIND_RECT, 0.0, 0.0],
                    clip: NO_CLIP,
                    ..Instance::zeroed()
                });
                instances.push(Instance::end_layer());

                renderer.prepare(&gpu.device, &gpu.queue, &mut text.atlas, &instances, (width, height));
                let target = Offscreen::with_format(&gpu.device, width, height, format);
                let mut encoder = gpu.device.create_command_encoder(&Default::default());
                renderer.draw(&mut encoder, &target.view, [0.08, 0.09, 0.12, 1.0]);
                target.copy(&mut encoder);
                renderer.submit(&gpu.queue, encoder);
                target.read(&gpu.device)
            })
        };

        for (format, srgb_blend) in [
            (OFFSCREEN_FORMAT, false),
            (wgpu::TextureFormat::Bgra8UnormSrgb, false),
            (wgpu::TextureFormat::Rgba8Unorm, true),
        ] {
            let metallib_pixels = render_scene(&gpu, true, format, srgb_blend);
            let wgsl_pixels = render_scene(&gpu, false, format, srgb_blend);
            assert_eq!(
                metallib_pixels, wgsl_pixels,
                "metallib and WGSL pixel output differed for format={format:?} srgb_blend={srgb_blend}"
            );
            #[cfg(all(target_os = "macos", feature = "metal-backend"))]
            for use_metallib in [true, false] {
                assert!(
                    render_scene(&native, use_metallib, format, srgb_blend) == wgsl_pixels,
                    "Rune's Metal backend and wgpu's differed for format={format:?} srgb_blend={srgb_blend} metallib={use_metallib}"
                );
            }
        }
    }
}
