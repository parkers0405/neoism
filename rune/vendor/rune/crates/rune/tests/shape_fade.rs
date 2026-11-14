//! A fading canvas whose pieces overlap takes its opacity once, through a layer: where two
//! strokes cross or a polyline turns it is no darker than one stroke alone. At rest, and for a
//! shape of one piece, it paints straight to the window as before.

use rune::render::{Gpu, Instance, KIND_LAYER, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, Shape, canvas, el};

const SIZE: (u32, u32) = (80, 80);

type State = (Shape, f32);

/// The shape on a 40 pt canvas at an opacity, in a window of the same size, at 2x.
fn view(s: &State, _: &mut Cx<State>) -> El {
    el().size(40.0, 40.0).child(canvas(s.0.clone()).size(40.0, 40.0).opacity(s.1))
}

fn frame(shape: Shape, opacity: f32) -> Runtime<State> {
    let mut rt = Runtime::new((shape, opacity), view, (40.0, 40.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn layered(instances: &[Instance]) -> bool {
    instances.iter().any(|i| i.params[1] == KIND_LAYER)
}

fn pixels(gpu: &Gpu, rt: &mut Runtime<State>) -> Vec<u8> {
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, SIZE.0, SIZE.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, SIZE);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    target.read(&gpu.device)
}

fn px(out: &[u8], x: usize, y: usize) -> [u8; 4] {
    out[(y * SIZE.0 as usize + x) * 4..][..4].try_into().unwrap()
}

fn cross() -> Shape {
    Shape::new().stroke(vec![[0.0, 20.0], [40.0, 20.0]], 8.0, Color::WHITE).stroke(vec![[20.0, 0.0], [20.0, 40.0]], 8.0, Color::WHITE)
}

fn corner() -> Shape {
    Shape::new().stroke(vec![[4.0, 20.0], [20.0, 20.0], [20.0, 36.0]], 8.0, Color::WHITE)
}

#[test]
fn only_a_fading_shape_of_several_pieces_paints_through_a_layer() {
    for shape in [cross(), corner()] {
        assert!(!layered(&frame(shape.clone(), 1.0).instances), "at rest it paints straight to the window");
        assert!(layered(&frame(shape.clone(), 0.5).instances), "mid-fade it takes its opacity once");
        assert!(!layered(&frame(shape, 0.0).instances), "faded out");
    }
    let line = Shape::new().line([0.0, 20.0], [40.0, 20.0], 8.0, rune::LineCap::Round, Color::WHITE);
    assert!(!layered(&frame(line, 0.5).instances), "one segment can't overlap itself");
}

#[test]
fn where_faded_pieces_meet_is_no_darker_than_one() {
    let gpu = Gpu::headless();
    // (40, 40) px is where the cross's strokes and the corner's segments meet; (8, 40) and
    // (16, 40) are on one stroke alone.
    for (shape, alone) in [(cross(), (8, 40)), (corner(), (16, 40))] {
        let half = pixels(&gpu, &mut frame(shape.clone(), 0.5));
        assert_eq!(px(&half, 40, 40), px(&half, alone.0, alone.1), "the joint at 50%");
        assert!((120..=136).contains(&px(&half, 40, 40)[3]), "half covered: {:?}", px(&half, 40, 40));
        let full = pixels(&gpu, &mut frame(shape, 1.0));
        assert_eq!(px(&full, 40, 40), px(&full, alone.0, alone.1), "the joint at rest");
    }
}
