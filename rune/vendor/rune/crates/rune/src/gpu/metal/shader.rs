//! Shader modules stay naga IR until a pipeline names an entry point; then naga translates it
//! to MSL with the pipeline layout's slots and Metal compiles it, as wgpu-hal does. A pipeline
//! of Rune's own (`builtin`) takes that function precompiled instead, and a module whose every
//! pipeline is one of those is never parsed. A module from a precompiled `.metallib`
//! (`create_shader_module_passthrough`) skips both: its functions are looked up by name, and
//! must read no sizes buffer.

use std::ffi::c_void;
use std::future::ready;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2_foundation_video::{NSError, NSString};
use objc2_metal::{MTLCompileOptions, MTLDevice, MTLFunction, MTLLibrary};
use objc2_video::msg_send;
use objc2_video::rc::Retained;
use objc2_video::runtime::{AnyObject, ProtocolObject};
use wgpu::custom::*;
use wgpu::naga::{
    self,
    back::{msl, pipeline_constants::process_overrides},
    proc::{BoundsCheckPolicies, BoundsCheckPolicy},
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::{Obj, Shared, builtin, send_sync};

#[derive(Debug)]
pub(crate) enum ShaderModule {
    Naga(Ir),
    /// A precompiled library, or why Metal refused it, and its entry points' workgroup sizes.
    Library { library: Result<Obj<dyn MTLLibrary>, String>, entries: Vec<(String, [u32; 3])> },
}
send_sync!(ShaderModule);

/// A WGSL module, validated when first needed, and the runtime checks its MSL keeps.
#[derive(Debug)]
pub(crate) struct Ir {
    label: String,
    source: String,
    pub(super) hash: blake3::Hash,
    parsed: OnceLock<(naga::Module, naga::valid::ModuleInfo)>,
    pub(super) checks: wgpu::ShaderRuntimeChecks,
}

/// An entry point compiled for one pipeline, and the storage bindings whose byte sizes it reads
/// from the sizes buffer, in the order it reads them.
pub(super) struct Compiled {
    pub(super) function: Obj<dyn MTLFunction>,
    pub(super) sized: Vec<naga::ResourceBinding>,
    /// Threads per workgroup, for a compute entry point.
    pub(super) workgroup: [u32; 3],
}

/// One stage of a pipeline, as `compile` needs it.
pub(super) struct Stage<'a> {
    pub(super) module: &'a wgpu::ShaderModule,
    pub(super) entry: Option<&'a str>,
    pub(super) constants: &'a [(&'a str, f64)],
    pub(super) kind: naga::ShaderStage,
}

/// The entry point a stage runs, named or the module's only one of its kind.
pub(super) struct Point<'a> {
    pub(super) kind: naga::ShaderStage,
    pub(super) entry: Option<&'a str>,
    pub(super) constants: &'a [(&'a str, f64)],
}

/// An entry point translated to MSL: the source, the function's name in it, and what `Compiled` keeps.
pub(super) struct Msl {
    pub(super) source: String,
    pub(super) function: String,
    pub(super) sized: Vec<naga::ResourceBinding>,
    pub(super) workgroup: [u32; 3],
}

#[link(name = "System")]
unsafe extern "C" {
    fn dispatch_data_create(buffer: NonNull<c_void>, size: usize, queue: *mut c_void, destructor: *mut c_void) -> *mut AnyObject;
    fn dispatch_release(object: *mut AnyObject);
}

impl Ir {
    /// Parses and validates `source` now, unless it is a built-in shader, which parses only if a
    /// pipeline needs a function the precompiled library lacks.
    pub(super) fn new(label: &str, source: &str, checks: wgpu::ShaderRuntimeChecks) -> Self {
        let hash = blake3::hash(source.as_bytes());
        let ir = Ir { label: label.into(), source: source.into(), hash, parsed: OnceLock::new(), checks };
        if !builtin::known(&hash) { ir.parsed(); }
        ir
    }

    #[cfg(test)]
    pub(super) fn parse(label: &str, source: &str, checks: wgpu::ShaderRuntimeChecks) -> Self {
        let ir = Ir { label: label.into(), source: source.into(), hash: blake3::hash(source.as_bytes()), parsed: OnceLock::new(), checks };
        ir.parsed();
        ir
    }

    fn parsed(&self) -> &(naga::Module, naga::valid::ModuleInfo) {
        self.parsed.get_or_init(|| {
            let (label, source) = (&self.label, &self.source);
            let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
            let info = Validator::new(ValidationFlags::all(), Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
            (module, info)
        })
    }

    pub(super) fn parts(&self) -> (&naga::Module, &naga::valid::ModuleInfo) {
        let (module, info) = self.parsed();
        (module, info)
    }
}

impl ShaderModule {
    pub(crate) fn new(desc: wgpu::ShaderModuleDescriptor<'_>, checks: wgpu::ShaderRuntimeChecks) -> Self {
        let wgpu::ShaderSource::Wgsl(source) = desc.source else { super::unsupported!("shaders other than WGSL") };
        ShaderModule::Naga(Ir::new(desc.label.unwrap_or("shader"), &source, checks))
    }

    pub(crate) fn passthrough(shared: &Shared, desc: &wgpu::ShaderModuleDescriptorPassthrough<'_>) -> Self {
        let bytes = desc.metallib.as_deref().unwrap_or_else(|| super::unsupported!("passthrough shaders other than a metallib"));
        let entries = desc.entry_points.iter().map(|e| (e.name.to_string(), [e.workgroup_size.0, e.workgroup_size.1, e.workgroup_size.2])).collect();
        ShaderModule::Library { library: library(shared, bytes), entries }
    }

    /// Whether every entry point can make a pipeline: a precompiled library loaded and holds
    /// them all. A module from WGSL already passed validation.
    pub(crate) fn ready(&self) -> bool {
        match self {
            ShaderModule::Naga(_) => true,
            ShaderModule::Library { library: Ok(library), entries } => {
                let names = library.functionNames();
                entries.iter().all(|(name, _)| names.iter().any(|n| n.to_string() == *name))
            }
            ShaderModule::Library { library: Err(_), .. } => false,
        }
    }
}

impl ShaderModuleInterface for ShaderModule {
    fn get_compilation_info(&self) -> Pin<Box<dyn ShaderCompilationInfoFuture>> {
        Box::pin(ready(wgpu::CompilationInfo { messages: Vec::new() }))
    }
}

pub(super) fn compile(shared: &Shared, stage: &Stage<'_>, resources: &msl::EntryPointResources, points: bool) -> Result<Compiled, String> {
    let shader = stage.module.as_custom::<ShaderModule>().expect("a Metal shader module");
    match shader {
        ShaderModule::Naga(ir) => {
            let point = Point { kind: stage.kind, entry: stage.entry, constants: stage.constants };
            let key = builtin::key(ir, &point, resources, points);
            if let Some(compiled) = builtin::find(shared, &key) {
                return Ok(compiled);
            }
            builtin::capture(ir, &point, resources, points, &key);
            let (lang, version) = shared.msl;
            let msl = translate(ir, &point, resources, points, lang)?;
            let compile = MTLCompileOptions::new();
            compile.setLanguageVersion(version);
            compile.setPreserveInvariance(true);
            let library = shared.device.newLibraryWithSource_options_error(&NSString::from_str(&msl.source), Some(&compile))
                .map_err(|e| format!("{}: Metal: {}", msl.function, e.localizedDescription()))?;
            let function = function(&library, &msl.function)?;
            Ok(Compiled { function, sized: msl.sized, workgroup: msl.workgroup })
        }
        ShaderModule::Library { library, entries } => {
            let library = library.as_ref().map_err(|e| format!("metallib: {e}"))?;
            if resources.sizes_buffer.is_some() {
                return Err("a precompiled library can't read storage buffers' sizes".into());
            }
            let (name, workgroup) = match stage.entry {
                Some(name) => entries.iter().find(|(n, _)| n == name).ok_or_else(|| format!("no entry point {name}"))?,
                None => match entries.as_slice() {
                    [entry] => entry,
                    _ => return Err(format!("no single {:?} entry point", stage.kind)),
                },
            };
            Ok(Compiled { function: function(library, name)?, sized: Vec::new(), workgroup: *workgroup })
        }
    }
}

/// The library a `.metallib`'s bytes hold, or why Metal refused it.
pub(super) fn library(shared: &Shared, bytes: &[u8]) -> Result<Obj<dyn MTLLibrary>, String> {
    NonNull::new(bytes.as_ptr().cast_mut()).ok_or_else(|| "an empty metallib".to_string()).and_then(|buffer| {
        // SAFETY: dispatch copies `bytes` (the default destructor), and the data object is
        // released after Metal has read it.
        unsafe {
            let data = dispatch_data_create(buffer.cast(), bytes.len(), std::ptr::null_mut(), std::ptr::null_mut());
            assert!(!data.is_null(), "dispatch data for a metallib");
            let library: Result<Retained<ProtocolObject<dyn MTLLibrary>>, Retained<NSError>> =
                msg_send![&*shared.device, newLibraryWithData: data, error: _];
            dispatch_release(data);
            library.map_err(|e| e.localizedDescription().to_string())
        }
    })
}

pub(super) fn function(library: &ProtocolObject<dyn MTLLibrary>, name: &str) -> Result<Obj<dyn MTLFunction>, String> {
    library.newFunctionWithName(&NSString::from_str(name)).ok_or_else(|| format!("no function {name}"))
}

/// Translates `point` to MSL `lang`, binding `resources`.
pub(super) fn translate(ir: &Ir, point: &Point<'_>, resources: &msl::EntryPointResources, points: bool, lang: (u8, u8)) -> Result<Msl, String> {
    let (module, info) = ir.parsed();
    let checks = &ir.checks;
    let name = match point.entry {
        Some(name) => name.to_string(),
        None => {
            let mut eps = module.entry_points.iter().filter(|e| e.stage == point.kind);
            match (eps.next(), eps.next()) {
                (Some(ep), None) => ep.name.clone(),
                _ => return Err(format!("no single {:?} entry point", point.kind)),
            }
        }
    };
    let constants: naga::back::PipelineConstants = point.constants.iter().map(|&(k, v)| (k.to_string(), v)).collect();
    let (module, info) = process_overrides(module, info, Some((point.kind, &name)), &constants)
        .map_err(|e| format!("{name}: overrides: {e:?}"))?;
    let policy = if checks.bounds_checks { BoundsCheckPolicy::Restrict } else { BoundsCheckPolicy::Unchecked };
    let options = msl::Options {
        lang_version: lang,
        inline_samplers: Default::default(),
        spirv_cross_compatibility: false,
        fake_missing_bindings: false,
        per_entry_point_map: msl::EntryPointResourceMap::from([(name.clone(), resources.clone())]),
        bounds_check_policies: BoundsCheckPolicies { index: policy, buffer: policy, image_load: policy, binding_array: BoundsCheckPolicy::Unchecked },
        zero_initialize_workgroup_memory: true,
        force_loop_bounding: checks.force_loop_bounding,
        task_dispatch_limits: None,
        mesh_shader_primitive_indices_clamp: checks.mesh_shader_primitive_indices_clamp,
        emit_int_div_checks: checks.int_div_checks,
        ray_query_initialization_tracking: checks.ray_query_initialization_tracking,
    };
    let pipeline = msl::PipelineOptions {
        entry_point: Some((point.kind, name.clone())),
        allow_and_force_point_size: points,
        vertex_pulling_transform: false,
        vertex_buffer_mappings: Vec::new(),
        binding_array_length_map: Default::default(),
    };
    let (source, translated) = msl::write_string(&module, &info, &options, &pipeline).map_err(|e| format!("{name}: MSL: {e:?}"))?;
    let function = translated.entry_point_names[0].as_ref().map_err(|e| format!("{name}: {e}"))?.clone();
    // The sizes buffer holds one length per global that needs it, in global order, as naga reads it.
    let sized = module.global_variables.iter()
        .filter(|(_, g)| matches!(g.space, naga::AddressSpace::Uniform | naga::AddressSpace::Storage { .. }))
        .filter(|(_, g)| module.types[g.ty].inner.needs_host_buffer_byte_size(&module.types))
        .filter_map(|(_, g)| g.binding)
        .collect();
    let workgroup = module.entry_points.iter().find(|e| e.stage == point.kind && e.name == name).map_or([1; 3], |e| e.workgroup_size);
    Ok(Msl { source, function, sized, workgroup })
}
