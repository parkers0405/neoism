use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use neoism_paint::text::DrawOpts;
use neoism_paint::{GraphicOverlay};
use unicode_segmentation::UnicodeSegmentation;

use super::DEPTH;
use crate::panels::agent_pane::selection_model::{
    SelectableCaretStop, SelectionGeometry,
};
use crate::primitives::draw_text_with_occlusion;

/// Shared running indicator geometry: eight stepped stops, four white squares.
pub(crate) use crate::render_policy::{running_square_position, RUNNING_SQUARE_ALPHAS};

const TEXT_MEASURE_CACHE_LIMIT: usize = 8192;
const CARET_STOP_CACHE_LIMIT: usize = 8192;
const CARET_STOP_CACHE_POINTS_LIMIT: usize = 262_144;
// Applied independently to code and inline caches. Account source/display bytes,
// stop storage, Arc headers, and per-entry map/style/FIFO bookkeeping. Counting
// shared source/display allocations twice is deliberately conservative. Hash
// table capacity/allocator rounding is additionally bounded by the entry limit.
const CARET_STOP_CACHE_BYTES_LIMIT: usize = 8 * 1024 * 1024;

thread_local! {
    static TEXT_MEASURE_CACHE: RefCell<TextMeasureCache> = RefCell::new(TextMeasureCache::new());
    static CARET_STOP_CACHE: RefCell<CaretStopCache> = RefCell::new(CaretStopCache::new());
    static INLINE_SELECTION_CACHE: RefCell<CaretStopCache> = RefCell::new(CaretStopCache::new());
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TextMeasureKey {
    text: String,
    style: TextMeasureStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TextMeasureStyle {
    font_size_bits: u32,
    bold: bool,
    italic: bool,
    font_id: Option<usize>,
    scale_factor_bits: u32,
    font_generation: u64,
    // Inline Markdown historically accumulates f32 advances from screen x.
    // Keep that exact rounding, and invalidate those entries when x changes.
    inline_origin_bits: u32,
}

struct TextMeasureCache {
    values: HashMap<TextMeasureStyle, HashMap<String, f32>>,
    order: VecDeque<TextMeasureKey>,
}

type CaretStopValues = HashMap<Arc<str>, SelectionGeometry>;

struct CaretStopCacheKey {
    source: Arc<str>,
    style: TextMeasureStyle,
    retained_bytes: usize,
}

struct CaretStopCache {
    values: HashMap<TextMeasureStyle, CaretStopValues>,
    order: VecDeque<CaretStopCacheKey>,
    points: usize,
    retained_bytes: usize,
}

impl TextMeasureCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, text: &str, style: &TextMeasureStyle) -> Option<f32> {
        self.values.get(style)?.get(text).copied()
    }

    fn insert(&mut self, key: TextMeasureKey, value: f32) {
        let values = self.values.entry(key.style).or_default();
        if let Some(previous) = values.get_mut(key.text.as_str()) {
            *previous = value;
            return;
        }
        values.insert(key.text.clone(), value);
        self.order.push_back(key);
        while self.order.len() > TEXT_MEASURE_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                if let Some(values) = self.values.get_mut(&old.style) {
                    values.remove(old.text.as_str());
                    if values.is_empty() {
                        self.values.remove(&old.style);
                    }
                }
            }
        }
    }
}

impl CaretStopCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
            points: 0,
            retained_bytes: 0,
        }
    }

    fn get(&self, text: &str, style: &TextMeasureStyle) -> Option<SelectionGeometry> {
        self.values.get(style)?.get(text).cloned()
    }

    fn entry_bytes(source: &str, geometry: &SelectionGeometry) -> usize {
        // Charge a separate style bucket per entry, even when it is shared.
        // Also charge the three possible Arc allocation headers (source, text,
        // stops), both map slots, and the FIFO slot, not just visible glyphs.
        let bookkeeping = std::mem::size_of::<CaretStopCacheKey>()
            + std::mem::size_of::<(Arc<str>, SelectionGeometry)>()
            + std::mem::size_of::<(TextMeasureStyle, CaretStopValues)>()
            + 6 * std::mem::size_of::<usize>();
        source
            .len()
            .saturating_add(geometry.text.len())
            .saturating_add(
                geometry
                    .stops
                    .len()
                    .saturating_mul(std::mem::size_of::<SelectableCaretStop>()),
            )
            .saturating_add(bookkeeping)
    }

    fn remove(&mut self, key: CaretStopCacheKey) {
        if let Some(values) = self.values.get_mut(&key.style) {
            if let Some(geometry) = values.remove(key.source.as_ref()) {
                self.points -= geometry.stops.len();
                self.retained_bytes -= key.retained_bytes;
            }
            if values.is_empty() {
                self.values.remove(&key.style);
            }
        }
    }

    fn insert(
        &mut self,
        style: TextMeasureStyle,
        source: &str,
        geometry: SelectionGeometry,
    ) {
        let retained_bytes = Self::entry_bytes(source, &geometry);
        let stop_count = geometry.stops.len();
        // Replacing an entry must also replace its accounting/FIFO record.
        // Normal warm hits call get(), so this scan is only a duplicate insert.
        if self
            .values
            .get(&style)
            .is_some_and(|values| values.contains_key(source))
        {
            if let Some(index) = self
                .order
                .iter()
                .position(|key| key.style == style && key.source.as_ref() == source)
            {
                let old = self
                    .order
                    .remove(index)
                    .expect("existing caret cache FIFO entry");
                self.remove(old);
            }
        }
        // Do not retain huge hidden link destinations or huge plain code rows,
        // even as the sole cache entry. The caller still receives exact geometry.
        if retained_bytes > CARET_STOP_CACHE_BYTES_LIMIT
            || stop_count > CARET_STOP_CACHE_POINTS_LIMIT
        {
            return;
        }
        while self.order.len() >= CARET_STOP_CACHE_LIMIT
            || self.retained_bytes.saturating_add(retained_bytes)
                > CARET_STOP_CACHE_BYTES_LIMIT
            || self.points.saturating_add(stop_count) > CARET_STOP_CACHE_POINTS_LIMIT
        {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            self.remove(old);
        }
        // Plain code can share its rendered text too. Inline source differs
        // from the rendered label but is still allocated only once for map/FIFO.
        let source: Arc<str> = if source == geometry.text.as_ref() {
            geometry.text.clone()
        } else {
            source.into()
        };
        self.values
            .entry(style)
            .or_default()
            .insert(source.clone(), geometry);
        self.order.push_back(CaretStopCacheKey {
            source,
            style,
            retained_bytes,
        });
        self.points += stop_count;
        self.retained_bytes += retained_bytes;
    }
}

#[cfg(test)]
fn text_measure_key(text: &str, opts: &DrawOpts, scale_factor: f32) -> TextMeasureKey {
    TextMeasureKey {
        text: text.to_owned(),
        style: text_measure_style(opts, scale_factor),
    }
}

fn text_measure_style(opts: &DrawOpts, scale_factor: f32) -> TextMeasureStyle {
    TextMeasureStyle {
        font_size_bits: opts.font_size.to_bits(),
        bold: opts.bold,
        italic: opts.italic,
        font_id: opts.font_id,
        scale_factor_bits: scale_factor.to_bits(),
        font_generation: 0,
        inline_origin_bits: 0,
    }
}

pub fn measure_text_cached(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    opts: &DrawOpts,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    // Borrow the text on hits; animated frames repeatedly measure unchanged runs.
    let mut style = text_measure_style(opts, sugarloaf.scale_factor());
    style.font_generation = sugarloaf.text_mut().font_cache_generation();
    if let Some(value) = TEXT_MEASURE_CACHE.with(|cache| cache.borrow().get(text, &style))
    {
        return value;
    }
    let key = TextMeasureKey {
        text: text.to_owned(),
        style,
    };
    let measured = sugarloaf.text_mut().measure(text, opts);
    let value = if text.chars().all(char::is_whitespace) {
        // Some shapers omit glyphs for a whitespace-only run and report zero
        // width. Agent Markdown deliberately splits differently styled words
        // into runs, leaving their separator as exactly such a run. Recover
        // the current face's space advance from an in-context probe so the
        // following run cannot paint over the preceding colored/bold token.
        let separated = sugarloaf.text_mut().measure("M M", opts);
        let joined = sugarloaf.text_mut().measure("MM", opts);
        let cell = sugarloaf.text_mut().measure("M", opts);
        stable_whitespace_advance(
            measured,
            separated - joined,
            cell,
            opts.font_size,
            whitespace_columns(text),
        )
    } else {
        measured
    };
    TEXT_MEASURE_CACHE.with(|cache| cache.borrow_mut().insert(key, value));
    value
}

fn whitespace_columns(text: &str) -> usize {
    text.chars().map(|ch| if ch == '\t' { 4 } else { 1 }).sum()
}

fn stable_whitespace_advance(
    measured: f32,
    probed_space: f32,
    cell_advance: f32,
    font_size: f32,
    columns: usize,
) -> f32 {
    // Agent text is monospaced. If the backend strips a whitespace-only run,
    // a full current-face cell is the correct separator; the old 0.35em
    // fallback left inline-code/color boundaries visibly crowded.
    let space = probed_space.max(cell_advance).max(font_size * 0.5);
    measured.max(space * columns as f32)
}

#[cfg(test)]
mod measure_tests {
    use super::{stable_whitespace_advance, text_measure_key, whitespace_columns};
    use neoism_paint::text::DrawOpts;

    #[test]
    fn measurement_cache_identity_includes_font_and_scale() {
        let opts = DrawOpts::default();
        let base = text_measure_key("Yes", &opts, 1.0);
        let mut alternate_font = opts;
        alternate_font.font_id = Some(1);
        assert_ne!(base, text_measure_key("Yes", &alternate_font, 1.0));
        assert_ne!(base, text_measure_key("Yes", &opts, 2.0));
    }

    #[test]
    fn measurement_cache_borrows_text_and_separates_styles() {
        let mut cache = super::TextMeasureCache::new();
        let opts = DrawOpts::default();
        let key = text_measure_key("unchanged text", &opts, 1.0);
        let style = key.style;
        cache.insert(key, 12.0);
        assert_eq!(cache.get("unchanged text", &style), Some(12.0));
        assert_eq!(cache.get("changed text", &style), None);
        assert_eq!(
            cache.get("unchanged text", &super::text_measure_style(&opts, 2.0)),
            None
        );
        cache.insert(text_measure_key("unchanged text", &opts, 1.0), 15.0);
        assert_eq!(cache.get("unchanged text", &style), Some(15.0));
        assert_eq!(cache.order.len(), 1);
    }

    #[test]
    fn measurement_cache_eviction_removes_empty_style_buckets() {
        let mut cache = super::TextMeasureCache::new();
        let opts = DrawOpts::default();
        cache.insert(text_measure_key("old", &opts, 2.0), 1.0);
        for i in 0..super::TEXT_MEASURE_CACHE_LIMIT {
            cache.insert(text_measure_key(&i.to_string(), &opts, 1.0), i as f32);
        }
        assert_eq!(cache.values.len(), 1);
        assert_eq!(cache.order.len(), super::TEXT_MEASURE_CACHE_LIMIT);
        assert_eq!(
            cache.get("old", &super::text_measure_style(&opts, 2.0)),
            None
        );
        assert_eq!(
            cache.get("0", &super::text_measure_style(&opts, 1.0)),
            Some(0.0)
        );
    }

    #[test]
    fn retained_caret_cache_warm_hits_share_allocations_and_invalidate_identity() {
        use crate::panels::agent_pane::selection_model::{
            SelectableCaretStop, SelectionGeometry,
        };
        use std::sync::Arc;
        let mut cache = super::CaretStopCache::new();
        let style = super::text_measure_style(&DrawOpts::default(), 1.0);
        let geometry = SelectionGeometry {
            text: "e\u{301}\t界".into(),
            stops: vec![
                SelectableCaretStop {
                    byte_offset: 0,
                    x: 0.0,
                },
                SelectableCaretStop {
                    byte_offset: 3,
                    x: 8.0,
                },
                SelectableCaretStop {
                    byte_offset: 4,
                    x: 40.0,
                },
                SelectableCaretStop {
                    byte_offset: 7,
                    x: 56.0,
                },
            ]
            .into(),
            width: 56.0,
        };
        cache.insert(style, &geometry.text, geometry.clone());
        // An independently allocated lookup string exercises borrowed lookup.
        let query = geometry.text.to_string();
        for _ in 0..3 {
            let hit = cache.get(&query, &style).unwrap();
            assert!(Arc::ptr_eq(&hit.text, &geometry.text));
            assert!(Arc::ptr_eq(&hit.stops, &geometry.stops));
            assert_eq!(hit.width, 56.0);
        }
        let mut changed = style;
        changed.font_generation += 1;
        assert!(cache.get(&query, &changed).is_none());
        changed = style;
        changed.scale_factor_bits = 2.0f32.to_bits();
        assert!(cache.get(&query, &changed).is_none());
        changed = style;
        changed.font_id = Some(99);
        assert!(cache.get(&query, &changed).is_none());
        assert!(cache.get("different source", &style).is_none());
        changed = style;
        changed.inline_origin_bits = 17.5f32.to_bits();
        assert!(cache.get(&query, &changed).is_none());
        // Rendered text is not source identity: different Markdown styles can
        // paint the same text with different exact caret advances.
        cache.insert(style, "**source**", geometry.clone());
        assert!(cache.get("**source**", &style).is_some());
        assert!(cache.get("*source*", &style).is_none());
    }

    fn tiny_selection_geometry(
    ) -> crate::panels::agent_pane::selection_model::SelectionGeometry {
        use crate::panels::agent_pane::selection_model::{
            SelectableCaretStop, SelectionGeometry,
        };
        SelectionGeometry {
            text: "x".into(),
            stops: vec![
                SelectableCaretStop {
                    byte_offset: 0,
                    x: 0.0,
                },
                SelectableCaretStop {
                    byte_offset: 1,
                    x: 8.0,
                },
            ]
            .into(),
            width: 8.0,
        }
    }

    #[test]
    fn caret_cache_skips_oversized_invisible_markup_and_plain_source() {
        let mut cache = super::CaretStopCache::new();
        let mut style = super::text_measure_style(&DrawOpts::default(), 1.0);
        let geometry = tiny_selection_geometry();
        let source = format!("[x]({})", "u".repeat(super::CARET_STOP_CACHE_BYTES_LIMIT));
        // Only two stops, but raw Markdown (not its visible label) is charged.
        for origin in 0..3 {
            style.inline_origin_bits = (origin as f32).to_bits();
            cache.insert(style, &source, geometry.clone());
            assert!(cache.get(&source, &style).is_none());
            assert!(cache.values.is_empty());
            assert!(cache.order.is_empty());
            assert_eq!(cache.retained_bytes, 0);
            assert_eq!(cache.points, 0);
        }
        // The same policy applies to the plain-code cache, where source and
        // rendered text coincide. Sharing that Arc must not bypass the limit.
        let plain = super::SelectionGeometry {
            text: source.clone().into(),
            ..geometry
        };
        cache.insert(style, &source, plain);
        assert!(cache.order.is_empty());
        assert_eq!(cache.retained_bytes, 0);
    }

    #[test]
    fn caret_cache_bounds_origin_copies_and_shares_fifo_source() {
        use std::sync::Arc;
        let mut cache = super::CaretStopCache::new();
        let mut style = super::text_measure_style(&DrawOpts::default(), 1.0);
        let geometry = tiny_selection_geometry();
        let source = format!(
            "[x]({})",
            "u".repeat(super::CARET_STOP_CACHE_BYTES_LIMIT / 2)
        );
        for origin in 0..3 {
            style.inline_origin_bits = (origin as f32).to_bits();
            cache.insert(style, &source, geometry.clone());
            // Two independently keyed origins would exceed the byte budget.
            assert_eq!(cache.order.len(), 1);
            assert_eq!(cache.values.len(), 1);
            assert_eq!(
                cache.retained_bytes,
                super::CaretStopCache::entry_bytes(&source, &geometry)
            );
            assert!(cache.retained_bytes <= super::CARET_STOP_CACHE_BYTES_LIMIT);
            let (map_source, _) = cache
                .values
                .get(&style)
                .unwrap()
                .get_key_value(source.as_str())
                .unwrap();
            assert!(Arc::ptr_eq(
                map_source,
                &cache.order.front().unwrap().source
            ));
            let bytes = cache.retained_bytes;
            for _ in 0..3 {
                let hit = cache.get(&source, &style).unwrap();
                assert!(Arc::ptr_eq(&hit.text, &geometry.text));
                assert!(Arc::ptr_eq(&hit.stops, &geometry.stops));
                assert_eq!(cache.order.len(), 1);
                assert_eq!(cache.retained_bytes, bytes);
            }
            // Duplicate inserts replace accounting, not append another record.
            cache.insert(style, &source, geometry.clone());
            assert_eq!(cache.order.len(), 1);
            assert_eq!(cache.retained_bytes, bytes);
        }
        let old_style = style;
        let plain = tiny_selection_geometry();
        style.inline_origin_bits = 99.0f32.to_bits();
        cache.insert(style, "x", plain.clone());
        let (source, _) = cache
            .values
            .get(&style)
            .unwrap()
            .get_key_value("x")
            .unwrap();
        assert!(Arc::ptr_eq(source, &plain.text));
        assert!(cache.get("x", &old_style).is_none());
    }

    #[test]
    fn caret_cache_accounts_rendered_stops_and_replacements() {
        let mut cache = super::CaretStopCache::new();
        let style = super::text_measure_style(&DrawOpts::default(), 1.0);
        let small = tiny_selection_geometry();
        let small_bytes = super::CaretStopCache::entry_bytes("raw", &small);
        assert!(
            small_bytes
                > "raw".len()
                    + small.text.len()
                    + small.stops.len()
                        * std::mem::size_of::<super::SelectableCaretStop>()
        );
        let mut larger = small.clone();
        larger.text = "a larger rendered label".into();
        larger.stops = vec![small.stops[0]; 10].into();
        let larger_bytes = super::CaretStopCache::entry_bytes("raw", &larger);
        assert_eq!(
            larger_bytes - small_bytes,
            larger.text.len() - small.text.len()
                + (larger.stops.len() - small.stops.len())
                    * std::mem::size_of::<super::SelectableCaretStop>()
        );
        cache.insert(style, "raw", small);
        cache.insert(style, "raw", larger.clone());
        assert_eq!(cache.order.len(), 1);
        assert_eq!(cache.retained_bytes, larger_bytes);
        assert_eq!(cache.points, larger.stops.len());
        assert!(std::sync::Arc::ptr_eq(
            &cache.get("raw", &style).unwrap().text,
            &larger.text
        ));
        // Replacing that entry with an oversized rendered payload removes all
        // old bookkeeping and does not retain an over-budget replacement.
        larger.text = "x".repeat(super::CARET_STOP_CACHE_BYTES_LIMIT).into();
        cache.insert(style, "raw", larger);
        assert!(cache.values.is_empty());
        assert!(cache.order.is_empty());
        assert_eq!(cache.retained_bytes, 0);
        assert_eq!(cache.points, 0);
    }

    #[test]
    fn whitespace_columns_preserve_spaces_and_expand_tabs() {
        assert_eq!(whitespace_columns(" "), 1);
        assert_eq!(whitespace_columns("  \t"), 6);
    }

    #[test]
    fn whitespace_advance_survives_a_zero_width_shaper_run() {
        assert_eq!(stable_whitespace_advance(0.0, 0.0, 12.0, 20.0, 1), 12.0);
        assert_eq!(stable_whitespace_advance(0.0, 8.0, 12.0, 20.0, 2), 24.0);
    }
}

/// Compatibility adapter for renderers still accepting screen-space slices.
pub fn measured_caret_stops(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    opts: &DrawOpts,
    start_x: f32,
) -> Vec<SelectableCaretStop> {
    measured_selection_geometry(sugarloaf, text, opts)
        .stops
        .iter()
        .map(|stop| SelectableCaretStop {
            byte_offset: stop.byte_offset,
            x: stop.x + start_x,
        })
        .collect()
}

/// Retain inline geometry by raw source, not its marker-stripped display text.
/// The builder runs only on a cold style/font/source identity.
pub(super) fn cached_inline_selection_geometry(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    source: &str,
    opts: &DrawOpts,
    origin_x: f32,
    build: impl FnOnce(&mut dyn crate::primitives::paint::Paint) -> SelectionGeometry,
) -> SelectionGeometry {
    let mut style = text_measure_style(opts, sugarloaf.scale_factor());
    style.font_generation = sugarloaf.text_mut().font_cache_generation();
    style.inline_origin_bits = origin_x.to_bits();
    if let Some(hit) =
        INLINE_SELECTION_CACHE.with(|cache| cache.borrow().get(source, &style))
    {
        return hit;
    }
    let geometry = build(sugarloaf);
    INLINE_SELECTION_CACHE
        .with(|cache| cache.borrow_mut().insert(style, source, geometry.clone()));
    geometry
}

pub fn measured_selection_geometry(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    opts: &DrawOpts,
) -> SelectionGeometry {
    let mut style = text_measure_style(opts, sugarloaf.scale_factor());
    style.font_generation = sugarloaf.text_mut().font_cache_generation();
    if let Some(geometry) =
        CARET_STOP_CACHE.with(|cache| cache.borrow().get(text, &style))
    {
        return geometry;
    }
    let mut stops = vec![SelectableCaretStop {
        byte_offset: 0,
        x: 0.0,
    }];
    // Code tends to repeat a small alphabet across very long lines. Measure
    // each distinct grapheme once instead of allocating and looking up one
    // cache key per character on every frame.
    let mut widths = HashMap::<&str, f32>::new();
    let mut measured_sum = 0.0;
    for grapheme in text.graphemes(true) {
        let width = if let Some(width) = widths.get(grapheme) {
            *width
        } else {
            let width = measure_text_cached(sugarloaf, grapheme, opts);
            widths.insert(grapheme, width);
            width
        };
        measured_sum += width;
    }
    let full_width = measure_text_cached(sugarloaf, text, opts);
    let correction = if measured_sum > f32::EPSILON {
        full_width / measured_sum
    } else {
        1.0
    };
    let mut byte_offset = 0usize;
    let mut x = 0.0;
    for grapheme in text.graphemes(true) {
        let width = widths[grapheme];
        byte_offset += grapheme.len();
        x += width * correction;
        stops.push(SelectableCaretStop { byte_offset, x });
    }
    let geometry = SelectionGeometry {
        text: text.into(),
        stops: stops.into(),
        width: full_width,
    };
    CARET_STOP_CACHE
        .with(|cache| cache.borrow_mut().insert(style, text, geometry.clone()));
    geometry
}

pub fn opts_with_clip(mut opts: DrawOpts, clip: [f32; 4]) -> Option<DrawOpts> {
    opts.clip_rect = match opts.clip_rect {
        Some(existing) => intersect_rect(existing, clip),
        None => Some(clip),
    };
    opts.clip_rect.map(|_| opts)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_status_dot_text(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    x: f32,
    y: f32,
    diameter: f32,
    color: [u8; 4],
    halo: Option<([u8; 4], f32)>,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
    s: f32,
) {
    let dot = "●";
    let font_size = (diameter * 1.55).max(10.0 * s);
    if let Some((mut halo_color, halo_alpha)) = halo {
        halo_color[3] = ((halo_color[3] as f32) * halo_alpha.clamp(0.0, 1.0)) as u8;
        let halo_size = font_size * 1.65;
        let halo_opts = DrawOpts {
            font_size: halo_size,
            color: halo_color,
            bold: true,
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let halo_w = sugarloaf.text_mut().measure(dot, &halo_opts);
        draw_text_with_occlusion(
            sugarloaf,
            x + (diameter - halo_w) * 0.5,
            y + (diameter - halo_size) * 0.5 - 0.5 * s,
            dot,
            &halo_opts,
            occlusion_rects,
        );
    }

    let dot_opts = DrawOpts {
        font_size,
        color,
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let dot_w = sugarloaf.text_mut().measure(dot, &dot_opts);
    draw_text_with_occlusion(
        sugarloaf,
        x + (diameter - dot_w) * 0.5,
        y + (diameter - font_size) * 0.5 - 0.5 * s,
        dot,
        &dot_opts,
        occlusion_rects,
    );
}

pub fn draw_rect_clipped(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    rect: [f32; 4],
    color: [f32; 4],
    order: u8,
    clip: [f32; 4],
) {
    let Some([x, y, w, h]) = intersect_rect(rect, clip) else {
        return;
    };
    sugarloaf.rect(None, x, y, w, h, color, DEPTH, order);
}

pub fn draw_rounded_rect_clipped(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    rect: [f32; 4],
    color: [f32; 4],
    radius: f32,
    order: u8,
    clip: [f32; 4],
) {
    crate::widgets::quad::rounded_rect_clipped(
        sugarloaf, clip, None, rect, color, DEPTH, radius, order,
    );
}

pub fn draw_top_rounded_rect_clipped(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    rect: [f32; 4],
    color: [f32; 4],
    radius: f32,
    order: u8,
    clip: [f32; 4],
) {
    if intersect_rect(rect, clip).is_none() {
        return;
    }
    sugarloaf.quad_clipped(rect, color, [radius, radius, 0.0, 0.0], DEPTH, order, clip);
}

pub fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    (x2 > x1 && y2 > y1).then_some([x1, y1, x2 - x1, y2 - y1])
}

pub fn same_rect(a: [f32; 4], b: [f32; 4]) -> bool {
    (a[0] - b[0]).abs() < 0.01
        && (a[1] - b[1]).abs() < 0.01
        && (a[2] - b[2]).abs() < 0.01
        && (a[3] - b[3]).abs() < 0.01
}

pub fn draw_text_clipped(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let reveals: &[neoism_paint::text::TextReveal] = &[];
    let y = sugarloaf.snap_y(y);
    if occlusion_rects.is_empty() && reveals.is_empty() {
        return sugarloaf.text_mut().draw(x, y, text, opts);
    }
    // Keep this tied to the draw shaper rather than the process-global
    // measurement cache. Font state can change without changing DrawOpts.
    let width = sugarloaf.text_mut().measure(text, opts);
    if width <= 0.0 {
        return 0.0;
    }
    // Rasterized ink can overhang the advance-sum `width` (nerd-font
    // icons in the patched primary font raster ~1px wider than their
    // advance, and clip-edge snapping to device pixels eats up to
    // another) — a synthetic clip cut exactly at `width` shaved the
    // right diagonal off the chip chevrons (˅) whenever any occlusion
    // was active. Pad the fallback clip only; explicit `clip_rect`s
    // stay exact, and occlusion carving below still cuts overlaps.
    // Match the GPU's four-physical-pixel support plus bilinear footprint.
    let scale = sugarloaf.scale_factor().max(f32::EPSILON);
    let reveal_halo = |radius: f32| {
        if radius > 0.0 {
            ((radius * scale).min(4.0) + 1.0) / scale
        } else {
            0.0
        }
    };
    let halo = reveals
        .iter()
        .map(|r| reveal_halo(r.blur_radius))
        .fold(0.0_f32, f32::max);
    let ink_slack = 2.0 + halo;
    let base_clip = opts.clip_rect.unwrap_or([
        x - ink_slack,
        y - 4.0 - halo,
        width + 2.0 * ink_slack,
        opts.font_size * 1.8 + 2.0 * halo,
    ]);
    let text_h = (opts.font_size * 1.8).max(opts.font_size + 8.0) + 2.0 * halo;
    let text_rect = [
        x - ink_slack,
        y - 4.0 - halo,
        width + 2.0 * ink_slack,
        text_h,
    ];
    let mut intervals = vec![(base_clip[0], base_clip[0] + base_clip[2])];

    for rect in occlusion_rects {
        if !rects_intersect(text_rect, *rect) {
            continue;
        }
        let cut_start = rect[0].max(base_clip[0]);
        let cut_end = (rect[0] + rect[2]).min(base_clip[0] + base_clip[2]);
        if cut_end <= cut_start {
            continue;
        }
        let mut next = Vec::with_capacity(intervals.len() + 1);
        for (start, end) in intervals {
            if cut_end <= start || cut_start >= end {
                next.push((start, end));
            } else {
                if cut_start > start {
                    next.push((start, cut_start));
                }
                if cut_end < end {
                    next.push((cut_end, end));
                }
            }
        }
        intervals = next;
        if intervals.is_empty() {
            return width;
        }
    }

    for (start, end) in intervals {
        let clip_w = end - start;
        if clip_w <= 0.0 {
            continue;
        }
        let mut clipped = *opts;
        clipped.clip_rect = Some([start, base_clip[1], clip_w, base_clip[3]]);
        sugarloaf
            .text_mut()
            .draw(x, y, text, &clipped);
    }
    width
}

pub fn draw_text_revealed_clipped(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
    reveals: &[neoism_paint::text::TextReveal],
) -> f32 {
    if reveals.is_empty() { return draw_text_clipped(sugarloaf,x,y,text,opts,occlusion_rects); }
    let y = sugarloaf.snap_y(y);
    if occlusion_rects.is_empty() && reveals.is_empty() {
        return sugarloaf.text_mut().draw(x, y, text, opts);
    }
    // Keep this tied to the draw shaper rather than the process-global
    // measurement cache. Font state can change without changing DrawOpts.
    let width = sugarloaf.text_mut().measure(text, opts);
    if width <= 0.0 {
        return 0.0;
    }
    // Rasterized ink can overhang the advance-sum `width` (nerd-font
    // icons in the patched primary font raster ~1px wider than their
    // advance, and clip-edge snapping to device pixels eats up to
    // another) — a synthetic clip cut exactly at `width` shaved the
    // right diagonal off the chip chevrons (˅) whenever any occlusion
    // was active. Pad the fallback clip only; explicit `clip_rect`s
    // stay exact, and occlusion carving below still cuts overlaps.
    // Match the GPU's four-physical-pixel support plus bilinear footprint.
    let scale = sugarloaf.scale_factor().max(f32::EPSILON);
    let reveal_halo = |radius: f32| {
        if radius > 0.0 {
            ((radius * scale).min(4.0) + 1.0) / scale
        } else {
            0.0
        }
    };
    let halo = reveals
        .iter()
        .map(|r| reveal_halo(r.blur_radius))
        .fold(0.0_f32, f32::max);
    let ink_slack = 2.0 + halo;
    let base_clip = opts.clip_rect.unwrap_or([
        x - ink_slack,
        y - 4.0 - halo,
        width + 2.0 * ink_slack,
        opts.font_size * 1.8 + 2.0 * halo,
    ]);
    let text_h = (opts.font_size * 1.8).max(opts.font_size + 8.0) + 2.0 * halo;
    let text_rect = [
        x - ink_slack,
        y - 4.0 - halo,
        width + 2.0 * ink_slack,
        text_h,
    ];
    let mut intervals = vec![(base_clip[0], base_clip[0] + base_clip[2])];

    for rect in occlusion_rects {
        if !rects_intersect(text_rect, *rect) {
            continue;
        }
        let cut_start = rect[0].max(base_clip[0]);
        let cut_end = (rect[0] + rect[2]).min(base_clip[0] + base_clip[2]);
        if cut_end <= cut_start {
            continue;
        }
        let mut next = Vec::with_capacity(intervals.len() + 1);
        for (start, end) in intervals {
            if cut_end <= start || cut_start >= end {
                next.push((start, end));
            } else {
                if cut_start > start {
                    next.push((start, cut_start));
                }
                if cut_end < end {
                    next.push((cut_end, end));
                }
            }
        }
        intervals = next;
        if intervals.is_empty() {
            return width;
        }
    }

    // Redraw ownership belongs to visible active ink, not a message, overscan,
    // or a wholly covered row. Prefix measurement preserves shaped run layout.
    for reveal in reveals {
        let rx = x + measure_text_cached(sugarloaf, &text[..reveal.range.start], opts);
        let rw = measure_text_cached(sugarloaf, &text[reveal.range.clone()], opts);
        let halo = reveal_halo(reveal.blur_radius) + 2.0;
        let active_rect = [
            rx - halo,
            y - 4.0 - halo,
            rw + 2.0 * halo,
            opts.font_size * 1.8 + 2.0 * halo,
        ];
        if reveal_ink_visible(active_rect, base_clip, &intervals) {
            crate::panels::agent_pane::text_reveal::painted_visible(
                std::slice::from_ref(reveal),
            );
        }
    }
    for (start, end) in intervals {
        let clip_w = end - start;
        if clip_w <= 0.0 {
            continue;
        }
        let mut clipped = *opts;
        clipped.clip_rect = Some([start, base_clip[1], clip_w, base_clip[3]]);
        sugarloaf
            .text_mut()
            .draw_revealed(x, y, text, &clipped, reveals);
    }
    width
}

fn reveal_ink_visible(ink: [f32; 4], clip: [f32; 4], intervals: &[(f32, f32)]) -> bool {
    intersect_rect(ink, clip).is_some()
        && intervals
            .iter()
            .any(|(start, end)| ink[0] + ink[2] > *start && ink[0] < *end)
}

#[cfg(test)]
mod reveal_visibility_tests {
    use super::reveal_ink_visible;
    #[test]
    fn reveal_ownership_excludes_offscreen_and_fully_occluded_ink() {
        let clip = [0.0, 0.0, 100.0, 100.0];
        assert!(!reveal_ink_visible(
            [10.0, 200.0, 20.0, 18.0],
            clip,
            &[(0.0, 100.0)]
        ));
        assert!(!reveal_ink_visible(
            [10.0, 10.0, 20.0, 18.0],
            clip,
            &[(0.0, 10.0), (30.0, 100.0)]
        ));
        assert!(reveal_ink_visible(
            [10.0, 10.0, 20.0, 18.0],
            clip,
            &[(0.0, 20.0)]
        ));
        // A real blur halo crossing the viewport edge is still visible ink.
        assert!(reveal_ink_visible(
            [-3.0, 10.0, 6.0, 18.0],
            clip,
            &[(0.0, 100.0)]
        ));
    }
}

#[derive(Clone, Copy)]
pub struct ImagePiece {
    pub rect: [f32; 4],
    pub source_rect: [f32; 4],
}

#[allow(clippy::too_many_arguments)]
pub fn push_image_overlay_clipped(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    panel_id: usize,
    image_id: u32,
    rect: [f32; 4],
    source_rect: [f32; 4],
    z_index: i32,
    scale: f32,
    occlusion_rects: &[[f32; 4]],
) {
    let mut pieces = vec![ImagePiece { rect, source_rect }];
    for occlusion in occlusion_rects {
        let mut next = Vec::new();
        for piece in pieces {
            next.extend(subtract_image_piece(piece, *occlusion));
        }
        pieces = next;
        if pieces.is_empty() {
            return;
        }
    }

    for piece in pieces {
        let [x, y, w, h] = piece.rect;
        if w <= 0.5 || h <= 0.5 {
            continue;
        }
        sugarloaf.push_image_overlay(
            panel_id,
            GraphicOverlay {
                image_id,
                x: x * scale,
                y: y * scale,
                width: w * scale,
                height: h * scale,
                z_index,
                source_rect: piece.source_rect,
            },
        );
    }
}

pub fn subtract_image_piece(piece: ImagePiece, occlusion: [f32; 4]) -> Vec<ImagePiece> {
    let [x, y, w, h] = piece.rect;
    if w <= 0.0 || h <= 0.0 || !rects_intersect(piece.rect, occlusion) {
        return vec![piece];
    }
    let x2 = x + w;
    let y2 = y + h;
    let ox1 = occlusion[0].max(x);
    let oy1 = occlusion[1].max(y);
    let ox2 = (occlusion[0] + occlusion[2]).min(x2);
    let oy2 = (occlusion[1] + occlusion[3]).min(y2);
    if ox2 <= ox1 || oy2 <= oy1 {
        return vec![piece];
    }

    let [u0, v0, u1, v1] = piece.source_rect;
    let map_x = |px: f32| u0 + ((px - x) / w) * (u1 - u0);
    let map_y = |py: f32| v0 + ((py - y) / h) * (v1 - v0);
    let mut out = Vec::with_capacity(4);

    push_piece(&mut out, x, y, w, oy1 - y, u0, v0, u1, map_y(oy1));
    push_piece(&mut out, x, oy2, w, y2 - oy2, u0, map_y(oy2), u1, v1);
    push_piece(
        &mut out,
        x,
        oy1,
        ox1 - x,
        oy2 - oy1,
        u0,
        map_y(oy1),
        map_x(ox1),
        map_y(oy2),
    );
    push_piece(
        &mut out,
        ox2,
        oy1,
        x2 - ox2,
        oy2 - oy1,
        map_x(ox2),
        map_y(oy1),
        u1,
        map_y(oy2),
    );
    out
}

#[allow(clippy::too_many_arguments)]
pub fn push_piece(
    out: &mut Vec<ImagePiece>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
) {
    if w > 0.5 && h > 0.5 {
        out.push(ImagePiece {
            rect: [x, y, w, h],
            source_rect: [u0, v0, u1, v1],
        });
    }
}

pub fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    let (ax1, ay1, ax2, ay2) = (a[0], a[1], a[0] + a[2], a[1] + a[3]);
    let (bx1, by1, bx2, by2) = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
    ax1 < bx2 && ax2 > bx1 && ay1 < by2 && ay2 > by1
}

pub fn wrap_input_text(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    width: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            out.push(String::new());
            continue;
        }

        let mut current = String::new();
        for ch in paragraph.chars() {
            let mut candidate = current.clone();
            candidate.push(ch);
            if !current.is_empty()
                && measure_text_cached(sugarloaf, &candidate, opts) > width
            {
                out.push(current);
                current = ch.to_string();
            } else {
                current = candidate;
            }
        }
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

pub fn wrap_text(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    width: f32,
    opts: &DrawOpts,
    limit: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if measure_text_cached(sugarloaf, &candidate, opts) <= width || current.is_empty()
        {
            current = candidate;
        } else {
            lines.push(current);
            current = word.to_string();
            if lines.len() >= limit {
                break;
            }
        }
    }
    if !current.is_empty() && lines.len() < limit {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
