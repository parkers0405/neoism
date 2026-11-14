//! Embedded `CAMetalLayer` host for UIKit, SwiftUI and AppKit views that own their own window
//! and event loop (e.g. a `UIViewRepresentable` driven by `CADisplayLink` on iOS).

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::render::{Gpu, Renderer};
use crate::runtime::{Env, Runtime, Waker};
use crate::time::Instant;
use crate::tooling::{AppSpec, runtime_for};

static SHARED_GPU: OnceLock<Result<Arc<Gpu>, String>> = OnceLock::new();

fn shared_gpu() -> Result<Arc<Gpu>, String> {
    SHARED_GPU
        .get_or_init(|| Gpu::try_new(Gpu::instance(), None).map(Arc::new))
        .clone()
}

/// Warms the shared Metal device and compiles the default `Bgra8Unorm` uber pipeline on a
/// background thread so the first embedded `LayerHost` opens without shader-compile latency.
pub fn prewarm() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new().name("rune-embed-prewarm".into()).spawn(|| {
            if let Ok(gpu) = shared_gpu() {
                let _ = Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm);
            }
        });
    });
}

/// Outcome of one [`LayerHost::frame`] pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbedFrame {
    /// Whether a drawable was acquired and presented.
    pub drew: bool,
    /// True while springs, reveals, pending input or live bindings need another display-link tick.
    pub animating: bool,
    /// Seconds until the next scheduled timer wake (`None` when none is pending).
    pub wake_after_secs: Option<f64>,
    /// CPU time spent stepping layout/motion and encoding GPU work, in microseconds.
    pub cpu_us: u64,
}

/// Drives one Rune [`Runtime`] and [`Renderer`] directly onto a `CAMetalLayer`.
///
/// Pause display-link calls while the host app is backgrounded on iOS: Metal forbids drawable
/// acquisition and command-buffer submission after `didEnterBackground`.
pub struct LayerHost<S: 'static> {
    gpu: Arc<Gpu>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    compositor: Option<Box<dyn Compositor<S>>>,
    rt: Runtime<S>,
    clock: FrameClock,
    suspended: bool,
}

impl<S: 'static> LayerHost<S> {
    /// Attaches a new Rune runtime and Metal surface to `layer` (`CAMetalLayer *`), blending in
    /// sRGB space (`srgb_blend = true`, matching Apple's native text compositing).
    ///
    /// # Safety
    /// `layer` must be a valid `CAMetalLayer` pointer that outlives this `LayerHost`, and surface
    /// creation / presentation must run on a thread allowed to touch that layer (the UI thread).
    pub unsafe fn from_core_animation_layer(
        layer: *mut c_void,
        spec: &AppSpec<S>,
        state: S,
        viewport: (f32, f32),
        scale: f32,
        dark: bool,
        wake: Waker,
    ) -> Result<Self, String> {
        if layer.is_null() {
            return Err("null CAMetalLayer pointer".into());
        }
        let gpu = shared_gpu()?;
        let surface = unsafe {
            gpu.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
                .map_err(|e| format!("create_surface_unsafe: {e}"))?
        };
        let scale = scale.max(1.0);
        let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1.max(1.0) * scale).round() as u32).clamp(1, max_dim);
        let srgb_blend = true;
        let (config, _) = crate::window::surface_config(&surface, &gpu.adapter, (px_w, px_h), srgb_blend)
            .ok_or_else(|| "CAMetalLayer has no supported wgpu surface configuration".to_owned())?;
        surface.configure(&gpu.device, &config);
        let target_format = config.format.remove_srgb_suffix();
        let mut renderer = Renderer::new(&gpu.device, target_format);
        renderer.set_srgb_blend(true);

        let w = px_w as f32 / scale;
        let h = px_h as f32 / scale;
        let mut rt = runtime_for(spec, state, (w, h), Env::new(dark, scale), wake);
        rt.set_srgb_blend(true);
        rt.set_os_drag(false);
        let now = Instant::now();
        rt.wall_start = Some(now);
        rt.record_events = false;

        Ok(Self {
            gpu,
            surface,
            config,
            renderer,
            compositor: None,
            rt,
            clock: FrameClock::new(now),
            suspended: viewport.0 <= 0.0 || viewport.1 <= 0.0,
        })
    }

    /// Attaches a custom [`Compositor`] (e.g. for [`El::effect`](crate::El::effect) shader passes)
    /// and turns on `Runtime::track_marks` so `rt.marks` aligns with `rt.instances`.
    pub fn with_compositor(mut self, compositor: impl Compositor<S>) -> Self {
        self.rt.track_marks = true;
        self.compositor = Some(Box::new(compositor));
        self
    }

    pub fn runtime(&self) -> &Runtime<S> {
        &self.rt
    }

    pub fn runtime_mut(&mut self) -> &mut Runtime<S> {
        &mut self.rt
    }

    /// Updates the viewport in points and display scale, reconfiguring the Metal swapchain only
    /// when its clamped device-pixel dimensions change.
    pub fn resize(&mut self, viewport: (f32, f32), scale: f32) {
        if viewport.0 <= 0.0 || viewport.1 <= 0.0 {
            self.suspended = true;
            return;
        }
        self.suspended = false;
        let scale = scale.max(1.0);
        let max_dim = self.gpu.device.limits().max_texture_dimension_2d.max(1);
        let px_w = ((viewport.0 * scale).round() as u32).clamp(1, max_dim);
        let px_h = ((viewport.1 * scale).round() as u32).clamp(1, max_dim);
        if self.config.width != px_w || self.config.height != px_h {
            self.config.width = px_w;
            self.config.height = px_h;
            self.surface.configure(&self.gpu.device, &self.config);
        }
        self.rt.set_viewport((px_w as f32 / scale, px_h as f32 / scale), scale);
    }

    /// Steps animations and layout to `Instant::now()` and presents one Metal frame.
    pub fn frame(&mut self) -> EmbedFrame {
        if self.suspended {
            return EmbedFrame::default();
        }
        let t0 = Instant::now();
        let (animating, dt) = self.clock.advance(&mut self.rt, t0, false);
        let update_ms = t0.elapsed().as_secs_f64() * 1000.0;
        crate::clipboard::flush_memory(&mut self.rt);
        if self.rt.popup_menu().is_some() {
            self.rt.cancel_popups();
        }
        self.rt.refuse_file_dialog("native file dialogs are unsupported in embedded views");

        let comp_animating = self.compositor.as_ref().is_some_and(|c| c.animating());
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                return EmbedFrame { animating: true, ..EmbedFrame::default() };
            }
            _ => return EmbedFrame { animating: animating || comp_animating, ..EmbedFrame::default() },
        };

        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.renderer.format()),
            ..Default::default()
        });
        self.renderer.upload_images(&self.gpu.device, &self.gpu.queue, &mut self.rt.images);
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let size = (self.config.width, self.config.height);
        let format = self.renderer.format();
        if let Some(comp) = &mut self.compositor {
            let mut f = Frame {
                device: &self.gpu.device,
                queue: &self.gpu.queue,
                encoder: &mut encoder,
                target: &target,
                format,
                size,
                dt,
                update_ms,
                rt: &mut self.rt,
                ui: &mut self.renderer,
            };
            comp.render(&mut f);
        } else {
            self.renderer.prepare(
                &self.gpu.device,
                &self.gpu.queue,
                &mut self.rt.text.atlas,
                &self.rt.instances,
                size,
            );
            self.renderer.draw(&mut encoder, &target, self.rt.clear_color());
        }
        self.renderer.submit(&self.gpu.queue, encoder);
        self.gpu.queue.present(frame);

        let cpu_us = t0.elapsed().as_micros() as u64;
        let wake_after_secs = self.rt.wake_deadline().and_then(|d| {
            let elapsed = self.rt.wall_start?.elapsed().as_secs_f64();
            Some((d - elapsed).max(0.0))
        });
        let comp_animating = self.compositor.as_ref().is_some_and(|c| c.animating());
        let keep_ticking = animating || comp_animating || self.rt.has_pending_input() || self.rt.live_pending();
        EmbedFrame {
            drew: true,
            animating: keep_ticking,
            wake_after_secs,
            cpu_us,
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use objc2_video::{
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject},
    };

    use super::LayerHost;
    use crate::compose::{Compositor, Frame};
    use crate::render::KIND_EFFECT;
    use crate::tooling::AppSpec;
    use crate::{Cx, El, col, effect};

    struct MarkProbe(Arc<AtomicU32>);

    impl Compositor<()> for MarkProbe {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            for (idx, inst) in f.rt.instances.iter().enumerate() {
                if inst.params[1] == KIND_EFFECT {
                    self.0.store(f.rt.marks.get(idx).copied().unwrap_or(0), Ordering::Relaxed);
                }
            }
            f.draw_ui();
        }
    }

    #[test]
    fn layer_host_runs_compositor_with_tracked_marks() {
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        let seen = Arc::new(AtomicU32::new(0));
        let view = |_: &(), _: &mut Cx<()>| -> El { col().size(120.0, 80.0).child(effect(42).size(60.0, 20.0)) };
        let spec = AppSpec::new("embed-test", (120.0, 80.0), || (), view);
        let host = unsafe {
            LayerHost::from_core_animation_layer(
                Retained::as_ptr(&layer) as *mut std::ffi::c_void,
                &spec,
                (),
                (120.0, 80.0),
                2.0,
                true,
                Arc::new(|| {}),
            )
        };
        let Ok(mut host) = host else { return };
        host = host.with_compositor(MarkProbe(seen.clone()));
        let out = host.frame();
        assert!(out.drew);
        assert_eq!(seen.load(Ordering::Relaxed), 42);
    }
}
