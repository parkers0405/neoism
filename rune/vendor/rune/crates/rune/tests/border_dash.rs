use rune::render::{Gpu, KIND_DASHED, KIND_RECT, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, el};

const TOP: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const BOTTOM: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
const RIM: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
const DARK: Color = Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 };

/// A 60×14 capsule at (4, 4) with a 1 pt border; the window is 68×22 points.
#[derive(Clone, Copy)]
enum Mode {
    Solid,
    Dashed,
    FilledDashed,
    GradientDashed,
}

fn view(mode: &Mode, _: &mut Cx<Mode>) -> El {
    let capsule = el().absolute(4.0, 4.0).size(60.0, 14.0).radius(7.0);
    let capsule = match mode {
        Mode::Solid => capsule.border(1.0, RIM),
        Mode::Dashed => capsule.border(1.0, RIM).border_dash(8.0, 4.0),
        Mode::FilledDashed => capsule.fill(DARK).border_dash(8.0, 4.0).border_dash_phase(2.0).border(1.0, RIM),
        Mode::GradientDashed => capsule.border_gradient(1.0, TOP, BOTTOM).border_dash(8.0, 4.0),
    };
    el().size(68.0, 22.0).child(capsule)
}

fn frame(mode: Mode) -> Runtime<Mode> {
    let mut rt = Runtime::new(mode, view, (68.0, 22.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn pixels(gpu: &Gpu, rt: &mut Runtime<Mode>) -> Vec<u8> {
    let size = (136, 44);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    target.read(&gpu.device)
}

#[test]
fn a_dashed_border_is_its_own_ring_over_the_fill() {
    let rt = frame(Mode::FilledDashed);
    let [fill, ring] = rt.instances.as_slice() else { panic!("fill and ring: {:?}", rt.instances) };
    assert_eq!((fill.params[0], fill.params[1], fill.fill), (0.0, KIND_RECT, DARK.premul(1.0)), "the fill has no ring");
    assert_eq!((ring.params[0], ring.params[1]), (2.0, KIND_DASHED), "1 pt at 2x");
    assert_eq!((ring.rect, ring.radii), ([8.0, 8.0, 120.0, 28.0], [14.0; 4]));
    assert_eq!((ring.fill, ring.border), (RIM.premul(1.0), RIM.premul(1.0)));
    assert_eq!(ring.uv, [16.0, 8.0, 4.0, 0.0], "dash, gap, phase in device pixels");

    let rt = frame(Mode::GradientDashed);
    let [ring] = rt.instances.as_slice() else { panic!("only the ring: {:?}", rt.instances) };
    assert_eq!((ring.params[1], ring.fill, ring.border), (KIND_DASHED, TOP.premul(1.0), BOTTOM.premul(1.0)));
}

#[test]
fn dashes_antialias_like_the_solid_border_and_show_the_fill_between() {
    let gpu = Gpu::headless();
    let px = |out: &[u8], x: usize, y: usize| -> [u8; 4] { out[(y * 136 + x) * 4..][..4].try_into().unwrap() };
    let solid = pixels(&gpu, &mut frame(Mode::Solid));
    let dashed = pixels(&gpu, &mut frame(Mode::Dashed));
    // The centre line runs 265.7 px, so 11 periods of 24.15 px: the first dash covers the top
    // edge from x = 22 to 38.1 and the first gap from 38.1 to 46.2.
    for y in 6..12 {
        assert_eq!(px(&dashed, 30, y), px(&solid, 30, y), "row {y} across a dash");
        assert_eq!(px(&dashed, 42, y)[3], 0, "row {y} across a gap");
    }
    assert!(px(&solid, 30, 8)[3] > 250 && px(&solid, 30, 9)[3] > 250);
    // Round the top-right corner (centre line radius 13 about (114, 22)) the pattern goes on:
    // the gap that started on the top edge, then a dash.
    let corner: Vec<u8> = (0..=16).map(|n| {
        let a = n as f32 / 16.0 * std::f32::consts::FRAC_PI_2;
        px(&dashed, (114.0 + 13.0 * a.sin()) as usize, (22.0 - 13.0 * a.cos()) as usize)[3]
    }).collect();
    assert!(corner[..2].iter().all(|a| *a == 0) && corner[6..14].iter().all(|a| *a > 150), "{corner:?}");

    let filled = pixels(&gpu, &mut frame(Mode::FilledDashed));
    let dark = px(&filled, 68, 20);
    assert!(dark[3] > 250 && dark[0] < 100, "inside is the fill: {dark:?}");
    // Phase 2 pt (4 px) moves the gap 4 px back along the top edge.
    assert_eq!(px(&filled, 38, 8), dark, "the fill shows through a gap");
    assert!(px(&filled, 26, 8)[0] > 250, "a dash draws over the fill");
}
