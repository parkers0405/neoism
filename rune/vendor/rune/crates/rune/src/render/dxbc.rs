//! Precompiled DX12 bytecode for Rune's uber shader on Windows.
//!
//! wgpu 30's DX12 backend translates `shader.wgsl` through Naga to HLSL and compiles it with
//! FXC each time a device makes the uber pipeline: about 250 ms on a desktop CPU, the largest
//! part of a first surface's cost, and paid again by every new device. Rune ships the bytecode
//! FXC makes from that HLSL for the Windfoil variant (render/variant.rs), the one Windows renderers
//! draw with (`uber.vs.dxbc`, `uber.fs.dxbc`), and loads it with
//! `Device::create_shader_module_passthrough`; the pipeline then takes about 2 ms, as the
//! driver caches its own compile across launches. D3D12 takes FXC's DXBC as it takes DXIL.
//!
//! The bytecode must be what wgpu itself would compile: the same HLSL, whose register
//! assignments wgpu's root signature for the uber layout expects. `uber_dxbc_is_fresh`
//! (Windows) compares the HLSL wgpu generates now with the HLSL the bytecode came from, and
//! `cargo run -p rune --example regenerate_dxbc` rewrites both without building the tests. At
//! run time the bytecode is used only while the WGSL still hashes the same, and a pipeline the
//! driver refuses falls back to WGSL.

#[cfg(target_os = "windows")]
include!("uber_dxbc_hash.rs");

#[cfg(target_os = "windows")]
const VERTEX: &[u8] = include_bytes!("../../uber.vs.dxbc");
#[cfg(target_os = "windows")]
const FRAGMENT: &[u8] = include_bytes!("../../uber.fs.dxbc");

/// The uber shader's modules: one WGSL (or metallib) module for both stages, or one
/// precompiled module per stage.
#[derive(Clone)]
pub(crate) struct Shaders {
    pub vertex: wgpu::ShaderModule,
    pub fragment: wgpu::ShaderModule,
}

impl Shaders {
    pub(crate) fn one(module: wgpu::ShaderModule) -> Self {
        Self { vertex: module.clone(), fragment: module }
    }
}

/// The precompiled uber shader and its pipeline on a DX12 device, when the bytecode matches
/// `wgsl` and the pipeline ABI `abi` and the driver takes it.
#[cfg(target_os = "windows")]
pub(super) fn uber<T>(device: &wgpu::Device, wgsl: &str, abi: u64, make_pipeline: impl FnOnce(&Shaders) -> T) -> Option<(Shaders, T)> {
    if device.adapter_info().backend != wgpu::Backend::Dx12 || !device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS) {
        return None;
    }
    if !enabled() || blake3::hash(wgsl.as_bytes()).to_hex().as_str() != UBER_DXBC_WGSL_BLAKE3
        || abi != UBER_DXBC_ABI_FNV64 {
        return None;
    }
    let module = |blob: &'static [u8], entry: &'static str| {
        // SAFETY: the bytecode is FXC's compile of the HLSL wgpu generates for `wgsl` with the
        // uber layout (checked by `uber_dxbc_is_fresh`), so it binds what the layout declares.
        unsafe {
            device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                label: Some("uber"),
                entry_points: std::borrow::Cow::Owned(vec![wgpu::PassthroughShaderEntryPoint { name: entry.into(), workgroup_size: (0, 0, 0) }]),
                dxil: Some(std::borrow::Cow::Borrowed(blob)),
                ..Default::default()
            })
        }
    };
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let shaders = Shaders { vertex: module(VERTEX, "vs"), fragment: module(FRAGMENT, "fs") };
    let pipeline = make_pipeline(&shaders);
    let refused = pollster::block_on(internal.pop()).or(pollster::block_on(validation.pop()));
    match refused {
        None => Some((shaders, pipeline)),
        Some(error) => {
            eprintln!("rune: the precompiled uber shader was refused ({error}); compiling WGSL");
            None
        }
    }
}

#[cfg(target_os = "windows")]
std::thread_local! {
    static FORCE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// `RUNE_DISABLE_DXBC=1` compiles the WGSL instead, as before.
#[cfg(target_os = "windows")]
fn enabled() -> bool {
    FORCE.with(std::cell::Cell::get).unwrap_or_else(|| !matches!(std::env::var("RUNE_DISABLE_DXBC").as_deref(), Ok("1" | "true" | "TRUE" | "yes")))
}

#[cfg(all(test, target_os = "windows"))]
pub(super) fn with_dxbc_enabled<R>(enabled: bool, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<bool>);
    impl Drop for Restore {
        fn drop(&mut self) { FORCE.with(|c| c.set(self.0)); }
    }
    let _restore = Restore(FORCE.with(|c| c.replace(Some(enabled))));
    f()
}

#[cfg(all(test, target_os = "windows"))]
#[path = "dxbc_tests.rs"]
mod tests;
