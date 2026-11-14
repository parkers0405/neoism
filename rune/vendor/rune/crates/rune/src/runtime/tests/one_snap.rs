//! Sub-pixel and pixel-snapped concentric placement across fills, vector shapes, icons and text
//! (`t1193`): two primitives in the same box keep the same centre and drawn size whether still,
//! scaled, anchored, gliding on a spring, or swapping between `canvas` and `icon`.
use crate::el::{Align, Side};
use crate::render::{Gpu, KIND_GLYPH, KIND_RECT, Offscreen, Renderer, write_png};
use crate::shape::Shape;
use crate::{Color, Cx, El, Env, Icon, Runtime, TextStyle, canvas, col, el, icon, row, text};
use rune_motion::Spring;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const GREEN: Color = Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
const BLUE: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };

static RING_ICON: Icon = Icon {
    name: "ring",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor" stroke-width="1.5" fill="none"/>"#,
};

static DISC_ICON: Icon = Icon {
    name: "disc",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="4" fill="currentColor"/>"#,
};

/// Symmetric 64-gon around `(cx, cy)`: 4-fold symmetric so its exact centroid is `(cx, cy)`.
fn circle_pts(cx: f32, cy: f32, r: f32) -> Vec<[f32; 2]> {
    (0..64)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / 64.0;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}

fn disc_shape(r: f32, color: Color) -> Shape {
    Shape::new().fill(circle_pts(8.0, 8.0, r), color)
}

struct Bench {
    gpu: Gpu,
    renderer: Renderer,
    target: Offscreen,
    dw: u32,
    dh: u32,
}

impl Bench {
    fn new(w: u32, h: u32, scale: f32) -> Self {
        let gpu = Gpu::headless();
        let renderer = Renderer::new(&gpu.device, FORMAT);
        let (dw, dh) = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
        let target = Offscreen::with_format(&gpu.device, dw, dh, FORMAT);
        Self { gpu, renderer, target, dw, dh }
    }

    fn draw<S: 'static>(&mut self, rt: &mut Runtime<S>, clear: [f64; 4]) -> Vec<u8> {
        self.renderer.prepare(
            &self.gpu.device,
            &self.gpu.queue,
            &mut rt.text.atlas,
            &rt.instances,
            (self.dw, self.dh),
        );
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.draw(
            &mut enc,
            &self.target.view,
            clear.map(|v| v as f32),
        );
        self.target.copy(&mut enc);
        self.renderer.submit(&self.gpu.queue, enc);
        self.target.read(&self.gpu.device)
    }
}

/// Coverage-weighted `[cx, cy]` (device px) and total pixel area (`sum(cov / 255)`) of channel `ch`
/// inside `rgba` (`dw × dh`).
fn channel_moments(rgba: &[u8], dw: u32, dh: u32, ch: usize) -> ([f32; 2], f32) {
    let (mut sum, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64);
    for y in 0..dh {
        for x in 0..dw {
            let v = rgba[((y * dw + x) * 4) as usize + ch] as f64 / 255.0;
            if v > 0.0 {
                sum += v;
                sx += v * (x as f64 + 0.5);
                sy += v * (y as f64 + 0.5);
            }
        }
    }
    assert!(sum > 0.0, "channel {ch} must have non-zero coverage");
    ([(sx / sum) as f32, (sy / sum) as f32], sum as f32)
}

fn whole(v: f32) -> bool {
    (v - v.round()).abs() < 1e-4
}

fn percentile(mut xs: Vec<f32>, p: f32) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(f32::total_cmp);
    let idx = ((xs.len() - 1) as f32 * p).round() as usize;
    xs[idx]
}

#[derive(Clone, Copy)]
struct ScaledCase {
    at: [f32; 2],
    zoom: f32,
    anchored: bool,
    only_icon_disc: bool,
}

fn scaled_view(s: &ScaledCase, _: &mut Cx<ScaledCase>) -> El {
    let badge = if s.only_icon_disc {
        el().key("badge")
            .size(16.0, 16.0)
            .child(icon(&DISC_ICON, 16.0).color(RED).absolute(0.0, 0.0))
    } else {
        el().key("badge")
            .size(16.0, 16.0)
            .fill(BLUE)
            .child(icon(&RING_ICON, 16.0).color(RED).absolute(0.0, 0.0))
            .child(canvas(disc_shape(4.0, GREEN)).size(16.0, 16.0).absolute(0.0, 0.0))
    };
    if s.anchored {
        let page = col()
            .key("page")
            .size(120.0, 120.0)
            .scale(s.zoom)
            .origin(0.0, 0.0)
            .child(el().key("anchor").absolute(12.0 + s.at[0], 10.0 + s.at[1]).size(24.0, 8.0));
        let pop = col()
            .key("pop")
            .padding(6.0)
            .fill(Color { r: 0.0, g: 0.0, b: 0.25, a: 1.0 })
            .child(badge)
            .anchored("anchor", Side::Bottom, Align::Start, 4.0);
        col().size(160.0, 160.0).children([page, pop])
    } else {
        col().size(160.0, 160.0).child(
            col()
                .key("scaled")
                .size(100.0, 100.0)
                .scale(s.zoom)
                .origin(0.0, 0.0)
                .child(badge.absolute(14.0 + s.at[0], 14.0 + s.at[1])),
        )
    }
}

/// (1) Still-scaled concentric icon + `canvas` shape in the same fractional rect (1.6× zoom on 2×
/// display, both direct `El::scale` and `El::anchored`), plus matching drawn area (`515 px`, no 2%
/// rounding drop) and whole-pixel snapping of the held `1.6×` anchored popover.
#[test]
fn concentric_icon_and_shape_share_centre_and_size_at_scaled_rest() {
    let mut bench = Bench::new(160, 160, 2.0);
    for anchored in [false, true] {
        let mut max_err = 0.0f32;
        let (mut min_icon_area, mut max_icon_area) = (f32::MAX, 0.0f32);
        for iy in 0..5 {
            for ix in 0..5 {
                let at = [ix as f32 * 0.21, iy as f32 * 0.21];
                let mut rt = Runtime::new(
                    ScaledCase { at, zoom: 1.6, anchored, only_icon_disc: false },
                    scaled_view,
                    (160.0, 160.0),
                    Env::new(true, 2.0),
                );
                rt.frame(0.0);
                rt.frame(1.0 / 120.0);

                // Every resting fill inside the scaled / anchored subtree must snap to whole pixels.
                let fills: Vec<[f32; 4]> = rt
                    .instances
                    .iter()
                    .filter(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0)
                    .map(|i| i.rect)
                    .collect();
                assert!(!fills.is_empty());
                for r in &fills {
                    assert!(
                        r.iter().all(|&v| whole(v)),
                        "anchored={anchored} at={at:?}: held 1.6x fill must snap to whole device px, got {r:?}"
                    );
                }

                let rgba = bench.draw(&mut rt, [0.0; 4]);
                let (c_icon, _) = channel_moments(&rgba, bench.dw, bench.dh, 0);
                let (c_shape, shape_disc_area) = channel_moments(&rgba, bench.dw, bench.dh, 1);
                let err_x = (c_icon[0] - c_shape[0]).abs();
                let err_y = (c_icon[1] - c_shape[1]).abs();
                max_err = max_err.max(err_x).max(err_y);
                assert!(
                    err_x <= 0.125 && err_y <= 0.125,
                    "anchored={anchored} at={at:?}: icon centre {c_icon:?} vs shape centre {c_shape:?} (err=({err_x:.4}, {err_y:.4}))"
                );

                // Compare the drawn area of the r=4 disc icon across both 51 px and 52 px snapped
                // boxes (`iy == 2`, `ix = 0..5`): continuous `drawn` sizing keeps the icon's area
                // steady within 0.5% (no `51 <-> 52 px` 4% jump) and within 1.5% of the ear-clipped
                // polygon disc.
                if iy == 2 {
                    let mut rt_disc = Runtime::new(
                        ScaledCase { at, zoom: 1.6, anchored, only_icon_disc: true },
                        scaled_view,
                        (160.0, 160.0),
                        Env::new(true, 2.0),
                    );
                    rt_disc.frame(0.0);
                    rt_disc.frame(1.0 / 120.0);
                    let rgba_disc = bench.draw(&mut rt_disc, [0.0; 4]);
                    let (_, icon_disc_area) = channel_moments(&rgba_disc, bench.dw, bench.dh, 0);
                    min_icon_area = min_icon_area.min(icon_disc_area);
                    max_icon_area = max_icon_area.max(icon_disc_area);
                    let rel = (icon_disc_area - shape_disc_area).abs() / shape_disc_area;
                    assert!(
                        rel < 0.015,
                        "anchored={anchored} at={at:?}: icon disc area {icon_disc_area:.2} vs shape disc area {shape_disc_area:.2} (rel={rel:.4})"
                    );
                }
            }
        }
        let spread = (max_icon_area - min_icon_area) / min_icon_area;
        assert!(
            spread < 0.005,
            "anchored={anchored}: icon disc area jumped across pixel phases: {min_icon_area:.2}..{max_icon_area:.2} ({:.2}%)",
            spread * 100.0
        );
        eprintln!(
            "still-scaled (anchored={anchored}): max axis centre offset = {max_err:.4} px, icon disc area = {min_icon_area:.2}..{max_icon_area:.2} px"
        );
    }
}

#[derive(Clone, Copy)]
struct MovingCase {
    at: [f32; 2],
    zoom: f32,
}

fn moving_view(s: &MovingCase, _: &mut Cx<MovingCase>) -> El {
    let badge = el()
        .key("badge")
        .size(16.0, 16.0)
        .fill(BLUE)
        .offset(s.at[0], s.at[1])
        .offset_motion(Spring::new(0.45, 0.0))
        .child(icon(&RING_ICON, 16.0).color(RED).absolute(0.0, 0.0))
        .child(canvas(disc_shape(4.0, GREEN)).size(16.0, 16.0).absolute(0.0, 0.0));
    col().size(120.0, 120.0).child(
        col()
            .key("stage")
            .size(96.0, 96.0)
            .scale(s.zoom)
            .origin(0.0, 0.0)
            .child(badge.absolute(14.15, 16.35)),
    )
}

/// (2) Moving box (`offset_motion` spring): fill, icon and `canvas` shape keep the same centre
/// within `1/8 px` on every frame of a diagonal glide, at `1×` and `1.6×` zoom.
#[test]
fn concentric_icon_shape_and_fill_glide_together_during_motion() {
    let mut bench = Bench::new(120, 120, 2.0);
    for zoom in [1.0, 1.6] {
        let mut rt = Runtime::new(
            MovingCase { at: [0.0, 0.0], zoom },
            moving_view,
            (120.0, 120.0),
            Env::new(true, 2.0),
        );
        rt.frame(0.0);
        rt.frame(1.0 / 120.0);
        rt.update(|s, _| s.at = [17.35, 13.65]);

        let mut max_icon_shape = 0.0f32;
        let mut max_icon_fill = 0.0f32;
        for f in 0..90 {
            rt.frame(1.0 / 120.0);
            let rgba = bench.draw(&mut rt, [0.0; 4]);
            let (c_icon, _) = channel_moments(&rgba, bench.dw, bench.dh, 0);
            let (c_shape, _) = channel_moments(&rgba, bench.dw, bench.dh, 1);
            let (c_fill, _) = channel_moments(&rgba, bench.dw, bench.dh, 3);
            let d_is = (c_icon[0] - c_shape[0]).abs().max((c_icon[1] - c_shape[1]).abs());
            let d_if = (c_icon[0] - c_fill[0]).abs().max((c_icon[1] - c_fill[1]).abs());
            max_icon_shape = max_icon_shape.max(d_is);
            max_icon_fill = max_icon_fill.max(d_if);
            assert!(
                d_is <= 0.125 && d_if <= 0.125,
                "zoom={zoom} frame={f}: icon={c_icon:?} shape={c_shape:?} fill={c_fill:?} (d_is={d_is:.4}, d_if={d_if:.4})"
            );
        }
        eprintln!(
            "moving (zoom={zoom}): max |icon - shape| = {max_icon_shape:.4} px, max |icon - fill| = {max_icon_fill:.4} px"
        );
    }
}

#[derive(Clone, Copy)]
struct SwapCase {
    swapped: bool,
    disc_r: f32,
}

fn swap_view(s: &SwapCase, _: &mut Cx<SwapCase>) -> El {
    let page = col()
        .key("page")
        .size(120.0, 120.0)
        .scale(1.6)
        .origin(0.0, 0.0)
        .child(el().key("anchor").absolute(12.2, 10.35).size(24.0, 8.0));
    let indicator = if s.swapped {
        el().key("badge")
            .size(16.0, 16.0)
            .child(icon(&RING_ICON, 16.0).color(BLUE).absolute(0.0, 0.0))
            .child(icon(&DISC_ICON, 16.0).color(RED).absolute(0.0, 0.0))
    } else {
        el().key("badge")
            .size(16.0, 16.0)
            .child(icon(&RING_ICON, 16.0).color(BLUE).absolute(0.0, 0.0))
            .child(canvas(disc_shape(s.disc_r, RED)).size(16.0, 16.0).absolute(0.0, 0.0))
    };
    let pop = col()
        .key("pop")
        .padding(6.15)
        .child(indicator)
        .anchored("anchor", Side::Bottom, Align::Start, 4.1);
    col().size(160.0, 160.0).children([page, pop])
}

/// (3) Shape-to-icon swap: unmounting `canvas(Shape)` and mounting the matching `El::icon` in the
/// same `16 × 16 pt` box inside a `1.6×` anchored popover causes no centre jump (`<= 1/8 px`) and
/// no area drop (`< 1%`).
#[test]
fn swapping_canvas_shape_to_icon_keeps_centre_and_area_steady() {
    let mut bench = Bench::new(160, 160, 2.0);
    let mut rt = Runtime::new(
        SwapCase { swapped: false, disc_r: 2.8 },
        swap_view,
        (160.0, 160.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);

    let mut centres = Vec::new();
    let mut areas = Vec::new();
    for f in 0..60 {
        let u = (f as f32 / 30.0).clamp(0.0, 1.0);
        let r = 2.8 + (4.0 - 2.8) * (1.0 - (1.0 - u).powi(3));
        let swapped = f >= 36;
        rt.update(|s, _| {
            s.disc_r = r;
            s.swapped = swapped;
        });
        rt.frame(1.0 / 120.0);
        let rgba = bench.draw(&mut rt, [0.0; 4]);
        let (c, area) = channel_moments(&rgba, bench.dw, bench.dh, 0);
        centres.push(c);
        areas.push(area);
    }

    let before = centres[35];
    let after = centres[36];
    let jump_x = (after[0] - before[0]).abs();
    let jump_y = (after[1] - before[1]).abs();
    let area_rel = (areas[36] - areas[35]).abs() / areas[35];
    eprintln!(
        "swap at 1.6x anchored: centre {before:?} -> {after:?} (jump=({jump_x:.4}, {jump_y:.4}) px), area {:.2} -> {:.2} (rel={area_rel:.4})",
        areas[35], areas[36]
    );
    assert!(
        jump_x <= 0.125 && jump_y <= 0.125,
        "shape->icon swap jumped in centre: {before:?} -> {after:?} ({jump_x:.4}, {jump_y:.4})"
    );
    assert!(
        area_rel < 0.01,
        "shape->icon swap changed drawn area by {:.2}% ({:.2} -> {:.2})",
        area_rel * 100.0,
        areas[35],
        areas[36]
    );
    let c0 = centres[0];
    for (f, c) in centres.iter().enumerate() {
        let dx = (c[0] - c0[0]).abs();
        let dy = (c[1] - c0[1]).abs();
        assert!(dx <= 0.125 && dy <= 0.125, "frame {f}: centre drifted from {c0:?} to {c:?}");
    }
}

/// Large text (`24 pt` at `2×` = `48 ppem > 33⅓ ppem`, where resting CoreGraphics x quantisation
/// has only 1 step/px) glides horizontally in sub-pixel phases with its pixel-snapped card and
/// returns to exact resting instances once still.
#[test]
fn large_heading_glides_horizontally_within_a_phase() {
    #[derive(Clone, Copy)]
    struct HeadingCard {
        x: f32,
    }
    fn view(s: &HeadingCard, _: &mut Cx<HeadingCard>) -> El {
        el().size(280.0, 100.0).child(
            col()
                .absolute(20.0, 20.0)
                .padding(10.0)
                .fill(Color::WHITE.mix(Color::BLACK, 0.8))
                .offset(s.x, 0.0)
                .offset_motion(Spring::new(0.35, 0.0))
                .child(text("Centered").font(TextStyle::new(24.0, 600.0)).color(Color::WHITE)),
        )
    }
    let ink_x = |rt: &Runtime<HeadingCard>| {
        let size = crate::text::ATLAS_SIZE as usize;
        let (mut sum, mut sx) = (0.0f64, 0.0f64);
        for i in rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH) {
            let data = rt.text.atlas.page_data(i.params[2] as usize).unwrap();
            let (u, v) = (i.uv[0] as usize, i.uv[1] as usize);
            for y in 0..i.rect[3] as usize {
                for x in 0..i.rect[2] as usize {
                    let c = data[(v + y) * size + u + x] as f64;
                    sum += c;
                    sx += c * (i.rect[0] as f64 + x as f64 + 0.5);
                }
            }
        }
        (sx / sum) as f32
    };

    let mut rt = Runtime::new(HeadingCard { x: 0.0 }, view, (280.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let glyphs = rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count();
    let masks0 = rt.text.atlas.glyph_entries();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for k in 1..=180 {
        let t = k as f32 / 60.0;
        let d = 2.5 * (0.5 - 0.5 * (std::f32::consts::TAU * t / 3.0).cos());
        rt.update(|s, _| s.x = d);
        rt.frame(1.0 / 60.0);
        let card_x = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).unwrap().rect[0];
        let off = ink_x(&rt) - card_x;
        (lo, hi) = (lo.min(off), hi.max(off));
    }
    let spread = hi - lo;
    let added = rt.text.atlas.glyph_entries() - masks0;
    assert!(spread <= 0.30, "48 ppem heading x-glide spread {spread:.3} px > 0.30 px");
    assert!(added <= glyphs * crate::text::GLIDE_PHASES as usize, "{added} masks for {glyphs} glyphs");
}

#[derive(Clone, Copy)]
struct ProofScene {
    at: [f32; 2],
    zoom: f32,
    disc_r: f32,
    swapped: bool,
}

fn proof_view(s: &ProofScene, _: &mut Cx<ProofScene>) -> El {
    let accent = Color { r: 0.96, g: 0.28, b: 0.24, a: 1.0 };
    let ring_col = Color { r: 0.92, g: 0.94, b: 0.98, a: 1.0 };
    let card_bg = Color { r: 0.08, g: 0.09, b: 0.12, a: 1.0 };
    let card_border = Color { r: 0.20, g: 0.23, b: 0.30, a: 1.0 };

    let indicator = if s.swapped {
        el().key("badge")
            .size(16.0, 16.0)
            .child(icon(&RING_ICON, 16.0).color(ring_col).absolute(0.0, 0.0))
            .child(icon(&DISC_ICON, 16.0).color(accent).absolute(0.0, 0.0))
    } else {
        el().key("badge")
            .size(16.0, 16.0)
            .child(icon(&RING_ICON, 16.0).color(ring_col).absolute(0.0, 0.0))
            .child(canvas(disc_shape(s.disc_r, accent)).size(16.0, 16.0).absolute(0.0, 0.0))
    };

    let page = col()
        .key("page")
        .size(90.0, 90.0)
        .scale(s.zoom)
        .scale_motion(Spring::new(0.42, 0.0))
        .origin(0.0, 0.0)
        .child(
            el().key("anchor")
                .absolute(7.2, 4.35)
                .offset(s.at[0], s.at[1])
                .offset_motion(Spring::new(0.55, 0.0))
                .size(20.0, 4.0),
        );

    let pop = row()
        .key("pop")
        .padding(7.15)
        .gap(6.0)
        .radius(8.0)
        .fill(card_bg)
        .border(1.0, card_border)
        .child(indicator)
        .anchored("anchor", Side::Bottom, Align::Start, 3.1);

    col().size(80.0, 80.0).children([page, pop])
}

/// Optional headless metric + frame exporter driven by `RUNE_ONE_SNAP_DUMP=<dir>` for generating
/// before/after proof clips and p95/max centre-offset tables without touching a real display.
#[test]
fn capture_one_snap_proof() {
    let Some(dir) = std::env::var_os("RUNE_ONE_SNAP_DUMP") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut bench160 = Bench::new(160, 160, 2.0);
    let mut still_scaled_errs = Vec::new();
    let mut still_anchored_errs = Vec::new();
    let mut disc_icon_area = 0.0f32;
    let mut disc_shape_area = 0.0f32;

    for anchored in [false, true] {
        for iy in 0..5 {
            for ix in 0..5 {
                let at = [ix as f32 * 0.21, iy as f32 * 0.21];
                let mut rt = Runtime::new(
                    ScaledCase { at, zoom: 1.6, anchored, only_icon_disc: false },
                    scaled_view,
                    (160.0, 160.0),
                    Env::new(true, 2.0),
                );
                rt.frame(0.0);
                rt.frame(1.0 / 120.0);
                let rgba = bench160.draw(&mut rt, [0.0; 4]);
                let (ci, _) = channel_moments(&rgba, bench160.dw, bench160.dh, 0);
                let (cs, s_area) = channel_moments(&rgba, bench160.dw, bench160.dh, 1);
                let dist = ((ci[0] - cs[0]).powi(2) + (ci[1] - cs[1]).powi(2)).sqrt();
                if anchored {
                    still_anchored_errs.push(dist);
                } else {
                    still_scaled_errs.push(dist);
                }
                if anchored && ix == 1 && iy == 2 {
                    disc_shape_area = s_area;
                    let mut rt_disc = Runtime::new(
                        ScaledCase { at, zoom: 1.6, anchored: true, only_icon_disc: true },
                        scaled_view,
                        (160.0, 160.0),
                        Env::new(true, 2.0),
                    );
                    rt_disc.frame(0.0);
                    rt_disc.frame(1.0 / 120.0);
                    let rgba_d = bench160.draw(&mut rt_disc, [0.0; 4]);
                    disc_icon_area = channel_moments(&rgba_d, bench160.dw, bench160.dh, 0).1;
                }
            }
        }
    }

    let mut bench120 = Bench::new(120, 120, 2.0);
    let mut moving_errs = Vec::new();
    let mut rt_move = Runtime::new(
        MovingCase { at: [0.0, 0.0], zoom: 1.6 },
        moving_view,
        (120.0, 120.0),
        Env::new(true, 2.0),
    );
    rt_move.frame(0.0);
    rt_move.frame(1.0 / 120.0);
    rt_move.update(|s, _| s.at = [17.35, 13.65]);
    for _ in 0..90 {
        rt_move.frame(1.0 / 120.0);
        let rgba = bench120.draw(&mut rt_move, [0.0; 4]);
        let (ci, _) = channel_moments(&rgba, bench120.dw, bench120.dh, 0);
        let (cs, _) = channel_moments(&rgba, bench120.dw, bench120.dh, 1);
        moving_errs.push(((ci[0] - cs[0]).powi(2) + (ci[1] - cs[1]).powi(2)).sqrt());
    }

    let mut rt_swap = Runtime::new(
        SwapCase { swapped: false, disc_r: 2.8 },
        swap_view,
        (160.0, 160.0),
        Env::new(true, 2.0),
    );
    rt_swap.frame(0.0);
    let mut swap_centres = Vec::new();
    let mut swap_areas = Vec::new();
    for f in 0..60 {
        let u = (f as f32 / 30.0).clamp(0.0, 1.0);
        let r = 2.8 + (4.0 - 2.8) * (1.0 - (1.0 - u).powi(3));
        let swapped = f >= 36;
        rt_swap.update(|s, _| {
            s.disc_r = r;
            s.swapped = swapped;
        });
        rt_swap.frame(1.0 / 120.0);
        let rgba = bench160.draw(&mut rt_swap, [0.0; 4]);
        let (c, a) = channel_moments(&rgba, bench160.dw, bench160.dh, 0);
        swap_centres.push(c);
        swap_areas.push(a);
    }
    let swap_step_errs: Vec<f32> = swap_centres
        .windows(2)
        .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
        .collect();
    let swap_jump = swap_step_errs[35];

    // Render 210 tightly-cropped 160×160 px frames of the 3-act visual sequence:
    //   0..70:  zoom 1.0 -> 1.6 while inner canvas disc pulses inside outer icon ring
    //   70..130: held at 1.6x fractional offset; frame 100 swaps canvas disc -> icon disc
    //   130..210: diagonal spring glide across subpixel offsets and settle
    let mut bench_clip = Bench::new(80, 80, 2.0);
    let mut rt_clip = Runtime::new(
        ProofScene { at: [0.0, 0.0], zoom: 1.0, disc_r: 2.6, swapped: false },
        proof_view,
        (80.0, 80.0),
        Env::new(true, 2.0),
    );
    rt_clip.frame(0.0);
    for f in 0..210 {
        rt_clip.update(|s, _| {
            s.zoom = if f >= 4 { 1.6 } else { 1.0 };
            let u = ((f as f32 - 4.0) / 56.0).clamp(0.0, 1.0);
            s.disc_r = 2.6 + (4.0 - 2.6) * (1.0 - (1.0 - u).powi(3));
            s.swapped = (100..150).contains(&f) || f >= 185;
            if f == 130 {
                s.at = [7.35, 6.65];
            }
        });
        rt_clip.frame(1.0 / 120.0);
        let rgba = bench_clip.draw(&mut rt_clip, [0.03, 0.035, 0.045, 1.0]);
        write_png(&dir.join(format!("frame_{f:03}.png")), bench_clip.dw, bench_clip.dh, &rgba).unwrap();
    }

    let report = serde_json::json!({
        "still_scaled": {
            "max_px": percentile(still_scaled_errs.clone(), 1.0),
            "p95_px": percentile(still_scaled_errs.clone(), 0.95),
            "median_px": percentile(still_scaled_errs, 0.50),
        },
        "still_anchored": {
            "max_px": percentile(still_anchored_errs.clone(), 1.0),
            "p95_px": percentile(still_anchored_errs.clone(), 0.95),
            "median_px": percentile(still_anchored_errs, 0.50),
        },
        "moving": {
            "max_px": percentile(moving_errs.clone(), 1.0),
            "p95_px": percentile(moving_errs.clone(), 0.95),
            "median_px": percentile(moving_errs, 0.50),
        },
        "swap": {
            "boundary_jump_px": swap_jump,
            "max_step_px": percentile(swap_step_errs.clone(), 1.0),
            "p95_step_px": percentile(swap_step_errs, 0.95),
            "shape_area_px": swap_areas[35],
            "icon_area_px": swap_areas[36],
            "disc_shape_area_px": disc_shape_area,
            "disc_icon_area_px": disc_icon_area,
        }
    });
    std::fs::write(dir.join("metrics.json"), serde_json::to_string_pretty(&report).unwrap()).unwrap();
}
