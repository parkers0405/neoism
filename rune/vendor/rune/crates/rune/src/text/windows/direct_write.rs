//! DirectWrite rasterising glyphs as Chromium's Skia asks it to (`SkScalerContext_DW`): the
//! rendering mode its `gasp` rules pick, grid fitting on (Chromium's hinting is "medium"),
//! grayscale antialiasing, and the pen's x phase in the glyph's transform. The faces load from
//! the same bytes Parley shapes with, at the run's variation coordinates.

use std::collections::HashMap;

use swash::FontRef;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::Interface;

use super::{owner, skia_gamma};
use crate::text::{Atlas, Rasterized};

/// Faces (a font at one set of variation coordinates) kept before the cache starts over.
const MAX_FACES: usize = 256;

/// One glyph to rasterise: `key` names the face (font blob id, index, coordinates hash).
pub(crate) struct Job<'a> {
    pub font: FontRef<'a>,
    /// The bytes `font` reads, kept alive for DirectWrite while it holds the font.
    pub blob: &'a parley::fontique::Blob<u8>,
    pub key: (u64, u32, u64),
    pub coords: &'a [i16],
    pub glyph: u16,
    pub ppem: f32,
    /// The pen's x phase in device px (quarters, as Skia positions glyphs).
    pub phase_x: f32,
    /// Device px the mask moves down, for a gliding box.
    pub shift_y: f32,
    /// Synthetic oblique, in degrees.
    pub skew: u8,
    /// The text's luminance bucket ([`skia_gamma::bucket`]).
    pub lum: u8,
}

pub(crate) struct DirectWrite {
    factory: IDWriteFactory6,
    /// The same factory as IDWriteFactory2, whose glyph analysis takes a grid-fit mode and
    /// grayscale antialiasing.
    factory2: IDWriteFactory2,
    loader: IDWriteInMemoryFontFileLoader,
    /// Per font blob and index: its resource and the `gasp` facts the rendering mode needs.
    sources: HashMap<(u64, u32), Option<Source>>,
    faces: HashMap<(u64, u32, u64), Option<IDWriteFontFace>>,
    tables: [Option<Box<[u8; 256]>>; 8],
    scratch: Vec<u8>,
}

struct Source {
    resource: IDWriteFontResource,
    /// The `gasp` ranges (max ppem, behaviour) when its version is 1 or later.
    gasp: Option<Vec<(u16, u16)>>,
    /// The font has TrueType bytecode (`maxp.maxSizeOfInstructions` above zero).
    hinted: bool,
}

impl DirectWrite {
    /// The shared DirectWrite factory, or None where the system lacks variable-font support
    /// (`IDWriteFactory6`, Windows 10 1803).
    pub(crate) fn new() -> Option<DirectWrite> {
        // SAFETY: plain COM factory creation; the interfaces are reference counted.
        unsafe {
            let factory: IDWriteFactory6 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let factory2 = factory.cast().ok()?;
            let loader = factory.CreateInMemoryFontFileLoader().ok()?;
            factory.RegisterFontFileLoader(&loader).ok()?;
            Some(DirectWrite { factory, factory2, loader, sources: HashMap::new(), faces: HashMap::new(), tables: Default::default(), scratch: Vec::new() })
        }
    }

    /// Rasterises `job` into `atlas` with Skia's gamma table for its luminance; None when
    /// DirectWrite can't load its font (the caller then rasterises it itself).
    pub(crate) fn rasterize(&mut self, atlas: &mut Atlas, job: &Job<'_>) -> Option<Rasterized> {
        let (face, mode, em) = self.face(job)?;
        let index = job.glyph;
        let advance = 0.0f32;
        let offset = DWRITE_GLYPH_OFFSET::default();
        let run = DWRITE_GLYPH_RUN {
            fontFace: std::mem::ManuallyDrop::new(Some(face)),
            fontEmSize: em,
            glyphCount: 1,
            glyphIndices: &index,
            glyphAdvances: &advance,
            glyphOffsets: &offset,
            isSideways: false.into(),
            bidiLevel: 0,
        };
        let lean = (job.skew as f32).to_radians().tan();
        let transform = DWRITE_MATRIX { m11: 1.0, m12: 0.0, m21: -lean, m22: 1.0, dx: job.phase_x, dy: job.shift_y };
        // SAFETY: `run` borrows locals that outlive the analysis; the face is released below.
        let made = unsafe {
            let analysis = self.factory2.CreateGlyphRunAnalysis(&run, Some(&transform), mode, DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_GRID_FIT_MODE_ENABLED, DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE, 0.0, 0.0);
            analysis.and_then(|a| {
                let mut bounds = a.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
                // Smoothed vertically, a glyph shifted down part of a pixel inks one more row.
                // Its unshifted mask keeps that row, empty, so a gliding glyph's quad keeps one
                // size across its y phases and moves rather than growing and shrinking.
                if job.shift_y == 0.0 && mode == DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC && bounds.bottom > bounds.top {
                    bounds.bottom += 1;
                }
                let (w, h) = ((bounds.right - bounds.left).max(0) as u32, (bounds.bottom - bounds.top).max(0) as u32);
                self.scratch.clear();
                self.scratch.resize((w * h) as usize, 0);
                if w > 0 && h > 0 {
                    a.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, &mut self.scratch)?;
                }
                Ok((bounds, w, h))
            })
        };
        drop(std::mem::ManuallyDrop::into_inner(run.fontFace));
        let (bounds, w, h) = made.ok()?;
        if w == 0 || h == 0 || !self.scratch.iter().any(|&c| c != 0) {
            return Some(Rasterized::Empty);
        }
        let table = self.tables[job.lum as usize & 7].get_or_insert_with(|| Box::new(skia_gamma::table(job.lum)));
        for c in &mut self.scratch {
            *c = table[*c as usize];
        }
        Some(match atlas.put(w, h, &self.scratch, bounds.left, -bounds.top) {
            Some(g) => Rasterized::Placed(g),
            None => Rasterized::Unplaced,
        })
    }

    /// `job`'s face, the rendering mode Skia picks for its size and the em size to ask for.
    fn face(&mut self, job: &Job<'_>) -> Option<(IDWriteFontFace, DWRITE_RENDERING_MODE, f32)> {
        let source_key = (job.key.0, job.key.1);
        if !self.sources.contains_key(&source_key) {
            let source = self.source(job.font, job.blob, job.key.1);
            self.sources.insert(source_key, source);
        }
        let source = self.sources.get(&source_key)?.as_ref()?;
        let (mode, em) = rendering_mode(source, job.ppem);
        if self.faces.len() >= MAX_FACES && !self.faces.contains_key(&job.key) {
            self.faces.clear();
        }
        let face = self.faces.entry(job.key).or_insert_with(|| {
            let axes = axis_values(job.font, job.coords);
            // SAFETY: the resource outlives the call; DirectWrite copies the axis values.
            unsafe { source.resource.CreateFontFace(DWRITE_FONT_SIMULATIONS_NONE, &axes).ok()?.cast().ok() }
        });
        Some((face.clone()?, mode, em))
    }

    fn source(&self, font: FontRef<'_>, blob: &parley::fontique::Blob<u8>, index: u32) -> Option<Source> {
        let data = blob.as_ref();
        // SAFETY: the owner keeps `data` alive for as long as DirectWrite holds the reference,
        // so it reads the bytes in place instead of copying them.
        let resource = unsafe {
            let owner = owner::new(blob.clone());
            let file = self.loader.CreateInMemoryFontFileReference(&self.factory, data.as_ptr().cast(), data.len() as u32, &owner).ok()?;
            self.factory.CreateFontResource(&file, index).ok()?
        };
        Some(Source { resource, gasp: gasp(font), hinted: hinted(font) })
    }
}

impl Drop for DirectWrite {
    fn drop(&mut self) {
        // SAFETY: the loader was registered with this factory in `new`.
        unsafe { self.factory.UnregisterFontFileLoader(&self.loader) }.ok();
    }
}

/// Skia's choice (`SkScalerContext_DW`): a `gasp` table of version 1 says per size whether to
/// smooth vertically; otherwise hinted fonts at 20 px and under render without it, at the
/// size GDI would round to.
fn rendering_mode(source: &Source, ppem: f32) -> (DWRITE_RENDERING_MODE, f32) {
    const SYMMETRIC_SMOOTHING: u16 = 0x0008;
    let gdi = ((ppem * 64.0).round() / 64.0).max(1.0);
    if let Some(ranges) = &source.gasp
        && let Some(&(_, flags)) = ranges.iter().find(|&&(max, _)| (gdi.round() as i32) <= max as i32)
    {
        let mode = if flags & SYMMETRIC_SMOOTHING != 0 { DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC } else { DWRITE_RENDERING_MODE_NATURAL };
        return (mode, ppem);
    }
    if ppem > 20.0 || !source.hinted { (DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, ppem) } else { (DWRITE_RENDERING_MODE_NATURAL, gdi) }
}

fn gasp(font: FontRef<'_>) -> Option<Vec<(u16, u16)>> {
    let t = font.table(u32::from_be_bytes(*b"gasp"))?;
    let u16_at = |i: usize| t.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    if u16_at(0)? < 1 { return None; }
    let n = u16_at(2)? as usize;
    (0..n).map(|i| Some((u16_at(4 + 4 * i)?, u16_at(6 + 4 * i)?))).collect()
}

fn hinted(font: FontRef<'_>) -> bool {
    font.table(u32::from_be_bytes(*b"maxp"))
        .filter(|t| t.len() >= 32 && t[0..4] == [0, 1, 0, 0])
        .is_some_and(|t| u16::from_be_bytes([t[26], t[27]]) > 0)
}

/// The user-space axis values whose normalized coordinates (after `avar`) are `coords`, which
/// DirectWrite normalizes again to the same ones.
fn axis_values(font: FontRef<'_>, coords: &[i16]) -> Vec<DWRITE_FONT_AXIS_VALUE> {
    font.variations()
        .enumerate()
        .map(|(i, v)| {
            let target = coords.get(i).copied().unwrap_or(0);
            let first_at = |above: bool| {
                let (mut lo, mut hi) = (v.min_value(), v.max_value());
                for _ in 0..48 {
                    let mid = (lo + hi) * 0.5;
                    let n = v.normalize(mid);
                    if n > target || (!above && n == target) { hi = mid } else { lo = mid }
                }
                hi
            };
            let value = if target == 0 { v.default_value() } else { (first_at(false) + first_at(true)) * 0.5 };
            DWRITE_FONT_AXIS_VALUE { axisTag: DWRITE_FONT_AXIS_TAG(v.tag().swap_bytes()), value }
        })
        .collect()
}
