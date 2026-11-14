//! Every WGSL shader in rune, rune-fx and rune-light, composed the way its crate composes it,
//! through naga to MSL with wgpu-hal's Metal options and then `newLibraryWithSource`, one library
//! per entry point as wgpu-hal builds them. The question it answers: can a native backend keep
//! WGSL as the one shader language, by translating with naga itself?

use objc2_foundation_video::NSString;
use objc2_metal::{MTLCompileOptions, MTLCreateSystemDefaultDevice, MTLDevice};
use objc2_video::runtime::ProtocolObject;
use super::device::msl_version;
use wgpu::naga::{
    self,
    back::{msl, pipeline_constants::process_overrides},
    proc::{BoundsCheckPolicies, BoundsCheckPolicy},
    valid::{Capabilities, ValidationFlags, Validator},
};

const CLIP: &str = include_str!("../../clip.wgsl");
const FX_COMMON: &str = include_str!("../../../../rune-fx/src/shaders/common.wgsl");
const LIGHT_COMMON: &str = include_str!("../../../../rune-light/src/shaders/common.wgsl");
const GI: &str = include_str!("../../../../rune-light/src/shaders/gi.wgsl");
const GLASS: &str = include_str!("../../../../rune-light/src/shaders/glass.wgsl");
const REEL: &str = include_str!("../../../../rune-light/examples/reel/reel.wgsl");

/// One shader module as its crate builds it: the files it is made of, its source, and the
/// pipeline constants it is built with (one module per set).
struct Case {
    name: String,
    files: Vec<&'static str>,
    source: String,
    constants: Vec<Vec<(&'static str, f64)>>,
}

fn case(name: &str, files: &[&'static str], source: String) -> Case {
    Case { name: name.into(), files: files.to_vec(), source, constants: vec![Vec::new()] }
}

/// `gi_section` in rune-light's gpu.rs.
fn gi_section(name: &str) -> String {
    let mut parts = GI.split("// ---- ");
    let header = parts.next().unwrap_or_default();
    let body = parts.find(|p| p.starts_with(name)).unwrap_or_default();
    format!("{LIGHT_COMMON}\n{header}\n// ---- {body}")
}

/// `section` in rune-light's glass.rs.
fn glass_section(name: &str) -> String {
    let (main, pyramid) = GLASS.split_once("// ---- pyramid ----").unwrap_or((GLASS, ""));
    if name == "pyramid" {
        format!("@vertex\nfn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {{\n    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));\n    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);\n}}\n{pyramid}")
    } else {
        main.to_string()
    }
}

/// `wgsl_part` in rune-light's reel example.
fn reel_part(name: &str) -> String {
    let (viz, rest) = REEL.split_once("// ---- bloom ----").unwrap();
    let (bloom, rest) = rest.split_once("// ---- per-probe mean of a cascade level ----").unwrap();
    let (mean, enc) = rest.split_once("// ---- final encode ----").unwrap();
    match name { "viz" => viz, "bloom" => bloom, "mean" => mean, _ => enc }.to_string()
}

fn cases() -> Vec<Case> {
    let mut cases = vec![
        case("rune uber", &["rune/src/clip.wgsl", "rune/src/shader.wgsl", "rune/src/gpu/springs.wgsl"],
            format!("{CLIP}\n{}\n{}", include_str!("../../shader.wgsl"), include_str!("../springs.wgsl"))),
        case("rune backdrop blur", &["rune/src/backdrop_blur.wgsl"], include_str!("../../backdrop_blur.wgsl").into()),
        case("rune backdrop composite", &["rune/src/clip.wgsl", "rune/src/backdrop_composite.wgsl"],
            format!("{CLIP}\n{}", include_str!("../../backdrop_composite.wgsl"))),
        case("rune video", &["rune/src/native_surface/video.wgsl"], include_str!("../../native_surface/video.wgsl").into()),
    ];
    let mut blur = case("rune layer blur", &["rune/src/layer_blur.wgsl"], include_str!("../../layer_blur.wgsl").into());
    blur.constants = [(0.0, 0.0), (0.0, 1.0), (2.0, 0.0), (2.0, 1.0), (3.0, 0.0)].iter().map(|&(m, a)| vec![("MODE", m), ("ADDITIVE", a)]).collect();
    cases.push(blur);
    let fx_effects: [(&str, &'static str, &str); 8] = [
        ("glow", "rune-fx/src/shaders/glow.wgsl", include_str!("../../../../rune-fx/src/shaders/glow.wgsl")),
        ("pulse", "rune-fx/src/shaders/pulse.wgsl", include_str!("../../../../rune-fx/src/shaders/pulse.wgsl")),
        ("electric", "rune-fx/src/shaders/electric.wgsl", include_str!("../../../../rune-fx/src/shaders/electric.wgsl")),
        ("fire", "rune-fx/src/shaders/fire.wgsl", include_str!("../../../../rune-fx/src/shaders/fire.wgsl")),
        ("distort", "rune-fx/src/shaders/distort.wgsl", include_str!("../../../../rune-fx/src/shaders/distort.wgsl")),
        ("motion", "rune-fx/src/shaders/motion.wgsl", include_str!("../../../../rune-fx/src/shaders/motion.wgsl")),
        ("extras", "rune-fx/src/shaders/extras.wgsl", include_str!("../../../../rune-fx/src/shaders/extras.wgsl")),
        ("fill", "rune-fx/src/shaders/fill.wgsl", include_str!("../../../../rune-fx/src/shaders/fill.wgsl")),
    ];
    for (name, file, src) in fx_effects {
        cases.push(case(&format!("fx {name}"), &["rune/src/clip.wgsl", "rune-fx/src/shaders/common.wgsl", file], format!("{CLIP}\n{FX_COMMON}\n{src}")));
    }
    cases.extend([
        case("fx mip", &["rune-fx/src/shaders/mip.wgsl"], include_str!("../../../../rune-fx/src/shaders/mip.wgsl").into()),
        case("fx blur", &["rune-fx/src/shaders/blur.wgsl"], include_str!("../../../../rune-fx/src/shaders/blur.wgsl").into()),
        case("fx firesim", &["rune-fx/src/shaders/firesim.wgsl"], include_str!("../../../../rune-fx/src/shaders/firesim.wgsl").into()),
        case("fx gallery ui", &["rune-fx/examples/gallery/ui.wgsl"], include_str!("../../../../rune-fx/examples/gallery/ui.wgsl").into()),
        case("fx gallery resolve", &["rune-fx/examples/gallery/resolve.wgsl"], include_str!("../../../../rune-fx/examples/gallery/resolve.wgsl").into()),
    ]);
    let light: [(&str, &'static str, &str); 4] = [
        ("gbuffer", "rune-light/src/shaders/gbuffer.wgsl", include_str!("../../../../rune-light/src/shaders/gbuffer.wgsl")),
        ("field", "rune-light/src/shaders/field.wgsl", include_str!("../../../../rune-light/src/shaders/field.wgsl")),
        ("shadow", "rune-light/src/shaders/shadow.wgsl", include_str!("../../../../rune-light/src/shaders/shadow.wgsl")),
        ("composite", "rune-light/src/shaders/composite.wgsl", include_str!("../../../../rune-light/src/shaders/composite.wgsl")),
    ];
    for (name, file, src) in light {
        cases.push(case(&format!("light {name}"), &["rune-light/src/shaders/common.wgsl", file], format!("{LIGHT_COMMON}\n{src}")));
    }
    for name in ["scene", "cascades", "fluence"] {
        cases.push(case(&format!("light gi {name}"), &["rune-light/src/shaders/common.wgsl", "rune-light/src/shaders/gi.wgsl"], gi_section(name)));
    }
    for name in ["glass", "pyramid"] {
        cases.push(case(&format!("light glass {name}"), &["rune-light/src/shaders/glass.wgsl"], glass_section(name)));
    }
    for name in ["viz", "bloom", "mean", "enc"] {
        cases.push(case(&format!("light reel {name}"), &["rune-light/examples/reel/reel.wgsl"], reel_part(name)));
    }
    cases
}

/// Slots as wgpu-hal hands them out for a layout holding every binding the module declares:
/// buffers, textures and samplers each counted up in (group, binding) order.
fn resources(module: &naga::Module, stage: naga::ShaderStage) -> msl::EntryPointResources {
    let mut bound: Vec<_> = module.global_variables.iter().filter_map(|(_, g)| Some((g.binding.clone()?, g))).collect();
    bound.sort_by_key(|(b, _)| (b.group, b.binding));
    let (mut buffers, mut textures, mut samplers) = (0u8, 0u8, 0u8);
    let mut out = msl::EntryPointResources::default();
    for (binding, global) in bound {
        let mut target = msl::BindTarget::default();
        match (global.space, &module.types[global.ty].inner) {
            (naga::AddressSpace::Uniform, _) => { target.buffer = Some(buffers); buffers += 1; }
            (naga::AddressSpace::Storage { access }, _) => {
                target.buffer = Some(buffers);
                target.mutable = access.contains(naga::StorageAccess::STORE);
                buffers += 1;
            }
            (_, naga::TypeInner::Sampler { .. }) => { target.sampler = Some(msl::BindSamplerTarget::Resource(samplers)); samplers += 1; }
            (_, naga::TypeInner::Image { class, .. }) => {
                target.texture = Some(textures);
                target.mutable = matches!(class, naga::ImageClass::Storage { access, .. } if access.contains(naga::StorageAccess::STORE));
                textures += 1;
            }
            (_, other) => panic!("binding type this test doesn't map yet: {other:?}"),
        }
        out.resources.insert(binding, target);
    }
    if stage == naga::ShaderStage::Vertex || buffers > 0 { out.sizes_buffer = Some(buffers); }
    out
}

fn vertex_format(inner: &naga::TypeInner) -> Option<wgpu::VertexFormat> {
    use wgpu::VertexFormat as V;
    let (n, scalar) = match *inner {
        naga::TypeInner::Scalar(s) => (1, s),
        naga::TypeInner::Vector { size, scalar } => (size as u8, scalar),
        _ => return None,
    };
    Some(match (scalar.kind, n) {
        (naga::ScalarKind::Float, 1) => V::Float32,
        (naga::ScalarKind::Float, 2) => V::Float32x2,
        (naga::ScalarKind::Float, 3) => V::Float32x3,
        (naga::ScalarKind::Float, _) => V::Float32x4,
        (naga::ScalarKind::Uint, 1) => V::Uint32,
        (naga::ScalarKind::Uint, 2) => V::Uint32x2,
        (naga::ScalarKind::Uint, 3) => V::Uint32x3,
        (naga::ScalarKind::Uint, _) => V::Uint32x4,
        (naga::ScalarKind::Sint, 1) => V::Sint32,
        (naga::ScalarKind::Sint, 2) => V::Sint32x2,
        (naga::ScalarKind::Sint, 3) => V::Sint32x3,
        _ => V::Sint32x4,
    })
}

/// One tightly packed vertex buffer at slot 30 holding every `@location` input, which is the
/// shape the vertex-pulling transform needs; the real pipelines' strides differ, the MSL doesn't.
fn vertex_buffers(module: &naga::Module, ep: &naga::EntryPoint) -> Vec<msl::VertexBufferMapping> {
    let mut inputs = Vec::new();
    for arg in &ep.function.arguments {
        match (&arg.binding, &module.types[arg.ty].inner) {
            (Some(naga::Binding::Location { location, .. }), inner) => inputs.push((*location, vertex_format(inner).unwrap())),
            (None, naga::TypeInner::Struct { members, .. }) => {
                for m in members {
                    if let Some(naga::Binding::Location { location, .. }) = m.binding {
                        inputs.push((location, vertex_format(&module.types[m.ty].inner).unwrap()));
                    }
                }
            }
            _ => {}
        }
    }
    if inputs.is_empty() { return Vec::new(); }
    let mut offset = 0;
    let attributes = inputs.into_iter().map(|(shader_location, format)| {
        let at = offset;
        offset += format.size() as u32;
        msl::AttributeMapping { shader_location, offset: at, format }
    }).collect();
    vec![msl::VertexBufferMapping { id: 30, stride: offset, step_mode: msl::VertexBufferStepMode::ByInstance, attributes }]
}

/// Translates and compiles each entry point; the error of the first that fails.
fn compile(device: &ProtocolObject<dyn MTLDevice>, case: &Case) -> Result<usize, String> {
    let module = naga::front::wgsl::parse_str(&case.source).map_err(|e| format!("parse: {}", e.emit_to_string(&case.source)))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .map_err(|e| format!("validate: {}", e.emit_to_string(&case.source)))?;
    let ((major, minor), version) = msl_version();
    let limits = wgpu::Limits::default();
    let mut libraries = 0;
    for constants in &case.constants {
        let constants: naga::back::PipelineConstants = constants.iter().map(|&(k, v)| (k.to_string(), v)).collect();
        for ep in &module.entry_points {
            let (ep_module, ep_info) = process_overrides(&module, &info, Some((ep.stage, &ep.name)), &constants)
                .map_err(|e| format!("{}: overrides: {e:?}", ep.name))?;
            let ep = ep_module.entry_points.iter().find(|e| e.name == ep.name && e.stage == ep.stage).expect("the entry point survives");
            let options = msl::Options {
                lang_version: (major, minor),
                inline_samplers: Default::default(),
                spirv_cross_compatibility: false,
                fake_missing_bindings: false,
                per_entry_point_map: msl::EntryPointResourceMap::from([(ep.name.clone(), resources(&ep_module, ep.stage))]),
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
            let pipeline = msl::PipelineOptions {
                entry_point: Some((ep.stage, ep.name.clone())),
                allow_and_force_point_size: false,
                vertex_pulling_transform: true,
                vertex_buffer_mappings: if ep.stage == naga::ShaderStage::Vertex { vertex_buffers(&ep_module, ep) } else { Vec::new() },
                binding_array_length_map: Default::default(),
            };
            let (source, _) = msl::write_string(&ep_module, &ep_info, &options, &pipeline).map_err(|e| format!("{}: msl: {e:?}", ep.name))?;
            let compile = MTLCompileOptions::new();
            compile.setLanguageVersion(version);
            compile.setPreserveInvariance(true);
            device.newLibraryWithSource_options_error(&NSString::from_str(&source), Some(&compile))
                .map_err(|e| format!("{}: metal: {}", ep.name, e.localizedDescription()))?;
            libraries += 1;
        }
    }
    Ok(libraries)
}

#[test]
fn every_wgsl_shader_compiles_to_a_metal_library() {
    let device = MTLCreateSystemDefaultDevice().expect("a Metal device");
    let mut files: Vec<(&str, bool)> = Vec::new();
    let mut failures = Vec::new();
    for case in cases() {
        let result = compile(&device, &case);
        match &result {
            Ok(n) => println!("ok    {:<26} {n} libraries", case.name),
            Err(e) => { println!("FAIL  {:<26} {e}", case.name); failures.push(case.name.clone()); }
        }
        for &file in &case.files {
            match files.iter_mut().find(|(f, _)| *f == file) {
                Some((_, ok)) => *ok &= result.is_ok(),
                None => files.push((file, result.is_ok())),
            }
        }
    }
    let passed = files.iter().filter(|(_, ok)| *ok).count();
    println!("{passed}/{} WGSL files compile in every module they are part of", files.len());
    assert_eq!(files.len(), 29, "a WGSL file was added or removed: update this test");
    assert!(failures.is_empty(), "failed: {failures:?}");
}
