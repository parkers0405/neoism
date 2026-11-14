//! The glyph atlas: shelf-packed greyscale pages of glyph and icon masks, aged and evicted by
//! use, and the quads that sample them.

use super::*;
use super::atlas_fill::Residency;

pub const ATLAS_SIZE: u32 = 2048;

/// Looked up for every glyph of every frame, so hashed with Fx rather than SipHash.
pub(super) type GlyphMap<V> = HashMap<GlyphKey, V, std::hash::BuildHasherDefault<rune_motion::Fx>>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GlyphKey {
    pub(super) font: u64,
    pub(super) index: u32,
    pub(super) glyph: u32,
    pub(super) size: u32,
    pub(super) coords: u64,
    /// The x phase in sixtieths of a pixel (`coretext_match::PHASES`).
    pub(super) subpx: u8,
    /// How far down the mask is drawn, in quarters of a pixel (`glyph_emit::GLIDE_PHASES`).
    pub(super) subpy: u8,
    /// The dilation radius in device px, as f32 bits.
    pub(super) dilation: u32,
    /// The CoreGraphics pipeline's stages (`macos::smoothing::Smoothing::key`).
    pub(super) apple: u8,
    /// Synthetic oblique, in degrees (0 for none).
    pub(super) skew: u8,
}

#[derive(Clone, Copy)]
pub(super) struct AtlasGlyph {
    pub(super) page: u16,
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) left: i32,
    pub(super) top: i32,
}

/// Steady-state shelf pages (page 0 + one secondary page for ping-pong eviction).
pub(super) const MAX_STEADY_PAGES: usize = 2;
/// Hard ceiling on total glyph atlas pages if a single frame or unframed loop overflows.
pub(super) const MAX_OVERFLOW_PAGES: usize = 8;
/// CPU byte budget for cached RGBA colour glyphs (`color_pixels`) before frame-boundary eviction.
pub(super) const COLOR_GLYPH_BUDGET: usize = 16 * 1024 * 1024;

struct Shelf {
    x: u32,
    y: u32,
    h: u32,
    shown: bool,
    touched: bool,
    full: bool,
    age: u64,
    live_texels: u32,
}

impl Shelf {
    fn new(age: u64) -> Self {
        Shelf { x: 1, y: 1, h: 0, shown: false, touched: false, full: false, age, live_texels: 0 }
    }

    fn reset(&mut self, age: u64) {
        *self = Self::new(age);
    }

    fn non_empty(&self) -> bool {
        self.x > 1 || self.y > 1 || self.h > 0
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.x + w + 1 > ATLAS_SIZE {
            self.y += self.h + 1;
            self.x = 1;
            self.h = 0;
        }
        if self.y + h + 1 > ATLAS_SIZE {
            self.full = true;
            return None;
        }
        let pos = (self.x, self.y);
        self.x += w + 1;
        self.h = self.h.max(h);
        Some(pos)
    }

    fn touch(&mut self, texels: u32) {
        self.touched = true;
        self.live_texels = self.live_texels.saturating_add(texels);
    }

    fn end_frame(&mut self) {
        self.shown = std::mem::take(&mut self.touched);
        self.live_texels = 0;
    }
}

pub struct AtlasPage {
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    shelf: Shelf,
}

impl AtlasPage {
    pub(super) fn new(age: u64) -> Self {
        AtlasPage {
            data: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            dirty: None,
            shelf: Shelf::new(age),
        }
    }
}

pub(super) enum Rasterized {
    Empty,
    Placed(AtlasGlyph),
    Unplaced,
}

/// One glyph to draw: device-pixel rect, atlas origin, brush and source byte (for reveals).
#[derive(Clone, Copy, Debug)]
pub struct GlyphQuad {
    pub rect: [f32; 4],
    /// Atlas origin with the page packed into y (`page * ATLAS_SIZE + y`).
    /// Use [`Self::atlas_page`] and [`Self::atlas_uv`] when sampling an atlas page directly.
    pub uv: [f32; 2],
    pub ink: u16,
    pub byte: u32,
    /// A colour glyph (emoji) drawn from [`TextSystem::color_glyph`] instead of the alpha
    /// atlas: 0 for none, else its index plus one.
    pub color: u32,
    /// A glyph drawn straight from its outline instead of the atlas (`uv` unused).
    pub outline: Option<super::Outline>,
}

impl GlyphQuad {
    /// Alpha atlas page. Placements remain valid until the next frame begins.
    pub fn atlas_page(&self) -> usize { self.uv[1] as usize / ATLAS_SIZE as usize }

    /// Pixel origin within [`Self::atlas_page`], without the packed page index.
    pub fn atlas_uv(&self) -> [f32; 2] {
        [self.uv[0], self.uv[1] - (self.atlas_page() * ATLAS_SIZE as usize) as f32]
    }
}

/// A colour glyph's RGBA image (`GlyphQuad::color`) and its placement around the pen (device px).
#[derive(Clone, Copy)]
pub(super) struct ColorGlyph {
    pub(super) index: u32,
    pub(super) used: bool,
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) w: u32,
    pub(super) h: u32,
}

impl ColorGlyph {
    /// The quad for a resting pen at `[x, y]`, rounded at rest, then moved by `glide`
    /// (the image draws filtered).
    pub(super) fn quad(&self, x: f32, y: f32, glide: [f32; 2], ink: u16, byte: u32) -> GlyphQuad {
        let rect = [
            x.round() + self.left as f32 + glide[0],
            y.round() - self.top as f32 + glide[1],
            self.w as f32,
            self.h as f32,
        ];
        GlyphQuad { rect, uv: [0.0, 0.0], ink, byte, color: self.index, outline: None }
    }
}

pub struct Atlas {
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    /// `(node_id, signature)` of `El::cache` targets currently resident in the GPU renderer.
    pub(crate) resident_caches: HashMap<u64, u64>,
    /// Secondary and overflow shelf pages (`page >= 1` lives at `pages[page - 1]`).
    pub(crate) pages: Vec<AtlasPage>,
    pub(crate) resets: u64,
    pub(crate) rasterized: u64,
    shelf: Shelf,
    pub(super) clock: u64,
    pub(super) map: GlyphMap<Residency>,
    /// Per page, the entries whose GPU fill is queued (`atlas_fill.rs`).
    pub(super) fills: Vec<Vec<GlyphKey>>,
    pub(super) icons: HashMap<(usize, u32), Option<AtlasGlyph>>,
    /// Glyph outlines drawn straight on the GPU, uploaded with the pages.
    pub(crate) outlines: super::outline::OutlineStore,
}

impl Atlas {
    /// Marks every row for upload (a new texture, e.g. after the window's format changed).
    pub fn invalidate(&mut self) {
        self.dirty = Some((0, ATLAS_SIZE));
        for page in &mut self.pages {
            page.dirty = Some((0, ATLAS_SIZE));
        }
        self.resident_caches.clear();
        self.outlines.invalidate();
    }

    /// Number of allocated 2048×2048 shelf pages (at least 1).
    pub fn page_count(&self) -> usize {
        1 + self.pages.len()
    }

    /// Alpha texels for a page returned by [`GlyphQuad::atlas_page`].
    pub fn page_data(&self, page: usize) -> Option<&[u8]> {
        if page == 0 { Some(&self.data) } else { self.pages.get(page - 1).map(|p| p.data.as_slice()) }
    }

    /// Number of shelf page resets since the atlas was created.
    pub fn resets(&self) -> u64 {
        self.resets
    }

    /// Number of glyphs and icons placed into the atlas since creation.
    pub fn rasterized(&self) -> u64 {
        self.rasterized
    }

    /// Rasterized glyph entries, for tests that bound the atlas.
    #[cfg(test)]
    pub(crate) fn glyph_entries(&self) -> usize { self.map.len() }

    pub(super) fn new() -> Self {
        Atlas {
            data: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            dirty: None,
            resident_caches: HashMap::new(),
            pages: Vec::new(),
            resets: 0,
            rasterized: 0,
            shelf: Shelf::new(1),
            clock: 1,
            map: GlyphMap::default(),
            fills: Vec::new(),
            icons: HashMap::new(),
            outlines: Default::default(),
        }
    }

    fn shelf(&self, page: usize) -> &Shelf {
        if page == 0 { &self.shelf } else { &self.pages[page - 1].shelf }
    }

    fn shelf_mut(&mut self, page: usize) -> &mut Shelf {
        if page == 0 { &mut self.shelf } else { &mut self.pages[page - 1].shelf }
    }

    pub(super) fn touch(&mut self, page: u16, texels: u32) {
        if page == 0 {
            self.shelf.touch(texels);
        } else if let Some(p) = self.pages.get_mut(page as usize - 1) {
            p.shelf.touch(texels);
        }
    }

    pub(super) fn evict_page(&mut self, page: usize) {
        let p16 = page as u16;
        self.map.retain(|_, v| v.mask().is_none_or(|g| g.page != p16));
        if let Some(queue) = self.fills.get_mut(page) { queue.clear(); }
        self.icons.retain(|_, v| v.is_none_or(|g| g.page != p16));
        self.clock += 1;
        self.resets += 1;
        let age = self.clock;
        if page == 0 {
            self.dirty = None;
            self.shelf.reset(age);
        } else if let Some(p) = self.pages.get_mut(page - 1) {
            p.dirty = None;
            p.shelf.reset(age);
        }
    }

    pub(crate) fn begin_frame(&mut self, pinned_mask: u32) {
        // A text flight's glyphs keep their outline records until it lands.
        if self.outlines.begin_frame(pinned_mask != 0) {
            self.forget_outline_masks();
        }
        let total = self.page_count();
        let total_live: u64 = (0..total).map(|p| self.shelf(p).live_texels as u64).sum();
        let half_page = (ATLAS_SIZE * ATLAS_SIZE / 2) as u64;
        let needed = ((total_live / half_page) as usize + MAX_STEADY_PAGES)
            .clamp(MAX_STEADY_PAGES, MAX_OVERFLOW_PAGES);
        let full_count = (0..total).filter(|&p| self.shelf(p).full).count();
        if full_count >= needed || total > needed {
            let keep = needed.min(total);
            if let Some(oldest) = (0..keep)
                .filter(|&p| self.shelf(p).full && (pinned_mask & (1u32 << p.min(31))) == 0)
                .min_by_key(|&p| self.shelf(p).age)
            {
                self.evict_page(oldest);
            }
            for p in keep..total {
                if (pinned_mask & (1u32 << p.min(31))) == 0 && self.shelf(p).non_empty() {
                    self.evict_page(p);
                }
            }
            while self.page_count() > needed {
                let last = self.page_count() - 1;
                if (pinned_mask & (1u32 << last.min(31))) != 0 || self.shelf(last).non_empty() {
                    break;
                }
                self.pages.pop();
            }
        }
        for p in 0..self.page_count() {
            self.shelf_mut(p).end_frame();
            if (pinned_mask & (1u32 << p.min(31))) != 0 {
                self.touch(p as u16, 0);
            }
        }
    }

    pub(super) fn alloc(&mut self, w: u32, h: u32) -> Option<(usize, u32, u32)> {
        if w == 0 || h == 0 || w + 2 > ATLAS_SIZE || h + 2 > ATLAS_SIZE {
            return None;
        }
        let total = self.page_count();
        for p in 0..total {
            if !self.shelf(p).full && let Some((x, y)) = self.shelf_mut(p).alloc(w, h) {
                return Some((p, x, y));
            }
        }
        // Try shelf tails on full pages for small glyphs before opening or resetting a page.
        for p in 0..total {
            if let Some((x, y)) = self.shelf_mut(p).alloc(w, h) {
                return Some((p, x, y));
            }
        }
        // Recycle an idle page (untouched in both the last frame and this frame so far).
        if let Some(p) = (0..total)
            .filter(|&p| {
                let s = self.shelf(p);
                !s.touched && !s.shown && s.non_empty()
            })
            .min_by_key(|&p| self.shelf(p).age)
        {
            self.evict_page(p);
            let (x, y) = self.shelf_mut(p).alloc(w, h).unwrap();
            return Some((p, x, y));
        }
        // Once we have the steady-state page pair, recycle the oldest page not touched this frame.
        if total >= MAX_STEADY_PAGES
            && let Some(p) = (0..total)
                .filter(|&p| {
                    let s = self.shelf(p);
                    !s.touched && s.non_empty()
                })
                .min_by_key(|&p| self.shelf(p).age)
        {
            self.evict_page(p);
            let (x, y) = self.shelf_mut(p).alloc(w, h).unwrap();
            return Some((p, x, y));
        }
        if total < MAX_OVERFLOW_PAGES {
            self.clock += 1;
            let mut page = AtlasPage::new(self.clock);
            let (x, y) = page.shelf.alloc(w, h).unwrap();
            self.pages.push(page);
            return Some((self.pages.len(), x, y));
        }
        // Every page is referenced by this frame (or pinned by a text flight). Never
        // overwrite emitted quads to satisfy a later request. Failure is not cached,
        // so the request can succeed after a subsequent frame frees capacity.
        None
    }

    pub(super) fn put(&mut self, w: u32, h: u32, src: &[u8], left: i32, top: i32) -> Option<AtlasGlyph> {
        let (page, ax, ay) = self.alloc(w, h)?;
        let (data, dirty) = if page == 0 {
            (&mut self.data, &mut self.dirty)
        } else {
            let p = &mut self.pages[page - 1];
            (&mut p.data, &mut p.dirty)
        };
        // Zero the 1px border around `[ax..ax+w, ay..ay+h]` so bilinear filtering never samples
        // stale texels from a previous generation without zeroing the whole 4 MB page on reset.
        let x0 = (ax - 1) as usize;
        let x1 = (ax + w + 1) as usize;
        let top_row = (ay - 1) as usize * ATLAS_SIZE as usize;
        data[top_row + x0..top_row + x1].fill(0);
        for row in 0..h {
            let s = &src[(row * w) as usize..((row + 1) * w) as usize];
            let line = (ay + row) as usize * ATLAS_SIZE as usize;
            data[line + x0] = 0;
            data[line + ax as usize..line + (ax + w) as usize].copy_from_slice(s);
            data[line + (ax + w) as usize] = 0;
        }
        let bot_row = (ay + h) as usize * ATLAS_SIZE as usize;
        data[bot_row + x0..bot_row + x1].fill(0);
        let (y0, y1) = (ay - 1, ay + h + 1);
        let d = dirty.get_or_insert((y0, y1));
        *d = (d.0.min(y0), d.1.max(y1));
        self.rasterized += 1;
        self.touch(page as u16, w * h);
        Some(AtlasGlyph { page: page as u16, x: ax, y: ay, w, h, left, top })
    }
}
