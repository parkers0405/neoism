//! How a runtime's colours blend and its glyphs rasterise: sRGB or linear-light blending,
//! the font smoothing mode, and the per-glyph instances they shape.

use super::*;
use crate::text::FontSmoothing;

impl<S: 'static> Runtime<S> {
    /// Enable or disable gamma-space (sRGB) blending for translucent fills, shadows, blurs,
    /// images and video, as `App::srgb_blend` does for a window. Every colour the next frame
    /// builds, hover and press tints included, is premultiplied in sRGB encoding
    /// (`Color::premul_srgb`). A runtime drawn by the app itself (`Runtime::with_text`, a
    /// headless snapshot) then draws with `Renderer::set_srgb_blend(true)` into a non-sRGB
    /// target (`format.remove_srgb_suffix()`), cleared to `clear_color()`.
    pub fn set_srgb_blend(&mut self, enabled: bool) {
        if self.srgb_blend != enabled {
            self.srgb_blend = enabled;
            self.dirty = true;
        }
    }

    pub fn srgb_blend(&self) -> bool {
        self.srgb_blend
    }

    /// Rasterise and place glyphs in `mode` from the next frame, as `App::font_smoothing` does
    /// for a window. Glyphs already rasterised in another mode stay cached for a switch back.
    /// Blending is separate: both modes match CoreText with [`Self::set_srgb_blend`] on.
    pub fn set_font_smoothing(&mut self, mode: FontSmoothing) {
        if self.text.smoothing != mode {
            self.text.set_font_smoothing(mode);
            self.dirty = true;
        }
    }

    pub fn font_smoothing(&self) -> FontSmoothing {
        self.text.smoothing
    }

    pub fn clear_color(&self) -> [f32; 4] {
        if self.transparent { return [0.0; 4]; }
        self.nodes.get(self.app_root()).map_or([0.0, 0.0, 0.0, 1.0], |n| {
            let c = self.color(n.id, P_FILL, n.vis.fill);
            if self.srgb_blend {
                [srgb_encode(c.r), srgb_encode(c.g), srgb_encode(c.b), 1.0]
            } else {
                [c.r, c.g, c.b, 1.0]
            }
        })
    }
}

pub(crate) fn glyph_uv_page(uv: [f32; 2]) -> ([f32; 2], f32) {
    let page = (uv[1] as u32) / crate::text::ATLAS_SIZE;
    ([uv[0], uv[1] - (page * crate::text::ATLAS_SIZE) as f32], page as f32)
}

pub(crate) fn glyph(q: &GlyphQuad, fill: [f32; 4], clip: [f32; 4], fade: [f32; 4]) -> Instance {
    if let Some(o) = q.outline {
        // `text::Outline`'s layout: radii.w > 0 tells the shader to read the outline store.
        return Instance {
            rect: q.rect,
            radii: [o.pen[0], o.pen[1], o.scale, o.zoom],
            fill,
            border: [o.record as f32, o.hint as f32, o.radius, 0.0],
            params: [0.0, KIND_GLYPH, 0.0, 1.0],
            uv: [0.0; 4],
            clip,
            fade: crate::render::pack_fade(fade, clip, clip),
        };
    }
    let (uv, page) = glyph_uv_page(q.uv);
    Instance {
        rect: q.rect,
        radii: [0.0; 4],
        fill,
        border: [0.0; 4],
        params: [0.0, KIND_GLYPH, page, 1.0],
        uv: [uv[0], uv[1], 0.0, 0.0],
        clip,
        fade: crate::render::pack_fade(fade, clip, clip),
    }
}

/// An icon's `KIND_GLYPH` instance at `pos` with drawn side `drawn`. At an unrotated, whole-pixel
/// 1:1 placement it loads texels directly (`span == 0`); when fractional, scaled or turned it
/// expands one texel into the icon's transparent atlas border so bilinear filtering neither clips
/// at `KIND_GLYPH`'s quad edge nor bleeds across the shelf gutter.
pub(crate) fn icon_inst(
    q: &GlyphQuad,
    pos: [f32; 2],
    drawn: f32,
    turn: f32,
    fill: [f32; 4],
    clip: [f32; 4],
    fade: [f32; 4],
) -> Instance {
    let (uv, page) = glyph_uv_page(q.uv);
    let px = q.rect[2].max(1.0);
    let near = |v: f32| (v - v.round()).abs() < 1.0 / 512.0;
    let (rect, uv) = if turn == 0.0 && near(pos[0]) && near(pos[1]) && (drawn - px).abs() < 1.0 / 512.0 {
        ([pos[0].round(), pos[1].round(), px, px], [uv[0], uv[1], 0.0, 0.0])
    } else {
        let t = drawn / px;
        (
            [pos[0] - t, pos[1] - t, drawn + 2.0 * t, drawn + 2.0 * t],
            [uv[0] - 1.0, uv[1] - 1.0, px + 2.0, px + 2.0],
        )
    };
    Instance {
        rect,
        radii: [0.0; 4],
        fill,
        border: [0.0; 4],
        params: [turn, KIND_GLYPH, page, 1.0],
        uv,
        clip,
        fade: crate::render::pack_fade(fade, clip, clip),
    }
}

/// An icon instance's place in the atlas (`uv.xy` and page), which no text glyph shares.
pub(crate) fn apron_key(i: &Instance) -> [u32; 3] {
    [i.uv[0].to_bits(), i.uv[1].to_bits(), i.params[2].to_bits()]
}

pub(crate) fn premul(c: Color, opacity: f32, srgb: bool) -> [f32; 4] {
    if srgb { c.premul_srgb(opacity) } else { c.premul(opacity) }
}
