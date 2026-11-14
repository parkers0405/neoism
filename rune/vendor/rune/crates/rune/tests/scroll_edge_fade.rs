//! `El::scroll_edge_fade`: each edge fades only while content is hidden under it, at strength
//! hidden / 15 pt. `El::scroll_margin`: revealed rows stop clear of the fade.
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer, unpack_fade};
use rune::{Color, Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el};

const SCALE: f32 = 2.0;
const FADE: f32 = 24.0;

#[derive(Default)]
struct List { rows: usize, selected: usize, stick: bool, virtual_list: bool, margin: f32 }

fn view(s: &List, _: &mut Cx<List>) -> El {
    let mut stack = col().w_full();
    if s.virtual_list { stack = stack.growth(GrowthSpec::default()).virtualize(20.0); }
    for i in 0..s.rows {
        let fill = if i == s.selected { Color::WHITE } else { Color::WHITE.alpha(0.999) };
        stack = stack.child(el().key(format!("row{i}")).w_full().h(20.0).fill(fill));
    }
    col().w_full().h_full().child(col().key("scroll").w_full().h(200.0)
        .scroll(ScrollSpec { stick_bottom: s.stick })
        .scroll_edge_fade(FADE).scroll_margin(s.margin)
        .child(stack))
}

fn list(s: List) -> Runtime<List> {
    let mut rt = Runtime::new(s, view, (200.0, 300.0), Env::new(true, SCALE));
    rt.frame(0.0);
    rt
}

/// The (top, bottom) fade widths in device px and their ramp offsets on the rows' fill.
fn edges(rt: &Runtime<List>) -> ([f32; 2], [f32; 2]) {
    let row = rt.instances.iter().find(|i| i.fill[3] > 0.99).expect("a row");
    assert!(rt.instances.iter().all(|i| i.fade[3] == 0), "fade[3] stays reserved for the layer clip radius");
    let (w, o) = unpack_fade(row.fade);
    ([w[1], w[3]], [o[1], o[3]])
}

fn settle(rt: &mut Runtime<List>) {
    for _ in 0..120 { if !rt.frame(1.0 / 60.0) { return; } }
    panic!("never settled");
}

#[test]
fn each_edge_fades_only_once_content_is_hidden_under_it() {
    let mut rt = list(List { rows: 40, ..Default::default() });
    let (w, o) = edges(&rt);
    assert_eq!(w[0], 0.0, "nothing above: a hard top edge");
    assert_eq!((w[1], o[1]), (FADE * SCALE, 0.0), "rows below: the full bottom fade");

    // 7.5 pt hidden is half strength: the ramp starts halfway in and ends a full width in.
    rt.wheel(100.0, 100.0, -7.5);
    rt.frame(1.0 / 60.0);
    let (w, o) = edges(&rt);
    assert!((o[0] - 0.5).abs() < 0.01 && ((1.0 - o[0]) * w[0] - FADE * SCALE).abs() < 0.5, "half strength: {w:?} {o:?}");
    rt.wheel(100.0, 100.0, -7.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(edges(&rt), ([FADE * SCALE; 2], [0.0; 2]), "15 pt hidden: full strength");

    rt.wheel(100.0, 100.0, -10_000.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(edges(&rt).0, [FADE * SCALE, 0.0], "at the bottom: a hard bottom edge");
    assert!(!rt.frame(1.0 / 60.0), "no extra frames at rest");
}

#[test]
fn stuck_and_virtual_lists_follow_their_bottom() {
    let rt = list(List { rows: 40, stick: true, ..Default::default() });
    assert_eq!(edges(&rt).0, [FADE * SCALE, 0.0], "stick-to-bottom opens with a hard bottom edge");

    // A virtual list fades against its estimated extent before the tail is ever measured.
    let mut rt = list(List { rows: 400, virtual_list: true, ..Default::default() });
    assert_eq!(edges(&rt).0, [0.0, FADE * SCALE]);
    rt.update(|_, tx| tx.scroll_to_bottom_instant("scroll"));
    settle(&mut rt);
    assert_eq!(edges(&rt).0, [FADE * SCALE, 0.0]);
}

#[test]
fn keyboard_selection_stops_clear_of_the_fade() {
    let mut rt = list(List { rows: 40, margin: FADE, ..Default::default() });
    let viewport = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("scroll")).unwrap().rect;
    let row = |rt: &Runtime<List>, i: usize| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&*format!("row{i}"))).unwrap().rect;
    // What an arrow-key handler does: move the selection and reveal it.
    let step = |rt: &mut Runtime<List>, down: bool| {
        rt.update(|s, tx| {
            s.selected = if down { s.selected + 1 } else { s.selected - 1 };
            tx.scroll_into_view(format!("row{}", s.selected));
        });
        rt.frame(1.0 / 60.0);
    };
    for _ in 0..12 { step(&mut rt, true); }
    settle(&mut rt);
    let r = row(&rt, 12);
    assert!((viewport[1] + viewport[3] - (r[1] + r[3]) - FADE).abs() < 0.01, "row 12 stops {FADE} pt above the bottom: {r:?} in {viewport:?}");
    for _ in 0..8 { step(&mut rt, false); }
    settle(&mut rt);
    let r = row(&rt, 4);
    assert!((r[1] - viewport[1] - FADE).abs() < 0.01, "row 4 stops {FADE} pt below the top: {r:?}");
    for _ in 0..4 { step(&mut rt, false); }
    settle(&mut rt);
    assert_eq!(row(&rt, 0)[1], viewport[1], "the first row still lands flush, where nothing is hidden");
    assert_eq!(edges(&rt).0[0], 0.0, "so the top edge is hard and the selection unfaded");
}

#[test]
fn edge_pixels_fade_by_the_hidden_extent() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let size = ((200.0 * SCALE) as u32, (300.0 * SCALE) as u32);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    let mut alpha_at = |rt: &mut Runtime<List>, y: f32| {
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        target.copy(&mut encoder);
        gpu.queue.submit([encoder.finish()]);
        let px = target.read(&gpu.device);
        let row = (y * SCALE) as usize;
        px[(row * size.0 as usize + 40) * 4 + 3]
    };
    // The selected first row: fully opaque at the top edge until something scrolls under it.
    let mut rt = list(List { rows: 40, ..Default::default() });
    assert_eq!(alpha_at(&mut rt, 0.25), 255, "nothing hidden above: the top row is untouched");
    assert!(alpha_at(&mut rt, 199.75) < 10, "rows hidden below: the bottom edge is faded out");
    rt.wheel(100.0, 100.0, -7.5);
    rt.frame(1.0 / 60.0);
    let half = alpha_at(&mut rt, 0.25);
    assert!((110..=145).contains(&half), "half strength leaves about half alpha at the edge: {half}");
    assert_eq!(alpha_at(&mut rt, FADE + 0.5), 255, "and is done a full fade width in");
}
