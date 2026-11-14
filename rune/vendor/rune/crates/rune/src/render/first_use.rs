//! What Rune's pipelines other than the uber shader cost the first time a process makes them,
//! for rune-light's `shader_first_use` example and `builtin_shaders` test, which between them
//! build every pipeline Rune and its effect crates ship.

use std::time::Instant;

use super::{Renderer, backdrop_blur::BackdropBlur, layer_blur, surface_alpha::AlphaSurface};

impl Renderer {
    /// Makes, times and drops what a first backdrop blur, layer blur, video frame, transparent
    /// window and spring-driven renderer build, in milliseconds each.
    #[doc(hidden)]
    pub fn first_use_ms(&self, device: &wgpu::Device) -> Vec<(&'static str, f64)> {
        fn ms(f: impl FnOnce()) -> f64 {
            let start = Instant::now();
            f();
            start.elapsed().as_secs_f64() * 1e3
        }
        use crate::gpu::springs::{Node, Track};
        let node = Node { x: Track::constant(0.0), y: Track::constant(0.0), opacity: Track::constant(1.0) };
        vec![
            ("backdrop_blur", ms(|| BackdropBlur::new(device, &self.bind_layout).ensure_composite(device, self.format))),
            ("layer_blur", ms(|| drop(layer_blur::Pipeline::new(device)))),
            ("video", ms(|| drop(crate::native_surface::SurfaceRenderer::new(device, self.format)))),
            ("alpha", ms(|| drop(AlphaSurface::new(device, self.format, (16, 16))))),
            ("springs", ms(|| drop(Renderer::with_springs(device, self.format, &[node], &[0])))),
        ]
    }
}

/// How many pipelines Rune's Metal backend compiled at runtime because its precompiled library
/// lacked them; `None` on any other backend.
#[doc(hidden)]
pub fn precompiled_misses(device: &wgpu::Device) -> Option<usize> {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    if crate::gpu::metal::raw_device(device).is_some() {
        return Some(crate::gpu::metal::builtin_misses());
    }
    let _ = device;
    None
}
