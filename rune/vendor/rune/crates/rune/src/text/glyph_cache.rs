//! Each painted paragraph's glyphs, flattened out of Parley once, so a frame where the text
//! didn't change replays them instead of walking Parley's runs and looking up every glyph's
//! atlas mask again.
//!
//! An entry belongs to one `Rc<Layout>` (held weakly, so its address can't be reused while the
//! entry lives), one raster factor, the ink buckets that choose dilation and the pipeline. It
//! keeps per line each glyph's pen inputs and the atlas mask it last drew at its phases. A
//! frame recomputes every position from the origin with the same arithmetic as
//! [`TextSystem::glyphs_in_band`], so quads come out bit for bit the same; a glyph whose phase
//! moved, or any glyph after the atlas evicts a page, asks the atlas again.
use std::rc::Weak;

use super::glyph_emit::{DIRECT_WRITE_KEY, Placed, WINDFOIL_KEY, atlas_quad, place};
use super::*;

/// Bytes all entries may hold before the least recently painted go.
const MAX_BYTES: usize = 8 << 20;
/// Entries kept at most.
const MAX_ENTRIES: usize = 4096;
/// One paragraph's share: past it, an entry drops its lines and starts again from what shows.
const ENTRY_BYTES: usize = 2 << 20;
/// A line not flattened yet.
const UNFLAT: u32 = u32::MAX;

const UNKNOWN: u8 = 0;
const PLACED: u8 = 1;
const EMPTY: u8 = 2;
const NO_MASK: AtlasGlyph = AtlasGlyph { page: 0, x: 0, y: 0, w: 0, h: 0, left: 0, top: 0 };

#[derive(Default)]
pub(crate) struct GlyphCache {
    entries: HashMap<Key, Entry, std::hash::BuildHasherDefault<rune_motion::Fx>>,
    bytes: usize,
    frame: u64,
    /// The buffers of an entry whose layout went this frame, for the next new entry to grow
    /// into. A streamed text's new layout replaces its last each commit, and its lines come
    /// into view over the frames after, so they flatten into room it already has.
    spare: Option<Entry>,
    /// Lines flattened out of Parley, a running count.
    pub(crate) flattened: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    layout: usize,
    inks: u64,
    apple: u8,
}

#[derive(Default)]
struct Entry {
    layout: Weak<Layout<Ink>>,
    inks: Vec<u8>,
    k: u32,
    bytes: bool,
    /// The atlas clock the masks below were looked up under.
    clock: u64,
    used: u64,
    /// Per line, its glyphs' range (`UNFLAT` until it first paints).
    lines: Vec<(u32, u32)>,
    glyphs: Vec<Glyph>,
    /// One per glyph run, with its normalized coordinates' range in `coords`.
    faces: Vec<(Face, u32, u32)>,
    coords: Vec<i16>,
}

#[derive(Clone, Copy)]
struct Glyph {
    id: u32,
    /// The run's pen before this glyph and the glyph's own offset (device px, before `k`).
    rel: f32,
    x: f32,
    y: f32,
    byte: u32,
    face: u16,
    /// The x and y phases its mask was looked up at.
    sub: [u8; 2],
    state: u8,
    mask: AtlasGlyph,
}

impl Entry {
    fn size(&self) -> usize {
        size_of::<Self>() + size_of::<Layout<Ink>>() + self.inks.capacity()
            + self.lines.capacity() * size_of::<(u32, u32)>()
            + self.glyphs.capacity() * size_of::<Glyph>()
            + self.faces.capacity() * size_of::<(Face, u32, u32)>()
            + self.coords.capacity() * size_of::<i16>()
    }

    fn clear(&mut self, lines: usize) {
        self.lines.clear();
        self.lines.resize(lines, (UNFLAT, UNFLAT));
        self.glyphs.clear();
        self.faces.clear();
        self.coords.clear();
    }

    /// Records line `i`'s glyphs, as [`TextSystem::glyphs_in_band`] walks them.
    fn flatten(&mut self, layout: &Layout<Ink>, i: usize, table: &mut Vec<(f32, u32)>, coords: &mut Vec<i16>) {
        let Some(line) = layout.get(i) else { return };
        let start = self.glyphs.len() as u32;
        if self.bytes {
            table.clear();
            let mut x = line.metrics().offset;
            for run in line.runs() {
                for cl in run.visual_clusters() {
                    table.push((x, cl.text_range().start as u32));
                    x += cl.advance();
                }
            }
        }
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
            let face = Face::of(&glyph_run, coords);
            let at = self.coords.len() as u32;
            self.coords.extend_from_slice(coords);
            self.faces.push((face, at, self.coords.len() as u32));
            let face = (self.faces.len() - 1) as u16;
            let mut rel = glyph_run.offset();
            for g in glyph_run.glyphs() {
                let byte = if self.bytes {
                    let i = table.partition_point(|&(x0, _)| x0 <= rel + 0.01);
                    table.get(i.saturating_sub(1)).map_or(0, |e| e.1)
                } else {
                    0
                };
                self.glyphs.push(Glyph { id: g.id, rel, x: g.x, y: g.y, byte, face, sub: [0; 2], state: UNKNOWN, mask: NO_MASK });
                rel += g.advance;
            }
        }
        self.lines[i] = (start, self.glyphs.len() as u32);
    }
}

impl GlyphCache {
    /// Drops `layout`'s entries, so a layout re-broken in place stays at its address
    /// (`Rc::make_mut` moves a value that weak references still point to).
    pub(crate) fn forget(&mut self, layout: &Rc<Layout<Ink>>) {
        if Rc::weak_count(layout) == 0 {
            return;
        }
        let at = Rc::as_ptr(layout) as usize;
        for (_, e) in self.entries.extract_if(|k, _| k.layout == at) {
            retire(&mut self.spare, e);
        }
        self.bytes = self.entries.values().map(Entry::size).sum();
    }

    /// Drops every entry, for layouts shaped in fonts that no longer apply.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.spare = None;
        self.bytes = 0;
    }

    /// Drops entries whose layout is gone, then the least recently painted while over budget.
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
        self.spare = None;
        let before = self.entries.len();
        for (_, e) in self.entries.extract_if(|_, e| e.layout.strong_count() == 0) {
            retire(&mut self.spare, e);
        }
        if self.entries.len() != before {
            self.bytes = self.entries.values().map(Entry::size).sum();
        }
        if self.bytes <= MAX_BYTES && self.entries.len() <= MAX_ENTRIES {
            return;
        }
        let mut order: Vec<(u64, Key)> = self.entries.iter().map(|(k, e)| (e.used, *k)).collect();
        order.sort_unstable_by_key(|e| e.0);
        for (_, key) in order {
            if self.bytes <= MAX_BYTES * 3 / 4 && self.entries.len() <= MAX_ENTRIES * 3 / 4 {
                break;
            }
            if let Some(e) = self.entries.remove(&key) {
                self.bytes -= e.size();
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize { self.entries.len() }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize { self.bytes }
}

/// Keeps the roomiest of the entries that went this frame, up to one paragraph's share, and
/// lets go of its layout, so one re-broken in place stays where it is.
fn retire(spare: &mut Option<Entry>, mut e: Entry) {
    let room = |e: &Entry| e.glyphs.capacity();
    if room(&e) * size_of::<Glyph>() <= ENTRY_BYTES && spare.as_ref().is_none_or(|s| room(s) < room(&e)) {
        e.layout = Weak::new();
        *spare = Some(e);
    }
}

impl TextSystem {
    /// [`Self::glyphs_in_band`] for a layout a node paints frame after frame: the first frame
    /// flattens the lines it shows, later ones replay them. Same quads, same order.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn glyphs_in_band_cached(
        &mut self,
        layout: &Rc<Layout<Ink>>,
        origin: [f32; 2],
        k: f32,
        bytes: bool,
        band: std::ops::Range<f32>,
        out: &mut Vec<GlyphQuad>,
    ) {
        let mut cache = std::mem::take(&mut self.glyph_cache);
        let skia = self.direct_write();
        let tag = if self.smoothing == FontSmoothing::Windfoil { WINDFOIL_KEY } else { self.apple.key() };
        let (inks, apple) = if skia { (self.windows.inks(), DIRECT_WRITE_KEY) } else { (self.apple.inks(), tag) };
        let key = Key {
            layout: Rc::as_ptr(layout) as usize,
            inks: inks.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3)),
            apple,
        };
        let frame = cache.frame;
        let (entry, before) = match cache.entries.entry(key) {
            std::collections::hash_map::Entry::Occupied(o) => {
                let size = o.get().size();
                (o.into_mut(), size)
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                let mut e = cache.spare.take().unwrap_or_default();
                e.layout = Rc::downgrade(layout);
                e.inks.clear();
                e.inks.extend_from_slice(inks);
                (e.k, e.bytes, e.clock, e.used) = (k.to_bits(), bytes, self.atlas.clock, frame);
                e.clear(layout.len());
                (v.insert(e), 0)
            }
        };
        let fits = entry.k == k.to_bits() && entry.bytes == bytes && entry.inks == inks;
        // A raster factor in motion changes every frame: draw directly until it rests.
        if !fits && entry.used + 1 >= frame {
            self.glyph_cache = cache;
            return self.glyphs_in_band(layout, origin, k, bytes, band, out);
        }
        if !fits || entry.glyphs.len() * size_of::<Glyph>() > ENTRY_BYTES {
            (entry.k, entry.bytes, entry.inks) = (k.to_bits(), bytes, inks.to_vec());
            entry.clear(layout.len());
        }
        entry.used = frame;
        let mut lo = 0;
        if band.start > 0.0 {
            let mut hi = layout.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if layout.get(mid).is_some_and(|l| l.metrics().block_max_coord < band.start) { lo = mid + 1 } else { hi = mid }
            }
        }
        let mut table = std::mem::take(&mut self.cluster_scratch);
        let mut coords = std::mem::take(&mut self.coords_scratch);
        let mut i = lo;
        while layout.get(i).is_some_and(|l| l.metrics().block_min_coord <= band.end) {
            if entry.lines[i].0 == UNFLAT {
                entry.flatten(layout, i, &mut table, &mut coords);
                cache.flattened += 1;
            }
            self.replay(entry, i, origin, k, out);
            i += 1;
        }
        self.cluster_scratch = table;
        self.coords_scratch = coords;
        cache.bytes = (cache.bytes + entry.size()).saturating_sub(before);
        self.glyph_cache = cache;
    }

    /// Line `i`'s quads from `entry`, positioned as [`Self::glyphs_in_band`] positions them.
    fn replay(&mut self, entry: &mut Entry, i: usize, origin: [f32; 2], k: f32, out: &mut Vec<GlyphQuad>) {
        let (start, end) = entry.lines[i];
        let (mut face, mut baseline, mut size) = (u16::MAX, 0.0, 0.0);
        let quarters = self.quarter_pens();
        for gi in start as usize..end as usize {
            if entry.clock != self.atlas.clock {
                // A page went: every mask is looked up again.
                entry.clock = self.atlas.clock;
                for g in &mut entry.glyphs { g.state = UNKNOWN; }
            }
            let g = entry.glyphs[gi];
            let f = &entry.faces[g.face as usize];
            if g.face != face {
                (face, baseline, size) = (g.face, (origin[1] + f.0.baseline * k).round(), f.0.size * k);
            }
            let at = if k == 1.0 { [origin[0] + g.rel + g.x, baseline + g.y] }
                else { [origin[0] + (g.rel + g.x) * k, baseline + g.y * k] };
            let ([fx, fy], sub) = place(at, size, self.glide, quarters);
            if g.sub == sub && g.state == PLACED {
                self.atlas.touch(g.mask.page, g.mask.w * g.mask.h);
                out.push(atlas_quad(g.mask, fx, fy, f.0.ink, g.byte));
                continue;
            }
            if g.sub == sub && g.state == EMPTY {
                continue;
            }
            let placed = self.push_glyph(&f.0, &entry.coords[f.1 as usize..f.2 as usize], g.id, at, k, g.byte, out);
            let g = &mut entry.glyphs[gi];
            (g.state, g.sub, g.mask) = match placed {
                Placed::Atlas(mask, sub) => (PLACED, sub, mask),
                Placed::Empty(sub) => (EMPTY, sub, NO_MASK),
                Placed::Other => (UNKNOWN, [0; 2], NO_MASK),
            };
        }
    }
}

#[cfg(test)]
#[path = "glyph_cache_tests.rs"]
mod tests;
