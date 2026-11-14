//! Checks against CoreGraphics output captured on macOS 15.3 (24D60): dilated paths, native
//! autohint transforms, the luminance sweep and glyph masks with smoothing on and off. The
//! full corpora run with `cargo test -p rune --lib macos_smoothing -- --ignored`:
//! `RUNE_MACOS_SMOOTHING_CORPUS=<dir with paths.jsonl, cases.jsonl, masks.bin>` (Inter and
//! Helvetica: 1,800 paths, 14,400 mask pairs; `RUNE_MACOS_SMOOTHING_FONTS=<dir>` adds Inter
//! Bold) and `RUNE_MACOS_SMOOTHING_SF=<dir>` (SF Pro from the system's SFNS.ttf). `LIST=1`
//! prints every case over 3 levels; `NO_STEM=1` drops the descriptor stemH.

use serde_json::Value;
use swash::FontRef;
use swash::scale::ScaleContext;

use super::*;

const INTER: &[u8] = include_bytes!("../../../assets/Inter-Regular.ttf");
/// Corpus tiles: the pen sits at x = 8 and the baseline 24 px above the bottom edge.
const TILE: (usize, usize) = (96, 112);

fn parse_path(v: &Value) -> Path {
    v.as_array().unwrap().iter().map(|r| {
        let r = r.as_array().unwrap();
        let p = |i: usize| [r[i].as_f64().unwrap(), r[i + 1].as_f64().unwrap()];
        match r[0].as_u64().unwrap() {
            0 => Cmd::Move(p(1)),
            1 => Cmd::Line(p(1)),
            2 => Cmd::Quad(p(1), p(3)),
            3 => Cmd::Cubic(p(1), p(3), p(5)),
            _ => Cmd::Close,
        }
    }).collect()
}

fn points(c: &Cmd) -> Vec<Point> {
    match *c {
        Cmd::Move(p) | Cmd::Line(p) => vec![p],
        Cmd::Quad(a, b) => vec![a, b],
        Cmd::Cubic(a, b, c) => vec![a, b, c],
        Cmd::Close => vec![],
    }
}

fn rgb(v: &Value) -> [f64; 3] {
    std::array::from_fn(|i| srgb_to_linear(v[i].as_f64().unwrap()))
}

/// Largest coordinate difference, or None when the command structure differs.
fn path_error(model: &Path, oracle: &Path) -> Option<f64> {
    let same = model.len() == oracle.len()
        && model.iter().zip(oracle).all(|(a, b)| std::mem::discriminant(a) == std::mem::discriminant(b));
    same.then(|| {
        model.iter().zip(oracle).flat_map(|(a, b)| points(a).into_iter().zip(points(b)))
            .fold(0.0f64, |m, (a, b)| m.max((a[0] - b[0]).abs()).max((a[1] - b[1]).abs()))
    })
}

/// A prepared glyph's mask placed in a corpus tile.
fn tile(path: &Path) -> Vec<u8> {
    let (w, h) = TILE;
    let mut out = vec![0u8; w * h];
    if path.is_empty() {
        return out;
    }
    let (data, at) = mask(path);
    for row in 0..at.height as i32 {
        for col in 0..at.width as i32 {
            let (x, y) = (8 + at.left + col, h as i32 - 24 - at.top + row);
            assert!(x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h, "glyph outside the tile");
            out[y as usize * w + x as usize] = data[(row * at.width as i32 + col) as usize];
        }
    }
    out
}

#[derive(Default, Debug)]
struct Stats {
    pixels: u64,
    sum: u64,
    max: u8,
    /// |error| = 0, 1, 2, 3, more.
    hist: [u64; 5],
    cases: u64,
}

impl Stats {
    /// Compares over the union of both masks' ink plus a one-pixel border, as the spec does.
    fn add(&mut self, a: &[u8], b: &[u8]) {
        let (w, h) = TILE;
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if a[y * w + x] != 0 || b[y * w + x] != 0 {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
            }
        }
        self.cases += 1;
        for y in y0.saturating_sub(1)..(y1 + 1).min(h) {
            for x in x0.saturating_sub(1)..(x1 + 1).min(w) {
                let d = a[y * w + x].abs_diff(b[y * w + x]);
                self.pixels += 1;
                self.sum += d as u64;
                self.max = self.max.max(d);
                self.hist[(d as usize).min(4)] += 1;
            }
        }
    }

    fn mae(&self) -> f64 {
        self.sum as f64 / self.pixels.max(1) as f64
    }

    fn line(&self) -> String {
        let pct = |n: u64| 100.0 * n as f64 / self.pixels.max(1) as f64;
        format!("cases {} pixels {} mae {:.5} max {} | 0: {:.3}% 1: {:.3}% 2: {:.3}% 3: {:.3}% >3: {:.3}%",
            self.cases, self.pixels, self.mae(), self.max, pct(self.hist[0]), pct(self.hist[1]), pct(self.hist[2]), pct(self.hist[3]), pct(self.hist[4]))
    }
}

#[test]
fn radius_matches_the_native_luminance_sweep() {
    let sweep: Value = serde_json::from_str(include_str!("smoothing_data/sweep.json")).unwrap();
    let steps = sweep["steps"].as_u64().unwrap();
    let ppem = sweep["ppem"].as_f64().unwrap();
    let transitions: Vec<(u64, f64)> = sweep["transitions"].as_array().unwrap().iter()
        .map(|t| (t[0].as_u64().unwrap(), t[1].as_f64().unwrap())).collect();
    for i in 0..=steps {
        let v = i as f64 / steps as f64;
        let want = transitions.iter().rev().find(|t| t.0 <= i).unwrap().1;
        let lin = srgb_to_linear(v);
        assert_eq!(radius_for_bucket(ppem, luminance_bucket([lin; 3])), want, "grey {v}");
    }
    for c in sweep["chromatic"].as_array().unwrap() {
        assert_eq!(radius_for_bucket(ppem, luminance_bucket(rgb(&c[0]))), c[1].as_f64().unwrap(), "{c}");
    }
}

#[test]
fn radius_grows_with_ppem_to_the_f32_cap() {
    // Native radii for #1d1e22 (bucket 0).
    for (ppem, want) in [(8.0, 0.0464), (32.0, 0.1856), (51.0, 0.29579999999999995),
        (52.0, 0.30000001192092896), (200.0, 0.30000001192092896)] {
        assert_eq!(radius_for_bucket(ppem, color_bucket(crate::color::hex("#1d1e22").unwrap())), want);
    }
    assert_eq!(color_bucket(crate::color::hex("#e6e6e6").unwrap()), 3);
    assert_eq!(color_bucket(crate::color::hex("#f1f1f1").unwrap()), 4);
    assert_eq!(color_bucket(crate::color::hex("#ff0000").unwrap()), 1);
}

#[test]
fn dilation_matches_coregraphics_paths() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("smoothing_data/dilation.json")).unwrap();
    let mut worst = 0.0f64;
    for c in &cases {
        let d = radius_for_bucket(c["ppem"].as_f64().unwrap(), luminance_bucket(rgb(&c["color"])));
        let model = dilate(&parse_path(&c["path"]), d, d);
        let err = path_error(&model, &parse_path(&c["oracle"])).unwrap_or_else(|| panic!("structure differs in path {}", c["id"]));
        worst = worst.max(err);
    }
    println!("{} paths, max coordinate error {worst:e} px", cases.len());
    assert!(worst < 1e-11, "max coordinate error {worst:e}");
}

/// The fonts' metrics as the native hint capture reported them, keyed by font name.
fn native_metrics(rows: &[Value]) -> std::collections::BTreeMap<String, HintMetrics> {
    let f = |r: &Value, k: &str| r[k].as_f64().unwrap();
    rows.iter().filter(|r| r["valid"] == 1).map(|r| (r["font"].as_str().unwrap().to_owned(), HintMetrics {
        upem: f(r, "upem"), x_height: f(r, "otop"), cap_height: f(r, "Xtop"), max_y: f(r, "fonttop"), stem_height: f(r, "stem"),
    })).collect()
}

#[test]
fn autohint_matches_native_transforms() {
    let rows: Vec<Value> = include_str!("smoothing_data/hints.jsonl").lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let metrics = native_metrics(&rows);
    let (mut count, mut worst, mut mismatches) = (0, 0.0f64, 0);
    for r in &rows {
        let m = metrics[r["font"].as_str().unwrap()];
        let t = autohint(m, r["ppem"].as_f64().unwrap(), r["d"].as_f64().unwrap());
        mismatches += (t.is_some() != (r["valid"] == 1)) as u32;
        if let Some(t) = t {
            count += 1;
            for (i, v) in t.values.iter().enumerate() {
                worst = worst.max((v - r["transform"][i].as_f64().unwrap()).abs());
            }
        }
    }
    println!("{count} transforms, max parameter error {worst:e}, {mismatches} validity mismatches");
    assert_eq!((count, mismatches), (180, 0));
    assert!(worst < 1e-12, "max parameter error {worst:e}");
}

#[test]
fn swash_metrics_match_the_native_hint_inputs() {
    let font = FontRef::from_index(INTER, 0).unwrap();
    let m = truetype_metrics(font, &[], &mut ScaleContext::new()).unwrap();
    assert_eq!(m, HintMetrics { upem: 2048.0, x_height: 1132.0, cap_height: 1490.0, max_y: 2269.0, stem_height: 0.0 });
}

#[test]
fn flattening_emits_lines_within_coregraphics_tolerance() {
    let path = vec![Cmd::Move([0.0, 0.0]), Cmd::Cubic([0.0, 10.0], [10.0, 10.0], [10.0, 0.0]), Cmd::Quad([5.0, -6.0], [0.0, 0.0]), Cmd::Close];
    let flat = flatten(&path);
    assert!(flat.iter().all(|c| matches!(c, Cmd::Move(_) | Cmd::Line(_) | Cmd::Close)));
    assert!(flat.contains(&Cmd::Line([10.0, 0.0])) && flat[flat.len() - 2] == Cmd::Line([0.0, 0.0]));
    let lines = flat.iter().filter(|c| matches!(c, Cmd::Line(_))).count();
    assert!(lines > 8 && lines < 64, "{lines}");
}

/// A committed reference mask, cropped to its ink, back in its corpus tile.
fn reference_tile(blob: &[u8], crop: &Value) -> Vec<u8> {
    let [x0, y0, cw, ch, at] = std::array::from_fn(|i| crop[i].as_u64().unwrap() as usize);
    let w = TILE.0;
    let mut out = vec![0u8; TILE.0 * TILE.1];
    for y in 0..ch {
        out[(y0 + y) * w + x0..(y0 + y) * w + x0 + cw].copy_from_slice(&blob[at + y * cw..at + (y + 1) * cw]);
    }
    out
}

/// Inter Regular through each pipeline against CoreGraphics with smoothing on (dark and white
/// ink) and off, at hinted and unhinted sizes and two phases.
#[test]
fn masks_match_coregraphics_within_three_levels() {
    let index: Vec<Value> = serde_json::from_str(include_str!("smoothing_data/masks.json")).unwrap();
    let blob = include_bytes!("smoothing_data/masks.bin");
    let font = FontRef::from_index(INTER, 0).unwrap();
    let mut cx = ScaleContext::new();
    let metrics = truetype_metrics(font, &[], &mut cx);
    for (pipeline, tag) in [(ApplePipeline::SMOOTHED, "on"), (ApplePipeline::UNSMOOTHED, "off")] {
        let mut stats = Stats::default();
        for c in &index {
            let ppem = c["ppem"].as_f64().unwrap();
            let phase = [c["phase"][0].as_f64().unwrap(), c["phase"][1].as_f64().unwrap()];
            let outline = design_outline(&mut cx, font, &[], c["glyph"].as_u64().unwrap() as u16, ppem).unwrap();
            let d = radius_for_bucket(ppem, luminance_bucket(rgb(&c["color"])));
            let ours = tile(&prepare_outline(&outline, pipeline, ppem, d, phase, metrics));
            let before = stats.max;
            stats.add(&ours, &reference_tile(blob, &c[tag]));
            assert!(stats.max <= 3 || before > 3, "smoothing {tag}: case {} ({} at {ppem} px) differs by {}", c["case"], c["char"], stats.max);
        }
        println!("smoothing {tag}: {}", stats.line());
        assert!(stats.max <= 3 && stats.mae() < 0.2, "{}", stats.line());
    }
}

/// The text system's own path: light ink dilates more than dark ink, so it gets its own masks.
#[test]
fn text_system_keys_glyphs_by_ink() {
    let mut ts = crate::text::TextSystem::default();
    ts.set_apple_pipeline(ApplePipeline::SMOOTHED);
    let style = crate::tok::TextStyle::new(16.0, 400.0);
    let mut dark = Vec::new();
    let mut light = Vec::new();
    ts.set_glyph_inks(crate::color::hex("#1d1e22").unwrap(), &[]);
    ts.glyphs("Hamburg", style, 1.0, [0.0, 0.0], &mut dark);
    let before = ts.atlas.rasterized();
    ts.set_glyph_inks(crate::color::Color::WHITE, &[]);
    ts.glyphs("Hamburg", style, 1.0, [0.0, 0.0], &mut light);
    assert_eq!(dark.len(), light.len());
    assert!(ts.atlas.rasterized() > before);
    let area = |q: &[crate::text::GlyphQuad]| q.iter().map(|q| q.rect[2] * q.rect[3]).sum::<f32>();
    assert!(area(&light) >= area(&dark));
    assert!(dark.iter().zip(&light).any(|(a, b)| a.uv != b.uv));
}

/// Each pipeline rasterizes its own masks; undilated glyphs ignore ink.
#[test]
fn pipelines_never_share_masks() {
    let mut ts = crate::text::TextSystem::default();
    let style = crate::tok::TextStyle::new(16.0, 400.0);
    let mut seen = Vec::new();
    for pipeline in [ApplePipeline::SMOOTHED, ApplePipeline::UNSMOOTHED, ApplePipeline { flatten: false, ..ApplePipeline::UNSMOOTHED }] {
        ts.set_apple_pipeline(pipeline);
        let before = ts.atlas.rasterized();
        let mut q = Vec::new();
        ts.glyphs("Hamburg", style, 1.0, [0.0, 0.0], &mut q);
        assert!(ts.atlas.rasterized() > before, "{pipeline:?} reused another pipeline's masks");
        seen.push(q);
    }
    ts.set_apple_pipeline(ApplePipeline::UNSMOOTHED);
    ts.set_glyph_inks(crate::color::Color::WHITE, &[]);
    let before = ts.atlas.rasterized();
    ts.glyphs("Hamburg", style, 1.0, [0.0, 0.0], &mut Vec::new());
    assert_eq!(ts.atlas.rasterized(), before);
}

// ---------------------------------------------------------------------------------------------
// Full corpus and cost measurements: explicit runs only.

fn corpus_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("RUNE_MACOS_SMOOTHING_CORPUS").map(Into::into)
}

/// Inter Regular and Bold (the corpus's TTFs, `RUNE_MACOS_SMOOTHING_FONTS`) and the system
/// Helvetica, each with the face index that has the corpus's glyph ids.
fn corpus_fonts() -> Vec<(&'static str, Vec<u8>, usize)> {
    let mut fonts = vec![("Inter-Regular", INTER.to_vec(), 0)];
    if let Some(dir) = std::env::var_os("RUNE_MACOS_SMOOTHING_FONTS")
        && let Ok(bold) = std::fs::read(std::path::Path::new(&dir).join("Inter-Bold.ttf"))
    {
        fonts.push(("Inter-Bold", bold, 0));
    }
    if let Ok(helvetica) = std::fs::read("/System/Library/Fonts/Helvetica.ttc") {
        let full = |f: FontRef<'_>| f.localized_strings().find_by_id(swash::StringId::Full, None).map(|s| s.chars().collect::<String>());
        let index = swash::FontDataRef::new(&helvetica).and_then(|d| d.fonts().position(|f| full(f).as_deref() == Some("Helvetica")));
        if let Some(index) = index {
            fonts.push(("Helvetica", helvetica, index));
        }
    }
    fonts
}

#[test]
#[ignore = "needs RUNE_MACOS_SMOOTHING_CORPUS"]
fn full_corpus() {
    use std::io::{BufRead, Read};
    let dir = corpus_dir().expect("RUNE_MACOS_SMOOTHING_CORPUS");
    let lines = |name: &str| -> Vec<Value> {
        std::io::BufReader::new(std::fs::File::open(dir.join(name)).unwrap()).lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect()
    };
    let paths = lines("paths.jsonl");
    let (mut worst, mut structure) = (0.0f64, 0);
    for p in &paths {
        let d = radius_for_bucket(p["ppem"].as_f64().unwrap(), luminance_bucket(rgb(&p["color"])));
        if let Some(e) = path_error(&dilate(&parse_path(&p["path"]), d, d), &parse_path(&p["oracle"])) {
            structure += 1;
            worst = worst.max(e);
        }
    }
    println!("dilation: {structure}/{} paths with matching structure, max coordinate error {worst:e} px", paths.len());

    let fonts = corpus_fonts();
    let mut cx = ScaleContext::new();
    let refs: Vec<_> = fonts.iter().map(|(name, data, index)| {
        let font = FontRef::from_index(data, *index).unwrap();
        let mut metrics = truetype_metrics(font, &[], &mut cx);
        if std::env::var_os("NO_STEM").is_some() {
            metrics = metrics.map(|m| HintMetrics { stem_height: 0.0, ..m });
        }
        println!("{name}: {metrics:?}");
        (*name, font, metrics)
    }).collect();
    for (name, font, _) in &refs {
        let (mut same, mut total, mut worst) = (0, 0, 0.0f64);
        for p in paths.iter().filter(|p| p["font"] == *name) {
            total += 1;
            let ours = design_outline(&mut cx, *font, &[], p["glyph"].as_u64().unwrap() as u16, p["ppem"].as_f64().unwrap()).unwrap();
            if let Some(e) = path_error(&ours, &parse_path(&p["path"])) {
                same += 1;
                worst = worst.max(e);
            } else if std::env::var_os("DBG").is_some() && p["bucket"] == 0 && p["ppem"] == 8.0 {
                println!("{name} {} ours {:?}\nct {:?}", p["char"], ours, parse_path(&p["path"]));
            }
        }
        println!("{name}: swash outlines match CoreText's structure in {same}/{total}, max coordinate error {worst:e} px");
    }
    let mut groups: std::collections::BTreeMap<String, Stats> = Default::default();
    let mut masks = std::io::BufReader::new(std::fs::File::open(dir.join("masks.bin")).unwrap());
    let mut cg = vec![0u8; TILE.0 * TILE.1];
    let mut off = cg.clone();
    let undilated_no_hint = ApplePipeline { hint: false, ..ApplePipeline::UNSMOOTHED };
    let undilated_zeno = ApplePipeline { flatten: false, ..ApplePipeline::UNSMOOTHED };
    let runs = [
        ("on", ApplePipeline::SMOOTHED, true),
        ("off", ApplePipeline::UNSMOOTHED, false),
        ("off, no hinter", undilated_no_hint, false),
        ("off, zeno flattening", undilated_zeno, false),
    ];
    for c in lines("cases.jsonl") {
        masks.read_exact(&mut cg).unwrap();
        masks.read_exact(&mut off).unwrap();
        let p = &paths[c["path"].as_u64().unwrap() as usize];
        let Some((name, font, metrics)) = refs.iter().find(|r| r.0 == p["font"]) else { continue };
        let ppem = p["ppem"].as_f64().unwrap();
        let phase = [c["phase"][0].as_f64().unwrap(), c["phase"][1].as_f64().unwrap()];
        let bucket = luminance_bucket(rgb(&p["color"]));
        let outline = design_outline(&mut cx, *font, &[], p["glyph"].as_u64().unwrap() as u16, ppem).unwrap();
        let scale = c["scale"].as_u64().unwrap();
        let hinted = phase[1] == 0.0 && ppem <= 18.0;
        let ink = match bucket { 0 => " dark", 4 => " light", _ => "" };
        for (run, pipeline, smoothed) in runs {
            let ours = tile(&prepare_outline(&outline, pipeline, ppem, radius_for_bucket(ppem, bucket), phase, *metrics));
            let reference = if smoothed { &cg } else { &off };
            let mut one = Stats::default();
            one.add(&ours, reference);
            if one.max > 3 && (run == "on" || run == "off") && std::env::var_os("LIST").is_some() {
                println!("over 3 [{run}]: {name} {} {ppem} px bucket {bucket} phase {phase:?} {scale}x: max {}", p["char"], one.max);
            }
            for g in [format!("{run}: all"), format!("{run}: {scale}x{ink}"), format!("{run}: font {name}"),
                format!("{run}: {}", if hinted { "hinted sizes" } else { "unhinted" })] {
                groups.entry(g).or_default().add(&ours, reference);
            }
            if run == "on" || run == "off" {
                groups.entry(format!("{run}: ppem {ppem:>2}")).or_default().add(&ours, reference);
            }
        }
    }
    for (g, s) in &groups {
        println!("{g:<20} {}", s.line());
    }
    assert!(worst < 1e-11 && structure == paths.len());
    // zeno's f32 coverage at a different integer offset than the spec's tiles adds the odd level.
    assert!(groups["on: all"].max <= 4 && groups["off: all"].max <= 3);
}

/// SF Pro from the system's SFNS.ttf against a CoreText capture of it (body 16 px at opsz 16,
/// headings 28–32 px at opsz = size; `RUNE_MACOS_SMOOTHING_SF=<dir>`). `SF_STEM` overrides the
/// descriptor stemH the autohinter uses.
#[test]
#[ignore = "needs RUNE_MACOS_SMOOTHING_SF and macOS's SFNS.ttf"]
fn sf_corpus() {
    use std::io::{BufRead, Read};
    let dir: std::path::PathBuf = std::env::var_os("RUNE_MACOS_SMOOTHING_SF").expect("RUNE_MACOS_SMOOTHING_SF").into();
    let lines = |name: &str| -> Vec<Value> {
        std::io::BufReader::new(std::fs::File::open(dir.join(name)).unwrap()).lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect()
    };
    let data = std::fs::read("/System/Library/Fonts/SFNS.ttf").unwrap();
    let font = FontRef::from_index(&data, 0).unwrap();
    let stem: Option<f64> = std::env::var("SF_STEM").ok().and_then(|v| v.parse().ok());
    let mut cx = ScaleContext::new();
    let coords = |p: &Value| -> Vec<i16> {
        font.variations().normalized_coords([("opsz", p["opsz"].as_f64().unwrap() as f32), ("wght", p["wght"].as_f64().unwrap() as f32)]).collect()
    };
    let paths = lines("paths.jsonl");
    // Native dilation and autohint transforms.
    let (mut worst, mut structure) = (0.0f64, 0);
    for p in &paths {
        let d = radius_for_bucket(p["ppem"].as_f64().unwrap(), luminance_bucket(rgb(&p["color"])));
        // serde_json parses decimals to within an ulp; the radius itself is exact.
        assert!((d - p["dy"].as_f64().unwrap()).abs() < 1e-15, "{d} vs {}", p["dy"]);
        if let Some(e) = path_error(&dilate(&parse_path(&p["path"]), d, d), &parse_path(&p["oracle"])) {
            structure += 1;
            worst = worst.max(e);
        }
    }
    println!("SF dilation: {structure}/{} paths with matching structure, max coordinate error {worst:e} px", paths.len());
    let (mut hints, mut hint_err, mut validity) = (0, 0.0f64, 0);
    let mut metric_err = 0.0f64;
    for r in lines("hints.jsonl") {
        let c = coords(&r);
        let m = truetype_metrics(font, &c, &mut cx).unwrap();
        metric_err = metric_err.max((m.x_height - r["otop"].as_f64().unwrap()).abs()).max((m.cap_height - r["Xtop"].as_f64().unwrap()).abs())
            .max((m.max_y - r["fonttop"].as_f64().unwrap()).abs());
        let m = HintMetrics { stem_height: r["stem"].as_f64().unwrap(), ..m };
        let t = autohint(m, r["ppem"].as_f64().unwrap(), r["d"].as_f64().unwrap());
        validity += (t.is_some() != (r["valid"] == 1)) as u32;
        if let Some(t) = t {
            hints += 1;
            for (i, v) in t.values.iter().enumerate() {
                hint_err = hint_err.max((v - r["transform"][i].as_f64().unwrap()).abs());
            }
        }
    }
    println!("SF descriptor stemH: {}", truetype_metrics(font, &[], &mut cx).unwrap().stem_height);
    println!("SF autohint: swash metrics vs CoreGraphics max error {metric_err} units; {hints} transforms with the native stemH, max parameter error {hint_err:e}, {validity} validity mismatches");

    let mut groups: std::collections::BTreeMap<String, Stats> = Default::default();
    let mut masks = std::io::BufReader::new(std::fs::File::open(dir.join("masks.bin")).unwrap());
    let mut on = vec![0u8; TILE.0 * TILE.1];
    let mut off = on.clone();
    let mut metrics: std::collections::HashMap<Vec<i16>, Option<HintMetrics>> = Default::default();
    for c in lines("cases.jsonl") {
        masks.read_exact(&mut on).unwrap();
        masks.read_exact(&mut off).unwrap();
        let p = &paths[c["path"].as_u64().unwrap() as usize];
        let co = coords(p);
        let m = *metrics.entry(co.clone()).or_insert_with(|| truetype_metrics(font, &co, &mut cx).map(|m| HintMetrics { stem_height: stem.unwrap_or(m.stem_height), ..m }));
        let ppem = p["ppem"].as_f64().unwrap();
        let phase = [c["phase"][0].as_f64().unwrap(), c["phase"][1].as_f64().unwrap()];
        let bucket = luminance_bucket(rgb(&p["color"]));
        let outline = design_outline(&mut cx, font, &co, p["glyph"].as_u64().unwrap() as u16, ppem).unwrap();
        let scale = c["scale"].as_u64().unwrap();
        let ink = match bucket { 0 => " dark", 4 => " light", _ => "" };
        let size = format!("{ppem} px wght {}", p["wght"]);
        for (run, pipeline, reference) in [("on", ApplePipeline::SMOOTHED, &on), ("off", ApplePipeline::UNSMOOTHED, &off)] {
            let ours = tile(&prepare_outline(&outline, pipeline, ppem, radius_for_bucket(ppem, bucket), phase, m));
            let mut one = Stats::default();
            one.add(&ours, reference);
            if one.max > 3 && std::env::var_os("LIST").is_some() {
                println!("over 3 [{run}]: {} {size} bucket {bucket} phase {phase:?} {scale}x: max {}", p["char"], one.max);
            }
            for g in [format!("{run}: all"), format!("{run}: {scale}x{ink}"), format!("{run}: {size}"),
                format!("{run}: {}", if phase[1] == 0.0 && ppem <= 18.0 { "hinted (16 px, phase y 0)" } else { "unhinted" })] {
                groups.entry(g).or_default().add(&ours, reference);
            }
        }
    }
    for (g, s) in &groups {
        println!("SF {g:<28} {}", s.line());
    }
}

/// Rasterization time per glyph on the macOS path, smoothed and unsmoothed, and the atlas
/// texels five ink buckets need against one.
#[test]
#[ignore = "timing; run in release"]
fn rasterize_cost() {
    use std::time::Instant;
    let font = FontRef::from_index(INTER, 0).unwrap();
    let mut cx = ScaleContext::new();
    let text = "The quick brown fox jumps over the lazy dog. SPHINX OF BLACK QUARTZ, JUDGE MY VOW! 0123456789 @&%?";
    let glyphs: Vec<u16> = {
        let mut ids: Vec<u16> = text.chars().map(|ch| font.charmap().map(ch)).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let rounds = 20;
    for (ppem, scale) in [(13.0f32, 1.0f32), (16.0, 1.0), (26.0, 2.0), (32.0, 2.0)] {
        let n = (rounds * glyphs.len() * 4) as f64;
        let mut smoothing = Smoothing::default();
        smoothing.set_pipeline(ApplePipeline::SMOOTHED);
        let mut per_bucket = [0usize; 5];
        let start = Instant::now();
        for _ in 0..rounds {
            for &g in &glyphs {
                for subpx in 0..4 {
                    let radius = radius_for_bucket(ppem as f64, 0);
                    let job = Job { font, key: (0, 0, 0), coords: &[], glyph: g, ppem, phase_x: subpx as f32 / 4.0, shift_y: 0.0, radius, skew: 0 };
                    per_bucket[0] += smoothing.mask(&mut cx, &job).map_or(0, |m| m.0.len());
                }
            }
        }
        let new = start.elapsed().as_secs_f64() * 1e6 / n;
        smoothing.set_pipeline(ApplePipeline::UNSMOOTHED);
        let start = Instant::now();
        for _ in 0..rounds {
            for &g in &glyphs {
                for subpx in 0..4 {
                    let job = Job { font, key: (0, 0, 0), coords: &[], glyph: g, ppem, phase_x: subpx as f32 / 4.0, shift_y: 0.0, radius: 0.0, skew: 0 };
                    std::hint::black_box(smoothing.mask(&mut cx, &job));
                }
            }
        }
        let unsmoothed = start.elapsed().as_secs_f64() * 1e6 / n;
        smoothing.set_pipeline(ApplePipeline::SMOOTHED);
        let mut radii = vec![radius_for_bucket(ppem as f64, 0)];
        for (b, slot) in per_bucket.iter_mut().enumerate().skip(1) {
            let radius = radius_for_bucket(ppem as f64, b as u8);
            if radii.contains(&radius) {
                continue;
            }
            radii.push(radius);
            for &g in &glyphs {
                for subpx in 0..4 {
                    let job = Job { font, key: (0, 0, 0), coords: &[], glyph: g, ppem, phase_x: subpx as f32 / 4.0, shift_y: 0.0, radius, skew: 0 };
                    *slot += smoothing.mask(&mut cx, &job).map_or(0, |m| m.0.len()) * rounds;
                }
            }
        }
        let one = per_bucket[0] as f64;
        println!("{ppem} px ({scale}x): {:.2} us/glyph smoothed, {:.2} us/glyph unsmoothed; texels {}, all five buckets {} distinct radii, x{:.2} of one",
            new, unsmoothed, per_bucket[0] / rounds, radii.len(), per_bucket.iter().sum::<usize>() as f64 / one);
    }
}

