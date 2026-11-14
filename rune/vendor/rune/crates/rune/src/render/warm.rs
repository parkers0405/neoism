//! `warm_gpu` away from macOS: a GPU device and Rune's renderer made on a thread ahead of a
//! surface that may show soon, and the slot the next surface takes them from (`Gpu::for_surface`).
//! Unclaimed, they go after `LINGER`, so an app that warms on a hint and then shows nothing
//! holds no device for long. A hidden overlay that gives its surface back parks its device and
//! renderer here too (`park`), for `PARKED` rather than `LINGER`: an overlay shown for each
//! dictation takes the same device every time instead of making one per show (hex#1268).

use super::{Gpu, Renderer};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// How long a warmed device waits for a surface, from the last `warm_gpu`.
pub(super) const LINGER: Duration = Duration::from_secs(30);

/// How long a parked device waits for the next surface.
pub(crate) const PARKED: Duration = Duration::from_secs(5 * 60);

/// The format overlays and windows draw in on Windows and Linux (`surface_config`, after
/// `srgb_blend` drops the sRGB suffix); a surface in another one rebuilds the pipeline.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

struct Warmed(Gpu, Renderer);
// SAFETY: wgpu's objects are `Send`. The one part of a `Renderer` that isn't, the native
// surface renderer, is made on first use: a new renderer has none (asserted where it's made).
unsafe impl Send for Warmed {}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Making { until: Instant },
    Ready { warmed: Warmed, until: Instant },
}

struct Slot {
    state: Mutex<State>,
    changed: Condvar,
    /// The device a window's or overlay's surface draws with, while one does.
    live: Mutex<Weak<Gpu>>,
}

static SLOT: Slot = Slot { state: Mutex::new(State::Idle), changed: Condvar::new(), live: Mutex::new(Weak::new()) };

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Some surface draws with `gpu`: a warm-up while it lives would make a second device.
pub(crate) fn surface_gpu(gpu: &Arc<Gpu>) {
    *lock(&SLOT.live) = Arc::downgrade(gpu);
}

/// The device a surface draws with now, if any.
pub(super) fn live() -> Option<Arc<Gpu>> {
    lock(&SLOT.live).upgrade()
}

/// Starts making a device with `features` (whichever the adapter has) and the renderer,
/// unless a surface's device lives or one is being made or waits; either way the warmed one
/// waits `LINGER` from now. `prepare` runs on the warming thread with the new device, or the
/// live one, so an app can make its own pipelines off the UI thread.
pub(super) fn warm(features: wgpu::Features, prepare: Option<Box<dyn FnOnce(&wgpu::Device) + Send>>) {
    if let Some(gpu) = live() {
        if let Some(prepare) = prepare {
            let _ = std::thread::Builder::new().name("rune-gpu-warm".into()).spawn(move || prepare(&gpu.device));
        }
        return;
    }
    let mut state = lock(&SLOT.state);
    let until = Instant::now() + LINGER;
    match &mut *state {
        State::Making { until: at } | State::Ready { until: at, .. } => { *at = (*at).max(until); return; }
        State::Idle => *state = State::Making { until },
    }
    drop(state);
    let spawned = std::thread::Builder::new().name("rune-gpu-warm".into()).spawn(move || {
        let made = make(features);
        // Before the slot says ready, so a surface waiting for it gets the app's pipelines too;
        // outside the lock, so `warm` never waits on them.
        if let (Some(warmed), Some(prepare)) = (&made, prepare) { prepare(&warmed.0.device); }
        let mut state = lock(&SLOT.state);
        let (Some(warmed), State::Making { until }) = (made, &*state) else {
            *state = State::Idle;
            SLOT.changed.notify_all();
            return;
        };
        let until = *until;
        *state = State::Ready { warmed, until };
        SLOT.changed.notify_all();
        wait_for_surface(state);
    });
    if spawned.is_err() { *lock(&SLOT.state) = State::Idle; }
}

/// Keeps what's ready for a surface to take, or lets it go once nothing asked for it by its
/// time (which `warm` may push back).
fn wait_for_surface(mut state: std::sync::MutexGuard<'_, State>) {
    loop {
        let State::Ready { until, .. } = &*state else { return };
        let now = Instant::now();
        if now >= *until { break; }
        let wait = *until - now;
        state = SLOT.changed.wait_timeout(state, wait).unwrap_or_else(|e| e.into_inner()).0;
    }
    let expired = std::mem::take(&mut *state);
    drop(state);
    drop(expired);
}

/// A hidden overlay's device and renderer, its surface already dropped, for the next surface to
/// take within `linger`. The device first finishes its work, so what the dropped surface and
/// its swapchain left is freed now rather than whenever the device next runs. When something
/// already waits in the slot, this one goes.
pub(crate) fn park(gpu: Gpu, renderer: Renderer, linger: Duration) {
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    if renderer.surfaces.is_some() { return; }
    let mut state = lock(&SLOT.state);
    if !matches!(*state, State::Idle) { return; }
    *state = State::Ready { warmed: Warmed(gpu, renderer), until: Instant::now() + linger };
    SLOT.changed.notify_all();
    drop(state);
    let spawned = std::thread::Builder::new().name("rune-gpu-parked".into()).spawn(|| wait_for_surface(lock(&SLOT.state)));
    if spawned.is_err() { discard(); }
}

/// A device for a future surface on a GPU that drives a display, with the renderer, and one
/// pass run offscreen, so the device has drawn (`GpuEvent::Drew`).
fn make(features: wgpu::Features) -> Option<Warmed> {
    super::gpu_events::starting();
    let gpu = Gpu::try_for_display(Gpu::instance_for(features), features).ok()?;
    let renderer = Renderer::new(&gpu.device, FORMAT);
    assert!(renderer.surfaces.is_none(), "a new renderer has no native surface renderer");
    let target = super::Offscreen::with_format(&gpu.device, 8, 8, FORMAT);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("rune warm-up"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &target.view, depth_slice: None, resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    }));
    gpu.queue.submit([encoder.finish()]);
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    super::gpu_events::presented();
    Some(Warmed(gpu, renderer))
}

/// What `warm` made, waiting for it if it's still making it.
pub(super) fn take() -> Option<(Gpu, Renderer)> {
    let mut state = lock(&SLOT.state);
    while matches!(*state, State::Making { .. }) {
        state = SLOT.changed.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    let State::Ready { warmed: Warmed(gpu, renderer), .. } = std::mem::take(&mut *state) else { return None };
    SLOT.changed.notify_all();
    Some((gpu, renderer))
}

/// `prewarm_gpu` is macOS's; elsewhere the surface decides the adapter.
pub(super) fn start() {}

/// What's warmed goes; a thread still making it lets it go when it's done.
#[cfg_attr(test, allow(dead_code))]
pub(super) fn discard() {
    let mut state = lock(&SLOT.state);
    if let State::Ready { .. } = &*state { drop(std::mem::take(&mut *state)); }
    else if let State::Making { until } = &mut *state { *until = Instant::now(); }
    SLOT.changed.notify_all();
}
