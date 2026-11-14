//! The GPU under a surface. On macOS and iOS, the Metal backend: wgpu's own, or with `metal-backend` (on by
//! default) Rune's native one (`metal`). A Mac app draws with the native one unless `GpuBackend::Wgpu` or
//! `RUNE_GPU_BACKEND=wgpu` selects wgpu's; an iOS app draws with wgpu's unless it chooses `GpuBackend::Metal`.

pub(crate) mod backend;
#[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
pub(crate) mod metal;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod pick;
pub mod springs;

#[cfg(target_os = "macos")]
use objc2_video::runtime::AnyObject;

/// Runs `f` on the CAMetalLayer `surface` presents into, under wgpu-hal's lock on wgpu's Metal
/// backend. `None` when the surface is on neither backend.
#[cfg(target_os = "macos")]
pub(crate) fn with_layer<R>(surface: &wgpu::Surface<'_>, f: impl FnOnce(&AnyObject) -> R) -> Option<R> {
    // SAFETY: the layer is only borrowed, under wgpu's lock.
    if let Some(hal) = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() } {
        let layer = hal.render_layer().lock();
        // SAFETY: a live CAMetalLayer, which the lock keeps; wgpu's objc2 may be another version
        // than ours, the object is the same.
        return Some(f(unsafe { &*objc2_video::rc::Retained::as_ptr(&*layer).cast::<AnyObject>() }));
    }
    #[cfg(feature = "metal-backend")]
    if let Some(layer) = metal::layer(surface) {
        return Some(f(layer));
    }
    None
}

/// Whether `queue` is Rune's native Metal backend's, where a frame's submit and present commit
/// one command buffer, too cheap to hand to a present thread.
#[cfg(target_os = "macos")]
pub(crate) fn native(queue: &wgpu::Queue) -> bool {
    #[cfg(feature = "metal-backend")]
    return metal::owns(queue);
    #[cfg(not(feature = "metal-backend"))]
    {
        let _ = queue;
        false
    }
}
