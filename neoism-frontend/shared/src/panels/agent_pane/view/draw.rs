use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use sugarloaf::text::DrawOpts;
use sugarloaf::{GraphicOverlay, Sugarloaf};

use super::DEPTH;

const TEXT_MEASURE_CACHE_LIMIT: usize = 8192;

thread_local! {
    static TEXT_MEASURE_CACHE: RefCell<TextMeasureCache> = RefCell::new(TextMeasureCache::new());
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TextMeasureKey {
    text: String,
    font_size_bits: u32,
    bold: bool,
    italic: bool,
}

struct TextMeasureCache {
    values: HashMap<TextMeasureKey, f32>,
    order: VecDeque<TextMeasureKey>,
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

pub fn measure_text_cached(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    opts: &DrawOpts,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let key = TextMeasureKey {
        text: text.to_owned(),
        font_size_bits: opts.font_size.to_bits(),
        bold: opts.bold,
        italic: opts.italic,
    };
    if let Some(value) = TEXT_MEASURE_CACHE.with(|cache| cache.borrow().get(&key)) {
        return value;
    }
    let value = sugarloaf.text_mut().measure(text, opts);
    TEXT_MEASURE_CACHE.with(|cache| cache.borrow_mut().insert(key, value));
    value
}

pub fn opts_with_clip(mut opts: DrawOpts, clip: [f32; 4]) -> Option<DrawOpts> {
    opts.clip_rect = match opts.clip_rect {
        Some(existing) => intersect_rect(existing, clip),
        None => Some(clip),
    };
    opts.clip_rect.map(|_| opts)
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
    let base_clip = opts
        .clip_rect
        .unwrap_or([x, y - 4.0, width, opts.font_size * 1.8]);
    let text_h = (opts.font_size * 1.8).max(opts.font_size + 8.0);
    let text_rect = [x, y - 4.0, width, text_h];
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
