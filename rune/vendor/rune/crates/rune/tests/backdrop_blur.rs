//! `El::backdrop_blur`: an element blurs what is drawn behind it in the same window, inside its
//! rounded rect and clip, under its fill.
use rune::render::{Gpu, KIND_BACKDROP_BLUR, KIND_LAYER, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, col, el, row};

const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const BLUE: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };

fn pixels<S: 'static>(gpu: &Gpu, rt: &mut Runtime<S>, size: (u32, u32)) -> Vec<u8> {
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0, 0.0, 0.0, 1.0]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    target.read(&gpu.device)
}

fn at(px: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

/// Largest minus smallest red value along row `y` from `x0` to `x1`.
fn spread(px: &[u8], width: u32, y: u32, x0: u32, x1: u32) -> i32 {
    let reds: Vec<i32> = (x0..x1).map(|x| at(px, width, x, y)[0] as i32).collect();
    reds.iter().max().unwrap() - reds.iter().min().unwrap()
}

#[derive(Clone, Copy)]
struct Bar {
    rect: [f32; 4],
    blur: f32,
    fill: Color,
    opacity: f32,
}

const BAR: Bar = Bar { rect: [40.0, 30.0, 120.0, 60.0], blur: 8.0, fill: Color::TRANSPARENT, opacity: 1.0 };

#[derive(Clone)]
struct Scene {
    bars: Vec<Bar>,
    /// Calls `backdrop_blur` with each bar's sigma; false leaves it unset.
    blur: bool,
}

/// 4 pt black and white stripes under each bar.
fn stripes(s: &Scene, _: &mut Cx<Scene>) -> El {
    let stripes = row().size(200.0, 120.0).absolute(0.0, 0.0)
        .children((0..50).map(|i| el().size(4.0, 120.0).fill(if i % 2 == 0 { Color::BLACK } else { Color::WHITE })));
    el().size(200.0, 120.0).child(stripes).children(s.bars.iter().map(|b| {
        let bar = el().absolute(b.rect[0], b.rect[1]).size(b.rect[2], b.rect[3]).radius(16.0).fill(b.fill).opacity(b.opacity)
            .child(el().size(10.0, 10.0).fill(Color::WHITE));
        if s.blur { bar.backdrop_blur(b.blur) } else { bar }
    }))
}

fn render(scene: Scene, scale: f32) -> (Runtime<Scene>, Vec<u8>) {
    let gpu = Gpu::headless();
    let mut rt = Runtime::new(scene, stripes, (200.0, 120.0), Env::new(false, scale));
    rt.frame(0.0);
    let size = ((200.0 * scale) as u32, (120.0 * scale) as u32);
    let px = pixels(&gpu, &mut rt, size);
    (rt, px)
}

fn records<S>(rt: &Runtime<S>) -> usize {
    rt.instances.iter().filter(|i| i.params[1] == KIND_BACKDROP_BLUR).count()
}

#[test]
fn stripes_blur_inside_the_rounded_rect_and_nowhere_else() {
    let (rt, blurred) = render(Scene { bars: vec![BAR], blur: true }, 1.0);
    let (_, plain) = render(Scene { bars: vec![BAR], blur: false }, 1.0);
    assert_eq!(records(&rt), 1);
    let w = 200;
    assert!(spread(&plain, w, 60, 60, 140) > 200, "stripes under a plain bar");
    assert!(spread(&blurred, w, 60, 60, 140) <= 3, "blurred flat inside: {}", spread(&blurred, w, 60, 60, 140));
    // Half black, half white, averaged in linear light and encoded back to sRGB.
    let mid = at(&blurred, w, 100, 60)[0] as i32;
    assert!((mid - 188).abs() <= 8, "linear mid grey, got {mid}");
    for y in 0..120 {
        for x in 0..200 {
            let inside = (39..=160).contains(&x) && (29..=90).contains(&y);
            if !inside { assert_eq!(at(&blurred, w, x, y), at(&plain, w, x, y), "untouched at {x},{y}"); }
        }
    }
    // Past the rounded corner, inside the rect's bounds.
    assert_eq!(at(&blurred, w, 41, 31), at(&plain, w, 41, 31));
    assert_eq!(at(&blurred, w, 158, 88), at(&plain, w, 158, 88));
}

#[test]
fn the_fill_tints_over_the_blur() {
    let tinted = Bar { fill: RED.alpha(0.5), ..BAR };
    let (_, px) = render(Scene { bars: vec![tinted], blur: true }, 1.0);
    let [r, g, b, _] = at(&px, 200, 100, 60);
    // Linear: grey 0.5 under red at 0.5 gives (0.75, 0.25, 0.25).
    assert!((r as i32 - 225).abs() <= 8 && (g as i32 - 137).abs() <= 8 && (b as i32 - 137).abs() <= 8, "{r} {g} {b}");
}

#[test]
fn without_backdrop_blur_the_frame_is_unchanged() {
    let off = Bar { blur: 0.0, ..BAR };
    let (rt, zero) = render(Scene { bars: vec![off], blur: true }, 1.0);
    let (plain_rt, plain) = render(Scene { bars: vec![off], blur: false }, 1.0);
    assert_eq!(records(&rt), 0);
    assert_eq!(bytemuck::cast_slice::<_, u8>(&rt.instances), bytemuck::cast_slice::<_, u8>(&plain_rt.instances));
    assert_eq!(zero, plain);
}

#[test]
fn two_bars_each_blur_their_own_backdrop() {
    let left = Bar { rect: [10.0, 30.0, 70.0, 60.0], ..BAR };
    let right = Bar { rect: [120.0, 30.0, 70.0, 60.0], fill: RED.alpha(0.5), ..BAR };
    let (rt, px) = render(Scene { bars: vec![left, right], blur: true }, 1.0);
    assert_eq!(records(&rt), 2);
    assert!(spread(&px, 200, 60, 25, 65) <= 3);
    assert!(spread(&px, 200, 60, 135, 175) <= 3);
    assert!(spread(&px, 200, 60, 85, 115) > 200, "stripes between the bars");
    let [r, g, _, _] = at(&px, 200, 155, 60);
    assert!(r > g + 60, "the right bar is tinted over its own blur");
}

#[test]
fn bars_past_the_cap_draw_only_their_fill() {
    let bars = (0..rune::MAX_BACKDROP_BLURS + 1).map(|i| Bar { rect: [i as f32 * 40.0, 30.0, 36.0, 60.0], ..BAR }).collect();
    let (rt, px) = render(Scene { bars, blur: true }, 1.0);
    assert_eq!(records(&rt), rune::MAX_BACKDROP_BLURS);
    // Near the window edge the clamped taps lean towards the edge stripe.
    assert!(spread(&px, 200, 60, 8, 28) <= 12, "the first bar blurs: {}", spread(&px, 200, 60, 8, 28));
    assert!(spread(&px, 200, 60, 168, 188) > 200, "the last bar shows what is under it");
}

#[test]
fn a_faded_bar_blurs_the_window_not_its_own_layer() {
    let faded = Bar { opacity: 0.5, fill: Color::WHITE.alpha(0.2), ..BAR };
    let (rt, px) = render(Scene { bars: vec![faded], blur: true }, 1.0);
    let record = rt.instances.iter().position(|i| i.params[1] == KIND_BACKDROP_BLUR).expect("a record");
    let layer = rt.instances.iter().position(|i| i.params[1] == KIND_LAYER).expect("the faded bar's layer");
    assert!(record < layer, "the record sits ahead of the bar's layer");
    assert_eq!(rt.instances[record].fill[3], 0.5);
    // Half the blur over the stripes: still striped, at half the contrast.
    let s = spread(&px, 200, 60, 60, 140);
    assert!((40..200).contains(&s), "half-faded spread {s}");
}

#[test]
fn hidpi_blurs_in_device_pixels() {
    let (rt, px) = render(Scene { bars: vec![BAR], blur: true }, 2.0);
    let record = rt.instances.iter().find(|i| i.params[1] == KIND_BACKDROP_BLUR).unwrap();
    assert_eq!(record.params[0], 16.0);
    assert_eq!(record.rect, [80.0, 60.0, 240.0, 120.0]);
    assert!(spread(&px, 400, 120, 120, 280) <= 3);
    let (_, plain) = render(Scene { bars: vec![BAR], blur: false }, 2.0);
    assert_eq!(at(&px, 400, 20, 120), at(&plain, 400, 20, 120));
}

#[derive(Default)]
struct Edge;

fn white_corners(_: &Edge, _: &mut Cx<Edge>) -> El {
    el().size(100.0, 100.0).fill(Color::WHITE)
        .child(el().absolute(0.0, 0.0).size(60.0, 40.0).backdrop_blur(10.0))
        .child(el().absolute(55.0, 70.0).size(45.0, 30.0).backdrop_blur(10.0))
}

#[test]
fn window_edges_do_not_darken() {
    let gpu = Gpu::headless();
    let mut rt = Runtime::new(Edge, white_corners, (100.0, 100.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let px = pixels(&gpu, &mut rt, (100, 100));
    for (x, y) in [(0, 0), (1, 1), (30, 0), (0, 20), (59, 39), (99, 99), (56, 71), (99, 70), (70, 99)] {
        assert!(at(&px, 100, x, y)[..3].iter().all(|&c| c >= 254), "white at {x},{y}: {:?}", at(&px, 100, x, y));
    }
}

#[derive(Default)]
struct Scroll;

/// 30 pt red and blue rows scrolling under a frosted bar over the bottom 30 pt.
fn scrolling(_: &Scroll, _: &mut Cx<Scroll>) -> El {
    let rows = (0..12).map(|i| el().size(100.0, 30.0).fill(if i % 2 == 0 { RED } else { BLUE }));
    el().size(100.0, 120.0)
        .child(col().key("list").size(100.0, 120.0).scroll(ScrollSpec { stick_bottom: false }).child(col().w_full().children(rows)))
        .child(el().absolute(0.0, 90.0).size(100.0, 30.0).backdrop_blur(4.0))
}

#[test]
fn scrolled_content_changes_the_blur_and_a_still_frame_stays_idle() {
    let gpu = Gpu::headless();
    let mut rt = Runtime::new(Scroll, scrolling, (100.0, 120.0), Env::new(false, 1.0));
    rt.frame(0.0);
    for _ in 0..3 { rt.frame(1.0 / 60.0); }
    assert!(!rt.frame(1.0 / 60.0), "a still frame with a backdrop blur asks for no redraw");
    let before = at(&pixels(&gpu, &mut rt, (100, 120)), 100, 50, 105);
    assert!(before[2] as i32 > before[0] as i32 + 100, "blue row under the bar: {before:?}");
    rt.wheel(50.0, 60.0, -30.0);
    rt.frame(1.0 / 120.0);
    let px = pixels(&gpu, &mut rt, (100, 120));
    let after = at(&px, 100, 50, 105);
    assert!(after[0] as i32 > after[2] as i32 + 100, "red row under the bar after scrolling: {after:?}");
}

#[derive(Default)]
struct Clipped;

/// A bar half outside a clipping box: the blur stops at the clip.
fn clipped(_: &Clipped, _: &mut Cx<Clipped>) -> El {
    let stripes = row().size(200.0, 120.0).absolute(0.0, 0.0)
        .children((0..50).map(|i| el().size(4.0, 120.0).fill(if i % 2 == 0 { Color::BLACK } else { Color::WHITE })));
    el().size(200.0, 120.0).child(stripes)
        .child(el().absolute(0.0, 0.0).size(200.0, 60.0).clip_rounded(0.0)
            .child(el().absolute(40.0, 30.0).size(120.0, 60.0).backdrop_blur(8.0)))
}

#[test]
fn the_blur_stops_at_an_ancestor_clip() {
    let gpu = Gpu::headless();
    let mut rt = Runtime::new(Clipped, clipped, (200.0, 120.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let px = pixels(&gpu, &mut rt, (200, 120));
    assert!(spread(&px, 200, 50, 60, 140) <= 3, "blurred inside the clip");
    assert!(spread(&px, 200, 70, 60, 140) > 200, "stripes below the clip");
}

#[derive(Default)]
struct Grouped;

/// The bar inside a half-faded group: the group draws as one layer, which the bar blurs.
fn grouped(_: &Grouped, _: &mut Cx<Grouped>) -> El {
    let stripes = row().size(200.0, 120.0)
        .children((0..50).map(|i| el().size(4.0, 120.0).fill(if i % 2 == 0 { Color::BLACK } else { Color::WHITE })));
    el().size(200.0, 120.0).fill(Color::WHITE)
        .child(el().absolute(0.0, 0.0).size(200.0, 120.0).group_opacity(0.5, 0.5)
            .child(stripes)
            .child(el().absolute(40.0, 30.0).size(120.0, 60.0).backdrop_blur(8.0).child(el().size(10.0, 10.0).fill(Color::WHITE))))
}

#[test]
fn inside_a_layer_the_blur_reads_the_layer() {
    let gpu = Gpu::headless();
    let mut rt = Runtime::new(Grouped, grouped, (200.0, 120.0), Env::new(false, 1.0));
    rt.frame(0.0);
    assert_eq!(records(&rt), 1);
    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_LAYER), "the group is a layer");
    let px = pixels(&gpu, &mut rt, (200, 120));
    assert!(spread(&px, 200, 60, 60, 140) <= 3, "blurred inside: {}", spread(&px, 200, 60, 60, 140));
    assert!(spread(&px, 200, 10, 60, 140) > 60, "faded stripes outside");
}
