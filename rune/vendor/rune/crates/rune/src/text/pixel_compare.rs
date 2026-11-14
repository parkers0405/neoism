//! Rune's text against CoreText, pixel for pixel. Fixed cases (four ink/paper pairs; Inter
//! Regular and Bold at seven sizes and the Rune vs macOS report's paragraph, or San Francisco
//! body and heading sizes, or SF Mono at code sizes; 1× and 2×) are drawn through Rune's real glyph rasteriser, atlas and
//! GPU blend, read back, and compared with CoreText reference sheets that
//! `scripts/coretext-reference.sh` draws on a Mac, with font smoothing on and off.
//!
//! The Inter sheets are committed in `crates/rune/tests/coretext/`. San Francisco can't be
//! redistributed, so its sheets (and SF Mono's) are drawn from the system's fonts into
//! `tests/coretext/sf/` and `tests/coretext/mono/` (ignored by git) and their comparisons are skipped where they're missing.
//! `cargo run -p rune --example coretext_compare` prints the table; the macOS-only test
//! `text::pixel_compare::tests` holds it to loose bounds.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use parley::PositionedLayoutItem;

use crate::color::{hex, srgb_encode};
use crate::render::{Gpu, NO_CLIP, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::text::{FontSmoothing, GlyphQuad, TextSystem};
use crate::tok::{Family, TextStyle};

/// A text colour on a page colour, sRGB hex.
#[derive(Clone, Copy, Debug)]
pub struct Pair {
    pub name: &'static str,
    pub fg: &'static str,
    pub bg: &'static str,
}

/// Dark ink on paper; light grey (macOS's q = 0.75 smoothing bucket) and near-white (q = 1) on
/// a dark page; a mid blue (q = 0.25).
pub const PAIRS: [Pair; 4] = [
    Pair { name: "light", fg: "#1d1e22", bg: "#fcfcfb" },
    Pair { name: "dark", fg: "#e6e6e6", bg: "#1d1e22" },
    Pair { name: "white", fg: "#fcfcfb", bg: "#1d1e22" },
    Pair { name: "blue", fg: "#296d9a", bg: "#fcfcfb" },
];
pub const SCALES: [f32; 2] = [1.0, 2.0];
/// Inter sizes, in points (device px at 1×).
pub const SIZES: [f32; 7] = [9.0, 11.0, 13.0, 16.0, 20.0, 24.0, 32.0];
/// The smoothing study's glyphs (straight stems, diagonals, round tops and bottoms) and a word.
pub const SAMPLE: &str = "IHT\u{2014}/AVSog@8 Hamburgefonstiv";
/// The paragraph of the Rune vs macOS text report: 16/26 pt, wrapped at 567 pt, at (49, 40) pt,
/// measured over the band 30–120 pt of a 640 pt canvas, as the report did.
/// SF Mono's glyphs: thin stems on a fixed grid (i, l, 1, t), wide ones (m), punctuation.
pub const MONO_SAMPLE: &str = "It doesn't look like it; illicit 1111 mmmm @@ -33,3 +33,4 @@";
/// A diff, as the History drawer's view shows it, in a code block's 13.5/22 pt.
pub const MONO_PARAGRAPH: &str = "diff --git a/Welcome.md b/Welcome.md --- a/Welcome.md +++ b/Welcome.md @@ -33,3 +33,4 @@ $1 + 1$ It doesn't look like it +*Italics work though!* iiiiiiii llllllll mmmmmmmm";
pub const PARAGRAPH: &str = "The quick brown fox jumps over the lazy dog. AV To Wa Ty Yo LT fi fl ffi office flour 0123456789 ($4.50, 87%) \u{2014} \"quoted\" & semicolons; colons: question? Incomprehensibilities and HYPERVENTILATION.";
pub const SHEET_WIDTH: f32 = 640.0;
const ROW_X: f32 = 16.0;
const PARAGRAPH_HEIGHT: f32 = 130.0;
/// The family Rune names the macOS system font by.
const SYSTEM_FONT: &str = "System Font";

/// Which font's cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set {
    /// Inter 4.1 Regular and Bold; references committed.
    Inter,
    /// San Francisco (the system font, SFNS) at its `opsz` for each size; references local.
    Sf,
    /// SF Mono (`Family::SystemMono`, SFNSMono) at inline-code and code-block sizes; references local.
    Mono,
}

/// Which CoreText rendering a sheet holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reference {
    /// Font smoothing on, macOS's default (compare `FontSmoothing::On`).
    Smoothed,
    /// Font smoothing off, as `-webkit-font-smoothing: antialiased` draws (compare `Off`).
    Unsmoothed,
}

impl Reference {
    /// The reference a mode is meant to match.
    pub fn for_mode(mode: FontSmoothing) -> Reference {
        match mode {
            FontSmoothing::On | FontSmoothing::Windfoil => Reference::Smoothed,
            FontSmoothing::Off | FontSmoothing::DirectWrite => Reference::Unsmoothed,
        }
    }
    fn prefix(self) -> &'static str {
        match self { Reference::Smoothed => "", Reference::Unsmoothed => "flat-" }
    }
    /// Unsmoothed sheets cover the light and dark pairs: without dilation, ink colour changes
    /// only the blend.
    pub fn pairs(self) -> &'static [Pair] {
        match self { Reference::Smoothed => &PAIRS, Reference::Unsmoothed => &PAIRS[..2] }
    }
}

/// One case on a sheet; every length in points.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    pub family: Family,
    pub weight: f32,
    pub size: f32,
    pub line_height: f32,
    pub wrap: Option<f32>,
    pub text: &'static str,
    pub origin: [f32; 2],
    /// The region compared: `[x, y, w, h]`. Rows compare the ink bounds inside it (plus a pixel);
    /// the paragraph compares all of it, so its numbers line up with the report's band.
    pub region: [f32; 4],
    pub whole_region: bool,
}

impl Case {
    /// The font the reference draws it with: "regular", "bold" or "system".
    fn font(&self) -> &'static str {
        match (self.family, self.weight >= 700.0) {
            (Family::Named(SYSTEM_FONT), _) => "system",
            (Family::SystemMono, _) => "mono",
            (_, true) => "bold",
            _ => "regular",
        }
    }
}

/// The cases of `set`, top to bottom.
pub fn cases(set: Set) -> Vec<Case> {
    let text = if set == Set::Mono { MONO_SAMPLE } else { SAMPLE };
    let (family, rows): (Family, Vec<(&str, f32, f32)>) = match set {
        Set::Inter => {
            let mut rows = Vec::new();
            for (weight, font) in [(400.0, "regular"), (700.0, "bold")] {
                rows.extend(SIZES.iter().map(|&size| (font, weight, size)));
            }
            (Family::Sans, rows)
        }
        Set::Sf => (Family::Named(SYSTEM_FONT), vec![("sf regular", 400.0, 16.0), ("sf bold", 700.0, 16.0), ("sf bold", 700.0, 28.0), ("sf bold", 700.0, 32.0)]),
        Set::Mono => (Family::SystemMono, [11.0, 12.0, 13.0, 13.5, 14.0, 16.0].iter().map(|&size| ("mono", 400.0, size)).collect()),
    };
    let mut out = Vec::new();
    let mut y = 4.0f32;
    for (font, weight, size) in rows {
        let height = (size * 1.25).ceil() + 6.0;
        out.push(Case {
            name: format!("{font} {size}"),
            family,
            weight,
            size,
            line_height: 0.0,
            wrap: None,
            text,
            origin: [ROW_X, y + 3.0],
            region: [0.0, y, SHEET_WIDTH, height],
            whole_region: false,
        });
        y += height;
    }
    let top = y.ceil() + 4.0;
    out.push(paragraph_case(family, top));
    out
}

fn paragraph_case(family: Family, top: f32) -> Case {
    if family == Family::SystemMono {
        return Case {
            name: "mono paragraph 13.5/22".into(),
            family,
            weight: 400.0,
            size: 13.5,
            line_height: 22.0,
            wrap: Some(567.0),
            text: MONO_PARAGRAPH,
            origin: [49.0, top + 40.0],
            region: [0.0, top + 30.0, SHEET_WIDTH, 90.0],
            whole_region: true,
        };
    }
    Case {
        name: if family == Family::Sans { "paragraph 16/26".into() } else { "sf paragraph 16/26".into() },
        family,
        weight: 400.0,
        size: 16.0,
        line_height: 26.0,
        wrap: Some(567.0),
        text: PARAGRAPH,
        origin: [49.0, top + 40.0],
        region: [0.0, top + 30.0, SHEET_WIDTH, 90.0],
        whole_region: true,
    }
}

pub fn sheet_height(set: Set) -> f32 {
    let last = cases(set).pop().expect("cases");
    last.region[1] - 30.0 + PARAGRAPH_HEIGHT
}

/// Where the committed references, the Bold subset and `layout.txt` live.
pub fn refs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/coretext")
}

impl Set {
    /// Where this set's sheets and `layout.txt` live.
    pub fn dir(self) -> PathBuf {
        match self { Set::Inter => refs_dir(), Set::Sf => refs_dir().join("sf"), Set::Mono => refs_dir().join("mono") }
    }
    pub fn name(self) -> &'static str {
        match self { Set::Inter => "Inter", Set::Sf => "San Francisco", Set::Mono => "SF Mono" }
    }
}

/// The font files: Rune's own Inter Regular and an ASCII subset of Inter 4.1 Bold.
pub fn font_paths() -> [PathBuf; 2] {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    [root.join("assets/Inter-Regular.ttf"), refs_dir().join("Inter-Bold-subset.ttf")]
}

/// The system font file Rune loads for "System Font", when this machine has one.
pub fn system_font_path() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") { return None; }
    super::system_font_files(&["sfnsttf"]).into_iter().next()
}

/// The SF Mono file `Family::SystemMono` loads, when this machine has one.
pub fn system_mono_path() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") { return None; }
    super::system_font_files(&["sfnsmono"]).into_iter().find(|p| !p.to_string_lossy().to_lowercase().contains("italic"))
}

fn text_system() -> Result<TextSystem, String> {
    let mut fonts: Vec<&'static [u8]> = Vec::new();
    for path in font_paths() {
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        fonts.push(Box::leak(bytes.into_boxed_slice()));
    }
    Ok(TextSystem::new(&fonts, ("Inter", "Inter")))
}

fn style(case: &Case) -> TextStyle {
    TextStyle { line_height: case.line_height, ..TextStyle::new(case.size, case.weight).family(case.family) }
}

/// Each case's glyphs as `(id, x, baseline)` in device px: the pen x before any quantisation
/// and the whole-pixel baseline, as `TextSystem::glyphs_for_layout` computes them.
fn positions(text: &mut TextSystem, case: &Case, scale: f32) -> Vec<(u32, f32, f32)> {
    let layout = text.layout_rich(case.text, &[], style(case), scale, case.wrap.map(|w| w * scale));
    let origin = case.origin.map(|v| v * scale);
    let mut out = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else { continue };
            let baseline = (origin[1] + run.baseline()).round();
            let mut rel = run.offset();
            for g in run.glyphs() {
                out.push((g.id, origin[0] + rel + g.x, baseline + g.y));
                rel += g.advance;
            }
        }
    }
    out
}

/// Whether `set` can be compared here: its font and its sheets exist.
pub fn available(set: Set) -> Result<(), String> {
    if set == Set::Sf && system_font_path().is_none() {
        return Err("no system font (SFNS) on this machine".into());
    }
    if set == Set::Mono && system_mono_path().is_none() {
        return Err("no SF Mono (SFNSMono) on this machine".into());
    }
    let have = std::fs::read_to_string(set.dir().join("layout.txt")).map_err(|_| {
        format!("no {} references in {}: run scripts/coretext-reference.sh", set.name(), set.dir().display())
    })?;
    let want = layout_file(set)?;
    if have != want {
        return Err(format!("{} references are stale (layout.txt {have:?}, now {want:?}): run scripts/coretext-reference.sh", set.name()));
    }
    Ok(())
}

/// The layout the CoreText references are drawn from, for `scripts/coretext-reference.swift`.
pub fn export(set: Set, scale: f32) -> Result<serde_json::Value, String> {
    let mut text = text_system()?;
    let [regular, bold] = font_paths().map(|p| p.display().to_string());
    let system = system_font_path().map(|p| p.display().to_string());
    let mono = system_mono_path().map(|p| p.display().to_string());
    if set == Set::Sf && system.is_none() {
        return Err("no system font (SFNS) on this machine".into());
    }
    if set == Set::Mono && mono.is_none() {
        return Err("no SF Mono (SFNSMono) on this machine".into());
    }
    let mut rows = Vec::new();
    for case in cases(set) {
        let glyphs: Vec<_> = positions(&mut text, &case, scale).into_iter().map(|(id, x, y)| serde_json::json!([id, x, y])).collect();
        rows.push(serde_json::json!({
            "name": case.name, "font": case.font(), "size": case.size, "weight": case.weight, "opsz": case.size, "glyphs": glyphs,
        }));
    }
    let sheets: Vec<_> = [Reference::Smoothed, Reference::Unsmoothed]
        .iter()
        .flat_map(|r| r.pairs().iter().map(move |p| serde_json::json!({
            "file": format!("{}{}-{scale}x.png", r.prefix(), p.name), "smooth": *r == Reference::Smoothed, "fg": p.fg, "bg": p.bg,
        })))
        .collect();
    Ok(serde_json::json!({
        "scale": scale,
        "width": (SHEET_WIDTH * scale) as u32,
        "height": (sheet_height(set) * scale) as u32,
        "fonts": { "regular": regular, "bold": bold, "system": system, "mono": mono },
        "sheets": sheets,
        "cases": rows,
    }))
}

/// A digest of every case's glyph ids and positions at `scale`. The references are stale when
/// it differs from the one in `layout.txt`.
pub fn layout_digest(set: Set, scale: f32) -> Result<String, String> {
    let mut text = text_system()?;
    let mut h = blake3::Hasher::new();
    for case in cases(set) {
        for (id, x, y) in positions(&mut text, &case, scale) {
            h.update(&id.to_le_bytes());
            h.update(&x.to_le_bytes());
            h.update(&y.to_le_bytes());
        }
    }
    Ok(h.finalize().to_hex()[..16].to_string())
}

/// `layout.txt`'s contents for the current layout of `set`.
pub fn layout_file(set: Set) -> Result<String, String> {
    let mut out = String::new();
    for scale in SCALES {
        let _ = writeln!(out, "{scale}x {}", layout_digest(set, scale)?);
    }
    Ok(out)
}

/// An opaque RGB image.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Image {
    fn px(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    fn green(&self, x: i64, y: i64) -> Option<u8> {
        (x >= 0 && y >= 0 && x < self.width as i64 && y < self.height as i64).then(|| self.px(x as u32, y as u32)[1])
    }

    pub fn read_png(path: &Path) -> Result<Image, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let channels = info.color_type.samples();
        let rgb = buf[..info.buffer_size()]
            .chunks_exact(channels)
            .flat_map(|p| if channels >= 3 { [p[0], p[1], p[2]] } else { [p[0]; 3] })
            .collect();
        Ok(Image { width: info.width, height: info.height, rgb })
    }

    pub fn write_png(&self, path: &Path) -> std::io::Result<()> {
        let rgba: Vec<u8> = self.rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        crate::render::write_png(path, self.width, self.height, &rgba)
    }

    /// The green channel as a greyscale PNG, compressed hard: how the reference sheets are kept.
    pub fn write_green_png(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        enc.set_filter(png::Filter::Adaptive);
        let green: Vec<u8> = self.rgb.chunks_exact(3).map(|p| p[1]).collect();
        enc.write_header().and_then(|mut w| w.write_image_data(&green)).map_err(|e| e.to_string())
    }
}

/// Draws sheets of cases through the glyph instances the runtime builds, blended as a window
/// with `srgb_blend` blends.
pub struct Harness {
    gpu: Gpu,
    text: TextSystem,
    /// One renderer, since it holds the atlas pages uploaded so far.
    renderer: Option<Renderer>,
}

impl Harness {
    pub fn new() -> Result<Harness, String> {
        Ok(Harness { gpu: Gpu::try_new(Gpu::instance(), None)?, text: text_system()?, renderer: None })
    }

    /// `cases` in `pair`'s colours on a `size` (points) canvas at `scale`.
    pub fn render(&mut self, pair: Pair, scale: f32, srgb_blend: bool, smoothing: FontSmoothing, cases: &[Case], size: [f32; 2]) -> Image {
        self.text.set_font_smoothing(smoothing);
        let (fg, bg) = (hex(pair.fg).expect("fg"), hex(pair.bg).expect("bg"));
        let (w, h) = ((size[0] * scale) as u32, (size[1] * scale) as u32);
        let mut quads: Vec<GlyphQuad> = Vec::new();
        self.text.set_glyph_inks(fg, &[]);
        for case in cases {
            let layout = self.text.layout_rich(case.text, &[], style(case), scale, case.wrap.map(|w| w * scale));
            self.text.glyphs_for_layout(&layout, scale, case.origin.map(|v| v * scale), false, &mut quads);
        }
        let fill = crate::runtime::premul(fg, 1.0, srgb_blend);
        // The coverage exponent a window's paint applies, so the sheets are what an app draws.
        let exponent = self.text.coverage_exponent(fill);
        let instances: Vec<_> = quads.iter().map(|q| { let mut g = crate::runtime::glyph(q, fill, NO_CLIP, [0.0; 4]); g.params[3] = exponent; g }).collect();
        let format = if srgb_blend { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
        let renderer = self.renderer.get_or_insert_with(|| Renderer::new(&self.gpu.device, format));
        renderer.set_format(&self.gpu.device, format);
        renderer.set_srgb_blend(srgb_blend);
        let target = Offscreen::with_format(&self.gpu.device, w, h, format);
        renderer.prepare(&self.gpu.device, &self.gpu.queue, &mut self.text.atlas, &instances, (w, h));
        let clear = if srgb_blend { [srgb_encode(bg.r), srgb_encode(bg.g), srgb_encode(bg.b), 1.0] } else { [bg.r, bg.g, bg.b, 1.0] };
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, clear);
        target.copy(&mut encoder);
        renderer.submit(&self.gpu.queue, encoder);
        let rgba = target.read(&self.gpu.device);
        Image { width: w, height: h, rgb: rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect() }
    }
}

/// Differences in the green channel over a region (the references keep only green), on 0–255.
/// "Ink" is the summed distance from the page, so it works for light on dark too.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub pixels: u64,
    pub differing: u64,
    pub over8: u64,
    pub max: u8,
    pub abs_sum: f64,
    pub ink_rune: f64,
    pub ink_ref: f64,
}

impl Stats {
    pub fn add(&mut self, o: &Stats) {
        self.pixels += o.pixels;
        self.differing += o.differing;
        self.over8 += o.over8;
        self.max = self.max.max(o.max);
        self.abs_sum += o.abs_sum;
        self.ink_rune += o.ink_rune;
        self.ink_ref += o.ink_ref;
    }
    /// Mean |difference|.
    pub fn meanabs(&self) -> f64 { self.abs_sum / self.pixels.max(1) as f64 }
    /// Rune's ink over CoreText's.
    pub fn ink_ratio(&self) -> f64 { self.ink_rune / self.ink_ref.max(1.0) }
    pub fn differing_share(&self) -> f64 { self.differing as f64 / self.pixels.max(1) as f64 }
    /// Share of pixels off by more than 8.
    pub fn over8_share(&self) -> f64 { self.over8 as f64 / self.pixels.max(1) as f64 }
}

/// Stats over `[x0, y0, x1, y1)` (px of `reference`), with Rune's image read `shift` px away.
fn stats_in(rune: &Image, reference: &Image, [x0, y0, x1, y1]: [u32; 4], shift: [i64; 2]) -> Stats {
    let (paper_rune, paper_ref) = (rune.px(0, 0)[1], reference.px(0, 0)[1]);
    let mut s = Stats::default();
    for y in y0..y1 {
        for x in x0..x1 {
            let a = rune.green(x as i64 - shift[0], y as i64 - shift[1]).unwrap_or(paper_rune);
            let b = reference.px(x, y)[1];
            let d = a.abs_diff(b);
            s.pixels += 1;
            s.differing += (d > 0) as u64;
            s.over8 += (d > 8) as u64;
            s.max = s.max.max(d);
            s.abs_sum += d as f64;
            s.ink_rune += a.abs_diff(paper_rune) as f64;
            s.ink_ref += b.abs_diff(paper_ref) as f64;
        }
    }
    s
}

/// The ink bounds of either image inside `[x0, y0, x1, y1)`, grown by a pixel.
fn ink_bounds(images: [&Image; 2], [x0, y0, x1, y1]: [u32; 4]) -> Option<[u32; 4]> {
    let (mut lo, mut hi) = ((u32::MAX, u32::MAX), (0, 0));
    for image in images {
        let paper = image.px(0, 0)[1];
        for y in y0..y1.min(image.height) {
            for x in x0..x1.min(image.width) {
                if image.px(x, y)[1] != paper {
                    lo = (lo.0.min(x), lo.1.min(y));
                    hi = (hi.0.max(x + 1), hi.1.max(y + 1));
                }
            }
        }
    }
    (hi.0 > 0).then(|| [lo.0.saturating_sub(1).max(x0), lo.1.saturating_sub(1).max(y0), (hi.0 + 1).min(x1), (hi.1 + 1).min(y1)])
}

/// Compares `case`'s region of two sheets of the same size.
pub fn compare(rune: &Image, reference: &Image, case: &Case, scale: f32) -> Stats {
    let [x, y, w, h] = case.region.map(|v| (v * scale).round() as u32);
    let region = [x, y, (x + w).min(rune.width), (y + h).min(rune.height)];
    let rect = if case.whole_region { Some(region) } else { ink_bounds([rune, reference], region) };
    rect.map_or_else(Stats::default, |r| stats_in(rune, reference, r, [0, 0]))
}

pub fn reference_path(set: Set, reference: Reference, pair: Pair, scale: f32) -> PathBuf {
    set.dir().join(format!("{}{}-{scale}x.png", reference.prefix(), pair.name))
}

/// One table row.
pub struct Row {
    pub pair: Pair,
    pub scale: f32,
    pub case: Case,
    pub stats: Stats,
}

/// What [`run`] draws and what it compares with.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub set: Set,
    /// None: the platform's default.
    pub smoothing: Option<FontSmoothing>,
    /// None: as a window blends by default (sRGB).
    pub srgb_blend: Option<bool>,
    /// None: the reference the mode is meant to match.
    pub reference: Option<Reference>,
}

impl Options {
    pub fn new(set: Set) -> Options {
        Options { set, smoothing: None, srgb_blend: None, reference: None }
    }
    pub fn smoothing(&self) -> FontSmoothing { self.smoothing.unwrap_or_default() }
    pub fn srgb_blend(&self) -> bool { self.srgb_blend.unwrap_or(true) }
    pub fn reference(&self) -> Reference { self.reference.unwrap_or(Reference::for_mode(self.smoothing())) }
}

/// Every case of every pair at every scale. Errs when the set can't be compared here
/// ([`available`]).
pub fn run(o: Options, mut keep: impl FnMut(Pair, f32, &Image, &Image)) -> Result<Vec<Row>, String> {
    available(o.set)?;
    let cases = cases(o.set);
    let mut harness = Harness::new()?;
    let mut rows = Vec::new();
    for scale in SCALES {
        for &pair in o.reference().pairs() {
            let reference = Image::read_png(&reference_path(o.set, o.reference(), pair, scale))?;
            let rune = harness.render(pair, scale, o.srgb_blend(), o.smoothing(), &cases, [SHEET_WIDTH, sheet_height(o.set)]);
            if (rune.width, rune.height) != (reference.width, reference.height) {
                return Err(format!("{} {scale}x: reference is {}x{}, Rune {}x{}", pair.name, reference.width, reference.height, rune.width, rune.height));
            }
            keep(pair, scale, &rune, &reference);
            for case in &cases {
                let stats = compare(&rune, &reference, case, scale);
                rows.push(Row { pair, scale, case: case.clone(), stats });
            }
        }
    }
    Ok(rows)
}

fn line(out: &mut String, label: &str, s: &Stats) {
    let _ = writeln!(
        out,
        "{label:<28} {:>7.2}% {:>7.2}% {:>4} {:>8.3} {:>7.3}",
        100.0 * s.differing_share(),
        100.0 * s.over8_share(),
        s.max,
        s.meanabs(),
        s.ink_ratio()
    );
}

/// The paragraph's stats for `pair` at `scale`, the report's headline numbers.
pub fn paragraph(rows: &[Row], pair: &str, scale: f32) -> Option<Stats> {
    rows.iter().find(|r| r.pair.name == pair && r.scale == scale && r.case.whole_region).map(|r| r.stats)
}

/// All rows of `pair` at `scale` but the paragraph, pooled by pixel.
pub fn pooled(rows: &[Row], pair: &str, scale: f32) -> Stats {
    let mut s = Stats::default();
    for r in rows.iter().filter(|r| r.pair.name == pair && r.scale == scale && !r.case.whole_region) {
        s.add(&r.stats);
    }
    s
}

const HEAD: &str = "case                           differ       >8  max  meanabs     ink\n";

/// The full table, then a summary per pair and scale.
pub fn table(o: &Options, rows: &[Row]) -> String {
    let mut out = String::new();
    let against = match o.reference() { Reference::Smoothed => "smoothing on", Reference::Unsmoothed => "smoothing off" };
    let _ = writeln!(out, "{}: Rune {:?} ({} blending) vs CoreText with {against}", o.set.name(), o.smoothing(), if o.srgb_blend() { "sRGB" } else { "linear" });
    for scale in SCALES {
        for pair in o.reference().pairs() {
            let _ = writeln!(out, "\n{} ({} on {}), {scale}x", pair.name, pair.fg, pair.bg);
            out += HEAD;
            for r in rows.iter().filter(|r| r.pair.name == pair.name && r.scale == scale) {
                line(&mut out, &r.case.name, &r.stats);
            }
        }
    }
    let _ = writeln!(out, "\nsummary: paragraph (report band) and all sample rows pooled");
    out += HEAD;
    for scale in SCALES.iter().rev() {
        for pair in o.reference().pairs() {
            if let Some(p) = paragraph(rows, pair.name, *scale) {
                line(&mut out, &format!("{} {scale}x paragraph", pair.name), &p);
            }
            line(&mut out, &format!("{} {scale}x rows", pair.name), &pooled(rows, pair.name, *scale));
        }
    }
    out
}

/// Rune's Inter paragraph against a 2× browser screenshot of the same page (text at x = 49 pt,
/// Inter 16/26, wrapped at 567 pt). CSS places the line box differently from Rune, so Rune's
/// render is moved by the whole-pixel offset (within ±120 px down, ±4 px across) that minimises
/// the mean difference over the shot's ink bounds; returns that offset and the stats there.
pub fn compare_shot(shot: &Image, pair: Pair, smoothing: FontSmoothing, srgb_blend: bool) -> Result<([i64; 2], Stats), String> {
    let scale = 2.0;
    let mut harness = Harness::new()?;
    let case = paragraph_case(Family::Sans, 0.0);
    let rune = harness.render(pair, scale, srgb_blend, smoothing, &[case], [shot.width as f32 / scale, shot.height as f32 / scale]);
    let all = [0, 0, shot.width, shot.height];
    let bounds = ink_bounds([shot, shot], all).ok_or("no ink in the screenshot")?;
    let band = [0, bounds[1], shot.width, bounds[3]];
    let mut best: Option<([i64; 2], Stats)> = None;
    for dy in -120..=120 {
        for dx in -4..=4 {
            let s = stats_in(&rune, shot, band, [dx, dy]);
            if best.as_ref().is_none_or(|b| s.meanabs() < b.1.meanabs()) {
                best = Some(([dx, dy], s));
            }
        }
    }
    best.ok_or_else(|| "nothing compared".into())
}

/// Heatmap of `rune` against `reference`: grey where equal, red where Rune has less ink, blue
/// where it has more (differences amplified four times).
pub fn heatmap(rune: &Image, reference: &Image) -> Image {
    let (paper_rune, paper_ref) = (rune.px(0, 0)[1] as i32, reference.px(0, 0)[1] as i32);
    let mut rgb = Vec::with_capacity(rune.rgb.len());
    for (a, b) in rune.rgb.chunks_exact(3).zip(reference.rgb.chunks_exact(3)) {
        let (ia, ib) = ((a[1] as i32 - paper_rune).abs(), (b[1] as i32 - paper_ref).abs());
        let k = ((ib - ia).abs() * 4).min(255) as u8;
        rgb.extend_from_slice(&if a[1] == b[1] {
            let g = (252 - ia / 4) as u8;
            [g, g, g]
        } else if ib > ia {
            [255, 255 - k, 255 - k]
        } else {
            [255 - k, 255 - k, 255]
        });
    }
    Image { width: rune.width, height: rune.height, rgb }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "pixel_compare_tests.rs"]
mod tests;
