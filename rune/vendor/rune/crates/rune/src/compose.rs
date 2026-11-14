//! Compositor hook: an app takes over drawing each frame (lighting, effect layers) while Rune
//! keeps layout, animation, input, the window and the headless tooling. `rune::run` never
//! builds one, so the default path is unchanged; `rune::run_with` or `App::compositor` attaches
//! one.

use crate::render::{Instance, KIND_GLYPH, Renderer};
use crate::runtime::Runtime;

/// Draws a frame in place of Rune's default.
///
/// To paint custom content at `El::effect` slots, draw the frame with `Renderer::draw_with` and
/// paint in its hook: it runs at each slot in painter order, inside the slot's isolated layers,
/// so the slot's opacity (an exit fade included), clip and transform already apply. Keep
/// resources per `EffectSite::mark`, read the slot's `EffectSite::data`, and drop a mark's
/// resources once a frame passes without it:
///
/// ```ignore
/// struct Hud { waves: HashMap<u32, Wave>, seen: HashSet<u32> }
///
/// impl Compositor<App> for Hud {
///     fn render(&mut self, f: &mut Frame<'_, App>) {
///         let (device, queue, clear) = (f.device, f.queue, f.rt.clear_color());
///         f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
///         self.seen.clear();
///         f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
///             self.seen.insert(site.mark);
///             let wave = self.waves.entry(site.mark).or_insert_with(|| Wave::new(device));
///             wave.draw(device, queue, pass, site, site.data);
///         });
///         self.waves.retain(|mark, _| self.seen.contains(mark));
///     }
/// }
/// ```
pub trait Compositor<S: 'static>: 'static {
    /// Device features to request when the adapter has them (e.g. `TIMESTAMP_QUERY`).
    /// `IMMEDIATES` also raises `max_immediate_size` to the adapter's (4 KB on Metal): per-draw
    /// values set with `RenderPass::set_immediates` travel inside the pass, where a uniform
    /// written through the queue or `Frame::write_buffer` costs the frame another command buffer.
    fn features(&self) -> wgpu::Features {
        wgpu::Features::empty()
    }

    /// Draws the frame into `f.target`. Runs after `Runtime::frame` built the instances, in the
    /// window and headless alike. Nothing has been recorded yet; `El::pixels` images are uploaded.
    /// `f.device` can change between frames (an overlay shown again on another GPU): keep
    /// resources made on it with `f.device_key()`, and make them again when it changes. Never
    /// compare `wgpu::Device`s for this: wgpu compares them by id, and every Rune GPU has its
    /// own instance, whose first device has the same id as the last one's (hex#982).
    fn render(&mut self, f: &mut Frame<'_, S>);

    /// True while something the compositor draws moves on its own: the window keeps its display
    /// link running. False lets it go idle (no frames at rest) and rest: a due `wake_after`
    /// runs `poll` alone and draws only when that poll changed something.
    fn animating(&self) -> bool {
        false
    }

    /// Printed after a headless run (e.g. GPU timings).
    fn summary(&mut self, _device: &wgpu::Device) -> Option<String> {
        None
    }
}

/// One frame handed to a compositor.
pub struct Frame<'a, S: 'static> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The window surface or the offscreen target.
    pub target: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    /// Target size in device pixels.
    pub size: (u32, u32),
    /// Real seconds since the previous frame (0 on the first).
    pub dt: f64,
    /// CPU milliseconds `Runtime::frame` took this frame (handlers, view, layout, animation).
    pub update_ms: f64,
    /// Instances, `marks`, the clock (`t`), the pointer, element rects and app state.
    pub rt: &'a mut Runtime<S>,
    /// Rune's own renderer: `prepare_lists`, `encode_layers`, then `draw_range` draw complete
    /// painter scopes into any target (`add_format` for non-target formats). Layer begin/end
    /// records must remain balanced; do not partition a scope by primitive kind.
    pub ui: &'a mut Renderer,
}

/// Which device a frame draws on, unique for the process: a new device never has an earlier
/// key, as a `wgpu::Device` can have an earlier id. Keep it beside resources made with
/// `Frame::device`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceKey(u64);

impl DeviceKey {
    pub(crate) fn next() -> DeviceKey {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        DeviceKey(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

impl<S: 'static> Frame<'_, S> {
    /// The device `device` is, as a key that changes whenever the frame moves to another one.
    pub fn device_key(&self) -> DeviceKey {
        self.ui.device_key()
    }

    /// True when the frame is sRGB-blended (`App::srgb_blend`): `target` is a non-sRGB view and
    /// premultiplied colours, in `ui` passes and in layers, are sRGB-encoded. False: linear
    /// premultiplied values, encoded by the `*Srgb` target on write. Build effect pipelines and
    /// encode their output to match (`FxRenderer::set_srgb_blend` in rune-fx).
    pub fn srgb_blend(&self) -> bool {
        self.ui.srgb_blend()
    }

    /// `queue.write_buffer` without its per-call staging buffer: `data` goes through Rune's
    /// reused staging memory and lands in `buffer` at `offset` ahead of the commands recorded
    /// into `encoder` after this call. `offset` and `data.len()` are multiples of 4.
    pub fn write_buffer(&mut self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        self.ui.write_buffer(self.device, self.queue, self.encoder, buffer, offset, data);
    }

    /// The default frame: every instance into the target, cleared to the window colour.
    pub fn draw_ui(&mut self) {
        let clear = self.rt.clear_color();
        self.ui.prepare(self.device, self.queue, &mut self.rt.text.atlas, &self.rt.instances, self.size);
        self.ui.draw(self.encoder, self.target, clear);
    }
}

pub fn is_glyph(i: &Instance) -> bool {
    i.params[1] == KIND_GLYPH
}
