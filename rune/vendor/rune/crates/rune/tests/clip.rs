use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, col, el, text};

fn pixels<S: 'static>(gpu: &Gpu, rt: &mut Runtime<S>, size: (u32, u32)) -> Vec<u8> {
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    target.read(&gpu.device)
}

fn alpha(px: &[u8], width: u32, x: u32, y: u32) -> u8 {
    px[((y * width + x) * 4 + 3) as usize]
}

#[derive(Default)]
struct S {
    pressed: bool,
    stick: bool,
    soft: bool,
}

/// A 100×100 viewport at y 40 whose 140 pt of content overflows it by 40 pt.
fn view(s: &S, _: &mut Cx<S>) -> El {
    let mut scroll = col().key("scroll").size(100.0, 100.0).scroll(ScrollSpec { stick_bottom: s.stick })
        .child(el().size(100.0, 60.0).fill(Color::WHITE))
        .child(el().key("under").size(100.0, 80.0).fill(Color::WHITE).on_press(|s: &mut S| s.pressed = true));
    if s.soft {
        scroll = scroll.clip_fade(0.0, 0.0, 0.0, 16.0);
    }
    col().size(100.0, 200.0).child(el().h(40.0)).child(scroll)
}

#[test]
fn overflowing_scroll_content_stays_inside_the_box_as_it_scrolls() {
    let gpu = Gpu::headless();
    let size = (100, 200);
    let mut rt = Runtime::new(S::default(), view, (100.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let px = pixels(&gpu, &mut rt, size);
    assert_eq!(alpha(&px, 100, 50, 130), 255, "inside the box");
    assert_eq!(alpha(&px, 100, 50, 160), 0, "overflow below the box isn't drawn");
    assert!(rt.instances.iter().all(|i| i.fade[3] == 0), "clip-radius lane stays reserved");

    // Scrolled to the end, the content moves under a clip that stays put.
    rt.wheel(50.0, 90.0, -40.0);
    rt.frame(1.0 / 120.0);
    let px = pixels(&gpu, &mut rt, size);
    assert_eq!(alpha(&px, 100, 50, 45), 255, "top of the box");
    assert_eq!(alpha(&px, 100, 50, 135), 255, "bottom of the box");
    assert_eq!(alpha(&px, 100, 50, 20), 0, "content scrolled above the box isn't drawn");

    // Opened at the bottom, the top of the content is clipped too.
    let mut rt = Runtime::new(S { stick: true, ..S::default() }, view, (100.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let px = pixels(&gpu, &mut rt, size);
    assert_eq!(alpha(&px, 100, 50, 45), 255);
    assert_eq!(alpha(&px, 100, 50, 20), 0, "stick_bottom content above the box isn't drawn");
}

#[test]
fn a_press_outside_the_box_misses_clipped_content() {
    let mut rt = Runtime::new(S::default(), view, (100.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_down(50.0, 160.0);
    rt.pointer_up(50.0, 160.0);
    rt.frame(1.0 / 120.0);
    assert!(!rt.state.pressed, "the overflowing part of the child isn't hit");
    rt.pointer_down(50.0, 130.0);
    rt.pointer_up(50.0, 130.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.state.pressed, "the visible part still is");
}

#[test]
fn an_explicit_soft_clip_still_wins() {
    let gpu = Gpu::headless();
    let mut rt = Runtime::new(S { soft: true, ..S::default() }, view, (100.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let px = pixels(&gpu, &mut rt, (100, 200));
    let edge = alpha(&px, 100, 50, 136);
    assert!(edge > 0 && edge < 128, "16 pt fade at the bottom edge: {edge}");
    assert_eq!(alpha(&px, 100, 50, 160), 0);
    assert!(rt.instances.iter().all(|i| i.fade[3] == 0));
}

#[test]
fn packed_fades_survive_group_layer_projection_without_double_application() {
    fn layered(state: &(f32, bool), _: &mut Cx<(f32, bool)>) -> El {
        let mut group = col().size(80.0, 64.0).opacity(state.0)
            .child(el().size(80.0, 32.0).fill(Color::WHITE))
            .child(el().size(80.0, 32.0).fill(Color::WHITE));
        if state.1 { group = group.clip_rounded(16.0); }
        el().size(128.0, 128.0).child(col().absolute(16.0, 16.0).size(80.0, 80.0)
            .clip_fade(0.0, 16.0, 0.0, 0.0)
            // Intersect before isolation: the composite must decode a nonzero origin offset.
            .child(col().absolute(0.0, 8.0).size(80.0, 64.0).clip_soft(0.0).child(group)))
    }
    let gpu = Gpu::headless();
    for (scale, rounded) in [(1.0, false), (2.0, false), (1.0, true), (2.0, true)] {
        let size = ((128.0 * scale) as u32, (128.0 * scale) as u32);
        let env = Env::new(true, scale);
        let mut plain = Runtime::new((1.0, rounded), layered, (128.0, 128.0), env);
        let mut isolated = Runtime::new((0.5, rounded), layered, (128.0, 128.0), env);
        plain.frame(0.0); isolated.frame(0.0);
        assert!(isolated.instances.iter().any(|i| i.params[1] == rune::render::KIND_LAYER));
        assert!(isolated.instances.iter().any(|i| i.fade[2] != 0), "exercise inherited origin offsets");
        assert_eq!(isolated.instances.iter().any(|i| i.fade[3] != 0), rounded);
        let a = pixels(&gpu, &mut plain, size);
        let b = pixels(&gpu, &mut isolated, size);
        assert!(a.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));
        for (direct, group) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            let expected = (f32::from(direct[3]) * 0.5).round() as u8;
            assert!(group[3].abs_diff(expected) <= 1, "layer changed fade coverage: {direct:?} / {group:?}");
        }
    }
}

#[test]
fn narrower_nested_edge_and_streaming_mask_survive_outer_fades() {
    use rune::render::unpack_fade;
    let gpu = Gpu::headless();
    for scale in [1.0, 2.0] {
        fn nested(outer: &bool, _: &mut Cx<bool>) -> El {
            col().size(80.0, 200.0).clip_fade(0.0, if *outer { 24.0 } else { 0.0 }, 0.0, 0.0)
                .child(el().h(40.0))
                .child(col().size(80.0, 80.0).clip_fade(0.0, 8.0, 0.0, 0.0)
                    .child(el().size(80.0, 80.0).fill(Color::WHITE)))
        }
        let size = ((80.0 * scale) as u32, (200.0 * scale) as u32);
        let env = Env::new(true, scale);
        let mut plain = Runtime::new(false, nested, (80.0, 200.0), env);
        let mut outer = Runtime::new(true, nested, (80.0, 200.0), env);
        plain.frame(0.0); outer.frame(0.0);
        let a = pixels(&gpu, &mut plain, size);
        let b = pixels(&gpu, &mut outer, size);
        assert_eq!(a, b, "8px inner edge remains inside the finished 24px ancestor ramp");
        assert!(b[(((40.0 * scale) as u32 * size.0 + 20) * 4 + 3) as usize] < 30);

        fn streaming(s: &(bool, f32), _: &mut Cx<(bool, f32)>) -> El {
            col().size(80.0, 200.0).clip_fade(0.0, 0.0, 0.0, if s.0 { 24.0 } else { 0.0 })
                .child(col().w_full().growth(rune::el::GrowthSpec { edge: rune::growth::edge::FAST_FEATHER, ..Default::default() })
                    .child(col().key("row").w_full().streaming(true)
                        .child(el().size(80.0, s.1).fill(Color::WHITE))))
        }
        let render = |faded| {
            let mut rt = Runtime::new((faded, 20.0), streaming, (80.0, 200.0), env);
            rt.frame(0.0);
            rt.update(|s, _| s.1 = 80.0);
            rt.frame(1.0 / 60.0);
            let instance = rt.instances.iter().find(|i| i.fill[3] == 1.0).unwrap();
            let (width, offset) = unpack_fade(instance.fade);
            assert!(width[3] > 0.0 && width[3] < 24.0 * scale);
            assert_eq!(offset[3], 0.0, "row_mask belongs to the growing row's bottom");
            pixels(&gpu, &mut rt, size)
        };
        assert_eq!(render(false), render(true), "streaming row mask survives a transcript's larger bottom fade");
    }
}

fn faded_rows(faded: &bool, _: &mut Cx<bool>) -> El {
    let row = || col().size(120.0, 20.0).clip_fade(0.0, 0.0, 8.0, 0.0)
        .child(text("MMMMMMMMMMMMMM").color(Color::WHITE));
    col().size(120.0, 100.0).clip_fade(0.0, if *faded { 16.0 } else { 0.0 }, 0.0, if *faded { 16.0 } else { 0.0 })
        .child(row()).child(el().h(20.0)).child(row()).child(el().h(20.0)).child(row())
}

#[test]
fn nested_row_text_keeps_full_alpha_in_the_middle_of_a_faded_list() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    for scale in [1.0, 2.0] {
        let (w, h) = ((120.0 * scale) as u32, (100.0 * scale) as u32);
        let target = Offscreen::new(&gpu.device, w, h);
        let mut render = |faded, old_bug| {
            let mut rt = Runtime::new(faded, faded_rows, (120.0, 100.0), Env::new(true, scale));
            rt.frame(0.0);
            if old_bug {
                for instance in &mut rt.instances { instance.fade[2] = 0; }
            }
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut encoder, &target.view, [0.0; 4]);
            target.copy(&mut encoder);
            gpu.queue.submit([encoder.finish()]);
            target.read(&gpu.device)
        };
        let plain = render(false, false);
        let faded = render(true, false);
        let broken = render(true, true);
        let middle = ((40.0 * scale) as usize * w as usize * 4)..((60.0 * scale) as usize * w as usize * 4);
        assert_eq!(plain[middle.clone()], faded[middle.clone()], "the row's own right fade must not restart top/bottom fades");
        let alpha = |pixels: &[u8]| pixels.chunks_exact(4).map(|p| u64::from(p[3])).sum::<u64>();
        assert!(alpha(&faded[middle.clone()]) > 0);
        assert!(alpha(&broken[middle.clone()]) < alpha(&faded[middle]) / 2, "fixture must catch the old washout");
        let top = 0..(20.0 * scale) as usize * w as usize * 4;
        assert!(alpha(&faded[top.clone()]) < alpha(&plain[top]), "the ancestor fade still acts at the list edge");
    }
}

#[test]
fn a_linear_fade_falls_in_proportion_to_distance_from_the_edge() {
    fn faded(linear: &bool, _: &mut Cx<bool>) -> El {
        let content = el().size(100.0, 100.0).fill(Color::WHITE);
        let clip = col().size(100.0, 100.0);
        if *linear { clip.clip_fade_linear(0.0, 0.0, 0.0, 40.0) } else { clip.clip_fade(0.0, 0.0, 0.0, 40.0) }.child(content)
    }
    let gpu = Gpu::headless();
    let mut linear = Runtime::new(true, faded, (100.0, 100.0), Env::new(true, 1.0));
    let mut smooth = Runtime::new(false, faded, (100.0, 100.0), Env::new(true, 1.0));
    linear.frame(0.0); smooth.frame(0.0);
    let bottom = |rt: &Runtime<bool>| rt.instances.iter().map(|i| rune::render::unpack_fade(i.fade).0[3]).find(|&w| w != 0.0);
    assert_eq!((bottom(&linear), bottom(&smooth)), (Some(-40.0), Some(40.0)), "a linear width packs negative");
    let (lin, eased) = (pixels(&gpu, &mut linear, (100, 100)), pixels(&gpu, &mut smooth, (100, 100)));
    // Row `y` has its centre 99.5 - y in from the bottom edge.
    for (y, x) in [(89, 0.2625), (79, 0.5125), (69, 0.7625)] {
        let a = alpha(&lin, 100, 50, y);
        assert!((f32::from(a) - x * 255.0).abs() <= 2.0, "row {y}: {a}, want {}", x * 255.0);
    }
    assert!(alpha(&eased, 100, 50, 89) < alpha(&lin, 100, 50, 89) - 20, "smoothstep stays lower near the edge");
    assert_eq!(alpha(&lin, 100, 50, 50), 255, "whole above the ramp");
}
