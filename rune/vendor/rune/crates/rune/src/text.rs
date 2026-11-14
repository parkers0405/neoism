//! Parley layout (wrapping, rich spans, registered families) over embedded fonts, swash rasterising
//! glyphs and SVG icons into one greyscale atlas.
//!
//! Invariants: layouts are in device pixels; every layout is cached by a 64-bit hash of
//! (text, spans, style, scale, width), kept for two commits unused and then while a budget of
//! stale layouts lasts (`STALE_BYTES`), least recently used out first; a width not cached re-breaks
//! a cached layout of the same text and style rather than shaping it again; glyph keys include
//! the run's variation coordinates, so one variable font serves every weight.

use std::collections::HashMap;
use std::hash::Hash;
use crate::runtime::{HashMap as FxHashMap, HashSet as FxHashSet};
use std::rc::Rc;
use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, FamilyId, Script, SourceCache};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight, Layout, LayoutContext,
    LineHeight, PositionedLayoutItem, StyleProperty,
};
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};

use crate::color::Color;
use crate::tok::{Family, OpticalSize, TextStyle};

mod atlas;
mod atlas_fill;
pub(crate) use atlas_fill::QueuedFill;
mod case;
mod color_icon;
mod cursor;
mod decorations;
mod families;
pub(crate) mod full_faces;
mod language;
pub use language::set_language;
mod line_break;
mod span;
mod system_fonts;
mod system_weight;
mod default_fonts;
#[cfg(all(test, target_os = "macos"))]
mod core_text_tests;
pub(crate) mod coretext_match;
pub use coretext_match::FontSmoothing;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod pixel_compare;
mod fallback;
mod glyph_cache;
mod glyph_emit;
mod hash;
mod hidden_axes;
pub(crate) mod icon;
mod layout_cache;
mod macos;
pub use macos::smoothing::ApplePipeline;
mod windows;
mod measure;
pub(crate) mod outline;
pub use outline::Outline;
mod pill_chips;
mod rich_layout;
mod run;
pub(crate) use pill_chips::{bare_icons, pill_chips};
mod svg_path;
mod trak;
mod underline;
mod line_metrics;
mod windfoil;
pub use line_metrics::{last_line_fill, layout_height, stream_height_lag};
pub(crate) use line_metrics::spec_height;
pub(crate) use underline::underline_edges;
pub use case::TextCase;
pub use hash::text_hash;
pub(crate) use hash::{directed, elide_key, shape_key};
pub(crate) use families::choose_families;
pub use measure::{FamiliesGuard, TextInk, advance, content_widths, font_epoch, ink, measure, measure_with, with_families};
pub use rich_layout::RichLayoutOptions;
pub use run::{Run, RunLayout};
pub(crate) use case::Source as CaseSource;
pub(crate) use layout_cache::CachedElision;

use default_fonts::DEFAULT_FONTS;
use fallback::Route;
use glyph_emit::Face;
use hidden_axes::hidden_axes;
use layout_cache::{Cache, Entry};
#[cfg(test)]
use layout_cache::STALE_BYTES;
pub use icon::{Icon, icon_coverage, set_icon_swap, set_icon_swap_size};
pub(crate) use icon::Facing;
use icon::rasterize_icon;
use trak::{Trak, Traks, font_trak};

pub use atlas::{ATLAS_SIZE, Atlas, AtlasPage, GlyphQuad};
pub(crate) use cursor::{cursor_from_point, cursor_geometry, line_edge, move_line, range_boxes};
pub use decorations::{Deco, SPAN_RADIUS};
pub(crate) use decorations::PaintedDeco;
pub use span::{Ink, Link, LinkPaint, Span, TextHighlight};
pub(crate) use span::{BareIcon, PillChip, PillMetrics, flatten_spans};

use decorations::Decoration;
use atlas::{AtlasGlyph, COLOR_GLYPH_BUDGET, ColorGlyph, GlyphKey, GlyphMap, Rasterized};
#[cfg(test)]
use atlas::{MAX_OVERFLOW_PAGES, MAX_STEADY_PAGES};
#[cfg(test)]
pub(crate) use glyph_emit::GLIDE_PHASES;
use line_break::break_lines_with_blocks;
use span::PILL_BOX;
use system_fonts::{
    LoadedFiles, SYMBOL_FAMILIES, SYMBOL_FONT_FILES, family_list, load_families, load_installed, load_system_mono, named_list, names_system_mono,
    register_cjk_fallbacks, register_script_fallbacks,
};
#[cfg(any(test, not(target_arch = "wasm32")))]
use system_fonts::system_font_files;
#[cfg(test)]
use system_fonts::CJK_FONT_FILES;

/// Lean of a synthetic oblique, in degrees, for faces with no italic.
const SYNTHETIC_OBLIQUE: u8 = 12;

/// The colour image of `glyph` (COLR layers or an embedded bitmap scaled to size), or None
/// when the font draws it as an outline.
fn rasterize_color(scaler: &mut swash::scale::Scaler<'_>, glyph: u16) -> Option<(crate::pixels::Pixels, swash::zeno::Placement)> {
    if !scaler.has_color_bitmaps() && !scaler.has_color_outlines() {
        return None;
    }
    let image = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit)]).render(scaler, glyph)?;
    let (w, h) = (image.placement.width, image.placement.height);
    if image.content != Content::Color || w == 0 || h == 0 || image.data.len() != (w * h * 4) as usize {
        return None;
    }
    let pixels = crate::pixels::Pixels::new();
    pixels.set(w, h, image.data);
    Some((pixels, image.placement))
}

// ---------------------------------------------------------------------------------------------

pub struct TextSystem {
    font_cx: FontContext,
    layout_cx: LayoutContext<Ink>,
    scale_cx: ScaleContext,
    layouts: Cache<Rc<Layout<Ink>>>,
    /// The `layouts` key of the latest layout of each shaping (see `layout_blocks_inner`), whose
    /// glyphs a new width re-breaks instead of shaping the text again.
    shapes: HashMap<u64, u64>,
    /// Shapings whose text was edited this commit (see [`Self::supersede`]).
    superseded: FxHashSet<u64>,
    elisions: Cache<CachedElision>,
    runs: Cache<Rc<RunLayout>>,
    generation: u32,
    /// The slots' families as named; `sans` and `mono` are the lists text asks for.
    families: (String, String),
    /// The `choose_families` change these slots follow.
    chosen: u64,
    /// The language text shapes in, and the `set_language` change it follows.
    language: Option<parley::fontique::Language>,
    language_epoch: u64,
    /// Whether it swapped in the full faces a web app fetches (see `full_faces`).
    full_faces: bool,
    pub sans: String,
    pub mono: String,
    symbol_families: Vec<&'static str>,
    /// The colour emoji family among `symbol_families`.
    emoji: Option<&'static str>,
    /// The installed math families, once a text has needed them.
    math: Option<Vec<&'static str>>,
    /// The installed `Family::SystemMono` families, once a text has needed them.
    system_mono: Option<Vec<String>>,
    /// The sans or mono slot names `ui-monospace` (see `family_list`).
    slot_system_mono: bool,
    /// Bit `i` is set once `fallback::SCRIPT_FONTS[i]` has been looked up.
    scripts: u64,
    font_files: LoadedFiles,
    /// The loaded system faces' families, for the face DirectWrite picks (`system_weight`).
    system_faces: system_weight::Faces,
    /// Installed families looked up by name (`load_installed`), lowercase.
    installed: std::collections::HashSet<String>,
    pub atlas: Atlas,
    color_glyphs: GlyphMap<ColorGlyph>,
    color_pixels: Vec<crate::pixels::Pixels>,
    color_bytes: usize,
    color_icons: color_icon::ColorIcons,
    /// Layouts shaped, a running count (shaping work counter for perf gates).
    pub shaped: u32,
    /// Layouts re-broken at a new width from an earlier shaping, a running count.
    pub rebroken: u32,
    /// Reused across lines and frames when mapping reveal glyphs back to source bytes.
    cluster_scratch: Vec<(f32, u32)>,
    coords_scratch: Vec<i16>,
    traks: Traks,
    pub(crate) smoothing: FontSmoothing,
    /// The `trak` table of the first font of a family slot at a weight, upright or italic.
    primary_traks: FxHashMap<(Family, u32, bool), Option<Rc<Trak>>>,
    /// The hidden-axis coordinates (`hidden_axes`) of the same first font.
    primary_axes: FxHashMap<(Family, u32, bool), Rc<[parley::FontVariation]>>,
    /// CoreGraphics' glyph pipeline for `smoothing`, and its per-ink state.
    apple: macos::smoothing::Smoothing,
    /// DirectWrite for `FontSmoothing::DirectWrite`, and its per-ink state.
    windows: windows::Rasterizer,
    pub(crate) glyph_cache: glyph_cache::GlyphCache,
    /// Device px `[x, y]` the next glyphs glide from their resting pixel ([`Self::set_glide`]).
    glide: [f32; 2],
    /// How the glyphs placed next draw ([`Self::begin_text`], [`Self::set_emit`]).
    emit: outline::Emit,
    /// Each painted text's raster path (`outline::RasterPath`), by node.
    paths: outline::Paths,
    /// What each face (font, index) is, for drawing it from outlines.
    face_kinds: FxHashMap<(u64, u32), windfoil::FaceKind>,
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new(&[], ("Inter", "Inter"))
    }
}

fn variations(style: TextStyle, size: f32) -> Vec<parley::FontVariation> {
    let mut settings: Vec<_> = parley::FontVariation::parse_css_list(style.variations).flatten().collect();
    let opsz = parley::setting::Tag::new(b"opsz");
    let value = match style.optical_size {
        OpticalSize::Auto if !settings.iter().any(|v| v.tag == opsz) => Some(size),
        OpticalSize::Fixed(value) => { settings.retain(|v| v.tag != opsz); Some(value) }
        _ => None,
    };
    if let Some(value) = value.filter(|v| v.is_finite() && *v > 0.0) {
        settings.push(parley::FontVariation::new(opsz, value));
    }
    settings
}

/// The byte of each tab in monospace text (mono spans, or all of it in a mono style) with the
/// columns it fills to reach the next 4-column stop. Columns count from the start of the line
/// or of the monospace run, whichever is later.
fn mono_tab_stops(text: &str, spans: &[Span], mono_style: bool) -> Vec<(usize, u32)> {
    let mut regions: Vec<(usize, usize)> = if mono_style {
        vec![(0, text.len())]
    } else {
        let mut r: Vec<_> = spans.iter().filter(|s| s.mono && s.start < s.end).map(|s| (s.start as usize, (s.end as usize).min(text.len()))).collect();
        r.sort_unstable();
        r
    };
    regions.dedup_by(|next, prev| {
        let joined = next.0 <= prev.1;
        if joined {
            prev.1 = prev.1.max(next.1);
        }
        joined
    });
    let mut out = Vec::new();
    for (start, end) in regions {
        let Some(run) = text.get(start..end) else { continue };
        let mut column = 0u32;
        for (i, ch) in run.char_indices() {
            match ch {
                '\n' => column = 0,
                '\t' => {
                    let fill = 4 - column % 4;
                    out.push((start + i, fill));
                    column += fill;
                }
                _ => column += 1,
            }
        }
    }
    out
}

impl TextSystem {
    /// `fonts` replaces the built-in Inter faces when non-empty on native. Web keeps its
    /// bundled sans and mono fallbacks, since there are no system fonts to discover.
    /// `families` names the sans and mono slots; others are selectable with `Family::Named`.
    pub fn new(fonts: &[&'static [u8]], families: (&str, &str)) -> Self {
        measure::note_fonts(fonts, families);
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        let list: &[&'static [u8]] = if fonts.is_empty() { DEFAULT_FONTS } else { fonts };
        let mut registered = Vec::new();
        #[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
        if !fonts.is_empty() {
            for data in DEFAULT_FONTS {
                collection.register_fonts(Blob::new(Arc::new(*data)), None);
            }
        }
        for data in list {
            let mut added: Vec<FamilyId> = collection.register_fonts(Blob::new(Arc::new(*data)), None).into_iter().map(|(id, _)| id).collect();
            added.sort_unstable();
            registered.extend(added);
        }
        let mut font_files = LoadedFiles::new();
        register_cjk_fallbacks(&mut collection, &mut font_files, &registered, families);
        let symbol_families = load_families(&mut collection, SYMBOL_FONT_FILES, SYMBOL_FAMILIES);
        let emoji = symbol_families.iter().copied().find(|f| f.contains("Emoji"));
        #[cfg(target_arch = "wasm32")]
        let families = if fonts.is_empty() { ("Inter", "IBM Plex Mono") } else { families };
        let (sans, mono) = families::slots(families.0, families.1);
        let mut text = TextSystem {
            font_cx: FontContext { collection, source_cache: SourceCache::default() },
            layout_cx: LayoutContext::new(),
            scale_cx: ScaleContext::new(),
            layouts: Default::default(),
            shapes: Default::default(),
            superseded: Default::default(),
            elisions: Default::default(),
            runs: Default::default(),
            generation: 0,
            families: (families.0.to_owned(), families.1.to_owned()),
            chosen: 0,
            language: None,
            language_epoch: 0,
            full_faces: false,
            sans, mono,
            symbol_families,
            emoji,
            math: None,
            system_mono: None,
            slot_system_mono: names_system_mono(families.0) || names_system_mono(families.1),
            scripts: 0,
            font_files,
            system_faces: Default::default(),
            installed: Default::default(),
            atlas: Atlas::new(),
            color_glyphs: GlyphMap::default(),
            color_pixels: Vec::new(),
            color_bytes: 0,
            color_icons: color_icon::ColorIcons::default(),
            shaped: 0,
            rebroken: 0,
            cluster_scratch: Vec::new(),
            coords_scratch: Vec::new(),
            traks: Traks::new(),
            smoothing: FontSmoothing::default(),
            primary_traks: Default::default(),
            primary_axes: Default::default(),
            apple: Default::default(),
            windows: Default::default(),
            glyph_cache: Default::default(),
            glide: [0.0; 2],
            emit: outline::Emit::Atlas,
            paths: Default::default(),
            face_kinds: Default::default(),
        };
        text.load_installed(families.0);
        text.load_installed(families.1);
        text.follow_full_faces();
        text
    }

    /// The `trak` table of the font text in `family` at `weight` shapes with first.
    fn primary_trak(&mut self, family: Family, weight: f32, italic: bool) -> Option<Rc<Trak>> {
        let key = (family, weight.to_bits(), italic);
        if let Some(trak) = self.primary_traks.get(&key) {
            return trak.clone();
        }
        let font = self.primary_font(family, weight, italic);
        let trak = font.and_then(|f| font_trak(&mut self.traks, &f.blob, f.index));
        self.primary_traks.insert(key, trak.clone());
        trak
    }

    /// The hidden-axis coordinates (`hidden_axes`) of the font text in `family` at `weight`
    /// shapes with first.
    fn primary_axes(&mut self, family: Family, weight: f32, italic: bool) -> Rc<[parley::FontVariation]> {
        let key = (family, weight.to_bits(), italic);
        if let Some(axes) = self.primary_axes.get(&key) {
            return axes.clone();
        }
        let font = self.primary_font(family, weight, italic);
        let axes: Rc<[parley::FontVariation]> = font
            .and_then(|f| FontRef::from_index(f.blob.as_ref(), f.index as usize).map(|r| hidden_axes(r, weight)))
            .unwrap_or_default()
            .into();
        self.primary_axes.insert(key, axes.clone());
        axes
    }

    /// The font text in `family` at `weight` shapes with first.
    fn primary_font(&mut self, family: Family, weight: f32, italic: bool) -> Option<parley::fontique::QueryFont> {
        if family == Family::SystemMono || self.slot_system_mono {
            self.system_mono();
        }
        if let Family::Named(list) = family {
            self.load_installed(list);
        }
        let system = self.system_mono.as_deref().unwrap_or_default();
        let named = |list: &str| -> Vec<String> {
            family_list(list, system, &self.sans)
                .into_iter()
                .filter_map(|f| match f { FontFamilyName::Named(name) => Some(name.into_owned()), _ => None })
                .collect()
        };
        let names = match family {
            Family::Sans => named(&self.sans),
            Family::Mono => named(&self.mono),
            Family::Named(list) => named_list(list).into_iter().filter_map(|f| match f { FontFamilyName::Named(name) => Some(name.into_owned()), _ => None }).chain(named(&self.sans)).collect(),
            Family::SystemMono => system.iter().cloned().chain(named(&self.mono)).collect(),
        };
        let mut query = self.font_cx.collection.query(&mut self.font_cx.source_cache);
        query.set_families(names.iter().map(|name| parley::fontique::QueryFamily::Named(name)));
        let style = if italic { FontStyle::Italic } else { FontStyle::Normal };
        query.set_attributes(parley::fontique::Attributes::new(parley::fontique::FontWidth::NORMAL, style, FontWeight::new(weight)));
        query.first_available_font()
    }

    /// The installed `Family::SystemMono` families, loading them the first time.
    /// Loads the installed families CSS family list `list` names that aren't registered, by
    /// name, so a slot or `Family::Named` can be any font on the system.
    pub(crate) fn load_installed(&mut self, list: &str) {
        load_installed(&mut self.font_cx.collection, &mut self.font_files, &mut self.installed, list);
    }

    fn system_mono(&mut self) -> &[String] {
        let (collection, loaded) = (&mut self.font_cx.collection, &mut self.font_files);
        self.system_mono.get_or_insert_with(|| load_system_mono(collection, loaded))
    }

    /// The RGBA image of a colour glyph quad (`GlyphQuad::color`, nonzero).
    pub fn color_glyph(&self, color: u32) -> &crate::pixels::Pixels {
        &self.color_pixels[color as usize - 1]
    }

    /// The colours the next glyphs draw in (`spans` by `Ink`). On macOS they choose each
    /// glyph's dilation and on Windows its gamma table, so glyphs in light and dark inks
    /// rasterize separately.
    pub fn set_glyph_inks(&mut self, element: Color, spans: &[Span]) {
        self.apple.set_inks(element, spans);
        self.windows.set_inks(element, spans);
    }

    /// Whether glyphs rasterise with DirectWrite: `FontSmoothing::DirectWrite` where it exists.
    pub(crate) fn direct_write(&mut self) -> bool {
        self.smoothing == FontSmoothing::DirectWrite && self.windows.available()
    }

    /// How glyphs rasterise and place ([`FontSmoothing`]).
    pub fn font_smoothing(&self) -> FontSmoothing { self.smoothing }

    /// How far each gliding glyph mask's ink sits below the top of its quad, in device px, keyed
    /// as `blend::apron_key` keys its instance: a DirectWrite or Windfoil mask keeps one box across its y
    /// phases and moves its ink down that box by its phase, so the box moves a whole pixel only
    /// where the phase wraps. The motion gate's raster, which spreads a glyph over its quad,
    /// moves its coverage by this so a glide reads as the quarter-pixel steps it draws.
    pub(crate) fn glyph_shifts(&self) -> std::collections::HashMap<[u32; 3], f32> {
        self.atlas.map.iter()
            .filter(|(k, _)| k.subpy != 0 && matches!(k.apple, glyph_emit::DIRECT_WRITE_KEY | glyph_emit::WINDFOIL_KEY))
            .filter_map(|(k, a)| a.mask().map(|a| ([(a.x as f32).to_bits(), (a.y as f32).to_bits(), (a.page as f32).to_bits()], k.subpy as f32 / glyph_emit::GLIDE_PHASES as f32)))
            .collect()
    }

    /// The exponent a glyph's coverage is raised to when drawn in premultiplied `fill` on this
    /// platform: 1 (coverage as is) on macOS and iOS, and a browser's text gamma for `Off`
    /// elsewhere.
    pub fn coverage_exponent(&self, fill: [f32; 4]) -> f32 {
        let apple = cfg!(any(target_os = "macos", target_os = "ios"));
        coretext_match::coverage_exponent(self.smoothing, fill, apple, cfg!(target_os = "windows"))
    }

    /// Rasterise and place glyphs in `mode` from the next placement on, with its CoreGraphics
    /// stages ([`FontSmoothing::pipeline`]). Glyphs already rasterised in another mode stay
    /// cached under their own keys.
    pub fn set_font_smoothing(&mut self, mode: FontSmoothing) {
        self.smoothing = mode;
        self.apple.set_pipeline(mode.pipeline());
    }

    /// The CoreGraphics stages glyphs rasterize with.
    pub fn apple_pipeline(&self) -> ApplePipeline { self.apple.pipeline() }
    /// Overrides the stages [`Self::set_font_smoothing`] chose, until the next call to it; for
    /// measuring the stages one at a time.
    pub fn set_apple_pipeline(&mut self, pipeline: ApplePipeline) { self.apple.set_pipeline(pipeline); }

    /// Begins a new batch of glyph placements, reclaiming unused atlas storage.
    /// Previously emitted quads must no longer be sampled after this call. Runtime
    /// calls this automatically; direct text/renderer users call it before shaping.
    pub fn begin_frame(&mut self) {
        self.begin_frame_pinned(0);
    }

    pub(crate) fn begin_frame_pinned(&mut self, pinned_mask: u32) {
        self.atlas.begin_frame(pinned_mask);
        self.paths.begin_frame();
        self.glyph_cache.begin_frame();
        if self.color_bytes > COLOR_GLYPH_BUDGET {
            let before = self.color_glyphs.len();
            self.color_glyphs.retain(|_, c| c.used);
            if self.color_glyphs.len() != before {
                // Only retire scales unused in the last frame. Clearing active emoji
                // would rerasterize the entire visible working set every frame once
                // that working set alone exceeds the soft budget.
                let old = std::mem::take(&mut self.color_pixels);
                self.color_bytes = 0;
                for c in self.color_glyphs.values_mut() {
                    self.color_pixels.push(old[c.index as usize - 1].clone());
                    c.index = self.color_pixels.len() as u32;
                    self.color_bytes += c.w as usize * c.h as usize * 4;
                }
            }
        }
        for c in self.color_glyphs.values_mut() { c.used = false; }
        self.color_icons.begin_frame();
    }

    /// `shape` is [`shape_key`] of the same text when the caller already has it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_keyed(
        &mut self,
        shape: Option<u64>,
        text: &str,
        spans: &[Span],
        blocks: &[crate::edit::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        wrap_anywhere: bool,
        cache: bool,
        base: parley::BaseDirection,
    ) -> Rc<Layout<Ink>> {
        self.follow_language();
        let shared = (!cache).then(|| std::mem::replace(&mut self.layout_cx, LayoutContext::new()));
        let has_indented_blocks = blocks.iter().any(|&b| b != crate::edit::Block::Paragraph);
        // Shaping depends on the text, spans, style, scale, overflow-wrap and pills; line
        // breaking adds the width, alignment and block indents. A layout cached at one width
        // is re-broken at another rather than shaped again.
        let shape_key = shape.unwrap_or_else(|| hash::directed(shape_key(text, spans, style, scale, wrap_anywhere, pills), base));
        let key = hash::break_key(shape_key, max_width, align, has_indented_blocks.then_some(blocks));
        let g = self.generation;
        if cache && let Some(Entry { value: l, used, .. }) = self.layouts.get_mut(&key) {
            *used = g;
            // A width change re-breaks the newest layout, not one measured since at another.
            self.shapes.entry(shape_key).or_insert(key);
            return l.clone();
        }
        // Each pill's label range, and the room before and after it in device px; boxes keep
        // the pill's index in their id, so ranges that fail the checks keep no chip.
        let pm = PillMetrics::new(style.size);
        let pills: Vec<(usize, std::ops::Range<usize>, u32, u32)> = pills
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let r = (p.start as usize).min(text.len())..(p.end as usize).min(text.len());
                let (lead, trail) = match &p.bare {
                    Some(b) => (b.size + b.gap, 0.0),
                    None => (pm.lead(p.icon.is_some()), pm.trail()),
                };
                (i, r, (lead * scale).round() as u32, (trail * scale).round() as u32)
            })
            .filter(|(_, r, ..)| {
                !r.is_empty() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end) && !text.as_bytes()[r.clone()].contains(&b'\n')
            })
            .collect();
        let break_lines = |layout: &mut Layout<Ink>| {
            if has_indented_blocks || !pills.is_empty() {
                break_lines_with_blocks(layout, blocks, style.size, scale, max_width);
            } else {
                layout.break_all_lines(max_width);
            }
        };
        if cache
            && let Some(prev) = self.shapes.get(&shape_key).copied()
            && let Some(Entry { value: shaped, used, .. }) = self.layouts.get(&prev)
        {
            self.rebroken += 1;
            let _break_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Text);
            // One this commit hasn't used and nothing else holds moves to the new width and
            // re-breaks in place; otherwise the shaped runs are copied.
            let (moves, mut l) = (*used != g && Rc::strong_count(shaped) == 1, shaped.clone());
            if moves { self.layouts.remove(&prev); self.glyph_cache.forget(&l); }
            let layout = Rc::make_mut(&mut l);
            break_lines(layout);
            layout.align(align, AlignmentOptions::default());
            self.layouts.insert(key, Entry::new(l.clone(), g, text.len(), shape_key));
            self.shapes.insert(shape_key, key);
            return l;
        }
        self.shaped += 1;
        let _shape_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Text);
        let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
        let tabs = if text.contains('\t') { mono_tab_stops(text, spans, mono_style) } else { Vec::new() };
        let widen = if tabs.is_empty() { Vec::new() } else { self.mono_tab_widening(&tabs, spans, style, scale) };
        // Parley has no tab stops and fonts draw a tab as a missing glyph (or nothing): shape it
        // as a space, one byte like the tab, so every byte range still holds. Monospace tabs
        // then widen to the next 4-column stop through word spacing.
        let spaced;
        let text = if text.contains('\t') {
            spaced = zeroize::Zeroizing::new(text.replace('\t', " "));
            spaced.as_str()
        } else {
            text
        };
        let routes = fallback::routes(text);
        if !text.is_ascii() {
            let (mut scripts, mut symbols) = (0u64, false);
            for ch in text.chars() {
                for (i, fonts) in fallback::SCRIPT_FONTS.iter().enumerate() {
                    if (self.scripts | scripts) & (1 << i) == 0 && fonts.covers(ch) {
                        scripts |= 1 << i;
                    }
                }
                symbols |= self.math.is_none() && fallback::needs_symbols(ch);
            }
            for (i, fonts) in fallback::SCRIPT_FONTS.iter().enumerate() {
                if scripts & (1 << i) != 0 {
                    self.scripts |= 1 << i;
                    register_script_fallbacks(&mut self.font_cx.collection, &mut self.font_files, fonts);
                }
            }
            if symbols {
                self.math = Some(load_families(&mut self.font_cx.collection, fallback::MATH_FONT_FILES, fallback::MATH_FAMILIES));
            }
        }
        // `trak` tracking at the point size (see `trak.rs`), on top of `style.tracking`. An
        // explicit optical size `o` picks the entry instead, as Core Text tracks `o`-point text
        // scaled to the set size; spans scale the lookup with their size. Each style range gets
        // its predicted first font's spacing; runs in another font are checked after shaping.
        let trak_tag = parley::setting::Tag::new(b"trak");
        let mut features: Vec<parley::FontFeature> = parley::FontFeature::parse_css_list(style.features).flatten().collect();
        let tracked = features.iter().rev().find(|f| f.tag == trak_tag).is_none_or(|f| f.value != 0);
        let listed = features.len();
        features.retain(|f| f.tag != trak_tag);
        let stripped = features.len() != listed;
        let optical = match style.optical_size {
            OpticalSize::Fixed(o) if o.is_finite() && o > 0.0 && style.size > 0.0 => Some(o),
            _ => None,
        };
        let spacing = |trak: Option<Rc<Trak>>, size: f32| {
            let px = size * scale;
            let points = px / scale;
            // `o` itself at the style's size (a run's size comes back through device px).
            let points = optical.map_or(points, |o| {
                if (points - style.size).abs() <= style.size * 1e-6 { o } else { o * points / style.size }
            });
            style.tracking + trak.map_or(0.0, |t| t.spacing(px, points, tracked) / scale)
        };
        let base_spacing = spacing(self.primary_trak(style.family, style.weight, style.italic), style.size);
        let span_spacing: Vec<Option<f32>> = spans
            .iter()
            .map(|s| {
                if !(s.mono || s.italic || s.weight.is_some() || s.size.is_some()) {
                    return None;
                }
                let family = if s.mono { Family::Mono } else { style.family };
                let trak = self.primary_trak(family, s.weight.unwrap_or(style.weight), s.italic || style.italic);
                Some(spacing(trak, s.size.unwrap_or(style.size))).filter(|&v| v != base_spacing)
            })
            .collect();
        // Each style range's predicted first font at its weight, drawn at its named instance.
        let base_axes = self.primary_axes(style.family, style.weight, style.italic);
        let span_axes: Vec<Rc<[parley::FontVariation]>> = spans
            .iter()
            .map(|s| {
                let family = if s.mono { Family::Mono } else { style.family };
                self.primary_axes(family, s.weight.unwrap_or(style.weight), s.italic || style.italic)
            })
            .collect();
        let with_axes = |mut settings: Vec<parley::FontVariation>, axes: &[parley::FontVariation]| {
            let extra: Vec<_> = axes.iter().filter(|a| !settings.iter().any(|v| v.tag == a.tag)).copied().collect();
            settings.extend(extra);
            settings
        };
        if style.family == Family::SystemMono || self.slot_system_mono {
            self.system_mono();
        }
        if let Family::Named(list) = style.family {
            self.load_installed(list);
        }
        let math = self.math.as_deref().unwrap_or(&[]);
        let emoji = self.emoji.as_slice();
        let system = self.system_mono.as_deref().unwrap_or_default();
        let with_symbols = |primary| {
            family_list(primary, system, self.sans.as_str())
                .into_iter()
                .chain(self.symbol_families.iter().chain(math).map(|name| FontFamilyName::named(name)))
                .collect::<Vec<_>>()
        };
        let sans = match style.family {
            Family::Sans => with_symbols(self.sans.as_str()),
            Family::Mono => with_symbols(self.mono.as_str()),
            Family::Named(list) => {
                let mut families = with_symbols(self.sans.as_str());
                families.splice(0..0, named_list(list));
                families
            }
            Family::SystemMono => {
                let mut families = with_symbols(self.mono.as_str());
                families.splice(0..0, system.iter().map(|name| FontFamilyName::named(name)));
                families
            }
        };
        let mono = with_symbols(self.mono.as_str());
        let language = self.language;
        // Routed clusters try their fallback first, then the run's own stack (mono spans included).
        // Each of the four {math, emoji} × {sans, mono} stacks is built at most once.
        let mut stacks: [Option<Vec<FontFamilyName<'_>>>; 4] = Default::default();
        let mut routed = Vec::new();
        for (range, route) in routes {
            let fonts = match route { Route::Math => math, Route::Emoji => emoji };
            if fonts.is_empty() {
                continue;
            }
            let mono_span = spans.iter().any(|s| s.mono && (s.start as usize) < range.end && range.start < s.end as usize);
            let stack = 2 * (route == Route::Emoji) as usize + mono_span as usize;
            stacks[stack].get_or_insert_with(|| {
                let base = if mono_span { &mono } else { &sans };
                fonts.iter().map(|name| FontFamilyName::named(name)).chain(base.iter().cloned()).collect()
            });
            routed.push((range, stack));
        }
        let span_range = |s: &Span| (s.start as usize).min(text.len())..(s.end as usize).min(text.len());
        // `exact` overrides the predicted spacing with each run's own, by byte range.
        // Code never takes ligatures (`->` stays two characters); later features override earlier.
        let off = [b"liga", b"clig", b"calt", b"dlig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 0));
        let unligated: Vec<parley::FontFeature> = features.iter().chain(&off).copied().collect();
        type Spacings = [(std::ops::Range<usize>, f32)];
        let build = |lcx: &mut LayoutContext<Ink>, fcx: &mut FontContext, exact: &Spacings, widen: &Spacings, weights: &Spacings| {
            let mut b = lcx.ranged_builder(fcx, text, scale, true);
            b.set_base_direction(base);
            // Lines break where Chromium breaks them: after every run of spaces, even before a
            // closing bracket or comma (UAX #14 LB13 forbids that), and by Blink's ASCII pair
            // table elsewhere, so "number }" wraps before "}" rather than before "number".
            b.set_line_break_override(Some(parley::CHROMIUM_LINE_BREAK_OVERRIDE));
            b.push_default(FontFamily::from(sans.as_slice()));
            b.push_default(StyleProperty::Locale(language));
            b.push_default(StyleProperty::FontSize(style.size));
            b.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight)));
            if style.italic {
                b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
            }
            // Parley turns optional ligatures off under letter spacing; Core Text's tracking
            // keeps them, so spacing that only corrects `trak` turns them back on.
            let corrected = base_spacing != 0.0 || span_spacing.iter().flatten().chain(exact.iter().map(|e| &e.1)).any(|&v| v != 0.0);
            if mono_style {
                b.push_default(StyleProperty::FontFeatures(unligated.as_slice().into()));
            } else if style.tracking == 0.0 && corrected {
                let ligatures = [b"liga", b"clig"].map(|tag| parley::FontFeature::new(parley::setting::Tag::new(tag), 1));
                let list = ligatures.iter().chain(&features).copied().collect();
                b.push_default(StyleProperty::FontFeatures(parley::FontFeatures::List(std::borrow::Cow::Owned(list))));
            } else if stripped {
                b.push_default(StyleProperty::FontFeatures(features.as_slice().into()));
            } else if !style.features.is_empty() {
                b.push_default(StyleProperty::FontFeatures(style.features.into()));
            }
            let settings = with_axes(variations(style, style.size), &base_axes);
            if !settings.is_empty() {
                b.push_default(parley::FontVariations::List(std::borrow::Cow::Owned(settings)));
            }
            b.push_default(LineHeight::Absolute(style.line_px()));
            if wrap_anywhere {
                b.push_default(StyleProperty::OverflowWrap(parley::OverflowWrap::Anywhere));
            }
            if base_spacing != 0.0 {
                b.push_default(StyleProperty::LetterSpacing(base_spacing));
            }
            for (i, s) in spans.iter().enumerate() {
                let r = span_range(s);
                if r.is_empty() {
                    continue;
                }
                b.push(StyleProperty::Brush(Ink(i as u16 + 1)), r.clone());
                if s.mono {
                    b.push(FontFamily::from(mono.as_slice()), r.clone());
                    b.push(StyleProperty::FontFeatures(unligated.as_slice().into()), r.clone());
                }
                if let Some(w) = s.weight {
                    b.push(StyleProperty::FontWeight(FontWeight::new(w)), r.clone());
                }
                if s.italic {
                    b.push(StyleProperty::FontStyle(FontStyle::Italic), r.clone());
                }
                if let Some(sz) = s.size {
                    b.push(StyleProperty::FontSize(sz), r.clone());
                    // CSS's unitless line-height: a span of its own size (inline code, a smaller run) keeps the
                    // paragraph's leading ratio, so its box fits inside the line instead of growing it
                    b.push(LineHeight::FontSizeRelative(style.line_px() / style.size), r.clone());
                }
                let resized = s.size.is_some() && style.optical_size == OpticalSize::Auto;
                if resized || span_axes[i] != base_axes {
                    let settings = with_axes(variations(style, s.size.unwrap_or(style.size)), &span_axes[i]);
                    b.push(parley::FontVariations::List(std::borrow::Cow::Owned(settings)), r.clone());
                }
                if let Some(v) = span_spacing[i] {
                    b.push(StyleProperty::LetterSpacing(v), r.clone());
                }
            }
            for (range, w) in weights {
                b.push(StyleProperty::FontWeight(FontWeight::new(*w)), range.clone());
            }
            for (range, stack) in &routed {
                b.push(FontFamily::from(stacks[*stack].as_deref().unwrap_or_default()), range.clone());
            }
            for (range, v) in exact {
                b.push(StyleProperty::LetterSpacing(*v), range.clone());
            }
            for (range, v) in widen {
                b.push(StyleProperty::WordSpacing(*v), range.clone());
            }
            // Boxes before and after a pill's label make room for its chip. The label may break
            // like any text; `break_lines_with_blocks` moves a split pill whole to the next line
            // and lets it wrap only when it is wider than a line, as a browser wraps a span.
            for (i, r, lead, trail) in &pills {
                for (side, index, width) in [(0, r.start, *lead), (1, r.end, *trail)] {
                    b.push_inline_box(parley::InlineBox {
                        id: PILL_BOX | (*i as u64) << 1 | side,
                        kind: parley::InlineBoxKind::InFlow,
                        index,
                        width: width as f32,
                        height: 0.0,
                        baseline: None,
                        vertical_align: parley::VerticalAlign::BASELINE,
                    });
                }
            }
            let mut layout: Layout<Ink> = b.build(text);
            break_lines(&mut layout);
            layout
        };
        let mut layout = build(&mut self.layout_cx, &mut self.font_cx, &[], &widen, &[]);
        // On Windows, system faces as DirectWrite picks them for each run's weight, as Chromium.
        let weights = if cfg!(target_os = "windows") && !text.is_ascii() {
            let wanted = |at: usize| spans.iter().rev().find_map(|s| s.weight.filter(|_| span_range(s).contains(&at))).unwrap_or(style.weight);
            system_weight::corrections(&mut self.system_faces, &mut self.font_cx, &self.font_files, &layout, wanted)
        } else {
            Vec::new()
        };
        if !weights.is_empty() {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &[], &widen, &weights);
        }
        // The spacing each run got, against the spacing its own font needs.
        let given = |at: usize| {
            spans.iter().zip(&span_spacing).rev().find_map(|(s, v)| v.filter(|_| span_range(s).contains(&at))).unwrap_or(base_spacing)
        };
        let mut exact: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
        let mut mismatch = false;
        for line in layout.lines() {
            for run in line.runs() {
                let font = run.font();
                let trak = font_trak(&mut self.traks, &font.data, font.index);
                let needed = spacing(trak, run.font_size() / scale);
                let range = run.text_range();
                if range.is_empty() {
                    continue;
                }
                mismatch |= (needed - given(range.start)).abs() > 1e-4;
                match exact.last_mut() {
                    Some((last, v)) if last.end == range.start && *v == needed => last.end = range.end,
                    _ => exact.push((range, needed)),
                }
            }
        }
        if mismatch {
            layout = build(&mut self.layout_cx, &mut self.font_cx, &exact, &widen, &weights);
        }
        layout.align(align, AlignmentOptions::default());
        let l = Rc::new(layout);
        if cache {
            self.layouts.insert(key, Entry::new(l.clone(), g, text.len(), shape_key));
            self.shapes.insert(shape_key, key);
        }
        if let Some(shared) = shared {
            self.layout_cx = shared;
        }
        l
    }

    fn mono_tab_widening(
        &mut self,
        tabs: &[(usize, u32)],
        spans: &[Span],
        style: TextStyle,
        scale: f32,
    ) -> Vec<(std::ops::Range<usize>, f32)> {
        let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
        let default_family = if mono_style { style.family } else { Family::Mono };
        let mut last_probe: Option<((u32, u32, bool), f32)> = None;
        let mut widen: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
        for &(at, columns) in tabs {
            if columns <= 1 {
                continue;
            }
            let (mut sz, mut wt, mut it) = (style.size, style.weight, style.italic);
            for s in spans {
                if (s.start as usize) <= at && at < s.end as usize {
                    if let Some(v) = s.size { sz = v; }
                    if let Some(v) = s.weight { wt = v; }
                    if s.italic { it = true; }
                }
            }
            let probe_key = (sz.to_bits(), wt.to_bits(), it);
            let col_pt = match last_probe {
                Some((k, v)) if k == probe_key => v,
                _ => {
                    let probe = TextStyle { family: default_family, size: sz, weight: wt, italic: it, ..style };
                    let shaped = self.shaped;
                    let l = self.layout_rich(" ", &[], probe, scale, None);
                    self.shaped = shaped;
                    let adv = parley::Cluster::from_byte_index(&l, 0).map_or(0.0, |c| c.advance() / scale);
                    last_probe = Some((probe_key, adv));
                    adv
                }
            };
            if col_pt <= 0.0 {
                continue;
            }
            let add = (columns - 1) as f32 * col_pt;
            match widen.last_mut() {
                Some((r, last_add)) if r.end == at && (*last_add - add).abs() < 1e-4 => r.end = at + 1,
                _ => widen.push((at..at + 1, add)),
            }
        }
        widen
    }

    /// Size in points, rounded up to whole device pixels so layout stays on the pixel grid.
    pub fn measure(&mut self, text: &str, style: TextStyle, scale: f32) -> (f32, f32) {
        let l = self.layout_rich(text, &[], style, scale, None);
        (l.full_width().ceil() / scale, layout_height(&l, style, scale) / scale)
    }

    /// The atlas quad for `icon` at `px` device pixels square, top-left at `origin`.
    pub fn icon_quad(&mut self, icon: &'static Icon, px: u32, origin: [f32; 2]) -> Option<GlyphQuad> {
        self.icon_quad_facing(icon, px, origin, Facing::Upright)
    }

    /// [`Self::icon_quad`], drawn facing `facing` (`El::mirror_in_rtl` mirrors it).
    pub(crate) fn icon_quad_facing(&mut self, icon: &'static Icon, px: u32, origin: [f32; 2], facing: Facing) -> Option<GlyphQuad> {
        let icon = icon::swapped(icon);
        // A mirrored drawing is cached apart, under the size's top bit.
        let key = (icon as *const Icon as usize, px | (u32::from(facing == Facing::Mirrored) << 31));
        let entry = match self.atlas.icons.get(&key) {
            Some(&e) => {
                if let Some(a) = e {
                    self.atlas.touch(a.page, a.w * a.h);
                }
                e
            }
            None => match rasterize_icon(icon, px, facing, &mut self.atlas) {
                Rasterized::Placed(g) => {
                    self.atlas.icons.insert(key, Some(g));
                    Some(g)
                }
                Rasterized::Empty => {
                    self.atlas.icons.insert(key, None);
                    None
                }
                Rasterized::Unplaced => None,
            },
        };
        entry.map(|a| GlyphQuad {
            rect: [origin[0], origin[1], px as f32, px as f32],
            uv: [(a.x + 1) as f32, (a.page as u32 * ATLAS_SIZE + a.y + 1) as f32],
            ink: 0,
            byte: 0,
            color: 0,
            outline: None,
        })
    }
}

#[cfg(test)]
#[path = "text/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "text/quality_tests.rs"]
mod quality_tests;

#[cfg(test)]
#[path = "text/rewrap_tests.rs"]
mod rewrap_tests;
