use crate::compose::Compositor;
use crate::runtime::Runtime;

/// Whether a compositor draws motion of its own (`Compositor::animating`): its window never
/// rests, and a due timer draws.
pub(super) fn composing<S: 'static>(comp: Option<&dyn Compositor<S>>) -> bool {
    comp.is_some_and(|c| c.animating())
}

/// What an idle window's last frame drew. A timer that wakes the window builds a frame before
/// taking a drawable; when it would draw these same pixels, nothing is acquired or presented.
/// A compositor's frame lets the window rest but never counts as the same: it may draw
/// differently from the same instances.
pub(crate) struct Presented {
    instances: Vec<crate::render::Instance>,
    clear: [f32; 4],
    size: (u32, u32),
    format: wgpu::TextureFormat,
    srgb: bool,
    scale: f32,
    transparent: bool,
    composed: bool,
}

impl Presented {
    /// After a frame: whether the window goes idle, and the frame it rests on until anything
    /// asks for another. `moving`: motion, a compositor or a window frame in flight; `next_tick`:
    /// the running display link's next tick; `rests`: the window may rest on this frame (shown,
    /// and no trace counting frames). A frame that left work for the next one doesn't settle:
    /// a compositor may update or invalidate the state while it draws.
    pub(super) fn settle<S: 'static>(this: &mut Option<Self>, rt: &Runtime<S>, config: &wgpu::SurfaceConfiguration,
        moving: bool, next_tick: Option<crate::time::Instant>, rests: bool, composed: bool) -> bool {
        let idle = !(moving || rt.stale() || rt.live_pending() || rt.window_frame.is_some()
            || super::timer_wants_frame(rt, next_tick));
        if idle && rests { Self::record(this, rt, config, composed) } else { *this = None }
        idle
    }

    pub(crate) fn record<S: 'static>(this: &mut Option<Self>, rt: &Runtime<S>, config: &wgpu::SurfaceConfiguration, composed: bool) {
        let (clear, size, srgb, scale, transparent) = (rt.clear_color(), (config.width, config.height), rt.srgb_blend(), rt.env.scale, rt.transparent);
        let p = this.get_or_insert_with(|| Self {
            instances: Vec::new(), clear, size, format: config.format, srgb, scale, transparent, composed,
        });
        if composed { p.instances.clear() } else { p.instances.clone_from(&rt.instances) }
        (p.clear, p.size, p.format, p.srgb, p.scale, p.transparent, p.composed) = (clear, size, config.format, srgb, scale, transparent, composed);
    }

    /// Whether a timer frame `rt` built before taking a drawable can go without one: nothing
    /// moves or waits, no timer wants the display link's next tick (`next_tick`), and it draws
    /// what this frame did.
    pub(crate) fn unchanged<S: 'static>(&self, rt: &Runtime<S>, config: &wgpu::SurfaceConfiguration, animating: bool,
        next_tick: Option<crate::time::Instant>) -> bool {
        !animating && !rt.has_pending_input() && !rt.live_pending() && rt.window_frame.is_none()
            && !super::timer_wants_frame(rt, next_tick) && self.same(rt, config)
    }

    /// Whether `rt`'s built frame draws what this one did: the same instances into the same
    /// surface, with no glyph or image upload waiting, and nothing drawn outside the instances
    /// (a compositor, `SurfaceDraw` content, native materials).
    pub(super) fn same<S: 'static>(&self, rt: &Runtime<S>, config: &wgpu::SurfaceConfiguration) -> bool {
        let atlas = &rt.text.atlas;
        !self.composed && self.size == (config.width, config.height) && self.format == config.format && self.srgb == rt.srgb_blend()
            && self.scale == rt.env.scale && self.clear == rt.clear_color() && self.transparent == rt.transparent
            && rt.surfaces.is_empty() && rt.materials.is_empty()
            && atlas.dirty.is_none() && atlas.pages.iter().all(|p| p.dirty.is_none())
            && rt.images.pages.iter().all(|p| p.dirty.is_none())
            && bytemuck::cast_slice::<_, u8>(&self.instances) == bytemuck::cast_slice::<_, u8>(&rt.instances)
    }
}
