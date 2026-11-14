use super::*;
use crate::render::{Gpu, KIND_GLYPH, KIND_RECT, Offscreen, Renderer};
use crate::{Color, Cx, El, Env, Runtime, col, el, row, text};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn snap(r: [f32; 4], motion: [f32; 4], rigid: [f32; 2]) -> [f32; 4] {
    super::pixel_snap::snap(r, motion, rigid, None, |_, c| c).0
}

/// The alpha channel of a headless 2× render of `view`, `w` × `h` points, row-major.
fn coverage<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El, w: u32, h: u32) -> Vec<u8> {
    render(state, view, w, h, 2.0).chunks(4).map(|p| p[3]).collect()
}

/// A headless RGBA render of `view`, `w` × `h` points at `scale`, row-major.
fn render<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El, w: u32, h: u32, scale: f32) -> Vec<u8> {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, FORMAT);
    let (dw, dh) = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
    let target = Offscreen::with_format(&gpu.device, dw, dh, FORMAT);
    let mut rt = Runtime::new(state, view, (w as f32, h as f32), Env::new(true, scale));
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (dw, dh));
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0; 4]);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    target.read(&gpu.device)
}

fn fills<S: 'static>(rt: &Runtime<S>) -> Vec<[f32; 4]> {
    rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).map(|i| i.rect).collect()
}

fn whole(v: f32) -> bool {
    v.fract() == 0.0
}

#[test]
fn shared_edges_round_alike() {
    let a = snap([0.0, 10.3, 50.0, 30.5], [0.0; 4], [0.0; 2]);
    let b = snap([0.0, 30.5, 50.0, 52.7], [0.0; 4], [0.0; 2]);
    assert_eq!(a[3], b[1]);
    assert_eq!(a, [0.0, 10.0, 50.0, 31.0]);
}

#[test]
fn keeps_a_pixel_and_thin_rects() {
    let s = snap([0.0, 4.6, 9.0, 5.4], [0.0; 4], [0.0; 2]);
    assert_eq!((s[1], s[3]), (5.0, 6.0));
    let thin = snap([0.0, 4.6, 9.0, 4.9], [0.0; 4], [0.0; 2]);
    assert_eq!((thin[1], thin[3]), (4.6, 4.9));
}

#[test]
fn motion_carries_the_resting_correction() {
    // Resting at y 10.3..20.3, snapped to 10..20: the -0.3 rides along the flight.
    let moving = snap([0.0, 15.55, 9.0, 25.55], [0.0, 5.25, 0.0, 5.25], [0.0; 2]);
    assert!((moving[1] - 15.25).abs() < 1e-4 && (moving[3] - 25.25).abs() < 1e-4);
}

#[test]
fn rigid_translation_snaps_as_one() {
    // Two rows 2.2 px apart, scrolled: the gap is the same whole number at every offset.
    for k in 0..64 {
        let d = -(k as f32) * 0.37;
        let a = snap([0.0, 10.0 + d, 9.0, 20.0 + d], [0.0; 4], [0.0, d]);
        let b = snap([0.0, 22.2 + d, 9.0, 32.2 + d], [0.0; 4], [0.0, d]);
        assert_eq!(b[1] - a[3], 2.0);
        assert!(whole(a[1]));
    }
}

fn gap_column(on: &bool, _: &mut Cx<bool>) -> El {
    let rows = (0..6).map(|_| el().size(20.0, 7.3).fill(Color::WHITE));
    col().gap(1.1).padding(0.3).pixel_snap(*on).children(rows)
}

#[test]
fn rows_with_a_fractional_gap_have_hard_edges() {
    for on in [true, false] {
        let a = coverage(on, gap_column, 24, 52);
        let column: Vec<u8> = (0..104).map(|y| a[y * 48 + 10]).collect();
        let soft = column.iter().filter(|&&v| v > 1 && v < 254).count();
        if on {
            assert_eq!(soft, 0, "{column:?}");
            // Six rows, each separated by a visible gap.
            let starts = column.windows(2).filter(|w| w[0] < 128 && w[1] >= 128).count() + (column[0] >= 128) as usize;
            assert_eq!(starts, 6, "{column:?}");
        } else {
            assert!(soft > 0, "the opt-out still paints at fractional edges");
        }
    }
}

#[test]
fn layout_and_drawn_rects_stay_fractional() {
    let mut rt = Runtime::new(true, gap_column, (24.0, 52.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let rows: Vec<_> = rt.elements().into_iter().filter(|e| (e.rect[3] - 7.3).abs() < 0.02).collect();
    assert_eq!(rows.len(), 6);
    for (k, e) in rows.iter().enumerate() {
        let y = 0.3 + k as f32 * 8.4;
        assert!((e.rect[1] - y).abs() < 1.0 / 64.0, "row {k}: {:?} vs {y}", e.rect);
    }
    // What paints is on whole pixels.
    for r in fills(&rt) {
        assert!(r.iter().all(|&v| whole(v)), "{r:?}");
    }
}

#[test]
fn adjacent_boxes_meet_without_a_seam() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        row().padding(0.35).children((0..5).map(|_| el().size(7.15, 10.0).fill(Color::WHITE)))
    }
    let a = coverage((), view, 40, 12);
    let line = &a[12 * 80..13 * 80];
    let lit: Vec<usize> = (0..80).filter(|&x| line[x] > 0).collect();
    let (first, last) = (lit[0], *lit.last().unwrap());
    assert!(line[first..=last].iter().all(|&v| v == 255), "{line:?}");
    assert!(line.iter().all(|&v| v == 0 || v == 255), "{line:?}");
}

#[test]
fn hairline_border_is_one_crisp_pixel() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(32.0, 32.0).child(el().absolute(3.3, 4.15).size(20.3, 12.45).border(0.5, Color::WHITE))
    }
    let a = coverage((), view, 32, 32);
    let column: Vec<u8> = (0..64).map(|y| a[y * 64 + 20]).collect();
    assert!(column.iter().all(|&v| v <= 1 || v >= 254), "{column:?}");
    assert_eq!(column.iter().filter(|&&v| v >= 254).count(), 2, "{column:?}");
}

#[test]
fn an_opted_out_element_paints_its_exact_offset() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        el().size(40.0, 40.0).child(el().size(10.0, 10.0).offset(3.3, 2.15).pixel_snap(*on).fill(Color::WHITE))
    }
    for on in [true, false] {
        let mut rt = Runtime::new(on, view, (40.0, 40.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let r = fills(&rt)[0];
        if on {
            assert_eq!([r[0], r[1]], [7.0, 4.0], "{r:?}");
        } else {
            assert!((r[0] - 6.6).abs() < 1.0 / 32.0 && (r[1] - 4.3).abs() < 1.0 / 32.0, "{r:?}");
        }
    }
}

#[test]
fn a_moving_element_glides_and_lands_snapped() {
    fn view(down: &bool, _: &mut Cx<bool>) -> El {
        let y = if *down { 10.3 } else { 0.0 };
        el().size(40.0, 40.0)
            .child(el().size(10.0, 10.0).absolute(1.15, 1.15).offset(0.0, y).offset_motion(Spring::new(0.4, 0.0)).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(false, view, (40.0, 40.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(fills(&rt)[0][1], 2.0);
    rt.state = true;
    rt.invalidate();
    let mut ys = Vec::new();
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        ys.push(fills(&rt)[0][1]);
    }
    let end = *ys.last().unwrap();
    assert_eq!(end, 23.0, "lands on the snapped resting pixel");
    // It starts where it was: the first frame moves it about as far as the spring does (0.17 px).
    assert!(ys[0] - 2.0 < 0.25, "{ys:?}");
    assert!(ys.iter().any(|&y| !whole(y)), "glides between pixels: {ys:?}");
    // A spring that eases out never jumps: each step is no bigger than the one before.
    let steps: Vec<f32> = ys.windows(2).map(|w| w[1] - w[0]).collect();
    let peak = steps.iter().cloned().fold(0.0, f32::max);
    let after = steps.iter().position(|&d| d == peak).unwrap();
    for w in steps[after..].windows(2) {
        assert!(w[1] <= w[0] + 1.0 / 32.0 && w[1] >= -1.0 / 32.0, "{steps:?}");
    }
}

#[test]
fn text_keeps_its_place_against_its_fill() {
    fn view(y: &f32, _: &mut Cx<f32>) -> El {
        el().size(80.0, 40.0).child(text("Hg").padding(2.3).absolute(4.0, *y).fill(Color::WHITE.mix(Color::BLACK, 0.8)))
    }
    let offset = |y: f32| {
        let mut rt = Runtime::new(y, view, (80.0, 40.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let fill = fills(&rt)[0];
        let glyph = rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).map(|i| i.rect[1]).fold(f32::MAX, f32::min);
        assert!(glyph < f32::MAX, "draws glyphs");
        (glyph - fill[1], fill[1])
    };
    // At a whole-pixel position snapping changes nothing; elsewhere the text keeps that offset.
    let (want, _) = offset(10.0);
    for k in 1..10 {
        let (got, top) = offset(10.0 + k as f32 * 0.05);
        assert!(whole(top), "{top}");
        assert!((got - want).abs() < 1e-3, "y {}: {got} vs {want}", 10.0 + k as f32 * 0.05);
    }
}

#[test]
fn scrolled_rows_stay_crisp_and_keep_their_spacing() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(40.0, 60.0).gap(1.1).scroll(crate::el::ScrollSpec { stick_bottom: false })
            .children((0..20).map(|i| el().key(format!("r{i}")).size(20.0, 7.3).fill(Color::WHITE)))
    }
    let mut rt = Runtime::new((), view, (40.0, 60.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let pitch = |rt: &Runtime<()>| fills(rt).windows(2).map(|w| w[1][1] - w[0][1]).collect::<Vec<_>>();
    let want = pitch(&rt);
    let mut fractional = false;
    for _ in 0..12 {
        rt.wheel(20.0, 30.0, -0.37);
        rt.frame(1.0 / 120.0);
        let r0 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r0")).unwrap().rect;
        fractional |= !whole(r0[1] * 2.0);
        assert!(fills(&rt).iter().all(|r| r.iter().all(|&v| whole(v))), "{:?}", fills(&rt));
        assert_eq!(pitch(&rt), want);
    }
    assert!(fractional, "scrolled by fractional pixels");
}

#[test]
fn a_scale_settling_at_one_lands_snapped_without_a_jump() {
    fn view(big: &bool, _: &mut Cx<bool>) -> El {
        let k = if *big { 1.0 } else { 0.8 };
        el().size(60.0, 60.0)
            .child(el().size(20.3, 20.3).absolute(10.15, 10.15).scale(k).scale_motion(Spring::new(0.4, 0.0)).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(false, view, (60.0, 60.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state = true;
    rt.invalidate();
    let mut frames = Vec::new();
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        frames.push(fills(&rt)[0]);
    }
    let end = *frames.last().unwrap();
    assert!(end.iter().all(|&v| whole(v)), "lands on whole pixels: {end:?}");
    // Each edge eases in: once the scale is nearly done, no frame moves an edge further than the
    // frame before it did.
    for k in 0..2 {
        let steps: Vec<f32> = frames.windows(2).map(|w| (w[1][k] - w[0][k]).abs()).collect();
        let tail = &steps[steps.len() / 3..];
        for w in tail.windows(2) {
            assert!(w[1] <= w[0] + 1.0 / 32.0, "edge {k}: {steps:?}");
        }
    }
}

#[test]
fn a_trailing_child_holds_its_place_against_a_widening_parent() {
    fn view(wide: &bool, _: &mut Cx<bool>) -> El {
        let w = if *wide { 50.7 } else { 30.3 };
        el().size(80.0, 20.0).child(
            el().size(w, 12.0).absolute(3.15, 2.0).fill(Color::WHITE.mix(Color::BLACK, 0.5))
                .child(el().size(6.0, 6.0).absolute(w - 9.15, 3.0).ride_trailing().fill(Color::WHITE)),
        )
    }
    let mut rt = Runtime::new(false, view, (80.0, 20.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let gap = |f: &[[f32; 4]]| (f[0][0] + f[0][2]) - (f[1][0] + f[1][2]);
    let before = gap(&fills(&rt));
    rt.state = true;
    rt.invalidate();
    let (mut moved, mut last) = (false, before);
    let mut gaps = Vec::new();
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        let f = fills(&rt);
        moved |= f[0][2] != 61.0 && f[0][2] != 101.0;
        gaps.push(gap(&f));
        // The child glides with the parent's edge: their gap eases between its two resting
        // roundings, never stepping a pixel at once.
        assert!((gap(&f) - last).abs() < 0.25, "{gaps:?}");
        last = gap(&f);
    }
    assert!(moved, "the parent's width animates");
    let f = fills(&rt);
    assert!(f.iter().flatten().all(|&v| whole(v)), "{f:?}");
}

/// A zoomed canvas (`El::scale`) with boxes at fractional places, snapping or not.
fn canvas(st: &(f32, bool), _: &mut Cx<(f32, bool)>) -> El {
    let boxes = (0..4).map(|i| el().size(17.3, 11.1).absolute(3.3 + i as f32 * 21.7, 5.45 + i as f32 * 9.2).fill(Color::WHITE));
    el().size(160.0, 120.0).child(
        el().size(100.3, 70.15).absolute(20.15, 15.3).scale(st.0).pixel_snap(st.1)
            .fill(Color::WHITE.mix(Color::BLACK, 0.5)).children(boxes),
    )
}

fn edges(f: &[[f32; 4]]) -> Vec<f32> {
    f.iter().flat_map(|r| [r[0], r[1], r[0] + r[2], r[1] + r[3]]).collect()
}

#[test]
fn a_still_zoomed_canvas_draws_on_whole_pixels() {
    for on in [true, false] {
        let mut rt = Runtime::new((1.37, on), canvas, (160.0, 120.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let e = edges(&fills(&rt));
        assert_eq!(e.len(), 20);
        if on {
            assert!(e.iter().all(|&v| whole(v)), "{e:?}");
        } else {
            assert!(e.iter().any(|&v| !whole(v)), "the opt-out draws at the exact scaled edges: {e:?}");
        }
    }
}

#[test]
fn a_zoom_draws_exact_while_it_moves_and_eases_onto_the_grid() {
    let mut snapped = Runtime::new((1.2, true), canvas, (160.0, 120.0), Env::new(true, 2.0));
    let mut exact = Runtime::new((1.2, false), canvas, (160.0, 120.0), Env::new(true, 2.0));
    snapped.frame(0.0);
    exact.frame(0.0);
    // A pinch: the zoom lands at once on every frame for 30 frames, then holds.
    let mut frames = Vec::new();
    for k in 1..=80 {
        if k <= 30 {
            let z = 1.2 + 0.17 * k as f32 / 30.0;
            for rt in [&mut snapped, &mut exact] {
                rt.update(|st, tx| {
                    st.0 = z;
                    tx.instant();
                });
            }
        }
        let asks = snapped.frame(1.0 / 120.0);
        exact.frame(1.0 / 120.0);
        let (a, b) = (edges(&fills(&snapped)), edges(&fills(&exact)));
        // Once each box has moved a pixel, the zoom draws at its exact sub-pixel edges.
        if (8..=30).contains(&k) {
            assert!(a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1.0 / 64.0), "frame {k}: {a:?} vs {b:?}");
        }
        assert!(a.iter().zip(&b).all(|(a, b)| (a - b).abs() <= 0.5 + 1.0 / 64.0), "frame {k}: {a:?} vs {b:?}");
        frames.push((a, asks));
    }
    // Held still, it eases onto whole pixels within a few frames and then stops asking for them.
    let (end, asks) = frames.last().unwrap();
    assert!(end.iter().all(|&v| whole(v)), "{end:?}");
    assert!(!asks, "an idle canvas draws nothing");
    let landed = frames.iter().position(|(e, _)| e.iter().all(|&v| whole(v))).unwrap();
    assert!(landed <= 30 + 16, "lands {} frames after the zoom stops", landed + 1 - 30);
    assert!(frames[30..landed].iter().all(|(_, asks)| *asks), "asks for each frame of the ease");
    // No pop: once the zoom stops, no edge moves more than the ease's steepest step.
    for w in frames[29..].windows(2) {
        let step = w[0].0.iter().zip(&w[1].0).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(step < 0.07, "{step}: {:?} -> {:?}", w[0].0, w[1].0);
    }
}

#[test]
fn a_scale_spring_glides_exact_and_lands_snapped() {
    fn view(big: &bool, _: &mut Cx<bool>) -> El {
        let k = if *big { 1.37 } else { 1.0 };
        el().size(160.0, 120.0).child(
            el().size(60.3, 40.15).absolute(40.15, 35.3).scale(k).scale_motion(Spring::new(0.4, 0.0)).fill(Color::WHITE)
                .child(el().size(17.3, 11.1).absolute(13.3, 9.45).fill(Color::BLACK)),
        )
    }
    let mut rt = Runtime::new(false, view, (160.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state = true;
    rt.invalidate();
    let mut frames = Vec::new();
    for _ in 0..160 {
        rt.frame(1.0 / 120.0);
        frames.push(edges(&fills(&rt)));
    }
    let end = frames.last().unwrap();
    assert!(end.iter().all(|&v| whole(v)), "lands on whole pixels: {end:?}");
    let fractional = frames.iter().filter(|e| e.iter().any(|&v| !whole(v))).count();
    assert!(fractional > 20, "glides between pixels while it scales ({fractional} frames)");
    // Each edge eases in: once the spring is nearly done, no frame moves an edge further than
    // the frame before it did, beyond the ease onto the grid.
    for k in 0..end.len() {
        let steps: Vec<f32> = frames.windows(2).map(|w| (w[1][k] - w[0][k]).abs()).collect();
        for w in steps[steps.len() / 3..].windows(2) {
            assert!(w[1] <= w[0] + 0.07, "edge {k}: {steps:?}");
        }
    }
}

/// A zoomed canvas (`zoom`, scaled about `origin`) with boxes across it, some straddling the
/// point it scales about, and an overlay layer on top scaled about the window's centre and
/// shifted to line up, outlining each box: art and outlines drawn by two layers anchored apart.
#[derive(Clone, Copy)]
struct Anchored {
    zoom: f32,
    snap: bool,
    origin: [f32; 2],
    overlay: bool,
}

const CANVAS: [f32; 4] = [20.15, 15.3, 100.3, 70.15];

fn anchored(st: &Anchored, _: &mut Cx<Anchored>) -> El {
    let spots = [(41.3, 29.45), (45.85, 31.2), (3.3, 5.45), (70.1, 48.35), (12.6, 50.9), (0.0, 0.0)];
    let boxes = |dx: f32, dy: f32| spots.map(|(x, y)| el().size(17.3, 11.1).absolute(x + dx, y + dy).fill(Color::WHITE));
    let [cx, cy, cw, ch] = CANVAS;
    let mut root = el().size(160.0, 120.0).child(
        el().size(cw, ch).absolute(cx, cy).scale(st.zoom).origin(st.origin[0], st.origin[1]).pixel_snap(st.snap).children(boxes(0.0, 0.0)),
    );
    if st.overlay {
        // Scaled about (80, 60) and shifted so it maps a point the way the canvas does.
        let pivot = [cx + cw * st.origin[0], cy + ch * st.origin[1]];
        let off = [(pivot[0] - 80.0) * (1.0 - st.zoom), (pivot[1] - 60.0) * (1.0 - st.zoom)];
        root = root.child(el().size(160.0, 120.0).absolute(0.0, 0.0).scale(st.zoom).offset(off[0], off[1]).pixel_snap(st.snap).children(boxes(cx, cy)));
    }
    root
}

fn pinch(rts: &mut [&mut Runtime<Anchored>], k: usize) {
    if k <= 30 {
        let z = 1.2 + 0.17 * k as f32 / 30.0;
        for rt in rts.iter_mut() {
            rt.update(|st, tx| {
                st.zoom = z;
                tx.instant();
            });
        }
    }
    for rt in rts.iter_mut() {
        rt.frame(1.0 / 120.0);
    }
}

#[test]
fn outlines_on_a_layer_anchored_elsewhere_stay_on_the_art() {
    for origin in [[0.5, 0.5], [0.0, 0.0], [0.41, 0.62], [3.0, -2.0]] {
        let mut rt = Runtime::new(Anchored { zoom: 1.2, snap: true, origin, overlay: true }, anchored, (160.0, 120.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for k in 1..=80 {
            pinch(&mut [&mut rt], k);
            let e = edges(&fills(&rt));
            let (art, outline) = e.split_at(e.len() / 2);
            // Each layer lands a box's exact edges on the 1/64 px quantum through its own sums,
            // so they can sit a quantum apart, and so can how far each says the box has moved.
            let worst = art.iter().zip(outline).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(worst <= 1.0 / 32.0, "origin {origin:?}, frame {k}: off by {worst}: {art:?} vs {outline:?}");
            if k == 80 {
                assert!(e.iter().all(|&v| whole(v)), "origin {origin:?}: {e:?}");
            }
        }
    }
}

/// rune-kit's Small switch: a 16 pt knob inset 2 pt in a 20 pt track, `y` points down, the knob
/// `offset` points below its rest.
fn knob(&(y, offset): &(f32, f32), _: &mut Cx<(f32, f32)>) -> El {
    let knob = el().size(26.0, 16.0).absolute(2.0, 2.0).offset(0.0, offset).offset_motion(Spring::new(0.4, 0.0));
    el().size(80.0, 60.0).child(el().size(44.0, 20.0).absolute(3.0, y).fill(Color::BLACK).child(knob.fill(Color::WHITE)))
}

/// The gaps above and below the knob in its track (device px).
fn knob_gaps(f: &[[f32; 4]]) -> [f32; 2] {
    let (track, knob) = (f[0], f[1]);
    [knob[1] - track[1], (track[1] + track[3]) - (knob[1] + knob[3])]
}

#[test]
fn a_centred_knob_keeps_even_gaps_at_fractional_scales() {
    // 2 pt is 2.5 px at 1.25×: each edge rounding on its own put the knob 2 px from one edge
    // and 3 px from the other.
    for scale in [1.25, 1.5, 1.75, 2.0] {
        let mut heights = std::collections::HashSet::new();
        for k in 0..64 {
            let y = 10.0 + k as f32 / 16.0;
            let mut rt = Runtime::new((y, 0.0), knob, (80.0, 60.0), Env::new(true, scale));
            rt.frame(0.0);
            let f = fills(&rt);
            let [above, below] = knob_gaps(&f);
            assert_eq!(above, below, "{scale}× at y {y}: {f:?}");
            assert!(f.iter().flatten().all(|&v| whole(v)), "{scale}× at y {y}: {f:?}");
            heights.insert(f[1][3] as u32);
        }
        assert_eq!(heights.len(), 1, "{scale}×: the knob draws one height wherever it sits: {heights:?}");
    }
}

#[test]
fn a_centred_knob_draws_even_gaps_in_device_pixels_at_1_25x() {
    // Where an app's Small switch sat: the track 559.25 pt down is 699.0625 px, as 11.25 pt is.
    let rgba = render((11.25, 0.0), knob, 80, 60, 1.25);
    let column: Vec<[u8; 4]> = (0..75).map(|y| {
        let i = (y * 100 + 22) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }).collect();
    let track: Vec<usize> = (0..75).filter(|&y| column[y][3] == 255).collect();
    let knob: Vec<usize> = (0..75).filter(|&y| column[y][0] == 255).collect();
    assert!(column.iter().all(|p| p[3] == 0 || p[3] == 255), "hard edges: {column:?}");
    let (above, below) = (knob[0] - track[0], track.last().unwrap() - knob.last().unwrap());
    assert_eq!((track.len(), above, below), (25, 3, 3), "{column:?}");
}

#[test]
fn an_unpainted_parent_leaves_a_centred_child_its_size() {
    // Nothing shows the gaps to a parent that paints nothing, so the child keeps its own
    // rounding: 20 pt is 25 px at 1.25× wherever it sits.
    fn view(y: &f32, _: &mut Cx<f32>) -> El {
        el().size(80.0, 60.0).child(el().size(44.0, 22.0).absolute(3.0, *y).child(el().size(44.0, 20.0).absolute(0.0, 1.0).fill(Color::WHITE)))
    }
    for k in 0..16 {
        let y = 10.0 + k as f32 / 16.0;
        let mut rt = Runtime::new(y, view, (80.0, 60.0), Env::new(true, 1.25));
        rt.frame(0.0);
        assert_eq!(fills(&rt)[0][3], 25.0, "y {y}");
    }
}

#[test]
fn a_knob_glides_into_and_out_of_its_centred_rest() {
    let mut rt = Runtime::new((11.25, 0.0), knob, (80.0, 60.0), Env::new(true, 1.25));
    rt.frame(0.0);
    for offset in [3.1, 0.0] {
        let before = fills(&rt)[1];
        rt.state.1 = offset;
        rt.invalidate();
        let mut bottoms = vec![before[1] + before[3]];
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
            let k = fills(&rt)[1];
            bottoms.push(k[1] + k[3]);
        }
        // The spring's own first step is about 0.2 px: no edge steps a pixel at either end.
        let steps: Vec<f32> = bottoms.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        assert!(steps[0] < 0.3 && steps[steps.len() - 20..].iter().all(|&d| d < 0.1), "{bottoms:?}");
        let f = fills(&rt);
        assert!(f.iter().flatten().all(|&v| whole(v)), "lands snapped: {f:?}");
        if offset == 0.0 {
            let [above, below] = knob_gaps(&f);
            assert_eq!(above, below, "{f:?}");
        }
    }
}

#[test]
fn a_knob_keeps_even_gaps_while_its_track_slides() {
    fn view(slide: &f32, _: &mut Cx<f32>) -> El {
        let knob = el().size(26.0, 16.0).absolute(2.0, 2.0).fill(Color::WHITE);
        let track = el().size(44.0, 20.0).absolute(3.0, 11.25).offset(0.0, *slide).offset_motion(Spring::new(0.4, 0.0));
        el().size(80.0, 60.0).child(track.fill(Color::BLACK).child(knob))
    }
    let mut rt = Runtime::new(0.0, view, (80.0, 60.0), Env::new(true, 1.25));
    rt.frame(0.0);
    for slide in [7.3, 0.0] {
        rt.state = slide;
        rt.invalidate();
        let mut tops = vec![fills(&rt)[1][1]];
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
            let f = fills(&rt);
            let [above, below] = knob_gaps(&f);
            assert!((above - below).abs() < 1e-3, "{f:?}");
            tops.push(f[1][1]);
        }
        let steps: Vec<f32> = tops.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        assert!(steps[0] < 0.3 && steps[steps.len() - 20..].iter().all(|&d| d < 0.1), "{tops:?}");
        assert!(fills(&rt).iter().flatten().all(|&v| whole(v)));
    }
}

/// A 16 pt knob centred in a track `h` points tall, growing around the knob's centre.
fn growing(h: &f32, _: &mut Cx<f32>) -> El {
    let knob = el().size(26.0, 16.0).absolute(2.0, (h - 16.0) / 2.0).fill(Color::WHITE);
    el().size(80.0, 60.0).child(el().size(44.0, *h).absolute(3.0, 11.25 - (h - 20.0) / 2.0).fill(Color::BLACK).child(knob))
}

#[test]
fn a_still_knob_eases_with_its_track_growing_around_it() {
    // The knob doesn't move, but its centred rounding changes: it goes with the track's glide
    // rather than jump at the start, and starts over from where it is when the track retargets.
    for retarget in [None, Some(10)] {
        let mut rt = Runtime::new(20.0, growing, (80.0, 60.0), Env::new(true, 1.25));
        rt.frame(0.0);
        let start = fills(&rt)[1];
        rt.state = 20.8;
        rt.invalidate();
        let mut knobs = vec![start];
        for i in 0..120 {
            if retarget == Some(i) {
                rt.state = 22.4;
                rt.invalidate();
            }
            rt.frame(1.0 / 120.0);
            knobs.push(fills(&rt)[1]);
        }
        settles_without_a_jump(&rt, &knobs, start);
    }
}

fn settles_without_a_jump(rt: &Runtime<f32>, knobs: &[[f32; 4]], start: [f32; 4]) {
    let edges = |k: &[f32; 4]| [k[1], k[1] + k[3]];
    for w in knobs.windows(2) {
        let (a, b) = (edges(&w[0]), edges(&w[1]));
        assert!((0..2).all(|i| (a[i] - b[i]).abs() < 0.3), "{knobs:?}");
    }
    let f = fills(rt);
    let [above, below] = knob_gaps(&f);
    assert!(above == below && f.iter().flatten().all(|&v| whole(v)), "{f:?}");
    assert_ne!(edges(&start), edges(knobs.last().unwrap()), "the knob's rounding changes");
}
