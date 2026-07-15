// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Platform-agnostic glyph rasterization entry points. `ensure_glyph_by_id`
//! looks the glyph up in the grid atlas first; on miss it rasterizes via
//! the platform-native backend (CoreText on macOS, swash elsewhere) and
//! inserts the bitmap into the atlas.

use neoism_backend::sugarloaf::grid::{
    AtlasSlot, GlyphKey, GridRenderer, RasterizedGlyph,
};
use neoism_ui::terminal_grid_emit::terminal_glyph_placement;

use crate::terminal::grid_emit::run_shaping::GridGlyphRasterizer;

/// Look up or rasterize-and-insert a glyph into the grid atlas by
/// `glyph_id`. Platform-agnostic entry point; cfg branches inside to
/// call the CT or swash rasterizer.
#[allow(clippy::too_many_arguments)]
pub(super) fn ensure_glyph_by_id(
    rasterizer: &mut GridGlyphRasterizer,
    grid: &mut GridRenderer,
    font_id: u32,
    glyph_id: u16,
    size_bucket: u16,
    size_u16: u16,
    cell_h: f32,
    ascent_px: i16,
    is_emoji: bool,
    synthetic_italic: bool,
    synthetic_bold: bool,
) -> Option<(GlyphKey, AtlasSlot, bool)> {
    let key = GlyphKey {
        font_id,
        glyph_id: glyph_id as u32,
        size_bucket,
    };
    if let Some(slot) = grid.lookup_glyph(key) {
        return Some((key, slot, false));
    }
    if let Some(slot) = grid.lookup_glyph_color(key) {
        return Some((key, slot, true));
    }

    // Rasterize via the platform-native backend.
    let raw = rasterize_glyph_native(
        rasterizer,
        font_id,
        glyph_id,
        size_u16,
        is_emoji,
        synthetic_bold,
        synthetic_italic,
    )?;
    let is_color = raw.is_color;

    let placement = terminal_glyph_placement(
        raw.width, raw.height, raw.left, raw.top, cell_h, ascent_px,
    );
    let raster = RasterizedGlyph {
        width: placement.width,
        height: placement.height,
        bearing_x: placement.bearing_x,
        bearing_y: placement.bearing_y,
        bytes: &raw.bytes,
    };

    let slot = if is_color {
        grid.insert_glyph_color(key, raster)?
    } else {
        grid.insert_glyph(key, raster)?
    };
    Some((key, slot, is_color))
}

/// Platform-agnostic raw-glyph struct. Both backends populate this
/// shape and let the caller convert bearings to the grid's
/// cell-bottom-relative convention.
struct RawGlyph {
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    is_color: bool,
    bytes: Vec<u8>,
}

#[cfg(target_os = "macos")]
fn rasterize_glyph_native(
    rasterizer: &mut GridGlyphRasterizer,
    font_id: u32,
    glyph_id: u16,
    size_u16: u16,
    is_emoji: bool,
    synthetic_bold: bool,
    synthetic_italic: bool,
) -> Option<RawGlyph> {
    let handle = rasterizer.handle_cache.get(&font_id)?.clone();
    let raw = neoism_backend::sugarloaf::font::macos::rasterize_glyph(
        &handle,
        glyph_id,
        size_u16 as f32,
        is_emoji,
        synthetic_italic,
        synthetic_bold,
    )?;
    Some(RawGlyph {
        width: raw.width,
        height: raw.height,
        left: raw.left,
        top: raw.top,
        is_color: raw.is_color,
        bytes: raw.bytes,
    })
}

#[cfg(not(target_os = "macos"))]
fn rasterize_glyph_native(
    rasterizer: &mut GridGlyphRasterizer,
    font_id: u32,
    glyph_id: u16,
    size_u16: u16,
    _is_emoji: bool,
    synthetic_bold: bool,
    synthetic_italic: bool,
) -> Option<RawGlyph> {
    use neoism_backend::sugarloaf::swash::{
        scale::{
            image::{Content, Image as GlyphImage},
            Render, Source, StrikeWith,
        },
        zeno::{Angle, Format, Transform},
        FontRef,
    };

    let font_entry = rasterizer.font_data_cache.get(&font_id)?.clone();
    let font_ref = FontRef {
        data: font_entry.0.as_ref(),
        offset: font_entry.1,
        key: font_entry.2,
    };

    let hinting = font_library_hinting(rasterizer);
    let mut scaler = rasterizer
        .scale_ctx
        .builder(font_ref)
        .hint(hinting)
        .size(size_u16 as f32)
        .build();

    let sources: &[Source] = &[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ];
    let mut image = GlyphImage::new();
    let ok = Render::new(sources)
        .format(Format::Alpha)
        .embolden(if synthetic_bold { 0.5 } else { 0.0 })
        .transform(if synthetic_italic {
            Some(Transform::skew(
                Angle::from_degrees(14.0),
                Angle::from_degrees(0.0),
            ))
        } else {
            None
        })
        .render_into(&mut scaler, glyph_id, &mut image);
    if !ok {
        return None;
    }
    let is_color = image.content == Content::Color;
    if is_color {
        neoism_backend::sugarloaf::font::premultiply_color_glyph(&mut image.data);
    }
    Some(RawGlyph {
        width: image.placement.width,
        height: image.placement.height,
        left: image.placement.left,
        top: image.placement.top,
        is_color,
        bytes: image.data,
    })
}

/// Hinting is a library-wide setting. Read once per rasterize; the
/// RwLock read is cheap. (Caching it locally would require reset
/// plumbing on config reload.)
#[cfg(not(target_os = "macos"))]
#[inline]
fn font_library_hinting(_r: &GridGlyphRasterizer) -> bool {
    // TODO: thread through from a cache to avoid the lock per glyph.
    // For now the lock on swash rasterize is a small fraction of
    // render time; optimise if profiling flags it.
    true
}
