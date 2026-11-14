//! The CAMetalLayer a surface presents into, and its drawables. The layer comes from
//! raw-window-metal as wgpu-hal's does: a sublayer of the view's layer that follows the view's
//! bounds and the screen's scale as they change.

use std::sync::Arc;

use objc2_core_foundation::CGSize;
use objc2_quartz_core::{CALayer, CAMetalDrawable, CAMetalLayer};
use objc2_video::rc::{Retained, autoreleasepool};
use parking_lot::Mutex;
use raw_window_metal::Layer;
use wgpu::custom::*;

use super::resource::{MetalTexture, TextureRaw, pixel_format};
use super::{MetalDevice, Obj, Shared, send_sync};

#[derive(Debug)]
pub(crate) struct MetalSurface {
    layer: Retained<CAMetalLayer>,
    view: usize,
    configured: Mutex<Option<(Arc<Shared>, wgpu::TextureFormat)>>,
}

#[derive(Debug)]
pub(crate) struct MetalOutput {
    pub(super) view: usize,
    pub(super) scale: f64,
    pub(super) drawable: Option<Obj<dyn CAMetalDrawable>>,
    /// The layer's `presentsWithTransaction` when the drawable was acquired.
    pub(super) transaction: bool,
    shared: Option<Arc<Shared>>,
}

send_sync!(MetalSurface, MetalOutput);

/// A surface on a CAMetalLayer as given, or on one filling an AppKit or UIKit view (on the main thread).
pub(super) unsafe fn create(target: wgpu::SurfaceTargetUnsafe) -> DispatchSurface {
    let view = match &target {
        wgpu::SurfaceTargetUnsafe::RawHandle { raw_window_handle: wgpu::rwh::RawWindowHandle::AppKit(handle), .. } => handle.ns_view.as_ptr() as usize,
        _ => 0,
    };
    // SAFETY: the caller hands a live layer or view.
    let layer = unsafe {
        match target {
            wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer) => Layer::from_ca_layer(std::ptr::NonNull::new(layer).expect("a CALayer")),
            wgpu::SurfaceTargetUnsafe::RawHandle { raw_window_handle: wgpu::rwh::RawWindowHandle::AppKit(handle), .. } => Layer::from_ns_view(handle.ns_view),
            wgpu::SurfaceTargetUnsafe::RawHandle { raw_window_handle: wgpu::rwh::RawWindowHandle::UiKit(handle), .. } => Layer::from_ui_view(handle.ui_view),
            _ => panic!("RUNE_GPU_BACKEND=metal needs an AppKit or UIKit view or a CALayer"),
        }
    };
    // SAFETY: `into_raw` hands over its retain of a CAMetalLayer.
    let layer = unsafe { Retained::from_raw(layer.into_raw().as_ptr().cast::<CAMetalLayer>()) }.expect("a CAMetalLayer");
    DispatchSurface::custom(MetalSurface { layer, view, configured: Mutex::new(None) })
}

impl Drop for MetalSurface {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        crate::native_frame::remove(self.view);
    }
}

impl MetalSurface {
    pub(super) fn layer(&self) -> &CAMetalLayer { &self.layer }
}

/// The `NSWindow` hosting `layer`: the first ancestor layer with a delegate is the backing
/// `NSView`, and its window is the one.
#[cfg(target_os = "macos")]
fn hosting_window(layer: &CALayer) -> Option<Retained<objc2_video::runtime::NSObject>> {
    use objc2_video::{Message, msg_send};
    let mut current = Some(layer.retain());
    while let Some(layer) = current {
        if let Some(delegate) = layer.delegate() {
            // SAFETY: a layer's delegate with a `window` is its NSView.
            return unsafe { msg_send![&*delegate, window] };
        }
        current = layer.superlayer();
    }
    None
}

/// Whether the window hosting `layer` is fully covered or hidden, where `nextDrawable` would
/// wait out its timeout for a vsync that never comes. On iOS the host stops its display link
/// while the app is in the background, so there is nothing to check.
#[cfg(target_os = "macos")]
fn occluded(layer: &CALayer) -> bool {
    const VISIBLE: usize = 1 << 1;
    let Some(window) = hosting_window(layer) else { return false };
    // SAFETY: `occlusionState` is an NSWindow property returning NSWindowOcclusionState.
    let state: usize = unsafe { objc2_video::msg_send![&*window, occlusionState] };
    state & VISIBLE == 0
}

#[cfg(not(target_os = "macos"))]
fn occluded(_: &CALayer) -> bool { false }

impl SurfaceInterface for MetalSurface {
    fn get_capabilities(&self, _: &DispatchAdapter) -> wgpu::SurfaceCapabilities {
        use wgpu::TextureFormat as F;
        let formats = vec![F::Bgra8Unorm, F::Bgra8UnormSrgb, F::Rgba16Float, F::Rgb10a2Unorm];
        wgpu::SurfaceCapabilities {
            format_capabilities: formats.iter().map(|&format| wgpu::SurfaceFormatCapabilities { format, color_spaces: wgpu::SurfaceColorSpaces::SRGB }).collect(),
            formats,
            present_modes: vec![wgpu::PresentMode::Fifo, wgpu::PresentMode::Immediate],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque, wgpu::CompositeAlphaMode::PostMultiplied],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        }
    }

    fn configure(&self, device: &DispatchDevice, config: &wgpu::SurfaceConfiguration) {
        let device = device.as_custom::<MetalDevice>().expect("a Metal device");
        assert!(matches!(config.color_space, wgpu::SurfaceColorSpace::Auto | wgpu::SurfaceColorSpace::Srgb),
            "RUNE_GPU_BACKEND=metal: surface color space {:?}", config.color_space);
        *self.configured.lock() = Some((device.shared.clone(), config.format));
        let layer = &self.layer;
        layer.setDevice(Some(&device.shared.device));
        layer.setPixelFormat(pixel_format(config.format));
        layer.setFramebufferOnly(config.usage == wgpu::TextureUsages::RENDER_ATTACHMENT);
        match config.alpha_mode {
            wgpu::CompositeAlphaMode::Opaque => layer.setOpaque(true),
            wgpu::CompositeAlphaMode::PostMultiplied => layer.setOpaque(false),
            _ => {}
        }
        layer.setColorspace(None);
        layer.setMaximumDrawableCount(config.desired_maximum_frame_latency.clamp(1, 2) as usize + 1);
        layer.setDrawableSize(CGSize::new(f64::from(config.width), f64::from(config.height)));
        // iOS layers are always display-synced, and keep the drawable timeout as wgpu-hal does: with
        // no occlusion check, a host that misses a background transition gets a skipped frame
        // rather than a main thread waiting forever.
        #[cfg(target_os = "macos")]
        {
            layer.setAllowsNextDrawableTimeout(false);
            layer.setDisplaySyncEnabled(!matches!(config.present_mode, wgpu::PresentMode::Immediate | wgpu::PresentMode::AutoNoVsync));
        }
    }

    fn get_current_texture(&self) -> (Option<DispatchTexture>, wgpu::SurfaceStatus, DispatchSurfaceOutputDetail) {
        let (shared, format) = match self.configured.lock().clone() {
            Some((shared, format)) => (Some(shared), format),
            None => (None, wgpu::TextureFormat::Bgra8Unorm),
        };
        let none = |status, shared| (None, status, DispatchSurfaceOutputDetail::custom(MetalOutput { view: self.view, scale: 1.0, drawable: None, transaction: false, shared }));
        if occluded(&self.layer) { return none(wgpu::SurfaceStatus::Occluded, shared) }
        if let Some(shared) = &shared { shared.pace(); }
        #[cfg(target_os = "macos")]
        if crate::native_frame::get(self.view).is_some() { self.layer.setFramebufferOnly(false); }
        let Some(drawable) = autoreleasepool(|_| self.layer.nextDrawable()) else { return none(wgpu::SurfaceStatus::Timeout, shared) };
        let texture = MetalTexture { raw: Arc::new(TextureRaw::drawable(drawable.texture(), format)) };
        let scale = self.layer.drawableSize().width / self.layer.bounds().size.width.max(1.0);
        let output = MetalOutput { view: self.view, scale, drawable: Some(drawable), transaction: self.layer.presentsWithTransaction(), shared };
        (Some(DispatchTexture::custom(texture)), wgpu::SurfaceStatus::Good, DispatchSurfaceOutputDetail::custom(output))
    }
}

impl SurfaceOutputDetailInterface for MetalOutput {
    fn texture_discard(&self) {
        if let Some(shared) = &self.shared { shared.flush(); }
    }
    fn texture_release(&self) {}
}
