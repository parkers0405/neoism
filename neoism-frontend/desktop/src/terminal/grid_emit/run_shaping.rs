// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Run-shaping infrastructure: per-row glyph rasterizer cache, the
//! bucketed LRU `RunCacheEntry` table, and the platform-specific
//! shape-a-single-run helpers.

use neoism_backend::sugarloaf::font::FontLibrary;
use neoism_terminal_core::crosswords::square::Square;
use neoism_terminal_core::crosswords::style::StyleFlags;
use neoism_ui::terminal_grid_emit::is_terminal_run_breaker;
use rustc_hash::FxHashMap;

/// 256 × 8 bucketed LRU cache — CellCacheTable.
const RUN_BUCKET_COUNT: usize = 256;
const RUN_BUCKET_SIZE: usize = 8;

/// One shaped glyph. Same shape from both CoreText (macOS) and swash
/// (non-macOS). `cluster` is a UTF-8 byte offset into the run string.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // `x` / `y` / `advance` kept for future kerning-aware layout
pub(super) struct ShapedGlyph {
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    pub cluster: u32,
}

pub(super) struct RunCacheEntry {
    /// 64-bit rapidhash of (font_id, size_bucket, style_flags, run bytes).
    /// We key on the hash alone — no stored run string, no equality
    /// check on lookup. `CellCacheTable` pattern
    ///: rapidhash / wyhash pass
    /// SMHasher, so a random collision costs a wrong-glyph frame
    /// until the next row rebuild but never corrupts state. Birthday
    /// bound at N=10k concurrent cache entries ≈ 2.7×10⁻¹².
    pub hash: u64,
    pub glyphs: Vec<ShapedGlyph>,
}

/// Primary-font vertical metrics shared by every fallback run in a terminal
/// cell. `baseline_px` is measured from the natural cell top; callers add
/// half of any extra configured line-height before atlas placement.
#[derive(Clone, Copy, Debug)]
pub(super) struct GridVerticalMetrics {
    pub baseline_px: i16,
    pub natural_cell_height_px: u16,
}

impl GridVerticalMetrics {
    pub(super) fn fallback(size_px: u16) -> Self {
        Self {
            baseline_px: (f32::from(size_px) * 0.8)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            natural_cell_height_px: size_px.max(1),
        }
    }

    /// Centre the primary font's natural line box inside the terminal's
    /// configured cell height and return the resulting top-to-baseline
    /// distance. This keeps extra line-height symmetric above and below the
    /// text instead of accumulating it all below the baseline.
    pub(super) fn baseline_for_cell_height(self, cell_height_px: f32) -> i16 {
        let cell_height_px = cell_height_px.round().clamp(0.0, i16::MAX as f32) as i32;
        let extra_height = cell_height_px - i32::from(self.natural_cell_height_px);
        self.baseline_px.saturating_add(
            (extra_height / 2).clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        )
    }
}

pub struct GridGlyphRasterizer {
    pub(super) font_resolve: FxHashMap<(char, u8), (u32, bool)>,
    pub(super) vertical_metrics_cache: FxHashMap<(u32, u16), GridVerticalMetrics>,
    /// `(should_embolden, should_italicize)` per font_id. Read from
    /// `FontData` synthesis flags; matches the rich-text rasterizer's
    /// convention.
    pub(super) synthesis_cache: FxHashMap<u32, (bool, bool)>,
    pub(super) run_cache: Vec<Vec<RunCacheEntry>>,

    // macOS: stage the run in UTF-16 (what CoreText wants natively)
    // so the shaper call can hand the buffer straight to
    // `CFStringCreateWithCharactersNoCopy` with no encoding
    // conversion. `coretext.zig:88-104` — UTF-16
    // `unichars` + a parallel cell-start table for the cluster →
    // cell mapping.
    #[cfg(target_os = "macos")]
    pub(super) run_utf16_scratch: Vec<u16>,
    /// On macOS, `run_cell_starts[i]` is the offset (in UTF-16 code
    /// units) where cell `i` of the run begins inside
    /// `run_utf16_scratch`. Length = cells in the run. Used to walk
    /// shaped glyphs back to the cell they belong to.
    #[cfg(target_os = "macos")]
    pub(super) run_cell_starts: Vec<u32>,
    /// Cached CoreText handles per font_id.
    #[cfg(target_os = "macos")]
    pub(super) handle_cache:
        FxHashMap<u32, neoism_backend::sugarloaf::font::macos::FontHandle>,

    // non-macOS: swash wants UTF-8, so keep a `String` scratch.
    #[cfg(not(target_os = "macos"))]
    pub(super) run_str_scratch: String,
    #[cfg(not(target_os = "macos"))]
    pub(super) shape_ctx: neoism_backend::sugarloaf::swash::shape::ShapeContext,
    #[cfg(not(target_os = "macos"))]
    pub(super) scale_ctx: neoism_backend::sugarloaf::swash::scale::ScaleContext,
    #[cfg(not(target_os = "macos"))]
    pub(super) font_data_cache: FxHashMap<
        u32,
        (
            neoism_backend::sugarloaf::font::SharedData,
            u32,
            neoism_backend::sugarloaf::swash::CacheKey,
        ),
    >,
}

impl Default for GridGlyphRasterizer {
    fn default() -> Self {
        Self::new()
    }
}

impl GridGlyphRasterizer {
    pub fn new() -> Self {
        Self {
            font_resolve: FxHashMap::default(),
            vertical_metrics_cache: FxHashMap::default(),
            synthesis_cache: FxHashMap::default(),
            run_cache: (0..RUN_BUCKET_COUNT)
                .map(|_| Vec::with_capacity(RUN_BUCKET_SIZE))
                .collect(),
            #[cfg(target_os = "macos")]
            run_utf16_scratch: Vec::new(),
            #[cfg(target_os = "macos")]
            run_cell_starts: Vec::new(),
            #[cfg(not(target_os = "macos"))]
            run_str_scratch: String::new(),
            #[cfg(target_os = "macos")]
            handle_cache: FxHashMap::default(),
            #[cfg(not(target_os = "macos"))]
            shape_ctx: neoism_backend::sugarloaf::swash::shape::ShapeContext::new(),
            #[cfg(not(target_os = "macos"))]
            scale_ctx: neoism_backend::sugarloaf::swash::scale::ScaleContext::new(),
            #[cfg(not(target_os = "macos"))]
            font_data_cache: FxHashMap::default(),
        }
    }

    #[inline]
    pub(super) fn resolve_font(
        &mut self,
        ch: char,
        style_flags: u8,
        font_library: &FontLibrary,
    ) -> (u32, bool) {
        // ASCII printable + regular style → always primary font, never
        // emoji. Skips the FxHashMap lookup that dominates this fn's
        // cost on terminal-typical content.
        // `font/Group.zig` indexForCodepoint ASCII fast path.
        //
        // Bold / italic ASCII still goes through the cache because
        // the bold and italic font IDs are dynamic (depend on which
        // faces the user loaded), and non-ASCII can hit fallback.
        if style_flags == 0 && (' '..='~').contains(&ch) {
            return (
                neoism_backend::sugarloaf::font::FONT_ID_REGULAR as u32,
                false,
            );
        }

        *self
            .font_resolve
            .entry((ch, style_flags))
            .or_insert_with(|| {
                let span_style = span_style_for_flags(style_flags);
                let (id, emoji) = font_library.resolve_font_for_char(ch, &span_style);
                (id as u32, emoji)
            })
    }

    #[inline]
    pub(super) fn get_synthesis(
        &mut self,
        font_id: u32,
        font_library: &FontLibrary,
    ) -> (bool, bool) {
        *self.synthesis_cache.entry(font_id).or_insert_with(|| {
            let lib = font_library.inner.read();
            let fd = lib.get(&(font_id as usize));
            (fd.should_embolden, fd.should_italicize)
        })
    }
}

#[inline]
fn span_style_for_flags(style_flags: u8) -> neoism_backend::sugarloaf::SpanStyle {
    use neoism_backend::sugarloaf::{Attributes, Stretch, Style as FontStyle, Weight};
    let mut s = neoism_backend::sugarloaf::SpanStyle::default();
    let bold = (style_flags & StyleFlags::BOLD.bits() as u8) != 0;
    let italic = (style_flags & StyleFlags::ITALIC.bits() as u8) != 0;
    let weight = if bold { Weight::BOLD } else { Weight::NORMAL };
    let fstyle = if italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    s.font_attrs = Attributes::new(Stretch::NORMAL, weight, fstyle);
    s
}

/// Rapidhash-based run key. Rapidhash is the official successor to
/// wyhash (choice) — same
/// quality, passes SMHasher, near-ideal collision probability. We use
/// the streaming `Hasher` API so we don't have to glue the inputs
/// into a single byte slice.
#[inline]
pub(super) fn run_hash(
    font_id: u32,
    size_bucket: u16,
    style_flags: u8,
    run_bytes: &[u8],
) -> u64 {
    use core::hash::Hasher;
    // `fast` flavour = the standard rapidhash algorithm tuned for
    // throughput. Quality is still SMHasher-passing (near-ideal
    // collision rate). `quality` is overkill for in-memory cache
    // keys where we don't need DoS resistance.
    let mut h = rapidhash::fast::RapidHasher::default();
    h.write_u32(font_id);
    h.write_u16(size_bucket);
    h.write_u8(style_flags);
    h.write(run_bytes);
    h.finish()
}

// Force inline — called once per cell during run extension on the hot
// path; body is two field reads + two compares so a real call is pure
// overhead.
#[inline(always)]
pub(super) fn is_run_breaker(sq: Square) -> bool {
    is_terminal_run_breaker(sq.is_bg_only(), sq.c())
}

/// Lookup. Hash → bucket; scan from most-recent; rotate on hit. No
/// secondary comparison — we trust the 64-bit rapidhash to be
/// collision-free across realistic workloads. Matches
///.
pub(super) fn run_cache_get(
    buckets: &mut [Vec<RunCacheEntry>],
    hash: u64,
) -> Option<&[ShapedGlyph]> {
    let idx = (hash as usize) & (RUN_BUCKET_COUNT - 1);
    let bucket = &mut buckets[idx];
    let last = bucket.len().checked_sub(1)?;
    for i in (0..bucket.len()).rev() {
        if bucket[i].hash == hash {
            if i != last {
                bucket[i..=last].rotate_left(1);
            }
            return Some(&bucket[last].glyphs);
        }
    }
    None
}

/// Insert. Bucket full → evict oldest (front).
pub(super) fn run_cache_put(buckets: &mut [Vec<RunCacheEntry>], entry: RunCacheEntry) {
    let idx = (entry.hash as usize) & (RUN_BUCKET_COUNT - 1);
    let bucket = &mut buckets[idx];
    if bucket.len() >= RUN_BUCKET_SIZE {
        bucket.remove(0);
    }
    bucket.push(entry);
}

fn shared_grid_vertical_metrics(
    font_library: &FontLibrary,
    font_id: u32,
    size_px: u16,
) -> GridVerticalMetrics {
    font_library
        .inner
        .write()
        .get_font_metrics(&(font_id as usize), f32::from(size_px))
        .map(|(ascent, descent, leading)| GridVerticalMetrics {
            baseline_px: (ascent + leading.max(0.0) * 0.5)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            natural_cell_height_px: (ascent + descent.abs() + leading.max(0.0))
                .round()
                .clamp(1.0, u16::MAX as f32) as u16,
        })
        .unwrap_or_else(|| GridVerticalMetrics::fallback(size_px))
}

// Platform-specific shape + vertical-metrics helpers

/// Shape a single run on macOS via CoreText and populate shared primary-font
/// vertical metrics as a side effect via the rasterizer's cache.
/// Returns the glyph list if the handle is available.
#[cfg(target_os = "macos")]
pub(super) fn shape_run_ct(
    rasterizer: &mut GridGlyphRasterizer,
    font_id: u32,
    size_u16: u16,
    size_bucket: u16,
    font_library: &FontLibrary,
) -> Option<(Vec<ShapedGlyph>, GridVerticalMetrics)> {
    let handle = match rasterizer.handle_cache.entry(font_id) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut().clone(),
        std::collections::hash_map::Entry::Vacant(e) => {
            let h = font_library.ct_font(font_id as usize)?;
            e.insert(h.clone());
            h
        }
    };
    let vertical_metrics = *rasterizer
        .vertical_metrics_cache
        .entry((font_id, size_bucket))
        .or_insert_with(|| shared_grid_vertical_metrics(font_library, font_id, size_u16));
    let ct_glyphs = neoism_backend::sugarloaf::font::macos::shape_text_utf16(
        &handle,
        &rasterizer.run_utf16_scratch,
        size_u16 as f32,
    );
    let glyphs: Vec<ShapedGlyph> = ct_glyphs
        .iter()
        .map(|g| ShapedGlyph {
            id: g.id,
            x: g.x,
            y: g.y,
            advance: g.advance,
            cluster: g.cluster,
        })
        .collect();
    Some((glyphs, vertical_metrics))
}

/// Shape a single run on non-macOS via swash. Populates shared vertical
/// metrics + `rasterizer.font_data_cache` as a side effect.
#[cfg(not(target_os = "macos"))]
pub(super) fn shape_run_swash(
    rasterizer: &mut GridGlyphRasterizer,
    font_id: u32,
    size_u16: u16,
    size_bucket: u16,
    font_library: &FontLibrary,
) -> Option<(Vec<ShapedGlyph>, GridVerticalMetrics)> {
    use neoism_backend::sugarloaf::swash::FontRef;

    let font_entry = rasterizer
        .font_data_cache
        .entry(font_id)
        .or_insert_with(|| {
            let lib = font_library.inner.read();
            lib.get_data(&(font_id as usize))
                .expect("font id resolved but get_data returned None")
        });
    let font_ref = FontRef {
        data: font_entry.0.as_ref(),
        offset: font_entry.1,
        key: font_entry.2,
    };

    let vertical_metrics = *rasterizer
        .vertical_metrics_cache
        .entry((font_id, size_bucket))
        .or_insert_with(|| shared_grid_vertical_metrics(font_library, font_id, size_u16));

    let mut shaper = rasterizer
        .shape_ctx
        .builder(font_ref)
        .size(size_u16 as f32)
        .build();
    shaper.add_str(&rasterizer.run_str_scratch);
    let mut glyphs: Vec<ShapedGlyph> = Vec::new();
    shaper.shape_with(|cluster| {
        let byte_offset = cluster.source.start;
        for g in cluster.glyphs {
            glyphs.push(ShapedGlyph {
                id: g.id,
                x: g.x,
                y: g.y,
                advance: g.advance,
                cluster: byte_offset,
            });
        }
    });
    Some((glyphs, vertical_metrics))
}

#[cfg(test)]
mod tests {
    use super::GridVerticalMetrics;

    #[test]
    fn configured_line_height_is_split_around_the_baseline() {
        let metrics = GridVerticalMetrics {
            baseline_px: 12,
            natural_cell_height_px: 16,
        };

        assert_eq!(metrics.baseline_for_cell_height(16.0), 12);
        assert_eq!(metrics.baseline_for_cell_height(20.0), 14);
        assert_eq!(metrics.baseline_for_cell_height(12.0), 10);
    }

    #[test]
    fn fallback_metrics_keep_a_stable_em_box() {
        let metrics = GridVerticalMetrics::fallback(20);
        assert_eq!(metrics.baseline_px, 16);
        assert_eq!(metrics.natural_cell_height_px, 20);
        assert_eq!(metrics.baseline_for_cell_height(24.0), 18);
    }
}
