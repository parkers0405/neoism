//! Exact retained advances for diff tokens and line-number gutters.

use super::{code_font_opts, DrawOpts, SynTok};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Weak};

type FontIdentity = Weak<dyn std::any::Any>;

fn font_identity(sugarloaf: &dyn crate::primitives::paint::Paint) -> FontIdentity {
    let library: Arc<dyn std::any::Any> = sugarloaf.font_library().inner.clone();
    Arc::downgrade(&library)
}

const LINE_LIMIT: usize = 512;
const LINE_BYTES: usize = 4 * 1024 * 1024;
// Keep many visible rows resident; one large line must not flush its neighbors.
const MAX_LINE_BYTES: usize = 16 * 1024;
const GUTTER_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct FontKey {
    library: usize,
    generation: u64,
    dpi: u32,
    scale: u32,
}

impl FontKey {
    fn new(sugarloaf: &mut dyn crate::primitives::paint::Paint, scale: f32) -> Self {
        let library = Arc::as_ptr(&sugarloaf.font_library().inner) as usize;
        Self {
            library,
            generation: sugarloaf.text_mut().font_cache_generation(),
            dpi: sugarloaf.scale_factor().to_bits(),
            scale: scale.to_bits(),
        }
    }
}

#[derive(Default)]
struct PositionedAdvances {
    start: Option<u32>,
    points: Rc<Vec<f32>>,
}

impl PositionedAdvances {
    fn get(&mut self, advances: &[f32], start: f32) -> Rc<Vec<f32>> {
        if self.start != Some(start.to_bits()) {
            let points = Rc::make_mut(&mut self.points);
            points.clear();
            points.reserve(advances.len() + 1);
            let mut x = start;
            points.push(x);
            // Preserve the original accumulation order, including its exact
            // float rounding when the line is translated/horizontally scrolled.
            for advance in advances {
                x += advance;
                points.push(x);
            }
            self.start = Some(start.to_bits());
        }
        self.points.clone()
    }
}

pub(super) struct PreparedSyntaxLine {
    _spans: Rc<Vec<(SynTok, String)>>,
    pub advances: Vec<f32>,
    pub width: f32,
    pub monotonic: bool,
    positions: RefCell<PositionedAdvances>,
    // Pin allocation identity without retaining old font data after reload.
    _library: FontIdentity,
    bytes: usize,
}

impl PreparedSyntaxLine {
    pub fn positions(&self, start: f32) -> Rc<Vec<f32>> {
        self.positions.borrow_mut().get(&self.advances, start)
    }
}

#[derive(Default)]
struct LineCache {
    values: HashMap<(FontKey, usize), Rc<PreparedSyntaxLine>>,
    order: VecDeque<(FontKey, usize)>,
    bytes: usize,
}

struct GutterEntry {
    width: f32,
    _library: FontIdentity,
}

#[derive(Default)]
struct GutterCache {
    values: HashMap<(FontKey, usize), GutterEntry>,
    order: VecDeque<(FontKey, usize)>,
}

thread_local! {
    static LINES: RefCell<LineCache> = RefCell::new(LineCache::default());
    static GUTTERS: RefCell<GutterCache> = RefCell::new(GutterCache::default());
}

fn cacheable_line_bytes(spans: &Vec<(SynTok, String)>) -> Option<usize> {
    let bytes = spans.iter().map(|(_, text)| text.capacity()).sum::<usize>()
        + spans.capacity() * std::mem::size_of::<(SynTok, String)>()
        + spans.len() * 12
        + 32
        + std::mem::size_of::<PreparedSyntaxLine>();
    (bytes <= MAX_LINE_BYTES).then_some(bytes)
}

pub(super) fn syntax_line(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    spans: Rc<Vec<(SynTok, String)>>,
    scale: f32,
) -> Option<Rc<PreparedSyntaxLine>> {
    let key = (FontKey::new(sugarloaf, scale), Rc::as_ptr(&spans) as usize);
    if let Some(hit) = LINES.with(|cache| cache.borrow().values.get(&key).cloned()) {
        return Some(hit);
    }
    let bytes = cacheable_line_bytes(&spans)?;
    // Oversized lines retain the original viewport-limited measuring path;
    // never premeasure an entire uncacheable line on every animated frame.
    let opts = code_font_opts(scale, false);
    let advances: Vec<f32> = spans
        .iter()
        .map(|(_, text)| sugarloaf.text_mut().measure(text, &opts))
        .collect();
    let line = Rc::new(PreparedSyntaxLine {
        width: advances.iter().sum(),
        monotonic: advances
            .iter()
            .all(|advance| advance.is_finite() && *advance >= 0.0),
        _spans: spans,
        advances,
        positions: RefCell::new(PositionedAdvances::default()),
        _library: font_identity(sugarloaf),
        bytes,
    });
    LINES.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.bytes += bytes;
        cache.order.push_back(key);
        cache.values.insert(key, line.clone());
        while cache.order.len() > LINE_LIMIT || cache.bytes > LINE_BYTES {
            let Some(old) = cache.order.pop_front() else {
                break;
            };
            if let Some(old) = cache.values.remove(&old) {
                cache.bytes -= old.bytes;
            }
        }
    });
    Some(line)
}

pub(super) fn gutter_width(sugarloaf: &mut dyn crate::primitives::paint::Paint, max_line: u32, scale: f32) -> f32 {
    let digits = max_line.max(1).ilog10() as usize + 1;
    let key = (FontKey::new(sugarloaf, scale), digits);
    if let Some(width) =
        GUTTERS.with(|cache| cache.borrow().values.get(&key).map(|v| v.width))
    {
        return width;
    }
    let opts = DrawOpts {
        font_size: super::GUTTER_FONT_SIZE * scale,
        bold: true,
        ..DrawOpts::default()
    };
    let measured = (b'0'..=b'9')
        .map(|digit| {
            sugarloaf
                .text_mut()
                .measure(&char::from(digit).to_string().repeat(digits), &opts)
        })
        .fold(0.0_f32, f32::max);
    let width = super::gutter_width_for_text(measured, scale);
    GUTTERS.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.order.push_back(key);
        cache.values.insert(
            key,
            GutterEntry {
                width,
                _library: font_identity(sugarloaf),
            },
        );
        while cache.order.len() > GUTTER_LIMIT {
            if let Some(old) = cache.order.pop_front() {
                cache.values.remove(&old);
            }
        }
    });
    width
}

pub(super) fn first_visible_token(points: &[f32], left: f32) -> usize {
    points
        .get(1..)
        .unwrap_or_default()
        .partition_point(|end| *end <= left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_lines_cannot_evict_one_another_and_trigger_full_measurement_each_frame() {
        let first = vec![(SynTok::Plain, "x".repeat(LINE_BYTES / 2 + 1))];
        let second = vec![(SynTok::Plain, "y".repeat(LINE_BYTES / 2 + 1))];
        assert!(cacheable_line_bytes(&first).is_none());
        assert!(cacheable_line_bytes(&second).is_none());
        assert!(
            cacheable_line_bytes(&vec![(SynTok::Plain, "let x = 1;".into())]).is_some()
        );
        let empty_with_large_allocation =
            vec![(SynTok::Plain, String::with_capacity(MAX_LINE_BYTES))];
        assert!(cacheable_line_bytes(&empty_with_large_allocation).is_none());
        assert!(LINE_BYTES / MAX_LINE_BYTES >= 256);
    }

    #[test]
    fn cached_token_positions_preserve_original_rounding_and_share_warm_storage() {
        let advances: Vec<f32> =
            (0..2_000).map(|n| (n % 11) as f32 * 0.1234567).collect();
        let mut cache = PositionedAdvances::default();
        for start in [-9000.125, 0.0, 13.75, 1.234567] {
            let actual = cache.get(&advances, start);
            let mut x = start;
            assert_eq!(actual[0].to_bits(), x.to_bits());
            for (index, advance) in advances.iter().enumerate() {
                x += advance;
                assert_eq!(actual[index + 1].to_bits(), x.to_bits());
            }
            for _ in 0..100 {
                assert!(Rc::ptr_eq(&actual, &cache.get(&advances, start)));
            }
        }
    }

    #[test]
    fn translating_a_line_does_not_mutate_retained_geometry() {
        let mut cache = PositionedAdvances::default();
        let old = cache.get(&[1.0, 2.0], 10.0);
        let moved = cache.get(&[1.0, 2.0], -20.0);
        assert_eq!(*old, [10.0, 11.0, 13.0]);
        assert_eq!(*moved, [-20.0, -19.0, -17.0]);
    }

    #[test]
    fn horizontal_seek_matches_the_original_token_intersection_rule() {
        let points = [-10.0, -5.0, 0.0, 0.0, 8.0, 15.0];
        for left in [-20.0, -5.0, 0.0, 1.0, 8.0, 16.0] {
            let expected = points[1..]
                .iter()
                .position(|end| *end > left)
                .unwrap_or(points.len() - 1);
            assert_eq!(first_visible_token(&points, left), expected);
        }
    }
}
