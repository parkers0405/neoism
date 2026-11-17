//! Read-only shaping geometry for document editors that keep their own source model.
use super::{Ink, Span, RichLayoutOptions, measure::with_text};
use crate::tok::TextStyle;
use parley::{Layout, editing::Cursor, layout::Affinity};
use std::{ops::Range, rc::Rc};

#[derive(Clone)]
pub struct Geometry {
    layout: Rc<Layout<Ink>>,
    len: usize,
}

/// Uses the same shared shaping cache as `rich`, including its inline font overrides.
pub fn geometry(value: &str, spans: &[Span], font: TextStyle, width: Option<f32>) -> Geometry {
    geometry_with(value, spans, font, width, RichLayoutOptions::default())
}

pub fn geometry_with(value: &str, spans: &[Span], font: TextStyle, width: Option<f32>, options: RichLayoutOptions) -> Geometry {
    with_text(|text| Geometry { layout: text.layout_rich_with(value, spans, font, 1.0, width, options), len: value.len() })
}

impl Geometry {
    pub fn line_ranges(&self) -> Vec<Range<usize>> {
        // Parley represents an empty paragraph with a synthetic character.
        let ranges: Vec<_> = self.layout.lines().map(|line| {
            let range = line.text_range();
            range.start.min(self.len)..range.end.min(self.len)
        }).collect();
        if ranges.is_empty() { vec![0..0] } else { ranges }
    }

    pub fn caret(&self, byte: usize) -> [f32; 4] {
        let cursor = Cursor::from_byte_index(&self.layout, byte.min(self.len), Affinity::Downstream);
        let rect = super::cursor::cursor_geometry(&cursor, &self.layout, 0.0);
        [rect.x0 as f32, rect.y0 as f32, (rect.x1 - rect.x0) as f32, (rect.y1 - rect.y0) as f32]
    }

    pub fn hit(&self, x: f32, y: f32) -> usize {
        super::cursor::cursor_from_point(&self.layout, x, y).index().min(self.len)
    }

    pub fn range_rects(&self, range: Range<usize>) -> Vec<[f32; 4]> {
        super::cursor::range_boxes(&self.layout, range.start.min(self.len), range.end.min(self.len), false).into_iter()
            .map(|(rect, _)| [rect.x0 as f32, rect.y0 as f32, (rect.x1 - rect.x0) as f32, (rect.y1 - rect.y0) as f32]).collect()
    }
}