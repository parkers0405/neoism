//! macOS: the Windfoil uber shader's precompiled libraries (render/variant.rs), which
//! `scripts/precompile-metal-shaders.sh` builds beside Classic's: `uber_windfoil.metallib` for
//! wgpu's Metal backend and `uber_windfoil_native.metallib` for Rune's. Only a Windfoil renderer
//! loads them, so they never touch Classic, the Mac's default. One that's missing or stale (its
//! WGSL or layout hash differs) is skipped with a log line, and Windfoil compiles from WGSL.

include!("uber_windfoil_hash.rs");

const LIBRARY: &[u8] = include_bytes!("../uber_windfoil.metallib");
#[cfg(feature = "metal-backend")]
const NATIVE_LIBRARY: &[u8] = include_bytes!("../uber_windfoil_native.metallib");

/// The Windfoil shader from the library for `device`'s backend and its pipeline, when the
/// library matches `wgsl` and `abi` and the device takes it.
pub(super) fn load<T>(device: &wgpu::Device, wgsl: &str, abi: u64, make_pipeline: impl FnOnce(&super::dxbc::Shaders) -> T) -> Option<(wgpu::ShaderModule, T)> {
    #[cfg(feature = "metal-backend")]
    let native = crate::gpu::metal::raw_device(device).is_some();
    #[cfg(not(feature = "metal-backend"))]
    let native = false;
    #[cfg(feature = "metal-backend")]
    let library = if native { NATIVE_LIBRARY } else { LIBRARY };
    #[cfg(not(feature = "metal-backend"))]
    let library = LIBRARY;
    if !super::metallib::metallib_enabled() {
        return None;
    }
    if library.is_empty() || blake3::hash(wgsl.as_bytes()).to_hex().as_str() != WINDFOIL_WGSL_BLAKE3 || abi != WINDFOIL_ABI_FNV64 {
        static STALE: std::sync::Once = std::sync::Once::new();
        STALE.call_once(|| eprintln!("rune: uber_windfoil.metallib is missing or stale, so Windfoil text compiles its shader from WGSL; \
            run scripts/precompile-metal-shaders.sh on a Mac to rebuild it"));
        return None;
    }
    // Rune's backend takes a library without the feature, which promises arbitrary passthrough.
    if !native && !device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS) {
        return None;
    }
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    // SAFETY: only the embedded library is loaded, built by the script from `wgsl` (checked by
    // its hash) with the bindings `abi` hashes, as Classic's `uber.metallib` is.
    let shader = unsafe {
        device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
            label: Some("uber"),
            entry_points: std::borrow::Cow::Borrowed(&[
                wgpu::PassthroughShaderEntryPoint { name: std::borrow::Cow::Borrowed("vs"), workgroup_size: (0, 0, 0) },
                wgpu::PassthroughShaderEntryPoint { name: std::borrow::Cow::Borrowed("fs"), workgroup_size: (0, 0, 0) },
            ]),
            metallib: Some(std::borrow::Cow::Borrowed(library)),
            ..Default::default()
        })
    };
    // Rune's backend reports no errors to scopes, so check it loaded and has both functions.
    #[cfg(feature = "metal-backend")]
    if native && !crate::gpu::metal::module_ready(&shader) {
        let _ = (pollster::block_on(internal.pop()), pollster::block_on(validation.pop()));
        eprintln!("rune: uber_windfoil_native.metallib didn't load; Windfoil text compiles its shader from WGSL");
        return None;
    }
    let pipeline = make_pipeline(&super::dxbc::Shaders::one(shader.clone()));
    if let Some(error) = pollster::block_on(internal.pop()).or(pollster::block_on(validation.pop())) {
        eprintln!("rune: uber_windfoil.metallib was refused ({error}); Windfoil text compiles its shader from WGSL");
        return None;
    }
    Some((shader, pipeline))
}
