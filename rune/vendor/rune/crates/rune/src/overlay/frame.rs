//! An overlay frame's steps apart from its window: the animation step, the compositor or
//! default draw, whether anything asks for another frame, and the submit and present.

use super::{Backing, Live};
use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::render::{Gpu, Renderer};
use crate::runtime::Runtime;
use crate::time::Instant;
use crate::window::Presented;

/// Whether nothing asks for the frame after one just presented. `next_tick` is the running
/// display link's next tick, if one paces the overlay: a timer due by then counts as asking,
/// so a poll stepping its own animation with `Tx::wake_after(Duration::ZERO)` gets every frame
/// at the display's rate instead of the link stopping after each frame. Unpaced, the timer
/// waits a frame in `wake_deadline` and never keeps the loop turning.
pub(super) fn idle_after<T: 'static>(rt: &Runtime<T>, animating: bool, next_tick: Option<Instant>) -> bool {
    !(animating || rt.has_pending_input() || crate::window::timer_wants_frame(rt, next_tick))
}

/// `draw_frame`'s animation step, apart from the surface so tests can drive it.
pub(super) fn step<T: 'static>(rt: &mut Runtime<T>, clock: &mut FrameClock, compositor: &Option<Box<dyn Compositor<T>>>, frame_time: Instant, prewarm: bool) -> (bool, f64) {
    if prewarm {
        clock.hold(frame_time);
        return (rt.frame(0.0), 0.0);
    }
    clock.advance(rt, frame_time, compositor.as_ref().is_some_and(|c| c.animating()))
}

/// What a shown overlay's last frame drew while it rests, and whether only a due timer asked
/// for the next. That frame builds before it takes a drawable or starts the display link, and
/// draws nothing when it would draw the same pixels: a poll that wakes on a timer and changes
/// nothing on screen presents nothing.
#[derive(Default)]
pub(super) struct Quiet {
    pub(super) timer: bool,
    pub(super) presented: Option<Presented>,
}

impl Quiet {
    /// After a frame presents: it rests on that frame, or its pixels are no longer known (a
    /// prewarm frame, motion, a compositor).
    pub(super) fn presented<T: 'static>(&mut self, rt: &Runtime<T>, config: &wgpu::SurfaceConfiguration, rests: bool) {
        if rests { Presented::record(&mut self.presented, rt, config, false) } else { self.presented = None }
    }
}

/// A resting overlay's timer frame, stepped before any drawable: `None` when it draws what
/// `presented` shows and nothing asks for the next (`next_tick`: the display link's next tick,
/// had it run), else the step's `(animating, dt)` to draw with.
pub(super) fn quiet_step<T: 'static>(rt: &mut Runtime<T>, clock: &mut FrameClock, presented: &Presented,
    config: &wgpu::SurfaceConfiguration, now: Instant, next_tick: Option<Instant>) -> Option<(bool, f64)> {
    let (animating, dt) = step(rt, clock, &None, now, false);
    (!presented.unchanged(rt, config, animating, next_tick)).then_some((animating, dt))
}

/// Whether a frame can leave the runtime as it is, drawing its last build again: after the
/// last frame it asked for no other (`runtime_idle`: only the compositor animates), nothing
/// marked the overlay dirty since, and no input, live value, timer, playground or poll (which
/// runs on every frame drawn) waits. An overlay without a compositor draws nothing then.
pub(super) fn rests<T: 'static>(rt: &Runtime<T>, runtime_idle: bool, dirty: bool) -> bool {
    runtime_idle && !dirty && rt.poll.is_none() && rt.playground.is_none() && rt.wake_deadline().is_none() && rt.rests()
}

/// `step` for a runtime that `rests`: its clocks (`clock`, `t`) move to the frame as a step
/// would move them, so a compositor reading them animates the same, and nothing runs or builds.
pub(super) fn rest<T: 'static>(rt: &mut Runtime<T>, clock: &mut FrameClock, compositor: &Option<Box<dyn Compositor<T>>>, frame_time: Instant) -> (bool, f64) {
    let elapsed = clock.step(frame_time, compositor.as_ref().is_some_and(|c| c.animating())).total();
    rt.frame_target = Some(frame_time);
    rt.clock = rt.wall_start.map_or(rt.clock + elapsed, |t0| t0.elapsed().as_secs_f64());
    rt.t += elapsed * rt.time_scale as f64;
    rt.scene.set_time(rt.t);
    (false, elapsed)
}

/// A due frame's target presentation time, or `None` when the display link says wait. The
/// opportunity is claimed before `settle` waits for the frame still on the present thread, so
/// a turn that draws nothing (the loop's second look after a tick drew) never blocks on it.
#[cfg(target_os = "macos")]
pub(super) fn claim(opportunity: Option<crate::platform::display_link::Opportunity>, settle: impl FnOnce()) -> Option<Option<Instant>> {
    let target = match opportunity { Some(opportunity) => opportunity.target()?, None => None };
    settle();
    Some(target)
}

/// Both native surface frames and offscreen prewarming use the same compositor path.
pub(super) fn render_frame<T: 'static>(f: &mut Frame<'_, T>, compositor: &mut Option<Box<dyn Compositor<T>>>) -> bool {
    f.ui.upload_images(f.device, f.queue, &mut f.rt.images);
    f.ui.prepare_surfaces(f.device, f.queue, &f.rt.surfaces, f.size);
    if let Some(c) = compositor {
        c.render(f);
        c.animating()
    } else {
        f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        f.ui.draw(f.encoder, f.target, [0.0; 4]);
        false
    }
}

/// Submits the frame's commands and presents its drawable. On macOS a plain frame (no material
/// views to move with the drawable, no transaction, no native video surface waiting on the
/// submission) is finished, submitted and presented on the overlay's present thread, so the
/// event loop's thread spends none of wgpu's encode, submit and present on it. Prewarm frames
/// present here, before the window is revealed.
pub(super) fn submit_and_present<T: 'static>(live: &mut Live, renderer: &Renderer, gpu: &Gpu, rt: &Runtime<T>,
    encoder: wgpu::CommandEncoder, frame: wgpu::SurfaceTexture, prewarm: bool) {
    #[allow(irrefutable_let_patterns)]
    if let Backing::Window(w) = &live.backing {
        w.pre_present_notify();
    }
    #[cfg(target_os = "macos")]
    {
        if !prewarm && live.materials.presents_plainly() && renderer.submits_plainly() && !crate::gpu::native(&gpu.queue) {
            if live.present.is_none() { live.present = crate::frame::present_worker::PresentWorker::spawn(); }
            if let Some(worker) = &mut live.present {
                let mut encoder = encoder;
                renderer.before_submit(&mut encoder);
                let queue = gpu.queue.clone();
                return worker.run(move || {
                    queue.submit([encoder.finish()]);
                    queue.present(frame);
                });
            }
        }
        renderer.submit(&gpu.queue, encoder);
        live.materials.present(&rt.materials, rt.env.scale, || gpu.queue.present(frame));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (rt, prewarm);
        renderer.submit(&gpu.queue, encoder);
        gpu.queue.present(frame);
    }
}

impl Live {
    /// Waits for the frame on the present thread, before the surface is touched again.
    pub(super) fn settle(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(worker) = &mut self.present { worker.wait(); }
    }
}
