//! Effect slots (`El::effect`): places in the painter stream a compositor paints into.
use crate::El;

/// An effect slot's settings (`El::effect` and its builders).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EffectSlot {
    /// Points past the rect the compositor may paint (`El::effect_bleed`).
    pub bleed: f32,
    /// Asks for a backdrop copy (`El::effect_backdrop`).
    pub backdrop: bool,
    /// The app's payload (`El::effect_data`).
    pub data: [f32; 4],
}

impl PartialEq for EffectSlot {
    // Bitwise, so a NaN-packed payload still lets a memoized row be reused.
    fn eq(&self, o: &Self) -> bool {
        self.bleed == o.bleed && self.backdrop == o.backdrop && self.data.map(f32::to_bits) == o.data.map(f32::to_bits)
    }
}

impl El {
    /// Makes this element an effect slot for a compositor, tagged `mark`. It draws nothing
    /// itself but keeps its place in the painter stream with its rect, clip, rounded clip and
    /// enclosing isolated layer (opacity, blur, dissolve), so a compositor can paint its own
    /// pass there. See `Renderer::draw_with` and the pattern on `compose::Compositor`.
    ///
    /// The hook gets the slot as an `EffectSite`: `mark` says which slot it is and `data`
    /// carries [`Self::effect_data`]. Give each slot that can be on screen at once its own mark,
    /// such as one per page, so an outgoing slot and its replacement keep separate resources:
    ///
    /// ```ignore
    /// effect(HUD_BASE + page.id).effect_data([page.wave_id as f32, page.phase, 0.0, 0.0])
    ///     .key(page.id).transition(Transition::opacity())
    /// ```
    ///
    /// The slot is where the element is presented: `El::scale`, `El::offset`, `live_scale`,
    /// and a transition's scale and offset while it enters or leaves move and scale its rect,
    /// and set `EffectSite::scale` (device pixels per point). A partial opacity, including an
    /// enter or exit fade, puts the slot in its own isolated layer that applies it. Rotation is
    /// not in the slot's record: under `El::rotate` the slot draws upright inside a turned
    /// isolated layer (`EffectSite::in_layer`), and Rune turns what the hook painted.
    pub fn effect(mut self, mark: u32) -> Self {
        if mark != 0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().mark = mark;
        }
        self.vis.cold_mut().effect.get_or_insert_with(EffectSlot::default);
        self
    }
    /// Points past the rect an effect slot may paint (a glow's reach). Grows the bounds of an
    /// enclosing isolated layer; the clip still applies.
    pub fn effect_bleed(mut self, pt: f32) -> Self {
        self.vis.cold_mut().effect.get_or_insert_with(EffectSlot::default).bleed = pt.max(0.0);
        self
    }
    /// Request a backdrop copy up to this effect slot (`EffectSite::backdrop`) when drawn with
    /// `Renderer::draw_with`, `Renderer::draw_with_backdrop`, or `Renderer::encode_layers_with`.
    /// Ordinary slots (`false`, the default) stay inside the open render pass with no copy.
    /// A target without `COPY_SRC` usage can't be copied: the slot draws in the open pass with `backdrop == None`.
    pub fn effect_backdrop(mut self, on: bool) -> Self {
        self.vis.cold_mut().effect.get_or_insert_with(EffectSlot::default).backdrop = on;
        self
    }
    /// Four floats this effect slot hands its compositor as `EffectSite::data`, such as which
    /// snapshot of data to draw, a seed or a phase. They travel bit for bit in the slot's
    /// record, so `f32::from_bits` round-trips a 32-bit id. A removed element keeps the payload
    /// it was last built with through its exit transition, with no bookkeeping in the app.
    /// Makes the element an effect slot if it isn't one yet (tag 0 unless `El::effect` sets one).
    pub fn effect_data(mut self, data: [f32; 4]) -> Self {
        self.vis.cold_mut().effect.get_or_insert_with(EffectSlot::default).data = data;
        self
    }
}
