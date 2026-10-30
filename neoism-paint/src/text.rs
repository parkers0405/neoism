// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! `text` — minimal immediate-mode text primitive for UI overlays.
//!
//! Text spans and layout used by tab titles, command palette, search input,
//! assistant, and other retained overlays.
//!
//! Public API (`draw` / `measure`) is identical across platforms.
//! On macOS the shape + rasterize backends are CoreText; everywhere
//! else they're swash (ShapeContext / ScaleContext + Render).
//! The host consumes the resulting CPU paint commands.

use rustc_hash::FxHashMap;
use std::sync::Arc;

mod cache;
#[cfg(test)]
mod cache_tests;
mod font_stamp;
#[cfg(test)]
mod invalidation_tests;
use cache::TextCache;
use font_stamp::FontStamp;

// Independent LRUs: shared allocations are conservatively charged to both.
const SHAPE_CACHE_ENTRIES: usize = 2048;
const SHAPE_CACHE_BYTES: usize = 8 * 1024 * 1024;
const FONT_LOOKUP_ENTRIES: usize = 16384;
type RunKey = (u32, u16, u16, u8);
type FullTextKey = (u32, u32, Option<usize>, bool, bool);

// Globally unique across renderer instances too, so a recreated renderer cannot
// accidentally validate measurement entries from its predecessor.
fn next_font_cache_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_update(
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
        |value| value.checked_add(1),
    )
    .expect("text cache generation space exhausted")
}

use crate::font::FontLibrary;

//  GPU vertex data (platform-agnostic)

/// Per-instance GPU vertex data for a UI text glyph.
///
/// `pos` is **pixel-space top-left** of the glyph's text bounding box.
/// `bearings.x` shifts it right to the glyph bitmap's left edge;
/// `bearings.y` shifts it down to the bitmap top. The vertex shader
/// writes: `out_px = pos + bearings + quad_corner * glyph_size`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TextInstance {
    pub pos: [f32; 2],
    pub glyph_pos: [u32; 2],
    pub glyph_size: [u32; 2],
    pub bearings: [i16; 2],
    pub color: [u8; 4],
    /// `0` = grayscale atlas; `1` = color atlas.
    pub atlas: u8,
    pub _pad: [u8; 3],
    /// `[x, y, width, height]` in physical pixels. `width <= 0` disables clipping.
    pub clip_rect: [f32; 4],
    /// Uniform geometry scale for canvas text; zero preserves normal UI text.
    pub raster_scale: f32,
    /// Streaming reveal blur radius in physical pixels; zero is the settled path.
    pub blur_radius: f32,
}

// 60 bytes (4-aligned); preserve raster_scale at 52 and blur_radius at 56.
const _: () = {
    assert!(std::mem::size_of::<TextInstance>() == 60);
    assert!(std::mem::offset_of!(TextInstance, raster_scale) == 52);
    assert!(std::mem::offset_of!(TextInstance, blur_radius) == 56);
};

mod canvas;

//  Public draw options

#[derive(Clone, Copy, Debug)]
pub struct DrawOpts {
    /// **Logical** (unscaled) font size. Text multiplies by its
    /// stored `scale_factor` internally before shaping / rasterizing.
    pub font_size: f32,
    /// Non-premultiplied RGBA. Shader premultiplies.
    pub color: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    /// Emit a two-step hard extrusion behind each monochrome glyph.
    pub extrude: bool,
    /// `None` → primary font.
    pub font_id: Option<usize>,
    /// Optional logical-pixel clip rect for this draw call. Converted to
    /// physical pixels per glyph so moving overlay text can be clipped by
    /// the GPU/CPU text pass instead of masked by follow-up rectangles.
    pub clip_rect: Option<[f32; 4]>,
}

impl Default for DrawOpts {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            color: [255, 255, 255, 255],
            bold: false,
            italic: false,
            extrude: false,
            font_id: None,
            clip_rect: None,
        }
    }
}

/// Per-cluster streaming effects. Ranges address UTF-8 bytes in the complete
/// input string, not characters or fallback-font substrings. Every range that
/// overlaps a cluster contributes; the smallest opacity (newest birth) wins.
/// Equal opacities select the larger blur radius. Unmatched clusters are settled.
/// Opacity is clamped to 0..=1. Radius is in logical pixels, bounded to a dense
/// four-physical-pixel kernel support on emission (plus bilinear footprint).
#[derive(Clone, Debug)]
pub struct TextReveal {
    pub range: std::ops::Range<usize>,
    pub opacity: f32,
    pub blur_radius: f32,
}

fn normalized_reveal(effect: &TextReveal, scale: f32) -> (f32, f32) {
    let opacity = if effect.opacity.is_finite() {
        effect.opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let radius = effect.blur_radius * scale;
    let radius = if radius.is_finite() {
        radius.clamp(0.0, 4.0)
    } else {
        0.0
    };
    // Canonical positive zero keeps nonnegative float bits numerically ordered.
    (
        if opacity == 0.0 { 0.0 } else { opacity },
        if radius == 0.0 { 0.0 } else { radius },
    )
}

type RevealEffects = FxHashMap<u32, (f32, f32)>;

// Birth ranges may arrive unsorted or overlap. Sweep source-ordered unique
// clusters with a minimum-opacity heap. Each birth enters/exits at most once;
// O((glyphs + births) log(glyphs + births)), not a per-glyph range scan. Repeated
// glyphs of a combining/ligature cluster share one computed effect. This work
// exists only on draw_revealed; normal draw keeps its specialized fast path.
fn prepare_reveal_effects(
    runs: &[ShapedRun],
    reveals: &[TextReveal],
    scale: f32,
) -> RevealEffects {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let mut clusters: Vec<_> = runs
        .iter()
        .flat_map(|run| run.source_glyphs())
        .map(|g| (g.cluster, g.cluster_end))
        .collect();
    clusters.sort_unstable();
    clusters.dedup();
    debug_assert!(clusters.windows(2).all(|pair| pair[0].1 <= pair[1].0));
    let mut births: Vec<_> = reveals
        .iter()
        .filter(|r| r.range.start < r.range.end)
        .collect();
    births.sort_unstable_by_key(|r| r.range.start);
    // Lower opacity first, then larger radius. Sanitized nonnegative float bits
    // sort numerically, avoiding partial-order/NaN comparisons in the heap.
    let mut active: BinaryHeap<Reverse<(u32, Reverse<u32>, usize)>> = BinaryHeap::new();
    let mut next = 0;
    let mut effects = RevealEffects::default();
    for (start, end) in clusters {
        while next < births.len() && births[next].range.start < end as usize {
            let birth = births[next];
            let (opacity, radius) = normalized_reveal(birth, scale);
            active.push(Reverse((
                opacity.to_bits(),
                Reverse(radius.to_bits()),
                birth.range.end,
            )));
            next += 1;
        }
        while active
            .peek()
            .is_some_and(|Reverse((_, _, end))| *end <= start as usize)
        {
            active.pop();
        }
        let effect = active
            .peek()
            .map(|Reverse((opacity, Reverse(radius), _))| {
                (f32::from_bits(*opacity), f32::from_bits(*radius))
            })
            .unwrap_or((1.0, 0.0));
        effects.insert(start, effect);
    }
    effects
}

// Simple linear reference for testing the sweep and half-open overlap policy.
#[cfg(test)]
fn reveal_effect(start: u32, end: u32, reveals: &[TextReveal], scale: f32) -> (f32, f32) {
    let mut selected = (1.0_f32, 0.0_f32);
    for effect in reveals {
        if effect.range.start >= effect.range.end
            || effect.range.start >= end as usize
            || effect.range.end <= start as usize
            || start >= end
        {
            continue;
        }
        let (opacity, radius) = normalized_reveal(effect, scale);
        if opacity < selected.0 || (opacity == selected.0 && radius > selected.1) {
            selected = (opacity, radius);
        }
    }
    selected
}

// Compute once on cache misses, independent of glyph order (RTL/reordering) or
// repeated starts (combining marks). Never scan the glyph list per emitted glyph.
// The last cluster ends at the shaped substring's actual source end, including
// trailing source bytes without their own glyph. Fallback substrings end at the
// next run's source start, or the complete input length for the final run.
fn set_cluster_ends(glyphs: &mut [ShapedGlyph], source_end: u32) {
    let mut starts: Vec<u32> = glyphs.iter().map(|g| g.cluster).collect();
    starts.sort_unstable();
    starts.dedup();
    for glyph in glyphs {
        let index = starts
            .binary_search(&glyph.cluster)
            .expect("collected cluster start");
        glyph.cluster_end = starts.get(index + 1).copied().unwrap_or(source_end);
    }
}

// Rebase the view, never the shared substring-local glyph allocation.
fn offset_run_clusters(mut run: ShapedRun, byte_start: usize) -> ShapedRun {
    run.source_offset = byte_start as u32;
    run
}

//  Shape result — unified across platforms

/// One shaped glyph in a run. Same shape on macOS (CoreText) and
/// non-macOS (swash) so the emit loop doesn't care which backend
/// produced it. Cached `cluster` offsets are local to the shaped substring;
/// `source_glyphs` applies a lightweight view's offset into the complete input.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct ShapedGlyph {
    id: u16,
    x: f32,
    y: f32,
    advance: f32,
    cluster: u32,
    /// Exclusive source end, always substring-local in immutable storage.
    cluster_end: u32,
}

/// A fully-shaped run with everything the emit step needs.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Some fields only read from one cfg path.
struct ShapedRun {
    font_id: u32,
    size_u16: u16,
    size_bucket: u16,
    synthetic_bold: bool,
    synthetic_italic: bool,
    /// Distance from the caller's nominal em-box top to the shared text
    /// baseline, in physical pixels.  This is derived from the primary
    /// font's line metrics for every resolved fallback run.
    baseline_px: i16,
    glyphs: Arc<[ShapedGlyph]>,
    source_offset: u32,
    advance: f32,
}

impl ShapedRun {
    /// Lightweight source-global view, including UTF-8 cluster ends. Copying one
    /// glyph here is stack-only; no glyph vector is cloned or mutated on hits.
    fn source_glyphs(&self) -> impl Iterator<Item = ShapedGlyph> + '_ {
        self.glyphs.iter().map(|glyph| ShapedGlyph {
            cluster: glyph.cluster + self.source_offset,
            cluster_end: glyph.cluster_end + self.source_offset,
            ..*glyph
        })
    }
}

/// Place a font's natural line box in the middle of the nominal `font_size`
/// box used by immediate-mode UI layout, returning top-to-baseline distance.
///
/// Font APIs report wildly different ascent/descent/leading totals for the
/// same requested point size.  UI call sites consistently treat `font_size`
/// as the height they centre inside a row, so using raw ascent as the draw
/// origin makes the visible text move when the family changes.  Splitting
/// both the font leading and any remaining em-box space evenly above/below
/// keeps that contract stable while preserving the designer's baseline.
fn centered_line_box_baseline_px(
    font_size_px: f32,
    ascent_px: f32,
    descent_px: f32,
    leading_px: f32,
) -> i16 {
    let font_size_px = font_size_px.max(1.0);
    if !ascent_px.is_finite()
        || !descent_px.is_finite()
        || !leading_px.is_finite()
        || ascent_px <= 0.0
        || descent_px < 0.0
    {
        return (font_size_px * 0.8)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }

    let leading_px = leading_px.max(0.0);
    let natural_line_height = ascent_px + descent_px + leading_px;
    let line_top = (font_size_px - natural_line_height) * 0.5;
    let baseline = line_top + leading_px * 0.5 + ascent_px;
    baseline.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

fn instances_ink_bounds_px(instances: &[TextInstance]) -> Option<[f32; 4]> {
    let mut bounds: Option<[f32; 4]> = None;
    for instance in instances
        .iter()
        .filter(|instance| instance.glyph_size[0] > 0 && instance.glyph_size[1] > 0)
    {
        let scale = if instance.raster_scale > 0.0 {
            instance.raster_scale
        } else {
            1.0
        };
        let left = instance.pos[0] + f32::from(instance.bearings[0]) * scale;
        let top = instance.pos[1] + f32::from(instance.bearings[1]) * scale;
        let right = left + instance.glyph_size[0] as f32 * scale;
        let bottom = top + instance.glyph_size[1] as f32 * scale;
        bounds = Some(match bounds {
            Some([x0, y0, x1, y1]) => {
                [x0.min(left), y0.min(top), x1.max(right), y1.max(bottom)]
            }
            None => [left, top, right, bottom],
        });
    }
    bounds
}

//  Per-OS GPU state

#[cfg(not(target_arch = "wasm32"))]
struct TextCpuState {
    atlas_grayscale: crate::glyph::cpu::CpuGridAtlas,
    atlas_color: crate::glyph::cpu::CpuGridAtlas,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PrefixMeasureKey {
    text: String,
    font_size: u32,
    scale: u32,
    font_id: Option<usize>,
    bold: bool,
    italic: bool,
}

// Bound both entry count and line length (at most ~2.5 MiB of text/stops).
const PREFIX_MEASURE_CACHE_ENTRIES: usize = 512;
const PREFIX_MEASURE_MAX_BYTES: usize = 1024;

#[derive(Clone, Debug)]
pub struct HostTextRaster {
    pub left: f32,
    pub top: f32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Text {
    /// Per-frame GPU instances, assembled inside `draw()` and drawn
    /// by the render-pass hook.
    instances: Vec<TextInstance>,

    /// Scale factor used to convert caller-supplied logical coords /
    /// font sizes to device pixels. Updated by `Sugarloaf::new` /
    /// `rescale`; defaults to 1.0.
    scale_factor: f32,

    // shared state across both OS paths
    font_library: FontLibrary,

    /// `(char, style_flags) → (font_id, is_emoji)` — first-char font
    /// resolution for a run.
    font_resolve: FxHashMap<(char, u8), (u32, bool)>,

    /// `(preferred_font_id, char) → covered`. Explicit UI fonts previously
    /// re-locked and reparsed the face for every character on every frame.
    font_coverage: FxHashMap<(u32, char), bool>,

    /// `font_id → (should_embolden, should_italicize)` from
    /// `FontData` load-time synthesis flags (parallel to the rich-text
    /// rasterizer's use of the same fields).
    synthesis_cache: FxHashMap<u32, (bool, bool)>,

    /// `(font_id, size_bucket) → baseline_px`. Secondary/fallback fonts use
    /// the primary face's cell metrics, keeping mixed icon/text runs on one
    /// baseline while each nominal font-size box remains vertically centred.
    baseline_cache: lru::LruCache<(u32, u16), i16>,

    /// Position-independent shape cache. Hash of
    /// `(font_id, size_bucket, style_flags, text)` → shaped run.
    shape_cache: TextCache<RunKey, ShapedRun>,
    /// Whole input cache bypasses character resolution and segmentation on hits.
    full_text_cache: TextCache<FullTextKey, Arc<[ShapedRun]>>,
    font_cache_generation: u64,
    font_stamp: FontStamp,
    #[cfg(test)]
    segmentation_chars: usize,
    #[cfg(test)]
    shape_misses: usize,

    /// Markdown caret/hit-test geometry. Unlike the shape cache this avoids
    /// walking, hashing and cloning every successive prefix on each redraw.
    prefix_measure_cache: lru::LruCache<PrefixMeasureKey, Vec<f32>>,

    #[cfg(target_os = "macos")]
    handle_cache: FxHashMap<u32, crate::font::macos::FontHandle>,

    #[cfg(not(target_os = "macos"))]
    shape_ctx: swash::shape::ShapeContext,
    #[cfg(not(target_os = "macos"))]
    scale_ctx: swash::scale::ScaleContext,
    /// Cached `(shared_data, offset, cache_key)` per font_id so the
    /// `FontLibraryData` read-lock isn't re-acquired per shape.
    #[cfg(not(target_os = "macos"))]
    font_data_cache: FxHashMap<u32, (crate::font::SharedData, u32, swash::CacheKey)>,
    /// Software backend. Initialised by `init_cpu` when the
    /// surrounding sugarloaf context is `ContextType::Cpu`.
    #[cfg(not(target_arch = "wasm32"))]
    cpu: Option<TextCpuState>,
}

impl Text {
    pub fn new(font_library: &FontLibrary) -> Self {
        Self {
            instances: Vec::new(),
            scale_factor: 1.0,
            font_library: font_library.clone(),
            font_resolve: FxHashMap::default(),
            font_coverage: FxHashMap::default(),
            synthesis_cache: FxHashMap::default(),
            baseline_cache: lru::LruCache::new(
                std::num::NonZeroUsize::new(SHAPE_CACHE_ENTRIES).unwrap(),
            ),
            shape_cache: TextCache::new(SHAPE_CACHE_ENTRIES, SHAPE_CACHE_BYTES),
            full_text_cache: TextCache::new(SHAPE_CACHE_ENTRIES, SHAPE_CACHE_BYTES),
            font_cache_generation: next_font_cache_generation(),
            font_stamp: FontStamp::new(font_library),
            #[cfg(test)]
            segmentation_chars: 0,
            #[cfg(test)]
            shape_misses: 0,
            prefix_measure_cache: lru::LruCache::new(
                std::num::NonZeroUsize::new(PREFIX_MEASURE_CACHE_ENTRIES).unwrap(),
            ),
            #[cfg(target_os = "macos")]
            handle_cache: FxHashMap::default(),
            #[cfg(not(target_os = "macos"))]
            shape_ctx: swash::shape::ShapeContext::new(),
            #[cfg(not(target_os = "macos"))]
            scale_ctx: swash::scale::ScaleContext::new(),
            #[cfg(not(target_os = "macos"))]
            font_data_cache: FxHashMap::default(),
            #[cfg(not(target_arch = "wasm32"))]
            cpu: None,
        }
    }

    /// Idempotent CPU-state init. Call once before the first
    /// `draw()` on the CPU backend; subsequent calls are no-ops.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn init_cpu(&mut self) {
        if self.cpu.is_some() {
            return;
        }
        self.cpu = Some(TextCpuState {
            atlas_grayscale: crate::glyph::cpu::CpuGridAtlas::new_grayscale(),
            atlas_color: crate::glyph::cpu::CpuGridAtlas::new_color(),
        });
    }

    /// Update the scale factor used to convert caller-supplied
    /// logical coords / font sizes to device pixels.
    #[inline]
    pub fn set_scale_factor(&mut self, scale: f32) {
        let scale = scale.max(1.0);
        if self.scale_factor.to_bits() != scale.to_bits() {
            self.scale_factor = scale;
            self.font_cache_generation = next_font_cache_generation();
        }
    }

    #[inline]
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.instances.clear();
    }

    /// Opaque, process-unique identity for external text measurement caches.
    /// Changes on renderer creation, DPI changes, explicit invalidation/library
    /// replacement (even the same Arc), and observed font metadata mutations.
    ///
    /// O(faces + symbol ranges), allocation-free on unchanged libraries. Takes
    /// one read lock, but never scans text/font bytes. Public library maps have
    /// no mutation epoch; comparing their immutable metadata is required here,
    /// BEFORE a caller may reuse a width without calling measure/draw.
    pub fn font_cache_generation(&mut self) -> u64 {
        self.observe_font_changes();
        self.font_cache_generation
    }

    fn observe_font_changes(&mut self) {
        match self.font_stamp.change(&self.font_library) {
            font_stamp::FontChange::Unchanged => {}
            font_stamp::FontChange::Replaced => self.clear_glyph_cache(),
            font_stamp::FontChange::Appended => {
                // Lazy fallback discovery can append faces mid-frame. Existing
                // atlas slots and emitted instances remain valid; do not erase
                // earlier labels while invalidating fallback/measurement data.
                self.font_cache_generation = next_font_cache_generation();
                self.shape_cache.clear();
                self.full_text_cache.clear();
                self.prefix_measure_cache.clear();
                self.font_resolve.clear();
                self.font_coverage.clear();
                self.baseline_cache.clear();
                self.font_stamp = FontStamp::new(&self.font_library);
            }
        }
    }

    /// Drop font-derived layout/raster state and recycle the UI glyph atlases.
    /// Font zoom can visit many size buckets and exhaust finite atlas space;
    /// immediate-mode UI text repopulates the atlas on the next frame.
    pub fn clear_glyph_cache(&mut self) {
        self.font_cache_generation = next_font_cache_generation();
        self.instances.clear();
        self.shape_cache.clear();
        self.full_text_cache.clear();
        self.font_coverage.clear();
        self.font_resolve.clear();
        self.synthesis_cache.clear();
        #[cfg(target_os = "macos")]
        self.handle_cache.clear();
        #[cfg(not(target_os = "macos"))]
        self.font_data_cache.clear();
        #[cfg(not(target_os = "macos"))]
        {
            // Swash caches metadata by the public FontRef key. Replacement
            // bytes can reuse that key; outer-cache eviction is not enough.
            self.shape_ctx = swash::shape::ShapeContext::new();
            self.scale_ctx = swash::scale::ScaleContext::new();
        }
        self.font_stamp = FontStamp::new(&self.font_library);
        self.prefix_measure_cache.clear();
        self.baseline_cache.clear();
        if let Some(state) = self.cpu.as_mut() {
            state.atlas_grayscale.clear();
            state.atlas_color.clear();
        }
    }

    #[inline]
    pub(crate) fn occlude_since(&mut self, first: usize, holes: &[[f32; 4]]) {
        let holes = crate::occlusion::Occlusions::new(holes);
        crate::occlusion::filter_tail(&mut self.instances, first, |instance| {
            crate::occlusion::glyph_parts_with(instance, &holes)
        });
    }

    #[inline]
    pub fn instances(&self) -> &[TextInstance] {
        &self.instances
    }

    /// Replace the font registry used by immediate-mode UI text.
    ///
    /// A config reload constructs a new `FontLibrary` (and therefore a new
    /// font-id namespace).  Keeping any resolver, shape, raster, or native
    /// handle cache from the previous library can mix old glyphs with new
    /// layout metrics, so a library swap invalidates all font-derived state.
    pub fn set_font_library(&mut self, font_library: &FontLibrary) {
        self.font_library = font_library.clone();
        self.clear_glyph_cache();
    }

    /// Top-to-baseline distance for the nominal font-size box described by
    /// `opts`, returned in logical pixels.  Panels that intentionally mix
    /// different font sizes on one baseline (drop caps, badges) can use this
    /// instead of a font-family-specific ratio.
    pub fn baseline_offset(&mut self, opts: &DrawOpts) -> f32 {
        self.observe_font_changes();
        let scaled = opts.font_size * self.scale_factor;
        let size_bucket = (scaled * 4.0).round().clamp(0.0, u16::MAX as f32) as u16;
        let size_u16 = scaled.round().clamp(1.0, u16::MAX as f32) as u16;
        let font_id = opts.font_id.unwrap_or(0) as u32;
        f32::from(self.baseline_px_for(font_id, size_bucket, size_u16))
            / self.scale_factor
    }

    /// Reposition glyphs emitted since `first_instance` so their combined
    /// rasterized ink is centred inside a logical-pixel rectangle.
    ///
    /// Text labels should normally follow the shared baseline. Bitmap-like
    /// icon glyphs inside colored chips instead need optical box centering:
    /// their font advance and em box often contain asymmetric padding. Each
    /// axis can be enabled independently so inline icons may keep their x
    /// advance while centering only vertically.
    pub fn center_instances_in_rect(
        &mut self,
        first_instance: usize,
        rect: [f32; 4],
        center_x: bool,
        center_y: bool,
    ) -> bool {
        let Some(instances) = self.instances.get(first_instance..) else {
            return false;
        };
        let Some([ink_left, ink_top, ink_right, ink_bottom]) =
            instances_ink_bounds_px(instances)
        else {
            return false;
        };

        let target_center_x = (rect[0] + rect[2] * 0.5) * self.scale_factor;
        let target_center_y = (rect[1] + rect[3] * 0.5) * self.scale_factor;
        let dx = if center_x {
            (target_center_x - (ink_left + ink_right) * 0.5).round()
        } else {
            0.0
        };
        let dy = if center_y {
            (target_center_y - (ink_top + ink_bottom) * 0.5).round()
        } else {
            0.0
        };

        for instance in &mut self.instances[first_instance..] {
            instance.pos[0] += dx;
            instance.pos[1] += dy;
        }
        true
    }

    //  Public draw API

    /// Draw `text` at logical top-left `(x, y)` with `opts`. Returns
    /// rendered width in **logical** pixels.
    /// Draw canvas text from a bounded raster size, scaling glyph geometry and
    /// advances together. Normal UI text remains on the unscaled draw path.
    pub fn draw_scaled(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        opts: &DrawOpts,
        scale: f32,
    ) -> f32 {
        if !scale.is_finite() || scale <= 0.0 || text.is_empty() {
            return 0.0;
        }
        // Complete shaping (including pending destructive invalidation) before
        // taking the emission boundary. Do not call draw() here: its second
        // observer could see another shared-library mutation and clear this
        // boundary's instances. Emission itself does not observe or clear.
        let Some(shaped) = self.shape_for(text, opts) else {
            return 0.0;
        };
        let first = self.instances.len();
        let mut raster_opts = *opts;
        raster_opts.clip_rect = None;
        let advance = self.draw_shaped(x, y, &shaped, &raster_opts);
        let origin = [x * self.scale_factor, y * self.scale_factor];
        let clip = opts
            .clip_rect
            .map(|rect| rect.map(|value| value * self.scale_factor))
            .unwrap_or([0.0; 4]);
        for glyph in &mut self.instances[first..] {
            glyph.pos[0] = origin[0] + (glyph.pos[0] - origin[0]) * scale;
            glyph.pos[1] = origin[1] + (glyph.pos[1] - origin[1]) * scale;
            glyph.raster_scale = scale;
            glyph.clip_rect = clip;
        }
        advance * scale
    }

    pub fn draw(&mut self, x: f32, y: f32, text: &str, opts: &DrawOpts) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let Some(shaped) = self.shape_for(text, opts) else {
            return 0.0;
        };
        self.draw_shaped(x, y, &shaped, opts)
    }

    // No font-change observation below the caller's instance boundary.
    fn draw_shaped(
        &mut self,
        x: f32,
        y: f32,
        shaped: &[ShapedRun],
        opts: &DrawOpts,
    ) -> f32 {
        let width_px = shaped_text_width(shaped);
        let mut run_x = x;
        for run in shaped.iter() {
            self.emit_instances::<false>(run_x, y, run, opts, None);
            run_x += shaped_width(run) / self.scale_factor;
        }
        width_px / self.scale_factor
    }

    /// Shape the complete text exactly as `draw`, then apply supplied streaming
    /// effects per source cluster. Returns the unchanged logical advance.
    /// CPU rendering keeps text sharp but honors reveal opacity.
    pub fn draw_revealed(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        opts: &DrawOpts,
        reveals: &[TextReveal],
    ) -> f32 {
        if reveals.is_empty() {
            return self.draw(x, y, text, opts);
        }
        if text.is_empty() {
            return 0.0;
        }
        let Some(shaped) = self.shape_for(text, opts) else {
            return 0.0;
        };
        let width_px = shaped_text_width(&shaped);
        let effects = prepare_reveal_effects(&shaped, reveals, self.scale_factor);
        let mut run_x = x;
        for run in shaped.iter() {
            self.emit_instances::<true>(run_x, y, run, opts, Some(&effects));
            run_x += shaped_width(run) / self.scale_factor;
        }
        width_px / self.scale_factor
    }

    /// Measure `text` under `opts` without recording a draw. Returns
    /// logical-pixel width.
    pub fn measure(&mut self, text: &str, opts: &DrawOpts) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        self.shape_for(text, opts)
            .map(|runs| shaped_text_width(&runs) / self.scale_factor)
            .unwrap_or(0.0)
    }

    /// Exact logical advances for every Unicode-scalar prefix, including the
    /// empty prefix. Markdown uses these for source-accurate hit testing.
    /// Keep the existing prefix-shaping semantics: summing isolated character
    /// widths would change kerning, ligatures and fallback-font geometry.
    pub fn measure_char_prefixes(&mut self, text: &str, opts: &DrawOpts) -> Vec<f32> {
        self.observe_font_changes();
        let key = (text.len() <= PREFIX_MEASURE_MAX_BYTES).then(|| PrefixMeasureKey {
            text: text.to_owned(),
            font_size: opts.font_size.to_bits(),
            scale: self.scale_factor.to_bits(),
            font_id: opts.font_id,
            bold: opts.bold,
            italic: opts.italic,
        });
        if let Some(stops) = key
            .as_ref()
            .and_then(|key| self.prefix_measure_cache.get(key))
        {
            return stops.clone();
        }
        let mut stops = Vec::with_capacity(text.chars().count() + 1);
        stops.push(0.0);
        for (offset, ch) in text.char_indices() {
            stops.push(self.measure(&text[..offset + ch.len_utf8()], opts));
        }
        if let Some(key) = key {
            self.prefix_measure_cache.put(key, stops.clone());
        }
        stops
    }

    //  Shape pipeline — shared cache + cfg-gated backend call

    fn shape_for(&mut self, text: &str, opts: &DrawOpts) -> Option<Arc<[ShapedRun]>> {
        self.observe_font_changes();
        let key = (
            opts.font_size.to_bits(),
            self.scale_factor.to_bits(),
            opts.font_id,
            opts.bold,
            opts.italic,
        );
        if let Some(runs) = self.full_text_cache.get(&key, text) {
            return Some(Arc::clone(runs));
        }
        let runs: Arc<[ShapedRun]> = self.segment_and_shape(text, opts)?.into();
        let bytes = runs.len() * std::mem::size_of::<ShapedRun>()
            + runs
                .iter()
                .map(|run| std::mem::size_of_val(run.glyphs.as_ref()))
                .sum::<usize>();
        self.full_text_cache
            .insert(key, text, Arc::clone(&runs), bytes);
        Some(runs)
    }

    fn segment_and_shape(
        &mut self,
        text: &str,
        opts: &DrawOpts,
    ) -> Option<Vec<ShapedRun>> {
        use crate::{Attributes, SpanStyle, Stretch, Style as FontStyle, Weight};

        let scaled = opts.font_size * self.scale_factor;
        let size_bucket = (scaled * 4.0).round().clamp(0.0, u16::MAX as f32) as u16;
        let size_u16 = scaled.round().clamp(1.0, u16::MAX as f32) as u16;
        let style_flags =
            (if opts.bold { 1u8 } else { 0 }) | (if opts.italic { 2u8 } else { 0 });

        let mut ss = SpanStyle::default();
        let weight = if opts.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        };
        let fstyle = if opts.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        ss.font_attrs = Attributes::new(Stretch::NORMAL, weight, fstyle);

        if let Some(preferred) = opts.font_id.map(|id| id as u32) {
            return self.shape_with_preferred_font(
                text,
                preferred,
                size_bucket,
                size_u16,
                style_flags,
                &ss,
            );
        }

        let mut runs = Vec::new();
        let mut run_start = 0usize;
        let mut current_font_id: Option<u32> = None;

        for (byte_ix, ch) in text.char_indices() {
            #[cfg(test)]
            {
                self.segmentation_chars += 1;
            }
            let font_id = self.resolve_font_id_for_char(ch, style_flags, &ss);
            match current_font_id {
                None => current_font_id = Some(font_id),
                Some(current) if current != font_id => {
                    if byte_ix > run_start {
                        let run = self.shape_run_for(
                            &text[run_start..byte_ix],
                            current,
                            size_bucket,
                            size_u16,
                            style_flags,
                        )?;
                        runs.push(offset_run_clusters(run, run_start));
                    }
                    run_start = byte_ix;
                    current_font_id = Some(font_id);
                }
                Some(_) => {}
            }
        }

        let font_id = current_font_id?;
        if run_start < text.len() {
            let run = self.shape_run_for(
                &text[run_start..],
                font_id,
                size_bucket,
                size_u16,
                style_flags,
            )?;
            runs.push(offset_run_clusters(run, run_start));
        }

        Some(runs)
    }

    fn resolve_font_id_for_char(
        &mut self,
        ch: char,
        style_flags: u8,
        style: &crate::SpanStyle,
    ) -> u32 {
        if self.font_resolve.len() >= FONT_LOOKUP_ENTRIES {
            return self
                .font_resolve
                .get(&(ch, style_flags))
                .map(|v| v.0)
                .unwrap_or_else(|| {
                    self.font_library.resolve_font_for_char(ch, style).0 as u32
                });
        }
        let (font_id, _is_emoji) = match self.font_resolve.entry((ch, style_flags)) {
            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let resolved = self.font_library.resolve_font_for_char(ch, style);
                let v = (resolved.0 as u32, resolved.1);
                e.insert(v);
                v
            }
        };
        font_id
    }

    fn font_covers_char(&mut self, font_id: u32, ch: char) -> bool {
        if let Some(covers) = self.font_coverage.get(&(font_id, ch)) {
            return *covers;
        }
        let covers = self
            .font_library
            .inner
            .read()
            .font_covers_char(font_id as usize, ch);
        if self.font_coverage.len() < FONT_LOOKUP_ENTRIES {
            self.font_coverage.insert((font_id, ch), covers);
        }
        covers
    }

    fn resolve_font_id_for_char_with_preferred(
        &mut self,
        ch: char,
        style_flags: u8,
        style: &crate::SpanStyle,
        preferred: u32,
    ) -> u32 {
        if self.font_covers_char(preferred, ch) {
            return preferred;
        }
        self.resolve_font_id_for_char(ch, style_flags, style)
    }

    fn shape_with_preferred_font(
        &mut self,
        text: &str,
        preferred: u32,
        size_bucket: u16,
        size_u16: u16,
        style_flags: u8,
        style: &crate::SpanStyle,
    ) -> Option<Vec<ShapedRun>> {
        let mut runs = Vec::new();
        let mut run_start = 0usize;
        let mut current_font_id: Option<u32> = None;

        for (byte_ix, ch) in text.char_indices() {
            #[cfg(test)]
            {
                self.segmentation_chars += 1;
            }
            let font_id = self.resolve_font_id_for_char_with_preferred(
                ch,
                style_flags,
                style,
                preferred,
            );
            match current_font_id {
                None => current_font_id = Some(font_id),
                Some(current) if current != font_id => {
                    if byte_ix > run_start {
                        let run = self.shape_run_for(
                            &text[run_start..byte_ix],
                            current,
                            size_bucket,
                            size_u16,
                            style_flags,
                        )?;
                        runs.push(offset_run_clusters(run, run_start));
                    }
                    run_start = byte_ix;
                    current_font_id = Some(font_id);
                }
                Some(_) => {}
            }
        }

        let font_id = current_font_id?;
        if run_start < text.len() {
            let run = self.shape_run_for(
                &text[run_start..],
                font_id,
                size_bucket,
                size_u16,
                style_flags,
            )?;
            runs.push(offset_run_clusters(run, run_start));
        }

        Some(runs)
    }

    fn shape_run_for(
        &mut self,
        text: &str,
        font_id: u32,
        size_bucket: u16,
        size_u16: u16,
        style_flags: u8,
    ) -> Option<ShapedRun> {
        let key = (font_id, size_bucket, size_u16, style_flags);
        if let Some(entry) = self.shape_cache.get(&key, text) {
            return Some(entry.clone());
        }
        #[cfg(test)]
        {
            self.shape_misses += 1;
        }

        let (synthetic_bold, synthetic_italic) = match self.synthesis_cache.entry(font_id)
        {
            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let lib = self.font_library.inner.read();
                let fd = lib.get(&(font_id as usize));
                *e.insert((fd.should_embolden, fd.should_italicize))
            }
        };

        #[cfg(target_os = "macos")]
        let glyphs = {
            let handle = match self.handle_cache.entry(font_id) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut().clone(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let h = self.font_library.ct_font(font_id as usize)?;
                    e.insert(h.clone());
                    h
                }
            };
            let ct_glyphs =
                crate::font::macos::shape_text(&handle, text, size_u16 as f32);
            let glyphs: Vec<ShapedGlyph> = ct_glyphs
                .iter()
                .map(|g| ShapedGlyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                    advance: g.advance,
                    cluster: g.cluster,
                    cluster_end: 0,
                })
                .collect();
            glyphs
        };

        #[cfg(not(target_os = "macos"))]
        let glyphs = {
            use swash::FontRef;

            // Pull (or cache) the font bytes + offset + key once per
            // font_id to avoid the RwLock read-lock per shape.
            let font_entry = self.font_data_cache.entry(font_id).or_insert_with(|| {
                let lib = self.font_library.inner.read();
                lib.get_data(&(font_id as usize)).expect(
                    "font id resolved but get_data returned None — cache invariant",
                )
            });
            let font_ref = FontRef {
                data: font_entry.0.as_ref(),
                offset: font_entry.1,
                key: font_entry.2,
            };

            // Shape with swash. Flatten clusters to a Vec<ShapedGlyph>
            // with UTF-8 byte offset as `cluster`.
            let mut shaper = self
                .shape_ctx
                .builder(font_ref)
                .size(size_u16 as f32)
                .build();
            shaper.add_str(text);
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
                        cluster_end: 0,
                    });
                }
            });
            glyphs
        };

        let mut glyphs = glyphs;
        set_cluster_ends(&mut glyphs, text.len() as u32);
        let baseline_px = self.baseline_px_for(font_id, size_bucket, size_u16);

        let run = ShapedRun {
            font_id,
            size_u16,
            size_bucket,
            synthetic_bold,
            synthetic_italic,
            baseline_px,
            advance: glyphs.iter().map(|g| g.advance).sum(),
            source_offset: 0,
            glyphs: glyphs.into(),
        };
        self.shape_cache.insert(
            key,
            text,
            run.clone(),
            std::mem::size_of_val(run.glyphs.as_ref()),
        );
        Some(run)
    }

    fn baseline_px_for(&mut self, font_id: u32, size_bucket: u16, size_u16: u16) -> i16 {
        if let Some(value) = self.baseline_cache.get(&(font_id, size_bucket)) {
            return *value;
        }
        let value = {
            let metrics = self
                .font_library
                .inner
                .write()
                .get_ui_font_metrics(&(font_id as usize), size_u16 as f32);
            metrics.map_or_else(
                || {
                    centered_line_box_baseline_px(
                        size_u16 as f32,
                        size_u16 as f32 * 0.8,
                        size_u16 as f32 * 0.2,
                        0.0,
                    )
                },
                |(ascent, descent, leading)| {
                    centered_line_box_baseline_px(
                        size_u16 as f32,
                        ascent,
                        descent.abs(),
                        leading,
                    )
                },
            )
        };
        self.baseline_cache.put((font_id, size_bucket), value);
        value
    }

    //  Emit pipeline — rasterize + push TextInstance

    fn emit_instances<const REVEALED: bool>(
        &mut self,
        x: f32,
        y: f32,
        run: &ShapedRun,
        opts: &DrawOpts,
        effects: Option<&RevealEffects>,
    ) {
        let scale = self.scale_factor;
        // UI text uses pre-rasterized glyph masks. Keep each text run on
        // a stable device-pixel origin so animated Rust overlays don't
        // re-phase glyph antialiasing while they scroll, but preserve the
        // shaper's intra-run advances/kerning instead of rounding every
        // glyph independently.
        let mut pen_x = snap_px(x * scale);
        let py = snap_px(y * scale);
        let color = opts.color;
        let clip_rect = opts
            .clip_rect
            .map(|rect| snap_clip_rect(rect, scale))
            .unwrap_or([0.0; 4]);
        // Zero dimensions are the GPU/CPU sentinel for *no* clip. A provided
        // subpixel clip can round to zero; emitting it would draw unbounded.
        if opts.clip_rect.is_some() && (clip_rect[2] <= 0.0 || clip_rect[3] <= 0.0) {
            return;
        }

        for glyph in run.source_glyphs() {
            let Some((slot_x, slot_y, slot_w, slot_h, bearing_x, bearing_y, is_color)) =
                self.rasterize_slot(run, glyph.id)
            else {
                continue;
            };
            if slot_w == 0 || slot_h == 0 {
                pen_x += glyph.advance;
                continue;
            }

            let atlas_tag = if is_color { 1u8 } else { 0u8 };
            let (opacity, blur_radius) = if REVEALED {
                effects.expect("revealed effects")[&glyph.cluster]
            } else {
                (1.0, 0.0)
            };
            let mut instance_color = if is_color {
                [255u8, 255, 255, 255]
            } else {
                color
            };

            if REVEALED && opacity != 1.0 {
                instance_color[3] =
                    (f32::from(instance_color[3]) * opacity).round() as u8;
            }
            let foreground = TextInstance {
                pos: [pen_x + glyph.x, py + glyph.y.max(0.0)],
                glyph_pos: [slot_x as u32, slot_y as u32],
                glyph_size: [slot_w as u32, slot_h as u32],
                bearings: [bearing_x, bearing_y],
                color: instance_color,
                atlas: atlas_tag,
                _pad: [0; 3],
                clip_rect,
                raster_scale: 0.0,
                blur_radius,
            };
            if opts.extrude && !is_color {
                let mut far = foreground;
                far.pos[0] += snap_px(3.0 * scale);
                far.pos[1] += snap_px(3.0 * scale);
                far.color = [10, 10, 14, instance_color[3].saturating_mul(5) / 6];
                self.instances.push(far);

                let mut near = foreground;
                near.pos[0] += snap_px(1.5 * scale);
                near.pos[1] += snap_px(1.5 * scale);
                near.color = [62, 62, 72, instance_color[3]];
                self.instances.push(near);
            }
            self.instances.push(foreground);

            pen_x += glyph.advance;
        }
    }

    /// Lookup or rasterize-and-insert a glyph. Returns
    /// `(x, y, w, h, bearing_x, bearing_y, is_color)` from the atlas
    /// slot. Per-OS rasterize path; shared slot shape.
    #[allow(clippy::type_complexity)]
    fn rasterize_slot(
        &mut self,
        run: &ShapedRun,
        glyph_id: u16,
    ) -> Option<(u16, u16, u16, u16, i16, i16, bool)> {
        let key = crate::glyph::GlyphKey {
            font_id: run.font_id,
            glyph_id: glyph_id as u32,
            size_bucket: run.size_bucket,
        };

        self.rasterize_slot_cpu(run, glyph_id, key)
    }

    /// Software-atlas variant of `rasterize_slot`. Same flow as the
    /// GPU branches: lookup, then OS-native (CoreText) or swash
    /// rasterize, then insert into the matching CPU atlas. Returns
    /// `None` when shaping caches haven't been primed for this
    /// font_id (shouldn't happen — `shape()` always runs first) or
    /// when the atlas is at `ATLAS_MAX_SIZE` and the glyph won't fit.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::type_complexity)]
    fn rasterize_slot_cpu(
        &mut self,
        run: &ShapedRun,
        glyph_id: u16,
        key: crate::glyph::GlyphKey,
    ) -> Option<(u16, u16, u16, u16, i16, i16, bool)> {
        // Lookup first — both atlases.
        {
            let state = self.cpu.as_ref()?;
            if let Some(s) = state.atlas_grayscale.lookup(key) {
                return Some((s.x, s.y, s.w, s.h, s.bearing_x, s.bearing_y, false));
            }
            if let Some(s) = state.atlas_color.lookup(key) {
                return Some((s.x, s.y, s.w, s.h, s.bearing_x, s.bearing_y, true));
            }
        }

        // Rasterize. macOS uses CoreText so emoji come out correctly;
        // every other target uses swash.
        #[cfg(target_os = "macos")]
        let (raw_w, raw_h, raw_left, raw_top, raw_is_color, raw_bytes) = {
            let handle = self.handle_cache.get(&run.font_id)?.clone();
            let raw = crate::font::macos::rasterize_glyph(
                &handle,
                glyph_id,
                run.size_u16 as f32,
                /* is_emoji: */ false,
                run.synthetic_italic,
                run.synthetic_bold,
            )?;
            (
                raw.width,
                raw.height,
                raw.left,
                raw.top,
                raw.is_color,
                raw.bytes,
            )
        };
        #[cfg(not(target_os = "macos"))]
        let (raw_w, raw_h, raw_left, raw_top, raw_is_color, raw_bytes) = {
            let font_entry = self.font_data_cache.get(&run.font_id)?.clone();
            let raw = rasterize_swash_glyph(
                &mut self.scale_ctx,
                &font_entry,
                glyph_id,
                run.size_u16 as f32,
                run.synthetic_bold,
                run.synthetic_italic,
                self.font_library.inner.read().hinting,
            )?;
            (
                raw.width,
                raw.height,
                raw.left,
                raw.top,
                raw.is_color,
                raw.bytes,
            )
        };

        let raster = crate::glyph::RasterizedGlyph {
            width: raw_w.min(u16::MAX as u32) as u16,
            height: raw_h.min(u16::MAX as u32) as u16,
            bearing_x: raw_left.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            bearing_y: {
                let top_i16 = raw_top.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                run.baseline_px.saturating_sub(top_i16)
            },
            bytes: &raw_bytes,
        };

        let state = self.cpu.as_mut()?;
        let slot = if raw_is_color {
            state.atlas_color.insert(key, raster).or_else(|| {
                if state.atlas_color.grow() {
                    state.atlas_color.insert(key, raster)
                } else {
                    None
                }
            })?
        } else {
            state.atlas_grayscale.insert(key, raster).or_else(|| {
                if state.atlas_grayscale.grow() {
                    state.atlas_grayscale.insert(key, raster)
                } else {
                    None
                }
            })?
        };
        Some((
            slot.x,
            slot.y,
            slot.w,
            slot.h,
            slot.bearing_x,
            slot.bearing_y,
            raw_is_color,
        ))
    }

    /// Paint the queued UI text instances into the caller-supplied
    /// `0x00RRGGBB` u32 buffer. Mirrors `text_vertex` /
    /// `grid_text_fragment`: glyph origin = `pos + bearings`; mask
    /// glyphs use `instance.color`, color glyphs sample directly.
    /// No-op when CPU state is absent or no instances were queued.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_cpu(&self, buf: &mut [u32], buf_w: u32, buf_h: u32) {
        self.render_cpu_instances(&self.instances, buf, buf_w, buf_h, blend_premul_over);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn render_cpu_instances(
        &self,
        instances: &[TextInstance],
        buf: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        blend: fn([u8; 4], u32) -> u32,
    ) {
        if instances.is_empty() {
            return;
        }
        let Some(state) = self.cpu.as_ref() else {
            return;
        };
        let buf_w_i = buf_w as i32;
        let buf_h_i = buf_h as i32;
        let mask = state.atlas_grayscale.pixels();
        let mask_side = state.atlas_grayscale.side() as usize;
        let color_atlas = state.atlas_color.pixels();
        let color_side = state.atlas_color.side() as usize;

        for inst in instances {
            if inst.raster_scale > 0.0 {
                let (atlas, side) = if inst.atlas == 1 {
                    (color_atlas, color_side)
                } else {
                    (mask, mask_side)
                };
                canvas::draw_scaled_cpu_with_blend(
                    inst, atlas, side, buf, buf_w_i, buf_h_i, blend,
                );
                continue;
            }
            let gw = inst.glyph_size[0] as i32;
            let gh = inst.glyph_size[1] as i32;
            if gw <= 0 || gh <= 0 {
                continue;
            }
            let glyph_x = (inst.pos[0] + inst.bearings[0] as f32) as i32;
            let glyph_y = (inst.pos[1] + inst.bearings[1] as f32) as i32;
            let ax = inst.glyph_pos[0] as usize;
            let ay = inst.glyph_pos[1] as usize;

            if inst.atlas == 1 {
                blit_text_color_with_blend(
                    buf,
                    buf_w_i,
                    buf_h_i,
                    glyph_x,
                    glyph_y,
                    gw,
                    gh,
                    color_atlas,
                    color_side,
                    ax,
                    ay,
                    inst.clip_rect,
                    inst.color[3],
                    blend,
                );
            } else {
                blit_text_mask_with_blend(
                    buf,
                    buf_w_i,
                    buf_h_i,
                    glyph_x,
                    glyph_y,
                    gw,
                    gh,
                    mask,
                    mask_side,
                    ax,
                    ay,
                    inst.color,
                    inst.clip_rect,
                    blend,
                );
            }
        }
    }

    /// Export one already-shaped glyph run as a transparent native texture.
    /// Uses the same cached glyph atlas, clipping-free geometry and CPU sampler
    /// as the existing renderer; callers apply the original draw clip on GPU.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn host_run_raster(&self, first: usize) -> Option<HostTextRaster> {
        let source = self.instances.get(first..)?;
        if source.is_empty() {
            return None;
        }
        let mut left = f32::MAX;
        let mut top = f32::MAX;
        let mut right = f32::MIN;
        let mut bottom = f32::MIN;
        for glyph in source {
            let scale = if glyph.raster_scale > 0.0 {
                glyph.raster_scale
            } else {
                1.0
            };
            let x = glyph.pos[0] + glyph.bearings[0] as f32 * scale;
            let y = glyph.pos[1] + glyph.bearings[1] as f32 * scale;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + glyph.glyph_size[0] as f32 * scale);
            bottom = bottom.max(y + glyph.glyph_size[1] as f32 * scale);
        }
        let left = left.floor();
        let top = top.floor();
        let width = (right.ceil() - left).max(0.0) as u32;
        let height = (bottom.ceil() - top).max(0.0) as u32;
        if width == 0 || height == 0 || width as u64 * height as u64 > 16 * 1024 * 1024 {
            return None;
        }
        let instances: Vec<_> = source
            .iter()
            .map(|glyph| {
                let mut g = *glyph;
                g.pos[0] -= left;
                g.pos[1] -= top;
                g.clip_rect = [0.0; 4];
                g
            })
            .collect();
        let mut pixels = vec![0u32; width as usize * height as usize];
        self.render_cpu_instances(
            &instances,
            &mut pixels,
            width,
            height,
            blend_premul_rgba_over,
        );
        let mut rgba = Vec::with_capacity(pixels.len() * 4);
        for pixel in pixels {
            let alpha = (pixel >> 24) as u8;
            let unpremul = |channel: u32| {
                if alpha == 0 {
                    0
                } else {
                    ((channel * 255 + alpha as u32 / 2) / alpha as u32).min(255) as u8
                }
            };
            rgba.extend_from_slice(&[
                unpremul((pixel >> 16) & 255),
                unpremul((pixel >> 8) & 255),
                unpremul(pixel & 255),
                alpha,
            ]);
        }
        Some(HostTextRaster {
            left,
            top,
            width,
            height,
            rgba,
        })
    }
}

//  Helpers

#[inline]
fn shaped_width(run: &ShapedRun) -> f32 {
    run.advance
}

#[inline]
fn shaped_text_width(runs: &[ShapedRun]) -> f32 {
    runs.iter().map(shaped_width).sum()
}

#[inline]
fn snap_px(value: f32) -> f32 {
    value.round()
}

#[inline]
fn snap_clip_rect([x, y, w, h]: [f32; 4], scale: f32) -> [f32; 4] {
    let x0 = snap_px(x * scale);
    let y0 = snap_px(y * scale);
    let x1 = snap_px((x + w) * scale);
    let y1 = snap_px((y + h) * scale);
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

//  Swash rasterize — non-macOS

#[cfg(not(target_os = "macos"))]
struct SwashRawGlyph {
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    is_color: bool,
    bytes: Vec<u8>,
}

#[cfg(not(target_os = "macos"))]
fn rasterize_swash_glyph(
    scale_ctx: &mut swash::scale::ScaleContext,
    font_entry: &(crate::font::SharedData, u32, swash::CacheKey),
    glyph_id: u16,
    size_px: f32,
    synthetic_bold: bool,
    synthetic_italic: bool,
    hint: bool,
) -> Option<SwashRawGlyph> {
    use swash::scale::{
        image::{Content, Image as GlyphImage},
        Render, Source, StrikeWith,
    };
    use swash::zeno::{Angle, Format, Transform};
    use swash::FontRef;

    let font_ref = FontRef {
        data: font_entry.0.as_ref(),
        offset: font_entry.1,
        key: font_entry.2,
    };

    let mut scaler = scale_ctx.builder(font_ref).hint(hint).size(size_px).build();

    let mut image = GlyphImage::new();
    let sources: &[Source] = &[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ];
    let rendered = Render::new(sources)
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

    if !rendered {
        return None;
    }

    let is_color = image.content == Content::Color;
    if is_color {
        crate::font::premultiply_color_glyph(&mut image.data);
    }
    Some(SwashRawGlyph {
        width: image.placement.width,
        height: image.placement.height,
        left: image.placement.left,
        top: image.placement.top,
        is_color,
        bytes: image.data,
    })
}

//  CPU blit helpers for `Text::render_cpu`. Same blend model as
//  `grid::cpu`: premultiplied source-over against an opaque
//  `0x00RRGGBB` destination.

#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn pack_opaque(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn blend_premul_rgba_over(src: [u8; 4], dst: u32) -> u32 {
    let sa = src[3] as u32;
    let inv = 255 - sa;
    let channel = |value: u8, shift: u32| {
        (value as u32 + (((dst >> shift) & 255) * inv + 127) / 255).min(255)
    };
    let alpha = sa + (((dst >> 24) & 255) * inv + 127) / 255;
    (alpha << 24)
        | (channel(src[0], 16) << 16)
        | (channel(src[1], 8) << 8)
        | channel(src[2], 0)
}

fn blend_premul_over(src: [u8; 4], dst: u32) -> u32 {
    let sa = src[3] as u32;
    if sa == 0 {
        return dst;
    }
    if sa == 255 {
        return pack_opaque(src[0], src[1], src[2]);
    }
    let inv = 255 - sa;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let or = src[0] as u32 + (dr * inv + 127) / 255;
    let og = src[1] as u32 + (dg * inv + 127) / 255;
    let ob = src[2] as u32 + (db * inv + 127) / 255;
    pack_opaque(or.min(255) as u8, og.min(255) as u8, ob.min(255) as u8)
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn blit_text_mask(
    buf: &mut [u32],
    buf_w: i32,
    buf_h: i32,
    glyph_x: i32,
    glyph_y: i32,
    gw: i32,
    gh: i32,
    atlas: &[u8],
    atlas_side: usize,
    ax: usize,
    ay: usize,
    color: [u8; 4],
    clip_rect: [f32; 4],
) {
    blit_text_mask_with_blend(
        buf,
        buf_w,
        buf_h,
        glyph_x,
        glyph_y,
        gw,
        gh,
        atlas,
        atlas_side,
        ax,
        ay,
        color,
        clip_rect,
        blend_premul_over,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn blit_text_mask_with_blend(
    buf: &mut [u32],
    buf_w: i32,
    buf_h: i32,
    glyph_x: i32,
    glyph_y: i32,
    gw: i32,
    gh: i32,
    atlas: &[u8],
    atlas_side: usize,
    ax: usize,
    ay: usize,
    color: [u8; 4],
    clip_rect: [f32; 4],
    blend: fn([u8; 4], u32) -> u32,
) {
    if color[3] == 0 {
        return;
    }
    let stride = buf_w as usize;
    let (clip_x0, clip_y0, clip_x1, clip_y1) = cpu_clip_bounds(clip_rect, buf_w, buf_h);
    let x_start = glyph_x.max(0).max(clip_x0);
    let y_start = glyph_y.max(0).max(clip_y0);
    let x_end = (glyph_x + gw).min(buf_w).min(clip_x1);
    let y_end = (glyph_y + gh).min(buf_h).min(clip_y1);
    if x_end <= x_start || y_end <= y_start {
        return;
    }
    let r = color[0] as u32;
    let g = color[1] as u32;
    let b = color[2] as u32;
    let ca = color[3] as u32;

    for dst_y in y_start..y_end {
        let src_y = (dst_y - glyph_y) as usize + ay;
        if src_y >= atlas_side {
            continue;
        }
        let atlas_row = src_y * atlas_side;
        let buf_row = (dst_y as usize) * stride;
        for dst_x in x_start..x_end {
            let src_x = (dst_x - glyph_x) as usize + ax;
            if src_x >= atlas_side {
                continue;
            }
            let m = atlas[atlas_row + src_x] as u32;
            if m == 0 {
                continue;
            }
            let a = (m * ca + 127) / 255;
            if a == 0 {
                continue;
            }
            let pr = (r * a + 127) / 255;
            let pg = (g * a + 127) / 255;
            let pb = (b * a + 127) / 255;
            let src = [pr as u8, pg as u8, pb as u8, a as u8];
            let idx = buf_row + (dst_x as usize);
            buf[idx] = blend(src, buf[idx]);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn blit_text_color(
    buf: &mut [u32],
    buf_w: i32,
    buf_h: i32,
    glyph_x: i32,
    glyph_y: i32,
    gw: i32,
    gh: i32,
    atlas: &[u8],
    atlas_side: usize,
    ax: usize,
    ay: usize,
    clip_rect: [f32; 4],
    opacity: u8,
) {
    blit_text_color_with_blend(
        buf,
        buf_w,
        buf_h,
        glyph_x,
        glyph_y,
        gw,
        gh,
        atlas,
        atlas_side,
        ax,
        ay,
        clip_rect,
        opacity,
        blend_premul_over,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn blit_text_color_with_blend(
    buf: &mut [u32],
    buf_w: i32,
    buf_h: i32,
    glyph_x: i32,
    glyph_y: i32,
    gw: i32,
    gh: i32,
    atlas: &[u8],
    atlas_side: usize,
    ax: usize,
    ay: usize,
    clip_rect: [f32; 4],
    opacity: u8,
    blend: fn([u8; 4], u32) -> u32,
) {
    let stride = buf_w as usize;
    let (clip_x0, clip_y0, clip_x1, clip_y1) = cpu_clip_bounds(clip_rect, buf_w, buf_h);
    let x_start = glyph_x.max(0).max(clip_x0);
    let y_start = glyph_y.max(0).max(clip_y0);
    let x_end = (glyph_x + gw).min(buf_w).min(clip_x1);
    let y_end = (glyph_y + gh).min(buf_h).min(clip_y1);
    if x_end <= x_start || y_end <= y_start {
        return;
    }
    for dst_y in y_start..y_end {
        let src_y = (dst_y - glyph_y) as usize + ay;
        if src_y >= atlas_side {
            continue;
        }
        let atlas_row = src_y * atlas_side * 4;
        let buf_row = (dst_y as usize) * stride;
        for dst_x in x_start..x_end {
            let src_x = (dst_x - glyph_x) as usize + ax;
            if src_x >= atlas_side {
                continue;
            }
            let off = atlas_row + src_x * 4;
            let r = atlas[off];
            let g = atlas[off + 1];
            let b = atlas[off + 2];
            let a = atlas[off + 3];
            if a == 0 {
                continue;
            }
            let src = if opacity == 255 {
                [r, g, b, a]
            } else {
                [r, g, b, a]
                    .map(|v| ((u32::from(v) * u32::from(opacity) + 127) / 255) as u8)
            };
            let idx = buf_row + (dst_x as usize);
            buf[idx] = blend(src, buf[idx]);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn cpu_clip_bounds(clip_rect: [f32; 4], buf_w: i32, buf_h: i32) -> (i32, i32, i32, i32) {
    if clip_rect[2] <= 0.0 || clip_rect[3] <= 0.0 {
        return (0, 0, buf_w, buf_h);
    }
    let x0 = clip_rect[0].floor().max(0.0).min(buf_w as f32) as i32;
    let y0 = clip_rect[1].floor().max(0.0).min(buf_h as f32) as i32;
    let x1 = (clip_rect[0] + clip_rect[2])
        .ceil()
        .max(0.0)
        .min(buf_w as f32) as i32;
    let y1 = (clip_rect[1] + clip_rect[3])
        .ceil()
        .max(0.0)
        .min(buf_h as f32) as i32;
    (x0, y0, x1, y1)
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    use super::blit_text_color;
    use super::{
        centered_line_box_baseline_px, instances_ink_bounds_px, prepare_reveal_effects,
        reveal_effect, set_cluster_ends, snap_clip_rect, DrawOpts, ShapedGlyph,
        ShapedRun, Text, TextInstance, TextReveal, PREFIX_MEASURE_CACHE_ENTRIES,
        PREFIX_MEASURE_MAX_BYTES,
    };
    use crate::font::{fonts::FontConfig, FontLibrary};
    use std::sync::Arc;

    #[test]
    fn subpixel_text_clip_snaps_to_empty_instead_of_becoming_unclipped() {
        for scale in [1.0, 1.25, 2.0] {
            let clip = snap_clip_rect([5.0, 20.0, 10.0, 0.1 / scale], scale);
            assert_eq!(clip[3], 0.0);
            assert!(snap_clip_rect([5.0, 20.0, 10.0, 1.0 / scale], scale)[3] > 0.0);
        }
    }

    #[test]
    fn centered_line_box_uses_font_metrics_without_changing_nominal_height() {
        // Both fonts have a 20px natural line box but put different amounts
        // above/below the baseline.  Their natural boxes are centred inside
        // the same 14px UI em box while preserving each baseline ratio.
        assert_eq!(centered_line_box_baseline_px(14.0, 14.0, 6.0, 0.0), 11);
        assert_eq!(centered_line_box_baseline_px(14.0, 12.0, 8.0, 0.0), 9);
    }

    #[test]
    fn centered_line_box_splits_font_leading_evenly() {
        assert_eq!(centered_line_box_baseline_px(16.0, 11.0, 3.0, 2.0), 12);
        assert_eq!(centered_line_box_baseline_px(16.0, 11.0, 3.0, 0.0), 12);
    }

    #[test]
    fn centered_line_box_has_a_safe_invalid_metrics_fallback() {
        assert_eq!(centered_line_box_baseline_px(20.0, f32::NAN, 4.0, 0.0), 16);
        assert_eq!(centered_line_box_baseline_px(20.0, 12.0, -1.0, 0.0), 16);
    }

    #[test]
    fn instance_ink_bounds_include_bearings_and_faux_bold_copies() {
        let instances = [
            TextInstance {
                pos: [10.0, 20.0],
                glyph_size: [8, 6],
                bearings: [-2, 3],
                ..TextInstance::default()
            },
            TextInstance {
                pos: [11.0, 20.0],
                glyph_size: [8, 6],
                bearings: [-2, 3],
                ..TextInstance::default()
            },
        ];

        assert_eq!(
            instances_ink_bounds_px(&instances),
            Some([8.0, 23.0, 17.0, 29.0])
        );
    }

    #[test]
    fn deferred_inverse_clip_observes_centering_and_post_draw_reveal_mutations() {
        let (font_library, _errors) = FontLibrary::new(FontConfig::default());
        // Both Sugarloaf recorders use the same deferred tail filter.
        {
            let mut text = Text::new(&font_library);
            text.scale_factor = 2.0;
            let glyph = TextInstance {
                pos: [0.0; 2],
                glyph_size: [8, 6],
                glyph_pos: [10, 20],
                ..TextInstance::default()
            };
            text.instances.push(glyph); // Content before the scope must survive.
            let checkpoint = text.instances.len();
            text.instances.push(glyph);
            assert!(text.center_instances_in_rect(
                checkpoint,
                [10.0, 20.0, 20.0, 10.0],
                true,
                true
            ));
            let centered = text.instances[checkpoint];
            // Model a reveal pass which adjusts the instance after drawing.
            text.instances[checkpoint].pos[0] += 1.0;
            text.instances[checkpoint].blur_radius = 2.0;
            let final_pos = text.instances[checkpoint].pos;
            let hole = [final_pos[0] + 2.0, final_pos[1], 2.0, 6.0];
            text.occlude_since(checkpoint, &[hole]);
            assert_eq!(text.instances.len(), 3);
            assert_eq!(text.instances[0].clip_rect, [0.0; 4]);
            for part in &text.instances[checkpoint..] {
                assert_eq!(part.pos, final_pos);
                assert_eq!(part.glyph_pos, centered.glyph_pos);
                assert_eq!(part.blur_radius, 2.0);
                assert!(crate::occlusion::intersection(part.clip_rect, hole).is_none());
            }
            // Restoring to an empty scope must not mask later glyphs.
            text.instances.push(glyph);
            assert_eq!(text.instances.last().unwrap().clip_rect, [0.0; 4]);

            // Conversely, never discard a glyph at draw time just because it
            // overlaps a hole before its icon-centering transform.
            let mut moved = Text::new(&font_library);
            moved.scale_factor = 2.0;
            moved.instances.push(glyph);
            moved.center_instances_in_rect(0, [10.0, 20.0, 20.0, 10.0], true, true);
            moved.occlude_since(0, &[[0.0, 0.0, 8.0, 6.0]]);
            assert_eq!(moved.instances.len(), 1);
            assert_eq!(moved.instances[0].clip_rect, [0.0; 4]);
        }
    }

    #[test]
    fn emitted_icon_ink_centers_in_a_scaled_logical_rect() {
        let (font_library, _errors) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&font_library);
        text.scale_factor = 2.0;
        text.instances.push(TextInstance {
            pos: [0.0, 0.0],
            glyph_size: [8, 6],
            ..TextInstance::default()
        });

        assert!(text.center_instances_in_rect(0, [10.0, 20.0, 20.0, 10.0], true, true));
        assert_eq!(text.instances[0].pos, [36.0, 47.0]);
    }

    #[test]
    fn reveal_instance_layout_preserves_existing_offsets() {
        assert_eq!(std::mem::size_of::<TextInstance>(), 60);
        assert_eq!(std::mem::offset_of!(TextInstance, raster_scale), 52);
        assert_eq!(std::mem::offset_of!(TextInstance, blur_radius), 56);
    }

    #[test]
    fn reveal_ranges_are_half_open_utf8_bytes_and_newest_overlap_wins() {
        // é occupies 1..3, crab 3..7; these are byte offsets, not char indices.
        let source = "aé🦀z";
        let reveals = [
            TextReveal {
                range: 1..3,
                opacity: 0.25,
                blur_radius: 2.0,
            },
            TextReveal {
                range: 3..7,
                opacity: 0.5,
                blur_radius: 1.0,
            },
            TextReveal {
                range: 7..8,
                opacity: 0.0,
                blur_radius: 0.0,
            },
        ];
        let effects: Vec<_> = source
            .char_indices()
            .map(|(byte, ch)| {
                reveal_effect(byte as u32, (byte + ch.len_utf8()) as u32, &reveals, 2.0)
            })
            .collect();
        assert_eq!(effects, [(1.0, 0.0), (0.25, 4.0), (0.5, 2.0), (0.0, 0.0)]);
        assert_eq!(reveal_effect(8, 9, &reveals, 2.0), (1.0, 0.0));
        let invalid = [TextReveal {
            range: 0..1,
            opacity: f32::NAN,
            blur_radius: f32::INFINITY,
        }];
        assert_eq!(reveal_effect(0, 1, &invalid, 2.0), (1.0, 0.0));
        let bounded = [TextReveal {
            range: 0..1,
            opacity: -2.0,
            blur_radius: 100.0,
        }];
        assert_eq!(reveal_effect(0, 1, &bounded, 2.0), (0.0, 4.0));
    }

    fn source_glyph(cluster: u32) -> ShapedGlyph {
        ShapedGlyph {
            id: 1,
            x: 0.0,
            y: 0.0,
            advance: 1.0,
            cluster,
            cluster_end: 0,
        }
    }

    fn source_run(glyphs: Vec<ShapedGlyph>) -> ShapedRun {
        ShapedRun {
            font_id: 0,
            size_u16: 14,
            size_bucket: 56,
            synthetic_bold: false,
            synthetic_italic: false,
            baseline_px: 12,
            advance: glyphs.iter().map(|g| g.advance).sum(),
            source_offset: 0,
            glyphs: glyphs.into(),
        }
    }

    #[test]
    fn indexed_reveal_sweep_matches_overlap_reference_including_unsorted_ranges() {
        let mut seed = 7_u32;
        let mut random = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            seed
        };
        for _ in 0..100 {
            let mut glyphs = Vec::new();
            let mut source_end = 0;
            for _ in 0..40 {
                glyphs.push(source_glyph(source_end));
                glyphs.push(source_glyph(source_end)); // repeated glyphs share effect
                source_end += 1 + random() % 4;
            }
            glyphs.reverse(); // RTL-like visual order
            set_cluster_ends(&mut glyphs, source_end);
            let runs = [source_run(glyphs)];
            let births: Vec<_> = (0..80)
                .map(|_| {
                    let start = (random() % (source_end + 20)) as usize;
                    TextReveal {
                        range: start..start + (random() % 20) as usize,
                        opacity: (random() % 5) as f32 / 4.0,
                        blur_radius: (random() % 10) as f32,
                    }
                })
                .collect();
            let effects = prepare_reveal_effects(&runs, &births, 1.5);
            assert_eq!(effects.len(), 40);
            for glyph in runs[0].source_glyphs() {
                assert_eq!(
                    effects[&glyph.cluster],
                    reveal_effect(glyph.cluster, glyph.cluster_end, &births, 1.5)
                );
            }
        }
        let runs = [source_run(vec![ShapedGlyph {
            cluster_end: 1,
            ..source_glyph(0)
        }])];
        let births = [
            TextReveal {
                range: 0..1,
                opacity: -0.0,
                blur_radius: -0.0,
            },
            TextReveal {
                range: 0..1,
                opacity: 0.0,
                blur_radius: 2.0,
            },
        ];
        assert_eq!(prepare_reveal_effects(&runs, &births, 1.0)[&0], (0.0, 2.0));
    }

    #[test]
    fn cluster_bounds_handle_reordering_duplicate_starts_and_source_tail() {
        // Visual order need not equal source order; repeated starts represent
        // multiple glyphs in a single source cluster. Last cluster includes all
        // remaining source, even invisible trailing bytes with no glyph.
        let mut glyphs = [
            source_glyph(4),
            source_glyph(0),
            source_glyph(2),
            source_glyph(2),
        ];
        set_cluster_ends(&mut glyphs, 9);
        assert_eq!(
            glyphs.map(|g| (g.cluster, g.cluster_end)),
            [(4, 9), (0, 2), (2, 4), (2, 4)]
        );
        set_cluster_ends(&mut [], 9);
    }

    #[test]
    fn ligature_continuation_selects_newest_overlapping_birth_not_old_base() {
        // Shaped source "AffiZ": two glyphs for the ffi cluster (e.g. a font
        // returning a multi-glyph ligature). Byte 3 is the appended continuation.
        let mut glyphs = [
            source_glyph(0),
            source_glyph(1),
            source_glyph(1),
            source_glyph(4),
        ];
        set_cluster_ends(&mut glyphs, 5);
        let births = [
            TextReveal {
                range: 0..2,
                opacity: 0.9,
                blur_radius: 0.25,
            },
            TextReveal {
                range: 2..3,
                opacity: 0.6,
                blur_radius: 0.5,
            },
            TextReveal {
                range: 3..4,
                opacity: 0.2,
                blur_radius: 2.0,
            },
        ];
        for glyph in &glyphs[1..3] {
            assert_eq!((glyph.cluster, glyph.cluster_end), (1, 4));
            assert_eq!(
                reveal_effect(glyph.cluster, glyph.cluster_end, &births[2..], 1.0),
                (0.2, 2.0)
            );
            assert_eq!(
                reveal_effect(glyph.cluster, glyph.cluster_end, &births, 1.0),
                (0.2, 2.0)
            );
            let reversed: Vec<_> = births.iter().cloned().rev().collect();
            assert_eq!(
                reveal_effect(glyph.cluster, glyph.cluster_end, &reversed, 1.0),
                (0.2, 2.0)
            );
        }
        let runs = [source_run(glyphs.to_vec())];
        let newest_only = prepare_reveal_effects(&runs, &births[2..], 1.0);
        assert_eq!(newest_only[&1], (0.2, 2.0));
        assert_eq!(newest_only.len(), 3, "repeated cluster evaluated once");
        let all = prepare_reveal_effects(&runs, &births, 1.0);
        assert_eq!(all[&1], (0.2, 2.0));
        assert_eq!(all[&4], (1.0, 0.0));
        assert_eq!(reveal_effect(4, 5, &births, 1.0), (1.0, 0.0));
        assert_eq!(reveal_effect(1, 1, &births, 1.0), (1.0, 0.0));
        let empty = [TextReveal {
            range: 2..2,
            opacity: 0.0,
            blur_radius: 4.0,
        }];
        assert_eq!(reveal_effect(1, 4, &empty, 1.0), (1.0, 0.0));
    }

    #[test]
    fn paired_blur_kernel_is_dense_normalized_and_has_no_stamped_impulse_gaps() {
        // Exact optimized kernel used in each shader: adjacent binomial taps
        // are paired with bilinear sampling, not nine displaced glyph copies.
        let offsets = [-28.0_f32 / 9.0, -4.0 / 3.0, 0.0, 4.0 / 3.0, 28.0 / 9.0];
        let weights = [9.0_f32, 84.0, 70.0, 84.0, 9.0];
        let mut dense = [0.0_f32; 9];
        for (offset, weight) in offsets.into_iter().zip(weights) {
            let base = offset.floor() as i32;
            let f = offset - offset.floor();
            dense[(base + 4) as usize] += weight * (1.0 - f);
            dense[(base + 5) as usize] += weight * f;
        }
        let expected = [1.0, 8.0, 28.0, 56.0, 70.0, 56.0, 28.0, 8.0, 1.0];
        for (actual, expected) in dense.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.0001);
        }
        assert!((dense.iter().sum::<f32>() - 256.0).abs() < 0.0001);
    }

    #[test]
    fn appended_combining_bytes_reveal_the_complete_existing_cluster() {
        let (library, _) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&library);
        let source = "Ae\u{301} ffi";
        let runs = text.shape_for(source, &DrawOpts::default()).unwrap();
        // Only the newly appended combining mark bytes, not the old base e.
        let effects = [TextReveal {
            range: 2..4,
            opacity: 0.25,
            blur_radius: 1.0,
        }];
        let prepared = prepare_reveal_effects(&runs, &effects, 1.0);
        assert_eq!(prepared[&1], (0.25, 1.0));
        let mut affected = 0;
        for glyph in runs.iter().flat_map(|r| r.source_glyphs()) {
            assert!(source.is_char_boundary(glyph.cluster as usize));
            if glyph.cluster == 1 {
                assert_eq!(glyph.cluster_end, 4);
                assert_eq!(
                    reveal_effect(glyph.cluster, glyph.cluster_end, &effects, 1.0),
                    (0.25, 1.0)
                );
                affected += 1;
            } else if glyph.cluster >= 4 {
                assert_eq!(
                    reveal_effect(glyph.cluster, glyph.cluster_end, &effects, 1.0),
                    (1.0, 0.0)
                );
            }
        }
        assert!(affected > 0);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn appended_combining_reveal_changes_whole_cluster_emission_not_geometry() {
        let (library, _) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&library);
        text.init_cpu();
        let opts = DrawOpts {
            color: [80, 120, 160, 200],
            ..DrawOpts::default()
        };
        let source = "Ae\u{301} Z";
        let advance = text.draw(1.0, 2.0, source, &opts);
        let ordinary = text.instances.clone();
        text.instances.clear();
        let revealed = text.draw_revealed(
            1.0,
            2.0,
            source,
            &opts,
            &[TextReveal {
                range: 2..4,
                opacity: 0.25,
                blur_radius: 1.0,
            }],
        );
        assert_eq!(advance, revealed);
        assert_eq!(ordinary.len(), text.instances.len());
        let mut changed = 0;
        for (sharp, reveal) in ordinary.iter().zip(&text.instances) {
            assert_eq!(sharp.pos, reveal.pos);
            assert_eq!(sharp.glyph_pos, reveal.glyph_pos);
            if reveal.blur_radius > 0.0 {
                assert_eq!(reveal.color[3], 50);
                changed += 1;
            } else {
                assert_eq!(sharp.color, reveal.color);
            }
        }
        assert!(
            changed > 0,
            "new mark bytes must reveal the old base cluster"
        );
    }

    #[test]
    fn fallback_clusters_are_global_and_cache_remains_substring_local() {
        let (library, _) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&library);
        for preferred in [None, Some(0)] {
            let opts = DrawOpts {
                font_id: preferred,
                ..DrawOpts::default()
            };
            let source = "é \u{f07b} café 🦀 z";
            let runs = text.shape_for(source, &opts).unwrap();
            assert!(runs.len() >= 2);
            // Every cached run can be found in the source at its global start.
            // Re-shaping that suffix as an isolated substring must start at 0.
            for (index, run) in runs.iter().enumerate() {
                let expected_end = runs
                    .get(index + 1)
                    .map(|r| r.source_glyphs().map(|g| g.cluster).min().unwrap())
                    .unwrap_or(source.len() as u32);
                assert_eq!(
                    run.source_glyphs().map(|g| g.cluster_end).max().unwrap(),
                    expected_end
                );
                let first = run.source_offset as usize;
                assert!(source.is_char_boundary(first));
                let cached = text
                    .shape_cache
                    .values()
                    .find(|cached| {
                        cached.font_id == run.font_id
                            && cached.glyphs.len() == run.glyphs.len()
                            && cached.glyphs.iter().zip(run.source_glyphs()).all(
                                |(a, b)| {
                                    a.id == b.id
                                        && a.cluster + first as u32 == b.cluster
                                        && a.cluster_end + first as u32 == b.cluster_end
                                },
                            )
                    })
                    .expect("global offset must not contaminate cached clusters");
                assert_eq!(cached.glyphs[0].cluster, 0);
                assert!(run
                    .source_glyphs()
                    .all(|g| source.is_char_boundary(g.cluster as usize)
                        && source.is_char_boundary(g.cluster_end as usize)
                        && g.cluster < g.cluster_end));
            }
            let again = text.shape_for(source, &opts).unwrap();
            assert_eq!(
                runs.iter()
                    .flat_map(|r| r.source_glyphs().map(|g| g.cluster))
                    .collect::<Vec<_>>(),
                again
                    .iter()
                    .flat_map(|r| r.source_glyphs().map(|g| g.cluster))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn revealed_draw_keeps_full_run_geometry_and_settled_instances() {
        let (library, _) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&library);
        text.init_cpu();
        text.set_scale_factor(2.0);
        let opts = DrawOpts {
            color: [80, 120, 160, 200],
            ..DrawOpts::default()
        };
        let source = "AV café office";
        let width = text.draw(1.0, 2.0, source, &opts);
        let ordinary = text.instances.clone();
        text.instances.clear();
        let width_revealed = text.draw_revealed(
            1.0,
            2.0,
            source,
            &opts,
            &[TextReveal {
                range: 3..8,
                opacity: 0.5,
                blur_radius: 1.5,
            }],
        );
        assert_eq!(width, width_revealed);
        assert_eq!(ordinary.len(), text.instances.len());
        let mut changed = 0;
        let mut settled = 0;
        for (a, b) in ordinary.iter().zip(&text.instances) {
            assert_eq!(a.pos, b.pos);
            assert_eq!(a.glyph_pos, b.glyph_pos);
            assert_eq!(a.glyph_size, b.glyph_size);
            if b.blur_radius > 0.0 {
                assert_eq!(b.blur_radius, 3.0);
                assert_eq!(b.color[3], 100);
                changed += 1;
            } else {
                assert_eq!(a.color, b.color);
                settled += 1;
            }
        }
        assert!(changed > 0 && settled > 0);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn cpu_color_reveal_modulates_premultiplied_rgba_without_tint() {
        let mut pixels = [0u32];
        blit_text_color(
            &mut pixels,
            1,
            1,
            0,
            0,
            1,
            1,
            &[100, 60, 20, 200],
            1,
            0,
            0,
            [0.0; 4],
            128,
        );
        assert_eq!(pixels[0], (50 << 16) | (30 << 8) | 10);
    }

    #[test]
    fn shapes_icon_and_label_as_separate_font_runs() {
        let (font_library, _errors) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&font_library);

        let runs = text
            .shape_for("\u{f07b} neoism-project", &DrawOpts::default())
            .expect("shape mixed icon/text label");

        assert!(
            runs.len() >= 2,
            "mixed icon/text labels must not be shaped with one first-character font"
        );
        assert_ne!(
            runs[0].font_id, runs[1].font_id,
            "icon and ASCII label should resolve to different font runs"
        );
    }

    #[test]
    fn preferred_markdown_font_falls_back_for_icons() {
        let (library, _errors) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&library);
        let preferred = 0;
        let mixed = text
            .shape_for(
                "\u{f07b} Neoism 🦀",
                &DrawOpts {
                    font_id: Some(preferred),
                    ..DrawOpts::default()
                },
            )
            .expect("shape mixed markdown with preferred font");
        assert!(
            mixed.len() >= 2,
            "pinned markdown fonts must still split missing glyphs onto fallback faces"
        );
        let cached_coverage = text.font_coverage.len();
        text.shape_for(
            "\u{f07b} Neoism 🦀",
            &DrawOpts {
                font_id: Some(preferred),
                ..DrawOpts::default()
            },
        )
        .expect("reshape mixed markdown with preferred font");
        assert_eq!(text.font_coverage.len(), cached_coverage);
    }

    #[test]
    fn markdown_prefix_measurements_preserve_exact_geometry_and_skip_reshaping() {
        let (library, _errors) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&library);
        for source in [
            "",
            "AV office ffi",
            "café e\u{301} 日本語",
            "\u{f07b} Neoism 🦀",
        ] {
            for opts in [
                DrawOpts::default(),
                DrawOpts {
                    bold: true,
                    italic: true,
                    ..DrawOpts::default()
                },
                DrawOpts {
                    font_size: 21.25,
                    font_id: Some(0),
                    ..DrawOpts::default()
                },
            ] {
                for scale in [1.0, 1.5, 2.0] {
                    text.set_scale_factor(scale);
                    let expected: Vec<f32> = std::iter::once(0.0)
                        .chain(source.char_indices().map(|(offset, ch)| {
                            text.measure(&source[..offset + ch.len_utf8()], &opts)
                        }))
                        .collect();
                    assert_eq!(text.measure_char_prefixes(source, &opts), expected);
                    // A warm geometry hit must not even consult/populate shaping.
                    text.shape_cache.clear();
                    let painted = DrawOpts {
                        color: [10, 20, 30, 255],
                        clip_rect: Some([0.0, 10.0, 300.0, 200.0]),
                        ..opts
                    };
                    assert_eq!(text.measure_char_prefixes(source, &painted), expected);
                    assert!(text.shape_cache.is_empty());
                }
            }
        }
        assert!(!text.prefix_measure_cache.is_empty());
        text.clear_glyph_cache();
        assert!(text.prefix_measure_cache.is_empty());
    }

    #[test]
    fn markdown_prefix_cache_is_bounded_and_bypasses_oversized_lines() {
        let (library, _errors) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&library);
        let opts = DrawOpts::default();
        for index in 0..PREFIX_MEASURE_CACHE_ENTRIES + 8 {
            text.measure_char_prefixes(&format!("line {index}"), &opts);
        }
        assert_eq!(
            text.prefix_measure_cache.len(),
            PREFIX_MEASURE_CACHE_ENTRIES
        );
        let long = "x".repeat(PREFIX_MEASURE_MAX_BYTES + 1);
        assert_eq!(
            text.measure_char_prefixes(&long, &opts).len(),
            long.len() + 1
        );
        assert!(text
            .prefix_measure_cache
            .iter()
            .all(|(key, _)| key.text != long));
        // Font replacement must also invalidate cached logical advances.
        text.set_font_library(&library);
        assert!(text.prefix_measure_cache.is_empty());
    }

    #[test]
    #[ignore = "manual CPU-only markdown hit-test microbenchmark"]
    fn markdown_prefix_measurement_benchmark() {
        let (library, _errors) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&library);
        let opts = DrawOpts::default();
        let lines: Vec<_> = (0..48).map(|i| {
            format!("Line {i}: Markdown redraw retains exact kerning and Unicode café hit-test geometry.")
        }).collect();
        for line in &lines {
            text.measure_char_prefixes(line, &opts);
        }
        let start = std::time::Instant::now();
        for _ in 0..100 {
            for line in &lines {
                for (offset, ch) in line.char_indices() {
                    std::hint::black_box(
                        text.measure(&line[..offset + ch.len_utf8()], &opts),
                    );
                }
            }
        }
        let previous = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            for line in &lines {
                std::hint::black_box(text.measure_char_prefixes(line, &opts));
            }
        }
        eprintln!(
            "48 lines x 100 warm frames: prefix loop {previous:?}, cached {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn font_library_swap_invalidates_every_font_derived_cache() {
        let (first, _errors) = FontLibrary::new(FontConfig::default());
        let (second, _errors) = FontLibrary::new(FontConfig::default());
        let mut text = Text::new(&first);

        text.shape_for("Neoism \u{f07b}", &DrawOpts::default())
            .expect("shape text before library swap");
        assert!(!text.shape_cache.is_empty());
        assert!(!text.font_resolve.is_empty());
        assert!(!text.baseline_cache.is_empty());

        text.set_font_library(&second);

        assert!(Arc::ptr_eq(&text.font_library.inner, &second.inner));
        assert!(!Arc::ptr_eq(&text.font_library.inner, &first.inner));
        assert!(text.shape_cache.is_empty());
        assert!(text.font_resolve.is_empty());
        assert!(text.synthesis_cache.is_empty());
        assert!(text.baseline_cache.is_empty());
    }
}
