//! Glyph emission: walking a laid-out paragraph's glyphs into atlas quads.
use super::*;
use super::atlas_fill::Residency;

/// Positions a device pixel that the glyphs of a gliding box draw at down the screen
/// ([`TextSystem::set_glide`]): a glyph has at most this many masks for each x phase.
pub(crate) const GLIDE_PHASES: u8 = 4;

impl TextSystem {
    /// Glyph quads for single-line `text` with its layout box's top-left at `origin` (device px).
    pub fn glyphs(&mut self, text: &str, style: TextStyle, scale: f32, origin: [f32; 2], out: &mut Vec<GlyphQuad>) {
        let l = self.layout_rich(text, &[], style, scale, None);
        self.glyphs_for_layout(&l, scale, origin, false, out);
    }

    /// Glyph quads for a cached [`Layout`] with its top-left at `origin` (device px). When
    /// `bytes` is set, each quad carries the byte offset of its cluster. The layout already
    /// holds its scale, so `_scale` is unused; it stays for source compatibility.
    pub fn glyphs_for_layout(
        &mut self,
        layout: &Layout<Ink>,
        _scale: f32,
        origin: [f32; 2],
        bytes: bool,
        out: &mut Vec<GlyphQuad>,
    ) {
        self.glyphs_for_layout_scaled(layout, origin, 1.0, bytes, out);
    }

    /// [`Self::glyphs_for_layout`] with the layout magnified `k` times about `origin`: pen
    /// positions scale and each glyph rasterizes at `k` times its size, so it stays crisp.
    pub(crate) fn glyphs_for_layout_scaled(
        &mut self,
        layout: &Layout<Ink>,
        origin: [f32; 2],
        k: f32,
        bytes: bool,
        out: &mut Vec<GlyphQuad>,
    ) {
        self.glyphs_in_band(layout, origin, k, bytes, f32::NEG_INFINITY..f32::INFINITY, out);
    }

    /// [`Self::glyphs_for_layout_scaled`] for only the lines whose box meets `band`, a span of
    /// the layout's own y (device px, before `k`). Lines are found by binary search, so a long
    /// text in a scroll view costs its visible lines, not all of them.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn glyphs_in_band(
        &mut self,
        layout: &Layout<Ink>,
        origin: [f32; 2],
        k: f32,
        bytes: bool,
        band: std::ops::Range<f32>,
        out: &mut Vec<GlyphQuad>,
    ) {
        let mut table = std::mem::take(&mut self.cluster_scratch);
        let mut coords = std::mem::take(&mut self.coords_scratch);
        let mut lo = 0;
        if band.start > 0.0 {
            let mut hi = layout.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if layout.get(mid).is_some_and(|l| l.metrics().block_max_coord < band.start) { lo = mid + 1 } else { hi = mid }
            }
        }
        let lines = (lo..layout.len()).map_while(|i| layout.get(i).filter(|l| l.metrics().block_min_coord <= band.end));
        for line in lines {
            if bytes {
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
                let face = Face::of(&glyph_run, &mut coords);
                let baseline = (origin[1] + face.baseline * k).round();
                let mut rel = glyph_run.offset();
                for g in glyph_run.glyphs() {
                    let byte = if bytes {
                        let i = table.partition_point(|&(x0, _)| x0 <= rel + 0.01);
                        table.get(i.saturating_sub(1)).map_or(0, |e| e.1)
                    } else {
                        0
                    };
                    let at = if k == 1.0 { [origin[0] + rel + g.x, baseline + g.y] }
                        else { [origin[0] + (rel + g.x) * k, baseline + g.y * k] };
                    self.push_glyph(&face, &coords, g.id, at, k, byte, out);
                    rel += g.advance;
                }
            }
        }
        self.cluster_scratch = table;
        self.coords_scratch = coords;
    }

    /// Glyph quads for the grapheme at byte `at` of a shaped run, with the run's top-left at
    /// `origin` (device px). Only that grapheme's glyphs are visited. `_scale` is unused (the
    /// run holds its scale); it stays for source compatibility.
    pub fn glyphs_in(&mut self, run: &RunLayout, at: usize, _scale: f32, origin: [f32; 2], out: &mut Vec<GlyphQuad>) {
        let Some(i) = run.grapheme(at) else { return };
        let glyphs = run.graphemes[i].2 as usize..run.graphemes[i + 1].2 as usize;
        for g in &run.glyphs[glyphs] {
            let face = &run.faces[g.face as usize];
            let baseline = (origin[1] + face.face.baseline).round();
            self.push_glyph(&face.face, &face.coords, g.id, [origin[0] + g.x, baseline + g.y], 1.0, at as u32, out);
        }
    }

    /// Rasterizes (once) and appends one glyph at device position `at`, `k` times its size.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_glyph(&mut self, face: &Face, coords: &[i16], glyph: u32, at: [f32; 2], k: f32, byte: u32, out: &mut Vec<GlyphQuad>) -> Placed {
        if self.emit == outline::Emit::Outline && let Some(placed) = self.push_outline(face, coords, glyph, at, k, byte, out) {
            return placed;
        }
        let [gx, gy] = at;
        let glide = self.glide;
        let (font, font_size, ink, skew) = (&face.font, face.size * k, face.ink, face.skew);
        let direct_write = self.direct_write();
        let windfoil = self.smoothing == FontSmoothing::Windfoil;
        let radius = if direct_write { 0.0 } else { self.apple.radius(ink, font_size) * if windfoil { outline::TEXT_WEIGHT } else { 1.0 } };
        let gamma = self.windows.bucket(ink);
        let ([fx, fy], [subpx, subpy]) = place(at, font_size, self.glide, direct_write || windfoil);
        let key = GlyphKey {
            font: font.data.id(),
            index: font.index,
            glyph,
            size: font_size.to_bits(),
            coords: face.coords_key,
            subpx,
            subpy,
            dilation: if direct_write { gamma as u32 } else { (radius as f32).to_bits() },
            apple: if direct_write { DIRECT_WRITE_KEY } else if windfoil { WINDFOIL_KEY } else { self.apple.key() },
            skew,
        };
        let entry = match self.atlas.map.get(&key) {
            Some(e) => {
                let mask = e.mask();
                if let Some(a) = mask {
                    self.atlas.touch(a.page, a.w * a.h);
                }
                mask
            }
            None => {
                // Colour glyphs sit on whole pixels and ignore emboldening and obliques.
                let color_key = GlyphKey { subpx: 0, subpy: 0, dilation: 0, apple: 0, skew: 0, ..key };
                if let Some(c) = self.color_glyphs.get_mut(&color_key) {
                    c.used = true;
                    out.push(c.quad(gx, gy, glide, ink, byte));
                    return Placed::Other;
                }
                let Some(font_ref) = FontRef::from_index(font.data.as_ref(), font.index as usize) else {
                    return Placed::Other;
                };
                let mut scaler = self
                    .scale_cx
                    .builder(font_ref)
                    .size(font_size)
                    .hint(false)
                    .normalized_coords(coords.iter().copied())
                    .build();
                if let Some((pixels, at)) = rasterize_color(&mut scaler, glyph as u16) {
                    self.color_bytes = self.color_bytes.saturating_add((at.width as usize) * (at.height as usize) * 4);
                    self.color_pixels.push(pixels);
                    let index = self.color_pixels.len() as u32;
                    let c = ColorGlyph { index, used: true, left: at.left, top: at.top, w: at.width, h: at.height };
                    self.color_glyphs.insert(color_key, c);
                    out.push(c.quad(gx, gy, glide, ink, byte));
                    return Placed::Other;
                }
                // Windfoil masks place their own entries (GPU-filled ones are queued, not uploaded).
                if windfoil && let Some(placed) = self.windfoil_mask(key, face, coords, font_size, [subpx, subpy]) {
                    return match placed {
                        Rasterized::Placed(a) => {
                            out.push(atlas_quad(a, fx, fy, ink, byte));
                            Placed::Atlas(a, [subpx, subpy])
                        }
                        Rasterized::Empty => Placed::Empty([subpx, subpy]),
                        Rasterized::Unplaced => Placed::Other,
                    };
                }
                let (phase_x, shift_y) = (subpx as f32 / coretext_match::PHASES, subpy as f32 / GLIDE_PHASES as f32);
                #[cfg(target_os = "windows")]
                let placed = direct_write.then(|| self.windows.rasterize(&mut self.atlas, &windows::Job {
                    font: font_ref, blob: &font.data, key: (key.font, key.index, key.coords), coords, glyph: glyph as u16, ppem: font_size,
                    phase_x, shift_y, skew, lum: gamma,
                })).flatten();
                #[cfg(not(target_os = "windows"))]
                let placed = None;
                let placed = placed.unwrap_or_else(|| self.apple.rasterize(&mut self.scale_cx, &mut self.atlas, macos::smoothing::Job {
                    font: font_ref, key: (key.font, key.index, key.coords), coords, glyph: glyph as u16, ppem: font_size,
                    phase_x, shift_y, radius, skew,
                }));
                match placed {
                    Rasterized::Placed(g) => {
                        self.atlas.map.insert(key, Residency::Uploaded(g));
                        Some(g)
                    }
                    Rasterized::Empty => {
                        self.atlas.map.insert(key, Residency::Blank);
                        None
                    }
                    Rasterized::Unplaced => return Placed::Other,
                }
            }
        };
        match entry {
            Some(a) => {
                out.push(atlas_quad(a, fx, fy, ink, byte));
                Placed::Atlas(a, [subpx, subpy])
            }
            None => Placed::Empty([subpx, subpy]),
        }
    }

    /// Moves the glyphs placed from here on by `glide` device px (`[x, y]`, each within half a
    /// pixel), in at least [`GLIDE_PHASES`] steps a pixel on any gliding axis. Pixel snapping
    /// glides a moving box between pixels; its text lays out from the box's nearest whole pixel on
    /// `y` and passes the rest here, so the words move with the box instead of a pixel at a time.
    /// `[0.0; 2]`, the default, places glyphs as text at rest.
    pub(crate) fn set_glide(&mut self, glide: [f32; 2]) {
        self.glide = glide;
    }
}

/// The glyph key tag of a DirectWrite mask (CoreGraphics' pipelines use the low bits).
pub(super) const DIRECT_WRITE_KEY: u8 = 0x80;
/// The glyph key tag of a Windfoil mask (`windfoil.rs`).
pub(super) const WINDFOIL_KEY: u8 = 0x40;

/// Where a glyph with its resting pen at `at` draws at `ppem` device px, gliding by `glide`: its
/// quad's whole-pixel pen and its x phase (sixtieths, [`coretext_match::PHASES`]) and y phase
/// ([`GLIDE_PHASES`] a pixel). Pens sit on CoreGraphics' subpixel steps, or on Skia's quarters
/// when `skia` (DirectWrite). Adding the quantised glide on top of the resting pen keeps each
/// glyph continuous at glide start/end and visits at most [`GLIDE_PHASES`] phases per axis.
pub(super) fn place(at: [f32; 2], ppem: f32, glide: [f32; 2], skia: bool) -> ([f32; 2], [u8; 2]) {
    let (rx, r_subpx) = if skia { windows::glyph_x(at[0]) } else { coretext_match::glyph_x(at[0], ppem) };
    let n = GLIDE_PHASES as f32;
    let gx = rx * coretext_match::PHASES + r_subpx as f32 + (glide[0] * n).round() * (coretext_match::PHASES / n);
    let fx = (gx / coretext_match::PHASES).floor();
    let subpx = (gx - fx * coretext_match::PHASES) as u8;
    let y = at[1].round() * n + (glide[1] * n).round();
    let fy = (y / n).floor();
    ([fx, fy], [subpx, (y - fy * n) as u8])
}

/// What [`TextSystem::push_glyph`] drew: an atlas mask or nothing at an x and y phase, which a
/// later frame can replay at those phases, or a quad it must ask for again.
pub(super) enum Placed {
    Atlas(AtlasGlyph, [u8; 2]),
    Empty([u8; 2]),
    Other,
}

/// The quad of atlas mask `a` for a pen at whole-pixel x `fx` and baseline `gy`.
pub(super) fn atlas_quad(a: AtlasGlyph, fx: f32, gy: f32, ink: u16, byte: u32) -> GlyphQuad {
    GlyphQuad {
        rect: [fx + a.left as f32, gy.round() - a.top as f32, a.w as f32, a.h as f32],
        uv: [a.x as f32, (a.page as u32 * ATLAS_SIZE + a.y) as f32],
        ink,
        byte,
        color: 0,
        outline: None,
    }
}

/// One glyph run's face, as the rasterizer needs it.
pub(super) struct Face {
    pub(super) font: parley::FontData,
    pub(super) size: f32,
    pub(super) coords_key: u64,
    pub(super) ink: u16,
    /// A face without italics leans by a fixed angle instead of fontique's 14°.
    pub(super) skew: u8,
    pub(super) baseline: f32,
}

impl Face {
    /// The face of `glyph_run`; its normalized coordinates go to `coords`.
    pub(super) fn of(glyph_run: &parley::GlyphRun<'_, Ink>, coords: &mut Vec<i16>) -> Face {
        let run = glyph_run.run();
        coords.clear();
        coords.extend(run.normalized_coords().iter().map(|c| c.to_bits()));
        Face {
            font: run.font().clone(),
            size: run.font_size(),
            coords_key: coords.iter().fold(0u64, |h, &c| h.wrapping_mul(31).wrapping_add(c as u16 as u64)),
            ink: glyph_run.style().brush.0,
            skew: if run.synthesis().skew().is_some() { SYNTHETIC_OBLIQUE } else { 0 },
            baseline: glyph_run.baseline(),
        }
    }
}
