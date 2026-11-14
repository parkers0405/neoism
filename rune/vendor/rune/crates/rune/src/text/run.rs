//! [`Run`]: a string text cells each show one grapheme of, shaped once as a line.
use std::rc::Rc;

use parley::{Layout, PositionedLayoutItem};

use super::{Face, Ink, hash};
use crate::tok::TextStyle;

/// A string that several text cells each show one grapheme of (see [`El::advance_in`]), hashed
/// once so cells and frames can look up its shaped layout without rehashing it.
///
/// [`El::advance_in`]: crate::El::advance_in
#[derive(Clone, Debug)]
pub struct Run {
    text: Rc<str>,
    pub(crate) hash: u64,
}

impl Run {
    pub fn new(text: impl Into<Rc<str>>) -> Run {
        let text = text.into();
        Run { hash: hash::run_hash(&text), text }
    }
    pub fn as_str(&self) -> &str {
        &self.text
    }
    /// The run's cache key for one style and scale.
    pub(crate) fn key(&self, style: TextStyle, scale: f32) -> u64 {
        hash::run_key(self.hash, style, scale)
    }
}

pub(super) struct RunFace {
    pub(super) face: Face,
    pub(super) coords: Vec<i16>,
}

pub(super) struct RunGlyph {
    pub(super) face: u16,
    pub(super) id: u32,
    /// From the layout's left edge and its line's baseline (device px).
    pub(super) x: f32,
    pub(super) y: f32,
}

/// A [`Run`] shaped as one line, with each grapheme's offset and glyphs.
pub struct RunLayout {
    pub layout: Rc<Layout<Ink>>,
    /// Per grapheme: (byte, x on the 1/64 px grid element rects snap to, first glyph); then
    /// (len, end rounded up to a device pixel, glyph count).
    pub(super) graphemes: Vec<(u32, f32, u32)>,
    pub(super) faces: Vec<RunFace>,
    pub(super) glyphs: Vec<RunGlyph>,
}

impl RunLayout {
    pub(super) fn new(layout: Rc<Layout<Ink>>, text: &str) -> RunLayout {
        use unicode_segmentation::UnicodeSegmentation;
        let starts: Vec<u32> = text.grapheme_indices(true).map(|(i, _)| i as u32).collect();
        let grapheme_of = |byte: u32| starts.partition_point(|&s| s <= byte).saturating_sub(1);
        let mut xs = vec![f32::INFINITY; starts.len()];
        let mut faces = Vec::new();
        let mut glyphs: Vec<(usize, RunGlyph)> = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
                let mut coords = Vec::new();
                let face = Face::of(&glyph_run, &mut coords);
                faces.push(RunFace { face, coords });
                let face = faces.len() as u16 - 1;
                // Glyphs belong to the cluster they were shaped in (by count, not by x), so a
                // zero-advance mark stays with its base.
                let mut owners = glyph_run.run().visual_clusters().flat_map(|cl| {
                    std::iter::repeat_n(cl.text_range().start as u32, cl.glyphs().count())
                });
                let mut x = glyph_run.offset();
                for cl in glyph_run.run().visual_clusters() {
                    let gi = grapheme_of(cl.text_range().start as u32);
                    if let Some(g) = xs.get_mut(gi) {
                        *g = g.min(x);
                    }
                    x += cl.advance();
                }
                let mut rel = glyph_run.offset();
                for g in glyph_run.glyphs() {
                    let byte = owners.next().unwrap_or(0);
                    glyphs.push((grapheme_of(byte), RunGlyph { face, id: g.id, x: rel + g.x, y: g.y }));
                    rel += g.advance;
                }
            }
        }
        glyphs.sort_by_key(|g| g.0);
        let mut graphemes = Vec::with_capacity(starts.len() + 1);
        let mut prev = 0.0;
        for (i, &start) in starts.iter().enumerate() {
            let x = if xs[i].is_finite() { xs[i] } else { prev };
            prev = x;
            graphemes.push((start, (x * 64.0).round() / 64.0, glyphs.partition_point(|g| g.0 < i) as u32));
        }
        graphemes.push((text.len() as u32, (layout.full_width() - 1e-3).ceil(), glyphs.len() as u32));
        RunLayout { layout, graphemes, faces, glyphs: glyphs.into_iter().map(|g| g.1).collect() }
    }

    /// The index of the grapheme starting at byte `at` (never the end sentinel).
    pub(super) fn grapheme(&self, at: usize) -> Option<usize> {
        let i = self.graphemes.binary_search_by_key(&(at as u32), |e| e.0).ok()?;
        (i + 1 < self.graphemes.len()).then_some(i)
    }

    /// The x (device px) of the grapheme at byte `at`.
    pub fn x(&self, at: usize) -> Option<f32> {
        self.grapheme(at).map(|i| self.graphemes[i].1)
    }

    /// Width in points of the grapheme at byte `at`: the gap to the next grapheme, or for the
    /// last to the run's end rounded up as [`TextSystem::measure`](super::TextSystem::measure)
    /// does, so cells measured this way keep the run's kerning and sum to its measured width.
    pub fn advance(&self, at: usize, scale: f32) -> Option<f32> {
        let i = self.grapheme(at)?;
        Some(((self.graphemes[i + 1].1 - self.graphemes[i].1) / scale).max(0.0))
    }
}

impl super::TextSystem {
    /// `run` shaped as one line in `style`, cached as [`Self::end_commit`] keeps layouts.
    pub fn run_layout(&mut self, run: &Run, style: TextStyle, scale: f32) -> Rc<RunLayout> {
        let key = run.key(style, scale);
        let g = self.generation;
        if let Some(super::Entry { value: r, used, .. }) = self.runs.get_mut(&key) {
            *used = g;
            return r.clone();
        }
        let r = Rc::new(RunLayout::new(self.layout_rich(run.as_str(), &[], style, scale, None), run.as_str()));
        self.runs.insert(key, super::Entry::new(r.clone(), g, run.as_str().len(), 0));
        r
    }
}
