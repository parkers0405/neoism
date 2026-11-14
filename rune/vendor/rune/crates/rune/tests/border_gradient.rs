use rune::render::{Gpu, Instance, KIND_GRADIENT, KIND_RECT, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, el};

const TOP: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const BOTTOM: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
const RIM: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.15 };
const DARK: Color = Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 };

/// A 60×14 capsule at (4, 4); the window is 68×22 points.
#[derive(Clone, Copy)]
enum Mode {
    Gradient,
    BorderThenGradient,
    GradientThenBorder,
    RimGradient,
    RimBorder,
    SolidBottom,
}

fn view(mode: &Mode, _: &mut Cx<Mode>) -> El {
    let capsule = el().absolute(4.0, 4.0).size(60.0, 14.0).radius(7.0);
    let capsule = match mode {
        Mode::Gradient => capsule.border_gradient(0.5, TOP, BOTTOM),
        Mode::BorderThenGradient => capsule.border(1.0, RIM).border_gradient(0.5, TOP, BOTTOM),
        Mode::GradientThenBorder => capsule.border_gradient(0.5, TOP, BOTTOM).border(1.0, RIM),
        Mode::RimGradient => capsule.fill(DARK).border_gradient(0.5, RIM, RIM),
        Mode::RimBorder => capsule.fill(DARK).border(0.5, RIM),
        Mode::SolidBottom => capsule.border(0.5, BOTTOM),
    };
    el().size(68.0, 22.0).child(capsule)
}

fn frame(mode: Mode) -> Runtime<Mode> {
    let mut rt = Runtime::new(mode, view, (68.0, 22.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn ring(rt: &Runtime<Mode>) -> Instance {
    *rt.instances.iter().find(|i| i.params[0] > 0.0).expect("a border instance")
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
fn border_gradient_is_a_gradient_ring_instance() {
    let i = ring(&frame(Mode::Gradient));
    assert_eq!(i.params[1], KIND_GRADIENT);
    assert_eq!(i.params[0], 1.0, "0.5 pt at 2x");
    assert_eq!(i.rect, [8.0, 8.0, 120.0, 28.0]);
    assert_eq!(i.radii, [14.0; 4]);
    assert_eq!(i.fill, [0.0; 4]);
    assert_eq!(i.border, TOP.premul(1.0), "top colour");
    assert_eq!(i.uv, BOTTOM.premul(1.0), "bottom colour");
}

#[test]
fn border_and_border_gradient_replace_each_other() {
    let i = ring(&frame(Mode::BorderThenGradient));
    assert_eq!((i.params[1], i.params[0], i.border, i.uv), (KIND_GRADIENT, 1.0, TOP.premul(1.0), BOTTOM.premul(1.0)));
    let i = ring(&frame(Mode::GradientThenBorder));
    assert_eq!((i.params[1], i.params[0], i.border, i.uv), (KIND_RECT, 2.0, RIM.premul(1.0), [0.0; 4]));
}

#[test]
fn border_gradient_springs_from_a_solid_border() {
    let mut rt = frame(Mode::RimBorder);
    rt.update(|s, _| *s = Mode::Gradient);
    rt.frame(0.03);
    let i = ring(&rt);
    assert_eq!(i.params[1], KIND_GRADIENT);
    let (rim, bottom) = (RIM.premul(1.0), BOTTOM.premul(1.0));
    assert!(i.uv[3] > rim[3] && i.uv[3] < bottom[3] && i.uv[2] < bottom[2], "bottom mid-spring: {:?}", i.uv);
    rt.frame(5.0);
    assert_eq!(ring(&rt).uv, bottom);
}

#[test]
fn solid_border_springs_from_a_gradient_without_moving_a_settled_bottom() {
    // The new solid colour equals the old bottom stop, so only the top edge has anywhere to go.
    let mut rt = frame(Mode::Gradient);
    rt.update(|s, _| *s = Mode::SolidBottom);
    let bottom = BOTTOM.premul(1.0);
    let mut kinds = Vec::new();
    for step in 1..=120 {
        rt.frame(1.0 / 60.0);
        let i = ring(&rt);
        let edge = if i.params[1] == KIND_GRADIENT { i.uv } else { i.border };
        assert_eq!(edge, bottom, "frame {step}: the bottom edge left `bottom` ({:?})", i);
        kinds.push(i.params[1]);
    }
    assert_eq!(kinds[0], KIND_GRADIENT, "the top edge springs rather than jumping");
    rt.frame(5.0);
    let i = ring(&rt);
    assert_eq!((i.params[1], i.border, i.uv), (KIND_RECT, bottom, [0.0; 4]));
}

#[test]
fn border_gradient_pixels_run_top_to_bottom_with_border_coverage() {
    let gpu = Gpu::headless();
    let px = |out: &[u8], x: usize, y: usize| -> [u8; 4] { out[(y * 136 + x) * 4..][..4].try_into().unwrap() };
    let out = pixels(&gpu, &mut frame(Mode::Gradient));
    // The 1 px ring's top row is y = 8 and its bottom row y = 35, at the capsule's centre column.
    let top = px(&out, 68, 8);
    let bottom = px(&out, 68, 35);
    assert!(top[0] > 240 && top[2] < 70 && top[3] > 250, "top edge is `top`: {top:?}");
    assert!(bottom[2] > 240 && bottom[0] < 70 && bottom[3] > 250, "bottom edge is `bottom`: {bottom:?}");
    assert_eq!(px(&out, 68, 20)[3], 0, "the ring leaves the inside clear");

    // A gradient with equal stops antialiases exactly like `border`, at 0.5 pt on 2x.
    let a = pixels(&gpu, &mut frame(Mode::RimGradient));
    let b = pixels(&gpu, &mut frame(Mode::RimBorder));
    assert!(a.chunks_exact(4).zip(b.chunks_exact(4)).all(|(a, b)| a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 1)));
    assert_ne!(px(&a, 68, 8), px(&a, 68, 20), "the rim shows over the fill");
}
