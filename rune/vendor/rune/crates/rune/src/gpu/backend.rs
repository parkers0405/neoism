//! Which backend an Apple app draws with: Rune's native Metal backend (`metal-backend`, on by
//! default, and the default on macOS), or wgpu's.

use std::sync::atomic::{AtomicU8, Ordering};

/// The GPU backend an app draws with on macOS (`App::gpu_backend`) and iOS (`prewarm_gpu_on`, before
/// the first `embed::LayerHost`). Elsewhere wgpu picks one.
///
/// A Mac app that chooses neither draws with `Metal` when Rune's `metal-backend` feature is on (it
/// is by default), and with `Wgpu` when it's off; an iOS app draws with `Wgpu`. `Default` stays `Wgpu`; passing it to
/// `App::gpu_backend` opts out of the native backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuBackend {
    /// wgpu's Metal backend.
    #[default]
    Wgpu,
    /// Rune's native Metal backend, with Rune's `metal-backend` feature; without it, wgpu.
    Metal,
}

const UNSET: u8 = 0;
const WGPU: u8 = 1;
const METAL: u8 = 2;

/// What the app chose: nothing yet, `Wgpu` or `Metal`.
#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
static CHOSEN: AtomicU8 = AtomicU8::new(UNSET);

/// The backend an app that chooses none draws with: the native one on macOS with the feature.
const DEFAULT: GpuBackend = if cfg!(all(target_os = "macos", feature = "metal-backend")) { GpuBackend::Metal } else { GpuBackend::Wgpu };

/// Makes GPUs from now on with `backend`, process-wide. A `prewarm_gpu` running on another
/// backend starts over on this one.
#[cfg_attr(not(target_vendor = "apple"), allow(unused_variables))]
pub(crate) fn choose(backend: GpuBackend) {
    #[cfg(all(target_vendor = "apple", not(feature = "metal-backend")))]
    if backend == GpuBackend::Metal {
        eprintln!("rune: GpuBackend::Metal needs Rune's `metal-backend` feature; drawing with wgpu");
    }
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    {
        let before = current();
        CHOSEN.store(match backend { GpuBackend::Wgpu => WGPU, GpuBackend::Metal => METAL }, Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        if current() != before {
            crate::render::restart_prewarm();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = before;
    }
}

fn chosen() -> Option<GpuBackend> {
    match CHOSEN.load(Ordering::Relaxed) {
        WGPU => Some(GpuBackend::Wgpu),
        METAL => Some(GpuBackend::Metal),
        _ => None,
    }
}

fn env() -> Option<String> {
    std::env::var("RUNE_GPU_BACKEND").ok()
}

#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
/// The backend GPUs are made with: `RUNE_GPU_BACKEND` (`wgpu` or `metal`) over the app's
/// choice, over the default.
pub(crate) fn current() -> GpuBackend {
    resolve(env().as_deref(), chosen())
}

/// What `RUNE_GPU_LOG` calls the backend `adapter` is on.
pub(crate) fn label(adapter: &wgpu::Adapter) -> &'static str {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    if crate::gpu::metal::owns_adapter(adapter) {
        return "rune backend";
    }
    let _ = adapter;
    "wgpu"
}

/// Whether `device` is the native Metal backend's.
pub(crate) fn native(device: &wgpu::Device) -> bool {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    return crate::gpu::metal::raw_device(device).is_some();
    #[cfg(not(all(target_vendor = "apple", feature = "metal-backend")))]
    {
        let _ = device;
        false
    }
}

/// Features a device on `adapter` gets whether or not a compositor asks: all the native backend
/// offers, which cost it nothing, so a prewarmed device serves any compositor it can draw for.
pub(crate) fn free_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    if crate::gpu::metal::owns_adapter(adapter) {
        return crate::gpu::metal::FEATURES;
    }
    let _ = adapter;
    wgpu::Features::empty()
}

/// The features the last compositor that fell back to wgpu wanted and the native backend lacks.
#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
static MISSING: std::sync::Mutex<wgpu::Features> = std::sync::Mutex::new(wgpu::Features::empty());

/// Whether a GPU for a compositor that wants `wanted` draws with wgpu instead of the native
/// backend: only when the native backend is the default, neither the app nor the environment
/// asked for it, and it lacks one of `wanted`'s features. Says so once.
#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(unused_variables))]
pub(crate) fn wgpu_for(wanted: wgpu::Features) -> bool {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    {
        let missing = wanted - crate::gpu::metal::FEATURES;
        if !missing.is_empty() && DEFAULT == GpuBackend::Metal && defaulted(env().as_deref(), chosen()) {
            *MISSING.lock().unwrap_or_else(|e| e.into_inner()) = missing;
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| eprintln!("rune: the compositor wants {missing}, which Rune's Metal backend lacks; drawing with wgpu"));
            return true;
        }
    }
    false
}

/// Which backend draws on `device`, on which GPU, and why, for an app's log: "Rune's Metal
/// backend on Apple M4 (the default)", or "wgpu Metal on Apple M4 (a compositor wants
/// POLYGON_MODE_LINE, which Rune's Metal backend lacks)". A window's compositor gets its device in
/// `compose::Frame::device`. `RUNE_GPU_LOG=1` prints it for each window and overlay GPU Rune makes.
pub fn describe(device: &wgpu::Device) -> String {
    let info = device.adapter_info();
    let backend = if native(device) { "Rune's Metal backend".to_string() } else { format!("wgpu {:?}", info.backend) };
    let why = reason(native(device));
    if why.is_empty() { format!("{backend} on {}", info.name) } else { format!("{backend} on {} ({why})", info.name) }
}

/// Why the backend is the native one (`native`) or wgpu's; empty where wgpu is the only one.
fn reason(native: bool) -> String {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    {
        let (env, chosen) = (env(), chosen());
        let missing = *MISSING.lock().unwrap_or_else(|e| e.into_inner());
        return match (native, env.as_deref(), chosen) {
            (true, Some("metal"), _) => "RUNE_GPU_BACKEND=metal".into(),
            (true, _, Some(GpuBackend::Metal)) => "App::gpu_backend chose it".into(),
            (true, ..) => "the default".into(),
            (false, Some("wgpu"), _) => "RUNE_GPU_BACKEND=wgpu".into(),
            (false, _, Some(GpuBackend::Wgpu)) => "App::gpu_backend chose it".into(),
            (false, ..) if DEFAULT == GpuBackend::Wgpu => "the default here".into(),
            (false, ..) if !missing.is_empty() => format!("a compositor wants {missing}, which Rune's Metal backend lacks"),
            (false, ..) => String::new(),
        };
    }
    #[cfg(all(target_vendor = "apple", not(feature = "metal-backend")))]
    return if native { String::new() } else { "Rune's metal-backend feature is off".into() };
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = native;
        String::new()
    }
}

/// Whether a GPU on `device` serves a compositor that wants `wanted`: false for a native
/// backend device when `wgpu_for(wanted)`.
pub(crate) fn serves(device: &wgpu::Device, wanted: wgpu::Features) -> bool {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    if crate::gpu::metal::raw_device(device).is_some() {
        return !wgpu_for(wanted);
    }
    let _ = (device, wanted);
    true
}

#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
fn resolve(env: Option<&str>, chosen: Option<GpuBackend>) -> GpuBackend {
    match env {
        Some("metal") => GpuBackend::Metal,
        Some("wgpu") => GpuBackend::Wgpu,
        _ => chosen.unwrap_or(DEFAULT),
    }
}

/// Whether the backend is the default one, which neither the environment nor the app chose.
#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
fn defaulted(env: Option<&str>, chosen: Option<GpuBackend>) -> bool {
    !matches!(env, Some("metal" | "wgpu")) && chosen.is_none()
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT, GpuBackend, defaulted, resolve};

    #[test]
    fn the_environment_overrides_the_app() {
        assert_eq!(resolve(None, Some(GpuBackend::Metal)), GpuBackend::Metal);
        assert_eq!(resolve(Some("wgpu"), Some(GpuBackend::Metal)), GpuBackend::Wgpu);
        assert_eq!(resolve(Some("metal"), Some(GpuBackend::Wgpu)), GpuBackend::Metal);
        assert_eq!(resolve(Some(""), Some(GpuBackend::Metal)), GpuBackend::Metal);
    }

    #[test]
    fn an_app_that_chooses_wgpu_keeps_it_over_the_default() {
        assert_eq!(resolve(None, Some(GpuBackend::Wgpu)), GpuBackend::Wgpu);
        assert_eq!(resolve(None, None), DEFAULT);
        assert_eq!(resolve(Some("bogus"), None), DEFAULT);
        assert!(defaulted(None, None));
        assert!(!defaulted(None, Some(GpuBackend::Metal)));
        assert!(!defaulted(Some("metal"), None));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_draws_with_the_native_backend_by_default_with_the_feature() {
        let expected = if cfg!(feature = "metal-backend") { GpuBackend::Metal } else { GpuBackend::Wgpu };
        assert_eq!(DEFAULT, expected);
    }
}
