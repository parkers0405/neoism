//! The outline store: every glyph outline the GPU draws, in one texture of `[u32; 4]` texels
//! the renderer uploads by rows (`Renderer::prepare`), with CoreGraphics' hint transforms
//! beside them. It only grows within a frame; past its budget it empties at the next frame.

use std::collections::HashMap;

use super::build::Built;

/// Texels per row of the store's texture.
pub(crate) const WIDTH: u32 = 1024;
/// Rows the store may fill (64 MB of texels) before it starts again.
const MAX_ROWS: u32 = 4096;

/// A glyph outline: font, face index, glyph, variation coordinates, synthetic oblique, and the
/// ppem its TrueType hints ran at (f32 bits; 0 for the design outline).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub font: u64,
    pub index: u32,
    pub glyph: u32,
    pub coords: u64,
    pub skew: u8,
    pub hinted: u32,
}

/// Where a glyph's record starts, its ink box in font units (x0, y0, x1, y1, y up) and the
/// font's units per em.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Glyph {
    pub record: u32,
    pub bbox: [f32; 4],
    pub upem: f32,
}

/// What the renderer can do with outlines: its shader machine (render/variant.rs) as text sees
/// it, set at each upload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Support {
    /// No renderer has said: its shader may still become one that draws outlines.
    #[default]
    Untried,
    /// The renderer draws outline glyphs.
    Drawn,
    /// It can't: text draws from the atlas alone.
    Unavailable,
}

/// What [`OutlineStore::glyph`] found.
pub(crate) enum Found {
    Outline(Glyph),
    /// The glyph draws nothing (a space).
    Empty,
    /// No room this frame: draw it another way.
    Full,
}

pub(crate) struct OutlineStore {
    /// Whole rows of texels; `used` of them hold records. Texel 0 stays unused, so record 0
    /// means none.
    texels: Vec<[u32; 4]>,
    used: usize,
    glyphs: HashMap<Key, Option<Glyph>, std::hash::BuildHasherDefault<rune_motion::Fx>>,
    hints: HashMap<[u32; 11], u32>,
    /// Rows `[y0, y1)` changed since the last upload.
    dirty: Option<(u32, u32)>,
    /// A record didn't fit: start again at the next frame.
    full: bool,
    support: Support,
}

impl Default for OutlineStore {
    fn default() -> Self {
        OutlineStore { texels: vec![[0; 4]; WIDTH as usize], used: 1, glyphs: Default::default(), hints: Default::default(), dirty: None, full: false, support: Support::Untried }
    }
}

impl OutlineStore {
    /// The record of `key`, built by `build` the first time it's asked for.
    pub(crate) fn glyph(&mut self, key: Key, build: impl FnOnce() -> Option<Built>) -> Found {
        if let Some(g) = self.glyphs.get(&key) {
            return g.map_or(Found::Empty, Found::Outline);
        }
        let Some(built) = build() else {
            self.glyphs.insert(key, None);
            return Found::Empty;
        };
        let Some(record) = self.push(&built.texels, true) else { return Found::Full };
        let g = Glyph { record, bbox: built.bbox, upem: built.upem };
        self.glyphs.insert(key, Some(g));
        Found::Outline(g)
    }

    /// The record of a CoreGraphics hint transform (`HintTransform::values`), or 0 without room.
    pub(crate) fn hint(&mut self, values: [f64; 11]) -> u32 {
        let bits = values.map(|v| (v as f32).to_bits());
        if let Some(&at) = self.hints.get(&bits) {
            return at;
        }
        let texels = [[bits[0], bits[1], bits[2], bits[3]], [bits[4], bits[5], bits[6], bits[7]], [bits[8], bits[9], bits[10], 0]];
        let Some(at) = self.push(&texels, false) else { return 0 };
        self.hints.insert(bits, at);
        at
    }

    /// Appends `texels`, moving a glyph record's offsets (`relative`) to where it lands.
    fn push(&mut self, texels: &[[u32; 4]], relative: bool) -> Option<u32> {
        let at = self.used;
        let end = at + texels.len();
        if end > (MAX_ROWS * WIDTH) as usize {
            self.full = true;
            return None;
        }
        if end > self.texels.len() {
            self.texels.resize(end.next_multiple_of(WIDTH as usize), [0; 4]);
        }
        self.texels[at..end].copy_from_slice(texels);
        if relative {
            let bands = texels[0][1] as usize;
            for t in &mut self.texels[at..at + 1 + bands] {
                t[0] += at as u32;
            }
        }
        self.used = end;
        let rows = (at as u32 / WIDTH, (end as u32).div_ceil(WIDTH));
        let d = self.dirty.get_or_insert(rows);
        *d = (d.0.min(rows.0), d.1.max(rows.1));
        Some(at as u32)
    }

    /// Starts again when a record didn't fit last frame, unless glyphs drawn earlier must stay
    /// (`pinned`); whether it did. Records are valid until then.
    pub(crate) fn begin_frame(&mut self, pinned: bool) -> bool {
        if !self.full || pinned {
            return false;
        }
        self.full = false;
        self.used = 1;
        self.glyphs.clear();
        self.hints.clear();
        true
    }

    /// Texel `i`, zero past the end.
    pub(crate) fn texel(&self, i: u32) -> [u32; 4] {
        self.texels.get(i as usize).copied().unwrap_or_default()
    }

    /// Whether any glyph has been drawn from its outline.
    pub(crate) fn has_records(&self) -> bool { self.used > 1 }

    pub(crate) fn support(&self) -> Support { self.support }

    pub(crate) fn set_support(&mut self, support: Support) { self.support = support; }

    /// Rows the texture needs.
    pub(crate) fn rows(&self) -> u32 { (self.texels.len() / WIDTH as usize) as u32 }

    /// The changed rows and their texels, as bytes, since the last call.
    pub(crate) fn take_dirty(&mut self) -> Option<(u32, &[u8])> {
        let (y0, y1) = self.dirty.take()?;
        let rows = &self.texels[(y0 * WIDTH) as usize..(y1 * WIDTH) as usize];
        Some((y0, bytemuck::cast_slice(rows)))
    }

    /// Marks every row for upload (a new texture).
    pub(crate) fn invalidate(&mut self) {
        self.dirty = Some((0, self.rows()));
    }
}
