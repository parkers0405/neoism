#[cfg(not(target_arch = "wasm32"))]
pub mod cpu;
#[cfg(target_os = "macos")]
pub mod metal;
#[cfg(target_os = "linux")]
pub mod vulkan;
#[cfg(feature = "wgpu")]
pub mod webgpu;

use crate::sugarloaf::{SugarloafBackend, SugarloafWindow};
use crate::{SugarloafRenderer, SugarloafWindowSize};

pub struct Context<'a> {
    pub inner: ContextType<'a>,
}

#[allow(clippy::large_enum_variant)]
pub enum ContextType<'a> {
    #[cfg(feature = "wgpu")]
    Wgpu(webgpu::WgpuContext<'a>),
    #[cfg(target_os = "macos")]
    Metal(metal::MetalContext),
    #[cfg(target_os = "linux")]
    Vulkan(vulkan::VulkanContext),
    #[cfg(not(target_arch = "wasm32"))]
    Cpu(cpu::CpuContext),
    /// Lifetime placeholder for the Wgpu variant when it's
    /// feature-gated out — keeps `'a` referenced across the enum so
    /// the compiler doesn't complain about an unused parameter on
    /// builds without wgpu.
    #[cfg(not(feature = "wgpu"))]
    #[doc(hidden)]
    _Phantom(std::marker::PhantomData<&'a ()>),
}

impl Context<'_> {
    /// Synchronous constructor (native targets). Forwards into each
    /// backend's blocking `new`. Not available on wasm32 — see
    /// `Context::from_canvas` for the async browser entrypoint.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new<'a>(
        sugarloaf_window: SugarloafWindow,
        renderer_config: SugarloafRenderer,
    ) -> Context<'a> {
        let backend_label = match &renderer_config.backend {
            #[cfg(feature = "wgpu")]
            SugarloafBackend::Wgpu(backends) => format!("wgpu({backends:?})"),
            #[cfg(target_os = "macos")]
            SugarloafBackend::Metal => "metal".to_string(),
            #[cfg(target_os = "linux")]
            SugarloafBackend::Vulkan => "vulkan".to_string(),
            #[cfg(not(target_arch = "wasm32"))]
            SugarloafBackend::Cpu => "cpu".to_string(),
        };
        tracing::info!(
            target: "sugarloaf::context",
            backend = %backend_label,
            raw_display_handle = ?sugarloaf_window.display,
            raw_window_handle = ?sugarloaf_window.handle,
            "creating sugarloaf context"
        );
        let inner = match renderer_config.backend {
            #[cfg(feature = "wgpu")]
            SugarloafBackend::Wgpu(backends) => ContextType::Wgpu(
                webgpu::WgpuContext::new(sugarloaf_window, renderer_config, backends),
            ),
            #[cfg(target_os = "macos")]
            SugarloafBackend::Metal => {
                ContextType::Metal(metal::MetalContext::new(sugarloaf_window))
            }
            #[cfg(target_os = "linux")]
            SugarloafBackend::Vulkan => {
                ContextType::Vulkan(vulkan::VulkanContext::new(sugarloaf_window))
            }
            #[cfg(not(target_arch = "wasm32"))]
            SugarloafBackend::Cpu => {
                ContextType::Cpu(cpu::CpuContext::new(sugarloaf_window))
            }
        };

        Context { inner }
    }

    /// Async constructor for browser targets. Wraps an `HtmlCanvasElement`
    /// in `wgpu::SurfaceTarget::Canvas` and awaits the adapter / device
    /// requests inside `WgpuContext::new_async`. Always produces a
    /// `ContextType::Wgpu` — there is no Metal/Vulkan/CPU path on wasm32.
    #[cfg(all(target_arch = "wasm32", feature = "wgpu"))]
    pub async fn from_canvas<'a>(
        canvas: web_sys::HtmlCanvasElement,
        size: SugarloafWindowSize,
        scale: f32,
        renderer_config: SugarloafRenderer,
    ) -> Result<Context<'a>, String> {
        let backends = match renderer_config.backend {
            SugarloafBackend::Wgpu(b) => b,
        };
        let target = wgpu::SurfaceTarget::Canvas(canvas);
        let ctx = webgpu::WgpuContext::new_async(
            target,
            size,
            scale,
            renderer_config,
            backends,
        )
        .await?;
        Ok(Context {
            inner: ContextType::Wgpu(ctx),
        })
    }

    #[inline]
    pub fn scale(&self) -> f32 {
        match &self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.scale,
            #[cfg(target_os = "macos")]
            ContextType::Metal(ctx) => ctx.scale,
            #[cfg(target_os = "linux")]
            ContextType::Vulkan(ctx) => ctx.scale,
            #[cfg(not(target_arch = "wasm32"))]
            ContextType::Cpu(ctx) => ctx.scale,
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    #[inline]
    pub fn set_scale(&mut self, scale: f32) {
        match &mut self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => {
                ctx.set_scale(scale);
            }
            #[cfg(target_os = "macos")]
            ContextType::Metal(ctx) => {
                ctx.set_scale(scale);
            }
            #[cfg(target_os = "linux")]
            ContextType::Vulkan(ctx) => {
                ctx.set_scale(scale);
            }
            #[cfg(not(target_arch = "wasm32"))]
            ContextType::Cpu(ctx) => {
                ctx.set_scale(scale);
            }
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    #[inline]
    pub fn size(&self) -> SugarloafWindowSize {
        match &self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.size,
            #[cfg(target_os = "macos")]
            ContextType::Metal(ctx) => ctx.size,
            #[cfg(target_os = "linux")]
            ContextType::Vulkan(ctx) => ctx.size,
            #[cfg(not(target_arch = "wasm32"))]
            ContextType::Cpu(ctx) => ctx.size,
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    pub fn set_frame_wait_timeout(&mut self, interval_ns: u64) {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            ContextType::Vulkan(ctx) => ctx.set_frame_wait_timeout(interval_ns),
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(_) => {}
            #[cfg(target_os = "macos")]
            ContextType::Metal(_) => {}
            #[cfg(not(target_arch = "wasm32"))]
            ContextType::Cpu(_) => {}
            #[cfg(not(any(target_os = "linux", feature = "wgpu")))]
            _ => {}
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.suspend_surface();
            return;
        }

        match &mut self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.resize(width, height),
            #[cfg(target_os = "macos")]
            ContextType::Metal(ctx) => ctx.resize(width, height),
            #[cfg(target_os = "linux")]
            ContextType::Vulkan(ctx) => ctx.resize(width, height),
            #[cfg(not(target_arch = "wasm32"))]
            ContextType::Cpu(ctx) => ctx.resize(width, height),
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    pub fn suspend_surface(&mut self) {
        match &mut self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.suspend_surface(),
            #[cfg(target_os = "macos")]
            ContextType::Metal(_) => {}
            #[cfg(target_os = "linux")]
            ContextType::Vulkan(_) => {}
            #[cfg(not(target_arch = "wasm32"))]
            ContextType::Cpu(_) => {}
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }

    #[inline]
    pub fn supports_f16(&self) -> bool {
        match &self.inner {
            #[cfg(feature = "wgpu")]
            ContextType::Wgpu(ctx) => ctx.supports_f16(),
            #[cfg(target_os = "macos")]
            ContextType::Metal(ctx) => ctx.supports_f16(),
            #[cfg(target_os = "linux")]
            ContextType::Vulkan(ctx) => ctx.supports_f16(),
            #[cfg(not(target_arch = "wasm32"))]
            ContextType::Cpu(ctx) => ctx.supports_f16(),
            #[cfg(not(feature = "wgpu"))]
            ContextType::_Phantom(_) => unreachable!(),
        }
    }
}
