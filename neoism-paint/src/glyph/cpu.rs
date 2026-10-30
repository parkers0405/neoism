// Copyright (c) 2023-present, Raphael Amorim.
//
// CPU backend for the grid renderer.
//
// Mirrors the GPU backends (`metal`, `vulkan`, `webgpu`) in scope and
// data layout, but rasterises directly into the softbuffer u32 pixel
// buffer instead of recording draw calls. Storage matches the Metal
// renderer cell-for-cell: one flat `Vec<CellBg>` indexed by
// `row * cols + col`, plus per-row `Vec<CellText>` slots (slot 0 =
// block-cursor cells, 1..=rows = content rows, last = non-block
// cursor) so the row-rebuild path in `frontends/neoism/src/terminal/grid_emit`
// stays backend-agnostic.
//
// The atlases live in RAM. `CpuGridAtlas` packs glyph bitmaps into a
// shelf-allocated grayscale (R8) or color (RGBA8 premul) buffer with
// the same `AtlasAllocator` the GPU paths use, so the atlas slot
// coordinates round-trip across backends.
//
// Position math in `render` mirrors `grid.metal`'s vertex shaders so
// glyph placement matches the GPU paths pixel-for-pixel:
//   - cell origin = (col * cell_w + grid_padding.left,
//                    row * cell_h + grid_padding.top)
//   - glyph origin = cell_origin + (bearing_x, cell_h - bearing_y)

use rustc_hash::FxHashMap;

use super::atlas::{AtlasSlot, GlyphKey, RasterizedGlyph};
use crate::atlas::AtlasAllocator;

/// Initial atlas side. 1024² bytes_per_pixel = 1 MiB grayscale,
/// 4 MiB color. Smaller than the Metal default (2048²) since CPU
/// builds are usually memory-constrained machines.
const ATLAS_SIZE: u16 = 1024;
const ATLAS_MAX_SIZE: u16 = 4096;

/// Slot 0 = block-cursor cells, slot `rows + 1` = non-block-cursor
/// cells. Matches the Metal layout in `metal::init_fg_rows`.
const CURSOR_ROW_SLOTS: usize = 2;

/// CPU-side glyph atlas. R8 for grayscale masks, RGBA8 premultiplied
/// for color emoji. Uses the same shelf allocator as the GPU atlases
/// so `AtlasSlot` coords round-trip.
pub struct CpuGridAtlas {
    pixels: Vec<u8>,
    side: u16,
    bytes_per_pixel: u8,
    allocator: AtlasAllocator,
    slots: FxHashMap<GlyphKey, AtlasSlot>,
}

impl CpuGridAtlas {
    fn new(bytes_per_pixel: u8) -> Self {
        let side = ATLAS_SIZE;
        let pixels =
            vec![0u8; (side as usize) * (side as usize) * (bytes_per_pixel as usize)];
        Self {
            pixels,
            side,
            bytes_per_pixel,
            allocator: AtlasAllocator::new(side, side),
            slots: FxHashMap::default(),
        }
    }

    pub fn new_grayscale() -> Self {
        Self::new(1)
    }

    pub fn new_color() -> Self {
        Self::new(4)
    }

    #[inline]
    pub fn lookup(&self, key: GlyphKey) -> Option<AtlasSlot> {
        self.slots.get(&key).copied()
    }

    pub fn insert(
        &mut self,
        key: GlyphKey,
        glyph: RasterizedGlyph<'_>,
    ) -> Option<AtlasSlot> {
        if glyph.width == 0 || glyph.height == 0 {
            // Zero-sized glyphs (e.g. spaces) still need a cache entry
            // so the rasterizer doesn't keep producing them, but they
            // occupy no atlas space.
            let slot = AtlasSlot {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
                bearing_x: glyph.bearing_x,
                bearing_y: glyph.bearing_y,
            };
            self.slots.insert(key, slot);
            return Some(slot);
        }

        let (x, y) = self.allocator.allocate(glyph.width, glyph.height)?;
        let slot = AtlasSlot {
            x,
            y,
            w: glyph.width,
            h: glyph.height,
            bearing_x: glyph.bearing_x,
            bearing_y: glyph.bearing_y,
        };
        self.slots.insert(key, slot);
        self.write_pixels(
            x as usize,
            y as usize,
            glyph.width as usize,
            glyph.height as usize,
            glyph.bytes,
        );
        Some(slot)
    }

    fn write_pixels(&mut self, x: usize, y: usize, w: usize, h: usize, src: &[u8]) {
        let bpp = self.bytes_per_pixel as usize;
        let stride = self.side as usize * bpp;
        let row_bytes = w * bpp;
        for row in 0..h {
            let src_off = row * row_bytes;
            let dst_off = (y + row) * stride + x * bpp;
            self.pixels[dst_off..dst_off + row_bytes]
                .copy_from_slice(&src[src_off..src_off + row_bytes]);
        }
    }

    /// Double the atlas side, copying old pixels into the top-left of
    /// the new buffer. Existing `AtlasSlot`s stay valid because their
    /// `(x, y)` fall inside the unchanged old region. Returns `false`
    /// when already at `ATLAS_MAX_SIZE`.
    pub fn grow(&mut self) -> bool {
        let (old_w, old_h) = self.allocator.dimensions();
        if old_w >= ATLAS_MAX_SIZE {
            return false;
        }
        let new_side = old_w.saturating_mul(2).min(ATLAS_MAX_SIZE);
        if new_side <= old_w {
            return false;
        }
        let bpp = self.bytes_per_pixel as usize;
        let mut new_pixels = vec![0u8; (new_side as usize) * (new_side as usize) * bpp];

        let old_stride = old_w as usize * bpp;
        let new_stride = new_side as usize * bpp;
        for row in 0..old_h as usize {
            let src_off = row * old_stride;
            let dst_off = row * new_stride;
            new_pixels[dst_off..dst_off + old_stride]
                .copy_from_slice(&self.pixels[src_off..src_off + old_stride]);
        }
        self.pixels = new_pixels;
        self.side = new_side;
        self.allocator.grow_to(new_side, new_side);
        true
    }

    #[inline]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[inline]
    pub fn side(&self) -> u16 {
        self.side
    }

    /// Reuse the existing pixel allocation for a fresh glyph generation.
    /// Old pixels do not need clearing: no slot references them after this
    /// metadata reset, and every newly allocated rectangle is overwritten.
    pub fn clear(&mut self) {
        self.allocator.clear();
        self.slots.clear();
    }
}
