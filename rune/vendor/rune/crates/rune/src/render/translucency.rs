//! Picking a GPU that composites translucent pixels on a surface. Some surfaces are opaque only:
//! a Win32 Vulkan surface on AMD's integrated GPUs, and any DXGI swapchain made from an HWND. A
//! transparent window or overlay drawn there shows black where it should be clear. Rune makes
//! DX12 surfaces through DirectComposition (`Gpu::instance`), which composites alpha on every
//! GPU, and gives a translucent surface the lowest-power adapter that composites it.
//!
//! On Windows only that DirectComposition swapchain composites alpha. A Vulkan surface there is
//! a swapchain on the HWND, whatever alpha modes its driver reports: a transparent window,
//! which has no redirection bitmap, shows it black or not at all (Hex #499, #510, on hybrid
//! laptops whose integrated GPU's Vulkan driver claimed premultiplied alpha).

/// The alpha mode that composites translucent pixels on `surface` through `adapter`, if any.
pub(crate) fn mode(surface: &wgpu::Surface<'_>, adapter: &wgpu::Adapter) -> Option<wgpu::CompositeAlphaMode> {
    if cfg!(target_os = "windows") && adapter.get_info().backend != wgpu::Backend::Dx12 { return None; }
    let caps = surface.get_capabilities(adapter);
    [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied, wgpu::CompositeAlphaMode::Inherit]
        .into_iter()
        .find(|mode| caps.alpha_modes.contains(mode))
}

/// Whether `gpu` can draw `surface` with `wanted`'s features its adapter supports, and, for a
/// `translucent` surface, composite translucent pixels on it.
pub(crate) fn serves(gpu: &crate::render::Gpu, surface: &wgpu::Surface<'_>, wanted: wgpu::Features, translucent: bool) -> bool {
    gpu.adapter.is_surface_supported(surface)
        && gpu.device.features().contains(wanted & gpu.adapter.features())
        && crate::gpu::backend::serves(&gpu.device, wanted)
        && (!translucent || mode(surface, &gpu.adapter).is_some())
}

/// `pick`'s adapter when it composites translucent pixels on `surface`; otherwise the adapter
/// on `instance` that does on the compositor's GPU (`gpu_pick`), then the lowest-power one that
/// does, or `pick`'s adapter when none can. When the compositor named its GPU, adapters on
/// others are never candidates, however their drivers answer: on GNOME with an NVIDIA card, whose
/// XWayland surfaces are opaque only, RADV and lavapipe claim premultiplied alpha for an overlay's
/// XWayland window but can't make its swapchain (hex#979, #982). It draws opaque there instead.
/// A software adapter never replaces a hardware one: on X11 with NVIDIA, whose surfaces are
/// opaque only, lavapipe claims premultiplied alpha, but its swapchain sends DRI3 requests
/// NVIDIA's Xorg refuses, and the X error lands in whichever window is next (hex#1216). The
/// hardware adapter draws opaque there, and an X compositor still blends its ARGB window.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn prefer(instance: &wgpu::Instance, surface: &wgpu::Surface<'_>, pick: &crate::gpu::pick::Pick) -> wgpu::Adapter {
    if mode(surface, &pick.adapter).is_some() {
        return pick.adapter.clone();
    }
    let mut able: Vec<_> = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .into_iter()
        .filter(|candidate| {
            let info = candidate.get_info();
            pick.shows(&info) && takes_over(pick.adapter.get_info().device_type, info.device_type)
                && candidate.is_surface_supported(surface) && mode(surface, candidate).is_some()
        })
        .collect();
    able.sort_by_key(|candidate| {
        let info = candidate.get_info();
        (!pick.on_compositor_device(&info), crate::gpu::pick::power_rank(info.device_type))
    });
    able.into_iter().next().unwrap_or_else(|| pick.adapter.clone())
}

/// Whether an adapter of type `candidate` may draw a translucent surface in place of `pick`'s:
/// any but a software adapter, which may only stand in for another.
#[cfg(not(target_arch = "wasm32"))]
fn takes_over(pick: wgpu::DeviceType, candidate: wgpu::DeviceType) -> bool {
    candidate != wgpu::DeviceType::Cpu || pick == wgpu::DeviceType::Cpu
}

#[cfg(test)]
mod tests {
    /// lavapipe never takes a translucent overlay from a hardware adapter (hex#1216), but a
    /// hardware adapter may still take one from software, and GPUs from each other.
    #[test]
    fn software_never_takes_over_from_hardware() {
        use super::takes_over;
        use wgpu::DeviceType::*;
        for hardware in [DiscreteGpu, IntegratedGpu, VirtualGpu, Other] {
            assert!(!takes_over(hardware, Cpu), "{hardware:?}");
            assert!(takes_over(Cpu, hardware), "{hardware:?}");
        }
        assert!(takes_over(DiscreteGpu, IntegratedGpu));
        assert!(takes_over(Cpu, Cpu));
    }
}
