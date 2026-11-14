//! `prewarm_gpu`'s thread and the slot a window takes its result from.

use super::{Gpu, Renderer};
use std::sync::Mutex;
use std::thread::JoinHandle;

/// A Mac window's target format (`surface_config`), which blends in sRGB space unless the
/// app turns that off, so the window renders with the pipeline compiled here.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

struct Prewarmed(Gpu, Renderer);
// SAFETY: wgpu's objects are `Send`. The one part of a `Renderer` that isn't, the native
// surface renderer with its CoreVideo texture cache, is made on first use: a new renderer
// has none (asserted where it's made).
unsafe impl Send for Prewarmed {}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Running(JoinHandle<Option<Prewarmed>>),
    Done,
}

pub(super) struct Slot(Mutex<State>);

impl Slot {
    pub(super) const fn new() -> Self {
        Self(Mutex::new(State::Idle))
    }

    pub(super) fn start(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, State::Idle) { return; }
        let thread = std::thread::Builder::new().name("rune-gpu-prewarm".into()).spawn(|| {
            objc2::rc::autoreleasepool(|_| {
                super::gpu_events::starting();
                let gpu = Gpu::try_new(Gpu::instance(), None).ok()?;
                #[cfg(test)]
                let loads_before = super::metallib::thread_loads();
                let renderer = Renderer::new(&gpu.device, FORMAT);
                assert!(renderer.surfaces.is_none(), "a new renderer has no native surface renderer");
                #[cfg(test)]
                if super::metallib::metallib_enabled() {
                    assert_eq!(super::metallib::thread_loads(), loads_before + 1, "prewarm_gpu loads the precompiled metallib");
                }
                Some(Prewarmed(gpu, renderer))
            })
        });
        *state = match thread {
            Ok(thread) => State::Running(thread),
            Err(_) => State::Done,
        };
    }

    pub(super) fn take(&self) -> Option<(Gpu, Renderer)> {
        let state = std::mem::replace(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()), State::Done);
        let State::Running(thread) = state else { return None };
        thread.join().ok().flatten().map(|Prewarmed(gpu, renderer)| (gpu, renderer))
    }

    /// Starts a new thread if one is working; that one drops its result when it finishes.
    #[cfg_attr(not(feature = "metal-backend"), allow(dead_code))]
    pub(super) fn restart(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, State::Running(_)) { return; }
        *state = State::Idle;
        drop(state);
        self.start();
    }

    /// The thread, if still working, drops its result when it finishes.
    pub(super) fn discard(&self) {
        let state = std::mem::replace(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()), State::Done);
        drop(state);
    }
}

static GLOBAL: Slot = Slot::new();

pub(super) fn start() { GLOBAL.start(); }
pub(super) fn take() -> Option<(Gpu, Renderer)> { GLOBAL.take() }
#[cfg(feature = "metal-backend")]
pub(super) fn restart() { GLOBAL.restart(); }
#[cfg_attr(test, allow(dead_code))]
pub(super) fn discard() { GLOBAL.discard(); }
