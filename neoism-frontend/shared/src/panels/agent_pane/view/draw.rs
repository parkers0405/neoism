use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use sugarloaf::text::DrawOpts;
use sugarloaf::{GraphicOverlay, Sugarloaf};
use unicode_segmentation::UnicodeSegmentation;

use super::DEPTH;
use crate::panels::agent_pane::selection_model::SelectableCaretStop;
use crate::primitives::draw_text_with_occlusion;

const TEXT_MEASURE_CACHE_LIMIT: usize = 8192;
const CARET_STOP_CACHE_LIMIT: usize = 8192;
const CARET_STOP_CACHE_POINTS_LIMIT: usize = 262_144;

thread_local! {
    static TEXT_MEASURE_CACHE: RefCell<TextMeasureCache> = RefCell::new(TextMeasureCache::new());
    static CARET_STOP_CACHE: RefCell<CaretStopCache> = RefCell::new(CaretStopCache::new());
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TextMeasureKey {
    text: String,
    font_size_bits: u32,
    bold: bool,
    italic: bool,
    font_id: Option<usize>,
    scale_factor_bits: u32,
}

struct TextMeasureCache {
    values: HashMap<TextMeasureKey, f32>,
    order: VecDeque<TextMeasureKey>,
}

struct CaretStopCache {
    values: HashMap<TextMeasureKey, Vec<SelectableCaretStop>>,
    order: VecDeque<TextMeasureKey>,
    points: usize,
}

impl TextMeasureCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &TextMeasureKey) -> Option<f32> {
        self.values.get(key).copied()
    }

    fn insert(&mut self, key: TextMeasureKey, value: f32) {
        if self.values.contains_key(&key) {
            self.values.insert(key, value);
            return;
        }
        self.order.push_back(key.clone());
        self.values.insert(key, value);
        while self.order.len() > TEXT_MEASURE_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
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
        }
    }

    fn get(
        &self,
        key: &TextMeasureKey,
        start_x: f32,
    ) -> Option<Vec<SelectableCaretStop>> {
        self.values.get(key).map(|stops| {
            stops
                .iter()
                .map(|stop| SelectableCaretStop {
                    byte_offset: stop.byte_offset,
                    x: stop.x + start_x,
                })
                .collect()
        })
    }

    fn insert(&mut self, key: TextMeasureKey, stops: Vec<SelectableCaretStop>) {
        let stop_count = stops.len();
        if let Some(previous) = self.values.insert(key.clone(), stops) {
            self.points = self
                .points
                .saturating_sub(previous.len())
                .saturating_add(stop_count);
            return;
        }
        self.order.push_back(key.clone());
        self.points = self.points.saturating_add(stop_count);
        while self.order.len() > CARET_STOP_CACHE_LIMIT
            || (self.points > CARET_STOP_CACHE_POINTS_LIMIT && self.values.len() > 1)
        {
            if let Some(old) = self.order.pop_front() {
                if let Some(stops) = self.values.remove(&old) {
                    self.points = self.points.saturating_sub(stops.len());
                }
            }
        }
    }
}

fn text_measure_key(text: &str, opts: &DrawOpts, scale_factor: f32) -> TextMeasureKey {
    TextMeasureKey {
        text: text.to_owned(),
        font_size_bits: opts.font_size.to_bits(),
        bold: opts.bold,
        italic: opts.italic,
        font_id: opts.font_id,
        scale_factor_bits: scale_factor.to_bits(),
    }
}

pub fn measure_text_cached(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    opts: &DrawOpts,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let key = text_measure_key(text, opts, sugarloaf.scale_factor());
    if let Some(value) = TEXT_MEASURE_CACHE.with(|cache| cache.borrow().get(&key)) {
        return value;
    }
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
    use sugarloaf::text::DrawOpts;

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

pub fn measured_caret_stops(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    opts: &DrawOpts,
    start_x: f32,
) -> Vec<SelectableCaretStop> {
    if text.is_empty() {
        return vec![SelectableCaretStop {
            byte_offset: 0,
            x: start_x,
        }];
    }
    let key = text_measure_key(text, opts, sugarloaf.scale_factor());
    if let Some(stops) = CARET_STOP_CACHE.with(|cache| cache.borrow().get(&key, start_x))
    {
        return stops;
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
    CARET_STOP_CACHE.with(|cache| cache.borrow_mut().insert(key, stops.clone()));
    for stop in &mut stops {
        stop.x += start_x;
    }
    stops
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
    sugarloaf: &mut Sugarloaf,
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
    sugarloaf: &mut Sugarloaf,
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
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    color: [f32; 4],
    radius: f32,
    order: u8,
    clip: [f32; 4],
) {
    crate::widgets::quad::rounded_rect_clipped(
        sugarloaf, clip, None, rect, color, DEPTH, radius, order, 0.01,
    );
}

pub fn draw_top_rounded_rect_clipped(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    color: [f32; 4],
    radius: f32,
    order: u8,
    clip: [f32; 4],
) {
    let Some(visible) = intersect_rect(rect, clip) else {
        return;
    };
    if same_rect(visible, rect) {
        let [x, y, w, h] = rect;
        sugarloaf.rounded_rect(None, x, y, w, h, color, DEPTH, radius, order);
        draw_rect_clipped(
            sugarloaf,
            [x, y + h - radius, w, radius],
            color,
            order + 1,
            clip,
        );
    } else {
        let [x, y, w, h] = visible;
        sugarloaf.rect(None, x, y, w, h, color, DEPTH, order);
    }
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
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
) {
    let y = snap_text_y(y);
    if occlusion_rects.is_empty() {
        sugarloaf.text_mut().draw(x, y, text, opts);
        return;
    }
    let width = measure_text_cached(sugarloaf, text, opts);
    if width <= 0.0 {
        return;
    }
    // Rasterized ink can overhang the advance-sum `width` (nerd-font
    // icons in the patched primary font raster ~1px wider than their
    // advance, and clip-edge snapping to device pixels eats up to
    // another) — a synthetic clip cut exactly at `width` shaved the
    // right diagonal off the chip chevrons (˅) whenever any occlusion
    // was active. Pad the fallback clip only; explicit `clip_rect`s
    // stay exact, and occlusion carving below still cuts overlaps.
    let ink_slack = 2.0;
    let base_clip = opts.clip_rect.unwrap_or([
        x - ink_slack,
        y - 4.0,
        width + 2.0 * ink_slack,
        opts.font_size * 1.8,
    ]);
    let text_h = (opts.font_size * 1.8).max(opts.font_size + 8.0);
    let text_rect = [x - ink_slack, y - 4.0, width + 2.0 * ink_slack, text_h];
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
            return;
        }
    }

    for (start, end) in intervals {
        let clip_w = end - start;
        if clip_w <= 0.0 {
            continue;
        }
        let mut clipped = *opts;
        clipped.clip_rect = Some([start, base_clip[1], clip_w, base_clip[3]]);
        sugarloaf.text_mut().draw(x, y, text, &clipped);
    }
}

fn snap_text_y(y: f32) -> f32 {
    if y.is_finite() {
        y.round()
    } else {
        y
    }
}

#[derive(Clone, Copy)]
pub struct ImagePiece {
    pub rect: [f32; 4],
    pub source_rect: [f32; 4],
}

#[allow(clippy::too_many_arguments)]
pub fn push_image_overlay_clipped(
    sugarloaf: &mut Sugarloaf,
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
    sugarloaf: &mut Sugarloaf,
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
    sugarloaf: &mut Sugarloaf,
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
